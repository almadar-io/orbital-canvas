use super::svg_sprite_tests::{draw, px, renderer};
use super::vector_layers::VECTOR_LAYER_TARGETS;
use super::*;
use orbital_canvas_base::packet::{PathSegment, Stroke, TextAlign};

const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

fn rect(x: f32, y: f32, w: f32, h: f32) -> Vec<PathSegment> {
    vec![
        PathSegment::MoveTo { x, y },
        PathSegment::LineTo { x: x + w, y },
        PathSegment::LineTo { x: x + w, y: y + h },
        PathSegment::LineTo { x, y: y + h },
        PathSegment::Close,
    ]
}

fn packet(commands: Vec<DrawCommand>) -> GameRenderPacket {
    GameRenderPacket {
        pattern: "learning-canvas".into(),
        width: 16.0,
        height: 16.0,
        draw_commands: commands,
        ..Default::default()
    }
}

fn filled(x: f32, color: [f32; 4]) -> DrawCommand {
    DrawCommand::Path {
        segments: rect(x, 4.0, 8.0, 8.0),
        fill: Some(color),
        stroke: None,
    }
}

#[test]
fn a_filled_path_reaches_the_target_at_its_logical_position() {
    let Some(mut r) = renderer() else { return };
    let Some(rgba) = draw(&mut r, &packet(vec![filled(4.0, RED)]), 16) else {
        return;
    };
    assert_eq!(px(&rgba, 16, 8, 8), [255, 0, 0, 255]);
    assert_eq!(
        px(&rgba, 16, 1, 1)[3],
        0,
        "control: outside the path stays clear"
    );
}

#[test]
fn density_scales_the_vector_layer_with_the_target() {
    let Some(mut r) = renderer() else { return };
    let Some(rgba) = draw(&mut r, &packet(vec![filled(4.0, RED)]), 32) else {
        return;
    };
    assert_eq!(px(&rgba, 32, 16, 16), [255, 0, 0, 255]);
    assert_eq!(
        px(&rgba, 32, 4, 16)[3],
        0,
        "control: 2x target keeps logical extents"
    );
}

#[test]
fn vector_commands_keep_z_order_with_quads() {
    let Some(mut r) = renderer() else { return };
    let quad = |color| DrawCommand::Quad {
        rect: Rect {
            x: 0.0,
            y: 0.0,
            w: 16.0,
            h: 16.0,
        },
        color,
    };
    let over = draw(&mut r, &packet(vec![quad(BLUE), filled(4.0, RED)]), 16).unwrap();
    assert_eq!(px(&over, 16, 8, 8), [255, 0, 0, 255], "path over quad");
    let under = draw(&mut r, &packet(vec![filled(4.0, RED), quad(BLUE)]), 16).unwrap();
    assert_eq!(
        px(&under, 16, 8, 8),
        [0, 0, 255, 255],
        "quad over path (control)"
    );
}

#[test]
fn gpu_clip_group_masks_mixed_quad_and_vector_children() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let group = DrawCommand::ClipGroup {
        clip: rect(0.0, 0.0, 8.0, 16.0),
        commands: vec![
            DrawCommand::Quad {
                rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 16.0,
                    h: 16.0,
                },
                color: BLUE,
            },
            filled(2.0, RED),
        ],
    };
    let Some(rgba) = draw(&mut renderer, &packet(vec![group]), 16) else {
        return;
    };
    assert_eq!(px(&rgba, 16, 4, 8), [255, 0, 0, 255]);
    assert_eq!(px(&rgba, 16, 1, 8), [0, 0, 255, 255]);
    assert_eq!(px(&rgba, 16, 12, 8)[3], 0);
}

#[test]
fn authored_shape_pivot_and_group_stroke_width_reach_gpu_pixels() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let rotated = orbital_canvas_map::game_canvas_2d::from_props(&serde_json::json!({
        "width": 64, "height": 64, "scale": 1.0, "projection": "free", "bgColor": "#00000000",
        "drawables": [{"type": "draw-shape", "shape": "rect",
            "position": {"x": 16, "y": 16}, "width": 32, "height": 16,
            "fill": "#00ff00", "rotate": std::f64::consts::FRAC_PI_2,
            "pivot": {"x": 0, "y": 0}}]
    }))
    .unwrap();
    let pixels = draw(&mut renderer, &rotated, 64).expect("rotated shape GPU frame");
    assert!(
        px(&pixels, 64, 8, 24)[1] > 200,
        "pivot rotates fill into left side"
    );
    assert_eq!(
        px(&pixels, 64, 24, 24)[3],
        0,
        "old unrotated area stays clear"
    );

    let stroked = orbital_canvas_map::game_canvas_2d::from_props(&serde_json::json!({
        "width": 64, "height": 64, "scale": 1.0, "projection": "free", "bgColor": "#00000000",
        "drawables": [{"type": "draw-group", "position": {"x": 0, "y": 0}, "scale": 2,
            "items": [{"type": "draw-shape", "shape": "rect",
                "position": {"x": 8, "y": 8}, "width": 16, "height": 16,
                "stroke": "#ff0000", "strokeWidth": 2}]}]
    }))
    .unwrap();
    let pixels = draw(&mut renderer, &stroked, 64).expect("group stroke GPU frame");
    assert!(
        px(&pixels, 64, 16, 32)[0] > 200,
        "stroke reaches transformed edge"
    );
    assert_eq!(
        px(&pixels, 64, 20, 32)[3],
        0,
        "group scale leaves stroke width in pixels"
    );
}

#[test]
fn authored_shape_blur_softens_edges_on_wgpu_target() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let packet = orbital_canvas_map::game_canvas_2d::from_props(&serde_json::json!({
        "width": 64, "height": 64, "scale": 1.0,
        "projection": "free", "bgColor": "#00000000",
        "drawables": [{"type": "draw-shape", "shape": "rect",
            "position": {"x": 16, "y": 16}, "width": 16, "height": 16,
            "fill": "#ff0000", "blur": 4}]
    }))
    .unwrap();
    assert!(packet
        .draw_commands
        .iter()
        .any(|command| matches!(command, DrawCommand::Blurred { .. })));
    let pixels = draw(&mut renderer, &packet, 64).expect("blurred shape GPU frame");
    assert!(
        px(&pixels, 64, 13, 24)[3] > 0,
        "blur reaches beyond the rect edge"
    );
    assert!(px(&pixels, 64, 24, 24)[0] > 150, "the center remains red");
    assert_eq!(px(&pixels, 64, 3, 24)[3], 0, "far outside remains clear");
}

#[test]
fn authored_lighter_shapes_add_color_on_wgpu_target() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let shape = serde_json::json!({"type": "draw-shape", "shape": "rect",
        "position": {"x": 16, "y": 16}, "width": 16, "height": 16,
        "fill": "#ff0000", "opacity": 0.5, "blendMode": "lighter"});
    let scene = |drawables| {
        orbital_canvas_map::game_canvas_2d::from_props(&serde_json::json!({
            "width": 64, "height": 64, "scale": 1.0, "projection": "free",
            "bgColor": "#00000000", "drawables": drawables,
        }))
        .unwrap()
    };
    let one = draw(&mut renderer, &scene(vec![shape.clone()]), 64).unwrap();
    let two = draw(&mut renderer, &scene(vec![shape.clone(), shape]), 64).unwrap();
    let single = px(&one, 64, 24, 24);
    let doubled = px(&two, 64, 24, 24);
    assert!(
        doubled[0] > single[0] && doubled[3] > single[3],
        "lighter adds overlapping red and alpha: {single:?} -> {doubled:?}"
    );
}

#[test]
fn skinned_triangle_samples_authored_uvs_on_gpu() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let mut pixels = image::RgbaImage::new(4, 4);
    for y in 0..4 {
        for x in 0..4 {
            pixels.put_pixel(
                x,
                y,
                if x < 2 {
                    image::Rgba([255, 0, 0, 255])
                } else {
                    image::Rgba([0, 0, 255, 255])
                },
            );
        }
    }
    let mut png = std::io::Cursor::new(Vec::new());
    pixels.write_to(&mut png, image::ImageFormat::Png).unwrap();
    let texture = super::svg_sprite_tests::fixture("skinned-uv.png", png.get_ref());
    let triangle = DrawCommand::TexturedTriangle {
        texture,
        points: [
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 16.0, y: 0.0 },
            Vec2 { x: 0.0, y: 16.0 },
        ],
        uv: [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
        uv_in_pixels: false,
        opacity: 1.0,
    };
    let Some(rgba) = draw(&mut renderer, &packet(vec![triangle]), 16) else {
        return;
    };
    assert!(px(&rgba, 16, 2, 2)[0] > 200, "left UV samples red");
    assert!(px(&rgba, 16, 10, 2)[2] > 200, "right UV samples blue");
    assert_eq!(px(&rgba, 16, 15, 15)[3], 0, "outside mesh stays clear");
}

#[test]
fn an_identical_packet_reuses_the_cached_layer_and_a_changed_one_adds_one() {
    let Some(mut r) = renderer() else { return };
    let _font = orbital_canvas_paint::vector::FONT_GENERATION_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _ = draw(&mut r, &packet(vec![filled(4.0, RED)]), 16);
    let _ = orbital_canvas_paint::asset::asset_bytes(
        "data:text/plain,cache-generation-must-not-change-solid-path",
    );
    let _ = draw(&mut r, &packet(vec![filled(4.0, RED)]), 16);
    assert_eq!(r.vector_layer_count(), 1);
    let _ = draw(&mut r, &packet(vec![filled(4.0, BLUE)]), 16);
    assert_eq!(r.vector_layer_count(), 2);
}

#[test]
fn a_vector_run_is_rasterized_into_its_box_not_the_whole_target() {
    let Some(mut r) = renderer() else { return };
    let packet = GameRenderPacket {
        pattern: "learning-canvas".into(),
        width: 128.0,
        height: 128.0,
        draw_commands: vec![DrawCommand::Path {
            segments: rect(100.0, 100.0, 8.0, 8.0),
            fill: Some(RED),
            stroke: None,
        }],
        ..Default::default()
    };
    let Some(rgba) = draw(&mut r, &packet, 128) else {
        return;
    };
    assert_eq!(px(&rgba, 128, 104, 104), [255, 0, 0, 255]);
    assert_eq!(px(&rgba, 128, 96, 104)[3], 0);
    assert_eq!(px(&rgba, 128, 104, 96)[3], 0);
    let sizes = r.vector_layer_sizes();
    assert_eq!(sizes.len(), 1);
    assert!(
        sizes[0].0 <= 10 && sizes[0].1 <= 10,
        "the layer is the run's box, got {:?}",
        sizes[0]
    );
}

#[test]
fn a_stroke_paints_past_its_outline_inside_the_box() {
    let Some(mut r) = renderer() else { return };
    let stroke = Stroke {
        color: RED,
        width: 4.0,
        dash: Vec::new(),
        dash_offset: 0.0,
        cap: orbital_canvas_base::packet::LineCap::Butt,
        join: orbital_canvas_base::packet::LineJoin::Miter,
    };
    let packet = GameRenderPacket {
        pattern: "learning-canvas".into(),
        width: 64.0,
        height: 64.0,
        draw_commands: vec![DrawCommand::Path {
            segments: rect(20.0, 20.0, 20.0, 20.0),
            fill: None,
            stroke: Some(stroke),
        }],
        ..Default::default()
    };
    let Some(rgba) = draw(&mut r, &packet, 64) else {
        return;
    };
    assert_eq!(
        px(&rgba, 64, 19, 30)[3],
        255,
        "the stroke's outer half is drawn"
    );
    assert_eq!(
        px(&rgba, 64, 30, 30)[3],
        0,
        "the inside of an unfilled path stays clear"
    );
}

#[test]
fn control_a_run_spanning_the_target_still_covers_it() {
    let Some(mut r) = renderer() else { return };
    let Some(rgba) = draw(
        &mut r,
        &packet(vec![DrawCommand::Path {
            segments: rect(0.0, 0.0, 16.0, 16.0),
            fill: Some(RED),
            stroke: None,
        }]),
        16,
    ) else {
        return;
    };
    assert_eq!(px(&rgba, 16, 0, 0), [255, 0, 0, 255]);
    assert_eq!(px(&rgba, 16, 15, 15), [255, 0, 0, 255]);
    assert_eq!(r.vector_layer_sizes(), vec![(16, 16)]);
}

#[test]
fn a_cached_layer_is_not_reused_once_the_world_size_changes_under_the_same_target() {
    let Some(mut r) = renderer() else { return };
    let path = || DrawCommand::Path {
        segments: rect(0.0, 4.0, 16.0, 4.0),
        fill: Some(RED),
        stroke: None,
    };
    let tall = GameRenderPacket {
        pattern: "learning-canvas".into(),
        width: 16.0,
        height: 16.0,
        draw_commands: vec![path()],
        ..Default::default()
    };
    let short = GameRenderPacket {
        pattern: "learning-canvas".into(),
        width: 16.0,
        height: 8.0,
        draw_commands: vec![path()],
        ..Default::default()
    };
    let _ = draw(&mut r, &tall, 16);
    let Some(rgba) = draw(&mut r, &short, 16) else {
        return;
    };
    assert_eq!(
        r.vector_layer_count(),
        2,
        "a new world scale is a new layer"
    );
    assert_eq!(
        px(&rgba, 16, 8, 9)[3],
        255,
        "world y 4..8 of 8 fills target rows 8..16"
    );
    assert_eq!(px(&rgba, 16, 8, 6)[3], 0);
}

#[test]
fn vector_layers_are_bounded_by_bytes_relative_to_the_target() {
    let Some(mut r) = renderer() else { return };
    for i in 0..12 {
        let _ = draw(&mut r, &packet(vec![filled(i as f32 * 0.25, RED)]), 16);
    }
    let budget = VECTOR_LAYER_TARGETS * 16 * 16 * 4;
    assert!(
        r.vector_layer_bytes() <= budget,
        "{} bytes held, budget {budget}",
        r.vector_layer_bytes()
    );
    assert!(
        r.vector_layer_count() > 1,
        "control: the cache holds more than one layer"
    );
}

const SIDE: u32 = 128;

fn text_packet(commands: Vec<DrawCommand>) -> GameRenderPacket {
    GameRenderPacket {
        pattern: "learning-canvas".into(),
        width: SIDE as f32,
        height: SIDE as f32,
        draw_commands: commands,
        ..Default::default()
    }
}

fn word(text: &str, x: f32, y: f32, size: f32) -> DrawCommand {
    DrawCommand::Text {
        color_runs: Vec::new(),
        rotation: 0.0,
        text: text.into(),
        pos: Vec2 { x, y },
        size,
        color: RED,
        align: TextAlign::Left,
        weight: 400,
        family: orbital_canvas_base::packet::TextFamily::Sans,
        halo: None,
    }
}

fn alpha(rgba: &[u8]) -> Vec<u8> {
    rgba.chunks_exact(4).map(|p| p[3]).collect()
}

fn cpu_alpha(command: &DrawCommand) -> Vec<u8> {
    alpha(
        &orbital_canvas_paint::vector::rasterize(&[command], SIDE, SIDE, 1.0, 1.0)
            .expect("cpu layer"),
    )
}

fn gpu_alpha(r: &mut SharedDeviceRenderer, command: &DrawCommand) -> Option<Vec<u8>> {
    draw(r, &text_packet(vec![command.clone()]), SIDE).map(|rgba| alpha(&rgba))
}

fn mean_diff(a: &[u8], b: &[u8]) -> f64 {
    let total: u64 = a.iter().zip(b).map(|(x, y)| x.abs_diff(*y) as u64).sum();
    total as f64 / a.len() as f64
}

fn ink_sum(a: &[u8]) -> u64 {
    a.iter().map(|v| *v as u64).sum()
}

fn ink_width(a: &[u8]) -> u32 {
    let cols: Vec<u32> = (0..SIDE)
        .filter(|x| (0..SIDE).any(|y| a[(y * SIDE + x) as usize] > 40))
        .collect();
    match (cols.first(), cols.last()) {
        (Some(first), Some(last)) => last - first + 1,
        _ => 0,
    }
}

#[test]
fn atlas_text_paints_the_same_pixels_as_the_cpu_path() {
    let Some(mut r) = renderer() else { return };
    let command = word("Hello Wg", 10.3, 40.7, 24.0);
    let cpu = cpu_alpha(&command);
    let Some(gpu) = gpu_alpha(&mut r, &command) else {
        return;
    };
    assert!(ink_sum(&cpu) > 0, "the CPU path inks");
    let diff = mean_diff(&cpu, &gpu);
    assert!(diff < 0.6, "mean alpha difference {diff}");
    let (c, g) = (ink_sum(&cpu) as f64, ink_sum(&gpu) as f64);
    assert!((c - g).abs() / c < 0.03, "ink {c} vs {g}");
    let shifted = cpu_alpha(&word("Hello Wg", 13.3, 40.7, 24.0));
    assert!(
        mean_diff(&shifted, &gpu) > diff * 4.0,
        "control: text three pixels over is far from the atlas pixels"
    );
}

#[test]
fn atlas_text_draws_in_the_text_colour_and_keeps_z_order() {
    let Some(mut r) = renderer() else { return };
    let under = DrawCommand::Quad {
        rect: Rect {
            x: 0.0,
            y: 0.0,
            w: SIDE as f32,
            h: SIDE as f32,
        },
        color: [0.0, 0.0, 1.0, 1.0],
    };
    let text = word("WWW", 4.0, 40.0, 40.0);
    let Some(over) = draw(
        &mut r,
        &text_packet(vec![under.clone(), text.clone()]),
        SIDE,
    ) else {
        return;
    };
    assert!(
        over.chunks_exact(4).any(|p| p[0] > 240 && p[2] < 15),
        "red text over the blue quad"
    );
    let Some(hidden) = draw(&mut r, &text_packet(vec![text, under]), SIDE) else {
        return;
    };
    assert!(
        hidden.chunks_exact(4).all(|p| p[0] == 0 && p[2] == 255),
        "control: the quad last covers the text"
    );
}

#[test]
fn atlas_glyph_instances_carry_their_runs_colour() {
    let Some(mut r) = renderer() else { return };
    let cell = orbital_canvas_paint::vector::shape(
        "W",
        40.0,
        false,
        orbital_canvas_paint::vector::Family::Sans,
    )
    .width;
    let run = |runs: Vec<orbital_canvas_base::packet::ColorRun>| match word("WWW", 4.0, 60.0, 40.0)
    {
        DrawCommand::Text {
            text,
            pos,
            size,
            color,
            align,
            weight,
            halo,
            family,
            ..
        } => DrawCommand::Text {
            text,
            pos,
            size,
            color,
            align,
            weight,
            halo,
            family,
            color_runs: runs,
            rotation: 0.0,
        },
        other => other,
    };
    let ink = |rgba: &[u8], k: u32| -> Vec<[u8; 4]> {
        let x0 = (4.0 + cell * (k as f32 + 0.25)) as u32;
        let x1 = (4.0 + cell * (k as f32 + 0.75)) as u32;
        (0..SIDE)
            .flat_map(|y| (x0..x1).map(move |x| (x, y)))
            .map(|(x, y)| px(rgba, SIDE, x, y))
            .filter(|p| p[3] > 240)
            .collect()
    };
    let Some(striped) = draw(
        &mut r,
        &text_packet(vec![run(vec![orbital_canvas_base::packet::ColorRun {
            from: 1,
            to: 2,
            color: BLUE,
        }])]),
        SIDE,
    ) else {
        return;
    };
    let red = |p: &[u8; 4]| p[0] > 240 && p[2] < 15;
    let blue = |p: &[u8; 4]| p[2] > 240 && p[0] < 15;
    for (k, want_blue) in [(0, false), (1, true), (2, false)] {
        let pixels = ink(&striped, k);
        assert!(!pixels.is_empty(), "glyph {k} inks");
        assert!(
            pixels
                .iter()
                .all(|p| if want_blue { blue(p) } else { red(p) }),
            "glyph {k}"
        );
    }
    let Some(plain) = draw(&mut r, &text_packet(vec![run(Vec::new())]), SIDE) else {
        return;
    };
    assert!(
        ink(&plain, 1).iter().all(red),
        "control: without runs the middle glyph is the base colour"
    );
}

#[test]
fn arabic_through_the_atlas_is_drawn_and_joined() {
    let Some(mut r) = renderer() else { return };
    let joined = word("بيت", 20.0, 40.0, 28.0);
    let apart = word("ب ي ت", 20.0, 40.0, 28.0);
    let Some(gpu) = gpu_alpha(&mut r, &joined) else {
        return;
    };
    assert!(ink_sum(&gpu) > 0, "the joined word has coverage");
    let diff = mean_diff(&cpu_alpha(&joined), &gpu);
    assert!(
        diff < 0.6,
        "the atlas draws the CPU's joined glyphs: {diff}"
    );
    let Some(spread) = gpu_alpha(&mut r, &apart) else {
        return;
    };
    assert!(ink_sum(&spread) > 0);
    assert!(
        ink_width(&gpu) < ink_width(&spread),
        "control: the letters set apart are wider than the joined word"
    );
}

#[test]
fn a_second_frame_of_the_same_text_makes_no_new_instance_buffer() {
    let Some(mut r) = renderer() else { return };
    let command = word("Cache", 8.0, 40.0, 20.0);
    let Some(first) = gpu_alpha(&mut r, &command) else {
        return;
    };
    let made = r.glyph_buffers_made();
    assert!(made > 0, "control: the first frame makes its buffer");
    for _ in 0..3 {
        let Some(again) = gpu_alpha(&mut r, &command) else {
            return;
        };
        assert_eq!(first, again, "the same pixels");
    }
    assert_eq!(
        r.glyph_buffers_made(),
        made,
        "later frames write into the buffer the first one made"
    );
    let long = "The quick brown fox jumps over the lazy dog and then does it again";
    let _ = gpu_alpha(&mut r, &word(long, 0.0, 40.0, 6.0));
    assert!(
        r.glyph_buffers_made() > made,
        "control: a run that does not fit a held buffer makes a larger one"
    );
}

fn rasterized(r: &SharedDeviceRenderer) -> u64 {
    r.glyphs.as_ref().map_or(0, |g| g.rasterized())
}

#[test]
fn a_text_larger_than_an_atlas_page_is_refused_before_it_rasterizes() {
    let Some(mut r) = renderer() else { return };
    let Some(_) = gpu_alpha(&mut r, &word("W", 0.0, 0.0, 2048.0)) else {
        return;
    };
    assert_eq!(
        rasterized(&r),
        0,
        "no glyph bitmap is made for a page it cannot fit"
    );
    let _ = gpu_alpha(&mut r, &word("W", 8.0, 40.0, 20.0));
    assert!(rasterized(&r) > 0, "control: a page-sized text rasterizes");
}

#[test]
fn a_packet_without_area_draws_nothing() {
    let Some(mut r) = renderer() else { return };
    let red = DrawCommand::Quad {
        rect: Rect {
            x: 0.0,
            y: 0.0,
            w: SIDE as f32,
            h: SIDE as f32,
        },
        color: RED,
    };
    let flat = GameRenderPacket {
        height: 0.0,
        ..text_packet(vec![red.clone(), word("W", 8.0, 0.0, 15.0)])
    };
    let target = r.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("flat world target"),
        size: wgpu::Extent3d {
            width: SIDE,
            height: SIDE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    assert!(
        !r.render_packet_to_texture(&flat, &target),
        "nothing is drawn"
    );
    let encoder = r
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("readback"),
        });
    let Ok(blank) = copy_texture_to_rgba(r.device(), r.queue(), encoder, &target, SIDE, SIDE)
    else {
        return;
    };
    assert!(
        blank.chunks_exact(4).all(|p| p[0] < 15),
        "a world with no height paints nothing"
    );
    assert_eq!(rasterized(&r), 0, "and makes no glyph bitmaps");
    let Some(drawn) = draw(&mut r, &text_packet(vec![red]), SIDE) else {
        return;
    };
    assert!(
        drawn.chunks_exact(4).all(|p| p[0] > 240),
        "control: the same quad in a sized world paints"
    );
}

#[test]
fn a_second_frame_of_the_same_text_rasterizes_no_new_glyphs() {
    let Some(mut r) = renderer() else { return };
    let command = word("Cache", 8.0, 40.0, 20.0);
    let Some(first) = gpu_alpha(&mut r, &command) else {
        return;
    };
    let after_first = rasterized(&r);
    assert!(after_first > 0);
    let Some(second) = gpu_alpha(&mut r, &command) else {
        return;
    };
    assert_eq!(rasterized(&r), after_first, "a cache hit");
    assert_eq!(first, second, "the same pixels");
    let _ = gpu_alpha(&mut r, &word("Xyz", 8.0, 40.0, 20.0));
    assert!(
        rasterized(&r) > after_first,
        "control: new letters rasterize"
    );
}

#[test]
fn a_full_atlas_evicts_a_page_and_still_draws() {
    let Some(mut r) = renderer() else { return };
    r.glyphs = Some(crate::glyph_atlas::GlyphPass::with_limits(
        r.device(),
        wgpu::TextureFormat::Rgba8Unorm,
        64,
        1,
    ));
    let first = word("WMNO", 2.0, 30.0, 24.0);
    let second = word("QG", 2.0, 30.0, 24.0);
    let Some(_) = gpu_alpha(&mut r, &first) else {
        return;
    };
    let after_first = rasterized(&r);
    let Some(drawn) = gpu_alpha(&mut r, &second) else {
        return;
    };
    assert!(ink_sum(&drawn) > 0, "the evicting frame still draws");
    let diff = mean_diff(&cpu_alpha(&second), &drawn);
    assert!(diff < 0.6, "and draws the CPU's pixels: {diff}");
    assert_eq!(r.glyphs.as_ref().map(|g| g.page_count()), Some(1));
    let before_again = rasterized(&r);
    assert!(before_again > after_first);
    let Some(_) = gpu_alpha(&mut r, &first) else {
        return;
    };
    assert!(
        rasterized(&r) > before_again,
        "the evicted letters rasterize again"
    );
    let Some(_) = gpu_alpha(&mut r, &first) else {
        return;
    };
    let settled = rasterized(&r);
    let _ = gpu_alpha(&mut r, &first);
    assert_eq!(rasterized(&r), settled, "control: resident letters do not");
}

#[test]
fn one_frame_wider_than_the_budget_grows_then_trims_back() {
    let Some(mut r) = renderer() else { return };
    r.glyphs = Some(crate::glyph_atlas::GlyphPass::with_limits(
        r.device(),
        wgpu::TextureFormat::Rgba8Unorm,
        64,
        1,
    ));
    let wide = word("WMNOQGHKRS", 2.0, 30.0, 24.0);
    let Some(drawn) = gpu_alpha(&mut r, &wide) else {
        return;
    };
    assert!(ink_width(&drawn) > 100, "every glyph of the frame draws");
    assert!(r.glyphs.as_ref().is_some_and(|g| g.page_count() > 1));
    let _ = gpu_alpha(&mut r, &word("i", 2.0, 30.0, 24.0));
    assert_eq!(r.glyphs.as_ref().map(|g| g.page_count()), Some(1));
}

#[test]
fn stroke_and_text_commands_draw() {
    let Some(mut r) = renderer() else { return };
    let stroke = DrawCommand::Path {
        segments: vec![
            PathSegment::MoveTo { x: 0.0, y: 8.0 },
            PathSegment::LineTo { x: 16.0, y: 8.0 },
        ],
        fill: None,
        stroke: Some(Stroke {
            color: BLUE,
            width: 2.0,
            dash: vec![],
            dash_offset: 0.0,
            cap: Default::default(),
            join: Default::default(),
        }),
    };
    let text = DrawCommand::Text {
        color_runs: Vec::new(),
        rotation: 0.0,
        text: "W".into(),
        pos: Vec2 { x: 8.0, y: 8.0 },
        size: 12.0,
        color: RED,
        align: TextAlign::Center,
        weight: 400,
        family: orbital_canvas_base::packet::TextFamily::Sans,
        halo: None,
    };
    let Some(rgba) = draw(&mut r, &packet(vec![stroke]), 16) else {
        return;
    };
    assert_eq!(px(&rgba, 16, 3, 8), [0, 0, 255, 255]);
    assert_eq!(px(&rgba, 16, 3, 2)[3], 0, "control: off the stroke");
    let with_text = draw(&mut r, &packet(vec![text]), 16).unwrap();
    assert!(
        with_text.chunks_exact(4).any(|p| p[3] > 200 && p[0] > 200),
        "text ink is drawn"
    );
}

#[test]
fn a_run_moved_by_whole_pixels_reuses_its_layer() {
    let Some(mut r) = renderer() else { return };
    let _font = orbital_canvas_paint::vector::FONT_GENERATION_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _ = draw(&mut r, &packet(vec![filled(4.0, RED)]), 16);
    let Some(moved) = draw(&mut r, &packet(vec![filled(6.0, RED)]), 16) else {
        return;
    };
    assert_eq!(
        r.vector_layer_count(),
        1,
        "scrolled content is not re-rasterized"
    );
    assert_eq!(
        px(&moved, 16, 13, 8),
        [255, 0, 0, 255],
        "drawn at its new place"
    );
    assert_eq!(px(&moved, 16, 5, 8)[3], 0, "and not at its old one");
    let _ = draw(&mut r, &packet(vec![filled(4.5, RED)]), 16);
    assert_eq!(
        r.vector_layer_count(),
        2,
        "control: a half-pixel move is another quantized phase"
    );
}

fn clip_group(clip: Vec<PathSegment>) -> DrawCommand {
    DrawCommand::ClipGroup {
        clip,
        commands: vec![DrawCommand::Quad {
            rect: Rect {
                x: 0.0,
                y: 0.0,
                w: 16.0,
                h: 16.0,
            },
            color: RED,
        }],
    }
}

fn diamond(cx: f32) -> Vec<PathSegment> {
    vec![
        PathSegment::MoveTo { x: cx, y: 2.0 },
        PathSegment::LineTo {
            x: cx + 6.0,
            y: 8.0,
        },
        PathSegment::LineTo { x: cx, y: 14.0 },
        PathSegment::LineTo {
            x: cx - 6.0,
            y: 8.0,
        },
        PathSegment::Close,
    ]
}

#[test]
fn a_rect_clip_on_whole_pixels_draws_its_box_without_a_mask() {
    let Some(mut r) = renderer() else { return };
    let Some(rgba) = draw(
        &mut r,
        &packet(vec![clip_group(rect(4.0, 2.0, 6.0, 10.0))]),
        16,
    ) else {
        return;
    };
    assert_eq!(px(&rgba, 16, 5, 3), [255, 0, 0, 255], "inside the clip");
    assert_eq!(px(&rgba, 16, 3, 3)[3], 0, "left of it");
    assert_eq!(px(&rgba, 16, 10, 3)[3], 0, "right of it");
    assert_eq!(px(&rgba, 16, 5, 12)[3], 0, "below it");
    assert_eq!(r.clip_mask_count(), 0, "a rect needs no mask");
    let _ = draw(
        &mut r,
        &packet(vec![clip_group(rect(4.5, 2.0, 6.0, 10.0))]),
        16,
    );
    assert_eq!(
        r.clip_mask_count(),
        1,
        "control: a rect edge inside a pixel is masked"
    );
}

#[test]
fn a_path_clip_mask_is_reused_when_the_clip_moves_by_whole_pixels() {
    let Some(mut r) = renderer() else { return };
    let Some(first) = draw(&mut r, &packet(vec![clip_group(diamond(7.0))]), 16) else {
        return;
    };
    assert_eq!(px(&first, 16, 7, 8), [255, 0, 0, 255], "inside the diamond");
    assert_eq!(px(&first, 16, 2, 3)[3], 0, "outside its corner");
    let Some(moved) = draw(&mut r, &packet(vec![clip_group(diamond(9.0))]), 16) else {
        return;
    };
    assert_eq!(
        r.clip_mask_count(),
        1,
        "the mask is cached by shape, not place"
    );
    assert_eq!(
        px(&moved, 16, 9, 8),
        [255, 0, 0, 255],
        "inside the moved diamond"
    );
    assert_eq!(px(&moved, 16, 2, 8)[3], 0, "outside the moved diamond");
    let _ = draw(&mut r, &packet(vec![clip_group(diamond(9.5))]), 16);
    assert_eq!(
        r.clip_mask_count(),
        2,
        "control: a sub-pixel move masks anew"
    );
}

#[test]
fn clip_layers_are_kept_across_frames_one_per_group_in_a_frame() {
    let Some(mut r) = renderer() else { return };
    for _ in 0..3 {
        let _ = draw(
            &mut r,
            &packet(vec![clip_group(rect(4.0, 2.0, 6.0, 10.0))]),
            16,
        );
    }
    assert_eq!(r.clip_layer_count(), 1, "one layer serves every frame");
    let two = packet(vec![
        clip_group(rect(0.0, 0.0, 4.0, 4.0)),
        clip_group(rect(8.0, 8.0, 4.0, 4.0)),
    ]);
    let Some(rgba) = draw(&mut r, &two, 16) else {
        return;
    };
    assert_eq!(r.clip_layer_count(), 2, "two groups in one frame need two");
    assert_eq!(px(&rgba, 16, 1, 1), [255, 0, 0, 255]);
    assert_eq!(px(&rgba, 16, 9, 9), [255, 0, 0, 255]);
    assert_eq!(px(&rgba, 16, 6, 6)[3], 0);
}

#[test]
fn a_run_moved_by_a_fraction_of_a_pixel_reuses_its_quantized_layer() {
    let Some(mut r) = renderer() else { return };
    let _font = orbital_canvas_paint::vector::FONT_GENERATION_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _ = draw(&mut r, &packet(vec![filled(4.0, RED)]), 16);
    let _ = draw(&mut r, &packet(vec![filled(4.1, RED)]), 16);
    assert_eq!(
        r.vector_layer_count(),
        1,
        "a 0.1 px move snaps to the same quarter-pixel phase"
    );
    let _ = draw(&mut r, &packet(vec![filled(4.3, RED)]), 16);
    assert_eq!(
        r.vector_layer_count(),
        2,
        "a 0.3 px move is at most one more layer"
    );
    let _ = draw(&mut r, &packet(vec![filled(4.35, RED)]), 16);
    assert_eq!(r.vector_layer_count(), 2, "and 0.35 shares its phase");
    let _ = draw(&mut r, &packet(vec![filled(4.0, BLUE)]), 16);
    assert_eq!(
        r.vector_layer_count(),
        3,
        "control: a different shape still adds a layer"
    );
}

#[test]
fn quantized_placement_stays_within_an_eighth_of_a_pixel() {
    let Some(mut r) = renderer() else { return };
    let _font = orbital_canvas_paint::vector::FONT_GENERATION_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let Some(snapped) = draw(&mut r, &packet(vec![filled(4.1, RED)]), 16) else {
        return;
    };
    assert_eq!(
        px(&snapped, 16, 4, 8),
        [255, 0, 0, 255],
        "edge snapped to 4.0"
    );
    assert_eq!(px(&snapped, 16, 3, 8)[3], 0, "nothing left of the edge");
    let Some(moved) = draw(&mut r, &packet(vec![filled(4.3, RED)]), 16) else {
        return;
    };
    let coverage = px(&moved, 16, 4, 8)[3] as f32 / 255.0;
    assert!(
        (coverage - 0.7).abs() <= 0.125 + 0.01,
        "edge column coverage {coverage} is within 1/8 px of the true 0.7"
    );
    assert_eq!(
        px(&moved, 16, 3, 8)[3],
        0,
        "control: still nothing left of it"
    );
}

fn faded(opacity: f32, commands: Vec<DrawCommand>) -> DrawCommand {
    DrawCommand::Faded {
        opacity,
        scale: 1.0,
        origin: Vec2::default(),
        offset: Vec2::default(),
        commands,
    }
}

fn glow(x: f32) -> DrawCommand {
    DrawCommand::Shadow {
        commands: vec![filled(x, RED)],
        color: [1.0, 0.0, 0.0, 1.0],
        sigma: 1.0,
    }
}

#[test]
fn a_blurred_path_fading_over_five_frames_rasterizes_its_layer_once() {
    let Some(mut r) = renderer() else { return };
    let _font = orbital_canvas_paint::vector::FONT_GENERATION_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut centre = Vec::new();
    for step in 1..=5 {
        let opacity = step as f32 / 10.0;
        let Some(rgba) = draw(&mut r, &packet(vec![faded(opacity, vec![glow(4.0)])]), 16) else {
            return;
        };
        centre.push(px(&rgba, 16, 8, 8)[3] as f32 / 255.0);
        assert_eq!(r.vector_layer_count(), 1, "frame {step} reuses the layer");
    }
    for (step, alpha) in centre.iter().enumerate() {
        let want = (step + 1) as f32 / 10.0;
        assert!(
            (alpha - want).abs() < 0.06,
            "step {step}: {alpha} vs {want}"
        );
    }
}

#[test]
fn control_an_unanimated_path_and_a_changed_one_keep_their_layers() {
    let Some(mut r) = renderer() else { return };
    let _font = orbital_canvas_paint::vector::FONT_GENERATION_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let Some(rgba) = draw(&mut r, &packet(vec![glow(4.0)]), 16) else {
        return;
    };
    assert_eq!(px(&rgba, 16, 8, 8)[3], 255, "an unfaded layer is opaque");
    let _ = draw(&mut r, &packet(vec![faded(0.5, vec![glow(4.0)])]), 16);
    assert_eq!(r.vector_layer_count(), 1, "the fade shares the plain layer");
    let _ = draw(
        &mut r,
        &packet(vec![faded(0.5, vec![filled(4.0, BLUE)])]),
        16,
    );
    assert_eq!(
        r.vector_layer_count(),
        2,
        "different content is a new layer"
    );
}

#[test]
fn a_faded_run_keeps_z_order_between_quads() {
    let Some(mut r) = renderer() else { return };
    let quad = |color| DrawCommand::Quad {
        rect: Rect {
            x: 0.0,
            y: 0.0,
            w: 16.0,
            h: 16.0,
        },
        color,
    };
    let Some(over) = draw(
        &mut r,
        &packet(vec![quad(BLUE), faded(1.0, vec![filled(4.0, RED)])]),
        16,
    ) else {
        return;
    };
    assert_eq!(px(&over, 16, 8, 8), [255, 0, 0, 255]);
    let under = draw(
        &mut r,
        &packet(vec![faded(1.0, vec![filled(4.0, RED)]), quad(BLUE)]),
        16,
    )
    .unwrap();
    assert_eq!(
        px(&under, 16, 8, 8),
        [0, 0, 255, 255],
        "control: quad on top"
    );
}

const GLOW_PX: u32 = 32;

fn premultiplied(straight: &[u8]) -> Vec<u8> {
    straight
        .chunks_exact(4)
        .flat_map(|p| {
            let a = p[3] as f32 / 255.0;
            [
                (p[0] as f32 * a).round() as u8,
                (p[1] as f32 * a).round() as u8,
                (p[2] as f32 * a).round() as u8,
                p[3],
            ]
        })
        .collect()
}

fn cpu_glow(command: &DrawCommand) -> Vec<u8> {
    premultiplied(
        &orbital_canvas_paint::vector::rasterize(&[command], GLOW_PX, GLOW_PX, 2.0, 2.0)
            .expect("cpu layer"),
    )
}

fn max_channel_diff(a: &[u8], b: &[u8]) -> u8 {
    a.iter()
        .zip(b)
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0)
}

fn green_shadow() -> DrawCommand {
    DrawCommand::Shadow {
        commands: vec![DrawCommand::Path {
            segments: rect(5.0, 5.0, 6.0, 6.0),
            fill: Some(RED),
            stroke: None,
        }],
        color: [0.0, 1.0, 0.0, 1.0],
        sigma: 1.5,
    }
}

fn blurred_square() -> DrawCommand {
    DrawCommand::Blurred {
        command: Box::new(filled(5.0, RED)),
        radius: 1.5,
    }
}

fn assert_gpu_glow_matches_cpu(command: DrawCommand) {
    let Some(mut r) = renderer() else { return };
    let _font = orbital_canvas_paint::vector::FONT_GENERATION_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let Some(gpu) = draw(&mut r, &packet(vec![command.clone()]), GLOW_PX) else {
        return;
    };
    assert_eq!(r.gpu_blurred_layer_count(), 1, "blurred on the GPU");
    assert_eq!(r.vector_layer_count(), 1, "cached as one vector layer");
    let cpu = cpu_glow(&command);
    for (name, x, y) in [
        ("centre", 16, 16),
        ("edge", 10, 16),
        ("falloff", 7, 16),
        ("outside", 1, 1),
    ] {
        let (g, c) = (px(&gpu, GLOW_PX, x, y), px(&cpu, GLOW_PX, x, y));
        assert!(
            max_channel_diff(&g, &c) <= 8,
            "{name} ({x},{y}): gpu {g:?} vs cpu {c:?}"
        );
    }
    assert!(px(&gpu, GLOW_PX, 16, 16)[3] > 150, "the centre is lit");
    assert!(
        px(&gpu, GLOW_PX, 7, 16)[3] > 0,
        "the blur reaches past the shape"
    );
    assert_eq!(px(&gpu, GLOW_PX, 1, 1)[3], 0, "far outside stays clear");
    assert!(
        max_channel_diff(&gpu, &cpu) <= 12,
        "every pixel within tolerance of the CPU result"
    );
    let again = draw(&mut r, &packet(vec![command]), GLOW_PX).unwrap();
    assert_eq!(again, gpu, "the cached layer draws the same pixels");
    assert_eq!(
        r.gpu_blurred_layer_count(),
        1,
        "the second frame does not blur again"
    );
}

#[test]
fn a_shadow_is_blurred_on_the_gpu_and_matches_the_cpu_result() {
    assert_gpu_glow_matches_cpu(green_shadow());
}

#[test]
fn a_blurred_command_is_blurred_on_the_gpu_and_matches_the_cpu_result() {
    assert_gpu_glow_matches_cpu(blurred_square());
}

#[test]
fn control_a_plain_path_is_not_blurred_and_a_faded_shadow_still_fades() {
    let Some(mut r) = renderer() else { return };
    let _font = orbital_canvas_paint::vector::FONT_GENERATION_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let Some(plain) = draw(&mut r, &packet(vec![filled(4.0, RED)]), 16) else {
        return;
    };
    assert_eq!(px(&plain, 16, 8, 8), [255, 0, 0, 255]);
    assert_eq!(r.gpu_blurred_layer_count(), 0, "a path uses no blur pass");
    let faded_glow = draw(&mut r, &packet(vec![faded(0.5, vec![green_shadow()])]), 16).unwrap();
    assert_eq!(r.gpu_blurred_layer_count(), 1);
    let full = draw(&mut r, &packet(vec![green_shadow()]), 16).unwrap();
    assert_eq!(
        r.gpu_blurred_layer_count(),
        1,
        "the fade shares the blurred layer"
    );
    let (half, whole) = (
        px(&faded_glow, 16, 8, 8)[3] as f32,
        px(&full, 16, 8, 8)[3] as f32,
    );
    assert!(
        whole > 100.0 && (half - whole * 0.5).abs() < 8.0,
        "{half} vs {whole}"
    );
}

#[test]
fn a_shadow_keeps_its_z_order_between_paths_in_one_run() {
    let Some(mut r) = renderer() else { return };
    let _font = orbital_canvas_paint::vector::FONT_GENERATION_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let glow = DrawCommand::Shadow {
        commands: vec![filled(4.0, RED)],
        color: [0.0, 1.0, 0.0, 1.0],
        sigma: 0.5,
    };
    let Some(over) = draw(&mut r, &packet(vec![glow.clone(), filled(4.0, BLUE)]), 16) else {
        return;
    };
    assert_eq!(
        px(&over, 16, 8, 8),
        [0, 0, 255, 255],
        "the path is over the glow"
    );
    let under = draw(&mut r, &packet(vec![filled(4.0, BLUE), glow]), 16).unwrap();
    assert_eq!(
        px(&under, 16, 8, 8)[1],
        255,
        "control: the glow is over the path"
    );
}

fn scaled(scale: f32, commands: Vec<DrawCommand>) -> DrawCommand {
    DrawCommand::Faded {
        opacity: 1.0,
        scale,
        origin: Vec2 { x: 8.0, y: 8.0 },
        offset: Vec2::default(),
        commands,
    }
}

#[test]
fn a_scaled_layer_is_rasterized_once_per_scale_step() {
    let Some(mut r) = renderer() else { return };
    let _font = orbital_canvas_paint::vector::FONT_GENERATION_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    for scale in [1.05, 1.1, 1.2, 1.3, 1.4] {
        let _ = draw(
            &mut r,
            &packet(vec![scaled(scale, vec![filled(4.0, RED)])]),
            16,
        );
    }
    assert_eq!(
        r.vector_layer_count(),
        1,
        "one layer serves every scale inside its step"
    );
    let _ = draw(
        &mut r,
        &packet(vec![scaled(1.5, vec![filled(4.0, RED)])]),
        16,
    );
    assert_eq!(
        r.vector_layer_count(),
        2,
        "control: the next step is rasterized once more"
    );
}

#[test]
fn a_scaled_layer_lands_where_its_scale_puts_it() {
    let Some(mut r) = renderer() else { return };
    let _font = orbital_canvas_paint::vector::FONT_GENERATION_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let Some(grown) = draw(
        &mut r,
        &packet(vec![scaled(1.25, vec![filled(4.0, RED)])]),
        16,
    ) else {
        return;
    };
    assert_eq!(px(&grown, 16, 8, 8), [255, 0, 0, 255], "the middle");
    assert!(
        px(&grown, 16, 3, 8)[3] > 150,
        "the left edge moved out from 4 to 3: {:?}",
        px(&grown, 16, 3, 8)
    );
    assert_eq!(
        px(&grown, 16, 4, 8),
        [255, 0, 0, 255],
        "full just inside it"
    );
    assert!(px(&grown, 16, 12, 8)[3] > 150, "the right edge reaches 13");
    assert_eq!(px(&grown, 16, 1, 8)[3], 0, "nothing past the scaled edge");
    assert_eq!(px(&grown, 16, 14, 8)[3], 0, "nor on the other side");
    let Some(rest) = draw(
        &mut r,
        &packet(vec![scaled(1.0, vec![filled(4.0, RED)])]),
        16,
    ) else {
        return;
    };
    assert_eq!(
        px(&rest, 16, 3, 8)[3],
        0,
        "control: at scale 1 the edge stays at 4"
    );
    assert_eq!(px(&rest, 16, 4, 8), [255, 0, 0, 255]);
}

#[test]
fn a_scaled_layer_larger_than_the_target_is_still_rasterized_once_per_step() {
    let Some(mut r) = renderer() else { return };
    let _font = orbital_canvas_paint::vector::FONT_GENERATION_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let wide = || DrawCommand::Path {
        segments: rect(-40.0, -40.0, 96.0, 96.0),
        fill: Some(RED),
        stroke: None,
    };
    let mut last = draw(&mut r, &packet(vec![scaled(2.1, vec![wide()])]), 16);
    let held = r.vector_layer_keys();
    for scale in [2.3, 2.6, 2.8] {
        last = draw(&mut r, &packet(vec![scaled(scale, vec![wide()])]), 16);
    }
    assert_eq!(held.len(), 1);
    assert_eq!(
        r.vector_layer_keys(),
        held,
        "what the step's lowest scale shows serves every scale inside it"
    );
    let Some(rgba) = last else { return };
    assert_eq!(
        px(&rgba, 16, 0, 0),
        [255, 0, 0, 255],
        "it covers the corner"
    );
    assert_eq!(px(&rgba, 16, 15, 15), [255, 0, 0, 255], "and the far one");
    let _ = draw(&mut r, &packet(vec![scaled(3.0, vec![wide()])]), 16);
    assert!(
        r.vector_layer_keys().iter().any(|k| !held.contains(k)),
        "control: the next step is rasterized anew"
    );
}
