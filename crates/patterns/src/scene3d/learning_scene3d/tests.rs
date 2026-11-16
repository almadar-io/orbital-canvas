use super::*;

const EPS: f64 = 1e-9;

fn drawables(v: &Value) -> &Vec<Value> {
    v["drawables"].as_array().expect("drawables")
}

fn close(a: &Value, b: f64) -> bool {
    a.as_f64().is_some_and(|a| (a - b).abs() < EPS)
}

fn at(v: &Value, p: P3) -> bool {
    close(&v["position"]["x"], p[0])
        && close(&v["position"]["y"], p[1])
        && close(&v["position"]["z"], p[2])
}

fn rot(v: &Value, r: [f64; 3]) -> bool {
    (0..3).all(|i| close(&v["rotation"][i], r[i]))
}

fn err(result: Result<Value, CanvasSceneError>) -> String {
    match result {
        Err(CanvasSceneError::Unsupported3DContract(m)) => m,
        other => panic!("expected a contract error, got {other:?}"),
    }
}

#[test]
fn an_empty_node_is_the_wrappers_fixed_canvas() {
    let node =
        learning_canvas_node_3d(&json!({"type": "physics-canvas", "mode": "3d"})).expect("maps");
    assert_eq!(
        node,
        json!({
            "type": "canvas", "mode": "3d", "drawables": [],
            "camera": {"mode": "perspective"}, "showGrid": false,
        })
    );
    let scene = learning_scene3d(
        &json!({"type": "chemistry-canvas", "mode": "3d"}),
        600.0,
        400.0,
        &SceneTheme::default(),
    )
    .expect("scene");
    assert!(scene.meshes.is_empty());
    assert!(
        scene.grid.is_none(),
        "learning scenes hide the grid unless asked"
    );
    assert!(scene.shadows, "shadows default on, the host default");
}

#[test]
fn wrapper_settings_pass_through_and_interactive_drives_orbit() {
    let node = learning_canvas_node_3d(&json!({
        "type": "biology-canvas", "mode": "3d", "showGrid": true, "shadows": false,
        "interactive": false, "backgroundColor": "#102030", "series": ["#ff0000"],
        "camera": {"mode": "isometric", "zoom": 2, "azimuth": 0.5},
        "lighting": {"toneMapping": "none"}, "keyMap": {"Space": "TOGGLE"},
        "title": "Cell", "width": 600, "height": 400, "onShapeClick": "PICK",
    }))
    .expect("maps");
    assert_eq!(
        node["camera"],
        json!({"mode": "isometric", "zoom": 2, "azimuth": 0.5, "controls": false})
    );
    assert_eq!(node["showGrid"], json!(true));
    assert_eq!(node["shadows"], json!(false));
    assert_eq!(node["backgroundColor"], json!("#102030"));
    assert_eq!(node["series"], json!(["#ff0000"]));
    assert_eq!(node["lighting"], json!({"toneMapping": "none"}));
    assert_eq!(node["keyMap"], json!({"Space": "TOGGLE"}));
    assert!(
        node.get("title").is_none(),
        "control: the card title is not scene data"
    );
}

#[test]
fn unknown_and_ignored_props_error_by_name() {
    let unknown = err(learning_canvas_node_3d(
        &json!({"type": "physics-canvas", "mode": "3d", "bogus": 1}),
    ));
    assert!(unknown.starts_with("physics-canvas.bogus:"), "{unknown}");
    assert!(
        learning_canvas_node_3d(&json!({
            "type": "physics-canvas", "mode": "3d", "bodies": [{"x": 0, "y": 0, "mass": 5}],
        }))
        .is_ok(),
        "a body row's own fields (mass) are the row's, not props"
    );
    let flat = err(learning_canvas_node_3d(&json!({
        "type": "physics-canvas", "mode": "3d", "sceneObjects": [{"kind": "ground"}],
    })));
    assert!(flat.starts_with("physics-canvas.sceneObjects:"), "{flat}");
    let missing = err(learning_canvas_node_3d(&json!({
        "type": "chemistry-canvas", "mode": "3d", "atoms": [{"y": 0}],
    })));
    assert!(
        missing.starts_with("chemistry-canvas.atoms[0].x: is required"),
        "{missing}"
    );
    let camera_pos = err(learning_canvas_node_3d(&json!({
        "type": "biology-canvas", "mode": "3d", "camera": {"pos": {"x": 1, "y": 2}},
    })));
    assert!(
        camera_pos.starts_with("biology-canvas.camera.pos:"),
        "{camera_pos}"
    );
    assert!(
        learning_canvas_node_3d(&json!({
            "type": "physics-canvas", "mode": "3d", "sceneObjects": [], "readouts": [],
            "animate": false, "keyMap": {"item": "INIT"},
        }))
        .is_ok(),
        "control: the config defaults ask nothing"
    );
    assert!(matches!(
        learning_canvas_node_3d(&json!({"type": "physics-canvas", "mode": "2d"})),
        Err(CanvasSceneError::UnsupportedMode(m)) if m == "2d"
    ));
}

#[test]
fn a_2d_only_prop_left_empty_is_declared_and_control_a_drawn_one_is_refused() {
    let node = |meters: Value| json!({"type": "physics-canvas", "mode": "3d", "bodies": [{"id": "b", "x": 1, "y": 2}], "meters": meters});
    assert!(learning_scene3d(&node(json!([])), 600.0, 400.0, &SceneTheme::default()).is_ok());
    let refused = learning_scene3d(
        &node(json!([{"label": "v", "value": 1}])),
        600.0,
        400.0,
        &SceneTheme::default(),
    )
    .expect_err("a drawn 2D-only prop");
    assert!(
        refused
            .to_string()
            .contains("physics-canvas.meters: 2D only"),
        "{refused}"
    );
}

#[test]
fn cylinder_between_spans_and_tilts_the_segment() {
    let c = cylinder_between([0.0, 0.0, 0.0], [3.0, 4.0, 0.0], 0.1, "ink").expect("cyl");
    assert!(at(&c, [1.5, 2.0, 0.0]));
    assert!(close(&c["height"], 5.0));
    // wx = 3, wy (height) = 0, wz = 4.
    assert!(rot(&c, [0.0, 4.0_f64.atan2(-3.0), PI / 2.0]));
    assert_eq!(c["segments"], json!(12));
    assert!(cylinder_between([1.0; 3], [1.0; 3], 0.1, "ink").is_none());
}

#[test]
fn physics_bodies_constraints_and_arrows() {
    let node = learning_canvas_node_3d(&json!({
        "type": "physics-canvas", "mode": "3d", "showForces": true,
        "bodies": [
            {"id": "a", "x": 0, "y": 0},
            {"id": "b", "x": 2, "y": 3, "z": 1, "radius": 1, "label": "B", "vx": 0.1, "fz": 0.05},
        ],
        "constraints": [{"from": "a", "to": "b"}, {"from": "a", "to": "gone"}],
    }))
    .expect("maps");
    let d = drawables(&node);
    // rod, sphere a, sphere b, label b, velocity arrow, force arrow.
    assert_eq!(d.len(), 6);
    let rod = &d[0];
    assert!(
        close(&rod["radius"], 0.075),
        "max(0.05, min(0.5, 1) * 0.15)"
    );
    assert!(at(rod, [1.0, 1.5, 0.5]));
    assert_eq!(rod["material"]["color"], json!("ink"));
    assert_eq!(d[1]["material"]["color"], json!("series-1"));
    assert_eq!(d[1]["segments"], json!(24));
    assert_eq!(d[2]["material"]["color"], json!("series-2"));
    assert_eq!(d[2]["id"], json!("b"));
    assert_eq!(
        d[3],
        json!({"type": "draw-text", "text": "B", "position": {"x": 2.0, "y": 3.0, "z": 2.0}, "color": "ink"})
    );
    // Velocity 0.1 * 20 = 2 along x, shaft max(0.05, 0.15) = 0.15: tip min(1.2, 0.7) = 0.7, k = 0.65.
    let vel = &d[4];
    assert_eq!(vel["type"], json!("draw-group"));
    assert!(at(vel, [2.0, 3.0, 1.0]));
    let (shaft, tip) = (&vel["items"][0], &vel["items"][1]);
    assert!(at(shaft, [0.65, 0.0, 0.0]) && close(&shaft["height"], 1.3));
    assert!(rot(shaft, [0.0, PI, PI / 2.0]));
    assert!(close(&shaft["radius"], 0.15));
    assert!(at(tip, [1.65, 0.0, 0.0]) && close(&tip["height"], 0.7));
    assert!(close(&tip["radius"], 0.45));
    assert_eq!(tip["shape"], json!("cone"));
    assert_eq!(tip["material"]["color"], json!("series-3"));
    // Force 0.05 * 20 = 1 up: tip min(1.2, 0.35) = 0.35.
    let force = &d[5]["items"];
    assert!(at(&force[0], [0.0, 0.0, 0.325]) && close(&force[0]["height"], 0.65));
    assert!(rot(&force[0], [0.0, PI, 0.0]));
    assert!(close(&force[1]["height"], 0.35));
    assert_eq!(force[1]["material"]["color"], json!("series-5"));
}

#[test]
fn physics_trails_tube_and_cap_at_128_segments() {
    let short: Vec<Value> = [[0.0, 0.0], [1.0, 0.0], [1.0, 0.0], [1.0, 2.0]]
        .iter()
        .map(|p| json!({"x": p[0], "y": p[1]}))
        .collect();
    let long: Vec<Value> = (0..301).map(|i| json!({"x": i, "y": 0})).collect();
    let node = learning_canvas_node_3d(&json!({
        "type": "physics-canvas", "mode": "3d",
        "trails": [{"points": short, "opacity": 0.4}, {"points": long, "color": "#00ff00", "width": 0.2}],
    }))
    .expect("maps");
    let d = drawables(&node);
    assert_eq!(d.len(), 2 + 128, "the degenerate segment is skipped");
    assert!(close(&d[0]["radius"], 0.05));
    assert_eq!(d[0]["material"]["color"], json!("guide"));
    assert!(close(&d[0]["opacity"], 0.4));
    assert!(
        d[2].get("opacity").is_none(),
        "control: no opacity unless set"
    );
    // step = 300 / 128; kept[1] = round(2.34375) = 2, kept[127] = round(297.65625) = 298.
    assert!(at(&d[2], [1.0, 0.0, 0.0]) && close(&d[2]["height"], 2.0));
    assert!(at(&d[129], [299.0, 0.0, 0.0]) && close(&d[129]["height"], 2.0));
    assert!(close(&d[129]["radius"], 0.2));
}

#[test]
fn physics_surface_bands_and_vectors3d() {
    let node = learning_canvas_node_3d(&json!({
        "type": "physics-canvas", "mode": "3d", "vectorScale": 2,
        "surface3d": {"nx": 2, "ny": 2, "heights": [0, 0, 1, 1], "opacity": 0.8,
            "bands": [{"min": 0.5, "color": "#ff0000"}, {"color": "#0000ff"}]},
        "vectors3d": [{"id": "E", "x": 0, "y": 0, "dx": 1, "dy": 0, "label": "E"}],
    }))
    .expect("maps");
    let d = drawables(&node);
    assert_eq!(d.len(), 4);
    let vertices = json!([
        [-0.5, -0.5, 0.0],
        [0.5, -0.5, 0.0],
        [-0.5, 0.5, 1.0],
        [0.5, 0.5, 1.0]
    ]);
    // Face [0,1,2] centroid 1/3 takes the catch-all band; face [1,3,2] centroid 2/3 the 0.5 band.
    assert_eq!(d[0]["vertices"], vertices);
    assert_eq!(d[0]["faces"], json!([[0, 1, 2]]));
    assert_eq!(
        d[0]["material"],
        json!({"color": "#0000ff", "flatShading": true, "side": "double"})
    );
    assert_eq!(d[1]["faces"], json!([[1, 3, 2]]));
    assert_eq!(d[1]["material"]["color"], json!("#ff0000"));
    assert!(close(&d[1]["opacity"], 0.8));
    // 1 * 2 along x, default shaft 0.08: tip min(0.64, 0.7) = 0.64.
    let arrow = &d[2];
    assert_eq!(arrow["id"], json!("E"));
    assert!(close(&arrow["items"][0]["height"], 1.36));
    assert!(close(&arrow["items"][1]["radius"], 0.24));
    assert_eq!(arrow["items"][0]["material"]["color"], json!("series-5"));
    assert!(at(&d[3], [2.0, 0.0, 0.0]) && d[3]["type"] == json!("draw-text"));

    let bare = learning_canvas_node_3d(&json!({
        "type": "physics-canvas", "mode": "3d",
        "surface3d": {"nx": 2, "ny": 2, "heights": [0, 0, 1, 1]},
    }))
    .expect("maps");
    assert_eq!(drawables(&bare).len(), 1, "control: no bands is one sheet");
    assert_eq!(drawables(&bare)[0]["material"]["color"], json!("series-1"));
    let short = err(learning_canvas_node_3d(&json!({
        "type": "physics-canvas", "mode": "3d", "surface3d": {"nx": 2, "ny": 2, "heights": [0]},
    })));
    assert!(
        short.starts_with("physics-canvas.surface3d.heights:"),
        "{short}"
    );
}

#[test]
fn biology_nodes_edges_and_helix() {
    let node = learning_canvas_node_3d(&json!({
        "type": "biology-canvas", "mode": "3d",
        "nodes": [
            {"id": "a", "x": 0, "y": 0, "label": ""},
            {"id": "b", "x": 4, "y": 0, "radius": 1, "shape": "capsule", "opacity": 0.5, "label": "B"},
        ],
        "edges": [{"from": "a", "to": "b", "label": "bond"}],
        "helix3d": {"count": 2, "unwoundCount": 1, "rungs": [{"id": "r0", "label": "AT", "color": "#ff00ff"}]},
    }))
    .expect("maps");
    let d = drawables(&node);
    // edge, edge label, node a, node b, label b, then 11 helix drawables.
    assert_eq!(d.len(), 5 + 11);
    assert!(
        close(&d[0]["radius"], 0.06),
        "max(0.04, min(0.5, 1) * 0.12)"
    );
    assert!(at(&d[1], [2.0, 0.0, 0.0]) && d[1]["text"] == json!("bond"));
    assert_eq!(d[2]["shape"], json!("sphere"));
    assert_eq!(d[3]["shape"], json!("capsule"));
    assert!(close(&d[3]["opacity"], 0.5));
    assert_eq!(d[3]["material"]["color"], json!("series-2"));
    assert!(at(&d[4], [4.0, 0.0, 1.0]));

    let h = &d[5..];
    // Rung 0 is unwound: spread 1.8 at theta 0, y = (0 - 0.5) * 0.34.
    assert_eq!(h[0]["id"], json!("hx-a-0"));
    assert!(at(&h[0], [1.8, -0.17, 0.0]));
    assert!(close(&h[0]["radius"], 0.16));
    assert_eq!(h[0]["material"]["color"], json!("series-1"));
    assert!(at(&h[1], [-1.8, -0.17, 1.8 * PI.sin()]));
    assert_eq!(h[1]["material"]["color"], json!("series-2"));
    assert_eq!(h[2]["shape"], json!("cylinder"));
    assert!(close(&h[2]["radius"], 0.12) && close(&h[2]["height"], 3.6));
    assert_eq!(h[2]["material"]["color"], json!("#ff00ff"));
    assert_eq!(h[3]["id"], json!("r0"));
    assert_eq!(h[4]["text"], json!("AT"));
    assert!(close(&h[4]["position"]["z"], 1.8 * PI.sin() / 2.0 + 0.12));
    // Rung 1: theta 36°, wound, y = 0.17; a strand segment joins the beads.
    let t = 36.0_f64.to_radians();
    assert!(at(&h[5], [t.cos(), 0.17, t.sin()]));
    assert_eq!(h[7]["shape"], json!("cylinder"));
    assert!(close(&h[7]["radius"], 0.16));
    assert!(at(&h[7], [(1.8 + t.cos()) / 2.0, 0.0, t.sin() / 2.0]));
    assert_eq!(
        h[9]["material"]["color"],
        json!("muted"),
        "a padded rung is muted"
    );
    assert!(
        h[10].get("id").is_none(),
        "control: a padded rung's marker has no id"
    );
}

#[test]
fn chemistry_bond_orders_arrows_and_element_tones() {
    let node = learning_canvas_node_3d(&json!({
        "type": "chemistry-canvas", "mode": "3d",
        "atoms": [
            {"id": "o", "x": 0, "y": 0, "element": "O"},
            {"id": "h", "x": 2, "y": 0, "element": "H"},
            {"id": "c", "x": 0, "y": 2},
            {"id": "o2", "x": 2, "y": 2, "element": "O", "color": "#123456"},
        ],
        "bonds": [
            {"from": "o", "to": "h"},
            {"from": "o", "to": "h", "type": "double"},
            {"from": "o", "to": "h", "type": "triple"},
        ],
        "arrows": [{"x": 0, "y": 0, "angle": 90, "length": 2, "label": "heat"}],
    }))
    .expect("maps");
    let d = drawables(&node);
    // 1 + 2 + 3 bonds, arrow + label, 4 atoms + 3 element labels.
    assert_eq!(d.len(), 6 + 2 + 7);
    // Perpendicular to +x is (0, -1, 0); radius max(0.06, 0.45 * 0.22) = 0.099, step 0.2178.
    assert!(close(&d[0]["radius"], 0.099));
    assert!(at(&d[0], [1.0, 0.0, 0.0]));
    assert!(at(&d[1], [1.0, 0.2178, 0.0]) && at(&d[2], [1.0, -0.2178, 0.0]));
    assert!(
        at(&d[3], [1.0, 0.2178, 0.0])
            && at(&d[4], [1.0, 0.0, 0.0])
            && at(&d[5], [1.0, -0.2178, 0.0])
    );
    let arrow = &d[6];
    assert!(close(&arrow["items"][0]["radius"], 0.08));
    assert_eq!(arrow["items"][0]["material"]["color"], json!("ink"));
    assert!(at(&d[7], [(PI / 2.0).cos(), 1.0, 0.0]) && d[7]["text"] == json!("heat"));
    assert_eq!(d[8]["material"]["color"], json!("series-1"));
    assert!(close(&d[8]["radius"], 0.45));
    assert!(at(&d[9], [0.0, 0.0, 0.45]) && d[9]["text"] == json!("O"));
    assert_eq!(d[10]["material"]["color"], json!("series-2"));
    assert_eq!(
        d[12]["material"]["color"],
        json!("series-1"),
        "no element shares the first slot"
    );
    assert_eq!(d[13]["material"]["color"], json!("#123456"));
    assert_eq!(d[13]["id"], json!("o2"));
}

#[test]
fn chemistry_lattice_sites_bonds_selection_and_dimming() {
    let node = learning_canvas_node_3d(&json!({
        "type": "chemistry-canvas", "mode": "3d",
        "lattice3d": {
            "basis": [{"key": "a", "dx": 0, "dy": 0, "dz": 0, "element": "Na", "xEdge": true}],
            "nx": 1, "ny": 1, "nz": 1, "showLabels": true, "selectedId": "lat-a-1-0-0",
            "bonds": [{"from": "a", "to": "a", "dx": 1}],
        },
    }))
    .expect("maps");
    let d = drawables(&node);
    // Two sites (x edge), two labels, one bond (the second would leave the block).
    assert_eq!(d.len(), 5);
    assert_eq!(d[0]["id"], json!("lat-a-0-0-0"));
    assert!(at(&d[0], [-1.0, -1.0, -1.0]) && close(&d[0]["radius"], 0.3));
    assert_eq!(d[0]["material"]["color"], json!("series-1"));
    assert!(at(&d[1], [-1.0, -1.0, -0.7]) && d[1]["text"] == json!("Na"));
    assert_eq!(d[2]["id"], json!("lat-a-1-0-0"));
    assert!(at(&d[2], [1.0, -1.0, -1.0]) && close(&d[2]["radius"], 0.42));
    assert_eq!(d[2]["material"]["color"], json!("highlight"));
    assert!(at(&d[4], [0.0, -1.0, -1.0]) && close(&d[4]["height"], 2.0));
    assert!(close(&d[4]["radius"], 0.06));
    assert_eq!(d[4]["material"]["color"], json!("ink"));

    let dim = learning_canvas_node_3d(&json!({
        "type": "chemistry-canvas", "mode": "3d",
        "lattice3d": {
            "basis": [{"key": "a", "dx": 0, "dy": 0, "dz": 0}],
            "nx": 3, "ny": 1, "nz": 1, "highlightCell": true,
            "bonds": [{"from": "a", "to": "a", "dx": 1, "color": "#ffffff"}],
        },
    }))
    .expect("maps");
    let d = drawables(&dim);
    assert_eq!(d.len(), 3 + 2);
    assert!(at(&d[0], [-3.0, -1.0, -1.0]) && at(&d[2], [1.0, -1.0, -1.0]));
    assert_eq!(
        d[1]["material"]["color"],
        json!("series-1"),
        "i = 1 is still in the cell"
    );
    assert_eq!(d[2]["material"]["color"], json!("muted"));
    assert_eq!(d[3]["material"]["color"], json!("#ffffff"));
    assert_eq!(
        d[4]["material"]["color"],
        json!("muted"),
        "a bond to a dimmed site dims"
    );
}

#[test]
fn the_scene_resolves_tones_and_the_series_override() {
    let mut theme = SceneTheme::default();
    theme.tones.insert("series-2".into(), "#00ff00".into());
    theme.tones.insert("ink".into(), "#000000".into());
    let node = json!({
        "type": "physics-canvas", "mode": "3d", "showVelocity": false,
        "bodies": [{"x": 0, "y": 0, "color": "#ff0000"}, {"x": 2, "y": 0}, {"x": 4, "y": 0, "color": ""}],
    });
    let scene = learning_scene3d(&node, 600.0, 400.0, &theme).expect("scene");
    assert_eq!(scene.meshes.len(), 3);
    let linear = |c: [f32; 3]| orbital_canvas_base::scene3d::model::srgb_to_linear_rgb(c);
    assert_eq!(scene.meshes[0].material.color, linear([1.0, 0.0, 0.0]));
    assert_eq!(scene.meshes[1].material.color, linear([0.0, 1.0, 0.0]));
    assert_eq!(
        scene.meshes[2].material.color,
        linear([0.0, 0.0, 0.0]),
        "an empty color takes ink"
    );

    let mut series = node.clone();
    series["series"] = json!(["#0000ff"]);
    let scene = learning_scene3d(&series, 600.0, 400.0, &theme).expect("scene");
    assert_eq!(scene.meshes[1].material.color, linear([0.0, 0.0, 1.0]));
    assert_eq!(
        scene.meshes[0].material.color,
        linear([1.0, 0.0, 0.0]),
        "control: a literal stays"
    );

    let labelled =
        json!({"type": "physics-canvas", "mode": "3d", "bodies": [{"x": 0, "y": 0, "label": "m"}]});
    let unlabelled = json!({"type": "physics-canvas", "mode": "3d", "bodies": [{"x": 0, "y": 0}]});
    let with = learning_scene3d(&labelled, 600.0, 400.0, &theme).expect("a label draws as 3D text");
    let without = learning_scene3d(&unlabelled, 600.0, 400.0, &theme).expect("control");
    assert_eq!(
        with.meshes.len(),
        without.meshes.len() + 1,
        "the label is one billboard more"
    );
}
