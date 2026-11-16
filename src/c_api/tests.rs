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

fn svg_raster(key: &str, svg: &str, scale: f32) -> Option<(u32, u32, Vec<u8>)> {
    let key = CString::new(key).unwrap();
    let (mut w, mut h, mut px, mut len) = (0u32, 0u32, std::ptr::null_mut(), 0usize);
    let ok = unsafe {
        orbital_canvas_svg_rasterize(
            key.as_ptr(),
            svg.as_ptr(),
            svg.len(),
            scale,
            4096,
            &mut w,
            &mut h,
            &mut px,
            &mut len,
        )
    };
    if ok == 0 {
        return None;
    }
    let rgba = unsafe { std::slice::from_raw_parts(px, len) }.to_vec();
    unsafe { orbital_canvas_free_rgba(px, len) };
    Some((w, h, rgba))
}

#[test]
fn svg_rasterize_draws_at_intrinsic_size_times_scale() {
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="5"><rect width="10" height="5" fill="#0000ff"/></svg>"##;
    let (w, h, rgba) = svg_raster("t://blue", svg, 2.0).expect("rasterized");
    assert_eq!((w, h), (20, 10));
    assert_eq!(rgba.len(), 20 * 10 * 4);
    assert_eq!(&rgba[0..4], &[0, 0, 255, 255]);
    let (w1, h1, _) = svg_raster("t://blue", svg, 1.0).expect("rasterized");
    assert_eq!((w1, h1), (10, 5));
}

fn decode(url: &str) -> Option<(Vec<u8>, Option<String>)> {
    let url = CString::new(url).unwrap();
    let (mut bytes, mut len, mut mime) = (ptr::null_mut(), 0usize, ptr::null_mut());
    if unsafe { orbital_canvas_decode_data_url(url.as_ptr(), &mut bytes, &mut len, &mut mime) } == 0
    {
        return None;
    }
    let data = if bytes.is_null() {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(bytes, len) }.to_vec()
    };
    unsafe { orbital_canvas_free_rgba(bytes, len) };
    let mime = (!mime.is_null()).then(|| {
        let m = unsafe { CStr::from_ptr(mime) }
            .to_str()
            .unwrap()
            .to_string();
        unsafe { orbital_canvas_free_string(mime) };
        m
    });
    Some((data, mime))
}

#[test]
fn decode_data_url_percent_and_base64_payloads_and_refuses_malformed() {
    assert_eq!(
        decode("data:image/svg+xml;utf8,%3Csvg xmlns=%22a%22/%3E"),
        Some((b"<svg xmlns=\"a\"/>".to_vec(), Some("image/svg+xml".into())))
    );
    assert_eq!(
        decode("data:text/plain;base64,aGk="),
        Some((b"hi".to_vec(), Some("text/plain".into())))
    );
    assert_eq!(decode("data:,"), Some((Vec::new(), None)));
    set_last_error("");
    assert_eq!(decode("data:image/png;base64"), None);
    assert!(get_last_error().contains("no `,`"));
    set_last_error("");
}

#[test]
fn asset_is_svg_follows_the_declared_type_then_the_path_extension() {
    let is_svg = |url: &str, ct: Option<&str>| {
        let url = CString::new(url).unwrap();
        let ct = ct.map(|c| CString::new(c).unwrap());
        unsafe {
            orbital_canvas_asset_is_svg(
                url.as_ptr(),
                ct.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
            )
        }
    };
    assert_eq!(
        is_svg("data:image/svg+xml;base64,AA==", Some("image/svg+xml")),
        1
    );
    assert_eq!(is_svg("https://x/logo.svg?v=1", None), 1);
    assert_eq!(is_svg("https://x/photo.png", Some("image/png")), 0);
    assert_eq!(is_svg("https://x/render", None), 0);
}

#[test]
fn svg_size_reads_intrinsic_units_and_refuses_non_svg() {
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="12.5" height="4"/>"##;
    let key = CString::new("t://size").unwrap();
    let (mut w, mut h) = (0u32, 0u32);
    let ok =
        unsafe { orbital_canvas_svg_size(key.as_ptr(), svg.as_ptr(), svg.len(), &mut w, &mut h) };
    assert_eq!((ok, w, h), (1, 13, 4));
    let bad = "plain text";
    let key = CString::new("t://size-bad").unwrap();
    let ok =
        unsafe { orbital_canvas_svg_size(key.as_ptr(), bad.as_ptr(), bad.len(), &mut w, &mut h) };
    assert_eq!(ok, 0);
    set_last_error("");
}

#[test]
fn svg_rasterize_refuses_bytes_that_are_not_svg() {
    set_last_error("");
    assert!(svg_raster("t://not-svg", "not an svg", 2.0).is_none());
    assert!(get_last_error().contains("not a parsable SVG"));
    set_last_error("");
}

fn headless() -> *mut Canvas {
    Box::into_raw(Box::new(Canvas::new_headless().expect("headless renderer")))
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
    let undeclared_mode =
        CString::new(r#"{"type":"canvas","mode":"3d","camera":{"mode":"sideways"}}"#).unwrap();
    assert_eq!(
        unsafe { orbital_canvas_set_scene(renderer, undeclared_mode.as_ptr()) },
        0
    );
    assert!(get_last_error().contains("canvas.camera.mode"));
    assert!(get_last_error().contains("canvas.camera"));
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
    orbital_core_base::json_value::from_json_str(&text).unwrap()
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
    let ok = unsafe { orbital_canvas_scene_size(learning_scene().as_ptr(), &mut w, &mut h) };
    assert_eq!((ok, w, h), (1, 300.0, 200.0));
    let bad = CString::new("{nope").unwrap();
    set_last_error("");
    assert_eq!(
        unsafe { orbital_canvas_scene_size(bad.as_ptr(), &mut w, &mut h) },
        0
    );
    assert!(get_last_error().contains("scene_size"));
    let three_d =
        CString::new(r#"{"type":"canvas","mode":"3d","camera":{"mode":"sideways"}}"#).unwrap();
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
            orbital_canvas_render_scene_rgba(bad.as_ptr(), 10, 10, 1.0, &mut pixels, &mut len)
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
    let bytes = include_bytes!("../../crates/paint/assets/fonts/NotoSansArabic-Regular-subset.ttf");
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
        unsafe { orbital_canvas_add_font(good.as_ptr(), bytes.as_ptr(), bytes.len(), 0) },
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
        orbital_core_base::json_value::from_json_str::<Vec<serde_json::Value>>(&text).is_ok(),
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
        assert_eq!(unsafe { orbital_canvas_set_scale(r, bad) }, 0, "{bad}");
        assert!(get_last_error().starts_with("set_scale:"));
    }
    assert_eq!(
        unsafe { orbital_canvas_set_scene(r, scene_cstring().as_ptr()) },
        1
    );
    assert_eq!(logical_size(r), (300.0, 150.0), "still 2x");
    assert_eq!(unsafe { orbital_canvas_set_scale(ptr::null_mut(), 2.0) }, 0);
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
                unsafe { orbital_canvas_set_scene(r, board_scene(projection, fit).as_ptr()) },
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
        orbital_canvas_set_epoch_clock(ptr::null_mut(), 1000.0);
        orbital_canvas_set_motion(ptr::null_mut(), 0);
        assert_eq!(orbital_canvas_wants_frames(ptr::null_mut()), 0);
    }
    let r = Box::into_raw(Box::new(Canvas::new_headless().unwrap()));
    let scene = CString::new(r#"{"type":"svg-pulse","rings":3}"#).unwrap();
    unsafe {
        assert_eq!(orbital_canvas_set_scene(r, scene.as_ptr()), 1);
        assert_eq!(orbital_canvas_wants_frames(r), 1, "the pulse animates");
        orbital_canvas_set_motion(r, 0);
        assert!(!(*r).is_animated(), "reduced motion stops the animation");
        orbital_canvas_set_motion(r, 1);
        orbital_canvas_set_clock(r, f64::NAN);
        assert!(
            (*r).host.clock_epoch.is_none(),
            "a non-finite clock is ignored"
        );
        orbital_canvas_set_clock(r, 250.0);
        assert_eq!((*r).host.clock_epoch, Some(250.0));
        orbital_canvas_set_epoch_clock(r, f64::NAN);
        assert_eq!((*r).host.epoch_clock_ms, 0.0);
        orbital_canvas_set_epoch_clock(r, 1000.0);
        assert_eq!((*r).host.epoch_clock_ms, 1000.0);
        drop(Box::from_raw(r));
    }
}
