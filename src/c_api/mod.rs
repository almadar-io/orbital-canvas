//! C ABI exposed to Android (JNI) and iOS (Swift/Objective-C) shells.

use crate::packet::GameRenderPacket;
use crate::Canvas;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;

thread_local! {
    static LAST_ERROR: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

fn set_last_error(msg: &str) {
    LAST_ERROR.with(|e| *e.borrow_mut() = msg.to_owned());
}

fn get_last_error() -> String {
    LAST_ERROR.with(|e| e.borrow().clone())
}

/// Run an entry point body so a panic (e.g. a wgpu validation error) never unwinds across the
/// C boundary: it is reported through `orbital_canvas_last_error` and `on_panic` is returned.
pub(crate) fn guarded<T>(entry: &str, on_panic: T, body: impl FnOnce() -> T) -> T {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(value) => value,
        Err(payload) => {
            let detail = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "non-string panic payload".to_owned());
            tracing::error!(entry, panic = %detail, "game renderer entry point panicked");
            set_last_error(&format!("{entry}: panic: {detail}"));
            on_panic
        }
    }
}

/// Create a renderer from a raw platform surface.
///
/// - Desktop: `raw_surface` may be null; a headless renderer is created.
/// - Android: pass a valid `ANativeWindow*` as `raw_surface`.
/// - iOS: pass a valid `CAMetalLayer*` as `raw_surface`.
///
/// Returns null on failure (reason in `orbital_canvas_last_error`), including a panic.
///
/// # Safety
/// `raw_surface` must be a valid pointer of the platform-specific type, or null.
/// `resource_base_url` must be a valid null-terminated UTF-8 string, or null.
#[no_mangle]
pub unsafe extern "C" fn orbital_canvas_init(
    raw_surface: *mut c_void,
    _width: u32,
    _height: u32,
    resource_base_url: *const c_char,
) -> *mut Canvas {
    guarded("init", ptr::null_mut(), || {
        let _url = if resource_base_url.is_null() {
            ""
        } else {
            CStr::from_ptr(resource_base_url).to_str().unwrap_or("")
        };

        let result = if raw_surface.is_null() {
            Canvas::new_headless()
        } else {
            #[cfg(target_os = "android")]
            {
                crate::platform::android::init_from_native_window(
                    raw_surface,
                    _width,
                    _height,
                    _url,
                )
            }
            #[cfg(target_os = "ios")]
            {
                crate::platform::ios::init_from_metal_layer(raw_surface, _width, _height, _url)
            }
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            {
                // Non-mobile targets ignore the raw surface and create a headless renderer.
                Canvas::new_headless()
            }
        };

        match result {
            Ok(renderer) => Box::into_raw(Box::new(renderer)),
            Err(err) => {
                let msg = format!("init: {err}");
                tracing::error!(error = %err, "failed to initialize game renderer");
                set_last_error(&msg);
                ptr::null_mut()
            }
        }
    })
}

/// Destroy a renderer previously created with `orbital_canvas_init`.
///
/// # Safety
/// `renderer` must be a valid pointer returned by `orbital_canvas_init`,
/// and must not be used after this call.
#[no_mangle]
pub unsafe extern "C" fn orbital_canvas_destroy(renderer: *mut Canvas) {
    if !renderer.is_null() {
        guarded("destroy", (), || {
            let _ = Box::from_raw(renderer);
        });
    }
}

/// Submit a JSON-encoded `GameRenderPacket`.
///
/// # Safety
/// `renderer` must be a valid pointer. `json_packet` must be a valid
/// null-terminated UTF-8 string.
#[no_mangle]
pub unsafe extern "C" fn orbital_canvas_submit(renderer: *mut Canvas, json_packet: *const c_char) {
    if renderer.is_null() || json_packet.is_null() {
        return;
    }
    guarded("submit", (), || {
        let json = match CStr::from_ptr(json_packet).to_str() {
            Ok(s) => s,
            Err(err) => {
                tracing::error!(error = %err, "submit packet is not valid UTF-8");
                return;
            }
        };
        let packet = match orbital_core_base::json_value::from_json_str::<GameRenderPacket>(json) {
            Ok(p) => p,
            Err(err) => {
                tracing::error!(error = %err, "failed to parse submit packet");
                return;
            }
        };
        (*renderer).submit(packet);
    })
}

/// Submit a `canvas` / `canvas-2d` render-ui node (JSON) as the scene. The
/// renderer maps it to a display-list packet in Rust
/// (`mappers::game_canvas_2d::from_canvas_node_in_viewport` — the same walk the desktop
/// shells run, laid out in the surface size like the web `Canvas2D`), so hosts never
/// interpret drawables themselves.
///
/// Returns 1 when the scene was accepted; 0 on a null argument, invalid
/// UTF-8/JSON, a scene the native painter refuses (e.g. `mode: "3d"`), or a panic —
/// the reason is available from `orbital_canvas_last_error`.
///
/// # Safety
/// `renderer` must be a valid pointer. `scene_json` must be a valid
/// null-terminated UTF-8 string.
#[no_mangle]
pub unsafe extern "C" fn orbital_canvas_set_scene(
    renderer: *mut Canvas,
    scene_json: *const c_char,
) -> c_int {
    if renderer.is_null() || scene_json.is_null() {
        return 0;
    }
    guarded("set_scene", 0, || {
        let json = match CStr::from_ptr(scene_json).to_str() {
            Ok(s) => s,
            Err(err) => {
                set_last_error(&format!("set_scene: scene is not valid UTF-8: {err}"));
                return 0;
            }
        };
        let node = match orbital_core_base::json_value::from_json_str::<serde_json::Value>(json) {
            Ok(v) => v,
            Err(err) => {
                set_last_error(&format!("set_scene: scene is not valid JSON: {err}"));
                return 0;
            }
        };
        match (*renderer).set_scene(node) {
            Ok(()) => 1,
            Err(err) => {
                set_last_error(&format!("set_scene: {err}"));
                0
            }
        }
    })
}

/// Render one frame.
///
/// # Safety
/// `renderer` must be a valid pointer.
#[no_mangle]
pub unsafe extern "C" fn orbital_canvas_render(renderer: *mut Canvas) {
    if renderer.is_null() {
        return;
    }
    guarded("render", (), || {
        if let Err(err) = (*renderer).render() {
            let msg = format!("render: {err}");
            tracing::error!(error = %err, "render failed");
            set_last_error(&msg);
        }
    })
}

/// Resize the render surface.
///
/// # Safety
/// `renderer` must be a valid pointer.
#[no_mangle]
pub unsafe extern "C" fn orbital_canvas_resize(renderer: *mut Canvas, width: u32, height: u32) {
    if renderer.is_null() {
        return;
    }
    guarded("resize", (), || {
        if let Err(err) = (*renderer).resize(width, height) {
            let msg = format!("resize: {err}");
            tracing::error!(error = %err, "resize failed");
            set_last_error(&msg);
        }
    })
}

/// Set the display density (device pixels per logical point: iOS `contentScaleFactor`,
/// Android `displayMetrics.density`). `resize`/`init` dimensions stay in physical pixels;
/// the scene is laid out in logical points (`physical / scale`) and drawn at physical
/// resolution, matching the web `Canvas2D` (CSS-pixel layout, canvas at size x dpr).
/// Hosts that never call this get 1.0. Input coordinates are in logical points.
///
/// Returns 1 when applied; 0 on a null renderer or a non-finite / non-positive scale
/// (reason in `orbital_canvas_last_error`; the previous scale stays).
///
/// # Safety
/// `renderer` must be a valid pointer.
#[no_mangle]
pub unsafe extern "C" fn orbital_canvas_set_scale(renderer: *mut Canvas, scale: f32) -> c_int {
    if renderer.is_null() {
        return 0;
    }
    guarded("set_scale", 0, || match (*renderer).set_scale(scale) {
        Ok(()) => 1,
        Err(err) => {
            set_last_error(&format!("set_scale: {err}"));
            0
        }
    })
}

/// Give the renderer the host's frame clock in milliseconds (`CACurrentMediaTime`,
/// `withFrameNanos`...) before `orbital_canvas_render`. The first value is scene time zero,
/// so an animated scene draws its first frame as the web does at mount.
///
/// # Safety
/// `renderer` must be a valid pointer.
#[no_mangle]
pub unsafe extern "C" fn orbital_canvas_set_clock(renderer: *mut Canvas, ms: f64) {
    if renderer.is_null() || !ms.is_finite() {
        return;
    }
    guarded("set_clock", (), || (*renderer).set_clock(ms))
}

/// Give world FX its epoch paint clock (`bornAt` is an epoch timestamp).
///
/// # Safety
/// `renderer` must be a valid pointer.
#[no_mangle]
pub unsafe extern "C" fn orbital_canvas_set_epoch_clock(renderer: *mut Canvas, epoch_ms: f64) {
    if renderer.is_null() || !epoch_ms.is_finite() || epoch_ms <= 0.0 {
        return;
    }
    guarded("set_epoch_clock", (), || {
        (*renderer).set_epoch_clock(epoch_ms)
    })
}

/// Allow (non-zero) or forbid (0) motion: the host passes the OS reduced-motion setting here.
/// Forbidden, animated scenes draw their rest frame.
///
/// # Safety
/// `renderer` must be a valid pointer.
#[no_mangle]
pub unsafe extern "C" fn orbital_canvas_set_motion(renderer: *mut Canvas, enabled: c_int) {
    if renderer.is_null() {
        return;
    }
    guarded("set_motion", (), || (*renderer).set_motion(enabled != 0))
}

/// 1 while the host should keep drawing frames (the scene animates, or its assets are still
/// arriving), 0 when a static scene has settled and the frame loop can stop. Ask after
/// `orbital_canvas_set_scene` and after each `orbital_canvas_render`.
///
/// # Safety
/// `renderer` must be a valid pointer.
#[no_mangle]
pub unsafe extern "C" fn orbital_canvas_wants_frames(renderer: *mut Canvas) -> c_int {
    if renderer.is_null() {
        return 0;
    }
    guarded("wants_frames", 0, || {
        c_int::from((*renderer).wants_frames())
    })
}

/// Forward an input event. Returns 1 if consumed, 0 otherwise.
///
/// `x`/`y` are logical points in the scene's viewport (physical pixels / density). A
/// `touch-down` then `touch-up` within `TAP_SLOP` is a tap; a tap on an interactive scene
/// queues the scene's events, which the host collects with `orbital_canvas_take_events`.
///
/// # Safety
/// `renderer`, `event_type`, and `key` must be valid pointers (or null for `key`).
#[no_mangle]
pub unsafe extern "C" fn orbital_canvas_input(
    renderer: *mut Canvas,
    event_type: *const c_char,
    x: f32,
    y: f32,
    button: c_int,
    key: *const c_char,
) -> c_int {
    if renderer.is_null() || event_type.is_null() {
        return 0;
    }
    guarded("input", 0, || {
        let event = CStr::from_ptr(event_type).to_str().unwrap_or("");
        let key_opt = if key.is_null() {
            None
        } else {
            CStr::from_ptr(key).to_str().ok()
        };
        let consumed = (*renderer).input(event, x, y, button, key_opt);
        if consumed {
            1
        } else {
            0
        }
    })
}

/// The scene's own size in logical units, written to `*out_w`/`*out_h`: lets a host size its
/// surface to the scene's aspect ratio before a renderer exists. Returns 1 on success, 0 on a
/// null argument, invalid JSON or a scene the native painter refuses (reason via
/// `orbital_canvas_last_error`).
///
/// # Safety
/// `scene_json` must be a valid null-terminated UTF-8 string; `out_w` and `out_h` valid pointers.
#[no_mangle]
pub unsafe extern "C" fn orbital_canvas_scene_size(
    scene_json: *const c_char,
    out_w: *mut f32,
    out_h: *mut f32,
) -> c_int {
    if scene_json.is_null() || out_w.is_null() || out_h.is_null() {
        return 0;
    }
    guarded("scene_size", 0, || {
        let node = match CStr::from_ptr(scene_json)
            .to_str()
            .map_err(|e| e.to_string())
            .and_then(|s| {
                orbital_core_base::json_value::from_json_str::<serde_json::Value>(s)
                    .map_err(|e| e.to_string())
            }) {
            Ok(v) => v,
            Err(err) => {
                set_last_error(&format!("scene_size: scene is not valid UTF-8 JSON: {err}"));
                return 0;
            }
        };
        match crate::mappers::scene::scene_size(&node) {
            Ok((w, h)) => {
                *out_w = w;
                *out_h = h;
                1
            }
            Err(err) => {
                set_last_error(&format!("scene_size: {err}"));
                0
            }
        }
    })
}

/// Take the scene events that taps queued since the last call, as a newly allocated JSON
/// array of `{"event": "<name>", "payload": {<callback arg>: ...}}` (`[]` when none, null on a null renderer
/// or panic). The host sends each event to its runtime with the payload as the event payload.
/// Free the result with `orbital_canvas_free_string`.
///
/// # Safety
/// `renderer` must be a valid pointer.
#[no_mangle]
pub unsafe extern "C" fn orbital_canvas_take_events(renderer: *mut Canvas) -> *mut c_char {
    if renderer.is_null() {
        return ptr::null_mut();
    }
    guarded("take_events", ptr::null_mut(), || {
        let events: Vec<serde_json::Value> = (*renderer)
            .take_scene_events()
            .into_iter()
            .map(|e| serde_json::json!({ "event": e.event, "payload": e.delivered() }))
            .collect();
        match CString::new(serde_json::Value::Array(events).to_string()) {
            Ok(s) => s.into_raw(),
            Err(_) => ptr::null_mut(),
        }
    })
}

/// A pinch from the host's recognizer: zoom a pannable scene (a graph) by `factor` about the
/// focus point in logical points. A double tap is the `double-tap` input type.
///
/// # Safety
/// `renderer` must be a valid pointer.
#[no_mangle]
pub unsafe extern "C" fn orbital_canvas_zoom(renderer: *mut Canvas, factor: f32, x: f32, y: f32) {
    if renderer.is_null() {
        return;
    }
    guarded("zoom", (), || {
        (*renderer).zoom(factor, x, y);
    });
}

/// A view button on a pannable scene: `zoom-in`, `zoom-out` or `reset` (the web graph header).
/// Returns 1, or 0 with `last_error` set for an unknown command.
///
/// # Safety
/// `renderer` must be a valid pointer and `command` a valid C string.
#[no_mangle]
pub unsafe extern "C" fn orbital_canvas_view_command(
    renderer: *mut Canvas,
    command: *const c_char,
) -> c_int {
    if renderer.is_null() || command.is_null() {
        set_last_error("view_command: null argument");
        return 0;
    }
    guarded("view_command", 0, || {
        let command = CStr::from_ptr(command).to_string_lossy();
        match (*renderer).view_command(&command) {
            Ok(()) => 1,
            Err(err) => {
                set_last_error(&format!("view_command: {err}"));
                0
            }
        }
    })
}

/// Moves whenever an asset (sprite sheet, atlas, image) finishes loading or fails. A host that
/// sized a canvas from [`orbital_canvas_scene_size`] measures again when it changes: a
/// sprite with no authored size takes its aspect from the loaded image.
#[no_mangle]
pub extern "C" fn orbital_canvas_asset_generation() -> u64 {
    crate::asset::generation()
}

/// Scripts painted text needed and no face covers, each returned once, as a JSON array of
/// `{"script": <ISO 15924 short name>, "sample": <a char of it>}`. The host answers with [`orbital_canvas_add_font`] from its
/// system fonts and draws a frame. Free the result with `orbital_canvas_free_string`.
#[no_mangle]
pub extern "C" fn orbital_canvas_take_missing_scripts() -> *mut c_char {
    guarded("take_missing_scripts", ptr::null_mut(), || {
        let scripts = serde_json::json!(crate::vector::take_missing_scripts());
        CString::new(scripts.to_string()).map_or(ptr::null_mut(), CString::into_raw)
    })
}

/// Register the platform's font for `script` (ISO 15924 short name) from `len` bytes of a
/// TrueType/OpenType font (face `index` of a collection). Returns 1, or 0 with `last_error` set.
///
/// # Safety
/// `script` must be a valid C string and `bytes` valid for `len` bytes.
#[no_mangle]
pub unsafe extern "C" fn orbital_canvas_add_font(
    script: *const c_char,
    bytes: *const u8,
    len: usize,
    index: u32,
) -> c_int {
    if script.is_null() || bytes.is_null() {
        set_last_error("add_font: null argument");
        return 0;
    }
    guarded("add_font", 0, || {
        let script = CStr::from_ptr(script).to_string_lossy();
        let data = std::slice::from_raw_parts(bytes, len).to_vec();
        match crate::vector::add_font_face(&script, data, index) {
            Ok(()) => 1,
            Err(err) => {
                set_last_error(&format!("add_font: {err}"));
                0
            }
        }
    })
}

/// Return the last error message as a newly allocated C string.
///
/// Returns "" when no error has occurred since the last successful call.
/// The caller is responsible for freeing the returned pointer with
/// `orbital_canvas_free_string`.
///
/// # Safety
/// `renderer` may be null (errors from failed init are also captured).
#[no_mangle]
pub unsafe extern "C" fn orbital_canvas_last_error(_renderer: *mut Canvas) -> *mut c_char {
    guarded("last_error", ptr::null_mut(), || {
        let msg = get_last_error();
        match CString::new(msg) {
            Ok(s) => s.into_raw(),
            Err(_) => ptr::null_mut(),
        }
    })
}

/// Render the current packet to an RGBA pixel buffer (headless path).
///
/// On success, writes the pixel buffer address into `*out_pixels` and its length in bytes
/// into `*out_len`, then returns 1.  On failure (no adapter, no packet, panic), returns 0 and
/// leaves `*out_pixels` / `*out_len` unchanged.
///
/// The caller must free the buffer with `orbital_canvas_free_rgba`.
///
/// # Safety
/// `renderer`, `out_pixels`, and `out_len` must be valid non-null pointers.
#[no_mangle]
pub unsafe extern "C" fn orbital_canvas_render_to_rgba(
    renderer: *mut crate::Canvas,
    out_pixels: *mut *mut u8,
    out_len: *mut usize,
) -> c_int {
    if renderer.is_null() || out_pixels.is_null() || out_len.is_null() {
        return 0;
    }
    guarded("render_to_rgba", 0, || {
        let r = &mut *renderer;
        let packet = match r.packet.clone() {
            Some(p) => p,
            None => {
                tracing::warn!("render_to_rgba: no packet submitted");
                return 0;
            }
        };
        match r.render_packet_to_rgba(&packet) {
            Ok(buf) => {
                let mut buf = buf.into_boxed_slice();
                *out_len = buf.len();
                *out_pixels = buf.as_mut_ptr();
                std::mem::forget(buf);
                1
            }
            Err(err) => {
                let msg = format!("render_to_rgba: {err}");
                tracing::error!(error = %err, "render_to_rgba failed");
                set_last_error(&msg);
                0
            }
        }
    })
}

/// Map a scene and render it once, headless, into an RGBA buffer of `width` x `height` physical
/// pixels at display density `scale`: the one-shot raster twin of the surface path (same mapper,
/// same renderer), for hosts that cannot hold a GPU surface (JVM snapshot tests).
/// Returns 1 and writes `*out_pixels`/`*out_len` (free with `orbital_canvas_free_rgba`);
/// 0 on a null argument, an invalid or refused scene, or no GPU adapter (reason via last error).
///
/// # Safety
/// `scene_json` must be a valid null-terminated UTF-8 string; `out_pixels`/`out_len` valid pointers.
#[no_mangle]
pub unsafe extern "C" fn orbital_canvas_render_scene_rgba(
    scene_json: *const c_char,
    width: u32,
    height: u32,
    scale: f32,
    out_pixels: *mut *mut u8,
    out_len: *mut usize,
) -> c_int {
    if scene_json.is_null() || out_pixels.is_null() || out_len.is_null() {
        return 0;
    }
    guarded("render_scene_rgba", 0, || {
        let node = match CStr::from_ptr(scene_json)
            .to_str()
            .map_err(|e| e.to_string())
            .and_then(|s| {
                orbital_core_base::json_value::from_json_str::<serde_json::Value>(s)
                    .map_err(|e| e.to_string())
            }) {
            Ok(v) => v,
            Err(err) => {
                set_last_error(&format!(
                    "render_scene_rgba: scene is not valid UTF-8 JSON: {err}"
                ));
                return 0;
            }
        };
        let mut r = match Canvas::new_headless() {
            Ok(r) => r,
            Err(err) => {
                set_last_error(&format!("render_scene_rgba: {err}"));
                return 0;
            }
        };
        if let Err(err) = r.set_scale(scale) {
            set_last_error(&format!("render_scene_rgba: {err}"));
            return 0;
        }
        r.set_viewport(width, height);
        if let Err(err) = r.set_scene(node) {
            set_last_error(&format!("render_scene_rgba: {err}"));
            return 0;
        }
        r.settle_assets(std::time::Duration::from_secs(10));
        let Some(packet) = r.frame_packet() else {
            return 0;
        };
        match r.render_packet_to_rgba(&packet) {
            Ok(buf) => {
                let mut buf = buf.into_boxed_slice();
                *out_len = buf.len();
                *out_pixels = buf.as_mut_ptr();
                std::mem::forget(buf);
                1
            }
            Err(err) => {
                set_last_error(&format!("render_scene_rgba: {err}"));
                0
            }
        }
    })
}

/// Decode a `data:` URL (base64 or percent-encoded) the way the canvas asset loader does.
/// Returns 1 and writes the bytes (free with `orbital_canvas_free_rgba`; null with length 0 when
/// empty) and the declared mime type (free with `orbital_canvas_free_string`; null when none);
/// 0 on a null argument or a malformed URL (reason via last error).
///
/// # Safety
/// `url` must be a valid null-terminated UTF-8 string and every out pointer valid.
#[no_mangle]
pub unsafe extern "C" fn orbital_canvas_decode_data_url(
    url: *const c_char,
    out_bytes: *mut *mut u8,
    out_len: *mut usize,
    out_mime: *mut *mut c_char,
) -> c_int {
    if url.is_null() || out_bytes.is_null() || out_len.is_null() || out_mime.is_null() {
        return 0;
    }
    guarded("decode_data_url", 0, || {
        let Ok(url) = CStr::from_ptr(url).to_str() else {
            set_last_error("decode_data_url: url is not valid UTF-8");
            return 0;
        };
        match crate::asset::decode_data_url(url) {
            Ok((bytes, mime)) => {
                *out_mime = mime
                    .and_then(|m| CString::new(m).ok())
                    .map_or(ptr::null_mut(), CString::into_raw);
                if bytes.is_empty() {
                    *out_bytes = ptr::null_mut();
                    *out_len = 0;
                } else {
                    let mut buf = bytes.into_boxed_slice();
                    *out_len = buf.len();
                    *out_bytes = buf.as_mut_ptr();
                    std::mem::forget(buf);
                }
                1
            }
            Err(err) => {
                set_last_error(&format!("decode_data_url: {err}"));
                0
            }
        }
    })
}

/// 1 when an image fetched from `url` with the transport's `content_type` (null when none was
/// declared) decodes as SVG, else 0: the same rule the canvas asset loader applies.
///
/// # Safety
/// `url` must be a valid null-terminated UTF-8 string; `content_type` such a string or null.
#[no_mangle]
pub unsafe extern "C" fn orbital_canvas_asset_is_svg(
    url: *const c_char,
    content_type: *const c_char,
) -> c_int {
    if url.is_null() {
        return 0;
    }
    guarded("asset_is_svg", 0, || {
        let url = CStr::from_ptr(url).to_str().unwrap_or("");
        let declared = if content_type.is_null() {
            None
        } else {
            CStr::from_ptr(content_type).to_str().ok()
        };
        c_int::from(
            crate::asset::AssetKind::classify(url, declared) == crate::asset::AssetKind::Svg,
        )
    })
}

/// The intrinsic size (SVG user units, rounded up) of the SVG in `bytes`, cached under `key`.
/// Returns 1 and writes the size; 0 on a null argument or an SVG that does not parse.
///
/// # Safety
/// `key` must be a valid null-terminated UTF-8 string, `bytes` valid for `len` bytes, and the out
/// pointers valid.
#[no_mangle]
pub unsafe extern "C" fn orbital_canvas_svg_size(
    key: *const c_char,
    bytes: *const u8,
    len: usize,
    out_width: *mut u32,
    out_height: *mut u32,
) -> c_int {
    if key.is_null() || bytes.is_null() || out_width.is_null() || out_height.is_null() {
        return 0;
    }
    guarded("svg_size", 0, || {
        let Ok(key) = CStr::from_ptr(key).to_str() else {
            set_last_error("svg_size: key is not valid UTF-8");
            return 0;
        };
        match crate::svg::intrinsic_size(key, std::slice::from_raw_parts(bytes, len)) {
            Some((w, h)) => {
                *out_width = w;
                *out_height = h;
                1
            }
            None => {
                set_last_error(&format!("svg_size: {key} is not a parsable SVG"));
                0
            }
        }
    })
}

/// Rasterize an SVG image for a host image view: `key` names it for the parse cache (its URL),
/// `bytes`/`len` are the fetched SVG. The size is the intrinsic size times `scale` (device pixels
/// per SVG unit, bucketed like sprites), clamped to `max_dim`. Returns 1 and writes the size and a
/// straight-alpha RGBA buffer (free with `orbital_canvas_free_rgba`); 0 on a null argument or an
/// SVG that does not parse (reason via last error).
///
/// # Safety
/// `key` must be a valid null-terminated UTF-8 string, `bytes` valid for `len` bytes, and every
/// out pointer valid.
#[no_mangle]
pub unsafe extern "C" fn orbital_canvas_svg_rasterize(
    key: *const c_char,
    bytes: *const u8,
    len: usize,
    scale: f32,
    max_dim: u32,
    out_width: *mut u32,
    out_height: *mut u32,
    out_pixels: *mut *mut u8,
    out_len: *mut usize,
) -> c_int {
    if key.is_null()
        || bytes.is_null()
        || out_width.is_null()
        || out_height.is_null()
        || out_pixels.is_null()
        || out_len.is_null()
    {
        return 0;
    }
    guarded("svg_rasterize", 0, || {
        let Ok(key) = CStr::from_ptr(key).to_str() else {
            set_last_error("svg_rasterize: key is not valid UTF-8");
            return 0;
        };
        let data = std::slice::from_raw_parts(bytes, len);
        let Some(intrinsic) = crate::svg::intrinsic_size(key, data) else {
            set_last_error(&format!("svg_rasterize: {key} is not a parsable SVG"));
            return 0;
        };
        let (w, h) = crate::svg::raster_size(intrinsic, crate::svg::raster_scale(scale), max_dim);
        let Some(raster) = crate::svg::rasterize(key, data, w, h) else {
            set_last_error(&format!(
                "svg_rasterize: {key} could not be drawn at {w}x{h}"
            ));
            return 0;
        };
        let mut buf = raster.rgba.into_boxed_slice();
        *out_width = raster.width;
        *out_height = raster.height;
        *out_len = buf.len();
        *out_pixels = buf.as_mut_ptr();
        std::mem::forget(buf);
        1
    })
}

/// Free an RGBA buffer returned by `orbital_canvas_render_to_rgba`.
///
/// # Safety
/// `pixels` must be a pointer returned by `orbital_canvas_render_to_rgba`, and
/// `len` must be the value written to `*out_len` by that call.
#[no_mangle]
pub unsafe extern "C" fn orbital_canvas_free_rgba(pixels: *mut u8, len: usize) {
    if !pixels.is_null() && len > 0 {
        guarded("free_rgba", (), || {
            let _ = Box::from_raw(std::ptr::slice_from_raw_parts_mut(pixels, len));
        });
    }
}

/// Free a string returned by the renderer.
///
/// # Safety
/// `s` must be a pointer returned by this crate, and must not be used after.
#[no_mangle]
pub unsafe extern "C" fn orbital_canvas_free_string(s: *mut c_char) {
    if !s.is_null() {
        guarded("free_string", (), || {
            let _ = CString::from_raw(s);
        });
    }
}

#[cfg(test)]
mod tests;
