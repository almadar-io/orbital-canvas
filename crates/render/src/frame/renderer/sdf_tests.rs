use super::svg_sprite_tests::{draw, px, renderer};
use super::*;
use orbital_canvas_base::packet::{SdfKind, SdfMaterial};

const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

fn shape(kind: SdfKind, rect: Rect) -> DrawCommand {
    DrawCommand::Sdf {
        kind,
        rect,
        corners: [0.0; 4],
        corner_kind: 0,
        arc: [0.0; 3],
        polygon: [0.0; 2],
        fill: RED,
        stroke: None,
        shadow: None,
        rotation: 0.0,
        opacity: 1.0,
        material: None,
    }
}

fn full() -> Rect {
    Rect {
        x: 0.0,
        y: 0.0,
        w: 64.0,
        h: 64.0,
    }
}

fn packet(commands: Vec<DrawCommand>, time_ms: f64, pointer: Option<Vec2>) -> GameRenderPacket {
    GameRenderPacket {
        pattern: "canvas-2d".into(),
        width: 64.0,
        height: 64.0,
        draw_commands: commands,
        time_ms,
        pointer,
        ..Default::default()
    }
}

fn painted(rgba: &[u8], x: u32, y: u32) -> bool {
    px(rgba, 64, x, y)[3] > 200
}

fn rounded(radius: f32, kind: u32) -> DrawCommand {
    let mut command = shape(
        SdfKind::Rect,
        Rect {
            x: 8.0,
            y: 8.0,
            w: 48.0,
            h: 48.0,
        },
    );
    if let DrawCommand::Sdf {
        corners,
        corner_kind,
        ..
    } = &mut command
    {
        *corners = [radius; 4];
        *corner_kind = kind;
    }
    command
}

fn arc(sweep: f32) -> DrawCommand {
    let mut command = shape(SdfKind::Arc, full());
    if let DrawCommand::Sdf { arc, .. } = &mut command {
        *arc = [0.0, sweep, 8.0];
    }
    command
}

fn polygon(sides: f32, ratio: f32) -> DrawCommand {
    let mut command = shape(SdfKind::Polygon, full());
    if let DrawCommand::Sdf { polygon, .. } = &mut command {
        *polygon = [sides, ratio];
    }
    command
}

fn with_material(wgsl: &str) -> DrawCommand {
    let mut command = shape(SdfKind::Rect, full());
    if let DrawCommand::Sdf { material, .. } = &mut command {
        *material = Some(SdfMaterial {
            wgsl: wgsl.into(),
            params: vec![],
            live: true,
            since: 0.0,
            anchor_ms: None,
            targets: Vec::new(),
        });
    }
    command
}

#[test]
fn a_rounded_rect_paints_inside_and_not_outside_its_corners() {
    let Some(mut r) = renderer() else { return };
    let Some(rgba) = draw(&mut r, &packet(vec![rounded(16.0, 0)], 0.0, None), 64) else {
        return;
    };
    assert!(painted(&rgba, 32, 32), "the middle");
    assert!(painted(&rgba, 32, 9), "along an edge");
    assert!(!painted(&rgba, 9, 9), "outside the rounded corner");
    let Some(square) = draw(&mut r, &packet(vec![rounded(0.0, 0)], 0.0, None), 64) else {
        return;
    };
    assert!(
        painted(&square, 9, 9),
        "control: no radius keeps the corner"
    );
}

#[test]
fn a_bevel_corner_is_cut_straight_where_a_round_one_still_paints() {
    let Some(mut r) = renderer() else { return };
    let Some(bevel) = draw(&mut r, &packet(vec![rounded(16.0, 1)], 0.0, None), 64) else {
        return;
    };
    let Some(round) = draw(&mut r, &packet(vec![rounded(16.0, 0)], 0.0, None), 64) else {
        return;
    };
    assert!(!painted(&bevel, 13, 13), "the chamfer cuts it off");
    assert!(
        painted(&round, 13, 13),
        "control: the round corner keeps it"
    );
    assert!(painted(&bevel, 32, 32), "the middle");
}

#[test]
fn an_arc_of_sweep_pi_paints_half_a_ring() {
    let Some(mut r) = renderer() else { return };
    let Some(half) = draw(
        &mut r,
        &packet(vec![arc(std::f32::consts::PI)], 0.0, None),
        64,
    ) else {
        return;
    };
    assert!(painted(&half, 32, 60), "the lower half of the ring");
    assert!(!painted(&half, 32, 3), "not the upper half");
    assert!(!painted(&half, 32, 32), "not the hole");
    let Some(full_ring) = draw(
        &mut r,
        &packet(vec![arc(std::f32::consts::TAU)], 0.0, None),
        64,
    ) else {
        return;
    };
    assert!(
        painted(&full_ring, 32, 3),
        "control: a full sweep has the top"
    );
}

#[test]
fn an_eight_point_star_has_tips_and_notches_a_regular_polygon_does_not() {
    let Some(mut r) = renderer() else { return };
    let Some(star) = draw(&mut r, &packet(vec![polygon(8.0, 0.7)], 0.0, None), 64) else {
        return;
    };
    assert!(painted(&star, 32, 32), "the middle");
    assert!(painted(&star, 32, 6), "the upward tip");
    assert!(!painted(&star, 43, 5), "between two tips");
    let Some(octagon) = draw(&mut r, &packet(vec![polygon(8.0, 1.0)], 0.0, None), 64) else {
        return;
    };
    assert!(
        painted(&octagon, 43, 5),
        "control: the regular octagon fills it"
    );
}

#[test]
fn a_material_reading_time_changes_pixels_between_two_times() {
    let Some(mut r) = renderer() else { return };
    let wgsl =
        "fn shade(px: Pixel) -> vec4<f32> { return vec4<f32>(fract(px.time), 0.0, 0.0, 1.0); }";
    let at = |r: &mut SharedDeviceRenderer, ms: f64| {
        draw(r, &packet(vec![with_material(wgsl)], ms, None), 64)
    };
    let (Some(start), Some(later)) = (at(&mut r, 0.0), at(&mut r, 500.0)) else {
        return;
    };
    assert!(px(&start, 64, 32, 32)[0] < 5);
    let red = px(&later, 64, 32, 32)[0];
    assert!((100..160).contains(&red), "{red}");
    let Some(again) = at(&mut r, 0.0) else { return };
    assert_eq!(
        px(&again, 64, 32, 32),
        px(&start, 64, 32, 32),
        "control: the same time paints the same pixel"
    );
}

#[test]
fn a_material_reading_the_pointer_lights_only_when_it_is_over_the_shape() {
    let Some(mut r) = renderer() else { return };
    let wgsl = "fn shade(px: Pixel) -> vec4<f32> { let over = px.pointer.x > 0.0 && px.pointer.x < px.size.x; return vec4<f32>(select(0.0, 1.0, over), 0.0, 0.0, 1.0); }";
    let at = |r: &mut SharedDeviceRenderer, pointer: Option<Vec2>| {
        draw(r, &packet(vec![with_material(wgsl)], 0.0, pointer), 64)
    };
    let (Some(over), Some(away)) = (
        at(&mut r, Some(Vec2 { x: 10.0, y: 10.0 })),
        at(&mut r, None),
    ) else {
        return;
    };
    assert_eq!(px(&over, 64, 32, 32)[0], 255);
    assert_eq!(px(&away, 64, 32, 32)[0], 0, "control: no pointer");
}

#[test]
fn a_material_reads_the_page_scroll() {
    let Some(mut r) = renderer() else { return };
    let wgsl = "fn shade(px: Pixel) -> vec4<f32> { return vec4<f32>(clamp(px.scroll / 510.0, 0.0, 1.0), 0.0, 0.0, 1.0); }";
    let at = |r: &mut SharedDeviceRenderer, scroll: f32| {
        let mut p = packet(vec![with_material(wgsl)], 0.0, None);
        p.scroll = scroll;
        draw(r, &p, 64)
    };
    let (Some(half), Some(top)) = (at(&mut r, 255.0), at(&mut r, 0.0)) else {
        return;
    };
    assert!(
        (126..=129).contains(&px(&half, 64, 32, 32)[0]),
        "{:?}",
        px(&half, 64, 32, 32)
    );
    assert_eq!(px(&top, 64, 32, 32)[0], 0, "control: at the top");
}

#[test]
fn sdf_shapes_keep_z_order_with_quads() {
    let Some(mut r) = renderer() else { return };
    let blue_quad = DrawCommand::Quad {
        rect: full(),
        color: [0.0, 0.0, 1.0, 1.0],
    };
    let Some(over) = draw(
        &mut r,
        &packet(vec![blue_quad.clone(), rounded(0.0, 0)], 0.0, None),
        64,
    ) else {
        return;
    };
    assert_eq!(px(&over, 64, 32, 32), [255, 0, 0, 255], "the shape on top");
    let Some(under) = draw(
        &mut r,
        &packet(vec![rounded(0.0, 0), blue_quad], 0.0, None),
        64,
    ) else {
        return;
    };
    assert_eq!(
        px(&under, 64, 32, 32),
        [0, 0, 255, 255],
        "control: quad last"
    );
}

fn clipped_scene(clip: Option<serde_json::Value>) -> GameRenderPacket {
    let mut group = serde_json::json!({"type": "draw-group", "position": {"x": 0, "y": 0}, "items": [
        {"type": "draw-shape", "shape": "rect", "anchor": "top-left", "position": {"x": 0, "y": 0},
         "width": 64, "height": 64, "fill": "#ff0000", "corners": {"radius": [0, 0, 0, 0]}}]});
    if let Some(clip) = clip {
        group["clip"] = clip;
    }
    orbital_canvas_map::game_canvas_2d::from_props(&serde_json::json!({
        "width": 64, "height": 64, "scale": 1.0, "projection": "free", "bgColor": "#00000000",
        "drawables": [group]
    }))
    .unwrap()
}

#[test]
fn a_rect_clip_cuts_an_sdf_shape_at_its_edges_and_rounds_its_corners() {
    let Some(mut r) = renderer() else { return };
    let clip = serde_json::json!({"x": 8, "y": 8, "width": 48, "height": 48, "radius": 16});
    let Some(clipped) = draw(&mut r, &clipped_scene(Some(clip)), 64) else {
        return;
    };
    assert!(painted(&clipped, 32, 32), "inside the clip the shape shows");
    assert!(!painted(&clipped, 4, 32), "left of the clip it is cut");
    assert!(!painted(&clipped, 60, 32), "right of the clip it is cut");
    assert!(
        painted(&clipped, 32, 10),
        "just inside the top edge it shows"
    );
    assert!(
        !painted(&clipped, 9, 9),
        "the rounded corner of the clip cuts the shape"
    );
    let Some(free) = draw(&mut r, &clipped_scene(None), 64) else {
        return;
    };
    assert!(
        painted(&free, 4, 32) && painted(&free, 9, 9),
        "control: no clip, no cut"
    );
}

fn two_circles(radius: Option<f32>) -> GameRenderPacket {
    let circle = |x: f32, fill: [f32; 4]| DrawCommand::Sdf {
        kind: SdfKind::Ellipse,
        rect: Rect {
            x,
            y: 20.0,
            w: 24.0,
            h: 24.0,
        },
        corners: [0.0; 4],
        corner_kind: 0,
        arc: [0.0; 3],
        polygon: [0.0; 2],
        fill,
        stroke: None,
        shadow: None,
        rotation: 0.0,
        opacity: 1.0,
        material: None,
    };
    let (a, b) = (circle(4.0, RED), circle(36.0, [0.0, 0.0, 1.0, 1.0]));
    let commands = match radius {
        Some(radius) => vec![DrawCommand::SdfBlend {
            first: Box::new(a),
            second: Box::new(b),
            radius,
        }],
        None => vec![a, b],
    };
    packet(commands, 0.0, None)
}

#[test]
fn a_smooth_union_melts_two_apart_circles_into_one_neck_and_mixes_their_colours() {
    let Some(mut r) = renderer() else { return };
    let Some(blended) = draw(&mut r, &two_circles(Some(24.0)), 64) else {
        return;
    };
    assert!(painted(&blended, 32, 32), "the neck bridges the 8px gap");
    assert!(
        painted(&blended, 16, 32) && painted(&blended, 48, 32),
        "both shapes are there"
    );
    assert!(!painted(&blended, 32, 8), "above the neck stays empty");
    let near_a = px(&blended, 64, 12, 32);
    let near_b = px(&blended, 64, 52, 32);
    assert!(
        near_a[0] > 200 && near_a[2] < 60,
        "the first circle is red: {near_a:?}"
    );
    assert!(
        near_b[2] > 200 && near_b[0] < 60,
        "the second circle is blue: {near_b:?}"
    );
    let neck = px(&blended, 64, 32, 32);
    assert!(
        neck[0] > 60 && neck[2] > 60,
        "the neck mixes both fills: {neck:?}"
    );
    let Some(apart) = draw(&mut r, &two_circles(None), 64) else {
        return;
    };
    assert!(
        !painted(&apart, 32, 32),
        "control: two separate circles leave the gap empty"
    );
    assert!(painted(&apart, 16, 32) && painted(&apart, 48, 32));
}

#[test]
fn a_frames_transient_buffers_are_released_after_its_submit_and_the_next_frame_still_draws() {
    let Some(mut r) = renderer() else { return };
    let frame = packet(vec![shape(SdfKind::Rect, full())], 0.0, None);
    let Some(first) = draw(&mut r, &frame, 64) else {
        return;
    };
    assert_eq!(
        r.transient_len(),
        0,
        "every per-frame buffer is destroyed once submitted"
    );
    let Some(second) = draw(&mut r, &frame, 64) else {
        return;
    };
    assert_eq!(r.transient_len(), 0);
    assert_eq!(
        px(&first, 64, 32, 32),
        px(&second, 64, 32, 32),
        "a released frame does not disturb the next"
    );
    assert!(px(&second, 64, 32, 32)[0] > 200);
}

#[test]
fn releasing_from_a_mark_keeps_the_buffers_an_outer_frame_still_needs() {
    let Some(r) = renderer() else { return };
    let buffer = |label| {
        r.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: 16,
            usage: wgpu::BufferUsages::VERTEX,
            mapped_at_creation: false,
        })
    };
    r.keep(&buffer("outer"));
    let mark = r.transient_len();
    r.keep(&buffer("inner"));
    r.release_transient(mark);
    assert_eq!(
        r.transient_len(),
        1,
        "the nested frame releases only its own"
    );
    r.release_transient(0);
    assert_eq!(r.transient_len(), 0);
}

fn coloured(fill: [f32; 4]) -> DrawCommand {
    let mut command = shape(SdfKind::Rect, full());
    if let DrawCommand::Sdf { fill: f, .. } = &mut command {
        *f = fill;
    }
    command
}

fn pool_len(r: &SharedDeviceRenderer) -> usize {
    r.sdf.as_ref().map_or(0, |pass| pass.pool_len())
}

#[test]
fn a_second_frame_reuses_the_pooled_buffers_and_draws_its_own_data() {
    let Some(mut r) = renderer() else { return };
    let blue = [0.0, 0.0, 1.0, 1.0];
    let Some(first) = draw(&mut r, &packet(vec![coloured(RED)], 0.0, None), 64) else {
        return;
    };
    let after_first = pool_len(&r);
    let Some(second) = draw(&mut r, &packet(vec![coloured(blue)], 0.0, None), 64) else {
        return;
    };
    assert!(px(&first, 64, 32, 32)[0] > 200 && px(&first, 64, 32, 32)[2] < 50);
    assert!(
        px(&second, 64, 32, 32)[2] > 200 && px(&second, 64, 32, 32)[0] < 50,
        "the second frame paints its own colour, not the stale one"
    );
    assert_eq!(after_first, 1);
    assert_eq!(pool_len(&r), after_first, "no growth between frames");
}

#[test]
fn control_a_frame_with_fewer_shapes_shrinks_the_pool_on_the_next_frame() {
    let Some(mut r) = renderer() else { return };
    let many = vec![coloured(RED); crate::sdf::MAX_INSTANCES + 1];
    if draw(&mut r, &packet(many, 0.0, None), 64).is_none() {
        return;
    }
    assert_eq!(pool_len(&r), 2, "more than one batch needs two buffers");
    let one = packet(vec![coloured(RED)], 0.0, None);
    if draw(&mut r, &one, 64).is_none() || draw(&mut r, &one, 64).is_none() {
        return;
    }
    assert_eq!(pool_len(&r), 1);
}

#[test]
fn edge_two_runs_in_one_frame_get_two_pooled_buffers() {
    let Some(mut r) = renderer() else { return };
    let left = "fn shade(px: Pixel) -> vec4<f32> { return vec4<f32>(1.0, 0.0, 0.0, 1.0); }";
    let right = "fn shade(px: Pixel) -> vec4<f32> { return vec4<f32>(0.0, 0.0, 1.0, 1.0); }";
    let half = |wgsl: &str, x: f32| {
        let mut command = with_material(wgsl);
        if let DrawCommand::Sdf { rect, .. } = &mut command {
            *rect = Rect {
                x,
                y: 0.0,
                w: 32.0,
                h: 64.0,
            };
        }
        command
    };
    let frame = packet(vec![half(left, 0.0), half(right, 32.0)], 0.0, None);
    let Some(out) = draw(&mut r, &frame, 64) else {
        return;
    };
    assert!(px(&out, 64, 16, 32)[0] > 200 && px(&out, 64, 16, 32)[2] < 50);
    assert!(px(&out, 64, 48, 32)[2] > 200 && px(&out, 64, 48, 32)[0] < 50);
    assert!(pool_len(&r) >= 2);
}
