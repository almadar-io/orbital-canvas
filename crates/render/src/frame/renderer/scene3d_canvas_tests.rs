use super::*;

#[test]
fn authored_3d_canvas_scene_paints_box_pixels_on_the_gpu() {
    let Some(mut renderer) = super::svg_sprite_tests::renderer() else {
        return;
    };
    let node = serde_json::json!({
        "type": "canvas", "mode": "3d", "backgroundColor": "#000000",
        "camera": {"pos": {"x": 0, "y": 5, "z": 2},
            "target": {"x": 0, "y": 0, "z": 0}, "fov": 60},
        "drawables": [{"type": "draw-mesh", "shape": "box",
            "position": {"x": 0, "y": 0, "z": 0},
            "material": {"kind": "basic", "side": "double", "color": "#ff0000"}}]
    });
    let mapped = orbital_canvas_patterns::scene::map_scene(&node, Some((64.0, 64.0)))
        .expect("authored 3D scene");
    let pixels =
        super::svg_sprite_tests::draw(&mut renderer, &mapped.packet, 64).expect("GPU readback");
    let red = pixels
        .chunks_exact(4)
        .filter(|pixel| pixel[0] > 150 && pixel[1] < 60 && pixel[2] < 60)
        .count();
    assert!(
        red > 20,
        "authored box must cover visible red pixels: {red}"
    );
}

fn embedded(extra: serde_json::Value, theme: Option<serde_json::Value>) -> serde_json::Value {
    let mut sphere = serde_json::json!({
        "type": "draw-scene3d", "id": "orb", "position": {"x": 16, "y": 16},
        "width": 32, "height": 32,
        "scene": {"camera": {"mode": "front", "zoom": 2.4, "fov": 40, "target": {"x": 0, "y": 0, "z": 0.5}},
            "drawables": [{"type": "draw-mesh", "shape": "sphere",
            "position": {"x": 0, "y": 0},
            "material": {"kind": "standard", "color": "#ff0000"}}]}
    });
    for (key, value) in extra.as_object().expect("fields") {
        sphere[key] = value.clone();
    }
    let mut canvas = serde_json::json!({
        "type": "canvas", "projection": "free", "scale": 1.0,
        "backgroundColor": "#00000000", "drawables": [sphere]
    });
    if let Some(theme) = theme {
        canvas["canvasTheme"] = theme;
    }
    canvas
}

fn map_at(
    node: &serde_json::Value,
    epoch_ms: f64,
) -> orbital_canvas_base::packet::GameRenderPacket {
    orbital_canvas_map::game_canvas_2d::from_canvas_node_in_viewport_at(node, 64.0, 64.0, epoch_ms)
        .expect("a 2D scene with an embedded 3D scene")
}

fn painted(pixels: &[u8]) -> Vec<(u32, u32)> {
    pixels
        .chunks_exact(4)
        .enumerate()
        .filter(|(_, p)| p[3] > 0)
        .map(|(i, _)| ((i % 64) as u32, (i / 64) as u32))
        .collect()
}

fn bounds(points: &[(u32, u32)]) -> (u32, u32, u32, u32) {
    points
        .iter()
        .fold((u32::MAX, u32::MAX, 0, 0), |b, &(x, y)| {
            (b.0.min(x), b.1.min(y), b.2.max(x), b.3.max(y))
        })
}

#[test]
fn an_embedded_sphere_paints_inside_its_rect_and_nowhere_else() {
    let Some(mut r) = super::svg_sprite_tests::renderer() else {
        return;
    };
    let packet = map_at(&embedded(serde_json::json!({}), None), 0.0);
    let Some(pixels) = super::svg_sprite_tests::draw(&mut r, &packet, 64) else {
        return;
    };
    let drawn = painted(&pixels);
    assert!(
        drawn.len() > 100,
        "the sphere covers pixels: {}",
        drawn.len()
    );
    assert!(
        drawn
            .iter()
            .all(|&(x, y)| (16..48).contains(&x) && (16..48).contains(&y)),
        "nothing outside the rect: {:?}",
        bounds(&drawn)
    );
    let centre = super::svg_sprite_tests::px(&pixels, 64, 32, 32);
    assert!(
        centre[3] > 200 && centre[0] > centre[1] + 40,
        "a lit red sphere {centre:?}"
    );
}

#[test]
fn a_palette_name_colours_the_embedded_mesh_in_the_themes_hand() {
    let Some(mut r) = super::svg_sprite_tests::renderer() else {
        return;
    };
    let mut node = embedded(
        serde_json::json!({}),
        Some(serde_json::json!({"palette": {"brand": "#00ff00"}})),
    );
    node["drawables"][0]["scene"]["drawables"][0]["material"]["color"] = "brand".into();
    let Some(pixels) = super::svg_sprite_tests::draw(&mut r, &map_at(&node, 0.0), 64) else {
        return;
    };
    let centre = super::svg_sprite_tests::px(&pixels, 64, 32, 32);
    assert!(
        centre[3] > 200 && centre[1] > centre[0] + 40 && centre[1] > centre[2] + 40,
        "the brand colour is green {centre:?}"
    );
}

#[test]
fn the_page_scroll_moves_the_embedded_scene_up_with_the_page() {
    let Some(mut r) = super::svg_sprite_tests::renderer() else {
        return;
    };
    let packet = map_at(&embedded(serde_json::json!({}), None), 0.0);
    let Some(rest) = super::svg_sprite_tests::draw(&mut r, &packet, 64) else {
        return;
    };
    let Some(scrolled) = super::svg_sprite_tests::draw(&mut r, &packet.clone().scrolled(16.0), 64)
    else {
        return;
    };
    let (a, b) = (bounds(&painted(&rest)), bounds(&painted(&scrolled)));
    assert_eq!((b.0, b.2), (a.0, a.2), "the same columns");
    assert_eq!(
        (b.1 + 16, b.3 + 16),
        (a.1, a.3),
        "sixteen rows up: {a:?} -> {b:?}"
    );
}

fn spinning_box() -> serde_json::Value {
    embedded(
        serde_json::json!({"scene": {"camera": {"mode": "front", "zoom": 2.4, "fov": 40, "target": {"x": 0, "y": 0, "z": 0.5}},
            "drawables": [{"type": "draw-mesh", "shape": "box",
            "position": {"x": 0, "y": 0},
            "material": {"kind": "standard", "color": "#ff0000"},
            "animation": {"durationMs": 1000, "keyframes": [
                {"at": 0, "rotateY": 0}, {"at": 1, "rotateY": 6.0}]}}]}}),
        None,
    )
}

#[test]
fn a_time_animation_turns_the_embedded_object_between_frames() {
    let Some(mut r) = super::svg_sprite_tests::renderer() else {
        return;
    };
    let draw = |r: &mut SharedDeviceRenderer, ms: f64| {
        super::svg_sprite_tests::draw(r, &map_at(&spinning_box(), ms), 64)
    };
    let (Some(a), Some(b), Some(again)) = (
        draw(&mut r, 1000.0),
        draw(&mut r, 1125.0),
        draw(&mut r, 1000.0),
    ) else {
        return;
    };
    assert_ne!(a, b, "a quarter turn later the silhouette differs");
    assert_eq!(a, again, "control: the same instant paints the same pixels");
    let still = embedded(serde_json::json!({}), None);
    let (Some(c), Some(d)) = (
        super::svg_sprite_tests::draw(&mut r, &map_at(&still, 1000.0), 64),
        super::svg_sprite_tests::draw(&mut r, &map_at(&still, 1125.0), 64),
    ) else {
        return;
    };
    assert_eq!(
        c, d,
        "control: a scene that does not move is the same at both"
    );
}

#[test]
fn a_scene_without_an_embedded_scene_makes_no_target_and_an_embedded_one_reuses_its_target() {
    let Some(mut r) = super::svg_sprite_tests::renderer() else {
        return;
    };
    let plain = orbital_canvas_base::packet::GameRenderPacket {
        pattern: "canvas-2d".into(),
        width: 64.0,
        height: 64.0,
        draw_commands: vec![DrawCommand::Quad {
            rect: orbital_canvas_base::packet::Rect {
                x: 8.0,
                y: 8.0,
                w: 8.0,
                h: 8.0,
            },
            color: [1.0, 0.0, 0.0, 1.0],
        }],
        ..Default::default()
    };
    if super::svg_sprite_tests::draw(&mut r, &plain, 64).is_none() {
        return;
    }
    assert_eq!(
        (r.targets.entry_count(), r.targets.textures_created()),
        (0, 0)
    );
    let mut created = Vec::new();
    for ms in [1000.0, 1100.0, 1200.0, 1300.0, 1400.0] {
        if super::svg_sprite_tests::draw(&mut r, &map_at(&spinning_box(), ms), 64).is_none() {
            return;
        }
        created.push((r.targets.entry_count(), r.targets.textures_created()));
    }
    assert!(
        created.iter().all(|(entries, _)| *entries == 1),
        "one target: {created:?}"
    );
    assert_eq!(
        created[1].1, created[4].1,
        "no texture is created after the first two frames: {created:?}"
    );
    assert!(
        created[4].1 <= 2,
        "one target is two textures at most: {created:?}"
    );
}
