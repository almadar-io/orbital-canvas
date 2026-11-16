use super::*;
use serde_json::json;

const TOL: f32 = 1e-4;

fn close(actual: [f32; 3], expected: [f32; 3]) {
    for i in 0..3 {
        assert!(
            (actual[i] - expected[i]).abs() < TOL,
            "{actual:?} != {expected:?}"
        );
    }
}

fn scene(props: Value) -> Result<Scene3D, CanvasSceneError> {
    let mut canvas = json!({"type": "canvas", "mode": "3d", "showGrid": false});
    for (key, value) in props.as_object().unwrap() {
        canvas[key] = value.clone();
    }
    scene_from_canvas_node(&canvas, 800.0, 600.0)
}

fn scene_at(props: Value, elapsed_ms: f64) -> Result<Scene3D, CanvasSceneError> {
    let mut canvas = json!({"type": "canvas", "mode": "3d", "showGrid": false});
    for (key, value) in props.as_object().unwrap() {
        canvas[key] = value.clone();
    }
    scene_from_canvas_node_at(&canvas, 800.0, 600.0, &SceneTheme::default(), elapsed_ms)
}

fn message(result: Result<Scene3D, CanvasSceneError>) -> String {
    match result {
        Err(CanvasSceneError::Unsupported3DContract(m)) => m,
        other => panic!("expected an unsupported-contract error, got {other:?}"),
    }
}

fn two_meshes_camera(camera: Value) -> Camera {
    scene(json!({
        "camera": camera,
        "drawables": [
            {"type": "draw-mesh", "position": {"x": 0, "y": 0}},
            {"type": "draw-mesh", "position": {"x": 6, "y": 2}}
        ]
    }))
    .unwrap()
    .camera
}

#[test]
fn camera_framing_matches_each_three_host_mode() {
    let target = [3.0, 0.0, 1.0];
    for (mode, eye) in [
        ("isometric", [9.0, 4.8, 7.0]),
        ("perspective", [9.0, 6.0, 7.0]),
        ("top-down", [3.0, 12.0, 3.1]),
        ("front", [3.0, 1.92, 7.9]),
    ] {
        let camera = two_meshes_camera(json!({"mode": mode}));
        close(camera.eye, eye);
        close(camera.target, target);
        assert_eq!(camera.up, [0.0, 1.0, 0.0]);
        assert_eq!(
            camera.projection,
            Projection::Perspective {
                fov_y_degrees: 45.0
            }
        );
        assert_eq!((camera.near, camera.far), (0.1, 1000.0));
    }
    let follow = two_meshes_camera(json!({"mode": "follow"}));
    close(follow.eye, [3.0, 12.0, 15.0]);
    close(follow.target, target);
    let chase = two_meshes_camera(json!({"mode": "chase", "fov": 60}));
    close(chase.eye, [3.0, 4.0, -6.0]);
    assert_eq!(
        chase.projection,
        Projection::Perspective {
            fov_y_degrees: 60.0
        }
    );
    let tracked =
        two_meshes_camera(json!({"mode": "follow", "target": {"x": 2, "y": 1, "z": 0.5}}));
    close(tracked.eye, [2.0, 12.5, 15.0]);
    close(tracked.target, [2.0, 0.5, 1.0]);
    let orbited = two_meshes_camera(
        json!({"mode": "perspective", "zoom": 2, "azimuth": std::f64::consts::FRAC_PI_2}),
    );
    close(orbited.eye, [0.0, 3.0, 4.0]);
}

#[test]
fn orbit_controls_cap_the_polar_angle_and_the_distance() {
    let grazing = two_meshes_camera(json!({"mode": "perspective", "elevation": 0}));
    close(grazing.eye, [8.970025, 0.847115, 6.970025]);
    let close_in = two_meshes_camera(json!({"mode": "isometric", "zoom": 10}));
    let offset = [
        close_in.eye[0] - 3.0,
        close_in.eye[1],
        close_in.eye[2] - 1.0,
    ];
    let distance = (offset[0] * offset[0] + offset[1] * offset[1] + offset[2] * offset[2]).sqrt();
    assert!((distance - 2.0).abs() < TOL);
    assert!((offset[1] / offset[0] - 0.8).abs() < TOL);
    assert!((offset[2] - offset[0]).abs() < TOL);
}

#[test]
fn pixels_per_unit_scales_framing_and_positions() {
    let s = scene(json!({
        "pixelsPerUnit": 32,
        "drawables": [
            {"type": "draw-mesh", "position": {"x": 0, "y": 0}, "pivot": "center"},
            {"type": "draw-mesh", "position": {"x": 320, "y": 64, "z": 3}, "pivot": "center"}
        ]
    }))
    .unwrap();
    close(s.camera.eye, [15.0, 8.0, 11.0]);
    close(s.camera.target, [5.0, 0.0, 1.0]);
    let t = s.meshes[1].transform;
    close([t[12], t[13], t[14]], [10.0, 3.0, 2.0]);
}

#[test]
fn polyhedron_swaps_scene_axes_and_rewinds_faces() {
    let s = scene(json!({
        "drawables": [{
            "type": "draw-mesh",
            "shape": "polyhedron",
            "position": {"x": 2, "y": 3, "z": 0.5},
            "vertices": [[0, 0, -1], [1, 0, -1], [0, 1, -1]],
            "faces": [[0, 1, 2]]
        }]
    }))
    .unwrap();
    let mesh = &s.meshes[0];
    assert_eq!(
        mesh.mesh.positions,
        vec![[0.0, -1.0, 0.0], [1.0, -1.0, 0.0], [0.0, -1.0, 1.0]]
    );
    assert_eq!(mesh.mesh.indices, vec![0, 2, 1]);
    for normal in &mesh.mesh.normals {
        close(*normal, [0.0, 1.0, 0.0]);
    }
    let t = mesh.transform;
    close([t[12], t[13], t[14]], [0.0, 1.5, 0.0]);
}

#[test]
fn groups_compose_transforms_and_multiply_opacity() {
    let s = scene(json!({
        "drawables": [{
            "type": "draw-group",
            "position": {"x": 0, "y": 0},
            "rotate": std::f64::consts::FRAC_PI_2,
            "scale": 2,
            "opacity": 0.5,
            "items": [{
                "type": "draw-group",
                "position": {"x": 1, "y": 0},
                "opacity": 0.5,
                "items": [{
                    "type": "draw-mesh",
                    "position": {"x": 1, "y": 0},
                    "pivot": "center",
                    "opacity": 0.8,
                    "material": {"opacity": 0.5}
                }]
            }]
        }]
    }))
    .unwrap();
    let mesh = &s.meshes[0];
    let t = mesh.transform;
    close([t[12], t[13], t[14]], [0.0, 0.0, 4.0]);
    close([t[0], t[1], t[2]], [0.0, 0.0, 2.0]);
    close([t[4], t[5], t[6]], [0.0, 2.0, 0.0]);
    assert!((mesh.material.opacity - 0.1).abs() < 1e-6);
    assert!(mesh.material.is_transparent());
}

#[test]
fn omitted_lighting_is_the_three_host_rig_under_the_theme_fallbacks() {
    let s = scene(json!({})).unwrap();
    let lights = s.lights;
    assert_eq!(
        lights.ambient,
        Some(AmbientLight {
            color: [1.0; 3],
            intensity: 0.55
        })
    );
    assert_eq!(
        lights.directional,
        Some(DirectionalLight {
            position: [10.0, 20.0, 10.0],
            target: [0.0; 3],
            color: [1.0; 3],
            intensity: 1.0,
            shadow: Some(ShadowSettings {
                map_size: 2048,
                bias: -0.0004,
                normal_bias: 0.04,
                extent: 5.0,
                near: 0.5,
                far: 500.0,
            }),
        })
    );
    let channel = |byte: u8| srgb_to_linear(byte as f32 / 255.0);
    assert_eq!(
        lights.hemisphere,
        Some(HemisphereLight {
            sky: [channel(0x87), channel(0xce), channel(0xeb)],
            ground: [channel(0x36), channel(0x2d), channel(0x1d)],
            intensity: 0.3,
        })
    );
    assert!(lights.points.is_empty());
    assert!(s.shadows);
    assert_eq!(s.tone_mapping, ToneMapping::Aces);
    assert_eq!(s.background, [0.0; 4]);
    close(s.camera.target, [5.0, 0.0, 5.0]);
    assert!(s.meshes.is_empty());

    let unshadowed = scene(json!({"shadows": false})).unwrap();
    assert!(!unshadowed.shadows);
    assert_eq!(unshadowed.lights.directional.unwrap().shadow, None);
}

#[test]
fn a_material_with_every_field_maps_each_one() {
    let s = scene(json!({
        "drawables": [{
            "type": "draw-mesh",
            "shape": "sphere",
            "position": {"x": 0, "y": 0},
            "opacity": 0.5,
            "castShadow": false,
            "receiveShadow": false,
            "material": {
                "kind": "physical",
                "color": "#ff0000",
                "metalness": 0.25,
                "roughness": 0.75,
                "emissive": "#00ff00",
                "emissiveIntensity": 2,
                "opacity": 0.5,
                "transmission": 0.3,
                "ior": 1.33,
                "flatShading": true,
                "side": "double",
                "map": "textures/rock.png"
            }
        }]
    }))
    .unwrap();
    let mesh = &s.meshes[0];
    assert_eq!(
        mesh.material,
        Material {
            kind: MaterialKind::Physical,
            color: [1.0, 0.0, 0.0],
            opacity: 0.25,
            metalness: 0.25,
            roughness: 0.75,
            emissive: [0.0, 1.0, 0.0],
            emissive_intensity: 2.0,
            transmission: 0.3,
            ior: 1.33,
            flat_shading: true,
            side: Side::Double,
            map: Some("textures/rock.png".into()),
            shader: None,
        }
    );
    assert!(!mesh.cast_shadow);
    assert!(!mesh.receive_shadow);
    let t = mesh.transform;
    close([t[12], t[13], t[14]], [0.0, 0.4, 0.0]);

    let defaulted =
        scene(json!({"drawables": [{"type": "draw-mesh", "position": {"x": 0, "y": 0}}]})).unwrap();
    let m = &defaulted.meshes[0].material;
    assert_eq!(
        (m.kind, m.roughness, m.metalness, m.flat_shading),
        (MaterialKind::Standard, 0.6, 0.0, false)
    );
    assert!(defaulted.meshes[0].cast_shadow && defaulted.meshes[0].receive_shadow);
}

#[test]
fn a_ground_shape_lies_flat_facing_up() {
    let s = scene(json!({"drawables": [{"type": "draw-mesh", "shape": "plane", "position": {"x": 0, "y": 0}}]})).unwrap();
    let t = s.meshes[0].transform;
    // three's plane faces +Z in its own frame; the ground rotation turns that face up (+Y)
    close([t[0], t[1], t[2]], [1.0, 0.0, 0.0]);
    close([t[8], t[9], t[10]], [0.0, 1.0, 0.0]);
    close([t[4], t[5], t[6]], [0.0, 0.0, -1.0]);
}

#[test]
fn control_unknown_or_undrawn_fields_error_by_name() {
    let unknown = message(scene(json!({
        "drawables": [{"type": "draw-mesh", "position": {"x": 0, "y": 0}, "wobble": 1}]
    })));
    assert_eq!(unknown, "drawables[0].wobble: not a declared prop");
    let gradient = message(scene(json!({
        "drawables": [{"type": "draw-shape", "shape": "rect", "position": {"x": 0, "y": 0},
            "gradient": {"kind": "linear", "stops": []}}]
    })));
    assert_eq!(
        gradient,
        "drawables[0].gradient: not yet drawn by orbital-canvas (P-025 step 4)"
    );
    let grid = scene_from_canvas_node(&json!({"type": "canvas", "mode": "3d"}), 800.0, 600.0)
        .unwrap()
        .grid
        .unwrap();
    assert_eq!(
        (grid.center, grid.size),
        ([4.5, 0.0, 4.5], [12.0, 12.0]),
        "the empty scene's 0..10 bounds"
    );
    let shadow = message(scene(json!({"drawables": [{"type": "draw-sprite",
        "position": {"x": 0, "y": 0}, "asset": {"url": "a.png", "role": "unit", "category": "c"},
        "shadow": {"color": "#000000", "blur": 2}}]})));
    assert_eq!(
        shadow,
        "drawables[0].shadow: not yet drawn by orbital-canvas (P-025 step 4)"
    );
    let ignored = message(scene(json!({
        "drawables": [{"type": "draw-mesh", "position": {"x": 0, "y": 0}, "material": {"kind": "basic", "metalness": 1}}]
    })));
    assert!(
        ignored.starts_with("drawables[0].material.metalness: "),
        "{ignored}"
    );
    assert!(scene(
        json!({"keyMap": {"KeyW": "UP"}, "tileClickEvent": "TILE", "camera": {"controls": false}})
    )
    .is_ok());
}

fn shader_mesh(material: Value) -> Value {
    json!({"drawables": [{"type": "draw-mesh", "shape": "plane", "position": {"x": 0, "y": 0}, "material": material}]})
}

const SHADE: &str = "fn shade(px: Pixel) -> vec4<f32> { return vec4<f32>(px.a.rgb, 1.0); }";

#[test]
fn a_shader_material_carries_its_wgsl_its_two_vectors_and_its_blend() {
    let mapped = scene(shader_mesh(json!({
        "kind": "shader", "wgsl": SHADE, "blend": "additive",
        "uniforms": {"a": [1, 0.5, 0, 2], "b": [0, 0, 0, 9]}
    })))
    .expect("a shader material");
    let material = &mapped.meshes[0].material;
    assert_eq!(material.kind, MaterialKind::Shader);
    assert_eq!(
        material.shader,
        Some(SurfaceShader {
            wgsl: SHADE.to_owned(),
            a: [1.0, 0.5, 0.0, 2.0],
            b: [0.0, 0.0, 0.0, 9.0],
            since: 0.0,
            blend: ShaderBlend::Additive,
        })
    );
    assert!(
        mapped.animated,
        "a shader may read the clock, so the canvas keeps drawing"
    );
}

fn shader_scene(material: Value, epoch_ms: Option<f64>) -> Result<Scene3D, CanvasSceneError> {
    let canvas = json!({"type": "canvas", "mode": "3d", "showGrid": false,
        "drawables": [{"type": "draw-mesh", "shape": "plane", "position": {"x": 0, "y": 0}, "material": material}]});
    match epoch_ms {
        Some(epoch) => scene_from_canvas_node_at_epoch(
            &canvas,
            800.0,
            600.0,
            &SceneTheme::default(),
            0.0,
            epoch,
        ),
        None => scene_from_canvas_node(&canvas, 800.0, 600.0),
    }
}

fn shader_since(material: Value, epoch_ms: Option<f64>) -> f32 {
    let s = shader_scene(material, epoch_ms).expect("a scene");
    s.meshes[0]
        .material
        .shader
        .as_ref()
        .expect("a shader")
        .since
}

#[test]
fn anchor_since_is_seconds_from_the_anchor_to_the_frame_epoch() {
    let material = json!({"kind": "shader", "wgsl": SHADE, "anchor": 1_700_000_000_000.0});
    assert_eq!(
        shader_since(material.clone(), Some(1_700_000_002_500.0)),
        2.5
    );
    assert_eq!(
        shader_since(material, Some(1_699_999_999_000.0)),
        -1.0,
        "before the anchor is negative"
    );
}

#[test]
fn anchor_since_is_zero_without_an_anchor_or_without_an_epoch() {
    let plain = json!({"kind": "shader", "wgsl": SHADE});
    assert_eq!(shader_since(plain, Some(1_700_000_002_500.0)), 0.0);
    let anchored = json!({"kind": "shader", "wgsl": SHADE, "anchor": 1_700_000_000_000.0});
    assert_eq!(shader_since(anchored, None), 0.0);
}

#[test]
fn anchor_since_refuses_a_non_finite_anchor_and_a_kind_that_does_not_draw_it() {
    let bad_anchor = shader_scene(
        json!({"kind": "shader", "wgsl": SHADE, "anchor": "soon"}),
        None,
    );
    assert!(message(bad_anchor).starts_with("drawables[0].material.anchor: "));
    let other = shader_scene(json!({"kind": "standard", "anchor": 5}), None);
    assert!(message(other).starts_with("drawables[0].material.anchor: "));
}

#[test]
fn a_shader_material_reads_the_canvas_clock_in_seconds() {
    let at = scene_at(
        shader_mesh(json!({"kind": "shader", "wgsl": SHADE})),
        2500.0,
    )
    .expect("a scene");
    assert_eq!(at.time, 2.5);
    let plain = scene_at(shader_mesh(json!({"kind": "standard"})), 2500.0).expect("a scene");
    assert_eq!(
        plain.time, 0.0,
        "control: without a shader the scene does not carry the clock"
    );
}

#[test]
fn a_shader_material_is_refused_without_wgsl_with_wgsl_that_does_not_check_or_with_a_bad_vector() {
    assert!(message(scene(shader_mesh(json!({"kind": "shader"}))))
        .starts_with("drawables[0].material.wgsl: "));
    assert!(message(scene(shader_mesh(
        json!({"kind": "shader", "wgsl": "fn shade(px: Pixel) -> vec4<f32> { return 1.0; }"})
    )))
    .starts_with("drawables[0].material.wgsl: "));
    assert!(message(scene(shader_mesh(
        json!({"kind": "shader", "wgsl": SHADE, "uniforms": {"a": [1, 2, 3]}})
    )))
    .starts_with("drawables[0].material.uniforms.a: "));
    assert!(message(scene(shader_mesh(
        json!({"kind": "shader", "wgsl": SHADE, "blend": "screen"})
    )))
    .starts_with("drawables[0].material.blend: "));
}

#[test]
fn control_another_material_kind_does_not_take_wgsl() {
    assert!(message(scene(shader_mesh(
        json!({"kind": "standard", "wgsl": SHADE})
    )))
    .starts_with("drawables[0].material.wgsl: "));
    assert!(
        !scene(shader_mesh(json!({"kind": "standard"})))
            .expect("a scene")
            .animated
    );
}

#[test]
fn a_gesture_map_is_an_input_prop_and_changes_no_scene() {
    let plain = scene(json!({"camera": {"controls": false}})).expect("a scene");
    let mapped = scene(json!({"gestureMap": {"WheelDown": "NEXT", "SwipeLeft": "NEXT"}, "camera": {"controls": false}})).expect("a scene with a gesture map");
    assert_eq!(format!("{plain:?}"), format!("{mapped:?}"));
}

#[test]
fn control_an_undeclared_prop_is_still_refused_by_name() {
    assert!(
        message(scene(json!({"gestureMaps": {"WheelDown": "NEXT"}})))
            .starts_with("canvas.gestureMaps")
    );
}

fn pulsing_box(animation: Value) -> Value {
    json!({"drawables": [{
        "type": "draw-mesh",
        "position": {"x": 0, "y": 0},
        "opacity": 0.9,
        "material": {"color": "#ffffff", "emissive": "#000000"},
        "animation": animation
    }]})
}

fn pulse() -> Value {
    json!({
        "durationMs": 1000,
        "keyframes": [
            {"at": 0, "offsetX": 0, "offsetY": 0, "offsetZ": 0, "scale": 1, "opacity": 1,
             "emissive": "#ff0000", "emissiveIntensity": 0},
            {"at": 1, "offsetX": 2, "offsetY": 4, "offsetZ": 1, "scale": 3, "opacity": 0.5,
             "emissive": "#00ff00", "emissiveIntensity": 2}
        ]
    })
}

#[test]
fn a_mesh_animation_moves_scales_fades_and_glows_at_the_canvas_clock() {
    let s = scene_at(pulsing_box(pulse()), 500.0).unwrap();
    assert!(s.animated);
    let mesh = &s.meshes[0];
    let t = mesh.transform;
    // offset (1, 2, 0.5) cells → world (1, 0.5, 2); the ×2 scale also doubles the 0.5 pivot lift
    close([t[12], t[13], t[14]], [1.0, 1.5, 2.0]);
    close([t[0], t[5], t[10]], [2.0, 2.0, 2.0]);
    assert!(
        (mesh.material.opacity - 0.75).abs() < 1e-6,
        "the track replaces node opacity"
    );
    assert!(mesh.material.is_transparent());
    close(mesh.material.emissive, [1.0, 0.0, 0.0]);
    assert!((mesh.material.emissive_intensity - 1.0).abs() < 1e-6);
    close(mesh.material.color, [1.0, 1.0, 1.0]);

    let looped = scene_at(pulsing_box(pulse()), 1500.0).unwrap();
    assert_eq!(looped.meshes, s.meshes);
}

#[test]
fn a_mesh_rotation_track_adds_to_the_authored_and_ground_rotation() {
    let spin = json!({"durationMs": 1000, "keyframes": [
        {"at": 0, "rotateX": 0, "color": "#ff0000"},
        {"at": 1, "rotateX": std::f64::consts::PI, "color": "#0000ff"}
    ]});
    let s = scene_at(
        json!({"drawables": [{
            "type": "draw-mesh", "shape": "plane", "position": {"x": 0, "y": 0}, "animation": spin
        }]}),
        500.0,
    )
    .unwrap();
    let t = s.meshes[0].transform;
    // −π/2 ground rotation + π/2 from the track: the plane stands back up, facing +Z
    close([t[8], t[9], t[10]], [0.0, 0.0, 1.0]);
    close(s.meshes[0].material.color, [1.0, 0.0, 0.0]);
}

#[test]
fn a_group_animation_carries_its_children() {
    let s = scene_at(
        json!({"drawables": [{
            "type": "draw-group",
            "position": {"x": 0, "y": 0},
            "scale": 2,
            "animation": {"durationMs": 1000, "keyframes": [
                {"at": 0, "rotateY": 0, "offsetZ": 0, "scale": 1},
                {"at": 1, "rotateY": std::f64::consts::PI, "offsetZ": 2, "scale": 2}
            ]},
            "items": [{"type": "draw-mesh", "position": {"x": 1, "y": 0}, "pivot": "center"}]
        }]}),
        500.0,
    )
    .unwrap();
    assert!(s.animated);
    let t = s.meshes[0].transform;
    // yaw π/2 turns +X to −Z; scale 2 × 1.5; offsetZ 1 cell up
    close([t[12], t[13], t[14]], [0.0, 1.0, -3.0]);
    close([t[1], t[5], t[9]], [0.0, 3.0, 0.0]);
}

#[test]
fn control_no_animation_or_no_clock_is_the_unanimated_scene() {
    let still = json!({"drawables": [{
        "type": "draw-group", "position": {"x": 1, "y": 2}, "rotate": 0.3, "scale": 1.5, "opacity": 0.5,
        "items": [{"type": "draw-mesh", "shape": "circle", "position": {"x": 1, "y": 0},
                   "rotation": [0.1, 0.2, 0.3], "opacity": 0.8,
                   "material": {"emissive": "#102030", "emissiveIntensity": 2}}]
    }]});
    let rest = scene(still.clone()).unwrap();
    assert!(!rest.animated);
    assert_eq!(scene_at(still, 1234.0).unwrap(), rest);

    let at_rest = scene(pulsing_box(pulse())).unwrap();
    assert!(at_rest.animated);
    let mut plain = pulsing_box(Value::Null);
    plain["drawables"][0]
        .as_object_mut()
        .unwrap()
        .remove("animation");
    let plain = scene(plain).unwrap();
    assert_eq!(at_rest.meshes, plain.meshes);
}

#[test]
fn control_animation_tracks_three_ignores_error_by_name() {
    let group_fade = message(scene_at(
        json!({"drawables": [{
            "type": "draw-group", "position": {"x": 0, "y": 0}, "items": [],
            "animation": {"durationMs": 100, "keyframes": [{"at": 0, "scale": 1}, {"at": 1, "opacity": 0}]}
        }]}),
        0.0,
    ));
    assert_eq!(
        group_fade,
        "drawables[0].animation.keyframes[1].opacity: a group animates only its transform (the three host ignores it)"
    );
    let basic_glow = message(scene(json!({"drawables": [{
        "type": "draw-mesh", "position": {"x": 0, "y": 0}, "material": {"kind": "basic"},
        "animation": {"durationMs": 100, "keyframes": [{"at": 0, "emissive": "#ff0000"}]}
    }]})));
    assert!(
        basic_glow.starts_with("drawables[0].animation.keyframes[0].emissive: "),
        "{basic_glow}"
    );
    let dark_intensity = message(scene(json!({"drawables": [{
        "type": "draw-mesh", "position": {"x": 0, "y": 0},
        "animation": {"durationMs": 100, "keyframes": [{"at": 0, "emissiveIntensity": 2}]}
    }]})));
    assert!(
        dark_intensity.starts_with("drawables[0].animation.keyframes[0].emissiveIntensity: "),
        "{dark_intensity}"
    );
    let bad_color = message(scene(pulsing_box(json!({
        "durationMs": 100, "keyframes": [{"at": 0, "color": "not-a-color"}]
    }))));
    assert!(
        bad_color.starts_with("drawables[0].animation.keyframes[0].color: "),
        "{bad_color}"
    );
    assert!(scene_at(pulsing_box(pulse()), -1.0).is_err());
}

fn rig_group(tip_scope: bool, bones: Value) -> Value {
    let tip = json!({"type": "draw-group", "position": {"x": 2, "y": 0}, "bone": "tip",
        "rotation": [0, 0, std::f64::consts::FRAC_PI_2], "items": []});
    let tip = if tip_scope {
        json!({"type": "draw-group", "position": {"x": 0, "y": 0}, "skeleton": true, "items": [tip]})
    } else {
        tip
    };
    json!({"drawables": [{"type": "draw-group", "position": {"x": 0, "y": 0}, "skeleton": true, "items": [
        {"type": "draw-group", "position": {"x": 0, "y": 0}, "bone": "root", "items": []},
        tip,
        {"type": "draw-mesh", "shape": "polyhedron", "position": {"x": 0, "y": 0},
            "vertices": [[0, 0, 0], [1, 0, 0], [0, 1, 0]], "faces": [[0, 1, 2]],
            "skin": {"bones": bones,
                "inverseBindMatrices": [
                    [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1],
                    [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, -2, 0, 0, 1]
                ],
                "indices": [[0], [1], [0]],
                "weights": [[1], [1], [1]]}}
    ]}]})
}

#[test]
fn a_skinned_polyhedron_follows_its_named_bone_groups() {
    let s = scene(rig_group(false, json!(["root", "tip"]))).unwrap();
    assert_eq!(s.meshes.len(), 1);
    let mesh = &s.meshes[0];
    // scene [x, y, z] -> three [x, z, y]; vertex 1 rides `tip`: (1,0,0) - 2 = (-1,0,0), turned a quarter about
    // Z is (0,-1,0), back at x = 2
    close(mesh.mesh.positions[0], [0.0, 0.0, 0.0]);
    close(mesh.mesh.positions[1], [2.0, -1.0, 0.0]);
    close(mesh.mesh.positions[2], [0.0, 0.0, 1.0]);
    assert_eq!(mesh.transform, Mat4::IDENTITY.to_cols_array());
}

#[test]
fn control_skin_bones_resolve_only_inside_their_skeleton_scope() {
    let scoped_away = message(scene(rig_group(true, json!(["root", "tip"]))));
    assert_eq!(
        scoped_away,
        "drawables[0].items[2].skin.bones[1]: no draw-group in this skeleton scope declares bone `tip` (the three host never binds the skin)"
    );
    let unknown = message(scene(rig_group(false, json!(["root", "ghost"]))));
    assert!(unknown.contains("bone `ghost`"), "{unknown}");
    let mut turned = rig_group(false, json!(["root", "tip"]));
    turned["drawables"][0]["items"][2]["rotation"] = json!([0, 1, 0]);
    assert_eq!(
        message(scene(turned)),
        "drawables[0].items[2].rotation: a skinned mesh is placed by its bones alone (the three host ignores it)"
    );
    let boxed = message(scene(
        json!({"drawables": [{"type": "draw-mesh", "position": {"x": 0, "y": 0},
        "skin": {"bones": [], "inverseBindMatrices": [], "indices": [], "weights": []}}]}),
    ));
    assert!(
        boxed.starts_with("drawables[0].skin: only a polyhedron is skinned"),
        "{boxed}"
    );
}

fn register_model(url: &str, bytes: Vec<u8>) {
    orbital_canvas_paint::asset::register_asset(url, bytes.into(), Some("model/gltf-binary"));
}

fn model_sprite(url: &str, extra: Value) -> Value {
    let mut sprite = json!({"type": "draw-sprite", "id": "knight", "position": {"x": 0, "y": 0},
        "asset": {"url": url, "role": "unit", "category": "character", "dimension": "3d"},
        "width": 2, "animation": "walk"});
    for (key, value) in extra.as_object().unwrap() {
        sprite[key] = value.clone();
    }
    json!({"drawables": [sprite]})
}

#[test]
fn a_3d_sprite_draws_its_model_posed_by_the_named_clip_on_the_canvas_clock() {
    use orbital_canvas_paint::scene3d::gltf::fixtures::{rig, rig_material};
    let url = "test://canvas-scene3d/knight.glb";
    register_model(url, rig(rig_material()));
    let rest = scene(model_sprite(url, json!({}))).unwrap();
    assert_eq!(rest.meshes.len(), 1);
    let mesh = &rest.meshes[0];
    close(mesh.mesh.positions[2], [0.0, 1.0, 0.0]);
    // the rest pose spans one unit, so the sprite's width 2 is the scale
    let t = mesh.transform;
    close([t[0], t[5], t[10]], [2.0, 2.0, 2.0]);
    assert_eq!(mesh.id.as_deref(), Some("knight"));
    assert_eq!(
        mesh.material.map.as_deref(),
        Some("test://canvas-scene3d/knight.glb#gltf-image-0")
    );
    assert!(
        rest.animated,
        "a playing clip keeps the host mapping frames"
    );

    let walking = scene_at(model_sprite(url, json!({})), 500.0).unwrap();
    close(
        walking.meshes[0].mesh.positions[2],
        [1.0 - std::f32::consts::SQRT_2, 0.0, 0.0],
    );
    let turned = scene(model_sprite(url, json!({"rotation": 90}))).unwrap();
    let t = turned.meshes[0].transform;
    // ModelLoader reads the rotation in degrees: a quarter turn about Y
    close([t[0], t[1], t[2]], [0.0, 0.0, -2.0]);
    let unknown = scene_at(model_sprite(url, json!({"animation": "run"})), 500.0).unwrap();
    close(unknown.meshes[0].mesh.positions[2], [0.0, 1.0, 0.0]);
    assert!(
        !unknown.animated,
        "control: an unknown clip leaves the model at rest"
    );
}

#[test]
fn a_broken_model_draws_the_three_error_box_and_an_undrawn_one_errors_by_name() {
    let broken = "test://canvas-scene3d/broken.glb";
    register_model(broken, b"not a glb".to_vec());
    let s = scene(model_sprite(broken, json!({}))).unwrap();
    assert_eq!(s.meshes.len(), 1);
    let t = s.meshes[0].transform;
    close([t[12], t[13], t[14]], [0.0, 0.5, 0.0]);
    let channel = |byte: u8| srgb_to_linear(byte as f32 / 255.0);
    assert_eq!(
        s.meshes[0].material.color,
        [channel(0xff), channel(0x44), channel(0x44)]
    );

    use orbital_canvas_paint::scene3d::gltf::fixtures::rig;
    let masked = "test://canvas-scene3d/masked.glb";
    register_model(masked, rig(json!({"alphaMode": "MASK"})));
    assert_eq!(
        message(scene(model_sprite(masked, json!({})))),
        "drawables[0].asset.url: test://canvas-scene3d/masked.glb: glTF materials[0].alphaMode MASK (three's alphaTest): not yet drawn by orbital-canvas"
    );
    let faded = message(scene(model_sprite(broken, json!({"opacity": 0.5}))));
    assert_eq!(
        faded,
        "drawables[0].opacity: a 3D model does not draw it (the three host ignores it)"
    );
    let dimension = message(scene(
        json!({"drawables": [{"type": "draw-sprite", "position": {"x": 0, "y": 0},
        "asset": {"url": "a.png", "role": "unit", "category": "c", "dimension": "4d"}}]}),
    ));
    assert_eq!(dimension, "drawables[0].asset.dimension: must be 2d or 3d");
}

#[test]
fn a_skinned_2d_rig_lies_on_the_scene_plane_as_the_2d_painter_poses_it() {
    let s = scene(json!({"drawables": [{
        "type": "draw-skinned-mesh",
        "position": {"x": 1, "y": 2, "z": 0.5},
        "asset": {"url": "unused", "role": "unit", "category": "c"},
        "weightsOverlay": true,
        "mesh": {"vertices": [
            {"x": 0, "y": 0, "u": 0, "v": 0, "weights": [{"bone": 0, "w": 1}]},
            {"x": 1, "y": 0, "u": 1, "v": 0, "weights": [{"bone": 0, "w": 1}]},
            {"x": 0, "y": 1, "u": 0, "v": 1, "weights": [{"bone": 0, "w": 1}]}
        ], "triangles": [[0, 1, 2]]},
        "bones": [{"name": "root", "pivot": [0, 0], "tip": [1, 0]}],
        "pose": {"root": 90}
    }]}))
    .unwrap();
    assert_eq!(s.meshes.len(), 1);
    let mesh = &s.meshes[0];
    // posed (0,0), (0,1), (-1,0) about position (1,2); the hull's minimum (0,2) anchors the bounds
    close(mesh.mesh.positions[0], [1.0, 0.5, 0.0]);
    close(mesh.mesh.positions[1], [1.0, 0.5, 1.0]);
    close(mesh.mesh.positions[2], [0.0, 0.5, 0.0]);
    assert_eq!(
        (
            mesh.material.kind,
            mesh.material.side,
            mesh.material.opacity
        ),
        (MaterialKind::Basic, Side::Double, 1.0)
    );
    let ghost = message(scene(json!({"drawables": [{
        "type": "draw-skinned-mesh", "position": {"x": 0, "y": 0},
        "ghost": {"frame": 0, "opacity": 0.5}
    }]})));
    assert!(ghost.starts_with("drawables[0].ghost"), "{ghost}");
}

fn flat(m: &Material) -> Flat {
    match m.kind {
        MaterialKind::Flat(flat) => flat,
        other => panic!("expected a flat material, got {other:?}"),
    }
}

fn origin(t: [f32; 16]) -> [f32; 3] {
    [t[12], t[13], t[14]]
}

fn png(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(width, height, image::Rgba(rgba));
    let mut out = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

fn register_png(url: &str, width: u32, height: u32) {
    orbital_canvas_paint::asset::register_asset(
        url,
        png(width, height, [255, 255, 255, 255]).into(),
        Some("image/png"),
    );
}

fn shape_scene(shape: Value) -> Result<Scene3D, CanvasSceneError> {
    let mut node = json!({"type": "draw-shape", "position": {"x": 0, "y": 0}});
    for (key, value) in shape.as_object().unwrap() {
        node[key] = value.clone();
    }
    scene(json!({ "drawables": [node] }))
}

#[test]
fn a_shape_shadow_draws_a_glow_of_its_coverage_under_it() {
    let s = shape_scene(
        json!({"shape": "rect", "width": 1, "height": 1, "fill": "#ffffff",
        "stroke": "#000000", "strokeWidth": 4, "shadow": {"color": "#ff0000", "blur": 0.2}}),
    )
    .unwrap();
    let [glow, body, edge] = s.meshes.as_slice() else {
        panic!("expected glow, body, edge: {} meshes", s.meshes.len());
    };
    let xs = body.mesh.positions.iter().map(|p| p[0]);
    let cell = xs.clone().fold(f32::MIN, f32::max) - xs.fold(f32::MAX, f32::min);
    assert!(
        (glow.glow.unwrap() - 0.2 * cell / 2.0).abs() < 1e-6,
        "sigma is half the blur, in cells"
    );
    assert_eq!(
        glow.mesh.indices.len(),
        body.mesh.indices.len() + edge.mesh.indices.len()
    );
    assert_eq!(glow.transform, body.transform);
    assert_eq!(glow.material.color, [1.0, 0.0, 0.0]);
    assert!(glow.id.is_none(), "a glow is never picked");
    assert_eq!((body.glow, edge.glow), (None, None));
}

#[test]
fn control_a_shadow_canvas_would_not_draw_adds_no_glow_and_a_bad_one_is_refused() {
    for shadow in [
        json!({"color": "#ff0000", "blur": 0}),
        json!({"color": "rgba(255,0,0,0)", "blur": 1}),
    ] {
        let s =
            shape_scene(json!({"shape": "ellipse", "fill": "#ffffff", "shadow": shadow})).unwrap();
        assert_eq!(s.meshes.len(), 1);
        assert_eq!(s.meshes[0].glow, None);
    }
    let refused = message(shape_scene(json!({"shape": "ellipse", "fill": "#ffffff",
        "shadow": {"color": "#ff0000", "blur": -1}})));
    assert!(refused.starts_with("drawables[0].shadow.blur"), "{refused}");
}

#[test]
fn each_shape_kind_lies_flat_lifted_unlit_in_its_fill() {
    for (shape, extra, vertices) in [
        ("cell", json!({}), 4),
        ("rect", json!({"width": 2, "height": 1}), 4),
        ("ellipse", json!({"radiusX": 0.5}), 34),
        (
            "poly",
            json!({"points": [{"x": 0, "y": 0}, {"x": 1, "y": 0}, {"x": 0, "y": 1}]}),
            3,
        ),
        ("path", json!({"d": "M0 0 L1 0 L1 1 Z"}), 3),
    ] {
        let mut node = json!({"shape": shape, "fill": "#ff0000", "opacity": 0.5});
        for (key, value) in extra.as_object().unwrap() {
            node[key] = value.clone();
        }
        let s = shape_scene(node).unwrap();
        assert_eq!(s.meshes.len(), 1, "{shape}");
        let mesh = &s.meshes[0];
        assert_eq!(mesh.mesh.positions.len(), vertices, "{shape}");
        assert_eq!(flat(&mesh.material), Flat::default(), "{shape}");
        assert_eq!(mesh.material.color, [1.0, 0.0, 0.0], "{shape}");
        assert_eq!(
            (mesh.material.opacity, mesh.material.side),
            (0.5, Side::Double)
        );
        assert!(!mesh.cast_shadow && !mesh.receive_shadow);
        let t = mesh.transform;
        close(origin(t), [0.0, 0.02, 0.0]);
        // three's local +Y runs to world −Z, its face up
        close([t[4], t[5], t[6]], [0.0, 0.0, -1.0]);
        close([t[8], t[9], t[10]], [0.0, 1.0, 0.0]);
    }
    let cell = shape_scene(json!({"shape": "cell"})).unwrap();
    assert_eq!(cell.meshes[0].mesh.positions[1], [0.475, 0.475, 0.0]);
    assert_eq!(
        cell.meshes[0].material.color,
        [1.0, 1.0, 1.0],
        "control: no paint is white"
    );
}

#[test]
fn a_stroke_only_ellipse_is_three_ring_and_control_a_filled_one_adds_a_stroke_band() {
    let ring =
        shape_scene(json!({"shape": "ellipse", "radiusX": 0.5, "stroke": "#00ff00"})).unwrap();
    assert_eq!(ring.meshes.len(), 1);
    let radii: Vec<f32> = ring.meshes[0]
        .mesh
        .positions
        .iter()
        .map(|p| p[0].hypot(p[1]))
        .collect();
    let (lo, hi) = radii
        .iter()
        .fold((f32::MAX, 0.0f32), |(a, b), r| (a.min(*r), b.max(*r)));
    assert!(
        (lo - 0.42).abs() < 1e-5 && (hi - 0.5).abs() < 1e-5,
        "{lo} {hi}"
    );
    assert_eq!(ring.meshes[0].material.color, [0.0, 1.0, 0.0]);

    let both = shape_scene(
        json!({"shape": "rect", "fill": "#ff0000", "stroke": "#0000ff", "strokeWidth": 25.6}),
    )
    .unwrap();
    assert_eq!(both.meshes.len(), 2);
    assert_eq!(both.meshes[1].material.color, [0.0, 0.0, 1.0]);
    let edge_x = both.meshes[1]
        .mesh
        .positions
        .iter()
        .map(|p| p[0])
        .fold(0.0f32, f32::max);
    // 25.6 painter px is a tenth of a cell, centered on the 0.5 edge
    assert!((edge_x - 0.55).abs() < 1e-5, "{edge_x}");
    assert!(both.meshes[1].transform[13] > both.meshes[0].transform[13]);
}

#[test]
fn a_shape_offset_rotation_and_animation_move_it_on_the_ground() {
    let s = shape_scene(
        json!({"shape": "cell", "offsetX": 1, "offsetY": 2, "rotate": std::f64::consts::FRAC_PI_2}),
    )
    .unwrap();
    let t = s.meshes[0].transform;
    close(origin(t), [1.0, 0.02, 2.0]);
    // the yaw is −rotate (2D turns clockwise): a quarter turn takes the shape's X to world +Z
    close([t[0], t[1], t[2]], [0.0, 0.0, 1.0]);
    let moving = json!({"shape": "cell", "animation": {"durationMs": 1000, "keyframes": [
        {"at": 0, "offsetX": 0, "fill": "#ff0000"}, {"at": 1, "offsetX": 2, "fill": "#0000ff"}]}});
    let at_rest = shape_scene(moving.clone()).unwrap();
    assert!(at_rest.animated);
    close(origin(at_rest.meshes[0].transform), [0.0, 0.02, 0.0]);
    let mut canvas = json!({"type": "canvas", "mode": "3d", "showGrid": false,
        "drawables": [{"type": "draw-shape", "position": {"x": 0, "y": 0}}]});
    for (key, value) in moving.as_object().unwrap() {
        canvas["drawables"][0][key] = value.clone();
    }
    let half =
        scene_from_canvas_node_at(&canvas, 800.0, 600.0, &SceneTheme::default(), 500.0).unwrap();
    close(origin(half.meshes[0].transform), [1.0, 0.02, 0.0]);
    assert_eq!(
        half.meshes[0].material.color,
        [1.0, 0.0, 0.0],
        "a color track holds until the next key"
    );
}

#[test]
fn control_shape_props_without_a_3d_drawing_error_by_name() {
    let cases = [
        (
            json!({"shape": "rect", "anchor": "top-left"}),
            "drawables[0].anchor: ",
        ),
        (
            json!({"shape": "rect", "stroke": "#fff", "strokeWidth": 2}),
            "drawables[0].strokeWidth: ",
        ),
        (
            json!({"shape": "cell", "radiusX": 1}),
            "drawables[0].radiusX: ",
        ),
        (
            json!({"shape": "rect", "blendMode": "multiply"}),
            "drawables[0].blendMode: ",
        ),
        (
            json!({"shape": "rect", "strokeDash": [2, 2]}),
            "drawables[0].strokeDash: ",
        ),
        (
            json!({"shape": "poly", "points": [{"x": 0, "y": 0}, {"x": 1, "y": 1}]}),
            "drawables[0].points: ",
        ),
        (
            json!({"shape": "path", "d": "M0 0 L1 1 L1 0 L0 1 Z"}),
            "drawables[0]: path: ",
        ),
        (
            json!({"shape": "cell", "animation": {"durationMs": 100, "keyframes": [{"at": 0, "blur": 2}]}}),
            "drawables[0].animation.keyframes[0].blur: ",
        ),
    ];
    for (shape, prefix) in cases {
        let m = message(shape_scene(shape));
        assert!(m.starts_with(prefix), "{m}");
    }
    let lighter = shape_scene(json!({"shape": "cell", "blendMode": "lighter"})).unwrap();
    assert!(flat(&lighter.meshes[0].material).additive);
}

#[test]
fn a_label_is_a_billboard_lifted_over_its_position_and_control_empty_text_draws_nothing() {
    let s = scene(
        json!({"drawables": [{"type": "draw-text", "id": "hp", "text": "HP 3",
        "position": {"x": 0, "y": 0}, "color": "#ffcc00", "opacity": 0.5}]}),
    )
    .unwrap();
    assert_eq!(s.meshes.len(), 1);
    let mesh = &s.meshes[0];
    assert_eq!(
        flat(&mesh.material),
        Flat {
            billboard: true,
            ..Flat::default()
        }
    );
    close(origin(mesh.transform), [0.0, 1.2, 0.0]);
    assert_eq!(mesh.material.opacity, 0.5);
    assert_eq!(mesh.id.as_deref(), Some("hp"));
    let key = mesh.material.map.clone().unwrap();
    assert!(
        orbital_canvas_paint::asset::image_info(&key).is_some(),
        "the label raster is in the asset cache"
    );
    // the quad is centered: troika's anchorX center / anchorY middle
    let xs: Vec<f32> = mesh.mesh.positions.iter().map(|p| p[0]).collect();
    assert!((xs[0] + xs[1]).abs() < 1e-5);
    let empty = scene(
        json!({"drawables": [{"type": "draw-text", "text": "", "position": {"x": 0, "y": 0}}]}),
    )
    .unwrap();
    assert!(empty.meshes.is_empty());
    let aligned = scene(
        json!({"drawables": [{"type": "draw-text", "text": "HP 3", "align": "left",
        "position": {"x": 0, "y": 0}}]}),
    )
    .unwrap();
    let left = aligned.meshes[0]
        .mesh
        .positions
        .iter()
        .map(|p| p[0])
        .fold(f32::MAX, f32::min);
    assert!(
        left < 0.0 && left > -0.05,
        "a left-aligned label starts at its position: {left}"
    );
    let font = message(scene(
        json!({"drawables": [{"type": "draw-text", "text": "x",
        "font": "12px Papyrus", "position": {"x": 0, "y": 0}}]}),
    ));
    assert!(font.starts_with("drawables[0].font: "), "{font}");
}

#[test]
fn a_sprite_is_a_nearest_cut_out_ground_decal_by_default_and_a_billboard_when_standing() {
    let url = "test://canvas-scene3d/tile.png";
    register_png(url, 32, 16);
    let asset = json!({"url": url, "role": "tile", "category": "c"});
    let decal = scene(
        json!({"drawables": [{"type": "draw-sprite", "position": {"x": 0, "y": 0},
        "asset": asset, "width": 2}]}),
    )
    .unwrap();
    let mesh = &decal.meshes[0];
    assert_eq!(
        flat(&mesh.material),
        Flat {
            nearest: true,
            cutout: true,
            ..Flat::default()
        }
    );
    assert_eq!(mesh.material.map.as_deref(), Some(url));
    close(origin(mesh.transform), [1.0, 0.01, 0.5]);
    close(
        [mesh.transform[8], mesh.transform[9], mesh.transform[10]],
        [0.0, 1.0, 0.0],
    );

    let standing = scene(
        json!({"drawables": [{"type": "draw-sprite", "position": {"x": 0, "y": 0},
        "asset": asset, "anchor": "ground", "height": 2, "flipX": true,
        "frame": {"x": 8, "y": 0, "w": 8, "h": 16}}]}),
    )
    .unwrap();
    let mesh = &standing.meshes[0];
    assert!(flat(&mesh.material).billboard);
    close(origin(mesh.transform), [0.0, 1.0, 0.0]);
    // the frame's 1:2 aspect makes it one cell wide; flipX mirrors its u range
    assert!((mesh.mesh.positions[1][0] - 0.5).abs() < 1e-5);
    assert_eq!(mesh.mesh.uvs[0], [0.5, 0.0]);
    assert_eq!(mesh.mesh.uvs[1], [0.25, 0.0]);
}

#[test]
fn a_failed_sprite_draws_three_red_placeholder_and_no_asset_draws_the_fallback_box() {
    let broken = "test://canvas-scene3d/broken.png";
    orbital_canvas_paint::asset::register_asset(
        broken,
        b"not a png".to_vec().into(),
        Some("image/png"),
    );
    let s = scene(
        json!({"drawables": [{"type": "draw-sprite", "position": {"x": 0, "y": 0},
        "asset": {"url": broken, "role": "unit", "category": "c"}, "anchor": "center"}]}),
    )
    .unwrap();
    let mesh = &s.meshes[0];
    let channel = |byte: u8| srgb_to_linear(byte as f32 / 255.0);
    assert_eq!(
        mesh.material.color,
        [channel(0xff), channel(0x44), channel(0x44)]
    );
    assert!((mesh.material.opacity - 0.6).abs() < 1e-6);
    assert!(
        !flat(&mesh.material).billboard,
        "three's placeholder is a plain plane"
    );
    assert_eq!(mesh.material.map, None);

    let unset =
        scene(json!({"drawables": [{"type": "draw-sprite", "position": {"x": 0, "y": 0}}]}))
            .unwrap();
    let mesh = &unset.meshes[0];
    assert_eq!(mesh.material.kind, MaterialKind::Standard);
    assert_eq!(
        mesh.material.color,
        [channel(0x9b), channel(0x8f), channel(0x7f)]
    );
    assert_eq!(mesh.mesh.positions.len(), 24, "a box");
}

#[test]
fn layers_draw_each_item_as_its_atom_picked_by_its_id() {
    let s = scene(json!({"drawables": [
        {"type": "draw-shape-layer", "items": [
            {"type": "draw-shape", "id": "a", "shape": "cell", "position": {"x": 0, "y": 0}},
            {"type": "draw-shape", "id": "b", "shape": "cell", "position": {"x": 4, "y": 2}}
        ]},
        {"type": "draw-text-layer", "items": [
            {"type": "draw-text", "text": "hi", "position": {"x": 1, "y": 1}}
        ]}
    ]}))
    .unwrap();
    assert_eq!(s.meshes.len(), 3);
    assert_eq!(s.meshes[0].id.as_deref(), Some("a"));
    assert_eq!(s.meshes[1].id.as_deref(), Some("b"));
    close(origin(s.meshes[1].transform), [4.0, 0.02, 2.0]);
    // the layer items frame the camera as collectDrawnItems counts them
    close(s.camera.target, [2.0, 0.0, 1.0]);
    assert_eq!(s.interaction.cells.len(), 2);
    let wrong = message(scene(
        json!({"drawables": [{"type": "draw-shape-layer", "items": [
        {"type": "draw-text", "text": "x", "position": {"x": 0, "y": 0}}]}]}),
    ));
    assert_eq!(wrong, "drawables[0].items[0].type: must be `draw-shape`");
}

#[test]
fn an_fx_layer_expands_on_the_page_clock_and_keeps_the_host_mapping() {
    let fx = json!({"type": "draw-fx-layer", "items": [{"id": "hit-1", "type": "hit",
        "position": {"x": 2, "y": 3}, "ttl": 2, "maxTtl": 4, "bornAt": 1000}],
        "presets": [{"type": "hit", "shape": "puff", "size": 0.5, "count": 3, "color": "#00ff00",
            "vx": 0, "vy": 0, "vz": 0, "gravity": 0}]});
    let canvas = json!({"type": "canvas", "mode": "3d", "showGrid": false, "drawables": [fx]});
    let s =
        scene_from_canvas_node_at_epoch(&canvas, 800.0, 600.0, &SceneTheme::default(), 0.0, 1500.0)
            .unwrap();
    assert!(s.animated);
    assert_eq!(s.meshes.len(), 3, "three puffs");
    for mesh in &s.meshes {
        assert_eq!(mesh.material.color, [0.0, 1.0, 0.0]);
        // fade 0.75 × the puff's 0.6
        assert!((mesh.material.opacity - 0.45).abs() < 1e-6);
    }
    let empty = json!({"type": "canvas", "mode": "3d", "showGrid": false,
        "drawables": [{"type": "draw-fx-layer", "items": []}]});
    let rest = scene_from_canvas_node(&empty, 800.0, 600.0).unwrap();
    assert!(
        !rest.animated && rest.meshes.is_empty(),
        "control: no live entry, nothing to redraw"
    );
}

#[test]
fn a_mesh_outline_is_a_scaled_back_face_hull_and_on_a_polyhedron_a_pushed_out_shell() {
    let s = scene(
        json!({"drawables": [{"type": "draw-mesh", "position": {"x": 0, "y": 0},
        "opacity": 0.5, "outline": {"color": "#00ff00", "width": 0.1}}]}),
    )
    .unwrap();
    assert_eq!(s.meshes.len(), 2);
    let hull = &s.meshes[1];
    assert_eq!(
        (
            hull.material.kind,
            hull.material.side,
            hull.material.color,
            hull.material.opacity
        ),
        (MaterialKind::Basic, Side::Back, [0.0, 1.0, 0.0], 0.5)
    );
    assert_eq!(hull.mesh, s.meshes[0].mesh);
    let t = hull.transform;
    close([t[0], t[5], t[10]], [1.1, 1.1, 1.1]);
    close(origin(t), [0.0, 0.5, 0.0]);
    assert!(!hull.cast_shadow);

    let poly = scene(json!({"drawables": [{"type": "draw-mesh", "shape": "polyhedron",
        "position": {"x": 0, "y": 0}, "vertices": [[0, 0, 0], [1, 0, 0], [0, 1, 0]], "faces": [[0, 1, 2]],
        "outline": {"width": 0.25}}]}))
    .unwrap();
    let (body, shell) = (&poly.meshes[0].mesh, &poly.meshes[1].mesh);
    for (p, q) in body.positions.iter().zip(&shell.positions) {
        close(*q, [p[0], p[1] + 0.25, p[2]]);
    }

    let theme = SceneTheme {
        outline: 2.0,
        ..SceneTheme::default()
    };
    let canvas = json!({"type": "canvas", "mode": "3d", "showGrid": false,
        "drawables": [{"type": "draw-mesh", "position": {"x": 0, "y": 0}}]});
    let themed = scene_from_canvas_node_themed(&canvas, 800.0, 600.0, &theme).unwrap();
    assert_eq!(themed.meshes.len(), 2, "the theme's outline");
    close([themed.meshes[1].transform[0], 0.0, 0.0], [1.04, 0.0, 0.0]);
    assert_eq!(
        scene_from_canvas_node(&canvas, 800.0, 600.0)
            .unwrap()
            .meshes
            .len(),
        1,
        "control: no outline drawn when neither the mesh nor the theme asks"
    );
}

#[test]
fn a_skinned_polyhedron_outline_follows_the_posed_surface() {
    let mut rig = rig_group(false, json!(["root", "tip"]));
    rig["drawables"][0]["items"][2]["outline"] = json!({"width": 0.1});
    let s = scene(rig).unwrap();
    assert_eq!(s.meshes.len(), 2);
    let (body, shell) = (&s.meshes[0].mesh, &s.meshes[1].mesh);
    for (i, (p, q)) in body.positions.iter().zip(&shell.positions).enumerate() {
        let n = body.normals[i];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        close(
            *q,
            [
                p[0] + n[0] / len * 0.1,
                p[1] + n[1] / len * 0.1,
                p[2] + n[2] / len * 0.1,
            ],
        );
    }
}
