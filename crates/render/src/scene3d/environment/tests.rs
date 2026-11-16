use super::super::mipmap::test_support::read_rgba16f;
use super::*;

fn cube_uv_face(d: [f32; 3]) -> f32 {
    let a = d.map(f32::abs);
    if a[0] > a[2] {
        if a[0] > a[1] {
            if d[0] > 0.0 {
                0.0
            } else {
                3.0
            }
        } else if d[1] > 0.0 {
            1.0
        } else {
            4.0
        }
    } else if a[2] > a[1] {
        if d[2] > 0.0 {
            2.0
        } else {
            5.0
        }
    } else if d[1] > 0.0 {
        1.0
    } else {
        4.0
    }
}

fn cube_uv_face_uv(d: [f32; 3], face: f32) -> [f32; 2] {
    let uv = match face as u32 {
        0 => [d[2] / d[0].abs(), d[1] / d[0].abs()],
        1 => [-d[0] / d[1].abs(), -d[2] / d[1].abs()],
        2 => [-d[0] / d[2].abs(), d[1] / d[2].abs()],
        3 => [-d[2] / d[0].abs(), d[1] / d[0].abs()],
        4 => [-d[0] / d[1].abs(), d[2] / d[1].abs()],
        _ => [d[0] / d[2].abs(), d[1] / d[2].abs()],
    };
    [0.5 * (uv[0] + 1.0), 0.5 * (uv[1] + 1.0)]
}

fn output_direction(uv: [f32; 2], face: u32) -> [f32; 3] {
    let (u, v) = (2.0 * uv[0] - 1.0, 2.0 * uv[1] - 1.0);
    match face {
        0 => [1.0, v, u],
        1 => [-u, 1.0, -v],
        2 => [-u, v, 1.0],
        3 => [-1.0, v, -u],
        4 => [-u, -1.0, v],
        _ => [u, v, -1.0],
    }
}

/// cube_uv_bilinear's atlas texel (nearest) for `direction` at `mip_int`.
fn atlas_texel(direction: [f32; 3], mip_int: f32) -> (u32, u32) {
    let mut face = cube_uv_face(direction);
    let filter_int = (4.0 - mip_int).max(0.0);
    let face_size = mip_int.max(4.0).exp2();
    let uv = cube_uv_face_uv(direction, face);
    let (mut x, mut y) = (
        uv[0] * (face_size - 2.0) + 1.0,
        uv[1] * (face_size - 2.0) + 1.0,
    );
    if face > 2.0 {
        y += face_size;
        face -= 3.0;
    }
    x += face * face_size + filter_int * 3.0 * 16.0;
    y += 4.0 * (256.0 - face_size);
    (x as u32, y as u32)
}

#[test]
fn a_face_direction_maps_back_to_its_face_and_uv() {
    for face in 0..6u32 {
        for uv in [[0.3, 0.7], [0.9, 0.1], [0.5, 0.5]] {
            let d = output_direction(uv, face);
            assert_eq!(cube_uv_face(d), face as f32, "face {face} uv {uv:?}");
            let back = cube_uv_face_uv(d, face as f32);
            assert!(
                (back[0] - uv[0]).abs() < 1e-6 && (back[1] - uv[1]).abs() < 1e-6,
                "{back:?} vs {uv:?}"
            );
        }
    }
}

#[test]
fn the_lod_ladder_matches_create_planes() {
    let (sizes, sigmas) = lods();
    assert_eq!(sizes, vec![256, 128, 64, 32, 16, 16, 16, 16, 16, 16, 16]);
    assert_eq!(sigmas[0], 0.0);
    assert_eq!(sigmas[1], 1.0 / 128.0);
    assert_eq!(sigmas[4], 1.0 / 16.0);
    assert_eq!(&sigmas[5..], &EXTRA_LOD_SIGMA);
    assert_eq!(blur_viewport(0, &sizes), [0, 0, 768, 512]);
    assert_eq!(blur_viewport(1, &sizes), [0, 512, 384, 256]);
    assert_eq!(
        blur_viewport(10, &sizes),
        [288, 960, 48, 32],
        "the last extra lod sits sixth in the 16 row"
    );
    const { assert!(ATLAS_WIDTH == 768 && ATLAS_HEIGHT == 1024) };
}

#[test]
fn blur_weights_are_three_normalized_gaussians() {
    let (sizes, _) = lods();
    let scene = blur_pass(SCENE_SIGMA, 0, &sizes);
    assert_eq!(
        scene.samples, 20,
        "sigma 0.04 over 255 pixels takes the full 20 taps"
    );
    assert_eq!(scene.d_theta, (PI / 510.0) as f32);
    assert_eq!(scene.mip_int, 8.0);
    let sum: f32 = scene.weights[0] + 2.0 * scene.weights[1..20].iter().sum::<f32>();
    assert!((sum - 1.0).abs() < 1e-5, "{sum}");
    let widest = blur_pass((0.582f64.powi(2) - 0.526f64.powi(2)).sqrt(), 9, &sizes);
    assert_eq!(widest.samples, 8);
    assert_eq!(widest.mip_int, -1.0);
    let in_window: f32 = widest.weights[0] + 2.0 * widest.weights[1..8].iter().sum::<f32>();
    assert!(
        (in_window - 1.0).abs() < 1e-5,
        "only the taps the loop reads are normalized: {in_window}"
    );
}

fn generate() -> Option<Vec<[f32; 4]>> {
    let shared = crate::device::shared_device().ok()?;
    let (device, queue) = (&shared.device, &shared.queue);
    let mut encoder = device.create_command_encoder(&Default::default());
    let environment = Environment::encode_room(device, queue, &mut encoder);
    Some(read_rgba16f(
        device,
        queue,
        encoder,
        &environment.texture,
        0,
    ))
}

fn texel(atlas: &[[f32; 4]], direction: [f32; 3], mip_int: f32) -> [f32; 4] {
    let (x, y) = atlas_texel(direction, mip_int);
    atlas[(y * ATLAS_WIDTH + x) as usize]
}

#[test]
fn the_room_has_its_ceiling_light_overhead_and_a_dim_floor() {
    let Some(atlas) = generate() else { return };
    let sharp_up = texel(&atlas, [0.0, 1.0, 0.0], 8.0);
    let sharp_down = texel(&atlas, [0.0, -1.0, 0.0], 8.0);
    assert!(
        sharp_up[1] > 5.0,
        "the 100-strong panel overhead: {sharp_up:?}"
    );
    assert!(
        sharp_down[1] < 1.0,
        "control: the floor under the camera: {sharp_down:?}"
    );
    let up = texel(&atlas, [0.0, 1.0, 0.0], -2.0);
    let down = texel(&atlas, [0.0, -1.0, 0.0], -2.0);
    assert!(down[1] > 0.0 && down[1].is_finite(), "{down:?}");
    assert!(
        up[1] > 3.0 * down[1],
        "most blurred lod: up {up:?} down {down:?}"
    );
    for t in &atlas {
        assert!(t.iter().all(|c| c.is_finite()), "{t:?}");
    }
}

#[test]
fn the_lit_shader_snippet_lights_an_upward_surface_more_than_a_downward_one() {
    let Some(shared) = crate::device::shared_device().ok() else {
        return;
    };
    let (device, queue) = (&shared.device, &shared.queue);
    let environment = Environment::room(device, queue);
    let probe = r#"
@vertex
fn vs_ibl_probe(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
  let p = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
  return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_ibl_probe(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
  let n = select(vec3<f32>(0.0, -1.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), at.x < 1.0);
  let light = ibl_indirect(n, n, vec3<f32>(1.0), vec3<f32>(0.04), 1.0, 1.0);
  return vec4<f32>(light.diffuse + light.specular, 1.0);
}
"#;
    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("ibl probe"),
        source: wgpu::ShaderSource::Wgsl(
            [orbital_canvas_base::shaders::lit_source().as_str(), probe]
                .concat()
                .into(),
        ),
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ibl probe"),
        entries: &Environment::layout_entries(3),
    });
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("ibl probe"),
        layout: &layout,
        entries: &environment.bind_entries(3),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("ibl probe"),
        bind_group_layouts: &[&layout],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("ibl probe"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_ibl_probe"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_ibl_probe"),
            targets: &[Some(FORMAT.into())],
            compilation_options: Default::default(),
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview: None,
        cache: None,
    });
    let errors = pollster::block_on(device.pop_error_scope());
    assert!(
        errors.is_none(),
        "the snippet must compile beside prelude and lit: {errors:?}"
    );
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ibl probe"),
        size: wgpu::Extent3d {
            width: 2,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ibl probe"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
    }
    let pixels = read_rgba16f(device, queue, encoder, &target, 0);
    let (up, down) = (pixels[0], pixels[1]);
    assert!(
        down[1] > 0.0,
        "the floor still reflects some room light: {down:?}"
    );
    assert!(up[1] > down[1], "up {up:?} down {down:?}");

    let placeholder = Environment::placeholder(device);
    assert_eq!(
        placeholder.texture.format(),
        FORMAT,
        "control: the stand-in binds to the same layout"
    );
}
