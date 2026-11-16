use super::fixtures::*;
use super::*;
use serde_json::json;

fn close(a: [f32; 3], b: [f32; 3]) {
    for i in 0..3 {
        assert!((a[i] - b[i]).abs() < 1e-4, "{a:?} != {b:?}");
    }
}

#[test]
fn a_glb_loads_its_mesh_material_skin_clip_and_embedded_image() {
    let model =
        Model::from_glb(&rig(rig_material()), "https://cdn.test/3d/units/knight.glb").unwrap();
    assert_eq!(model.clips.len(), 1);
    assert_eq!(
        (model.clips[0].name.as_str(), model.clips[0].duration),
        ("Walk", 1.0)
    );
    assert_eq!(model.clip_index("Walk"), Some(0));
    assert_eq!(
        model.clip_index("walk"),
        Some(0),
        "case-insensitive fallback"
    );
    assert_eq!(
        model.clip_index("run"),
        None,
        "control: an unknown clip plays nothing"
    );
    assert_eq!(model.images.len(), 1);
    assert_eq!(
        model.images[0].key,
        "https://cdn.test/3d/units/knight.glb#gltf-image-0"
    );
    assert_eq!(model.images[0].mime, "image/png");

    let rest = model.pose(None, 0.0).unwrap();
    assert_eq!(rest.len(), 1);
    let m = &rest[0].material;
    assert_eq!(m.color, [1.0, 0.5, 0.25]);
    assert_eq!((m.metalness, m.roughness, m.opacity), (0.2, 0.7, 1.0));
    assert_eq!(
        m.map.as_deref(),
        Some("https://cdn.test/3d/units/knight.glb#gltf-image-0")
    );
    assert!(!m.flat_shading);
    close(rest[0].mesh.positions[2], [0.0, 1.0, 0.0]);
    assert!(rest[0].cast_shadow && rest[0].receive_shadow);
    let (lo, hi) = model.rest_bounds().unwrap();
    close(lo.to_array(), [0.0, 0.0, 0.0]);
    close(hi.to_array(), [1.0, 1.0, 0.0]);
}

#[test]
fn posing_plays_the_clip_through_the_skin() {
    let model = Model::from_glb(&rig(rig_material()), "m.glb").unwrap();
    let posed = model.pose(Some(0), 0.5).unwrap();
    let p = &posed[0].mesh.positions;
    close(p[0], [0.0, 0.0, 0.0]);
    close(p[1], [1.0, 0.0, 0.0]);
    // (0,1,0) relative to joint 1 is (-1,1,0); turned 45 degrees it is (-sqrt2, 0, 0); back at x = 1
    close(p[2], [1.0 - std::f32::consts::SQRT_2, 0.0, 0.0]);
    let looped = model.pose(Some(0), 1.5).unwrap();
    close(looped[0].mesh.positions[2], p[2]);
    let control = model.pose(None, 0.5).unwrap();
    close(control[0].mesh.positions[2], [0.0, 1.0, 0.0]);
}

#[test]
fn the_clip_fades_in_from_the_rest_pose() {
    let model = Model::from_glb(&rig(rig_material()), "m.glb").unwrap();
    let start = model.pose(Some(0), 0.0).unwrap();
    close(start[0].mesh.positions[2], [0.0, 1.0, 0.0]);
    // at 0.05 s the clip is 4.5 degrees in and weighs a third: 1.5 degrees about joint 1
    let early = model.pose(Some(0), 0.05).unwrap();
    let a = 1.5f32.to_radians();
    let (s, c) = a.sin_cos();
    close(early[0].mesh.positions[2], [1.0 - c - s, -s + c, 0.0]);
}

#[test]
fn urls_resolve_like_three() {
    assert_eq!(asset_root("https://h/3d/units/a.glb"), "https://h/3d/");
    assert_eq!(asset_root("https://h/models/a.glb"), "https://h/models/");
    assert_eq!(asset_root("a.glb"), "");
    assert_eq!(
        resolve_url("Textures/c.png", "https://h/3d/"),
        "https://h/3d/Textures/c.png"
    );
    assert_eq!(resolve_url("/c.png", "https://h/3d/"), "https://h/c.png");
    assert_eq!(resolve_url("//x/c.png", "https://h/3d/"), "//x/c.png");
    assert_eq!(
        resolve_url("data:image/png;base64,AA", "p/"),
        "data:image/png;base64,AA"
    );
    assert_eq!(resolve_url("", "p/"), "");
}

fn channel(
    property: Property,
    interpolation: Interpolation,
    times: &[f32],
    values: &[f32],
) -> Channel {
    Channel {
        node: 0,
        property,
        interpolation,
        times: times.to_vec(),
        values: values.to_vec(),
    }
}

#[test]
fn interpolation_matches_the_mixer() {
    let linear = channel(
        Property::Translation,
        Interpolation::Linear,
        &[0.0, 2.0],
        &[0.0, 0.0, 0.0, 4.0, 2.0, 0.0],
    );
    assert_eq!(linear.sample(0.5), vec![1.0, 0.5, 0.0]);
    assert_eq!(
        linear.sample(-1.0),
        vec![0.0, 0.0, 0.0],
        "before the first key"
    );
    assert_eq!(
        linear.sample(9.0),
        vec![4.0, 2.0, 0.0],
        "after the last key"
    );
    let step = channel(
        Property::Scale,
        Interpolation::Step,
        &[0.0, 1.0],
        &[1.0, 1.0, 1.0, 2.0, 2.0, 2.0],
    );
    assert_eq!(step.sample(0.99), vec![1.0, 1.0, 1.0]);
    assert_eq!(step.sample(1.0), vec![2.0, 2.0, 2.0]);
    // keys [in, value, out]: value 0 -> 1, out-tangent 2 at the first key, in-tangent 0 at the second
    let cubic = channel(
        Property::Translation,
        Interpolation::CubicSpline,
        &[0.0, 1.0],
        &[
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0,
            0.0,
        ],
    );
    let v = cubic.sample(0.5);
    assert!((v[0] - 0.75).abs() < 1e-6, "{v:?}");
    assert_eq!(cubic.sample(1.0), vec![1.0, 0.0, 0.0]);
    let s = std::f32::consts::FRAC_1_SQRT_2;
    let rot = channel(
        Property::Rotation,
        Interpolation::Linear,
        &[0.0, 1.0],
        &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, s, s],
    );
    let q = rot.sample(0.5);
    let half = std::f32::consts::FRAC_PI_8;
    assert!(
        (q[2] - half.sin()).abs() < 1e-6 && (q[3] - half.cos()).abs() < 1e-6,
        "{q:?}"
    );
}

#[test]
fn clip_time_loops_like_loop_repeat() {
    let clip = Clip {
        name: "a".into(),
        duration: 1.0,
        channels: vec![],
    };
    assert_eq!(clip.time(0.25), 0.25);
    assert_eq!(clip.time(1.25), 0.25);
    assert_eq!(clip.time(1.0), 0.0);
}

#[test]
fn control_undrawn_material_features_error_by_name_and_garbage_is_invalid() {
    let masked = Model::from_glb(&rig(json!({"alphaMode": "MASK"})), "m.glb").unwrap_err();
    assert_eq!(
        masked,
        GltfError::Unsupported {
            field: "glTF materials[0].alphaMode MASK (three's alphaTest)".into(),
            why: "not yet drawn by orbital-canvas".into()
        }
    );
    let plain = Model::from_glb(&rig(json!({})), "m.glb").unwrap();
    let m = &plain.pose(None, 0.0).unwrap()[0].material;
    assert_eq!(
        (m.color, m.metalness, m.roughness, m.map.clone()),
        ([1.0; 3], 1.0, 1.0, None)
    );
    assert!(matches!(
        Model::from_glb(b"not a glb", "m.glb"),
        Err(GltfError::Invalid(_))
    ));
}
