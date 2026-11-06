//! `environment: 'room'` as three.js lights it today: three r160's `RoomEnvironment` (built without a renderer, so
//! its point light has intensity 5) rendered by `PMREMGenerator.fromScene(scene, 0.04)` into the CubeUV atlas that
//! MeshStandardMaterial and MeshPhysicalMaterial sample as `scene.environment`.
//!
//! Integration in `render.rs`:
//! 1. Generate once and keep it: [`Environment::room`] (or [`Environment::encode_room`] on an encoder submitted
//!    later); bind [`Environment::placeholder`] when the scene has no environment.
//! 2. Append [`IBL_WGSL`] to the lit shader source and add [`Environment::layout_entries`]`(3)` to the frame bind
//!    group layout, [`Environment::bind_entries`]`(3)` to the frame bind group (bindings 3 and 4 of group 0).
//! 3. In `fs_main`, for standard and physical materials only (three gives `scene.environment` to
//!    MeshStandardMaterial and its subclass alone), when the frame says an environment is bound (e.g. `options.w`):
//!    `let ibl = ibl_indirect(n, v, s.diffuse, specular.rgb, specular.w, roughness);` with
//!    `specular = ibl_specular(base.rgb, metalness, ior, kind == 2u)` and
//!    `roughness = ibl_material_roughness(object.params.y, normalize(in.normal))` (taken before any non-uniform
//!    branch), then add `ibl.diffuse + ibl.specular` to the light. The ambient and hemisphere term stays as it is.

use glam::{Mat4, Quat, Vec3};
use std::f64::consts::PI;

/// The lit shader's IBL functions: the CubeUV sampler and three's `getIBLIrradiance`/`getIBLRadiance` with
/// `RE_IndirectSpecular_Physical`. It uses `PI` from prelude.wgsl and declares group 0 bindings 3 and 4.
pub const IBL_WGSL: &str = concat!(
    include_str!("cube_uv.wgsl"),
    include_str!("environment_ibl.wgsl")
);

pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub const CUBE_SIZE: u32 = 256;
const LOD_MAX: usize = 8;
const LOD_MIN: usize = 4;
pub const ATLAS_WIDTH: u32 = 3 * if CUBE_SIZE > 16 * 7 {
    CUBE_SIZE
} else {
    16 * 7
};
pub const ATLAS_HEIGHT: u32 = 4 * CUBE_SIZE;
const EXTRA_LOD_SIGMA: [f64; 6] = [0.125, 0.215, 0.35, 0.446, 0.526, 0.582];
const MAX_SAMPLES: usize = 20;
const SCENE_SIGMA: f64 = 0.04;
const NEAR: f32 = 0.1;
const FAR: f32 = 100.0;

const PHI: f32 = 1.618_034;
const INV_PHI: f32 = 1.0 / PHI;
const AXIS_DIRECTIONS: [[f32; 3]; 10] = [
    [1.0, 1.0, 1.0],
    [-1.0, 1.0, 1.0],
    [1.0, 1.0, -1.0],
    [-1.0, 1.0, -1.0],
    [0.0, PHI, INV_PHI],
    [0.0, PHI, -INV_PHI],
    [INV_PHI, 0.0, PHI],
    [-INV_PHI, 0.0, PHI],
    [PHI, INV_PHI, 0.0],
    [-PHI, INV_PHI, 0.0],
];

#[derive(Clone, Copy)]
enum Surface {
    /// MeshStandardMaterial (white, roughness 1, metalness 0); `inside` is BackSide.
    Standard { inside: bool },
    /// MeshBasicMaterial whose color is this gray (RoomEnvironment's area lights).
    Emitter(f32),
}

struct RoomObject {
    position: [f32; 3],
    rotation_y: f32,
    scale: [f32; 3],
    surface: Surface,
}

const ROOM: [RoomObject; 13] = [
    RoomObject {
        position: [-0.757, 13.219, 0.717],
        rotation_y: 0.0,
        scale: [31.713, 28.305, 28.591],
        surface: Surface::Standard { inside: true },
    },
    RoomObject {
        position: [-10.906, 2.009, 1.846],
        rotation_y: -0.195,
        scale: [2.328, 7.905, 4.651],
        surface: Surface::Standard { inside: false },
    },
    RoomObject {
        position: [-5.607, -0.754, -0.758],
        rotation_y: 0.994,
        scale: [1.970, 1.534, 3.955],
        surface: Surface::Standard { inside: false },
    },
    RoomObject {
        position: [6.167, 0.857, 7.803],
        rotation_y: 0.561,
        scale: [3.927, 6.285, 3.687],
        surface: Surface::Standard { inside: false },
    },
    RoomObject {
        position: [-2.017, 0.018, 6.124],
        rotation_y: 0.333,
        scale: [2.002, 4.566, 2.064],
        surface: Surface::Standard { inside: false },
    },
    RoomObject {
        position: [2.291, -0.756, -2.621],
        rotation_y: -0.286,
        scale: [1.546, 1.552, 1.496],
        surface: Surface::Standard { inside: false },
    },
    RoomObject {
        position: [-2.193, -0.369, -5.547],
        rotation_y: 0.516,
        scale: [3.875, 3.487, 2.986],
        surface: Surface::Standard { inside: false },
    },
    RoomObject {
        position: [-16.116, 14.37, 8.208],
        rotation_y: 0.0,
        scale: [0.1, 2.428, 2.739],
        surface: Surface::Emitter(50.0),
    },
    RoomObject {
        position: [-16.109, 18.021, -8.207],
        rotation_y: 0.0,
        scale: [0.1, 2.425, 2.751],
        surface: Surface::Emitter(50.0),
    },
    RoomObject {
        position: [14.904, 12.198, -1.832],
        rotation_y: 0.0,
        scale: [0.15, 4.265, 6.331],
        surface: Surface::Emitter(17.0),
    },
    RoomObject {
        position: [-0.462, 8.89, 14.520],
        rotation_y: 0.0,
        scale: [4.38, 5.441, 0.088],
        surface: Surface::Emitter(43.0),
    },
    RoomObject {
        position: [3.235, 11.486, -12.541],
        rotation_y: 0.0,
        scale: [2.5, 2.0, 0.1],
        surface: Surface::Emitter(20.0),
    },
    RoomObject {
        position: [0.0, 20.0, 0.0],
        rotation_y: 0.0,
        scale: [1.0, 0.1, 1.0],
        surface: Surface::Emitter(100.0),
    },
];

/// RoomEnvironment's `PointLight(0xffffff, 5, 28, 2)`.
const LIGHT_POSITION: [f32; 3] = [0.418, 16.199, 0.300];
const LIGHT_INTENSITY: f32 = 5.0;
const LIGHT_DISTANCE: f32 = 28.0;
const LIGHT_DECAY: f32 = 2.0;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct FaceUniform {
    view_proj: [[f32; 4]; 4],
    light_position: [f32; 4],
    light_color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ObjectUniform {
    model: [[f32; 4]; 4],
    normal_model: [[f32; 4]; 4],
    params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BlurUniform {
    weights: [[f32; 4]; 5],
    pole_axis: [f32; 4],
    params: [f32; 4],
    plane: [f32; 4],
}

/// `_createPlanes(lodMax)`: each lod's face size and the blur sigma that reaches it.
fn lods() -> (Vec<u32>, Vec<f64>) {
    let total = LOD_MAX - LOD_MIN + 1 + EXTRA_LOD_SIGMA.len();
    let (mut sizes, mut sigmas) = (Vec::with_capacity(total), Vec::with_capacity(total));
    let mut lod = LOD_MAX;
    for i in 0..total {
        let size = 1u32 << lod;
        sizes.push(size);
        let sigma = if i > LOD_MAX - LOD_MIN {
            EXTRA_LOD_SIGMA[i - (LOD_MAX - LOD_MIN) - 1]
        } else if i == 0 {
            0.0
        } else {
            1.0 / size as f64
        };
        sigmas.push(sigma);
        if lod > LOD_MIN {
            lod -= 1;
        }
    }
    (sizes, sigmas)
}

#[derive(Debug, Clone, PartialEq)]
struct BlurPass {
    samples: usize,
    weights: [f32; MAX_SAMPLES],
    d_theta: f32,
    mip_int: f32,
}

/// `_halfBlur`'s uniforms for reading lod `lod_in` (always finite sigmas here).
fn blur_pass(sigma: f64, lod_in: usize, sizes: &[u32]) -> BlurPass {
    let pixels = (sizes[lod_in] - 1) as f64;
    let radians_per_pixel = PI / (2.0 * pixels);
    let sigma_pixels = sigma / radians_per_pixel;
    let samples = 1 + (3.0 * sigma_pixels).floor() as usize;
    let mut raw = [0f64; MAX_SAMPLES];
    let mut sum = 0.0;
    for (i, weight) in raw.iter_mut().enumerate() {
        let x = i as f64 / sigma_pixels;
        *weight = (-x * x / 2.0).exp();
        if i == 0 {
            sum += *weight;
        } else if i < samples {
            sum += 2.0 * *weight;
        }
    }
    BlurPass {
        samples,
        weights: raw.map(|w| (w / sum) as f32),
        d_theta: radians_per_pixel as f32,
        mip_int: LOD_MAX as f32 - lod_in as f32,
    }
}

/// `_halfBlur`'s viewport for lod `lod_out`: (x, y, width, height) in atlas rows.
fn blur_viewport(lod_out: usize, sizes: &[u32]) -> [u32; 4] {
    let out = sizes[lod_out];
    let column = if lod_out > LOD_MAX - LOD_MIN {
        (lod_out - (LOD_MAX - LOD_MIN)) as u32
    } else {
        0
    };
    [3 * out * column, 4 * (CUBE_SIZE - out), 3 * out, 2 * out]
}

/// `_sceneToCubeUV`'s camera for face `i` at the origin.
fn face_view_proj(i: usize) -> Mat4 {
    const UP_SIGN: [f32; 6] = [1.0, -1.0, 1.0, 1.0, 1.0, 1.0];
    const FORWARD_SIGN: [f32; 6] = [1.0, 1.0, 1.0, -1.0, -1.0, -1.0];
    let (up, forward) = match i % 3 {
        0 => (
            Vec3::new(0.0, UP_SIGN[i], 0.0),
            Vec3::new(FORWARD_SIGN[i], 0.0, 0.0),
        ),
        1 => (
            Vec3::new(0.0, 0.0, UP_SIGN[i]),
            Vec3::new(0.0, FORWARD_SIGN[i], 0.0),
        ),
        _ => (
            Vec3::new(0.0, UP_SIGN[i], 0.0),
            Vec3::new(0.0, 0.0, FORWARD_SIGN[i]),
        ),
    };
    Mat4::perspective_rh(90f32.to_radians(), 1.0, NEAR, FAR)
        * Mat4::look_at_rh(Vec3::ZERO, forward, up)
}

/// A prefiltered environment in three's CubeUV layout, bound for the lit shader as a filterable 2D texture and a
/// linear clamped sampler.
pub struct Environment {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
}

fn uniform_layout_entry(binding: u32, size: usize) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: true,
            min_binding_size: wgpu::BufferSize::new(size as u64),
        },
        count: None,
    }
}

fn atlas(device: &wgpu::Device, label: &str, usage: wgpu::TextureUsages) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: ATLAS_WIDTH,
            height: ATLAS_HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage,
        view_formats: &[],
    })
}

fn linear_clamp(device: &wgpu::Device) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("scene3d environment"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    })
}

/// Uniform slots of one size, `stride` apart.
struct Slots {
    buffer: wgpu::Buffer,
    stride: u64,
}

impl Slots {
    fn new<T: bytemuck::Pod>(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        items: &[T],
        label: &str,
    ) -> Self {
        let align = device.limits().min_uniform_buffer_offset_alignment as u64;
        let size = std::mem::size_of::<T>() as u64;
        let stride = size.div_ceil(align) * align;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: stride * items.len().max(1) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        for (i, item) in items.iter().enumerate() {
            queue.write_buffer(&buffer, i as u64 * stride, bytemuck::bytes_of(item));
        }
        Self { buffer, stride }
    }

    fn binding<T>(&self) -> wgpu::BindingResource<'_> {
        wgpu::BindingResource::Buffer(wgpu::BufferBinding {
            buffer: &self.buffer,
            offset: 0,
            size: wgpu::BufferSize::new(std::mem::size_of::<T>() as u64),
        })
    }

    fn offset(&self, i: usize) -> u32 {
        (i as u64 * self.stride) as u32
    }
}

impl Environment {
    /// Generates the room environment and submits the work on `queue`.
    pub fn room(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("scene3d room environment"),
        });
        let environment = Self::encode_room(device, queue, &mut encoder);
        queue.submit([encoder.finish()]);
        environment
    }

    /// Records the generation on `encoder`; its uniforms are written through `queue`, so submit `encoder` on it.
    pub fn encode_room(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene3d pmrem"),
            source: wgpu::ShaderSource::Wgsl(
                concat!(
                    include_str!("cube_uv.wgsl"),
                    include_str!("environment.wgsl")
                )
                .into(),
            ),
        });
        let target = atlas(
            device,
            "scene3d environment",
            wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
        );
        let ping_pong = atlas(
            device,
            "scene3d environment ping-pong",
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let target_view = target.create_view(&Default::default());
        let ping_pong_view = ping_pong.create_view(&Default::default());
        let sampler = linear_clamp(device);

        Self::encode_scene(device, queue, encoder, &shader, &target_view);

        let (sizes, sigmas) = lods();
        // (lod in, lod out, sigma, pole axis): fromScene's blur, then _applyPMREM's
        let mut blurs = vec![(0usize, 0usize, SCENE_SIGMA, [0.0f32, 1.0, 0.0])];
        for i in 1..sizes.len() {
            let sigma = (sigmas[i] * sigmas[i] - sigmas[i - 1] * sigmas[i - 1]).sqrt();
            blurs.push((
                i - 1,
                i,
                sigma,
                AXIS_DIRECTIONS[(i - 1) % AXIS_DIRECTIONS.len()],
            ));
        }
        // each blur is a latitudinal half from the atlas into the ping-pong, then a longitudinal half back
        let mut halves = Vec::with_capacity(blurs.len() * 2);
        for &(lod_in, lod_out, sigma, pole) in &blurs {
            halves.push((lod_in, lod_out, sigma, pole, true));
            halves.push((lod_out, lod_out, sigma, pole, false));
        }
        let uniforms: Vec<BlurUniform> = halves
            .iter()
            .map(|&(lod_in, lod_out, sigma, pole, latitudinal)| {
                let pass = blur_pass(sigma, lod_in, &sizes);
                let mut weights = [[0f32; 4]; 5];
                for (i, w) in pass.weights.iter().enumerate() {
                    weights[i / 4][i % 4] = *w;
                }
                BlurUniform {
                    weights,
                    pole_axis: [pole[0], pole[1], pole[2], 0.0],
                    params: [
                        pass.samples as f32,
                        latitudinal as u32 as f32,
                        pass.d_theta,
                        pass.mip_int,
                    ],
                    plane: [sizes[lod_out] as f32, 0.0, 0.0, 0.0],
                }
            })
            .collect();
        let slots = Slots::new(device, queue, &uniforms, "scene3d pmrem blur");
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene3d pmrem blur"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                uniform_layout_entry(3, std::mem::size_of::<BlurUniform>()),
            ],
        });
        let group = |input: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("scene3d pmrem blur"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(input),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: slots.binding::<BlurUniform>(),
                    },
                ],
            })
        };
        let (from_target, from_ping_pong) = (group(&target_view), group(&ping_pong_view));
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene3d pmrem blur"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene3d pmrem blur"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_blur"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_blur"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        for (i, &(_, lod_out, _, _, latitudinal)) in halves.iter().enumerate() {
            let (input, output) = if latitudinal {
                (&from_target, &ping_pong_view)
            } else {
                (&from_ping_pong, &target_view)
            };
            let [x, y, w, h] = blur_viewport(lod_out, &sizes);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene3d pmrem blur"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: output,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, input, &[slots.offset(i)]);
            pass.set_viewport(x as f32, y as f32, w as f32, h as f32, 0.0, 1.0);
            pass.set_scissor_rect(x, y, w, h);
            pass.draw(0..36, 0..1);
        }
        Self {
            texture: target,
            view: target_view,
            sampler,
        }
    }

    /// `_sceneToCubeUV`: the six faces into the top 3 x 2 block. RoomEnvironment's room box surrounds the camera in
    /// every direction, so the background box three draws first is always covered and is not drawn.
    fn encode_scene(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        shader: &wgpu::ShaderModule,
        target: &wgpu::TextureView,
    ) {
        use wgpu::util::DeviceExt;
        let cube = super::geometry::cuboid(1.0, 1.0, 1.0).expect("a unit box");
        let vertices: Vec<[f32; 6]> = cube
            .positions
            .iter()
            .zip(&cube.normals)
            .map(|(p, n)| [p[0], p[1], p[2], n[0], n[1], n[2]])
            .collect();
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("scene3d pmrem box"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("scene3d pmrem box"),
            contents: bytemuck::cast_slice(&cube.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let faces: Vec<FaceUniform> = (0..6)
            .map(|i| FaceUniform {
                view_proj: face_view_proj(i).to_cols_array_2d(),
                light_position: [
                    LIGHT_POSITION[0],
                    LIGHT_POSITION[1],
                    LIGHT_POSITION[2],
                    LIGHT_DISTANCE,
                ],
                light_color: [
                    LIGHT_INTENSITY,
                    LIGHT_INTENSITY,
                    LIGHT_INTENSITY,
                    LIGHT_DECAY,
                ],
            })
            .collect();
        let objects: Vec<ObjectUniform> = ROOM
            .iter()
            .map(|o| {
                let model = Mat4::from_scale_rotation_translation(
                    Vec3::from(o.scale),
                    Quat::from_rotation_y(o.rotation_y),
                    Vec3::from(o.position),
                );
                let params = match o.surface {
                    Surface::Standard { inside } => {
                        [1.0, 0.0, if inside { -1.0 } else { 1.0 }, 0.0]
                    }
                    Surface::Emitter(gray) => [0.0, gray, 1.0, 0.0],
                };
                ObjectUniform {
                    model: model.to_cols_array_2d(),
                    normal_model: model.inverse().transpose().to_cols_array_2d(),
                    params,
                }
            })
            .collect();
        let face_slots = Slots::new(device, queue, &faces, "scene3d pmrem faces");
        let object_slots = Slots::new(device, queue, &objects, "scene3d pmrem objects");
        let face_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene3d pmrem face"),
            entries: &[uniform_layout_entry(0, std::mem::size_of::<FaceUniform>())],
        });
        let object_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene3d pmrem object"),
            entries: &[uniform_layout_entry(
                0,
                std::mem::size_of::<ObjectUniform>(),
            )],
        });
        let face_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene3d pmrem face"),
            layout: &face_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: face_slots.binding::<FaceUniform>(),
            }],
        });
        let object_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene3d pmrem object"),
            layout: &object_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: object_slots.binding::<ObjectUniform>(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene3d pmrem scene"),
            bind_group_layouts: &[&face_layout, &object_layout],
            push_constant_ranges: &[],
        });
        const ATTRIBUTES: [wgpu::VertexAttribute; 2] =
            wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene3d pmrem scene"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs_scene"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<[f32; 6]>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &ATTRIBUTES,
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("fs_scene"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            // every object is a closed box seen from one side, so culling (three's FrontSide/BackSide) hides only
            // what depth already hides
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let depth = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("scene3d pmrem depth"),
                size: wgpu::Extent3d {
                    width: ATLAS_WIDTH,
                    height: ATLAS_HEIGHT,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene3d pmrem scene"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        for face in 0..6 {
            let x = (face % 3) as u32 * CUBE_SIZE;
            let y = if face > 2 { CUBE_SIZE } else { 0 };
            pass.set_viewport(
                x as f32,
                y as f32,
                CUBE_SIZE as f32,
                CUBE_SIZE as f32,
                0.0,
                1.0,
            );
            pass.set_scissor_rect(x, y, CUBE_SIZE, CUBE_SIZE);
            pass.set_bind_group(0, &face_group, &[face_slots.offset(face)]);
            for object in 0..ROOM.len() {
                pass.set_bind_group(1, &object_group, &[object_slots.offset(object)]);
                pass.draw_indexed(0..cube.indices.len() as u32, 0, 0..1);
            }
        }
    }

    /// A black 1 x 1 stand-in for the bind group when the scene has no environment (the shader must not sample it).
    pub fn placeholder(device: &wgpu::Device) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scene3d no environment"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        Self {
            texture,
            view,
            sampler: linear_clamp(device),
        }
    }

    /// The atlas (`first`) and its sampler (`first + 1`) for a fragment-stage bind group layout.
    pub fn layout_entries(first: u32) -> [wgpu::BindGroupLayoutEntry; 2] {
        [
            wgpu::BindGroupLayoutEntry {
                binding: first,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: first + 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ]
    }

    pub fn bind_entries(&self, first: u32) -> [wgpu::BindGroupEntry<'_>; 2] {
        [
            wgpu::BindGroupEntry {
                binding: first,
                resource: wgpu::BindingResource::TextureView(&self.view),
            },
            wgpu::BindGroupEntry {
                binding: first + 1,
                resource: wgpu::BindingResource::Sampler(&self.sampler),
            },
        ]
    }
}

#[cfg(test)]
mod tests {
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
        assert!(ATLAS_WIDTH == 768 && ATLAS_HEIGHT == 1024);
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
                [super::super::render::lit_source().as_str(), probe]
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
}
