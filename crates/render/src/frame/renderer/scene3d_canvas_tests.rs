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
