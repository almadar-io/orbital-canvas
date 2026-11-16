use super::Renderer3D;
use glam::Mat4;
use orbital_canvas_base::scene3d::geometry;
use orbital_canvas_base::scene3d::model::*;

const SIZE: u32 = 64;

fn renderer() -> Option<Renderer3D> {
    let shared = crate::device::shared_device().ok()?;
    Some(Renderer3D::new(shared.device.clone(), shared.queue.clone()))
}

fn scene(meshes: Vec<MeshInstance>) -> Scene3D {
    Scene3D {
        camera: Camera {
            eye: [0.0, 0.0, 8.0],
            target: [0.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            projection: Projection::Perspective {
                fov_y_degrees: 45.0,
            },
            near: 0.1,
            far: 100.0,
        },
        background: [0.0, 0.0, 0.0, 1.0],
        lights: Lights::default(),
        shadows: false,
        tone_mapping: ToneMapping::None,
        meshes,
        grid: None,
        interaction: Interaction::default(),
        animated: false,
        environment: None,
        post: None,
        time: 0.0,
    }
}

fn instance(mesh: MeshData, transform: Mat4, material: Material) -> MeshInstance {
    MeshInstance {
        mesh,
        transform: transform.to_cols_array(),
        material,
        cast_shadow: false,
        receive_shadow: false,
        id: None,
        glow: None,
    }
}

fn draw(renderer: &mut Renderer3D, scene: &Scene3D) -> Vec<u8> {
    let shared = crate::device::shared_device().expect("device");
    let texture = shared.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("scene3d integration target"),
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
    shared
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);
    let mut encoder = shared.device.create_command_encoder(&Default::default());
    renderer
        .encode(&mut encoder, scene, &texture, 4)
        .expect("render");
    let rgba = crate::texture::copy_texture_to_rgba(
        &shared.device,
        &shared.queue,
        encoder,
        &texture,
        SIZE,
        SIZE,
    )
    .expect("readback");
    let errors = pollster::block_on(shared.device.pop_error_scope());
    assert!(errors.is_none(), "{errors:?}");
    rgba
}

fn pixel(rgba: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIZE + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

fn luma(p: [u8; 4]) -> u32 {
    p[0] as u32 + p[1] as u32 + p[2] as u32
}

#[test]
fn the_room_environment_lights_a_metal_sphere_that_no_light_reaches() {
    let Some(mut r) = renderer() else { return };
    let sphere = instance(
        geometry::sphere(2.0, 32, 16).unwrap(),
        Mat4::IDENTITY,
        Material {
            kind: MaterialKind::Standard,
            metalness: 1.0,
            roughness: 0.2,
            ..Material::default()
        },
    );
    let dark = scene(vec![sphere]);
    let lit = Scene3D {
        environment: Some(EnvironmentKind::Room),
        ..dark.clone()
    };
    let (with, without) = (draw(&mut r, &lit), draw(&mut r, &dark));
    let center = (SIZE / 2, SIZE / 2);
    assert!(
        luma(pixel(&with, center.0, center.1)) > 60,
        "the room reflects off the metal: {:?}",
        pixel(&with, center.0, center.1)
    );
    assert!(
        luma(pixel(&without, center.0, center.1)) < 6,
        "control: without the environment no light reaches it: {:?}",
        pixel(&without, center.0, center.1)
    );
    assert_eq!(
        pixel(&with, 1, 1),
        [0, 0, 0, 255],
        "the environment lights materials, not the background"
    );
}

#[test]
fn a_basic_material_ignores_the_environment() {
    let Some(mut r) = renderer() else { return };
    let gray = Material {
        kind: MaterialKind::Basic,
        color: [0.2, 0.2, 0.2],
        ..Material::default()
    };
    let plain = scene(vec![instance(
        geometry::plane(4.0, 4.0).unwrap(),
        Mat4::IDENTITY,
        gray,
    )]);
    let lit = Scene3D {
        environment: Some(EnvironmentKind::Room),
        ..plain.clone()
    };
    assert_eq!(
        pixel(&draw(&mut r, &lit), SIZE / 2, SIZE / 2),
        pixel(&draw(&mut r, &plain), SIZE / 2, SIZE / 2)
    );
}

#[test]
fn a_glow_spreads_a_shape_coverage_past_its_edge_in_its_color_under_the_shape() {
    let Some(mut r) = renderer() else { return };
    let flat = |rgb: [f32; 3]| Material {
        kind: MaterialKind::Flat(Flat::default()),
        color: rgb,
        side: Side::Double,
        ..Material::default()
    };
    let square = instance(
        geometry::plane(2.0, 2.0).unwrap(),
        Mat4::IDENTITY,
        flat([1.0, 1.0, 1.0]),
    );
    let mut glow = instance(
        geometry::plane(2.0, 2.0).unwrap(),
        Mat4::IDENTITY,
        flat([0.0, 1.0, 0.0]),
    );
    glow.glow = Some(0.3);
    let with = draw(&mut r, &scene(vec![glow, square.clone()]));
    let without = draw(&mut r, &scene(vec![square]));
    // the square spans about 19 pixels around the center; this one is a few pixels outside it
    let outside = (SIZE / 2 + 13, SIZE / 2);
    let p = pixel(&with, outside.0, outside.1);
    assert!(
        p[1] > 20 && p[0] < 8 && p[2] < 8,
        "the glow reaches past the edge in its color: {p:?}"
    );
    assert_eq!(
        pixel(&without, outside.0, outside.1),
        [0, 0, 0, 255],
        "control: without the glow nothing is drawn there"
    );
    let far = (2, 2);
    assert_eq!(
        pixel(&with, far.0, far.1),
        [0, 0, 0, 255],
        "three sigmas out the glow is gone"
    );
    let center = (SIZE / 2, SIZE / 2);
    assert_eq!(
        pixel(&with, center.0, center.1),
        pixel(&without, center.0, center.1),
        "the shape draws over its glow"
    );
}

fn glowing_square() -> Scene3D {
    scene(vec![instance(
        geometry::plane(2.0, 2.0).unwrap(),
        Mat4::IDENTITY,
        Material {
            kind: MaterialKind::Standard,
            color: [0.0, 0.0, 0.0],
            emissive: [1.0, 1.0, 1.0],
            emissive_intensity: 4.0,
            ..Material::default()
        },
    )])
}

#[test]
fn bloom_spreads_a_bright_emissive_past_its_edge() {
    let Some(mut r) = renderer() else { return };
    let plain = glowing_square();
    let bloomed = Scene3D {
        post: Some(PostSettings {
            bloom: Some(BloomSettings {
                intensity: 1.0,
                threshold: 0.9,
                smoothing: 0.3,
            }),
            vignette: None,
        }),
        ..plain.clone()
    };
    let (with, without) = (draw(&mut r, &bloomed), draw(&mut r, &plain));
    // the square spans about 19 pixels around the center; this one is a few pixels outside it
    let outside = (SIZE / 2 + 14, SIZE / 2);
    assert!(
        luma(pixel(&with, outside.0, outside.1)) > 30,
        "bloom glows past the edge: {:?}",
        pixel(&with, outside.0, outside.1)
    );
    assert_eq!(
        pixel(&without, outside.0, outside.1),
        [0, 0, 0, 255],
        "control: without post the background stays black"
    );
    assert!(
        pixel(&with, SIZE / 2, SIZE / 2)[0] == 255,
        "the square itself stays saturated"
    );
}

#[test]
fn a_vignette_alone_darkens_the_corners_and_skips_tone_mapping() {
    let Some(mut r) = renderer() else { return };
    let gray = Material {
        kind: MaterialKind::Basic,
        color: [0.5, 0.5, 0.5],
        ..Material::default()
    };
    let plain = Scene3D {
        tone_mapping: ToneMapping::Aces,
        ..scene(vec![instance(
            geometry::plane(20.0, 20.0).unwrap(),
            Mat4::IDENTITY,
            gray,
        )])
    };
    let vignetted = Scene3D {
        post: Some(PostSettings {
            bloom: None,
            vignette: Some(VignetteSettings {
                offset: 0.3,
                darkness: 0.6,
            }),
        }),
        ..plain.clone()
    };
    let (with, without) = (draw(&mut r, &vignetted), draw(&mut r, &plain));
    let corner = luma(pixel(&with, 1, 1));
    let center = luma(pixel(&with, SIZE / 2, SIZE / 2));
    assert!(corner + 30 < center, "corner {corner} center {center}");
    // linear 0.5 encodes to 188 untoned; three's ACES fit lifts it to about 197
    let untoned = pixel(&with, SIZE / 2, SIZE / 2)[0];
    assert!((185..=191).contains(&untoned), "{untoned}");
    let toned = pixel(&without, SIZE / 2, SIZE / 2)[0];
    assert!(
        toned >= 193,
        "control: without a composer ACES applies: {toned}"
    );
    let empty = Scene3D {
        post: Some(PostSettings::default()),
        ..plain.clone()
    };
    assert_eq!(
        draw(&mut r, &empty),
        without,
        "control: post with no effect mounts no composer"
    );
}

fn pane(transmission: f32, side: Side) -> Scene3D {
    let behind = instance(
        geometry::plane(20.0, 20.0).unwrap(),
        Mat4::from_translation(glam::Vec3::new(0.0, 0.0, -2.0)),
        Material {
            kind: MaterialKind::Basic,
            ..Material::default()
        },
    );
    let glass = instance(
        geometry::plane(3.0, 3.0).unwrap(),
        Mat4::IDENTITY,
        Material {
            kind: MaterialKind::Physical,
            color: [0.1, 1.0, 0.1],
            roughness: 0.0,
            transmission,
            side,
            ..Material::default()
        },
    );
    scene(vec![glass, behind])
}

#[test]
fn a_transmissive_pane_shows_what_is_behind_it_tinted() {
    let Some(mut r) = renderer() else { return };
    let (glass, opaque) = (
        draw(&mut r, &pane(1.0, Side::Front)),
        draw(&mut r, &pane(0.0, Side::Front)),
    );
    let [red, green, _, _] = pixel(&glass, SIZE / 2, SIZE / 2);
    assert!(
        green > 200 && red + 80 < green,
        "the white wall shows through, tinted green: {:?}",
        pixel(&glass, SIZE / 2, SIZE / 2)
    );
    assert!(
        luma(pixel(&opaque, SIZE / 2, SIZE / 2)) < 10,
        "control: an opaque unlit pane hides the wall: {:?}",
        pixel(&opaque, SIZE / 2, SIZE / 2)
    );
    assert_eq!(
        pixel(&glass, 2, 2),
        [255, 255, 255, 255],
        "the wall around the pane is untouched"
    );
    let double = draw(&mut r, &pane(1.0, Side::Double));
    let [_, green, _, _] = pixel(&double, SIZE / 2, SIZE / 2);
    assert!(
        green > 100,
        "a double-sided pane still transmits: {:?}",
        pixel(&double, SIZE / 2, SIZE / 2)
    );
}

fn register_png(url: &str, width: u32, height: u32) {
    let image = image::RgbaImage::from_fn(width, height, |x, y| {
        if (x + y) % 2 == 0 {
            image::Rgba([255, 255, 255, 255])
        } else {
            image::Rgba([0, 0, 0, 255])
        }
    });
    let mut bytes = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .expect("png");
    orbital_canvas_paint::asset::register_asset(url, bytes.into(), Some("image/png"));
}

#[test]
fn linear_maps_get_a_full_mip_chain_and_nearest_ones_keep_one_level() {
    let Some(mut r) = renderer() else { return };
    let shared = crate::device::shared_device().expect("device");
    let (checker, dot) = (
        "test://scene3d/mips-checker.png",
        "test://scene3d/mips-dot.png",
    );
    register_png(checker, 8, 4);
    register_png(dot, 1, 1);
    shared
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);
    let mut encoder = shared.device.create_command_encoder(&Default::default());
    for (url, nearest) in [(checker, false), (checker, true), (dot, false)] {
        assert!(r.map(url, nearest, &mut encoder).expect("map").is_some());
    }
    shared.queue.submit([encoder.finish()]);
    let errors = pollster::block_on(shared.device.pop_error_scope());
    assert!(errors.is_none(), "{errors:?}");
    assert_eq!(r.map_levels(checker, false), Some(4), "8x4 down to 1x1");
    assert_eq!(
        r.map_levels(checker, true),
        Some(1),
        "three's NearestFilter generates no mips"
    );
    assert_eq!(
        r.map_levels(dot, false),
        Some(1),
        "control: a 1x1 texture has one level"
    );
}

#[test]
fn a_mapped_plane_draws_through_its_mips() {
    let Some(mut r) = renderer() else { return };
    let url = "test://scene3d/mips-far-checker.png";
    register_png(url, 256, 256);
    let far = |nearest: bool| {
        let kind = if nearest {
            MaterialKind::Flat(Flat {
                nearest: true,
                ..Flat::default()
            })
        } else {
            MaterialKind::Basic
        };
        scene(vec![instance(
            geometry::plane(2.0, 2.0).unwrap(),
            Mat4::IDENTITY,
            Material {
                kind,
                map: Some(url.into()),
                ..Material::default()
            },
        )])
    };
    let filtered = draw(&mut r, &far(false));
    let [v, _, _, _] = pixel(&filtered, SIZE / 2, SIZE / 2);
    // a 256-texel checker over ~19 pixels averages to linear 0.5, which encodes to 188
    assert!((175..=200).contains(&v), "mipmapped average: {v}");
    let aliased = draw(&mut r, &far(true));
    let [n, _, _, _] = pixel(&aliased, SIZE / 2, SIZE / 2);
    assert!(
        !(10..=245).contains(&n),
        "control: nearest sampling picks one texel: {n}"
    );
}

// WebGL2 has multisampled renderbuffers and no multisampled textures, and the GL backend makes a renderbuffer only
// of a multisampled texture whose one usage is RENDER_ATTACHMENT; any other usage aborts the browser's fallback.
fn shaded(wgsl: &str, a: [f32; 4], blend: ShaderBlend) -> Material {
    Material {
        kind: MaterialKind::Shader,
        side: Side::Double,
        shader: Some(SurfaceShader {
            wgsl: wgsl.to_owned(),
            a,
            b: [0.0; 4],
            since: 0.0,
            blend,
        }),
        ..Material::default()
    }
}

const PAINT_A: &str = "fn shade(px: Pixel) -> vec4<f32> { return vec4<f32>(px.a.rgb, px.a.a); }";

#[test]
fn a_shader_material_colours_its_surface_with_the_programs_wgsl() {
    let Some(mut r) = renderer() else { return };
    let red = shaded(PAINT_A, [1.0, 0.0, 0.0, 1.0], ShaderBlend::Opaque);
    let plane = instance(geometry::plane(2.0, 2.0).unwrap(), Mat4::IDENTITY, red);
    let rgba = draw(&mut r, &scene(vec![plane]));
    assert_eq!(pixel(&rgba, SIZE / 2, SIZE / 2), [255, 0, 0, 255]);
    assert_eq!(
        pixel(&rgba, 2, 2),
        [0, 0, 0, 255],
        "control: nothing outside the plane"
    );
}

#[test]
fn two_shader_materials_each_run_their_own_wgsl() {
    let Some(mut r) = renderer() else { return };
    let left = instance(
        geometry::plane(1.0, 1.0).unwrap(),
        Mat4::from_translation(glam::Vec3::new(-1.0, 0.0, 0.0)),
        shaded(PAINT_A, [0.0, 1.0, 0.0, 1.0], ShaderBlend::Opaque),
    );
    let right = instance(
        geometry::plane(1.0, 1.0).unwrap(),
        Mat4::from_translation(glam::Vec3::new(1.0, 0.0, 0.0)),
        shaded(
            "fn shade(px: Pixel) -> vec4<f32> { return vec4<f32>(0.0, 0.0, 1.0, 1.0); }",
            [0.0; 4],
            ShaderBlend::Opaque,
        ),
    );
    let rgba = draw(&mut r, &scene(vec![left, right]));
    // a unit of the scene is about 9.7 pixels here
    assert_eq!(pixel(&rgba, SIZE / 2 - 10, SIZE / 2), [0, 255, 0, 255]);
    assert_eq!(pixel(&rgba, SIZE / 2 + 10, SIZE / 2), [0, 0, 255, 255]);
}

#[test]
fn a_shader_material_reads_the_canvas_clock_and_its_place_on_the_mesh() {
    let Some(mut r) = renderer() else { return };
    let wgsl = "fn shade(px: Pixel) -> vec4<f32> { return vec4<f32>(px.time, step(0.0, px.local.x), 0.0, 1.0); }";
    let plane = instance(
        geometry::plane(2.0, 2.0).unwrap(),
        Mat4::IDENTITY,
        shaded(wgsl, [0.0; 4], ShaderBlend::Opaque),
    );
    let at = |time: f32| Scene3D {
        time,
        ..scene(vec![plane.clone()])
    };
    let start = draw(&mut r, &at(0.0));
    let later = draw(&mut r, &at(1.0));
    assert_eq!(pixel(&start, SIZE / 2 + 5, SIZE / 2), [0, 255, 0, 255]);
    assert_eq!(pixel(&later, SIZE / 2 + 5, SIZE / 2), [255, 255, 0, 255]);
    assert_eq!(
        pixel(&later, SIZE / 2 - 5, SIZE / 2),
        [255, 0, 0, 255],
        "the mesh's left half"
    );
}

#[test]
fn an_additive_shader_material_adds_to_what_is_behind_it() {
    let Some(mut r) = renderer() else { return };
    let behind = instance(
        geometry::plane(2.0, 2.0).unwrap(),
        Mat4::from_translation(glam::Vec3::new(0.0, 0.0, -1.0)),
        shaded(PAINT_A, [1.0, 0.0, 0.0, 1.0], ShaderBlend::Opaque),
    );
    let glow = |blend| {
        instance(
            geometry::plane(2.0, 2.0).unwrap(),
            Mat4::IDENTITY,
            shaded(PAINT_A, [0.0, 0.0, 1.0, 1.0], blend),
        )
    };
    let added = draw(
        &mut r,
        &scene(vec![behind.clone(), glow(ShaderBlend::Additive)]),
    );
    let over = draw(&mut r, &scene(vec![behind, glow(ShaderBlend::Opaque)]));
    assert_eq!(pixel(&added, SIZE / 2, SIZE / 2), [255, 0, 255, 255]);
    assert_eq!(
        pixel(&over, SIZE / 2, SIZE / 2),
        [0, 0, 255, 255],
        "control: opaque covers"
    );
}

#[test]
fn a_shader_that_does_not_declare_shade_is_refused_before_the_device_sees_it() {
    assert!(orbital_canvas_base::shaders::check_surface(PAINT_A).is_ok());
    let wrong = orbital_canvas_base::shaders::check_surface(
        "fn tint(px: Pixel) -> vec4<f32> { return px.a; }",
    );
    assert!(wrong.is_err_and(|e| e.contains("shade")));
    let mistyped = orbital_canvas_base::shaders::check_surface(
        "fn shade(px: Pixel) -> vec4<f32> { return px.time; }",
    );
    assert!(mistyped.is_err());
}

#[test]
fn every_multisampled_target_is_only_a_render_attachment() {
    let Some(mut r) = renderer() else { return };
    let sphere = instance(
        geometry::sphere(2.0, 16, 8).unwrap(),
        Mat4::IDENTITY,
        Material::default(),
    );
    draw(&mut r, &scene(vec![sphere]));
    let targets = r.targets.as_ref().expect("the frame made its targets");
    assert_eq!(targets.samples, 4);
    let color = targets
        .color
        .as_ref()
        .expect("the multisampled color target");
    for (name, view) in [("color", color), ("depth", &targets.depth)] {
        assert_eq!(view.texture().sample_count(), 4, "{name}");
        assert_eq!(
            view.texture().usage(),
            wgpu::TextureUsages::RENDER_ATTACHMENT,
            "{name}"
        );
    }
}

#[test]
fn control_the_shadow_map_is_a_single_sample_texture_the_shader_reads() {
    let Some(mut r) = renderer() else { return };
    let view = r.shadow_view(256);
    assert_eq!(view.texture().sample_count(), 1);
    assert!(view
        .texture()
        .usage()
        .contains(wgpu::TextureUsages::TEXTURE_BINDING));
}
