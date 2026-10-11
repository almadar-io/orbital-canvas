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
pub unsafe extern "C" fn orbital_canvas_submit(
    renderer: *mut Canvas,
    json_packet: *const c_char,
) {
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
        let packet = match serde_json::from_str::<GameRenderPacket>(json) {
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
        let node = match serde_json::from_str::<serde_json::Value>(json) {
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
pub unsafe extern "C" fn orbital_canvas_resize(
    renderer: *mut Canvas,
    width: u32,
    height: u32,
) {
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
pub unsafe extern "C" fn orbital_canvas_set_scale(
    renderer: *mut Canvas,
    scale: f32,
) -> c_int {
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

/// Allow (non-zero) or forbid (0) motion: the host passes the OS reduced-motion setting here.
/// Forbidden, animated scenes draw their rest frame.
///
/// # Safety
/// `renderer` must be a valid pointer.
#[no_mangle]
pub unsafe extern "C" fn orbital_canvas_set_motion(
    renderer: *mut Canvas,
    enabled: c_int,
) {
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
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).map_err(|e| e.to_string()))
        {
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
pub unsafe extern "C" fn orbital_canvas_take_events(
    renderer: *mut Canvas,
) -> *mut c_char {
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
pub unsafe extern "C" fn orbital_canvas_zoom(
    renderer: *mut Canvas,
    factor: f32,
    x: f32,
    y: f32,
) {
    if renderer.is_null() {
        return;
    }
    guarded("zoom", (), || (*renderer).zoom(factor, x, y));
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
pub unsafe extern "C" fn orbital_canvas_last_error(
    _renderer: *mut Canvas,
) -> *mut c_char {
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
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).map_err(|e| e.to_string()))
        {
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

/// Free an RGBA buffer returned by `orbital_canvas_render_to_rgba`.
///
/// # Safety
/// `pixels` must be a pointer returned by `orbital_canvas_render_to_rgba`, and
/// `len` must be the value written to `*out_len` by that call.
#[no_mangle]
pub unsafe extern "C" fn orbital_canvas_free_rgba(pixels: *mut u8, len: usize) {
    if !pixels.is_null() && len > 0 {
        guarded("free_rgba", (), || {
            let _ = Box::from_raw(std::slice::from_raw_parts_mut(pixels, len));
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
mod tests {
    use super::*;

    #[test]
    fn last_error_empty_when_no_error() {
        // Fresh thread-local (or cleared state): must return "".
        set_last_error("");
        assert_eq!(get_last_error(), "");
    }

    #[test]
    fn last_error_captures_set_message() {
        set_last_error("init: no suitable graphics adapter found");
        assert_eq!(get_last_error(), "init: no suitable graphics adapter found");
        // Reset so we don't bleed into sibling tests on the same thread.
        set_last_error("");
    }

    #[test]
    fn last_error_c_api_returns_cstring() {
        set_last_error("render: surface error: Lost");
        let ptr = unsafe { orbital_canvas_last_error(std::ptr::null_mut()) };
        assert!(!ptr.is_null());
        let s = unsafe { CStr::from_ptr(ptr).to_str().unwrap() };
        assert_eq!(s, "render: surface error: Lost");
        unsafe { orbital_canvas_free_string(ptr) };
        set_last_error("");
    }

    fn headless() -> *mut Canvas {
        Box::into_raw(Box::new(
            Canvas::new_headless().expect("headless renderer"),
        ))
    }

    #[test]
    fn set_scene_maps_canvas_children_to_a_packet() {
        let renderer = headless();
        let scene = CString::new(
            r##"{"type":"canvas","projection":"flat","camera":{"zoom":1},"backgroundColor":"#000000",
              "children":[{"type":"draw-shape","shape":"rect","position":{"x":0,"y":0},
                           "width":1,"height":1,"fill":"#ff0000"}]}"##,
        )
        .unwrap();
        assert_eq!(
            unsafe { orbital_canvas_set_scene(renderer, scene.as_ptr()) },
            1
        );
        let packet = unsafe { (*renderer).host.packet.clone() }.expect("packet stored");
        assert_eq!(packet.draw_commands.len(), 2, "Clear + one Quad");
        unsafe { orbital_canvas_destroy(renderer) };
    }

    #[test]
    fn set_scene_rejections_leave_the_packet_alone_and_report() {
        let renderer = headless();
        let bad_json = CString::new("{not json").unwrap();
        set_last_error("");
        assert_eq!(
            unsafe { orbital_canvas_set_scene(renderer, bad_json.as_ptr()) },
            0
        );
        assert!(get_last_error().contains("not valid JSON"));
        let three_d = CString::new(r#"{"type":"canvas","mode":"3d"}"#).unwrap();
        assert_eq!(
            unsafe { orbital_canvas_set_scene(renderer, three_d.as_ptr()) },
            0
        );
        assert!(get_last_error().contains("3d"));
        assert!(unsafe { (*renderer).host.packet.is_none() });
        assert_eq!(
            unsafe { orbital_canvas_set_scene(renderer, ptr::null()) },
            0
        );
        assert_eq!(
            unsafe { orbital_canvas_set_scene(ptr::null_mut(), bad_json.as_ptr()) },
            0
        );
        set_last_error("");
        unsafe { orbital_canvas_destroy(renderer) };
    }

    fn learning_scene() -> CString {
        CString::new(
            r##"{"type":"learning-canvas","width":300,"height":200,"interactive":true,"onShapeClick":"TAP_SHAPE",
              "shapes":[{"type":"circle","id":"nucleus","x":150,"y":100,"radius":20}]}"##,
        )
        .unwrap()
    }

    fn take_events(renderer: *mut Canvas) -> serde_json::Value {
        let ptr = unsafe { orbital_canvas_take_events(renderer) };
        assert!(!ptr.is_null());
        let text = unsafe { CStr::from_ptr(ptr).to_str().unwrap().to_owned() };
        unsafe { orbital_canvas_free_string(ptr) };
        serde_json::from_str(&text).unwrap()
    }

    fn input(renderer: *mut Canvas, kind: &str, x: f32, y: f32) {
        let kind = CString::new(kind).unwrap();
        unsafe { orbital_canvas_input(renderer, kind.as_ptr(), x, y, 0, ptr::null()) };
    }

    #[test]
    fn a_tap_on_a_learning_shape_comes_back_as_its_event_in_logical_points() {
        let renderer = headless();
        assert_eq!(unsafe { orbital_canvas_set_scale(renderer, 3.0) }, 1);
        unsafe { orbital_canvas_resize(renderer, 900, 600) };
        assert_eq!(
            unsafe { orbital_canvas_set_scene(renderer, learning_scene().as_ptr()) },
            1
        );
        input(renderer, "touch-down", 150.0, 100.0);
        input(renderer, "touch-up", 151.0, 100.0);
        let events = take_events(renderer);
        assert_eq!(
            events,
            serde_json::json!([{"event": "TAP_SHAPE", "payload": {"payload": {"id": "nucleus", "type": "circle", "index": 0}}}])
        );
        assert_eq!(
            take_events(renderer),
            serde_json::json!([]),
            "taking drains the queue"
        );
        unsafe { orbital_canvas_destroy(renderer) };
    }

    #[test]
    fn a_drag_a_miss_and_a_stray_touch_up_emit_nothing() {
        let renderer = headless();
        unsafe { orbital_canvas_resize(renderer, 300, 200) };
        assert_eq!(
            unsafe { orbital_canvas_set_scene(renderer, learning_scene().as_ptr()) },
            1
        );
        input(renderer, "touch-down", 150.0, 100.0);
        input(renderer, "touch-up", 190.0, 100.0);
        input(renderer, "touch-down", 10.0, 10.0);
        input(renderer, "touch-up", 10.0, 10.0);
        input(renderer, "touch-up", 150.0, 100.0);
        assert_eq!(take_events(renderer), serde_json::json!([]));
        unsafe { orbital_canvas_destroy(renderer) };
    }

    #[test]
    fn scene_size_reports_the_authored_size_and_refuses_bad_scenes() {
        let (mut w, mut h) = (0.0f32, 0.0f32);
        let ok =
            unsafe { orbital_canvas_scene_size(learning_scene().as_ptr(), &mut w, &mut h) };
        assert_eq!((ok, w, h), (1, 300.0, 200.0));
        let bad = CString::new("{nope").unwrap();
        set_last_error("");
        assert_eq!(
            unsafe { orbital_canvas_scene_size(bad.as_ptr(), &mut w, &mut h) },
            0
        );
        assert!(get_last_error().contains("scene_size"));
        let three_d = CString::new(r#"{"type":"canvas","mode":"3d"}"#).unwrap();
        assert_eq!(
            unsafe { orbital_canvas_scene_size(three_d.as_ptr(), &mut w, &mut h) },
            0
        );
        assert_eq!(
            unsafe { orbital_canvas_scene_size(ptr::null(), &mut w, &mut h) },
            0
        );
        set_last_error("");
    }

    #[test]
    fn render_scene_rgba_draws_the_scene_at_density_and_refuses_bad_ones() {
        let mut pixels: *mut u8 = ptr::null_mut();
        let mut len = 0usize;
        let ok = unsafe {
            orbital_canvas_render_scene_rgba(
                learning_scene().as_ptr(),
                600,
                400,
                2.0,
                &mut pixels,
                &mut len,
            )
        };
        if ok == 0 {
            // No GPU adapter on this host: the call must have said why.
            assert!(get_last_error().contains("render_scene_rgba"));
            return;
        }
        assert_eq!(len, 600 * 400 * 4);
        let buf = unsafe { std::slice::from_raw_parts(pixels, len) };
        let at = |x: usize, y: usize| &buf[(y * 600 + x) * 4..(y * 600 + x) * 4 + 4];
        // shape at scene (150,100) r20, scene 300x200 fit into 300x200 logical at 2x => physical (300,200)
        assert!(
            at(340, 200)[0] < 200,
            "the circle's ring is inked at 2x radius, got {:?}",
            at(340, 200)
        );
        assert_eq!(
            at(300, 200),
            [255, 255, 255, 255],
            "control: the unfilled center is ground"
        );
        assert_eq!(
            at(10, 10),
            [255, 255, 255, 255],
            "control: ground elsewhere"
        );
        unsafe { orbital_canvas_free_rgba(pixels, len) };
        let bad = CString::new("{nope").unwrap();
        assert_eq!(
            unsafe {
                orbital_canvas_render_scene_rgba(
                    bad.as_ptr(),
                    10,
                    10,
                    1.0,
                    &mut pixels,
                    &mut len,
                )
            },
            0
        );
        set_last_error("");
    }

    #[test]
    fn take_events_on_a_null_renderer_is_null() {
        assert!(unsafe { orbital_canvas_take_events(ptr::null_mut()) }.is_null());
    }

    #[test]
    fn guarded_turns_a_panic_into_last_error() {
        set_last_error("");
        let out = guarded("resize", -1, || -> i32 {
            panic!("surface configure validation failed")
        });
        assert_eq!(out, -1);
        assert_eq!(
            get_last_error(),
            "resize: panic: surface configure validation failed"
        );
        set_last_error("");
    }

    #[test]
    fn guarded_formats_owned_string_payloads() {
        set_last_error("");
        let detail = String::from("texture 9000 exceeds limit");
        guarded("render", (), || panic!("{detail}"));
        assert_eq!(
            get_last_error(),
            "render: panic: texture 9000 exceeds limit"
        );
        set_last_error("");
    }

    #[test]
    fn add_font_rejects_an_unknown_script_with_last_error_and_accepts_a_real_face() {
        let _font = crate::vector::FONT_GENERATION_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let bytes = include_bytes!("../assets/fonts/NotoSansArabic-Regular-subset.ttf");
        let bad = CString::new("Zzzq").unwrap();
        assert_eq!(
            unsafe { orbital_canvas_add_font(bad.as_ptr(), bytes.as_ptr(), bytes.len(), 0) },
            0
        );
        assert!(
            get_last_error().contains("unknown script"),
            "{}",
            get_last_error()
        );
        let good = CString::new("Ethi").unwrap();
        assert_eq!(
            unsafe {
                orbital_canvas_add_font(good.as_ptr(), bytes.as_ptr(), bytes.len(), 0)
            },
            1
        );
    }

    #[test]
    fn asset_generation_reports_the_asset_cache_counter() {
        assert_eq!(
            orbital_canvas_asset_generation(),
            crate::asset::generation()
        );
    }

    #[test]
    fn take_missing_scripts_is_a_json_array() {
        let raw = orbital_canvas_take_missing_scripts();
        assert!(!raw.is_null());
        let text = unsafe { CStr::from_ptr(raw) }
            .to_string_lossy()
            .into_owned();
        unsafe { orbital_canvas_free_string(raw) };
        assert!(
            serde_json::from_str::<Vec<serde_json::Value>>(&text).is_ok(),
            "{text}"
        );
    }

    #[test]
    fn guarded_normal_path_returns_value_and_leaves_error_untouched() {
        set_last_error("");
        assert_eq!(guarded("input", 0, || 1), 1);
        assert_eq!(get_last_error(), "");
    }

    fn scene_cstring() -> CString {
        CString::new(
            r##"{"type":"canvas","projection":"flat","camera":{"zoom":1},"backgroundColor":"#000000",
              "children":[{"type":"draw-shape","shape":"rect","position":{"x":0,"y":0},
                           "width":1,"height":1,"fill":"#ff0000"}]}"##,
        )
        .unwrap()
    }

    #[test]
    fn scene_is_laid_out_in_the_surface_viewport_and_follows_resize() {
        let renderer = headless();
        unsafe { orbital_canvas_resize(renderer, 300, 600) };
        assert_eq!(
            unsafe { orbital_canvas_set_scene(renderer, scene_cstring().as_ptr()) },
            1
        );
        let p = unsafe { (*renderer).host.packet.clone() }.expect("packet");
        assert_eq!((p.width, p.height), (300.0, 600.0));
        unsafe { orbital_canvas_resize(renderer, 1206, 2172) };
        let p = unsafe { (*renderer).host.packet.clone() }.expect("packet");
        assert_eq!((p.width, p.height), (1206.0, 2172.0));
        unsafe { orbital_canvas_destroy(renderer) };
    }

    #[test]
    fn viewport_matching_the_default_size_changes_nothing() {
        let plain = headless();
        assert_eq!(
            unsafe { orbital_canvas_set_scene(plain, scene_cstring().as_ptr()) },
            1
        );
        let sized = headless();
        unsafe { orbital_canvas_resize(sized, 800, 600) };
        assert_eq!(
            unsafe { orbital_canvas_set_scene(sized, scene_cstring().as_ptr()) },
            1
        );
        let a = unsafe { (*plain).host.packet.clone() }.unwrap();
        let b = unsafe { (*sized).host.packet.clone() }.unwrap();
        assert_eq!((a.width, a.height), (800.0, 600.0));
        assert_eq!(
            serde_json::to_value(&a).unwrap(),
            serde_json::to_value(&b).unwrap()
        );
        unsafe {
            orbital_canvas_destroy(plain);
            orbital_canvas_destroy(sized);
        }
    }

    fn logical_size(r: *mut Canvas) -> (f32, f32) {
        let p = unsafe { (*r).host.packet.clone() }.expect("packet");
        (p.width, p.height)
    }

    /// A 3x device reports physical px; the scene lays out in logical points, so the
    /// cell size in logical units matches the 1x device with the same logical viewport.
    #[test]
    fn density_three_lays_out_the_same_logical_viewport_as_one() {
        let one = headless();
        unsafe { orbital_canvas_resize(one, 402, 724) };
        assert_eq!(
            unsafe { orbital_canvas_set_scene(one, scene_cstring().as_ptr()) },
            1
        );

        let three = headless();
        unsafe { orbital_canvas_resize(three, 1206, 2172) };
        assert_eq!(unsafe { orbital_canvas_set_scale(three, 3.0) }, 1);
        assert_eq!(
            unsafe { orbital_canvas_set_scene(three, scene_cstring().as_ptr()) },
            1
        );

        assert_eq!(logical_size(three), (402.0, 724.0));
        let a = unsafe { (*one).host.packet.clone() }.unwrap();
        let b = unsafe { (*three).host.packet.clone() }.unwrap();
        assert_eq!(
            serde_json::to_value(&a).unwrap(),
            serde_json::to_value(&b).unwrap()
        );
        unsafe {
            orbital_canvas_destroy(one);
            orbital_canvas_destroy(three);
        }
    }

    #[test]
    fn set_scale_after_the_scene_re_lays_it_out_and_resize_keeps_the_scale() {
        let r = headless();
        unsafe { orbital_canvas_resize(r, 1206, 2172) };
        assert_eq!(
            unsafe { orbital_canvas_set_scene(r, scene_cstring().as_ptr()) },
            1
        );
        assert_eq!(
            logical_size(r),
            (1206.0, 2172.0),
            "control: default scale is 1"
        );
        assert_eq!(unsafe { orbital_canvas_set_scale(r, 3.0) }, 1);
        assert_eq!(logical_size(r), (402.0, 724.0));
        unsafe { orbital_canvas_resize(r, 600, 300) };
        assert_eq!(logical_size(r), (200.0, 100.0));
        unsafe { orbital_canvas_destroy(r) };
    }

    #[test]
    fn set_scale_rejects_bad_values_and_keeps_the_previous_scale() {
        let r = headless();
        unsafe { orbital_canvas_resize(r, 600, 300) };
        assert_eq!(unsafe { orbital_canvas_set_scale(r, 2.0) }, 1);
        for bad in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            set_last_error("");
            assert_eq!(
                unsafe { orbital_canvas_set_scale(r, bad) },
                0,
                "{bad}"
            );
            assert!(get_last_error().starts_with("set_scale:"));
        }
        assert_eq!(
            unsafe { orbital_canvas_set_scene(r, scene_cstring().as_ptr()) },
            1
        );
        assert_eq!(logical_size(r), (300.0, 150.0), "still 2x");
        assert_eq!(
            unsafe { orbital_canvas_set_scale(ptr::null_mut(), 2.0) },
            0
        );
        set_last_error("");
        unsafe { orbital_canvas_destroy(r) };
    }

    /// Union of every Poly/Quad/Sprite the packet draws, as (min_x, min_y, max_x, max_y).
    fn drawn_bounds(p: &crate::packet::GameRenderPacket) -> (f32, f32, f32, f32) {
        use crate::packet::DrawCommand;
        let mut b = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        let mut add = |x: f32, y: f32| {
            b = (b.0.min(x), b.1.min(y), b.2.max(x), b.3.max(y));
        };
        for c in &p.draw_commands {
            match c {
                DrawCommand::Poly { points, .. } => points.iter().for_each(|v| add(v.x, v.y)),
                DrawCommand::Quad { rect, .. } | DrawCommand::Sprite { dst: rect, .. } => {
                    add(rect.x, rect.y);
                    add(rect.x + rect.w, rect.y + rect.h);
                }
                _ => {}
            }
        }
        b
    }

    fn board_scene(projection: &str, fit: bool) -> CString {
        let cells: Vec<String> = (0..6)
            .flat_map(|x| (0..4).map(move |y| (x, y)))
            .map(|(x, y)| {
                format!(
                    r##"{{"type":"draw-shape","shape":"cell","position":{{"x":{x},"y":{y}}},"fill":"#4ade80"}}"##
                )
            })
            .collect();
        CString::new(format!(
            r##"{{"type":"canvas","projection":"{projection}","fit":{fit},"camera":{{"zoom":0.4}},"backgroundColor":"#000000","children":[{}]}}"##,
            cells.join(",")
        ))
        .unwrap()
    }

    fn assert_board_centred(r: *mut Canvas, tolerance: f32, what: &str) {
        let p = unsafe { (*r).host.packet.clone() }.expect("packet");
        let (x0, y0, x1, y1) = drawn_bounds(&p);
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        assert!(
            (cx - p.width / 2.0).abs() <= tolerance && (cy - p.height / 2.0).abs() <= tolerance,
            "{what}: board centre ({cx}, {cy}) vs viewport centre ({}, {}), bounds ({x0},{y0})-({x1},{y1})",
            p.width / 2.0,
            p.height / 2.0
        );
    }

    /// The board centres in the LOGICAL viewport at 1x and 3x, with and without
    /// `fit`; a flat board checks the square-pitch layout too.
    #[test]
    fn grid_boards_centre_in_the_logical_viewport_at_one_and_three_x() {
        for (projection, fit) in [("isometric", true), ("isometric", false), ("flat", true)] {
            for (density, w, h) in [(1.0f32, 402u32, 724u32), (3.0, 1206, 2172)] {
                let r = headless();
                unsafe { orbital_canvas_resize(r, w, h) };
                assert_eq!(unsafe { orbital_canvas_set_scale(r, density) }, 1);
                assert_eq!(
                    unsafe {
                        orbital_canvas_set_scene(r, board_scene(projection, fit).as_ptr())
                    },
                    1
                );
                assert_eq!(logical_size(r), (402.0, 724.0));
                assert_board_centred(r, 1.0, &format!("{projection} fit={fit} at {density}x"));
                unsafe { orbital_canvas_destroy(r) };
            }
        }
    }

    /// Control: a viewport change re-centres (the board follows the surface, it is
    /// not pinned to the origin), and `fit` keeps the board inside the viewport.
    #[test]
    fn fitted_board_stays_inside_the_viewport_and_recentres_on_resize() {
        let r = headless();
        unsafe { orbital_canvas_resize(r, 402, 724) };
        assert_eq!(
            unsafe { orbital_canvas_set_scene(r, board_scene("isometric", true).as_ptr()) },
            1
        );
        let p = unsafe { (*r).host.packet.clone() }.unwrap();
        let (x0, y0, x1, y1) = drawn_bounds(&p);
        assert!(
            x0 >= 0.0 && x1 <= 402.0 && y0 >= 0.0 && y1 <= 724.0,
            "inside: {x0},{y0},{x1},{y1}"
        );
        unsafe { orbital_canvas_resize(r, 800, 300) };
        assert_board_centred(r, 1.0, "after resize");
        let p = unsafe { (*r).host.packet.clone() }.unwrap();
        let (x0, y0, x1, y1) = drawn_bounds(&p);
        assert!(
            x0 >= 0.0 && x1 <= 800.0 && y0 >= 0.0 && y1 <= 300.0,
            "inside after resize: {x0},{y0},{x1},{y1}"
        );
        unsafe { orbital_canvas_destroy(r) };
    }

    #[test]
    fn submitting_a_raw_packet_drops_the_scene_so_resize_does_not_overwrite_it() {
        let renderer = headless();
        assert_eq!(
            unsafe { orbital_canvas_set_scene(renderer, scene_cstring().as_ptr()) },
            1
        );
        let raw = CString::new(r#"{"pattern":"x","width":10.0,"height":10.0,"background_image":null,"asset_base_url":null,"camera":null,"draw_commands":[],"tick_event":null,"draw_event":null,"fps":null}"#).unwrap();
        unsafe { orbital_canvas_submit(renderer, raw.as_ptr()) };
        unsafe { orbital_canvas_resize(renderer, 300, 600) };
        let p = unsafe { (*renderer).host.packet.clone() }.unwrap();
        assert_eq!((p.pattern.as_str(), p.width), ("x", 10.0));
        unsafe { orbital_canvas_destroy(renderer) };
    }

    #[test]
    fn clock_motion_and_wants_frames_are_null_safe_and_drive_the_renderer() {
        unsafe {
            orbital_canvas_set_clock(ptr::null_mut(), 1.0);
            orbital_canvas_set_motion(ptr::null_mut(), 0);
            assert_eq!(orbital_canvas_wants_frames(ptr::null_mut()), 0);
        }
        let r = Box::into_raw(Box::new(Canvas::new_headless().unwrap()));
        let scene = CString::new(r#"{"type":"svg-pulse","rings":3}"#).unwrap();
        unsafe {
            assert_eq!(orbital_canvas_set_scene(r, scene.as_ptr()), 1);
            assert_eq!(
                orbital_canvas_wants_frames(r),
                1,
                "the pulse animates"
            );
            orbital_canvas_set_motion(r, 0);
            assert!(!(*r).is_animated(), "reduced motion stops the animation");
            orbital_canvas_set_motion(r, 1);
            orbital_canvas_set_clock(r, f64::NAN);
            assert!((*r).host.clock_epoch.is_none(), "a non-finite clock is ignored");
            orbital_canvas_set_clock(r, 250.0);
            assert_eq!((*r).host.clock_epoch, Some(250.0));
            drop(Box::from_raw(r));
        }
    }
}
