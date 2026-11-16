use super::*;
use crate::learning::shape::ShapeType;
use orbital_canvas_base::packet::{DrawCommand, PathSegment};
use serde_json::json;

fn of(shapes: &[Shape], ty: ShapeType) -> Vec<&Shape> {
    shapes.iter().filter(|s| s.ty == ty).collect()
}

fn near(a: Option<f32>, b: f32) -> bool {
    a.is_some_and(|a| (a - b).abs() < 1e-3)
}

/// `std-physics-incline` at its config defaults (600x400, ramp 0.3/0.55 -> 0.87/0.9,
/// vectorScale 40, gravity 0.5, friction 0.05): the resolved props its `InclineSim`
/// renders on INIT, with the ramp angle computed as the lolo does.
fn incline() -> Value {
    let (x1, y1, x2, y2) = (180.0_f64, 220.0_f64, 522.0_f64, 360.0_f64);
    let (dx, dy) = (x2 - x1, y2 - y1);
    let len = dx.hypot(dy);
    let (ux, uy) = (dx / len, dy / len);
    let angle = dy.atan2(dx).to_degrees();
    let n_mag = 0.5 * 40.0 * angle.to_radians().cos();
    let f_mag = 0.05 * 40.0 * 10.0;
    let label = format!("{}°", (angle * 10.0).round() / 10.0);
    json!({
        "type": "physics-canvas", "width": 600, "height": 400, "backgroundColor": "",
        "bodies": [{"id": "block", "x": x1, "y": y1, "radius": 16, "color": "series-1", "label": "", "vx": 0, "vy": 0, "fx": 0, "fy": 0}],
        "sceneObjects": [{"kind": "ramp", "x1": x1, "y1": y1, "x2": x2, "y2": y2, "color": "ink", "fill": "fill", "label": label}],
        "trails": [{"id": "block", "points": [{"x": x1, "y": y1}], "color": "series-1"}],
        "vectors": [
            {"body": "block", "dx": 0, "dy": 0.5 * 40.0, "color": "series-5", "label": "mg"},
            {"body": "block", "dx": uy * n_mag, "dy": -ux * n_mag, "color": "series-6", "label": "N"},
            {"body": "block", "dx": -ux * f_mag, "dy": -uy * f_mag, "color": "series-7", "label": "f"}
        ],
        "angles": [{"x": x2, "y": y2, "from": 180, "to": angle + 180.0, "radius": 22, "color": "series-2", "label": label}],
        "showVelocity": true, "showForces": false, "velocityScale": 6,
        "readouts": [{"label": "angle", "value": label}, {"label": "progress", "value": "0%"}]
    })
}

/// `std-physics-spring` at its config defaults: the wall fixture, the dashed
/// equilibrium line in `shapes`, a spring constraint from the anchor body to the mass.
fn spring() -> Value {
    json!({
        "type": "physics-canvas", "width": 600, "height": 400,
        "bodies": [
            {"id": "anchor", "x": 300, "y": 200, "radius": 6, "color": "ink", "label": "", "vx": 0, "vy": 0, "fx": 0, "fy": 0},
            {"id": "mass", "x": 420, "y": 200, "radius": 18, "color": "series-1", "label": "", "vx": 0, "vy": 0, "fx": 0, "fy": 0}
        ],
        "constraints": [{"from": "anchor", "to": "mass", "kind": "spring", "color": "ink"}],
        "sceneObjects": [{"kind": "wall", "x": 300, "y1": 140, "y2": 260, "color": "ink", "label": "wall"}],
        "shapes": [{"type": "line", "x1": 300, "y1": 40, "x2": 300, "y2": 360, "dash": "dashed", "color": "guide", "label": "equilibrium"}],
        "showVelocity": true, "showForces": false, "velocityScale": 10
    })
}

/// `std-physics-magnetic`: a `field` of kind `into` over 90% of the canvas.
fn magnetic() -> Value {
    json!({
        "type": "physics-canvas", "width": 600, "height": 400,
        "bodies": [{"id": "q", "x": 300, "y": 200, "radius": 8, "color": "series-1", "vx": 3, "vy": 0}],
        "field": {"kind": "into", "spacing": 60, "x": 30, "y": 20, "width": 540, "height": 360},
        "showVelocity": true, "showForces": false, "velocityScale": 8
    })
}

/// `std-learn-live-sim` at its config defaults: three free balls with velocity arrows.
fn live_sim() -> Value {
    json!({
        "type": "physics-canvas", "width": 600, "height": 400, "backgroundColor": "",
        "bodies": [
            {"id": "ball-a", "x": 100, "y": 60, "radius": 14, "color": "series-1", "vx": 4, "vy": 0},
            {"id": "ball-b", "x": 260, "y": 90, "radius": 18, "color": "series-2", "vx": -3, "vy": 2},
            {"id": "ball-c", "x": 420, "y": 40, "radius": 11, "color": "series-3", "vx": 2, "vy": -1}
        ],
        "showVelocity": true, "showForces": false, "velocityScale": 8
    })
}

#[test]
fn live_sim_draws_solid_body_circles_with_series_velocity_arrows() {
    let shapes = derive_shapes(&live_sim(), 600.0, 400.0);
    let circles = of(&shapes, ShapeType::Circle);
    assert_eq!(circles.len(), 3);
    assert_eq!(circles[1].id.as_deref(), Some("ball-b"));
    assert_eq!(circles[1].radius, Some(18.0));
    assert_eq!(circles[1].fill_style.as_deref(), Some("solid"));
    assert_eq!(circles[1].fill.as_deref(), Some("series-2"));
    let arrows = of(&shapes, ShapeType::Arrow);
    assert_eq!(arrows.len(), 3);
    assert!(
        near(arrows[0].x2, 100.0 + 4.0 * 8.0) && near(arrows[0].y2, 60.0),
        "{:?}",
        arrows[0]
    );
    assert_eq!(arrows[0].color.as_deref(), Some("series-3"));
    assert_eq!(arrows[0].stroke.as_deref(), Some("bold"));
}

#[test]
fn velocity_arrows_follow_showvelocity_and_skip_zero_velocity_with_a_paired_control() {
    let mut off = live_sim();
    off["showVelocity"] = json!(false);
    assert!(of(&derive_shapes(&off, 600.0, 400.0), ShapeType::Arrow).is_empty());
    let mut still = live_sim();
    still["bodies"][0]["vx"] = json!(0);
    assert_eq!(
        of(&derive_shapes(&still, 600.0, 400.0), ShapeType::Arrow).len(),
        2
    );
}

#[test]
fn force_arrows_use_series_5_and_the_force_scale_only_when_enabled() {
    let mut p = live_sim();
    p["bodies"][0]["fx"] = json!(1.5);
    p["bodies"][0]["fy"] = json!(-1.0);
    assert_eq!(
        of(&derive_shapes(&p, 600.0, 400.0), ShapeType::Arrow).len(),
        3,
        "control: showForces false"
    );
    p["showForces"] = json!(true);
    p["forceScale"] = json!(10);
    let shapes = derive_shapes(&p, 600.0, 400.0);
    let forces: Vec<_> = of(&shapes, ShapeType::Arrow)
        .into_iter()
        .filter(|a| a.color.as_deref() == Some("series-5"))
        .collect();
    assert_eq!(forces.len(), 1);
    assert!(near(forces[0].x2, 115.0) && near(forces[0].y2, 50.0));
}

#[test]
fn a_body_label_sits_up_and_right_of_the_circle_and_a_missing_color_takes_the_series_tone() {
    let p = json!({"bodies": [{"x": 50, "y": 60}, {"x": 80, "y": 90, "radius": 10, "label": "m"}]});
    let shapes = derive_shapes(&p, 600.0, 400.0);
    let circles = of(&shapes, ShapeType::Circle);
    assert_eq!(circles[0].color.as_deref(), Some("series-1"));
    assert_eq!(circles[1].color.as_deref(), Some("series-2"));
    assert_eq!(circles[0].radius, Some(12.0));
    let text = of(&shapes, ShapeType::Text);
    assert_eq!(text.len(), 1);
    assert!(near(text[0].x, 96.0) && near(text[0].y, 74.0));
    assert_eq!(text[0].tone.as_deref(), Some("ink"));
    assert_eq!(text[0].text_size.as_deref(), Some("sm"));
}

#[test]
fn incline_fixture_draws_the_ramp_triangle_the_arc_and_three_labeled_vectors() {
    let shapes = derive_shapes(&incline(), 600.0, 400.0);
    let polys = of(&shapes, ShapeType::Polygon);
    assert_eq!(polys.len(), 1);
    let pts = polys[0].points.clone().unwrap_or_default();
    assert_eq!(
        (pts[0].x, pts[0].y, pts[1].x, pts[1].y, pts[2].x, pts[2].y),
        (180.0, 220.0, 522.0, 360.0, 180.0, 360.0)
    );
    assert_eq!(polys[0].fill.as_deref(), Some("fill"));
    assert_eq!(polys[0].color.as_deref(), Some("ink"));
    let ramp_label = shapes
        .iter()
        .find(|s| s.ty == ShapeType::Text && s.tone.as_deref() == Some("label"))
        .expect("ramp label");
    assert!(
        near(ramp_label.x, (2.0 * 180.0 + 522.0) / 3.0)
            && near(ramp_label.y, (220.0 + 2.0 * 360.0) / 3.0)
    );
    assert_eq!(ramp_label.align, Some(Align::Center));
    let arc = of(&shapes, ShapeType::Ellipse);
    assert_eq!(arc.len(), 1);
    assert_eq!((arc[0].width, arc[0].height), (Some(44.0), Some(44.0)));
    assert_eq!(arc[0].start_angle, Some(180.0));
    assert_eq!(arc[0].color.as_deref(), Some("series-2"));
    let vectors: Vec<_> = of(&shapes, ShapeType::Arrow)
        .into_iter()
        .filter(|a| a.stroke.as_deref() == Some("bold"))
        .collect();
    assert_eq!(vectors.len(), 3);
    assert!(
        near(vectors[0].y2, 220.0 + 20.0),
        "mg points straight down from the block center"
    );
    let labels: Vec<_> = shapes
        .iter()
        .filter(|s| s.text_case.as_deref() == Some("verbatim"))
        .filter_map(|s| s.text.clone())
        .collect();
    assert!(
        labels.starts_with(&["mg".to_string(), "N".to_string(), "f".to_string()]),
        "{labels:?}"
    );
}

#[test]
fn a_vector_label_sits_eight_pixels_past_the_tip_and_a_zero_vector_points_up() {
    let p = json!({"bodies": [{"id": "b", "x": 100, "y": 100, "radius": 16}],
        "vectors": [{"body": "b", "dx": 0, "dy": 40, "label": "mg"}, {"body": "b", "dx": 0, "dy": 0, "label": "z"}]});
    let shapes = derive_shapes(&p, 600.0, 400.0);
    let texts = of(&shapes, ShapeType::Text);
    assert!(near(texts[0].x, 100.0) && near(texts[0].y, 100.0 + 40.0 + 8.0));
    assert!(
        near(texts[1].y, 100.0 - (16.0 + 8.0)),
        "zero length: anchor radius reach, upward"
    );
    assert_eq!(texts[0].color.as_deref(), Some("series-5"));
}

#[test]
fn a_vector_on_an_unknown_body_draws_nothing_and_a_point_vector_scales_and_dashes() {
    let p = json!({"vectors": [{"body": "ghost", "dx": 10, "dy": 0}, {"x": 10, "y": 20, "dx": 5, "dy": -5, "scale": 4, "dash": "dashed", "color": "series-6"}]});
    let shapes = derive_shapes(&p, 600.0, 400.0);
    let arrows = of(&shapes, ShapeType::Arrow);
    assert_eq!(arrows.len(), 1);
    assert!(near(arrows[0].x2, 30.0) && near(arrows[0].y2, 0.0));
    assert_eq!(arrows[0].dash, Some(Dash::Dashed));
    assert_eq!(arrows[0].color.as_deref(), Some("series-6"));
    let solid = json!({"vectors": [{"x": 10, "y": 20, "dx": 5, "dy": -5}]});
    assert_eq!(
        of(&derive_shapes(&solid, 600.0, 400.0), ShapeType::Arrow)[0].dash,
        None
    );
}

#[test]
fn spring_fixture_draws_a_zigzag_coil_between_lead_in_endpoints_not_a_straight_line() {
    let shapes = derive_shapes(&spring(), 600.0, 400.0);
    let coil: Vec<_> = of(&shapes, ShapeType::Line)
        .into_iter()
        .filter(|l| l.stroke.as_deref() == Some("normal") && l.dash.is_none() && l.x1 != l.x2)
        .collect();
    assert_eq!(
        coil.len(),
        1 + 1 + 16 + 1 + 1 - 1 + 1 - 1,
        "polyline of a, a', 16 coils, b', b has 20 points -> 19 segments"
    );
    let first = coil[0];
    assert_eq!(
        (first.x1, first.y1, first.x2, first.y2),
        (Some(300.0), Some(200.0), Some(310.0), Some(200.0))
    );
    let ys: Vec<f32> = coil.iter().skip(1).take(16).filter_map(|l| l.y2).collect();
    assert!(
        near(Some(ys[0]), 200.0 + 7.0) || near(Some(ys[0]), 200.0 - 7.0),
        "coil swings 7px off the axis: {ys:?}"
    );
    assert!(
        ys.windows(2).all(|w| (w[0] - w[1]).abs() > 13.0),
        "alternates sides"
    );
    let last = coil[coil.len() - 1];
    assert_eq!((last.x2, last.y2), (Some(420.0), Some(200.0)));
}

#[test]
fn rod_and_string_constraints_are_one_line_and_only_string_is_dashed() {
    let mut p = spring();
    p["constraints"][0]["kind"] = json!("rod");
    let rod = derive_shapes(&p, 600.0, 400.0);
    let rods: Vec<_> = of(&rod, ShapeType::Line)
        .into_iter()
        .filter(|l| l.x1 == Some(300.0) && l.x2 == Some(420.0))
        .collect();
    assert_eq!(rods.len(), 1);
    assert_eq!(rods[0].dash, None);
    p["constraints"][0]["kind"] = json!("string");
    let string = derive_shapes(&p, 600.0, 400.0);
    let strings: Vec<_> = of(&string, ShapeType::Line)
        .into_iter()
        .filter(|l| l.x1 == Some(300.0) && l.x2 == Some(420.0))
        .collect();
    assert_eq!(strings[0].dash, Some(Dash::Dashed));
    p["constraints"][0]["to"] = json!("nobody");
    let dangling = derive_shapes(&p, 600.0, 400.0);
    assert!(
        of(&dangling, ShapeType::Line)
            .iter()
            .all(|l| l.x2 != Some(420.0)),
        "unresolved endpoint draws nothing"
    );
}

#[test]
fn spring_fixture_wall_has_hatches_every_14px_and_a_left_aligned_label() {
    let shapes = derive_shapes(&spring(), 600.0, 400.0);
    let wall: Vec<_> = of(&shapes, ShapeType::Line)
        .into_iter()
        .filter(|l| l.x1 == Some(300.0) && l.x2 == Some(300.0) && l.dash.is_none())
        .collect();
    assert_eq!(wall.len(), 1);
    assert_eq!(
        (wall[0].y1, wall[0].y2, wall[0].stroke.as_deref()),
        (Some(140.0), Some(260.0), Some("normal"))
    );
    let hatches: Vec<_> = of(&shapes, ShapeType::Line)
        .into_iter()
        .filter(|l| l.stroke.as_deref() == Some("thin"))
        .collect();
    assert_eq!(hatches.len(), 9, "hy = 147, 161, ..., 245");
    assert!(near(hatches[0].y1, 147.0) && near(hatches[0].x2, 293.0) && near(hatches[0].y2, 154.0));
    let label = shapes
        .iter()
        .find(|s| s.text.as_deref() == Some("wall"))
        .expect("wall label");
    assert!(near(label.x, 312.0) && near(label.y, 200.0));
    assert_eq!(label.align, Some(Align::Left));
}

#[test]
fn authored_shapes_pass_through_last_in_the_derived_order() {
    let shapes = derive_shapes(&spring(), 600.0, 400.0);
    let last = shapes.last().expect("shapes");
    assert_eq!(last.dash, Some(Dash::Dashed));
    assert_eq!(last.color.as_deref(), Some("guide"));
    let mut p = spring();
    p["shapes"] = json!([]);
    assert_eq!(
        derive_shapes(&p, 600.0, 400.0).len(),
        shapes.len() - 1,
        "control: the passthrough is the only difference"
    );
}

#[test]
fn magnetic_field_into_draws_a_circle_and_an_x_per_lattice_cell() {
    let shapes = derive_shapes(&magnetic(), 600.0, 400.0);
    // gx: 60, 120, ..., 540 (9 columns); gy: 50, 110, ..., 350 (6 rows)
    let ring: Vec<_> = of(&shapes, ShapeType::Circle)
        .into_iter()
        .filter(|c| c.fill.is_none())
        .collect();
    assert_eq!(ring.len(), 54);
    assert!(near(ring[0].x, 60.0) && near(ring[0].y, 50.0));
    assert!(near(ring[0].radius, 14.0 / 3.0));
    assert_eq!(ring[0].color.as_deref(), Some("guide"));
    let crosses = of(&shapes, ShapeType::Line);
    assert_eq!(crosses.len(), 108);
    let d = 0.6 * (14.0 / 3.0) * std::f32::consts::FRAC_1_SQRT_2;
    assert!(near(crosses[0].x1, 60.0 - d) && near(crosses[0].x2, 60.0 + d));
    assert_eq!(crosses[0].stroke.as_deref(), Some("thin"));
}

#[test]
fn field_out_draws_a_ring_with_a_solid_dot_and_field_arrows_follow_the_angle() {
    let mut p = magnetic();
    p["field"]["kind"] = json!("out");
    let out = derive_shapes(&p, 600.0, 400.0);
    let dots: Vec<_> = of(&out, ShapeType::Circle)
        .into_iter()
        .filter(|c| c.radius == Some(1.5))
        .collect();
    assert_eq!(dots.len(), 54);
    assert_eq!(dots[0].fill_style.as_deref(), Some("solid"));
    assert!(of(&out, ShapeType::Line).is_empty());
    p["field"] = json!({"kind": "arrows", "spacing": 100, "angle": 90, "size": 20, "width": 200, "height": 100});
    let arrows = derive_shapes(&p, 600.0, 400.0);
    let a: Vec<_> = of(&arrows, ShapeType::Arrow)
        .into_iter()
        .filter(|a| a.color.as_deref() == Some("guide"))
        .collect();
    assert_eq!(a.len(), 2, "gx 50,150; gy 50");
    assert!(
        near(a[0].x1, 50.0) && near(a[0].y1, 40.0) && near(a[0].y2, 60.0),
        "90 degrees points down the screen"
    );
}

#[test]
fn a_non_positive_field_spacing_draws_no_lattice_instead_of_hanging() {
    let mut p = magnetic();
    p["field"]["spacing"] = json!(0);
    assert!(of(&derive_shapes(&p, 600.0, 400.0), ShapeType::Line).is_empty());
}

#[test]
fn trails_fade_oldest_first_and_fade_false_is_a_flat_0_6() {
    let p = json!({"trails": [{"points": [{"x": 0, "y": 0}, {"x": 10, "y": 0}, {"x": 20, "y": 0}, {"x": 30, "y": 0}], "color": "series-4", "width": 3, "opacity": 0.5}]});
    let shapes = derive_shapes(&p, 600.0, 400.0);
    let lines = of(&shapes, ShapeType::Line);
    assert_eq!(lines.len(), 3);
    assert!(near(lines[0].opacity, 0.12 * 0.5));
    assert!(near(lines[2].opacity, (0.12 + 0.68 * 2.0 / 3.0) * 0.5));
    assert_eq!(lines[0].line_width, Some(3.0));
    assert_eq!(lines[0].color.as_deref(), Some("series-4"));
    let mut flat = p.clone();
    flat["trails"][0]["fade"] = json!(false);
    assert!(near(
        of(&derive_shapes(&flat, 600.0, 400.0), ShapeType::Line)[0].opacity,
        0.6 * 0.5
    ));
    let single = json!({"trails": [{"points": [{"x": 0, "y": 0}]}]});
    assert!(
        derive_shapes(&single, 600.0, 400.0).is_empty(),
        "fewer than two points: no trail"
    );
}

#[test]
fn angle_arcs_default_to_radius_26_and_label_on_the_bisector() {
    let p = json!({"angles": [{"x": 100, "y": 100, "from": 0, "to": 90, "label": "θ"}]});
    let shapes = derive_shapes(&p, 600.0, 400.0);
    let arc = of(&shapes, ShapeType::Ellipse)[0];
    assert_eq!(
        (arc.width, arc.height, arc.end_angle),
        (Some(52.0), Some(52.0), Some(90.0))
    );
    assert_eq!(arc.color.as_deref(), Some("series-4"));
    let label = of(&shapes, ShapeType::Text)[0];
    let mid = 45.0_f32.to_radians();
    assert!(near(label.x, 100.0 + 39.0 * mid.cos()) && near(label.y, 100.0 + 39.0 * mid.sin()));
    assert_eq!(label.text_case.as_deref(), Some("verbatim"));
}

#[test]
fn ground_pivot_box_fixtures_follow_the_web_geometry() {
    let p = json!({"sceneObjects": [
        {"kind": "ground", "y": 300, "x1": 0, "x2": 50, "label": "floor"},
        {"kind": "pivot", "x": 200, "y": 40, "label": "O"},
        {"kind": "box", "x": 10, "y": 20, "width": 60, "height": 30, "fill": "series-2", "label": "M"}
    ]});
    let shapes = derive_shapes(&p, 600.0, 400.0);
    let ground: Vec<_> = of(&shapes, ShapeType::Line)
        .into_iter()
        .take(1 + 4)
        .collect();
    assert_eq!((ground[0].x2, ground[0].y1), (Some(50.0), Some(300.0)));
    assert!(
        near(ground[1].x1, 7.0) && near(ground[1].x2, 0.0) && near(ground[1].y2, 307.0),
        "hatches at 7, 21, 35, 49"
    );
    assert!(near(ground[4].x1, 49.0));
    let floor = shapes
        .iter()
        .find(|s| s.text.as_deref() == Some("floor"))
        .expect("label");
    assert!(near(floor.x, 25.0) && near(floor.y, 290.0));
    let bob = of(&shapes, ShapeType::Circle)[0];
    assert_eq!(
        (bob.radius, bob.fill.as_deref(), bob.fill_style.as_deref()),
        (Some(5.0), Some("ink"), Some("solid"))
    );
    let o = shapes
        .iter()
        .find(|s| s.text.as_deref() == Some("O"))
        .expect("pivot label");
    assert!(near(o.y, 10.0));
    let rect = of(&shapes, ShapeType::Rect)[0];
    assert_eq!(rect.fill.as_deref(), Some("series-2"));
    let m = shapes
        .iter()
        .find(|s| s.text.as_deref() == Some("M"))
        .expect("box label");
    assert!(near(m.x, 40.0) && near(m.y, 35.0));
    let bare = json!({"sceneObjects": [{"kind": "box"}]});
    let r = &derive_shapes(&bare, 600.0, 400.0)[0];
    assert_eq!(
        (r.width, r.height, r.fill.clone()),
        (Some(40.0), Some(40.0), None)
    );
}

#[test]
fn meters_stack_bottom_left_scaled_to_the_shared_max_and_formatted() {
    let p = json!({"meters": [{"label": "KE", "value": 4.5}, {"label": "PE", "value": 9, "color": "series-6"}, {"label": "E", "value": std::f64::consts::PI, "max": 12}]});
    let shapes = derive_shapes(&p, 600.0, 400.0);
    let bars = of(&shapes, ShapeType::Rect);
    assert_eq!(bars.len(), 3);
    assert_eq!(bars[0].width, Some(55.0), "4.5 / 9 of 110");
    assert_eq!(bars[1].width, Some(110.0));
    assert_eq!(bars[1].color.as_deref(), Some("series-6"));
    assert_eq!(bars[0].color.as_deref(), Some("series-1"));
    assert_eq!(bars[2].width, Some(29.0), "3.14159 / 12 of 110 rounded");
    assert!(near(bars[0].y, 400.0 - 10.0 - 16.0 * 3.0) && near(bars[2].y, 400.0 - 10.0 - 16.0));
    let texts: Vec<_> = of(&shapes, ShapeType::Text)
        .iter()
        .filter_map(|t| t.text.clone())
        .collect();
    assert_eq!(texts, ["KE", "4.5", "PE", "9", "E", "3.14"]);
    assert!(
        derive_shapes(&json!({"meters": []}), 600.0, 400.0).is_empty(),
        "control: no meters, no rows"
    );
}

#[test]
fn mode_3d_is_refused_and_2d_or_unset_maps() {
    let mut p = live_sim();
    assert!(map(&p, None).is_ok(), "control");
    p["mode"] = json!("2d");
    assert!(map(&p, None).is_ok());
    p["mode"] = json!("3d");
    assert!(matches!(map(&p, None), Err(CanvasSceneError::UnsupportedMode(m)) if m == "3d"));
}

#[test]
fn taps_report_the_body_id_and_its_index_in_the_derived_shapes() {
    let mut p = live_sim();
    p["interactive"] = json!(true);
    p["onShapeClick"] = json!("TAP_BODY");
    p["showVelocity"] = json!(false);
    let scene = map(&p, None).expect("2d");
    let hit = scene.emits_at(260.0, 90.0);
    assert_eq!(hit[0].event, "TAP_BODY");
    assert_eq!(hit[0].payload["id"], "ball-b");
    assert_eq!(hit[0].payload["type"], "circle");
    assert_eq!(hit[0].payload["index"], 1);
    assert!(scene.emits_at(5.0, 395.0).is_empty());
    p["interactive"] = json!(false);
    assert!(
        map(&p, None).expect("2d").emits_at(260.0, 90.0).is_empty(),
        "control: not interactive"
    );
}

#[test]
fn incline_scene_paints_the_packet_with_the_ramp_fill_and_the_arc() {
    let scene = map(&incline(), None).expect("2d");
    let cmds = &scene.packet.draw_commands;
    let arcs = cmds.iter().filter(|c| matches!(c, DrawCommand::Path { segments, .. } if matches!(segments[0], PathSegment::Arc { .. }))).count();
    assert!(arcs >= 2, "block circle + angle arc: {arcs}");
    assert!(cmds
        .iter()
        .any(|c| matches!(c, DrawCommand::Text { text, .. } if text == "mg")));
    assert!(cmds.iter().any(|c| matches!(c, DrawCommand::Text { text, .. } if text == "progress" || text.contains("progress"))), "readout chips ride along");
}
