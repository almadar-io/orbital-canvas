use super::*;
use crate::learning::theme::{DiagramTheme, Weight};
use orbital_canvas_base::packet::DrawCommand;
use serde_json::json;

fn close(a: Option<f32>, b: f32) -> bool {
    a.is_some_and(|a| (a - b).abs() < 0.01)
}

fn of(shapes: &[Shape], ty: ShapeType) -> Vec<&Shape> {
    shapes.iter().filter(|s| s.ty == ty).collect()
}

/// `std-bio-foodweb` at its config defaults (600x400): five species become
/// series circles sized by `clamp(population*0.4, 6, 55)` with "Label (pop)"
/// captions, `eats` edges are directed guide arrows, four trophic bands.
fn foodweb() -> Value {
    let species = [
        ("grass", 300, 300, "series-1", "Grass", 120),
        ("rabbit", 180, 230, "series-2", "Rabbit", 40),
        ("insect", 420, 230, "series-3", "Insect", 55),
        ("fox", 220, 110, "series-4", "Fox", 18),
        ("hawk", 400, 80, "series-5", "Hawk", 10),
    ];
    let nodes: Vec<Value> = species
        .iter()
        .map(|(id, x, y, c, l, p)| {
            let r = (*p as f64 * 0.4).clamp(6.0, 55.0);
            json!({"id": id, "x": x, "y": y, "radius": r, "color": c, "label": format!("{l} ({p})"), "kind": "organism"})
        })
        .collect();
    let edges: Vec<Value> = [
        ("rabbit", "grass"),
        ("insect", "grass"),
        ("fox", "rabbit"),
        ("fox", "insect"),
        ("hawk", "fox"),
    ]
    .iter()
    .map(|(p, q)| json!({"from": p, "to": q, "directed": true, "color": "guide", "label": "eats"}))
    .collect();
    json!({
        "type": "biology-canvas", "width": 600, "height": 400, "backgroundColor": "",
        "nodes": nodes, "edges": edges,
        "bands": [{"label": "Producers"}, {"label": "Herbivores"}, {"label": "Carnivores"}, {"label": "Apex"}],
        "traces": [{"series": [{"label": "Grass", "samples": [{"x": 0, "y": 120}]}]}]
    })
}

#[test]
fn foodweb_bands_stack_in_series_tones_with_labels_drawn_first() {
    let shapes = derive_shapes(&foodweb(), 600.0, 400.0);
    let bands = of(&shapes[..8], ShapeType::Rect);
    assert_eq!(bands.len(), 4);
    for (i, b) in bands.iter().enumerate() {
        assert_eq!(b.y, Some(i as f32 * 100.0));
        assert_eq!(
            (b.x, b.width, b.height),
            (Some(0.0), Some(600.0), Some(100.0))
        );
        assert_eq!(
            b.color.as_deref(),
            Some(format!("series-{}", i + 1).as_str())
        );
        assert_eq!(b.fill_style.as_deref(), Some("tint"));
        assert_eq!(b.stroke.as_deref(), Some("thin"));
        assert_eq!(b.opacity, Some(0.45));
    }
    let first_label = &shapes[1];
    assert_eq!(first_label.text.as_deref(), Some("Producers"));
    assert_eq!((first_label.x, first_label.y), (Some(8.0), Some(14.0)));
    assert_eq!(first_label.tone.as_deref(), Some("label"));
    assert_eq!(first_label.text_size.as_deref(), Some("xs"));
}

#[test]
fn foodweb_without_bands_has_no_background_rects() {
    let mut props = foodweb();
    props["bands"] = json!([]);
    let shapes = derive_shapes(&props, 600.0, 400.0);
    assert!(
        of(&shapes, ShapeType::Rect).is_empty(),
        "control: bands are the only rects"
    );
}

#[test]
fn foodweb_band_color_overrides_the_cycled_series_tone() {
    let mut props = foodweb();
    props["bands"] = json!([{"color": "#336699"}, {}]);
    let shapes = derive_shapes(&props, 600.0, 400.0);
    assert_eq!(shapes[0].color.as_deref(), Some("#336699"));
    assert_eq!(shapes[1].color.as_deref(), Some("series-2"));
    assert_eq!(shapes[1].y, Some(200.0), "two bands split the 400px height");
}

#[test]
fn foodweb_nodes_are_tinted_circles_with_centered_labels_below() {
    let shapes = derive_shapes(&foodweb(), 600.0, 400.0);
    let circles = of(&shapes, ShapeType::Circle);
    assert_eq!(circles.len(), 5);
    let grass = circles[0];
    assert_eq!(
        (grass.x, grass.y, grass.radius),
        (Some(300.0), Some(300.0), Some(48.0))
    );
    assert_eq!(grass.id.as_deref(), Some("grass"));
    assert_eq!(grass.fill.as_deref(), Some("series-1"));
    assert_eq!(grass.fill_style.as_deref(), Some("tint"));
    assert_eq!(grass.stroke.as_deref(), Some("normal"));
    assert_eq!(grass.opacity, None, "default state is opaque");
    let label = shapes
        .iter()
        .find(|s| s.text.as_deref() == Some("Hawk (10)"))
        .unwrap();
    assert_eq!((label.x, label.y), (Some(400.0), Some(80.0 + 6.0 + 14.0)));
    assert_eq!(label.align, Some(Align::Center));
    assert_eq!(label.tone.as_deref(), Some("ink"));
    assert_eq!(label.text_size.as_deref(), Some("sm"));
}

#[test]
fn node_without_a_color_cycles_the_series_by_index_and_radius_defaults_to_16() {
    let props = json!({"nodes": [{"x": 10, "y": 10}, {"x": 50, "y": 10, "label": ""}]});
    let shapes = derive_shapes(&props, 600.0, 400.0);
    assert_eq!(shapes.len(), 2, "an empty label draws no text");
    assert_eq!(shapes[0].color.as_deref(), Some("series-1"));
    assert_eq!(shapes[1].color.as_deref(), Some("series-2"));
    assert_eq!(shapes[0].radius, Some(16.0));
    assert_eq!(shapes[0].id, None);
}

#[test]
fn foodweb_directed_edge_is_a_rim_shortened_guide_arrow_with_a_midpoint_label() {
    let shapes = derive_shapes(&foodweb(), 600.0, 400.0);
    let arrows = of(&shapes, ShapeType::Arrow);
    assert_eq!(arrows.len(), 5);
    let a = arrows[0];
    let (dx, dy) = (120.0_f32, 70.0_f32);
    let d = dx.hypot(dy);
    assert!(
        close(a.x1, 180.0 + dx / d * 16.0) && close(a.y1, 230.0 + dy / d * 16.0),
        "starts on the rabbit rim (r=16)"
    );
    assert!(
        close(a.x2, 300.0 - dx / d * 48.0) && close(a.y2, 300.0 - dy / d * 48.0),
        "ends on the grass rim (r=48)"
    );
    assert_eq!(a.color.as_deref(), Some("guide"));
    assert_eq!(a.stroke.as_deref(), Some("normal"));
    let eats = shapes
        .iter()
        .find(|s| s.text.as_deref() == Some("eats"))
        .unwrap();
    assert_eq!((eats.x, eats.y), (Some(244.0), Some(261.0)));
    assert_eq!(eats.tone.as_deref(), Some("label"));
}

#[test]
fn an_undirected_edge_is_a_center_to_center_line_in_ink_and_a_dangling_edge_is_skipped() {
    let mut props = foodweb();
    props["edges"] = json!([{"from": "rabbit", "to": "grass"}, {"from": "rabbit", "to": "nobody"}]);
    let shapes = derive_shapes(&props, 600.0, 400.0);
    assert!(of(&shapes, ShapeType::Arrow).is_empty());
    let lines = of(&shapes, ShapeType::Line);
    assert_eq!(lines.len(), 1, "the edge naming an unknown node is skipped");
    let l = lines[0];
    assert_eq!(
        (l.x1, l.y1, l.x2, l.y2),
        (Some(180.0), Some(230.0), Some(300.0), Some(300.0))
    );
    assert_eq!(l.color.as_deref(), Some("ink"));
}

#[test]
fn the_packet_draws_the_guide_arrow_stroke_and_reads_traces_from_the_fixture() {
    let scene = map(&foodweb(), None).unwrap();
    let theme = DiagramTheme::heritage(false);
    let guide = theme.tone("guide").unwrap();
    let guide_strokes = scene
        .packet
        .draw_commands
        .iter()
        .filter(|c| matches!(c, DrawCommand::Path { stroke: Some(s), fill: None, .. } if s.color == guide && s.width == theme.stroke_width(Weight::Normal)))
        .count();
    assert!(guide_strokes >= 5, "five arrow shafts: {guide_strokes}");
    let mut bare = foodweb();
    bare["traces"] = json!([]);
    assert!(
        scene.packet.draw_commands.len() > map(&bare, None).unwrap().packet.draw_commands.len(),
        "control: traces add marks"
    );
}

/// `std-bio-cell` (coreOrganelles) with the nucleus selected: the membrane
/// becomes a compartment, the nucleus a 1.4x highlighted node.
fn cell_selected_nucleus() -> Value {
    json!({
        "type": "biology-canvas", "width": 600, "height": 400,
        "nodes": [
            {"id": "nucleus", "x": 320, "y": 180, "radius": 64.4, "color": "series-2", "label": "Nucleus", "kind": "cell", "state": "highlight"},
            {"id": "ribosomes", "x": 425, "y": 145, "radius": 9, "color": "series-4", "label": "Ribosomes", "kind": "organelle", "state": "default"}
        ],
        "compartments": [{"x": 300, "y": 200, "width": 320, "height": 320, "label": "Cell Membrane", "color": "ink", "stroke": "normal"}]
    })
}

#[test]
fn cell_membrane_compartment_is_a_tinted_ink_ellipse_with_a_top_label() {
    let shapes = derive_shapes(&cell_selected_nucleus(), 600.0, 400.0);
    let e = &shapes[0];
    assert_eq!(e.ty, ShapeType::Ellipse);
    assert_eq!(
        (e.x, e.y, e.width, e.height),
        (Some(300.0), Some(200.0), Some(320.0), Some(320.0))
    );
    assert_eq!(
        (
            e.color.as_deref(),
            e.fill.as_deref(),
            e.fill_style.as_deref()
        ),
        (Some("ink"), Some("ink"), Some("tint"))
    );
    assert_eq!(e.stroke.as_deref(), Some("normal"));
    assert_eq!(e.dash, None);
    let t = &shapes[1];
    assert_eq!(
        (t.text.as_deref(), t.x, t.y),
        (Some("Cell Membrane"), Some(300.0), Some(54.0))
    );
    assert_eq!(
        (t.tone.as_deref(), t.text_size.as_deref(), t.align),
        (Some("ink"), Some("sm"), Some(Align::Center))
    );
}

#[test]
fn compartment_defaults_and_overrides_diverge_on_color_fill_dash_and_line_width() {
    let props = json!({"compartments": [{"x": 0, "y": 0, "width": 10, "height": 10}, {"x": 0, "y": 0, "width": 10, "height": 10, "color": "series-3", "fill": "series-1", "dash": "dotted", "lineWidth": 4}]});
    let shapes = derive_shapes(&props, 600.0, 400.0);
    assert_eq!(
        (shapes[0].color.as_deref(), shapes[0].fill.as_deref()),
        (Some("ink"), Some("ink"))
    );
    assert_eq!(
        (shapes[0].stroke.as_deref(), shapes[0].line_width),
        (Some("normal"), None)
    );
    assert_eq!(
        (shapes[1].color.as_deref(), shapes[1].fill.as_deref()),
        (Some("series-3"), Some("series-1"))
    );
    assert_eq!(
        (shapes[1].stroke.as_deref(), shapes[1].line_width),
        (None, Some(4.0)),
        "lineWidth replaces the stroke role"
    );
    assert_eq!(shapes[1].dash, Some(Dash::Dotted));
}

#[test]
fn highlight_adds_a_ring_four_px_outside_and_muted_fades_node_and_label() {
    let shapes = derive_shapes(&cell_selected_nucleus(), 600.0, 400.0);
    let circles = of(&shapes, ShapeType::Circle);
    assert_eq!(circles.len(), 3, "two nodes + one highlight ring");
    let ring = circles[1];
    assert_eq!(ring.radius, Some(68.4));
    assert_eq!(ring.tone.as_deref(), Some("highlight"));
    assert!(ring.fill.is_none() && ring.fill_style.is_none());

    let mut muted = cell_selected_nucleus();
    muted["nodes"][0]["state"] = json!("muted");
    let shapes = derive_shapes(&muted, 600.0, 400.0);
    assert_eq!(
        of(&shapes, ShapeType::Circle).len(),
        2,
        "control: no ring when muted"
    );
    let node = of(&shapes, ShapeType::Circle)[0];
    assert_eq!(node.opacity, Some(0.35));
    let label = shapes
        .iter()
        .find(|s| s.text.as_deref() == Some("Nucleus"))
        .unwrap();
    assert_eq!(label.opacity, Some(0.35));
    let other = shapes
        .iter()
        .find(|s| s.text.as_deref() == Some("Ribosomes"))
        .unwrap();
    assert_eq!(other.opacity, None);
}

/// `std-bio-mitosis` prophase scene (config `phaseScenes[1]`): the dashed
/// `dotted` nucleus compartment inside the cell, chromosome nodes with empty
/// labels, and the five-phase timeline at step 1.
fn mitosis(step: usize) -> Value {
    let labels = [
        "Interphase",
        "Prophase",
        "Metaphase",
        "Anaphase",
        "Telophase",
    ];
    let stages: Vec<Value> = labels
        .iter()
        .enumerate()
        .map(|(i, l)| json!({"label": l, "state": if i < step { "done" } else if i == step { "active" } else { "pending" }}))
        .collect();
    json!({
        "type": "biology-canvas", "width": 600, "height": 400,
        "nodes": [
            {"id": "c1", "x": 270, "y": 180, "radius": 10, "color": "series-2", "label": "", "kind": "organelle"},
            {"id": "c2", "x": 330, "y": 180, "radius": 10, "color": "series-2", "label": "", "kind": "organelle"}
        ],
        "compartments": [
            {"x": 300, "y": 200, "width": 340, "height": 280, "label": "cell", "color": "ink"},
            {"x": 300, "y": 200, "width": 140, "height": 120, "label": "nucleus", "color": "series-3", "dash": "dotted"}
        ],
        "stages": stages
    })
}

#[test]
fn mitosis_timeline_places_five_chips_on_the_bottom_strip_with_arrows_between() {
    let shapes = derive_shapes(&mitosis(1), 600.0, 400.0);
    let chips = of(&shapes, ShapeType::Rect);
    assert_eq!(chips.len(), 5);
    let slot = 584.0_f32 / 5.0;
    for (i, c) in chips.iter().enumerate() {
        assert!(
            close(c.x, 8.0 + i as f32 * slot + 5.0) && close(c.width, slot - 10.0),
            "chip {i}"
        );
        assert_eq!((c.y, c.height), (Some(368.0), Some(26.0)));
    }
    let arrows = of(&shapes, ShapeType::Arrow);
    assert_eq!(arrows.len(), 4);
    assert!(close(arrows[0].x1, 13.0 + slot - 10.0) && close(arrows[0].x2, 8.0 + slot + 5.0));
    assert_eq!(
        (arrows[0].y1, arrows[0].tone.as_deref()),
        (Some(381.0), Some("guide"))
    );
    let colors: Vec<_> = chips.iter().map(|c| c.color.as_deref().unwrap()).collect();
    assert_eq!(
        colors,
        ["muted", "primary", "fill", "fill", "fill"],
        "done / active / pending"
    );
    let label = shapes
        .iter()
        .find(|s| s.text.as_deref() == Some("Prophase"))
        .unwrap();
    assert_eq!(
        (label.tone.as_deref(), label.y, label.text_size.as_deref()),
        (Some("ink"), Some(381.0), Some("xs"))
    );
    let pending = shapes
        .iter()
        .find(|s| s.text.as_deref() == Some("Telophase"))
        .unwrap();
    assert_eq!(
        pending.tone.as_deref(),
        Some("label"),
        "pending text is the label tone"
    );
}

#[test]
fn mitosis_stage_color_overrides_the_state_fill_and_a_later_step_changes_the_states() {
    let mut props = mitosis(4);
    props["stages"][0]["color"] = json!("series-6");
    let shapes = derive_shapes(&props, 600.0, 400.0);
    let chips = of(&shapes, ShapeType::Rect);
    let colors: Vec<_> = chips.iter().map(|c| c.color.as_deref().unwrap()).collect();
    assert_eq!(colors, ["series-6", "muted", "muted", "muted", "primary"]);
    let before = derive_shapes(&mitosis(1), 600.0, 400.0);
    assert_ne!(
        of(&before, ShapeType::Rect)[4].color,
        chips[4].color,
        "control: step 1 leaves Telophase pending"
    );
}

#[test]
fn a_single_stage_has_no_connecting_arrow_and_no_stages_draws_no_strip() {
    let mut props = mitosis(0);
    props["stages"] = json!([{"label": "Only"}]);
    let shapes = derive_shapes(&props, 600.0, 400.0);
    assert!(of(&shapes, ShapeType::Arrow).is_empty());
    assert_eq!(of(&shapes, ShapeType::Rect).len(), 1);
    props["stages"] = json!([]);
    assert!(of(&derive_shapes(&props, 600.0, 400.0), ShapeType::Rect).is_empty());
}

#[test]
fn mitosis_compartments_carry_dashes_and_empty_labels_draw_no_text() {
    let shapes = derive_shapes(&mitosis(1), 600.0, 400.0);
    let ellipses = of(&shapes, ShapeType::Ellipse);
    assert_eq!(
        (ellipses[0].dash, ellipses[1].dash),
        (None, Some(Dash::Dotted))
    );
    let texts: Vec<_> = shapes.iter().filter_map(|s| s.text.as_deref()).collect();
    assert_eq!(&texts[..2], ["cell", "nucleus"]);
    assert!(!texts.contains(&""), "chromosome nodes have label \"\"");
}

/// `std-bio-phasecycle` ring overview (ringWidth 480 x ringHeight 260) with
/// four phases at step 1: Prophase done, Metaphase active.
fn phase_ring() -> Value {
    json!({
        "type": "biology-canvas", "width": 480, "height": 260, "stageStyle": "ring",
        "stages": [
            {"label": "Prophase", "state": "done"}, {"label": "Metaphase", "state": "active"},
            {"label": "Anaphase", "state": "pending"}, {"label": "Telophase", "state": "pending"}
        ]
    })
}

#[test]
fn phase_ring_places_chips_clockwise_from_the_top_on_a_radius_of_min_side_half_minus_48() {
    let shapes = derive_shapes(&phase_ring(), 480.0, 260.0);
    let chips = of(&shapes, ShapeType::Rect);
    let centers: Vec<(f32, f32)> = chips
        .iter()
        .map(|c| {
            (
                c.x.unwrap() + c.width.unwrap() / 2.0,
                c.y.unwrap() + c.height.unwrap() / 2.0,
            )
        })
        .collect();
    let want = [
        (240.0, 48.0),
        (322.0, 130.0),
        (240.0, 212.0),
        (158.0, 130.0),
    ];
    for (got, want) in centers.iter().zip(want) {
        assert!(
            (got.0 - want.0).abs() < 0.01 && (got.1 - want.1).abs() < 0.01,
            "{got:?} vs {want:?}"
        );
    }
    assert_eq!(chips[0].width, Some(58.0), "8 chars * 6 + 10");
    assert_eq!(chips[1].width, Some(64.0));
    assert_eq!(chips[0].height, Some(18.0));
    let colors: Vec<_> = chips.iter().map(|c| c.color.as_deref().unwrap()).collect();
    assert_eq!(colors, ["muted", "primary", "fill", "fill"]);
}

#[test]
fn phase_ring_arrows_stop_46px_short_of_each_chip_centre_and_do_not_close_the_loop() {
    let shapes = derive_shapes(&phase_ring(), 480.0, 260.0);
    let arrows = of(&shapes, ShapeType::Arrow);
    assert_eq!(arrows.len(), 3);
    let u = std::f32::consts::FRAC_1_SQRT_2;
    assert!(close(arrows[0].x1, 240.0 + u * 46.0) && close(arrows[0].y1, 48.0 + u * 46.0));
    assert!(close(arrows[0].x2, 322.0 - u * 46.0) && close(arrows[0].y2, 130.0 - u * 46.0));
    assert_eq!(arrows[0].tone.as_deref(), Some("guide"));
    let mut timeline = phase_ring();
    timeline["stageStyle"] = json!("timeline");
    let t = derive_shapes(&timeline, 480.0, 260.0);
    assert_ne!(
        of(&t, ShapeType::Rect)[0].y,
        of(&shapes, ShapeType::Rect)[0].y,
        "control: stageStyle switches layout"
    );
}

#[test]
fn long_ring_labels_clamp_the_chip_width_to_84_and_short_ones_to_26() {
    let props = json!({"width": 480, "height": 260, "stageStyle": "ring", "stages": [{"label": "Extraordinarily long"}, {"label": "A"}]});
    let chips: Vec<_> = derive_shapes(&props, 480.0, 260.0)
        .into_iter()
        .filter(|s| s.ty == ShapeType::Rect)
        .collect();
    assert_eq!((chips[0].width, chips[1].width), (Some(84.0), Some(26.0)));
}

/// `std-bio-dna-replication` (sequence ATGCCTGA, 600x400) after two steps:
/// fork 2/8, rungs 0-1 new, rung 2 open, the rest paired.
fn replication_helix(fork_index: usize) -> Value {
    let bases = ["A", "T", "G", "C", "C", "T", "G", "A"];
    let comps = ["T", "A", "C", "G", "G", "A", "C", "T"];
    let rungs: Vec<Value> = (0..8)
        .map(|i| json!({"a": bases[i], "b": comps[i], "state": if i < fork_index { "new" } else if i == fork_index { "open" } else { "paired" }}))
        .collect();
    json!({"type": "biology-canvas", "width": 600, "height": 400, "helix": {"fork": fork_index as f64 / 8.0, "rungs": rungs}})
}

#[test]
fn replication_helix_unzips_the_rungs_behind_the_fork_and_stubs_them() {
    let shapes = derive_shapes(&replication_helix(2), 600.0, 400.0);
    let lines = of(&shapes, ShapeType::Line);
    assert_eq!(
        shapes.len(),
        14 + 8 + 18,
        "2 strands x 7 segments, 2 unzipped rungs, 6 paired rungs"
    );
    let bold: Vec<_> = lines
        .iter()
        .filter(|l| l.stroke.as_deref() == Some("bold"))
        .collect();
    assert_eq!(bold.len(), 14);
    assert_eq!(bold[0].color.as_deref(), Some("series-1"));
    assert_eq!(bold[7].color.as_deref(), Some("series-2"));
    assert!(
        close(bold[0].x1, 24.0 + 0.5 * 69.0) && close(bold[0].y1, 200.0 - 79.0 / 2.0),
        "rung 0 separation 28 + 68*0.75"
    );
    assert!(
        close(bold[0].y2, 200.0 - 45.0 / 2.0),
        "rung 1 separation 28 + 68*0.25"
    );
    let thin: Vec<_> = lines
        .iter()
        .filter(|l| l.stroke.as_deref() == Some("thin"))
        .collect();
    assert_eq!(
        thin[0].color.as_deref(),
        Some("accent"),
        "new rungs are accent"
    );
    assert!(
        close(thin[0].y1, 160.5) && close(thin[0].y2, 168.5),
        "an unzipped rung is an 8px stub from the strand"
    );
    let a = shapes
        .iter()
        .find(|s| s.text.as_deref() == Some("A") && s.y.is_some_and(|y| (y - 174.5).abs() < 0.01))
        .unwrap();
    assert_eq!(
        (a.align, a.tone.as_deref(), a.text_size.as_deref()),
        (Some(Align::Center), Some("label"), Some("xs"))
    );
    let open_rung = thin
        .iter()
        .find(|l| l.color.as_deref() == Some("muted"))
        .unwrap();
    assert!(
        close(open_rung.y1, 186.0) && close(open_rung.y2, 214.0),
        "paired rungs span the 28px separation"
    );
}

#[test]
fn a_fully_paired_helix_has_no_stubs_and_the_fork_moves_the_split() {
    let closed = derive_shapes(&replication_helix(0), 600.0, 400.0);
    assert!(
        of(&closed, ShapeType::Line)
            .iter()
            .all(|l| l.color.as_deref() != Some("accent")),
        "no new rungs at fork 0"
    );
    let stubs = |s: &[Shape]| {
        of(s, ShapeType::Line)
            .iter()
            .filter(|l| {
                l.stroke.as_deref() == Some("thin") && (l.y2.unwrap() - l.y1.unwrap()).abs() < 9.0
            })
            .count()
    };
    assert_eq!(stubs(&closed), 0);
    assert_eq!(
        stubs(&derive_shapes(&replication_helix(2), 600.0, 400.0)),
        4
    );
}

#[test]
fn helix_geometry_color_and_rung_overrides_apply() {
    let props = json!({"helix": {"x": 10, "y": 20, "width": 100, "height": 60, "colorA": "#112233", "colorB": "series-5", "rungColor": "series-7",
        "rungs": [{"a": "G"}, {"b": "C", "color": "series-8"}]}});
    let shapes = derive_shapes(&props, 600.0, 400.0);
    let strands: Vec<_> = of(&shapes, ShapeType::Line)
        .into_iter()
        .filter(|l| l.stroke.as_deref() == Some("bold"))
        .collect();
    assert_eq!(
        (strands[0].color.as_deref(), strands[1].color.as_deref()),
        (Some("#112233"), Some("series-5"))
    );
    assert!(close(strands[0].x1, 10.0 + 25.0) && close(strands[0].x2, 10.0 + 75.0));
    assert!(close(strands[0].y1, 50.0 - 14.0), "cy = 20 + 30, sep 28");
    let rungs: Vec<_> = of(&shapes, ShapeType::Line)
        .into_iter()
        .filter(|l| l.stroke.as_deref() == Some("thin"))
        .collect();
    assert_eq!(
        (rungs[0].color.as_deref(), rungs[1].color.as_deref()),
        (Some("series-7"), Some("series-8"))
    );
    let texts: Vec<_> = shapes.iter().filter_map(|s| s.text.as_deref()).collect();
    assert_eq!(texts, ["G", "C"], "a missing base letter draws nothing");
}

#[test]
fn authored_shapes_are_appended_last_and_hits_index_into_the_derived_list() {
    let mut props = cell_selected_nucleus();
    props["shapes"] = json!([{"type": "line", "x1": 0, "y1": 0, "x2": 5, "y2": 5, "id": "extra"}]);
    let shapes = derive_shapes(&props, 600.0, 400.0);
    assert_eq!(shapes.last().unwrap().id.as_deref(), Some("extra"));
    let mut interactive = props.clone();
    interactive["interactive"] = json!(true);
    interactive["onShapeClick"] = json!("PICK");
    let scene = map(&interactive, None).unwrap();
    let hit = scene.emits_at(425.0, 145.0);
    assert_eq!(
        (hit[0].event.as_str(), &hit[0].payload),
        (
            "PICK",
            &json!({"id": "ribosomes", "type": "circle", "index": 5})
        )
    );
    let ring = scene.emits_at(320.0, 180.0);
    assert_eq!(
        ring[0].payload,
        json!({"type": "circle", "index": 3}),
        "the unnamed highlight ring is topmost over the nucleus, as on the web"
    );
    assert!(
        map(&props, None).unwrap().hits.is_empty(),
        "control: not interactive"
    );
}

/// `std-bio-dna-3d` and `std-learn-tour-3d` render `mode: "3d"` with
/// `helix3d`/camera props: a three.js scene the 2D renderer refuses.
#[test]
fn three_d_mode_is_refused_and_two_d_is_not() {
    let mut props = json!({"type": "biology-canvas", "mode": "3d", "helix3d": {"count": 10}, "width": 600, "height": 400});
    assert!(matches!(map(&props, None), Err(CanvasSceneError::UnsupportedMode(m)) if m == "3d"));
    props["mode"] = json!("2d");
    assert!(map(&props, None).is_ok());
}
