//! SVG sprite decode: intrinsic size + rasterization at an exact pixel size.
//!
//! Raster sprites (PNG/JPEG) are decoded by the `image` crate at GPU-upload
//! time; an SVG has no pixel size of its own, so the renderer picks one: the
//! sprite's on-screen size x the surface density, bucketed ([`raster_scale`])
//! so a zoom animation does not mint a texture per frame. Parsed trees are
//! cached per URL, rasterized textures per (URL, size) by the renderer.

use resvg::{tiny_skia, usvg};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

const NS: &str = "almadar::game_renderer::svg";

/// A rasterized SVG: straight (non-premultiplied) RGBA8, like `image`'s decode.
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

static TREES: OnceLock<RwLock<HashMap<String, Option<Arc<usvg::Tree>>>>> = OnceLock::new();
static PARSES: OnceLock<RwLock<HashMap<String, usize>>> = OnceLock::new();

fn trees() -> &'static RwLock<HashMap<String, Option<Arc<usvg::Tree>>>> {
    TREES.get_or_init(Default::default)
}

/// Number of parses performed for `url` (cache-hit observability).
pub fn parse_count(url: &str) -> usize {
    PARSES
        .get_or_init(Default::default)
        .read()
        .expect("svg parse counter poisoned")
        .get(url)
        .copied()
        .unwrap_or(0)
}

fn parse(url: &str, bytes: &[u8]) -> Result<usvg::Tree, String> {
    *PARSES
        .get_or_init(Default::default)
        .write()
        .expect("svg parse counter poisoned")
        .entry(url.to_string())
        .or_default() += 1;
    usvg::Tree::from_data(bytes, &usvg::Options::default()).map_err(|e| e.to_string())
}

/// The parsed tree for `url`, parsed once (failures are cached as `None`).
fn tree_for(url: &str, bytes: &[u8]) -> Option<Arc<usvg::Tree>> {
    if let Some(t) = trees().read().expect("svg tree cache poisoned").get(url) {
        return t.clone();
    }
    let parsed = match parse(url, bytes) {
        Ok(t) => Some(Arc::new(t)),
        Err(e) => {
            tracing::warn!(target: NS, url, error = %e, "svg parse failed");
            None
        }
    };
    trees()
        .write()
        .expect("svg tree cache poisoned")
        .insert(url.to_string(), parsed.clone());
    parsed
}

/// Intrinsic size in user units, rounded up (>= 1) - the "image dimensions"
/// the mappers use for whole-image sprite source rects.
pub fn intrinsic_size(url: &str, bytes: &[u8]) -> Option<(u32, u32)> {
    let tree = tree_for(url, bytes)?;
    let size = tree.size();
    Some((
        size.width().ceil().max(1.0) as u32,
        size.height().ceil().max(1.0) as u32,
    ))
}

/// Raster scale (texels per intrinsic unit) for a sprite drawn at `target_scale`
/// device pixels per intrinsic unit: rounded UP to a quarter-octave bucket
/// (2^(k/4)) so the texture is never blurrier than the screen needs and
/// continuous zoom reuses a bounded set of textures.
pub fn raster_scale(target_scale: f32) -> f32 {
    if !target_scale.is_finite() || target_scale <= 0.0 {
        return 1.0;
    }
    2f32.powf((target_scale.log2() * 4.0).ceil() / 4.0)
}

/// Texture size for an `intrinsic` image at `scale`, clamped to `max_dim`.
pub fn raster_size(intrinsic: (u32, u32), scale: f32, max_dim: u32) -> (u32, u32) {
    let w = (intrinsic.0 as f32 * scale).ceil().max(1.0);
    let h = (intrinsic.1 as f32 * scale).ceil().max(1.0);
    let k = (max_dim as f32 / w).min(max_dim as f32 / h).min(1.0);
    (
        ((w * k).floor() as u32).max(1),
        ((h * k).floor() as u32).max(1),
    )
}

/// Rasterize the SVG for `url` into exactly `width` x `height` pixels.
pub fn rasterize(url: &str, bytes: &[u8], width: u32, height: u32) -> Option<Raster> {
    let tree = tree_for(url, bytes)?;
    let mut pixmap = tiny_skia::Pixmap::new(width, height)?;
    let size = tree.size();
    let transform = tiny_skia::Transform::from_scale(
        width as f32 / size.width(),
        height as f32 / size.height(),
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    let mut rgba = Vec::with_capacity(pixmap.data().len());
    for px in pixmap.pixels() {
        let c = px.demultiply();
        rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    Some(Raster {
        width,
        height,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED_32X16: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="16" viewBox="0 0 32 16"><rect width="32" height="16" fill="#ff0000"/></svg>"##;
    const HALF_ALPHA: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="4" height="4" fill="#00ff00" fill-opacity="0.5"/></svg>"##;

    #[test]
    fn intrinsic_size_reads_the_svg_dimensions() {
        assert_eq!(
            intrinsic_size("t://red", RED_32X16.as_bytes()),
            Some((32, 16))
        );
    }

    #[test]
    fn rasterizes_at_the_requested_size_not_the_intrinsic_one() {
        let r = rasterize("t://red-scaled", RED_32X16.as_bytes(), 96, 48).expect("raster");
        assert_eq!((r.width, r.height), (96, 48));
        assert_eq!(r.rgba.len(), 96 * 48 * 4);
        assert_eq!(
            &r.rgba[(24 * 96 + 48) * 4..][..4],
            &[255, 0, 0, 255],
            "center is opaque red"
        );
    }

    #[test]
    fn alpha_is_straight_not_premultiplied() {
        let r = rasterize("t://half", HALF_ALPHA.as_bytes(), 4, 4).expect("raster");
        let px = &r.rgba[..4];
        assert!(px[3] >= 126 && px[3] <= 129, "alpha ~0.5, got {}", px[3]);
        assert!(
            px[1] >= 253,
            "green stays full-strength after demultiply, got {}",
            px[1]
        );
    }

    #[test]
    fn invalid_svg_is_none_and_cached_as_failed() {
        assert!(intrinsic_size("t://bad", b"not an svg").is_none());
        let before = parse_count("t://bad");
        assert!(intrinsic_size("t://bad", b"not an svg").is_none());
        assert_eq!(
            parse_count("t://bad"),
            before,
            "a failed parse is cached, not retried per frame"
        );
    }

    #[test]
    fn parse_happens_once_per_url() {
        intrinsic_size("t://once", RED_32X16.as_bytes());
        let after_first = parse_count("t://once");
        rasterize("t://once", RED_32X16.as_bytes(), 8, 4);
        rasterize("t://once", RED_32X16.as_bytes(), 64, 32);
        assert_eq!(parse_count("t://once"), after_first);
        assert_eq!(after_first, 1);
    }

    #[test]
    fn raster_scale_buckets_up_and_never_below_target() {
        for t in [0.3f32, 0.5, 0.97, 1.0, 1.01, 2.4, 3.0, 7.7] {
            let s = raster_scale(t);
            assert!(s >= t - 1e-4 && s < t * 1.2 + 1e-4, "{t} -> {s}");
        }
        assert_eq!(raster_scale(1.0), 1.0, "control: exact powers stay put");
        assert_eq!(
            raster_scale(raster_scale(2.4)),
            raster_scale(2.4),
            "idempotent"
        );
        assert_eq!(raster_scale(0.0), 1.0);
    }

    #[test]
    fn raster_size_clamps_to_the_device_limit_keeping_aspect() {
        assert_eq!(raster_size((32, 16), 3.0, 8192), (96, 48));
        let (w, h) = raster_size((4000, 2000), 4.0, 8192);
        assert!(w <= 8192 && h <= 8192);
        assert!((w as f32 / h as f32 - 2.0).abs() < 0.01);
    }
}
