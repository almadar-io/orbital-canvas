//! Draws a [`Scene3D`] on the GPU: vertex transforms, lighting and shadows run in shaders, meshes upload once and are
//! cached by content, and the frame renders multisampled into any target texture of the renderer's device.

use super::environment::{self, Environment};
use super::mipmap::{mip_level_count, MipmapError, MipmapGenerator};
use super::model::{EnvironmentKind, Flat, MaterialKind, MeshData, Projection, Scene3D, Side};
use super::post::{self, PostStack};
use super::transmission::{self, Transmission};
use glam::{Mat4, Vec3};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

const OBJECT_STRIDE: u64 = 256;
const MAX_POINTS: usize = 8;
const KEEP_FRAMES: u64 = 120;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct FrameUniform {
    view_proj: [[f32; 4]; 4],
    light_view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 4],
    ambient: [f32; 4],
    hemi_sky: [f32; 4],
    hemi_ground: [f32; 4],
    dir_to_light: [f32; 4],
    dir_color: [f32; 4],
    shadow: [f32; 4],
    point_pos: [[f32; 4]; MAX_POINTS],
    point_color: [[f32; 4]; MAX_POINTS],
    options: [f32; 4],
    camera_right: [f32; 4],
    camera_up: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ObjectUniform {
    model: [[f32; 4]; 4],
    normal_model: [[f32; 4]; 4],
    color: [f32; 4],
    emissive: [f32; 4],
    params: [f32; 4],
    physical: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GridUniform {
    center: [f32; 4],
    size: [f32; 4],
    lines: [f32; 4],
    cell_color: [f32; 4],
    section_color: [f32; 4],
    camera_on_plane: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
}

const FLAG_FLAT: u32 = 1;
const FLAG_RECEIVE_SHADOW: u32 = 2;
const FLAG_MAP: u32 = 4;
const FLAG_CUTOUT: u32 = 8;
const FLAG_BILLBOARD: u32 = 16;

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    count: u32,
    last_used: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct PipelineKey {
    format: wgpu::TextureFormat,
    samples: u32,
    side: Side,
    transparent: bool,
    additive: bool,
}

/// three's `WebGLRenderList` lists, in drawing order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum List {
    Opaque,
    Transmissive,
    Transparent,
}

/// One mesh's draw: its object slot, cached mesh, view depth and the state its pipeline and map are chosen by.
struct Draw {
    slot: usize,
    mesh: u64,
    depth: f32,
    list: List,
    transparent: bool,
    additive: bool,
    side: Side,
    map: Option<(String, bool)>,
    cast_shadow: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GridKey {
    format: wgpu::TextureFormat,
    samples: u32,
}

struct Targets {
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    samples: u32,
    color: Option<wgpu::TextureView>,
    depth: wgpu::TextureView,
}

#[derive(Debug, thiserror::Error)]
pub enum Render3DError {
    #[error("the scene is invalid: {0}")]
    Invalid(String),
    #[error(transparent)]
    Mipmap(#[from] MipmapError),
}

pub struct Renderer3D {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    shader: wgpu::ShaderModule,
    frame_layout: wgpu::BindGroupLayout,
    shadow_frame_layout: wgpu::BindGroupLayout,
    object_layout: wgpu::BindGroupLayout,
    map_layout: wgpu::BindGroupLayout,
    lit_layout: wgpu::PipelineLayout,
    shadow_pipeline: wgpu::RenderPipeline,
    pipelines: HashMap<PipelineKey, wgpu::RenderPipeline>,
    grid_shader: wgpu::ShaderModule,
    grid_layout: wgpu::BindGroupLayout,
    grid_pipeline_layout: wgpu::PipelineLayout,
    grid_pipelines: HashMap<GridKey, wgpu::RenderPipeline>,
    grid_buffer: wgpu::Buffer,
    frame_buffer: wgpu::Buffer,
    object_buffer: wgpu::Buffer,
    object_capacity: u64,
    /// The frame uniform of the passes that draw into the transmission target (no tone mapping, no encode).
    linear_frame_buffer: wgpu::Buffer,
    color_sampler: wgpu::Sampler,
    nearest_sampler: wgpu::Sampler,
    shadow_sampler: wgpu::Sampler,
    shadow_map: Option<(u32, wgpu::TextureView)>,
    placeholder_shadow: wgpu::TextureView,
    white_map: wgpu::BindGroup,
    mips: MipmapGenerator,
    /// Keyed by the image and whether it samples nearest.
    maps: HashMap<(String, bool), (wgpu::BindGroup, wgpu::Texture, u64)>,
    room: Option<Environment>,
    no_environment: Environment,
    transmission: Transmission,
    post: Option<PostStack>,
    meshes: HashMap<u64, GpuMesh>,
    targets: Option<Targets>,
    clock: u64,
}

fn mesh_key(mesh: &MeshData) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytemuck::cast_slice::<[f32; 3], u8>(&mesh.positions).hash(&mut hasher);
    bytemuck::cast_slice::<[f32; 3], u8>(&mesh.normals).hash(&mut hasher);
    bytemuck::cast_slice::<[f32; 2], u8>(&mesh.uvs).hash(&mut hasher);
    bytemuck::cast_slice::<u32, u8>(&mesh.indices).hash(&mut hasher);
    hasher.finish()
}

fn uniform_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    dynamic: bool,
    size: u64,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: dynamic,
            min_binding_size: wgpu::BufferSize::new(size),
        },
        count: None,
    }
}

fn vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRIBUTES: [wgpu::VertexAttribute; 3] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2];
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<Vertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &ATTRIBUTES,
    }
}

fn color3(rgb: [f32; 3], scale: f32, w: f32) -> [f32; 4] {
    [rgb[0] * scale, rgb[1] * scale, rgb[2] * scale, w]
}

/// The lit shader's source: the shared prelude, the lit pass, three's IBL and its transmission.
pub(super) fn lit_source() -> String {
    [
        include_str!("prelude.wgsl"),
        include_str!("lit.wgsl"),
        environment::IBL_WGSL,
        transmission::WGSL,
    ]
    .concat()
}

/// The camera's view-projection for an `aspect` (width over height).
pub fn view_projection(scene: &Scene3D, aspect: f32) -> Mat4 {
    let c = &scene.camera;
    let view = Mat4::look_at_rh(Vec3::from(c.eye), Vec3::from(c.target), Vec3::from(c.up));
    let projection = match c.projection {
        Projection::Perspective { fov_y_degrees } => {
            Mat4::perspective_rh(fov_y_degrees.to_radians(), aspect, c.near, c.far)
        }
        Projection::Orthographic { height } => {
            let (h, w) = (height * 0.5, height * 0.5 * aspect);
            Mat4::orthographic_rh(-w, w, -h, h, c.near, c.far)
        }
    };
    projection * view
}

fn light_view_projection(position: Vec3, target: Vec3, extent: f32, near: f32, far: f32) -> Mat4 {
    let forward = (target - position).normalize_or_zero();
    let up = if forward.cross(Vec3::Y).length_squared() < 1e-6 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    Mat4::orthographic_rh(-extent, extent, -extent, extent, near, far)
        * Mat4::look_at_rh(position, target, up)
}

impl Renderer3D {
    pub fn new(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene3d shader"),
            source: wgpu::ShaderSource::Wgsl(lit_source().into()),
        });
        let frame_size = std::mem::size_of::<FrameUniform>() as u64;
        let object_size = std::mem::size_of::<ObjectUniform>() as u64;
        let vertex_fragment = wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT;
        let [environment_texture, environment_sampler] = Environment::layout_entries(3);
        let [transmission_texture, transmission_sampler] = Transmission::layout_entries(5);
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene3d frame"),
            entries: &[
                uniform_entry(0, vertex_fragment, false, frame_size),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
                environment_texture,
                environment_sampler,
                transmission_texture,
                transmission_sampler,
            ],
        });
        let shadow_frame_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("scene3d shadow frame"),
                entries: &[uniform_entry(0, vertex_fragment, false, frame_size)],
            });
        let object_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene3d object"),
            entries: &[uniform_entry(0, vertex_fragment, true, object_size)],
        });
        let map_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene3d map"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let lit_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene3d lit"),
            bind_group_layouts: &[&frame_layout, &object_layout, &map_layout],
            push_constant_ranges: &[],
        });
        let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene3d shadow"),
            bind_group_layouts: &[&shadow_frame_layout, &object_layout],
            push_constant_ranges: &[],
        });
        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene3d shadow"),
            layout: Some(&shadow_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_shadow"),
                buffers: &[vertex_layout()],
                compilation_options: Default::default(),
            },
            fragment: None,
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
        let grid_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene3d grid shader"),
            source: wgpu::ShaderSource::Wgsl(
                concat!(include_str!("prelude.wgsl"), include_str!("grid.wgsl")).into(),
            ),
        });
        let grid_size = std::mem::size_of::<GridUniform>() as u64;
        let grid_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene3d grid"),
            entries: &[uniform_entry(0, vertex_fragment, false, grid_size)],
        });
        let grid_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene3d grid"),
            bind_group_layouts: &[&shadow_frame_layout, &grid_layout],
            push_constant_ranges: &[],
        });
        let grid_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene3d grid"),
            size: grid_size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let frame_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene3d frame"),
            size: frame_size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let linear_frame_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene3d linear frame"),
            size: frame_size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let object_capacity = 64;
        let object_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene3d objects"),
            size: OBJECT_STRIDE * object_capacity,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let color_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("scene3d map sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let nearest_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("scene3d nearest map sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("scene3d shadow sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let placeholder_shadow = depth_texture(&device, 1, 1, 1, "scene3d no shadow");
        let white = upload_rgba(&device, &queue, 1, 1, 1, &[255, 255, 255, 255]);
        let white_map = map_bind_group(&device, &map_layout, &white, &color_sampler);
        let mips = MipmapGenerator::new(device.clone());
        let no_environment = Environment::placeholder(&device);
        let transmission = Transmission::new(device.clone());
        Self {
            device,
            queue,
            shader,
            frame_layout,
            shadow_frame_layout,
            object_layout,
            map_layout,
            lit_layout,
            shadow_pipeline,
            pipelines: HashMap::new(),
            grid_shader,
            grid_layout,
            grid_pipeline_layout,
            grid_pipelines: HashMap::new(),
            grid_buffer,
            frame_buffer,
            linear_frame_buffer,
            object_buffer,
            object_capacity,
            color_sampler,
            nearest_sampler,
            shadow_sampler,
            shadow_map: None,
            placeholder_shadow,
            white_map,
            mips,
            maps: HashMap::new(),
            room: None,
            no_environment,
            transmission,
            post: None,
            meshes: HashMap::new(),
            targets: None,
            clock: 0,
        }
    }

    /// How many distinct meshes are on the GPU.
    pub fn cached_meshes(&self) -> usize {
        self.meshes.len()
    }

    fn pipeline(&mut self, key: PipelineKey) -> &wgpu::RenderPipeline {
        let (device, shader, layout) = (&self.device, &self.shader, &self.lit_layout);
        self.pipelines.entry(key).or_insert_with(|| {
            let cull_mode = match key.side {
                Side::Front => Some(wgpu::Face::Back),
                Side::Back => Some(wgpu::Face::Front),
                Side::Double => None,
            };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("scene3d lit"),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some("vs_main"),
                    buffers: &[vertex_layout()],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: key.format,
                        blend: match (key.transparent, key.additive) {
                            (_, true) => Some(ADDITIVE),
                            (true, false) => Some(wgpu::BlendState::ALPHA_BLENDING),
                            (false, false) => None,
                        },
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    // three keeps depthWrite on for transparent materials
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::LessEqual,
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: key.samples,
                    ..Default::default()
                },
                multiview: None,
                cache: None,
            })
        })
    }

    fn grid_pipeline(&mut self, key: GridKey) {
        let (device, shader, layout) =
            (&self.device, &self.grid_shader, &self.grid_pipeline_layout);
        self.grid_pipelines.entry(key).or_insert_with(|| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("scene3d grid"),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some("vs_grid"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some("fs_grid"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: key.format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                // drei draws the grid's back side, which faces up: visible from above only
                primitive: wgpu::PrimitiveState {
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: Some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::LessEqual,
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: key.samples,
                    ..Default::default()
                },
                multiview: None,
                cache: None,
            })
        });
    }

    fn mesh(&mut self, data: &MeshData) -> u64 {
        let key = mesh_key(data);
        let clock = self.clock;
        if let Some(mesh) = self.meshes.get_mut(&key) {
            mesh.last_used = clock;
            return key;
        }
        let vertices: Vec<Vertex> = data
            .positions
            .iter()
            .enumerate()
            .map(|(i, &position)| Vertex {
                position,
                normal: data.normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]),
                uv: data.uvs.get(i).copied().unwrap_or([0.0, 0.0]),
            })
            .collect();
        use wgpu::util::DeviceExt;
        let vertex_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("scene3d vertices"),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let index_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("scene3d indices"),
                contents: bytemuck::cast_slice(&data.indices),
                usage: wgpu::BufferUsages::INDEX,
            });
        self.meshes.insert(
            key,
            GpuMesh {
                vertices: vertex_buffer,
                indices: index_buffer,
                count: data.indices.len() as u32,
                last_used: clock,
            },
        );
        key
    }

    /// The bind group of the image at `url`, uploaded as sRGB and sampled nearest or linear; `None` until the asset
    /// has loaded or when it fails. A linear map gets its full mip chain, generated on `encoder` (three's
    /// `generateMipmaps` with `LinearMipmapLinearFilter`); a nearest one keeps its single level, as three's
    /// `NearestFilter` does.
    fn map(
        &mut self,
        url: &str,
        nearest: bool,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<Option<&wgpu::BindGroup>, Render3DError> {
        let clock = self.clock;
        let key = (url.to_string(), nearest);
        if self.maps.contains_key(&key) {
            let entry = self.maps.get_mut(&key).expect("just checked");
            entry.2 = clock;
            return Ok(Some(&entry.0));
        }
        let Some(rgba) = self.decode_map(url) else {
            return Ok(None);
        };
        let (w, h) = rgba.dimensions();
        let levels = if nearest { 1 } else { mip_level_count(w, h) };
        let texture = upload_rgba(&self.device, &self.queue, w, h, levels, rgba.as_raw());
        self.mips.generate(encoder, &texture)?;
        let sampler = if nearest {
            &self.nearest_sampler
        } else {
            &self.color_sampler
        };
        let group = map_bind_group(&self.device, &self.map_layout, &texture, sampler);
        self.maps.insert(key.clone(), (group, texture, clock));
        Ok(self.maps.get(&key).map(|entry| &entry.0))
    }

    fn decode_map(&self, url: &str) -> Option<image::RgbaImage> {
        crate::asset::ensure_asset(url);
        let bytes = crate::asset::asset_bytes(url)?;
        let rgba = match crate::asset::asset_kind(url)? {
            crate::asset::AssetKind::Raster => image::ImageReader::new(std::io::Cursor::new(&bytes))
                .with_guessed_format()
                .ok()?
                .decode()
                .map_err(|error| tracing::warn!(target: "almadar::scene3d", url, %error, "map decode failed"))
                .ok()?
                .into_rgba8(),
            crate::asset::AssetKind::Svg => {
                let size = crate::svg::intrinsic_size(url, &bytes)?;
                let raster = crate::svg::raster_size(size, 2.0, self.device.limits().max_texture_dimension_2d);
                let r = crate::svg::rasterize(url, &bytes, raster.0, raster.1)?;
                image::RgbaImage::from_raw(raster.0, raster.1, r.rgba)?
            }
        };
        Some(rgba)
    }

    fn ensure_targets(
        &mut self,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        samples: u32,
    ) {
        let fits = self.targets.as_ref().is_some_and(|t| {
            t.width == width && t.height == height && t.format == format && t.samples == samples
        });
        if fits {
            return;
        }
        let color = (samples > 1).then(|| {
            self.device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("scene3d msaa color"),
                    size: wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        });
        let depth = depth_texture(&self.device, width, height, samples, "scene3d depth");
        self.targets = Some(Targets {
            width,
            height,
            format,
            samples,
            color,
            depth,
        });
    }

    fn shadow_view(&mut self, size: u32) -> wgpu::TextureView {
        if self.shadow_map.as_ref().map(|(s, _)| *s) != Some(size) {
            self.shadow_map = Some((
                size,
                depth_texture(&self.device, size, size, 1, "scene3d shadow map"),
            ));
        }
        self.shadow_map
            .as_ref()
            .map(|(_, view)| view.clone())
            .expect("just set")
    }

    /// Render `scene` into `target` (a texture of this renderer's device, usable as a render attachment), clearing it to
    /// the scene's background first. `samples` is the multisample count (1 or 4).
    pub fn render(
        &mut self,
        scene: &Scene3D,
        target: &wgpu::Texture,
        samples: u32,
    ) -> Result<(), Render3DError> {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("scene3d"),
            });
        self.encode(&mut encoder, scene, target, samples)?;
        self.queue.submit([encoder.finish()]);
        Ok(())
    }

    fn post_stack(&mut self, format: wgpu::TextureFormat) -> &mut PostStack {
        if self.post.as_ref().map(|p| p.format()) != Some(format) {
            self.post = Some(PostStack::new(
                self.device.clone(),
                self.queue.clone(),
                format,
            ));
        }
        self.post.as_mut().expect("just set")
    }

    fn frame_group(
        &self,
        uniform: &wgpu::Buffer,
        shadow: &wgpu::TextureView,
        with_environment: bool,
        transmission: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        let environment = if with_environment {
            self.room
                .as_ref()
                .expect("the environment is built before the frame")
        } else {
            &self.no_environment
        };
        let [environment_texture, environment_sampler] = environment.bind_entries(3);
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene3d frame"),
            layout: &self.frame_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(shadow),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.shadow_sampler),
                },
                environment_texture,
                environment_sampler,
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(transmission),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::Sampler(self.transmission.sampler()),
                },
            ],
        })
    }

    fn draw_mesh(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        draw: &Draw,
        objects: &wgpu::BindGroup,
        map: Option<&wgpu::BindGroup>,
        key: PipelineKey,
    ) {
        let mesh = &self.meshes[&draw.mesh];
        pass.set_pipeline(&self.pipelines[&key]);
        pass.set_bind_group(1, objects, &[(draw.slot as u64 * OBJECT_STRIDE) as u32]);
        pass.set_bind_group(2, map.unwrap_or(&self.white_map), &[]);
        pass.set_vertex_buffer(0, mesh.vertices.slice(..));
        pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..mesh.count, 0, 0..1);
    }

    pub fn encode(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        scene: &Scene3D,
        target: &wgpu::Texture,
        samples: u32,
    ) -> Result<(), Render3DError> {
        self.clock += 1;
        let (width, height, format) = (target.width(), target.height(), target.format());
        let samples = if samples > 1 { 4 } else { 1 };
        if scene.lights.points.len() > MAX_POINTS {
            return Err(Render3DError::Invalid(format!(
                "{} point lights; a scene holds at most {MAX_POINTS}",
                scene.lights.points.len()
            )));
        }
        // @react-three/postprocessing mounts its composer only around an effect
        let effects = scene
            .post
            .filter(|p| p.bloom.is_some() || p.vignette.is_some());
        let lit_format = if effects.is_some() {
            post::HDR_FORMAT
        } else {
            format
        };
        let with_environment = scene.environment == Some(EnvironmentKind::Room);
        let aspect = width as f32 / height.max(1) as f32;
        let view_proj = view_projection(scene, aspect);
        let view = Mat4::look_at_rh(
            Vec3::from(scene.camera.eye),
            Vec3::from(scene.camera.target),
            Vec3::from(scene.camera.up),
        );
        let (right, up) = (view.row(0), view.row(1));

        let shadow = scene
            .lights
            .directional
            .and_then(|light| light.shadow.filter(|_| scene.shadows).map(|s| (light, s)));
        let light_view_proj = shadow
            .map(|(light, s)| {
                light_view_projection(
                    Vec3::from(light.position),
                    Vec3::from(light.target),
                    s.extent,
                    s.near,
                    s.far,
                )
            })
            .unwrap_or(Mat4::IDENTITY);

        let mut frame = FrameUniform {
            view_proj: view_proj.to_cols_array_2d(),
            light_view_proj: light_view_proj.to_cols_array_2d(),
            camera_pos: [
                scene.camera.eye[0],
                scene.camera.eye[1],
                scene.camera.eye[2],
                1.0,
            ],
            ambient: scene
                .lights
                .ambient
                .map_or([0.0; 4], |a| color3(a.color, a.intensity, 0.0)),
            hemi_sky: scene
                .lights
                .hemisphere
                .map_or([0.0; 4], |h| color3(h.sky, h.intensity, 1.0)),
            hemi_ground: scene
                .lights
                .hemisphere
                .map_or([0.0; 4], |h| color3(h.ground, h.intensity, 0.0)),
            dir_to_light: [0.0; 4],
            dir_color: [0.0; 4],
            shadow: [0.0; 4],
            point_pos: [[0.0; 4]; MAX_POINTS],
            point_color: [[0.0; 4]; MAX_POINTS],
            options: [
                scene.lights.points.len() as f32,
                (effects.is_none() && scene.tone_mapping == super::model::ToneMapping::Aces) as u32
                    as f32,
                (effects.is_none() && !format.is_srgb()) as u32 as f32,
                with_environment as u32 as f32,
            ],
            camera_right: [right.x, right.y, right.z, 0.0],
            camera_up: [up.x, up.y, up.z, 0.0],
        };
        if let Some(light) = scene.lights.directional {
            let to_light =
                (Vec3::from(light.position) - Vec3::from(light.target)).normalize_or_zero();
            frame.dir_to_light = [to_light.x, to_light.y, to_light.z, 1.0];
            frame.dir_color = color3(light.color, light.intensity, 0.0);
        }
        if let Some((_, s)) = shadow {
            frame.shadow = [s.bias, s.normal_bias, 1.0 / s.map_size.max(1) as f32, 1.0];
        }
        for (i, p) in scene.lights.points.iter().enumerate() {
            frame.point_pos[i] = [p.position[0], p.position[1], p.position[2], p.distance];
            frame.point_color[i] = color3(p.color, p.intensity, p.decay);
        }
        self.queue
            .write_buffer(&self.frame_buffer, 0, bytemuck::bytes_of(&frame));
        // three renders the transmission target with NoToneMapping into a linear half-float buffer
        let linear_frame = FrameUniform {
            options: [frame.options[0], 0.0, 0.0, frame.options[3]],
            ..frame
        };
        self.queue.write_buffer(
            &self.linear_frame_buffer,
            0,
            bytemuck::bytes_of(&linear_frame),
        );

        // Objects: one uniform slot each, and the meshes uploaded (or found in the cache).
        let needed = scene.meshes.len().max(1) as u64;
        if needed > self.object_capacity {
            self.object_capacity = needed.next_power_of_two();
            self.object_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("scene3d objects"),
                size: OBJECT_STRIDE * self.object_capacity,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        let mut objects = vec![0u8; (OBJECT_STRIDE * needed) as usize];
        let mut draws = Vec::with_capacity(scene.meshes.len());
        for (i, instance) in scene.meshes.iter().enumerate() {
            let m = &instance.material;
            if instance.mesh.indices.len() % 3 != 0 {
                return Err(Render3DError::Invalid(format!(
                    "mesh {i}: the index count is not a multiple of three"
                )));
            }
            let model = Mat4::from_cols_array(&instance.transform);
            let mut flags = 0u32;
            if m.flat_shading {
                flags |= FLAG_FLAT;
            }
            if instance.receive_shadow && shadow.is_some() {
                flags |= FLAG_RECEIVE_SHADOW;
            }
            if m.map.is_some() {
                flags |= FLAG_MAP;
            }
            let flat = match m.kind {
                MaterialKind::Flat(flat) => flat,
                _ => Flat::default(),
            };
            if flat.cutout {
                flags |= FLAG_CUTOUT;
            }
            if flat.billboard {
                flags |= FLAG_BILLBOARD;
            }
            let kind = match m.kind {
                MaterialKind::Basic => 0.0,
                MaterialKind::Standard => 1.0,
                MaterialKind::Physical => 2.0,
                MaterialKind::Toon => 3.0,
                MaterialKind::Flat(_) => 4.0,
            };
            let uniform = ObjectUniform {
                model: model.to_cols_array_2d(),
                normal_model: model.inverse().transpose().to_cols_array_2d(),
                color: [m.color[0], m.color[1], m.color[2], m.opacity],
                emissive: color3(m.emissive, m.emissive_intensity, 0.0),
                params: [m.metalness, m.roughness, kind, flags as f32],
                // three's default thickness, 0
                physical: [m.transmission, m.ior, 0.0, 0.0],
            };
            let offset = i * OBJECT_STRIDE as usize;
            objects[offset..offset + std::mem::size_of::<ObjectUniform>()]
                .copy_from_slice(bytemuck::bytes_of(&uniform));
            let mesh = self.mesh(&instance.mesh);
            let center = if flat.billboard {
                model.transform_point3(Vec3::ZERO)
            } else {
                model.transform_point3(centroid(&instance.mesh))
            };
            let list = if matches!(m.kind, MaterialKind::Physical) && m.transmission > 0.0 {
                List::Transmissive
            } else if m.is_transparent() {
                List::Transparent
            } else {
                List::Opaque
            };
            draws.push(Draw {
                slot: i,
                mesh,
                depth: view_proj.project_point3(center).z,
                list,
                transparent: m.is_transparent(),
                additive: flat.additive,
                side: m.side,
                map: m.map.clone().map(|url| (url, flat.nearest)),
                cast_shadow: instance.cast_shadow,
            });
        }
        self.queue.write_buffer(&self.object_buffer, 0, &objects);
        let object_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene3d objects"),
            layout: &self.object_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &self.object_buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<ObjectUniform>() as u64),
                }),
            }],
        });

        // Shadow pass.
        let shadow_view = match shadow {
            Some((_, s)) => {
                let view = self.shadow_view(s.map_size.max(1));
                let frame_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("scene3d shadow frame"),
                    layout: &self.shadow_frame_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.frame_buffer.as_entire_binding(),
                    }],
                });
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("scene3d shadow"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_pipeline(&self.shadow_pipeline);
                pass.set_bind_group(0, &frame_group, &[]);
                for draw in &draws {
                    if !draw.cast_shadow || draw.transparent {
                        continue;
                    }
                    let mesh = &self.meshes[&draw.mesh];
                    pass.set_bind_group(
                        1,
                        &object_group,
                        &[(draw.slot as u64 * OBJECT_STRIDE) as u32],
                    );
                    pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                    pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..mesh.count, 0, 0..1);
                }
                drop(pass);
                view
            }
            None => self.placeholder_shadow.clone(),
        };

        if with_environment && self.room.is_none() {
            self.room = Some(Environment::room(&self.device, &self.queue));
        }

        // Opaque front to back, transmissive front to back, then transparent back to front.
        let mut order: Vec<usize> = (0..draws.len()).collect();
        order.sort_by(|&a, &b| {
            let (da, db) = (&draws[a], &draws[b]);
            da.list.cmp(&db.list).then_with(|| {
                if da.list == List::Transparent {
                    db.depth.total_cmp(&da.depth)
                } else {
                    da.depth.total_cmp(&db.depth)
                }
            })
        });
        let mut map_groups: Vec<Option<wgpu::BindGroup>> = Vec::with_capacity(draws.len());
        for draw in &draws {
            let group = match &draw.map {
                Some((url, nearest)) => self.map(url, *nearest, encoder)?.cloned(),
                None => None,
            };
            map_groups.push(group);
        }
        let transmissive = draws.iter().any(|d| d.list == List::Transmissive);
        for draw in &draws {
            self.pipeline(pipeline_key(draw, lit_format, samples, draw.side));
            if transmissive {
                match draw.list {
                    List::Opaque => {
                        self.pipeline(pipeline_key(
                            draw,
                            transmission::FORMAT,
                            transmission::SAMPLES,
                            draw.side,
                        ));
                    }
                    List::Transmissive if draw.side == Side::Double => {
                        self.pipeline(pipeline_key(
                            draw,
                            transmission::FORMAT,
                            transmission::SAMPLES,
                            Side::Back,
                        ));
                    }
                    _ => {}
                }
            }
        }
        let grid_key = GridKey {
            format: lit_format,
            samples,
        };
        let grid_draw = match &scene.grid {
            Some(g) => {
                let center = Vec3::from(g.center);
                let eye = Vec3::from(scene.camera.eye);
                let uniform = GridUniform {
                    center: [center.x, center.y, center.z, 1.0],
                    size: [g.size[0], g.size[1], g.cell_size, g.section_size],
                    lines: [
                        g.cell_thickness,
                        g.section_thickness,
                        g.fade_distance,
                        g.fade_strength,
                    ],
                    cell_color: [g.cell_color[0], g.cell_color[1], g.cell_color[2], 1.0],
                    section_color: [
                        g.section_color[0],
                        g.section_color[1],
                        g.section_color[2],
                        1.0,
                    ],
                    camera_on_plane: [eye.x, center.y, eye.z, g.fade_from],
                };
                self.queue
                    .write_buffer(&self.grid_buffer, 0, bytemuck::bytes_of(&uniform));
                self.grid_pipeline(grid_key);
                let frame_only = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("scene3d grid frame"),
                    layout: &self.shadow_frame_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.frame_buffer.as_entire_binding(),
                    }],
                });
                let params = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("scene3d grid"),
                    layout: &self.grid_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.grid_buffer.as_entire_binding(),
                    }],
                });
                Some((view_proj.project_point3(center).z, frame_only, params))
            }
            None => None,
        };

        // three's renderTransmissionPass: the opaque list, then double-sided transmissive back faces.
        let transmission_view = if transmissive {
            let opaque_group = self.frame_group(
                &self.linear_frame_buffer,
                &shadow_view,
                with_environment,
                self.transmission.placeholder_view(),
            );
            let mut pass = self
                .transmission
                .begin_opaque(encoder, width, height, scene.background);
            pass.set_bind_group(0, &opaque_group, &[]);
            for &index in &order {
                let draw = &draws[index];
                if draw.list != List::Opaque {
                    continue;
                }
                self.draw_mesh(
                    &mut pass,
                    draw,
                    &object_group,
                    map_groups[index].as_ref(),
                    pipeline_key(draw, transmission::FORMAT, transmission::SAMPLES, draw.side),
                );
            }
            drop(pass);
            self.transmission.end_opaque(encoder)?;
            let back_faces: Vec<usize> = order
                .iter()
                .copied()
                .filter(|&i| draws[i].list == List::Transmissive && draws[i].side == Side::Double)
                .collect();
            if !back_faces.is_empty() {
                let back_group = self.frame_group(
                    &self.linear_frame_buffer,
                    &shadow_view,
                    with_environment,
                    &self.transmission.opaque_view(),
                );
                let mut pass = self.transmission.begin_back_faces(encoder);
                pass.set_bind_group(0, &back_group, &[]);
                for &index in &back_faces {
                    let draw = &draws[index];
                    self.draw_mesh(
                        &mut pass,
                        draw,
                        &object_group,
                        map_groups[index].as_ref(),
                        pipeline_key(
                            draw,
                            transmission::FORMAT,
                            transmission::SAMPLES,
                            Side::Back,
                        ),
                    );
                }
                drop(pass);
                self.transmission.end_back_faces(encoder)?;
            }
            Some(self.transmission.view())
        } else {
            None
        };
        let frame_group = self.frame_group(
            &self.frame_buffer,
            &shadow_view,
            with_environment,
            transmission_view
                .as_ref()
                .unwrap_or(self.transmission.placeholder_view()),
        );

        let scene_view = match effects {
            Some(_) => Some(self.post_stack(format).scene_target(width, height)),
            None => None,
        };
        self.ensure_targets(width, height, lit_format, samples);
        let targets = self.targets.as_ref().expect("just ensured");
        let target_view = target.create_view(&Default::default());
        // the clear bypasses the shader, so it takes the same sRGB encode a non-sRGB target gets there, premultiplied;
        // under post it stays linear, as three's setClearColor into the composer's buffer
        let bg = scene.background;
        let channel = |linear: f32| -> f64 {
            let encoded = if effects.is_some() || format.is_srgb() {
                linear
            } else {
                linear_to_srgb(linear)
            };
            (encoded * bg[3]) as f64
        };
        let clear = wgpu::Color {
            r: channel(bg[0]),
            g: channel(bg[1]),
            b: channel(bg[2]),
            a: bg[3] as f64,
        };
        let output = scene_view.as_ref().unwrap_or(&target_view);
        let (view, resolve) = match &targets.color {
            Some(msaa) => (msaa, Some(output)),
            None => (output, None),
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene3d lit"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: resolve,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &targets.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_bind_group(0, &frame_group, &[]);
        let mut grid_pending = grid_draw.as_ref();
        for &index in &order {
            let draw = &draws[index];
            // the grid is a transparent object: drawn among them, back to front
            if draw.list == List::Transparent {
                if let Some((_, frame_only, params)) =
                    grid_pending.filter(|(d, _, _)| *d >= draw.depth)
                {
                    pass.set_pipeline(&self.grid_pipelines[&grid_key]);
                    pass.set_bind_group(0, frame_only, &[]);
                    pass.set_bind_group(1, params, &[]);
                    pass.draw(0..6, 0..1);
                    pass.set_bind_group(0, &frame_group, &[]);
                    grid_pending = None;
                }
            }
            self.draw_mesh(
                &mut pass,
                draw,
                &object_group,
                map_groups[index].as_ref(),
                pipeline_key(draw, lit_format, samples, draw.side),
            );
        }
        if let Some((_, frame_only, params)) = grid_pending {
            pass.set_pipeline(&self.grid_pipelines[&grid_key]);
            pass.set_bind_group(0, frame_only, &[]);
            pass.set_bind_group(1, params, &[]);
            pass.draw(0..6, 0..1);
        }
        drop(pass);

        if let (Some(settings), Some(scene_view)) = (effects, &scene_view) {
            let bloom = settings.bloom.map(|b| post::Bloom {
                intensity: b.intensity,
                threshold: b.threshold,
                smoothing: b.smoothing,
            });
            let vignette = settings.vignette.map(|v| post::Vignette {
                offset: v.offset,
                darkness: v.darkness,
            });
            self.post_stack(format).apply(
                encoder,
                scene_view,
                (width, height),
                &target_view,
                bloom,
                vignette,
            );
        }

        let clock = self.clock;
        self.meshes
            .retain(|_, mesh| clock - mesh.last_used <= KEEP_FRAMES);
        self.maps.retain(|_, entry| clock - entry.2 <= KEEP_FRAMES);
        Ok(())
    }

    #[cfg(test)]
    fn map_levels(&self, url: &str, nearest: bool) -> Option<u32> {
        self.maps
            .get(&(url.to_string(), nearest))
            .map(|entry| entry.1.mip_level_count())
    }
}

fn pipeline_key(draw: &Draw, format: wgpu::TextureFormat, samples: u32, side: Side) -> PipelineKey {
    PipelineKey {
        format,
        samples,
        side,
        transparent: draw.transparent,
        additive: draw.additive,
    }
}

/// three's `AdditiveBlending` (no premultiplied alpha): source times its alpha, added to what is there.
const ADDITIVE: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::SrcAlpha,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::SrcAlpha,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
};

fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn centroid(mesh: &MeshData) -> Vec3 {
    if mesh.positions.is_empty() {
        return Vec3::ZERO;
    }
    let sum = mesh
        .positions
        .iter()
        .fold(Vec3::ZERO, |acc, p| acc + Vec3::from(*p));
    sum / mesh.positions.len() as f32
}

fn depth_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    samples: u32,
    label: &str,
) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

fn upload_rgba(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    width: u32,
    height: u32,
    levels: u32,
    rgba: &[u8],
) -> wgpu::Texture {
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("scene3d map"),
        size,
        mip_level_count: levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * width),
            rows_per_image: Some(height),
        },
        size,
    );
    texture
}

fn map_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    texture: &wgpu::Texture,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    let view = texture.create_view(&Default::default());
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("scene3d map"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

#[cfg(test)]
mod integration_tests {
    use super::super::geometry;
    use super::super::model::*;
    use super::Renderer3D;
    use glam::Mat4;

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
        let rgba = crate::renderer::copy_texture_to_rgba(
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
        crate::asset::register_asset(url, bytes.into(), Some("image/png"));
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
            n < 10 || n > 245,
            "control: nearest sampling picks one texel: {n}"
        );
    }
}
