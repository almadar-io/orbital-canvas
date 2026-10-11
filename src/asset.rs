//! URL-keyed asset byte cache — the Rust counterpart of `@almadar/ui`'s
//! `useImageCache` + the `fetch` half of `lib/atlasSlice.ts`.
//!
//! One static cache maps a URL to `InFlight | Ready(bytes) | Failed`. Fetches
//! run on spawned std threads so render paths never block: callers
//! `ensure_asset(url)` every frame and skip the drawable until
//! `asset_bytes(url)` returns `Some` — exactly the React "draw nothing until
//! loaded" contract (`DrawSprite.tsx:48-59`). Transports:
//!
//! - `http(s)://` — `ureq` (rustls), behind the `http` cargo feature
//!   (native-only; off on wasm32 where no fetch transport exists yet).
//! - `file://`, absolute, and `asset_base_url`-relative paths — `fs::read`.
//! - `data:` URLs (base64 or percent-encoded) — decoded inline, no thread, so self-contained
//!   fixtures resolve on the first frame.
//!
//! SVG vs raster is decided here, once, per asset ([`AssetKind`]); SVG
//! dimensions come from the SVG's intrinsic size ([`crate::svg`]).
//!
//! Image *dimensions* are decoded lazily (`image_info`) so mappers can emit
//! concrete source rects for whole-image sprites; full RGBA decode happens at
//! GPU-upload time in the renderer, never here.

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::{Arc, OnceLock, RwLock};

/// Log namespace — mirrors `@almadar/logger`'s `almadar:ui:game-canvas`.
const NS: &str = "almadar::game_renderer::asset";

/// How an asset's bytes are decoded. Decided once, at fetch time, from the
/// transport's declared content type (`image/svg+xml`) or the URL path's
/// extension (`.svg`) - never by sniffing the bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetKind {
    Raster,
    Svg,
}

impl AssetKind {
    fn classify(url: &str, content_type: Option<&str>) -> Self {
        let declared = content_type.map(|c| {
            c.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        });
        if declared.as_deref() == Some("image/svg+xml") {
            return AssetKind::Svg;
        }
        let path = url.split(['?', '#']).next().unwrap_or(url);
        if path.to_ascii_lowercase().ends_with(".svg") {
            AssetKind::Svg
        } else {
            AssetKind::Raster
        }
    }
}

enum State {
    /// Native only: the browser fetch fails at once (no transport yet, G-ORB-246).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    InFlight,
    Ready(Arc<[u8]>, AssetKind),
    /// Fetch failed — the error is logged at fetch time (see `ensure_asset`).
    Failed,
}

static CACHE: OnceLock<RwLock<HashMap<String, State>>> = OnceLock::new();
static DIMS: OnceLock<RwLock<HashMap<String, Option<(u32, u32)>>>> = OnceLock::new();
/// Monotonic counter bumped on every fetch completion (Ready or Failed).
/// Consumers that cache mapper output (e.g. the dioxus paint source) rebuild
/// when this changes — the Rust analogue of React's asset-store subscription
/// re-rendering the component once an image/atlas lands.
static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Current asset-cache generation; bumps on every fetch completion.
pub fn generation() -> u64 {
    GENERATION.load(std::sync::atomic::Ordering::Acquire)
}

fn cache() -> &'static RwLock<HashMap<String, State>> {
    CACHE.get_or_init(Default::default)
}

fn dims() -> &'static RwLock<HashMap<String, Option<(u32, u32)>>> {
    DIMS.get_or_init(Default::default)
}

/// Schedule a fetch for `url` if it is unknown to the cache. Idempotent and
/// cheap — intended to be called once per drawable per frame.
pub fn ensure_asset(url: &str) {
    if url.is_empty() {
        return;
    }
    {
        let map = cache().read().expect("asset cache poisoned");
        if map.contains_key(url) {
            return;
        }
    }
    let mut map = cache().write().expect("asset cache poisoned");
    if map.contains_key(url) {
        return;
    }
    if url.starts_with("data:") {
        let state = match decode_data_url(url) {
            Ok((bytes, content_type)) => {
                let kind = AssetKind::classify(url, content_type.as_deref());
                State::Ready(bytes.into(), kind)
            }
            Err(e) => {
                tracing::warn!(target: NS, error = %e, "data url rejected");
                State::Failed
            }
        };
        map.insert(url.to_string(), state);
        GENERATION.fetch_add(1, std::sync::atomic::Ordering::Release);
        return;
    }
    // The browser has no threads (`std::thread::spawn` panics on wasm32) and no transport here
    // yet (G-ORB-246): the fetch fails, declared and logged, instead of taking the page down.
    #[cfg(target_arch = "wasm32")]
    {
        tracing::warn!(target: NS, url = %url, "asset fetch failed: no browser transport for non-data URLs");
        map.insert(url.to_string(), State::Failed);
        GENERATION.fetch_add(1, std::sync::atomic::Ordering::Release);
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        map.insert(url.to_string(), State::InFlight);
        drop(map);
        spawn_fetch(url.to_string());
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_fetch(owned: String) {
    std::thread::spawn(move || {
        let outcome = fetch(&owned);
        let state = match outcome {
            Ok((bytes, content_type)) => {
                let kind = AssetKind::classify(&owned, content_type.as_deref());
                tracing::info!(target: NS, url = %owned, bytes = bytes.len(), ?kind, "asset fetched");
                State::Ready(bytes.into(), kind)
            }
            Err(e) => {
                tracing::warn!(target: NS, url = %owned, error = %e, "asset fetch failed");
                State::Failed
            }
        };
        cache()
            .write()
            .expect("asset cache poisoned")
            .insert(owned, state);
        GENERATION.fetch_add(1, std::sync::atomic::Ordering::Release);
    });
}

/// Assets whose fetch has not completed.
pub fn pending() -> usize {
    cache()
        .read()
        .expect("asset cache poisoned")
        .values()
        .filter(|s| matches!(s, State::InFlight))
        .count()
}

fn percent_decode(s: &str) -> Result<Vec<u8>, String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3).ok_or("truncated %-escape")?;
            out.push(u8::from_str_radix(hex, 16).map_err(|_| format!("bad %-escape `%{hex}`"))?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    Ok(out)
}

fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes().filter(|c| !c.is_ascii_whitespace()) {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            other => return Err(format!("bad base64 byte {other:#x}")),
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

/// `data:[<mime>][;base64],<payload>` -> bytes plus the declared mime type.
fn decode_data_url(url: &str) -> Result<Fetched, String> {
    let rest = url.strip_prefix("data:").ok_or("not a data url")?;
    let (meta, payload) = rest.split_once(',').ok_or("data url has no `,`")?;
    let mut parts = meta.split(';');
    let mime = parts.next().filter(|m| !m.is_empty()).map(str::to_string);
    let base64 = parts.any(|p| p == "base64");
    let bytes = if base64 {
        base64_decode(payload)?
    } else {
        percent_decode(payload)?
    };
    Ok((bytes, mime))
}

/// The fetched bytes for `url` once available (also schedules the fetch).
pub fn asset_bytes(url: &str) -> Option<Arc<[u8]>> {
    ensure_asset(url);
    let map = cache().read().expect("asset cache poisoned");
    match map.get(url) {
        Some(State::Ready(bytes, _)) => Some(bytes.clone()),
        _ => None,
    }
}

/// The decode kind of a fetched asset (`None` while in flight or failed).
pub fn asset_kind(url: &str) -> Option<AssetKind> {
    ensure_asset(url);
    match cache().read().expect("asset cache poisoned").get(url) {
        Some(State::Ready(_, kind)) => Some(*kind),
        _ => None,
    }
}

/// Decoded pixel dimensions for an image URL, decoded once on first request
/// and cached (decode failures are cached as `None`; an in-flight fetch is
/// NOT cached — the caller retries next frame, like React's image cache).
pub fn image_info(url: &str) -> Option<(u32, u32)> {
    ensure_asset(url);
    {
        let map = dims().read().expect("dims cache poisoned");
        if let Some(d) = map.get(url) {
            return *d;
        }
    }
    let bytes = asset_bytes(url)?; // in-flight → None, uncached
    let d = match asset_kind(url)? {
        AssetKind::Svg => crate::svg::intrinsic_size(url, &bytes),
        AssetKind::Raster => image::ImageReader::new(Cursor::new(&bytes))
            .with_guessed_format()
            .ok()
            .and_then(|r| r.into_dimensions().ok()),
    };
    dims()
        .write()
        .expect("dims cache poisoned")
        .insert(url.to_string(), d);
    d
}

/// Fetched bytes plus the transport's declared content type, when it has one.
type Fetched = (Vec<u8>, Option<String>);

/// Fetch raw bytes for a URL — the only place transports live.
#[cfg(not(target_arch = "wasm32"))]
fn fetch(url: &str) -> Result<Fetched, String> {
    if url.starts_with("http://") || url.starts_with("https://") {
        #[cfg(feature = "http")]
        {
            let response = ureq::get(url)
                .call()
                .map_err(|e| format!("http {url}: {e}"))?;
            let content_type = Some(response.content_type().to_string());
            let mut reader = response.into_reader();
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut reader, &mut buf)
                .map_err(|e| format!("http {url}: read: {e}"))?;
            return Ok((buf, content_type));
        }
        #[cfg(not(feature = "http"))]
        {
            return Err(format!(
                "http transport disabled (feature `http` off): {url}"
            ));
        }
    }
    let path = url.strip_prefix("file://").unwrap_or(url);
    std::fs::read(path)
        .map(|bytes| (bytes, None))
        .map_err(|e| format!("fs {path}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_bytes(url: &str) -> Option<Arc<[u8]>> {
        for _ in 0..200 {
            if let Some(b) = asset_bytes(url) {
                return Some(b);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        None
    }

    /// A file:// fixture round-trips through the cache: encode a 2×2 PNG with
    /// the `image` crate, fetch it as an asset, expect the bytes + dims back.
    #[test]
    fn file_asset_fetches_and_decodes_dimensions() {
        let dir = std::env::temp_dir().join("orbital-game-renderer-asset-test");
        std::fs::create_dir_all(&dir).expect("tempdir");
        let path = dir.join("fixture.png");
        let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]));
        img.save(&path).expect("write fixture png");
        let url = format!("file://{}", path.display());

        let bytes = wait_bytes(&url).expect("fixture must arrive within 2s");
        assert!(!bytes.is_empty(), "fetched bytes must be non-empty");

        let info = image_info(&url).expect("png dims must decode");
        assert_eq!(info, (2, 2), "fixture is 2x2");
    }

    const SVG_20X10: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="20" height="10" fill="#0000ff"/></svg>"##;

    #[test]
    fn svg_file_asset_is_classified_by_extension_and_reports_intrinsic_size() {
        let dir = std::env::temp_dir().join("orbital-game-renderer-asset-test-svg");
        std::fs::create_dir_all(&dir).expect("tempdir");
        let path = dir.join("sprite.svg");
        std::fs::write(&path, SVG_20X10).expect("write svg");
        let url = format!("file://{}", path.display());
        wait_bytes(&url).expect("svg fixture arrives");
        assert_eq!(asset_kind(&url), Some(AssetKind::Svg));
        assert_eq!(image_info(&url), Some((20, 10)));
    }

    /// Control: the same SVG bytes under a non-svg extension are NOT sniffed -
    /// the kind is declared by extension/content type, deterministically.
    #[test]
    fn svg_bytes_without_svg_extension_stay_raster() {
        let dir = std::env::temp_dir().join("orbital-game-renderer-asset-test-svg");
        std::fs::create_dir_all(&dir).expect("tempdir");
        let path = dir.join("sprite.bin");
        std::fs::write(&path, SVG_20X10).expect("write");
        let url = format!("file://{}", path.display());
        wait_bytes(&url).expect("arrives");
        assert_eq!(asset_kind(&url), Some(AssetKind::Raster));
        assert_eq!(
            image_info(&url),
            None,
            "image crate cannot decode svg bytes"
        );
    }

    #[test]
    fn data_urls_resolve_inline_in_base64_and_percent_forms_and_a_bad_one_fails() {
        let svg = "data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHdpZHRoPSIyMCIgaGVpZ2h0PSIxMCI+PC9zdmc+";
        let bytes = asset_bytes(svg).expect("a data url is ready on the first call, no thread");
        assert!(String::from_utf8_lossy(&bytes).starts_with("<svg"));
        assert_eq!(
            asset_kind(svg),
            Some(AssetKind::Svg),
            "the declared mime classifies it"
        );
        assert_eq!(image_info(svg), Some((20, 10)));
        let plain = "data:image/svg+xml;utf8,%3Csvg%20xmlns%3D%22http%3A%2F%2Fwww.w3.org%2F2000%2Fsvg%22%20width%3D%2230%22%20height%3D%2210%22%2F%3E";
        assert_eq!(image_info(plain), Some((30, 10)), "percent-encoded payload");
        assert_eq!(
            decode_data_url("data:text/plain;base64,aGk=").unwrap().0,
            b"hi"
        );
        assert!(decode_data_url("data:image/png;base64,@@@").is_err());
        assert!(decode_data_url("data:nocomma").is_err());
        let bad = "data:image/png;base64,@@@";
        ensure_asset(bad);
        assert!(
            asset_bytes(bad).is_none(),
            "control: a malformed data url never becomes bytes"
        );
    }

    #[test]
    fn kind_classification_rules() {
        use AssetKind::*;
        assert_eq!(AssetKind::classify("https://x/a.svg?v=2#f", None), Svg);
        assert_eq!(AssetKind::classify("https://x/a.SVG", None), Svg);
        assert_eq!(
            AssetKind::classify("https://x/render", Some("image/svg+xml; charset=utf-8")),
            Svg
        );
        assert_eq!(AssetKind::classify("https://x/a.png", None), Raster);
        assert_eq!(
            AssetKind::classify("https://x/a.png", Some("image/png")),
            Raster
        );
        assert_eq!(AssetKind::classify("https://x/svg", None), Raster);
    }

    /// ensure_asset is idempotent: a second call does not re-schedule or panic.
    #[test]
    fn ensure_asset_is_idempotent() {
        let url = "file:///nonexistent/orbital-game-renderer/nope.png";
        ensure_asset(url);
        ensure_asset(url);
        for _ in 0..200 {
            let map = cache().read().expect("poisoned");
            if matches!(map.get(url), Some(State::Failed)) {
                return;
            }
            drop(map);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("missing file must resolve to Failed, not hang InFlight");
    }
}
