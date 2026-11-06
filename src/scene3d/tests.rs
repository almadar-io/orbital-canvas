//! Pixel checks of the 3D renderer on this machine's adapter; every test returns early when there is none.

use super::geometry;
use super::model::*;
use super::render::Renderer3D;
use glam::Mat4;

const SIZE: u32 = 64;

fn renderer() -> Option<Renderer3D> {
    let shared = crate::device::shared_device().ok()?;
    Some(Renderer3D::new(shared.device.clone(), shared.queue.clone()))
}

fn camera(eye: [f32; 3], target: [f32; 3]) -> Camera {
    Camera {
        eye,
        target,
        up: [0.0, 1.0, 0.0],
        projection: Projection::Perspective {
            fov_y_degrees: 45.0,
        },
        near: 0.1,
        far: 100.0,
    }
}

fn scene(meshes: Vec<MeshInstance>, lights: Lights, shadows: bool) -> Scene3D {
    Scene3D {
        camera: camera([0.0, 6.0, 8.0], [0.0, 0.0, 0.0]),
        background: [0.0, 0.0, 0.0, 1.0],
        lights,
        shadows,
        tone_mapping: ToneMapping::None,
        meshes,
    }
}

fn instance(mesh: MeshData, transform: Mat4, material: Material) -> MeshInstance {
    MeshInstance {
        mesh,
        transform: transform.to_cols_array(),
        material,
        cast_shadow: true,
        receive_shadow: true,
    }
}

fn sun(shadow: bool) -> Lights {
    Lights {
        ambient: None,
        hemisphere: None,
        directional: Some(DirectionalLight {
            position: [0.0, 10.0, 0.0],
            target: [0.0, 0.0, 0.0],
            color: [1.0, 1.0, 1.0],
            intensity: std::f32::consts::PI,
            shadow: shadow.then_some(ShadowSettings {
                map_size: 1024,
                bias: -0.0005,
                normal_bias: 0.02,
                extent: 6.0,
                near: 0.5,
                far: 50.0,
            }),
        }),
        points: Vec::new(),
    }
}

/// Renders `scene` into an RGBA8 sRGB texture and reads it back.
fn draw(renderer: &mut Renderer3D, scene: &Scene3D, samples: u32) -> Vec<u8> {
    let shared = crate::device::shared_device().expect("device");
    let texture = shared.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("scene3d test target"),
        size: wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let mut encoder = shared.device.create_command_encoder(&Default::default());
    renderer
        .encode(&mut encoder, scene, &texture, samples)
        .expect("render");
    crate::renderer::copy_texture_to_rgba(
        &shared.device,
        &shared.queue,
        encoder,
        &texture,
        SIZE,
        SIZE,
    )
    .expect("readback")
}

/// Writes the frame to `$SCENE3D_DUMP/<name>.png` when that variable is set, for looking at a failure.
fn dump(name: &str, rgba: &[u8]) {
    if let Ok(dir) = std::env::var("SCENE3D_DUMP") {
        let image = image::RgbaImage::from_raw(SIZE, SIZE, rgba.to_vec()).expect("frame");
        image.save(format!("{dir}/{name}.png")).expect("png");
    }
}

fn pixel(rgba: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIZE + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

fn luma(p: [u8; 4]) -> u32 {
    p[0] as u32 + p[1] as u32 + p[2] as u32
}

fn white() -> Material {
    Material {
        kind: MaterialKind::Standard,
        color: [0.8, 0.8, 0.8],
        ..Material::default()
    }
}

#[test]
fn a_lit_sphere_is_brighter_on_the_side_facing_its_light() {
    let Some(mut r) = renderer() else { return };
    let mut s = scene(
        vec![instance(
            geometry::sphere(1.5, 32).unwrap(),
            Mat4::IDENTITY,
            white(),
        )],
        sun(false),
        false,
    );
    s.camera = camera([0.0, 0.0, 8.0], [0.0, 0.0, 0.0]);
    let rgba = draw(&mut r, &s, 1);
    let (top, bottom) = (
        pixel(&rgba, SIZE / 2, SIZE / 2 - 10),
        pixel(&rgba, SIZE / 2, SIZE / 2 + 10),
    );
    assert!(
        luma(top) > luma(bottom) + 60,
        "top {top:?} bottom {bottom:?}"
    );
    assert_eq!(
        pixel(&rgba, 1, 1),
        [0, 0, 0, 255],
        "control: the background stays clear"
    );
}

#[test]
fn a_box_casts_a_shadow_on_the_floor_and_control_without_shadows_it_does_not() {
    let Some(mut r) = renderer() else { return };
    let floor = instance(
        geometry::plane(10.0, 10.0).unwrap(),
        Mat4::IDENTITY,
        white(),
    );
    let block = instance(
        geometry::cuboid(2.0, 2.0, 2.0).unwrap(),
        Mat4::from_translation([0.0, 3.0, 0.0].into()),
        white(),
    );
    let mut shadowed = scene(vec![floor.clone(), block.clone()], sun(true), true);
    shadowed.camera = camera([0.0, 2.0, 9.0], [0.0, 0.0, 2.0]);
    let mut lit = shadowed.clone();
    lit.shadows = false;
    let (with, without) = (draw(&mut r, &shadowed, 1), draw(&mut r, &lit, 1));
    dump("shadowed", &with);
    let screen = |world: [f32; 3]| {
        let ndc = super::view_projection(&shadowed, 1.0).project_point3(world.into());
        (
            ((ndc.x * 0.5 + 0.5) * SIZE as f32) as u32,
            ((0.5 - ndc.y * 0.5) * SIZE as f32) as u32,
        )
    };
    let (under, beside) = (screen([0.0, 0.0, 0.0]), screen([3.5, 0.0, 0.0]));
    let at = |rgba: &[u8], (x, y): (u32, u32)| pixel(rgba, x, y);
    assert!(
        luma(at(&with, under)) + 60 < luma(at(&with, beside)),
        "shadowed {:?} vs lit floor {:?}",
        at(&with, under),
        at(&with, beside)
    );
    let (a, b) = (
        luma(at(&without, under)) as i32,
        luma(at(&without, beside)) as i32,
    );
    assert!(
        (a - b).abs() <= 6,
        "control: without shadows the floor is evenly lit ({a} vs {b})"
    );
}

#[test]
fn a_basic_material_ignores_light_and_paints_its_color() {
    let Some(mut r) = renderer() else { return };
    let basic = Material {
        kind: MaterialKind::Basic,
        color: [srgb_to_linear(0.5), 0.0, 0.0],
        ..Material::default()
    };
    let mut s = scene(
        vec![instance(
            geometry::plane(20.0, 20.0).unwrap(),
            Mat4::IDENTITY,
            basic,
        )],
        Lights::default(),
        false,
    );
    s.camera = camera([0.0, 5.0, 0.01], [0.0, 0.0, 0.0]);
    let center = pixel(&draw(&mut r, &s, 1), SIZE / 2, SIZE / 2);
    assert!(
        (center[0] as i32 - 128).abs() <= 2 && center[1] == 0 && center[2] == 0,
        "{center:?}"
    );
}

#[test]
fn a_half_transparent_plane_blends_over_what_is_behind_it() {
    let Some(mut r) = renderer() else { return };
    let basic = |rgb: [f32; 3], opacity: f32| Material {
        kind: MaterialKind::Basic,
        color: rgb,
        opacity,
        ..Material::default()
    };
    let back = instance(
        geometry::plane(20.0, 20.0).unwrap(),
        Mat4::IDENTITY,
        basic([0.0, 0.0, 1.0], 1.0),
    );
    let front = instance(
        geometry::plane(20.0, 20.0).unwrap(),
        Mat4::from_translation([0.0, 1.0, 0.0].into()),
        basic([1.0, 0.0, 0.0], 0.5),
    );
    let mut s = scene(vec![front, back], Lights::default(), false);
    s.camera = camera([0.0, 5.0, 0.01], [0.0, 0.0, 0.0]);
    let center = pixel(&draw(&mut r, &s, 1), SIZE / 2, SIZE / 2);
    assert!(
        center[0] > 100 && center[2] > 100,
        "both red and blue show: {center:?}"
    );
}

#[test]
fn multisampling_softens_an_edge_that_a_single_sample_leaves_hard() {
    let Some(mut r) = renderer() else { return };
    let basic = Material {
        kind: MaterialKind::Basic,
        color: [1.0, 1.0, 1.0],
        ..Material::default()
    };
    let tilted = Mat4::from_rotation_y(0.3);
    let mut s = scene(
        vec![instance(geometry::plane(3.0, 3.0).unwrap(), tilted, basic)],
        Lights::default(),
        false,
    );
    s.camera = camera([0.0, 6.0, 0.01], [0.0, 0.0, 0.0]);
    let partial = |rgba: &[u8]| rgba.chunks(4).filter(|p| p[0] > 10 && p[0] < 245).count();
    let (hard, soft) = (draw(&mut r, &s, 1), draw(&mut r, &s, 4));
    assert_eq!(
        partial(&hard),
        0,
        "control: one sample has no partial coverage"
    );
    assert!(partial(&soft) > 0, "four samples blend the edge");
}

#[test]
fn a_mesh_drawn_twice_uploads_once() {
    let Some(mut r) = renderer() else { return };
    let mesh = geometry::sphere(1.0, 12).unwrap();
    let s = scene(
        vec![
            instance(mesh.clone(), Mat4::IDENTITY, white()),
            instance(
                mesh,
                Mat4::from_translation([2.0, 0.0, 0.0].into()),
                white(),
            ),
        ],
        sun(false),
        false,
    );
    draw(&mut r, &s, 1);
    assert_eq!(r.cached_meshes(), 1);
}
