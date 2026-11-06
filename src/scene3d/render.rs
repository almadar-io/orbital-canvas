//! Draws a [`Scene3D`] on the GPU: vertex transforms, lighting and shadows run in shaders, meshes upload once and are
//! cached by content, and the frame renders multisampled into any target texture of the renderer's device.

use super::model::{MaterialKind, MeshData, Projection, Scene3D, Side};
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
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ObjectUniform {
    model: [[f32; 4]; 4],
    normal_model: [[f32; 4]; 4],
    color: [f32; 4],
    emissive: [f32; 4],
    params: [f32; 4],
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
    frame_buffer: wgpu::Buffer,
    object_buffer: wgpu::Buffer,
    object_capacity: u64,
    color_sampler: wgpu::Sampler,
    shadow_sampler: wgpu::Sampler,
    shadow_map: Option<(u32, wgpu::TextureView)>,
    placeholder_shadow: wgpu::TextureView,
    white_map: wgpu::BindGroup,
    maps: HashMap<String, (wgpu::BindGroup, u64)>,
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
            source: wgpu::ShaderSource::Wgsl(include_str!("scene3d.wgsl").into()),
        });
        let frame_size = std::mem::size_of::<FrameUniform>() as u64;
        let object_size = std::mem::size_of::<ObjectUniform>() as u64;
        let vertex_fragment = wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT;
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
        let frame_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene3d frame"),
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
        let white = upload_rgba(&device, &queue, 1, 1, &[255, 255, 255, 255]);
        let white_map = map_bind_group(&device, &map_layout, &white, &color_sampler);
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
            frame_buffer,
            object_buffer,
            object_capacity,
            color_sampler,
            shadow_sampler,
            shadow_map: None,
            placeholder_shadow,
            white_map,
            maps: HashMap::new(),
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
                        blend: key.transparent.then_some(wgpu::BlendState::ALPHA_BLENDING),
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
                    depth_write_enabled: !key.transparent,
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

    /// The bind group of the image at `url`, uploaded as sRGB; `None` until the asset has loaded or when it fails.
    fn map(&mut self, url: &str) -> Option<&wgpu::BindGroup> {
        let clock = self.clock;
        if self.maps.contains_key(url) {
            let entry = self.maps.get_mut(url)?;
            entry.1 = clock;
            return Some(&entry.0);
        }
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
        let (w, h) = rgba.dimensions();
        let texture = upload_rgba(&self.device, &self.queue, w, h, rgba.as_raw());
        let group = map_bind_group(
            &self.device,
            &self.map_layout,
            &texture,
            &self.color_sampler,
        );
        self.maps.insert(url.to_string(), (group, clock));
        self.maps.get(url).map(|entry| &entry.0)
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
        let aspect = width as f32 / height.max(1) as f32;
        let view_proj = view_projection(scene, aspect);

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
                (scene.tone_mapping == super::model::ToneMapping::Aces) as u32 as f32,
                (!format.is_srgb()) as u32 as f32,
                0.0,
            ],
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
            let kind = match m.kind {
                MaterialKind::Basic => 0.0,
                MaterialKind::Standard => 1.0,
                MaterialKind::Physical => 2.0,
                MaterialKind::Toon => 3.0,
            };
            let uniform = ObjectUniform {
                model: model.to_cols_array_2d(),
                normal_model: model.inverse().transpose().to_cols_array_2d(),
                color: [m.color[0], m.color[1], m.color[2], m.opacity],
                emissive: color3(m.emissive, m.emissive_intensity, 0.0),
                params: [m.metalness, m.roughness, kind, flags as f32],
            };
            let offset = i * OBJECT_STRIDE as usize;
            objects[offset..offset + std::mem::size_of::<ObjectUniform>()]
                .copy_from_slice(bytemuck::bytes_of(&uniform));
            let key = self.mesh(&instance.mesh);
            let center = model.transform_point3(centroid(&instance.mesh));
            let depth = view_proj.project_point3(center).z;
            draws.push((
                i,
                key,
                depth,
                m.is_transparent(),
                m.side,
                m.map.clone(),
                instance.cast_shadow,
            ));
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
                for (i, key, _, transparent, _, _, cast) in &draws {
                    if !cast || *transparent {
                        continue;
                    }
                    let mesh = &self.meshes[key];
                    pass.set_bind_group(1, &object_group, &[(*i as u64 * OBJECT_STRIDE) as u32]);
                    pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                    pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..mesh.count, 0, 0..1);
                }
                drop(pass);
                view
            }
            None => self.placeholder_shadow.clone(),
        };

        let frame_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene3d frame"),
            layout: &self.frame_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.frame_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&shadow_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.shadow_sampler),
                },
            ],
        });

        // Opaque front to back, then transparent back to front.
        let mut order: Vec<usize> = (0..draws.len()).collect();
        order.sort_by(|&a, &b| {
            let (da, db) = (&draws[a], &draws[b]);
            da.3.cmp(&db.3).then_with(|| {
                if da.3 {
                    db.2.total_cmp(&da.2)
                } else {
                    da.2.total_cmp(&db.2)
                }
            })
        });
        let mut map_groups: Vec<Option<wgpu::BindGroup>> = Vec::with_capacity(draws.len());
        for draw in &draws {
            map_groups.push(draw.5.as_deref().and_then(|url| self.map(url).cloned()));
        }
        for draw in &draws {
            self.pipeline(PipelineKey {
                format,
                samples,
                side: draw.4,
                transparent: draw.3,
            });
        }
        self.ensure_targets(width, height, format, samples);
        let targets = self.targets.as_ref().expect("just ensured");
        let target_view = target.create_view(&Default::default());
        let bg = scene.background;
        let clear = wgpu::Color {
            r: bg[0] as f64,
            g: bg[1] as f64,
            b: bg[2] as f64,
            a: bg[3] as f64,
        };
        let (view, resolve) = match &targets.color {
            Some(msaa) => (msaa, Some(&target_view)),
            None => (&target_view, None),
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
        for index in order {
            let (i, key, _, transparent, side, _, _) = &draws[index];
            let pipeline = &self.pipelines[&PipelineKey {
                format,
                samples,
                side: *side,
                transparent: *transparent,
            }];
            let mesh = &self.meshes[key];
            pass.set_pipeline(pipeline);
            pass.set_bind_group(1, &object_group, &[(*i as u64 * OBJECT_STRIDE) as u32]);
            pass.set_bind_group(
                2,
                map_groups[index].as_ref().unwrap_or(&self.white_map),
                &[],
            );
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.count, 0, 0..1);
        }
        drop(pass);

        let clock = self.clock;
        self.meshes
            .retain(|_, mesh| clock - mesh.last_used <= KEEP_FRAMES);
        self.maps.retain(|_, entry| clock - entry.1 <= KEEP_FRAMES);
        Ok(())
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
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
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
