use super::*;

fn path_of(scene: &MappedScene, i: usize) -> &DrawCommand {
    &scene.packet.draw_commands[i]
}

#[test]
fn the_layout_seed_matches_the_web_hash_and_prng() {
    // values from running GraphCanvas.tsx hashSeed/mulberry32 in node
    assert_eq!(hash_seed("alpha"), 3867295292);
    let mut r = mulberry32(hash_seed("alpha"));
    assert!((r() - 0.25601538829505444).abs() < 1e-12);
    assert!((r() - 0.930887047899887).abs() < 1e-12);
}

#[test]
fn labels_truncate_at_22_chars_with_an_ellipsis_and_short_ones_pass() {
    assert_eq!(truncate_label("short"), "short");
    let t = truncate_label("abcdefghijklmnopqrstuvwxyz");
    assert_eq!(t.chars().count(), 22);
    assert!(t.ends_with('\u{2026}'));
    assert_eq!(truncate_label(&"x".repeat(22)).chars().count(), 22);
}

#[test]
fn edge_weight_sets_width_between_thin_and_normal_never_opacity() {
    assert_eq!(edge_stroke(None, 0.18, 1.0, 1.5), (0.18, 1.0));
    assert_eq!(edge_stroke(Some(1.0), 0.18, 1.0, 1.5), (0.18, 1.5));
    assert_eq!(edge_stroke(Some(0.0), 0.18, 1.0, 1.5), (0.18, 1.0));
    assert_eq!(edge_stroke(Some(9.0), 0.18, 1.0, 1.5), (0.18, 1.5));
}

fn opts(layout: &str) -> LayoutOpts {
    LayoutOpts {
        width: 800.0,
        height: 400.0,
        layout: layout.into(),
        show_labels: true,
        repulsion: 800.0,
        link_distance: 100.0,
        node_spacing: 28.0,
    }
}

fn nodes_of(ids: &[&str]) -> Vec<Node> {
    let v = json!(ids
        .iter()
        .map(|i| json!({"id": i, "label": i}))
        .collect::<Vec<_>>());
    let mut n = parse_nodes(Some(&v));
    for x in n.iter_mut() {
        x.label_w = 40.0;
    }
    n
}

#[test]
fn circular_and_grid_place_nodes_on_the_web_formulas_without_a_view() {
    let mut n = nodes_of(&["a", "b", "c", "d"]);
    let v = layout_nodes(&mut n, &[], &[], &opts("circular"));
    assert_eq!(
        v,
        View {
            zoom: 1.0,
            ox: 0.0,
            oy: 0.0
        }
    );
    assert!((n[0].x - (400.0 + 140.0)).abs() < 1e-9 && (n[0].y - 200.0).abs() < 1e-9);
    assert!((n[1].x - 400.0).abs() < 1e-9 && (n[1].y - (200.0 + 140.0)).abs() < 1e-9);
    let mut g = nodes_of(&["a", "b", "c", "d"]);
    layout_nodes(&mut g, &[], &[], &opts("grid"));
    assert_eq!((g[0].x, g[0].y), (800.0 / 3.0, 400.0 / 3.0));
    assert_eq!((g[3].x, g[3].y), (800.0 / 3.0 * 2.0, 400.0 / 3.0 * 2.0));
}

fn node_at(x: f64, y: f64) -> Node {
    let mut n = parse_nodes(Some(&json!([{"id": "n"}]))).remove(0);
    (n.x, n.y, n.vx, n.vy) = (x, y, 0.0, 0.0);
    n
}

fn force_cases() -> Vec<Value> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/graph-force/cases.json"
    );
    orbital_core_base::json_value::from_json_str(
        &std::fs::read_to_string(path).expect("graph-force fixture"),
    )
    .unwrap()
}

/// Max distance between the native and the d3 settled positions of one fixture case.
fn force_drift(case: &Value) -> f64 {
    let f = |v: &Value| v.as_f64().unwrap();
    let mut nodes: Vec<Node> = case["start"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| node_at(f(&p["x"]), f(&p["y"])))
        .collect();
    let radii: Vec<f64> = case["start"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| f(&p["radius"]))
        .collect();
    let raw = |key: &str| -> Vec<(usize, usize, f64)> {
        case[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| (f(&e["s"]) as usize, f(&e["t"]) as usize, f(&e["weight"])))
            .collect()
    };
    let ld = f(&case["linkDistance"]);
    let count = nodes.len();
    let edge_links = links(&raw("edges"), count, |w| ld * (0.35 + (1.0 - w) * 0.3));
    let sim_links = links(&raw("sims"), count, |w| ld * (1.35 - 0.55 * w));
    let goals: Vec<((f64, f64), f64)> = case["goals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| ((f(&g["x"]), f(&g["y"])), f(&g["strength"])))
        .collect();
    settle(
        &mut nodes,
        &edge_links,
        &sim_links,
        -f(&case["repulsion"]) * 0.5,
        &radii,
        &goals,
    );
    nodes
        .iter()
        .zip(case["settled"].as_array().unwrap())
        .map(|(n, p)| ((n.x - f(&p["x"])).powi(2) + (n.y - f(&p["y"])).powi(2)).sqrt())
        .fold(0.0, f64::max)
}

#[test]
fn the_force_layout_settles_where_d3_force_settles() {
    for case in force_cases() {
        let drift = force_drift(&case);
        assert!(
            drift < 1e-6,
            "{}: max drift {drift} px from d3",
            case["name"]
        );
    }
}

#[test]
fn control_a_perturbed_start_drifts_from_the_d3_fixture() {
    let mut case = force_cases().remove(0);
    case["start"][3]["x"] = json!(case["start"][3]["x"].as_f64().unwrap() + 40.0);
    assert!(force_drift(&case) > 1.0);
}

#[test]
fn force_layout_is_deterministic_and_separates_nodes() {
    let ids = ["a", "b", "c", "d", "e", "f"];
    let edges = parse_edges(Some(
        &json!([{"source":"a","target":"b"},{"source":"b","target":"c"},{"source":"d","target":"e"}]),
    ));
    let mut one = nodes_of(&ids);
    let mut two = nodes_of(&ids);
    let v1 = layout_nodes(&mut one, &edges, &[], &opts("force"));
    let v2 = layout_nodes(&mut two, &edges, &[], &opts("force"));
    assert_eq!(v1, v2);
    for (a, b) in one.iter().zip(&two) {
        assert_eq!((a.x, a.y), (b.x, b.y));
    }
    assert!(v1.zoom > 0.0 && v1.zoom <= 1.0);
    for i in 0..one.len() {
        for j in (i + 1)..one.len() {
            let d = ((one[i].x - one[j].x).powi(2) + (one[i].y - one[j].y).powi(2)).sqrt();
            assert!(d > 16.0, "{} and {} overlap at {d}", ids[i], ids[j]);
        }
    }
    let mut other = nodes_of(&ids);
    let v3 = layout_nodes(&mut other, &[], &[], &opts("force"));
    assert!(
        one.iter().zip(&other).any(|(a, b)| (a.x - b.x).abs() > 1.0),
        "control: edges change the layout, {v3:?}"
    );
}

#[test]
fn the_settled_force_layout_matches_d3_force_run_on_the_same_graph() {
    // positions from the web's d3-force setup (300 ticks, labelW 40) run in node; the port is exact pairwise where d3 uses Barnes-Hut, so a few px apart
    let want = [
        ("limits", 458.68, 180.79),
        ("derivatives", 350.27, 216.11),
        ("integrals", 415.94, 233.69),
        ("series", 317.95, 133.45),
        ("a", 483.13, 244.23),
        ("b", 367.91, 281.78),
        ("c", 392.99, 163.16),
        ("d", 525.82, 191.36),
        ("e", 440.39, 297.14),
    ];
    let ids: Vec<&str> = want.iter().map(|w| w.0).collect();
    let groups = ["L1", "L2", "L2", "L3", "L1", "L2", "L3", "L1", "L2"];
    let v = json!(ids
        .iter()
        .zip(groups)
        .map(|(i, g)| json!({"id": i, "group": g, "label": i}))
        .collect::<Vec<_>>());
    let mut nodes = parse_nodes(Some(&v));
    for n in nodes.iter_mut() {
        n.label_w = 40.0;
    }
    let pairs = [
        ("limits", "derivatives"),
        ("limits", "integrals"),
        ("derivatives", "series"),
        ("a", "b"),
        ("b", "c"),
        ("c", "d"),
        ("d", "e"),
        ("a", "limits"),
    ];
    let edges = parse_edges(Some(&json!(pairs
        .iter()
        .map(|(s, t)| json!({"source": s, "target": t}))
        .collect::<Vec<_>>())));
    layout_nodes(&mut nodes, &edges, &[], &opts("force"));
    for (n, w) in nodes.iter().zip(want) {
        assert!(
            (n.x - w.1).abs() < 6.0 && (n.y - w.2).abs() < 6.0,
            "{} at ({:.2},{:.2}) vs d3 ({},{})",
            w.0,
            n.x,
            n.y,
            w.1,
            w.2
        );
    }
}

#[test]
fn a_node_with_both_coordinates_keeps_them_as_the_start_and_a_zero_coordinate_does_not() {
    let v = json!([{"id":"a","x":100,"y":50},{"id":"b","x":0,"y":50}]);
    let n = parse_nodes(Some(&v));
    assert_eq!(n[0].given, Some((100.0, 50.0)));
    assert_eq!(n[1].given, None);
}

// std fixture: kflow-path `graph-canvas` (concept map: id/label/group levels, parent edges)
fn path_props() -> Value {
    json!({
        "type": "graph-canvas", "height": 420, "showLabels": true, "interactive": true, "draggable": true,
        "nodeClickEvent": "GRAPH_NODE_CLICK",
        "nodes": [
            {"id":"limits","label":"Limits","group":"Level 1"},
            {"id":"derivatives","label":"Derivatives","group":"Level 2"},
            {"id":"integrals","label":"Integrals","group":"Level 2"},
            {"id":"series","label":"Series and sequences","group":"Level 3"}
        ],
        "edges": [
            {"source":"limits","target":"derivatives"},
            {"source":"limits","target":"integrals"},
            {"source":"derivatives","target":"series"}
        ]
    })
}

// std fixture: studio-deploy topology graph (tone colors, size 10, selected unit, edge labels)
fn deploy_props() -> Value {
    json!({
        "type": "graph-canvas", "height": 320, "showLabels": true, "interactive": true, "draggable": true,
        "nodeClickEvent": "UNIT_CLICK", "selectedNodeId": "api",
        "nodes": [
            {"id":"api","label":"api","group":"compute","color":"success","size":10},
            {"id":"db","label":"db","group":"data","color":"warning","size":10},
            {"id":"cache","label":"cache","group":"data","color":"","size":10}
        ],
        "edges": [{"source":"api","target":"db","label":"sql"},{"source":"api","target":"cache","label":"get"}]
    })
}

fn circles(items: &[Item]) -> Vec<&Shape> {
    items
        .iter()
        .filter_map(|i| match i {
            Item::Shape(s) if s.ty == shape::ShapeType::Circle => Some(s.as_ref()),
            _ => None,
        })
        .collect()
}

#[test]
fn groups_take_series_tones_in_first_seen_order_and_ungrouped_is_muted_in_a_mixed_graph() {
    let g = build(&path_props(), Some((390.0, 300.0)));
    assert_eq!(g.groups, vec!["Level 1", "Level 2", "Level 3"]);
    assert_eq!(node_color(&g, &g.nodes[0]), "series-1");
    assert_eq!(node_color(&g, &g.nodes[1]), "series-2");
    assert_eq!(node_color(&g, &g.nodes[2]), "series-2");
    assert_eq!(node_color(&g, &g.nodes[3]), "series-3");
    let mixed = build(&json!({"nodes":[{"id":"a","group":"g"},{"id":"b"}]}), None);
    assert_eq!(node_color(&mixed, &mixed.nodes[1]), "muted");
    let none = build(&json!({"nodes":[{"id":"a"},{"id":"b"}]}), None);
    assert_eq!(node_color(&none, &none.nodes[0]), "series-1");
}

#[test]
fn an_explicit_node_color_wins_and_an_empty_one_falls_to_the_group_color() {
    let g = build(&deploy_props(), None);
    assert_eq!(node_color(&g, &g.nodes[0]), "success");
    assert_eq!(node_color(&g, &g.nodes[2]), "series-2");
}

#[test]
fn the_selected_node_is_larger_with_a_bold_highlight_ring_and_others_are_not() {
    let theme = DiagramTheme::heritage(false);
    let g = build(&deploy_props(), None);
    let items = derive_items(&g, &theme);
    let c = circles(&items);
    assert_eq!(c[0].radius, Some(14.0));
    assert_eq!(c[0].line_width, Some(theme.strokes[2]));
    assert_eq!(c[1].radius, Some(10.0));
    assert_eq!(c[1].line_width, Some(theme.strokes[0]));
    let mut p = deploy_props();
    p.as_object_mut().unwrap().remove("selectedNodeId");
    let g2 = build(&p, None);
    let items2 = derive_items(&g2, &theme);
    assert_eq!(
        circles(&items2)[0].radius,
        Some(10.0),
        "control: nothing selected"
    );
}

#[test]
fn edges_draw_first_faint_with_labels_at_the_midpoint_and_at_the_link_opacity() {
    let theme = DiagramTheme::heritage(false);
    let g = build(&deploy_props(), None);
    let items = derive_items(&g, &theme);
    let Item::Shape(first) = &items[0] else {
        panic!()
    };
    assert_eq!(first.ty, shape::ShapeType::Line);
    assert_eq!(first.opacity, Some(0.18));
    assert_eq!(first.line_width, Some(theme.strokes[0]));
    let Item::Shape(label) = &items[1] else {
        panic!()
    };
    assert_eq!(label.text.as_deref(), Some("sql"));
    let (a, b) = (&g.nodes[0], &g.nodes[1]);
    assert!((label.x.unwrap() as f64 - (a.x + b.x) / 2.0).abs() < 1e-3);
    assert!((label.y.unwrap() as f64 - ((a.y + b.y) / 2.0 - 4.0)).abs() < 1e-3);
    let mut p = deploy_props();
    p["showLabels"] = json!(false);
    let off = derive_items(&build(&p, None), &theme);
    assert!(
        !off.iter()
            .any(|i| matches!(i, Item::Shape(s) if s.ty == shape::ShapeType::Text)),
        "control: showLabels off"
    );
}

#[test]
fn proposed_nodes_are_muted_dotted_with_a_pulse_ring_and_suggested_get_a_ring_and_dot() {
    let theme = DiagramTheme::heritage(false);
    let props = json!({"nodes":[
        {"id":"p","label":"p","mark":{"kind":"proposed","suggestionId":"s1"}},
        {"id":"s","label":"s","mark":{"kind":"suggested","suggestionId":"s2"}},
        {"id":"n","label":"n"}
    ], "layout":"grid", "proposedEdges":[{"source":"p","target":"s"}]});
    let g = build(&props, None);
    let items = derive_items(&g, &theme);
    let Item::Shape(proposed_edge) = &items[0] else {
        panic!()
    };
    assert_eq!(
        (proposed_edge.dash, proposed_edge.opacity),
        (Some(Dash::Dashed), Some(0.4))
    );
    let c = circles(&items);
    assert_eq!(c[0].dash, Some(Dash::Dotted));
    assert_eq!(c[0].opacity, Some(0.55));
    assert_eq!(c[1].radius, Some(14.0), "pulse ring at radius + 6, phase 0");
    assert!((c[1].opacity.unwrap() - 0.22).abs() < 1e-6);
    assert_eq!(c[2].radius, Some(8.0));
    assert_eq!(c[3].radius, Some(11.0), "suggested ring at radius + 3");
    assert_eq!(
        items
            .iter()
            .filter(|i| matches!(i, Item::Dot { .. }))
            .count(),
        1
    );
    let plain = circles(&items).len();
    assert_eq!(plain, 5, "p circle+ring, s circle+ring, n circle");
}

#[test]
fn a_badge_over_one_draws_a_disc_and_a_haloless_600_count_and_one_does_not() {
    let props = json!({"nodes":[{"id":"a","label":"a","badge":5,"size":20}],"interactive":false});
    let theme = DiagramTheme::heritage(false);
    let with = derive_items(&build(&props, None), &theme);
    assert_eq!(
        circles(&with)[1].radius,
        Some(9.0),
        "badge radius clamp(7, 11, 0.45 * 20)"
    );
    let cmds = map(&props, None).unwrap().packet.draw_commands;
    let texts: Vec<_> = cmds
        .iter()
        .filter_map(|c| match c {
            DrawCommand::Text {
                text, weight, halo, ..
            } => Some((text.clone(), *weight, halo.is_some())),
            _ => None,
        })
        .collect();
    assert_eq!(
        texts,
        vec![("a".to_string(), 600, true), ("5".to_string(), 600, false)]
    );
    let one = derive_items(
        &build(&json!({"nodes":[{"id":"a","label":"a","badge":1}]}), None),
        &theme,
    );
    assert_eq!(circles(&one).len(), 1, "control: badge 1 draws nothing");
}

#[test]
fn a_selected_node_label_is_weight_700_and_an_unselected_one_keeps_the_theme_weight() {
    let weights = |p: &Value| -> Vec<u16> {
        map(p, None)
            .unwrap()
            .packet
            .draw_commands
            .iter()
            .filter_map(|c| match c {
                DrawCommand::Text { weight, .. } => Some(*weight),
                _ => None,
            })
            .collect()
    };
    let mut p = json!({"nodes":[{"id":"a","label":"a"}],"selectedNodeId":"a"});
    assert_eq!(weights(&p), vec![700]);
    p["selectedNodeId"] = json!("zzz");
    assert_eq!(weights(&p), vec![600]);
}

#[test]
fn the_scene_clears_to_ground_paints_in_order_and_fits_the_viewport() {
    let scene = map(&path_props(), Some((390.0, 300.0))).unwrap();
    assert_eq!(scene.packet.pattern, "graph-canvas");
    assert_eq!((scene.packet.width, scene.packet.height), (390.0, 300.0));
    let DrawCommand::Clear { color } = path_of(&scene, 0) else {
        panic!()
    };
    assert_eq!(*color, DiagramTheme::heritage(false).ground);
    let n = scene.packet.draw_commands.len();
    assert!(
        matches!(scene.packet.draw_commands[n - 1], DrawCommand::Text { .. }),
        "last node label is last"
    );
}

#[test]
fn a_tap_on_a_node_emits_the_node_click_event_with_id_and_row_and_empty_space_emits_nothing() {
    let scene = map(&path_props(), Some((390.0, 300.0))).unwrap();
    let g = build(&path_props(), Some((390.0, 300.0)));
    let (vf, ..) = fit(g.width as f32, g.height as f32, Some((390.0, 300.0)));
    let xf = composed(vf, g.view);
    let n = &g.nodes[1];
    let (px, py) = xf.p(n.x as f32, n.y as f32);
    let emits = scene.emits_at(px, py);
    assert_eq!(emits.len(), 1);
    assert_eq!(emits[0].event, "GRAPH_NODE_CLICK");
    assert_eq!(emits[0].payload["id"], "derivatives");
    assert_eq!(emits[0].payload["row"]["label"], "Derivatives");
    assert!(scene.emits_at(0.5, 0.5).is_empty(), "control: empty corner");
    let mut quiet = path_props();
    quiet.as_object_mut().unwrap().remove("nodeClickEvent");
    assert!(
        map(&quiet, Some((390.0, 300.0))).unwrap().hits.is_empty(),
        "control: no handler authored"
    );
    let mut off = path_props();
    off["interactive"] = json!(false);
    off["draggable"] = json!(false);
    assert!(
        map(&off, Some((390.0, 300.0))).unwrap().hits.is_empty(),
        "control: not interactive"
    );
}

#[test]
fn mark_click_takes_priority_over_node_click_and_badge_click_over_both() {
    let props = json!({
        "layout":"grid", "nodeClickEvent":"N", "onNodeClick":"NC", "onMarkClick":"M", "onBadgeClick":"B",
        "nodes":[{"id":"m","mark":{"kind":"suggested","suggestionId":"s"},"size":20},{"id":"b","badge":3,"size":20},{"id":"p","size":20}],
    });
    let g = build(&props, None);
    let scene = map(&props, None).unwrap();
    let xf = composed(fit(g.width as f32, g.height as f32, None).0, g.view);
    let at = |i: usize| xf.p(g.nodes[i].x as f32, g.nodes[i].y as f32);
    let (mx, my) = at(0);
    let e = scene.emits_at(mx, my);
    assert_eq!(
        e.iter().map(|e| e.event.as_str()).collect::<Vec<_>>(),
        vec!["M"]
    );
    let (px, py) = at(2);
    let e = scene.emits_at(px, py);
    assert_eq!(
        e.iter().map(|e| e.event.as_str()).collect::<Vec<_>>(),
        vec!["N", "NC"]
    );
    let (bx, by) = at(1);
    let (bbx, bby, _) = badge_geometry(&g.nodes[1], 20.0);
    let (sx, sy) = xf.p(bbx as f32, bby as f32);
    assert_eq!(scene.emits_at(sx, sy)[0].event, "B");
    assert_eq!(
        scene
            .emits_at(bx, by)
            .iter()
            .map(|e| e.event.as_str())
            .collect::<Vec<_>>(),
        vec!["N", "NC"],
        "control: badge body away from the badge"
    );
}

#[test]
fn an_empty_graph_loading_or_error_leaves_a_blank_canvas() {
    for p in [
        json!({}),
        json!({"isLoading": true, "nodes":[{"id":"a"}]}),
        json!({"error":{"message":"x"},"nodes":[{"id":"a"}]}),
    ] {
        let s = map(&p, Some((100.0, 100.0))).unwrap();
        assert_eq!(s.packet.draw_commands.len(), 1);
    }
    assert!(
        map(&json!({"nodes":[{"id":"a"}]}), None)
            .unwrap()
            .packet
            .draw_commands
            .len()
            > 1,
        "control"
    );
}

#[test]
fn scene_width_defaults_to_800_without_a_viewport_and_height_is_the_prop() {
    let s = map(&path_props(), None).unwrap();
    assert_eq!((s.packet.width, s.packet.height), (800.0, 420.0));
}
