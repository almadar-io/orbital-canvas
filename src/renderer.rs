//! GPU pipeline and offscreen rendering for [`Canvas`].

use crate::packet::{DrawCommand, GameRenderPacket, Rect, Vec2};
use crate::{Canvas, RendererError, Result};
use std::sync::Arc;
use wgpu::util::DeviceExt;

fn align_up(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}

/// Lazily-initialized GPU resources needed for offscreen rendering.
pub(crate) struct DeviceResources {
    #[allow(dead_code)]
    pub adapter: Option<wgpu::Adapter>,
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
    /// Format used by the render pipeline (Rgba8Unorm headless; surface format on-screen).
    pub surface_format: wgpu::TextureFormat,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 2],
    color: [f32; 4],
}

impl Vertex {
    const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[
            wgpu::VertexAttribute {
                offset: 0,
                shader_location: 0,
                format: wgpu::VertexFormat::Float32x2,
            },
            wgpu::VertexAttribute {
                offset: std::mem::size_of::<[f32; 2]>() as wgpu::BufferAddress,
                shader_location: 1,
                format: wgpu::VertexFormat::Float32x4,
            },
        ],
    };
}

const SHADER_SOURCE: &str = r#"
struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = vec4<f32>(in.position, 0.0, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return in.color;
}
"#;

/// Build the vertex-colored-quad pipeline on any device for the given target
/// format. Shared by [`Canvas`] (own-device headless/surface paths) and
/// [`SharedDeviceRenderer`] (a host's device, e.g. blitz custom paint).
pub(crate) fn build_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("game shader"),
        source: wgpu::ShaderSource::Wgsl(SHADER_SOURCE.into()),
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("game pipeline layout"),
        bind_group_layouts: &[],
        push_constant_ranges: &[],
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("game pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[Vertex::LAYOUT],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

/// Textured-sprite vertex: position + UV + tint (opacity rides the alpha).
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct SpriteVertex {
    position: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}

impl SpriteVertex {
    const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<SpriteVertex>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[
            wgpu::VertexAttribute {
                offset: 0,
                shader_location: 0,
                format: wgpu::VertexFormat::Float32x2,
            },
            wgpu::VertexAttribute {
                offset: std::mem::size_of::<[f32; 2]>() as wgpu::BufferAddress,
                shader_location: 1,
                format: wgpu::VertexFormat::Float32x2,
            },
            wgpu::VertexAttribute {
                offset: std::mem::size_of::<[f32; 4]>() as wgpu::BufferAddress,
                shader_location: 2,
                format: wgpu::VertexFormat::Float32x4,
            },
        ],
    };
}

const SPRITE_SHADER_SOURCE: &str = r#"
struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = vec4<f32>(in.position, 0.0, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    return out;
}

@group(0) @binding(0) var t_diffuse: texture_2d<f32>;
@group(0) @binding(1) var s_diffuse: sampler;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(t_diffuse, s_diffuse, in.uv) * in.color;
}
"#;

const TARGET_SPRITE_SHADER_SOURCE: &str = r#"
struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = vec4<f32>(in.position, 0.0, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    return out;
}

@group(0) @binding(0) var t_target: texture_2d<f32>;
@group(0) @binding(1) var s_target: sampler;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(t_target, s_target, in.uv) * in.color.a;
}
"#;

const MASKED_SPRITE_SHADER_SOURCE: &str = r#"
struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};
struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};
@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = vec4<f32>(in.position, 0.0, 1.0);
    out.uv = in.uv;
    return out;
}
@group(0) @binding(0) var t_content: texture_2d<f32>;
@group(0) @binding(1) var t_mask: texture_2d<f32>;
@group(0) @binding(2) var s_surface: sampler;
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let content = textureSample(t_content, s_surface, in.uv);
    let coverage = textureSample(t_mask, s_surface, in.uv).a;
    let rgb = select(vec3<f32>(0.0), content.rgb / max(content.a, 0.000001), content.a > 0.0);
    return vec4<f32>(rgb, content.a * coverage);
}
"#;

fn textured_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    layout: &wgpu::BindGroupLayout,
    source: &str,
    blend: wgpu::BlendState,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("game textured shader"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("game textured pipeline layout"),
        bind_group_layouts: &[layout],
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("game textured pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[SpriteVertex::LAYOUT],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(blend),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

/// Build the textured-sprite pipeline + its texture/sampler bind-group layout.
fn build_sprite_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout) {
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("game sprite layout"),
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

    let pipeline = textured_pipeline(
        device,
        format,
        &layout,
        SPRITE_SHADER_SOURCE,
        wgpu::BlendState::ALPHA_BLENDING,
    );
    (pipeline, layout)
}

fn build_masked_sprite_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout) {
    let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("game masked sprite layout"),
        entries: &[
            texture_entry(0),
            texture_entry(1),
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let pipeline = textured_pipeline(
        device,
        format,
        &layout,
        MASKED_SPRITE_SHADER_SOURCE,
        wgpu::BlendState::ALPHA_BLENDING,
    );
    (pipeline, layout)
}

/// Cache key of an uploaded sprite texture. Raster sheets are one texture per
/// URL; an SVG is one texture per (URL, rasterized size).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct SpriteKey {
    url: String,
    raster: Option<(u32, u32)>,
}

/// An uploaded sprite texture with its bind group. `width`/`height` are the
/// sheet's coordinate space (the one `src` rects and UVs use): the pixel size
/// for a raster sheet, the intrinsic size for an SVG whose texture is larger
/// or smaller.
struct GpuSprite {
    width: u32,
    height: u32,
    bind_group: wgpu::BindGroup,
    /// Value of `SharedDeviceRenderer::use_clock` at the last draw request;
    /// orders eviction (smallest = least recently used).
    last_used: u64,
}

/// One resolved draw of a frame, in z-order.
enum Draw {
    Quads(Vec<Vertex>),
    Particles(crate::particles::ParticleDraw),
    Sprite {
        key: SpriteKey,
        verts: Vec<SpriteVertex>,
        additive: bool,
    },
    MaskedSprite {
        bind_group: wgpu::BindGroup,
        verts: [SpriteVertex; 6],
    },
    /// Shapes of one SDF pipeline in order, before adjacent runs are merged into uniform-sized batches.
    SdfRun {
        key: u64,
        wgsl: Option<String>,
        instances: Vec<crate::sdf::SdfInstance>,
        target: Option<crate::render_target::TargetTexture>,
    },
    Sdf {
        key: u64,
        bind_group: wgpu::BindGroup,
        aux: wgpu::BindGroup,
        count: u32,
    },
    /// A backdrop shape: ends the render pass, blurs the frame so far, then paints the shape over it.
    Backdrop(Box<BackdropDraw>),
    /// A render target painted as a sprite (premultiplied texture).
    TargetSprite {
        bind_group: wgpu::BindGroup,
        verts: [SpriteVertex; 6],
    },
    /// Glyph quads of one atlas page, before adjacent runs of the page are merged.
    GlyphRun {
        page: u64,
        instances: Vec<crate::glyph_atlas::GlyphInstance>,
    },
    Glyphs {
        page: u64,
        buffer: wgpu::Buffer,
        count: u32,
    },
}

/// One backdrop shape resolved for the pass: the blur job and the bind groups of its three draws.
struct BackdropDraw {
    job: crate::backdrop::BackdropJob,
    key: u64,
    bind_group: wgpu::BindGroup,
    aux: wgpu::BindGroup,
}

/// A run of SDF shapes of one pipeline and one target, waiting to be cut into uniform-sized batches.
struct PendingSdf {
    key: u64,
    wgsl: Option<String>,
    instances: Vec<crate::sdf::SdfInstance>,
    target: Option<crate::render_target::TargetTexture>,
}

/// Cache-key prefix of vector layer textures (never a fetchable URL).
const VECTOR_LAYER_PREFIX: &str = "vector-layer:";

/// Vector layer textures kept across frames; a packet re-submitted unchanged
/// reuses its layer, older layers are evicted least recently used first.
const MAX_VECTOR_LAYERS: usize = 8;

/// Rasterized textures kept per SVG URL. Raster scales are bucketed to
/// quarter-octaves ([`crate::svg::raster_scale`]), so four sizes span one
/// octave of zoom; older sizes are evicted so a long session across many
/// zoom levels holds bounded GPU memory.
const MAX_SVG_RASTERS_PER_URL: usize = 4;

/// Renderer for hosts that own the wgpu device (e.g. dioxus-native/Blitz
/// custom paint): same display-list pipeline as [`Canvas`], but built on
/// the host's shared device/queue and drawing into a caller-provided texture
/// instead of an owned surface or a readback buffer. Handles the full 2D
/// command set: `Clear` + `Quad` through the vertex-colored pipeline and
/// `Sprite` through the textured pipeline (assets arrive via
/// [`crate::asset`]; not-yet-loaded sprites are skipped — React parity).
pub struct SharedDeviceRenderer {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    pipeline: wgpu::RenderPipeline,
    sprite_pipeline: wgpu::RenderPipeline,
    additive_sprite_pipeline: wgpu::RenderPipeline,
    sprite_layout: wgpu::BindGroupLayout,
    masked_sprite_pipeline: wgpu::RenderPipeline,
    masked_sprite_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    sprites: std::collections::HashMap<SpriteKey, GpuSprite>,
    /// Bumped on every sprite request; stamps `GpuSprite::last_used`.
    use_clock: u64,
    /// `use_clock` when the current frame began: sprites stamped after it are
    /// referenced by this frame's draw list and must not be evicted.
    frame_epoch: u64,
    render_depth: usize,
    scene3d: Option<crate::scene3d::Renderer3D>,
    /// Why the last packet's 3D scene was not drawn; a host with no log sink reports it ([`Self::take_scene3d_error`]).
    scene3d_error: Option<String>,
    format: wgpu::TextureFormat,
    particles: Option<crate::particles::ParticlePass>,
    sdf: Option<crate::sdf::SdfPass>,
    glyphs: Option<crate::glyph_atlas::GlyphPass>,
    layers: crate::backdrop::LayerPool,
    blur: Option<crate::backdrop::BlurPass>,
    targets: crate::render_target::TargetStore,
    target_sprite_pipeline: wgpu::RenderPipeline,
}

impl SharedDeviceRenderer {
    /// Build the game pipelines on the host's device for `format`
    /// (the texture format the host will hand to `render_packet_to_texture`).
    pub fn new(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        format: wgpu::TextureFormat,
    ) -> Self {
        let pipeline = build_pipeline(&device, format);
        let (sprite_pipeline, sprite_layout) = build_sprite_pipeline(&device, format);
        let additive_sprite_pipeline = textured_pipeline(
            &device,
            format,
            &sprite_layout,
            SPRITE_SHADER_SOURCE,
            wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::SrcAlpha,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
            },
        );
        let (masked_sprite_pipeline, masked_sprite_layout) =
            build_masked_sprite_pipeline(&device, format);
        let target_sprite_pipeline = textured_pipeline(
            &device,
            format,
            &sprite_layout,
            TARGET_SPRITE_SHADER_SOURCE,
            wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("game sprite sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        tracing::info!(target: "almadar::game_renderer", ?format, "shared-device renderer created");
        Self {
            device,
            queue,
            pipeline,
            sprite_pipeline,
            additive_sprite_pipeline,
            sprite_layout,
            masked_sprite_pipeline,
            masked_sprite_layout,
            sampler,
            sprites: std::collections::HashMap::new(),
            use_clock: 0,
            frame_epoch: 0,
            render_depth: 0,
            scene3d_error: None,
            scene3d: None,
            format,
            particles: None,
            sdf: None,
            glyphs: None,
            layers: Default::default(),
            blur: None,
            targets: Default::default(),
            target_sprite_pipeline,
        }
    }

    /// The host device this renderer draws with (target textures must be
    /// created on it).
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// The host queue this renderer submits to.
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Upload a sprite to the GPU once its bytes have arrived (decode here, at
    /// upload time - the byte cache stays format-agnostic). Raster sheets are
    /// uploaded once per URL at their own size. SVGs have no pixel size, so
    /// they are rasterized at `target_scale` device pixels per intrinsic unit
    /// (bucketed, see [`crate::svg::raster_scale`]) and cached per
    /// (URL, texture size); UVs still index the intrinsic size, so `src`
    /// rects mean the same thing for both kinds. Returns the cached entry
    /// when ready; `None` while in-flight or on failure.
    fn ensure_sprite(&mut self, url: &str, target_scale: f32) -> Option<(SpriteKey, &GpuSprite)> {
        let bytes = crate::asset::asset_bytes(url)?;
        self.use_clock += 1;
        let now = self.use_clock;
        let (key, rgba, sheet, tex) = match crate::asset::asset_kind(url)? {
            crate::asset::AssetKind::Raster => {
                let key = SpriteKey {
                    url: url.to_string(),
                    raster: None,
                };
                if let Some(sprite) = self.sprites.get_mut(&key) {
                    sprite.last_used = now;
                    return self.sprites.get(&key).map(|s| (key, s));
                }
                let decoded = match image::ImageReader::new(std::io::Cursor::new(&bytes))
                    .with_guessed_format()
                    .map_err(|e| e.to_string())
                    .and_then(|r| r.decode().map_err(|e| e.to_string()))
                {
                    Ok(img) => img.into_rgba8(),
                    Err(e) => {
                        tracing::warn!(target: "almadar::game_renderer", url, error = %e, "sprite decode failed");
                        return None;
                    }
                };
                let dims = decoded.dimensions();
                (key, decoded.into_raw(), dims, dims)
            }
            crate::asset::AssetKind::Svg => {
                let sheet = crate::svg::intrinsic_size(url, &bytes)?;
                let tex = crate::svg::raster_size(
                    sheet,
                    crate::svg::raster_scale(target_scale),
                    self.device.limits().max_texture_dimension_2d,
                );
                let key = SpriteKey {
                    url: url.to_string(),
                    raster: Some(tex),
                };
                if let Some(sprite) = self.sprites.get_mut(&key) {
                    sprite.last_used = now;
                    return self.sprites.get(&key).map(|s| (key, s));
                }
                let raster = crate::svg::rasterize(url, &bytes, tex.0, tex.1)?;
                (key, raster.rgba, sheet, tex)
            }
        };
        let (width, height) = tex;
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("game sprite sheet"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("game sprite bind group"),
            layout: &self.sprite_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        tracing::info!(target: "almadar::game_renderer", url, width, height, sheet_w = sheet.0, sheet_h = sheet.1, "sprite uploaded");
        if key.raster.is_some() {
            self.evict_stale_svg_rasters(url);
        }
        self.sprites.insert(
            key.clone(),
            GpuSprite {
                width: sheet.0,
                height: sheet.1,
                bind_group,
                last_used: now,
            },
        );
        self.sprites.get(&key).map(|s| (key, s))
    }

    /// Rasterize a run of consecutive vector commands (paths, lines, text)
    /// into one target-sized texture and queue it as a full-surface sprite.
    /// The layer is cached by the run's content and target size.
    fn flush_vector_run(
        &mut self,
        run: &mut Vec<&DrawCommand>,
        packet: &GameRenderPacket,
        target: &wgpu::Texture,
        draws: &mut Vec<Draw>,
    ) {
        self.flush_vector_run_with_blend(run, packet, target, draws, false);
    }

    fn flush_vector_run_with_blend(
        &mut self,
        run: &mut Vec<&DrawCommand>,
        packet: &GameRenderPacket,
        target: &wgpu::Texture,
        draws: &mut Vec<Draw>,
        additive: bool,
    ) {
        if run.is_empty() {
            return;
        }
        let commands = std::mem::take(run);
        let (tw, th) = (target.width(), target.height());
        if tw > self.device.limits().max_texture_dimension_2d
            || th > self.device.limits().max_texture_dimension_2d
        {
            tracing::warn!(target: "almadar::game_renderer", tw, th, "vector layer exceeds the device texture limit — skipped");
            return;
        }
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        if commands
            .iter()
            .any(|command| crate::vector::uses_text(command))
        {
            std::hash::Hash::hash(&crate::vector::font_generation(), &mut hasher);
        }
        if commands
            .iter()
            .any(|command| crate::vector::uses_image_pattern(command))
        {
            std::hash::Hash::hash(&crate::asset::generation(), &mut hasher);
        }
        for command in &commands {
            match serde_json::to_string(command) {
                Ok(json) => std::hash::Hash::hash(&json, &mut hasher),
                Err(err) => {
                    tracing::warn!(target: "almadar::game_renderer", error = %err, "vector command not serializable — skipped");
                    return;
                }
            }
        }
        let key = SpriteKey {
            url: format!(
                "{VECTOR_LAYER_PREFIX}{:016x}",
                std::hash::Hasher::finish(&hasher)
            ),
            raster: Some((tw, th)),
        };
        self.use_clock += 1;
        let now = self.use_clock;
        if let Some(sprite) = self.sprites.get_mut(&key) {
            sprite.last_used = now;
        } else {
            let sx = tw as f32 / packet.width.max(1.0);
            let sy = th as f32 / packet.height.max(1.0);
            let Some(rgba) = crate::vector::rasterize(&commands, tw, th, sx, sy) else {
                tracing::warn!(target: "almadar::game_renderer", tw, th, "vector layer surface not allocatable — skipped");
                return;
            };
            self.evict_stale_vector_layers();
            let sprite = self.upload_rgba(&rgba, tw, th, now);
            self.sprites.insert(key.clone(), sprite);
        }
        let width = packet.width.max(1.0) as u32;
        let height = packet.height.max(1.0) as u32;
        let verts = sprite_vertices(
            &Rect {
                x: 0.0,
                y: 0.0,
                w: tw as f32,
                h: th as f32,
            },
            &Rect {
                x: 0.0,
                y: 0.0,
                w: width as f32,
                h: height as f32,
            },
            tw,
            th,
            0.0,
            1.0,
            false,
            false,
            width,
            height,
        );
        draws.push(Draw::Sprite {
            key,
            verts: verts.to_vec(),
            additive,
        });
    }

    fn evict_stale_vector_layers(&mut self) {
        loop {
            let held = self
                .sprites
                .keys()
                .filter(|k| k.url.starts_with(VECTOR_LAYER_PREFIX))
                .count();
            if held < MAX_VECTOR_LAYERS {
                return;
            }
            let victim = self
                .sprites
                .iter()
                .filter(|(k, s)| {
                    k.url.starts_with(VECTOR_LAYER_PREFIX) && s.last_used <= self.frame_epoch
                })
                .min_by_key(|(_, s)| s.last_used)
                .map(|(k, _)| k.clone());
            match victim {
                Some(k) => {
                    self.sprites.remove(&k);
                }
                None => return,
            }
        }
    }

    /// Vector layer textures currently cached (cache observability for tests).
    #[cfg(test)]
    fn vector_layer_count(&self) -> usize {
        self.sprites
            .keys()
            .filter(|k| k.url.starts_with(VECTOR_LAYER_PREFIX))
            .count()
    }

    fn upload_rgba_texture(&self, rgba: &[u8], width: u32, height: u32) -> wgpu::Texture {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("game vector layer"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
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
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        texture
    }

    /// Upload straight-alpha RGBA8 as a sampled texture with its bind group.
    fn upload_rgba(&self, rgba: &[u8], width: u32, height: u32, now: u64) -> GpuSprite {
        let texture = self.upload_rgba_texture(rgba, width, height);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("game vector layer bind group"),
            layout: &self.sprite_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        GpuSprite {
            width,
            height,
            bind_group,
            last_used: now,
        }
    }

    /// Make room for one more raster of `url`: while the URL holds
    /// `MAX_SVG_RASTERS_PER_URL` textures, drop the least recently used one
    /// that the in-progress frame has not already requested.
    fn evict_stale_svg_rasters(&mut self, url: &str) {
        loop {
            let held = self
                .sprites
                .keys()
                .filter(|k| k.raster.is_some() && k.url == url)
                .count();
            if held < MAX_SVG_RASTERS_PER_URL {
                return;
            }
            let victim = self
                .sprites
                .iter()
                .filter(|(k, s)| {
                    k.raster.is_some() && k.url == url && s.last_used <= self.frame_epoch
                })
                .min_by_key(|(_, s)| s.last_used)
                .map(|(k, _)| k.clone());
            match victim {
                Some(k) => {
                    self.sprites.remove(&k);
                }
                None => return,
            }
        }
    }

    /// Rasterized SVG texture sizes currently cached (cache observability for tests).
    #[cfg(test)]
    fn svg_texture_sizes(&self) -> Vec<(u32, u32)> {
        let mut sizes: Vec<_> = self.sprites.keys().filter_map(|k| k.raster).collect();
        sizes.sort_unstable();
        sizes
    }

    /// Render one packet into `target`. The texture must be created on the
    /// same device with `RENDER_ATTACHMENT` usage and the format passed to
    /// [`Self::new`]. Draw commands execute in packet order (z-order); quads
    /// batch between sprites. Returns `false` when nothing drawable was
    /// emitted (the target is still cleared).
    fn masked_group_draw(
        &mut self,
        packet: &GameRenderPacket,
        target: &wgpu::Texture,
        clip: &[crate::packet::PathSegment],
        commands: &[DrawCommand],
    ) -> Option<Draw> {
        let (tw, th) = (target.width(), target.height());
        if tw > self.device.limits().max_texture_dimension_2d
            || th > self.device.limits().max_texture_dimension_2d
        {
            return None;
        }
        let sx = tw as f32 / packet.width.max(1.0);
        let sy = th as f32 / packet.height.max(1.0);
        let mask = crate::vector::rasterize_clip_mask(clip, tw, th, sx, sy)?;
        let layer = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("game clipped group surface"),
            size: wgpu::Extent3d {
                width: tw,
                height: th,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: target.format(),
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let child = GameRenderPacket {
            pattern: packet.pattern.clone(),
            width: packet.width,
            height: packet.height,
            draw_commands: commands.to_vec(),
            time_ms: packet.time_ms,
            pointer: packet.pointer,
            ..GameRenderPacket::default()
        };
        if !self.render_packet_to_texture(&child, &layer) {
            return None;
        }
        let mask_texture = self.upload_rgba_texture(&mask, tw, th);
        let layer_view = layer.create_view(&wgpu::TextureViewDescriptor::default());
        let mask_view = mask_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("game clipped group bind group"),
            layout: &self.masked_sprite_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&layer_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&mask_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let width = packet.width.max(1.0) as u32;
        let height = packet.height.max(1.0) as u32;
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: tw as f32,
            h: th as f32,
        };
        let dst = Rect {
            x: 0.0,
            y: 0.0,
            w: width as f32,
            h: height as f32,
        };
        let verts = sprite_vertices(&rect, &dst, tw, th, 0.0, 1.0, false, false, width, height);
        Some(Draw::MaskedSprite { bind_group, verts })
    }

    /// Merge adjacent SDF runs of one pipeline and one target (nothing is drawn between them, so z-order holds),
    /// compile each pipeline once, and bind each batch of up to [`crate::sdf::MAX_INSTANCES`] shapes.
    fn finalize_sdf(&mut self, draws: Vec<Draw>, packet: &GameRenderPacket) -> Vec<Draw> {
        let mut out: Vec<Draw> = Vec::with_capacity(draws.len());
        let mut pending: Option<PendingSdf> = None;
        for draw in draws {
            match draw {
                Draw::SdfRun {
                    key,
                    wgsl,
                    mut instances,
                    target,
                } => match pending.take() {
                    Some(mut run)
                        if run.key == key
                            && run.target.as_ref().map(|t| t.serial)
                                == target.as_ref().map(|t| t.serial) =>
                    {
                        run.instances.append(&mut instances);
                        pending = Some(run);
                    }
                    other => {
                        if let Some(run) = other {
                            self.bind_sdf(run, packet, &mut out);
                        }
                        pending = Some(PendingSdf {
                            key,
                            wgsl,
                            instances,
                            target,
                        });
                    }
                },
                other => {
                    if let Some(run) = pending.take() {
                        self.bind_sdf(run, packet, &mut out);
                    }
                    out.push(other);
                }
            }
        }
        if let Some(run) = pending {
            self.bind_sdf(run, packet, &mut out);
        }
        out
    }

    fn bind_sdf(&mut self, run: PendingSdf, packet: &GameRenderPacket, out: &mut Vec<Draw>) {
        let (device, format) = (self.device.clone(), self.format);
        let pass = self
            .sdf
            .get_or_insert_with(|| crate::sdf::SdfPass::new(&device, format));
        if let Err(error) = pass.ensure(&device, run.key, run.wgsl.as_deref()) {
            tracing::error!(target: "almadar::game_renderer", %error, "sdf material not drawn");
            return;
        }
        let aux = pass.aux_group(&device, None, run.target.as_ref().map(|t| &t.view));
        for chunk in crate::sdf::batches(&run.instances) {
            let bind_group = pass.bind_group(
                &device,
                chunk,
                (packet.width, packet.height),
                packet.time_ms,
                packet.pointer,
            );
            out.push(Draw::Sdf {
                key: run.key,
                bind_group,
                aux: aux.clone(),
                count: chunk.len() as u32,
            });
        }
    }

    /// The first render target a material names, as the reader resolving now sees it.
    fn material_target(
        &self,
        material: Option<&crate::packet::SdfMaterial>,
    ) -> Option<crate::render_target::TargetTexture> {
        material
            .and_then(|m| m.targets.first())
            .and_then(|id| self.targets.read(id))
    }

    /// Resolve a backdrop shape into its draw; `None` when it is off the frame or its material does not compile.
    fn backdrop_draw(
        &mut self,
        shape: &DrawCommand,
        backdrop: &crate::packet::SdfBackdrop,
        packet: &GameRenderPacket,
        set: &crate::backdrop::LayerSet,
    ) -> Option<Draw> {
        let DrawCommand::Sdf {
            rect, rotation, ..
        } = shape
        else {
            return None;
        };
        let (instance, material) = crate::sdf::SdfInstance::from_command(shape)?;
        let scale = (
            set.frame.width() as f32 / packet.width.max(1.0),
            set.frame.height() as f32 / packet.height.max(1.0),
        );
        let region = crate::backdrop::region(
            *rect,
            *rotation,
            2.0,
            scale,
            (set.frame.width(), set.frame.height()),
        )?;
        let wgsl = material.map(|m| m.wgsl.clone());
        let key = crate::sdf::pipeline_key(wgsl.as_deref());
        let target = self.material_target(material);
        let (device, format) = (self.device.clone(), self.format);
        self.blur
            .get_or_insert_with(|| crate::backdrop::BlurPass::new(&device, format));
        let pass = self
            .sdf
            .get_or_insert_with(|| crate::sdf::SdfPass::new(&device, format));
        if let Err(error) = pass.ensure(&device, key, wgsl.as_deref()) {
            tracing::error!(target: "almadar::game_renderer", %error, "sdf material not drawn");
            return None;
        }
        pass.ensure_backdrop(&device);
        let bind_group = pass.bind_group(
            &device,
            &[instance],
            (packet.width, packet.height),
            packet.time_ms,
            packet.pointer,
        );
        let behind = set.b.create_view(&wgpu::TextureViewDescriptor::default());
        let aux = pass.aux_group(&device, Some(&behind), target.as_ref().map(|t| &t.view));
        Some(Draw::Backdrop(Box::new(BackdropDraw {
            job: crate::backdrop::BackdropJob {
                frame: set.frame.clone(),
                a: set.a.clone(),
                b: set.b.clone(),
                sigma: backdrop.blur * 0.5 * scale.0,
                saturate: backdrop.saturate,
                tint: backdrop.tint,
                region,
            },
            key,
            bind_group,
            aux,
        })))
    }

    /// Blur the frame under a backdrop shape, then clear the frame where it covers, add the blur there and paint
    /// the shape over it.
    fn encode_backdrop(&self, encoder: &mut wgpu::CommandEncoder, draw: &BackdropDraw) {
        let Some(blur) = self.blur.as_ref() else {
            return;
        };
        let Some(sdf) = self.sdf.as_ref() else {
            return;
        };
        let (Some((cut, add)), Some(fill)) = (sdf.backdrop_pipelines(), sdf.pipeline(draw.key)) else {
            return;
        };
        blur.encode(&self.device, encoder, &draw.job);
        let view = draw
            .job
            .frame
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("backdrop shape pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            occlusion_query_set: None,
            timestamp_writes: None,
        });
        for pipeline in [cut, add, fill] {
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &draw.bind_group, &[]);
            pass.set_bind_group(1, &draw.aux, &[]);
            pass.draw(0..6, 0..1);
        }
    }

    /// Paint a `Target` command's children into its texture unless they are what it already holds.
    fn paint_target(
        &mut self,
        id: &str,
        size: (u32, u32),
        live: bool,
        commands: &[DrawCommand],
        packet: &GameRenderPacket,
    ) {
        let limit = self.device.limits().max_texture_dimension_2d;
        if size.0 > limit || size.1 > limit {
            tracing::warn!(target: "almadar::game_renderer", id, ?size, "render target exceeds the device texture limit; skipped");
            return;
        }
        let versions = |read: &str| self.targets.version(read);
        let Some(hash) =
            crate::render_target::content_hash(size.0, size.1, live, commands, packet.time_ms, &versions)
        else {
            tracing::warn!(target: "almadar::game_renderer", id, "render target commands not serializable; skipped");
            return;
        };
        if self.targets.keep(id, size, hash) {
            return;
        }
        let slot = self.targets.writer(&self.device, self.format, id, size);
        let child = GameRenderPacket {
            pattern: packet.pattern.clone(),
            width: size.0 as f32,
            height: size.1 as f32,
            draw_commands: commands.to_vec(),
            time_ms: packet.time_ms,
            ..GameRenderPacket::default()
        };
        self.render_packet_to_texture(&child, &slot.texture);
        self.targets.painted(id, hash);
    }

    /// The atlas draws for a plain `Text`; `None` sends it to the CPU vector layer.
    fn atlas_text(
        &mut self,
        command: &DrawCommand,
        packet: &GameRenderPacket,
        target: &wgpu::Texture,
    ) -> Option<Vec<Draw>> {
        let (device, queue, format) = (self.device.clone(), self.queue.clone(), self.format);
        let (tw, th) = (target.width(), target.height());
        let scale = (
            tw as f32 / packet.width.max(1.0),
            th as f32 / packet.height.max(1.0),
        );
        let pass = self
            .glyphs
            .get_or_insert_with(|| crate::glyph_atlas::GlyphPass::new(&device, format));
        let runs = pass.place_text(&device, &queue, command, scale, (tw, th))?;
        Some(
            runs.into_iter()
                .map(|(page, instances)| Draw::GlyphRun { page, instances })
                .collect(),
        )
    }

    /// Merge adjacent glyph runs of one page into one instance buffer each.
    fn finalize_glyphs(&self, draws: Vec<Draw>) -> Vec<Draw> {
        let mut out: Vec<Draw> = Vec::with_capacity(draws.len());
        let mut pending: Option<(u64, Vec<crate::glyph_atlas::GlyphInstance>)> = None;
        let flush = |pending: Option<(u64, Vec<crate::glyph_atlas::GlyphInstance>)>,
                     out: &mut Vec<Draw>| {
            if let Some((page, instances)) = pending {
                out.push(Draw::Glyphs {
                    page,
                    buffer: crate::glyph_atlas::GlyphPass::instance_buffer(&self.device, &instances),
                    count: instances.len() as u32,
                });
            }
        };
        for draw in draws {
            match draw {
                Draw::GlyphRun {
                    page,
                    mut instances,
                } => match pending.take() {
                    Some((p, mut list)) if p == page => {
                        list.append(&mut instances);
                        pending = Some((p, list));
                    }
                    other => {
                        flush(other, &mut out);
                        pending = Some((page, instances));
                    }
                },
                other => {
                    flush(pending.take(), &mut out);
                    out.push(other);
                }
            }
        }
        flush(pending, &mut out);
        out
    }

    /// The error that kept the last packet's 3D scene from drawing, once.
    pub fn take_scene3d_error(&mut self) -> Option<String> {
        self.scene3d_error.take()
    }

    pub fn render_packet_to_texture(
        &mut self,
        packet: &GameRenderPacket,
        target: &wgpu::Texture,
    ) -> bool {
        if self.render_depth == 0 {
            self.frame_epoch = self.use_clock;
            if let Some(glyphs) = self.glyphs.as_mut() {
                glyphs.begin_frame();
            }
            self.targets.begin_frame();
        }
        self.render_depth += 1;
        let final_target = target;
        let layer_set = packet
            .draw_commands
            .iter()
            .any(|c| matches!(c, DrawCommand::SdfBackdrop { .. }))
            .then(|| {
                self.layers
                    .get(&self.device, (target.width(), target.height()), self.format)
            });
        let frame_texture = layer_set.as_ref().map(|set| set.frame.clone());
        let target = frame_texture.as_ref().unwrap_or(final_target);
        let width = packet.width.max(1.0) as u32;
        let height = packet.height.max(1.0) as u32;
        let density = target.width() as f32 / packet.width.max(1.0);
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("game shared-device pass"),
            });

        let mut clear_color = wgpu::Color::TRANSPARENT;
        let mut drew = false;
        let mut load_3d = false;
        for command in &packet.draw_commands {
            if let DrawCommand::Scene3D { scene } = command {
                let renderer = self.scene3d.get_or_insert_with(|| {
                    crate::scene3d::Renderer3D::new(self.device.clone(), self.queue.clone())
                });
                match renderer.encode(&mut encoder, scene, target, 4) {
                    Ok(()) => {
                        load_3d = true;
                        drew = true;
                        self.scene3d_error = None;
                    }
                    Err(error) => {
                        tracing::error!(target: "almadar::game_renderer", %error, "3D scene not drawn");
                        self.scene3d_error = Some(error.to_string());
                    }
                }
            }
        }

        // Resolve every command into a sequenced draw list FIRST (GPU uploads
        // borrow &mut self; the pass borrows &self). Quads coalesce into one
        // vertex run between sprites; vector commands coalesce into one
        // CPU-rasterized layer sprite between quads and sprites.
        let mut draws: Vec<Draw> = Vec::new();
        let mut quad_batch: Vec<Vertex> = Vec::new();
        let mut vector_run: Vec<&DrawCommand> = Vec::new();

        for command in &packet.draw_commands {
            match command {
                DrawCommand::Clear { color } => {
                    clear_color = wgpu::Color {
                        r: color[0] as f64,
                        g: color[1] as f64,
                        b: color[2] as f64,
                        a: color[3] as f64,
                    };
                }
                DrawCommand::Quad { rect, color } => {
                    self.flush_vector_run(&mut vector_run, packet, target, &mut draws);
                    push_quad(&mut quad_batch, rect, color, width, height);
                }
                DrawCommand::Particles {
                    source,
                    stride,
                    radius,
                    palette,
                    scale,
                    origin,
                } => {
                    self.flush_vector_run(&mut vector_run, packet, target, &mut draws);
                    if !quad_batch.is_empty() {
                        draws.push(Draw::Quads(std::mem::take(&mut quad_batch)));
                    }
                    let (device, format) = (self.device.clone(), self.format);
                    let pass = self.particles.get_or_insert_with(|| {
                        crate::particles::ParticlePass::new(&device, format)
                    });
                    match pass.prepare(
                        &device,
                        source,
                        *stride,
                        *radius,
                        palette,
                        *scale,
                        *origin,
                        (width as f32, height as f32),
                    ) {
                        Ok(prepared) => draws.push(Draw::Particles(prepared)),
                        Err(error) => {
                            tracing::error!(target: "almadar::game_renderer", %error, "particles not drawn")
                        }
                    }
                }
                DrawCommand::Poly { points, color } => {
                    self.flush_vector_run(&mut vector_run, packet, target, &mut draws);
                    push_poly_fan(&mut quad_batch, points, color, width, height);
                }
                DrawCommand::Sdf { .. } => {
                    self.flush_vector_run(&mut vector_run, packet, target, &mut draws);
                    if !quad_batch.is_empty() {
                        draws.push(Draw::Quads(std::mem::take(&mut quad_batch)));
                    }
                    if let Some((instance, material)) =
                        crate::sdf::SdfInstance::from_command(command)
                    {
                        let wgsl = material.map(|m| m.wgsl.clone());
                        draws.push(Draw::SdfRun {
                            key: crate::sdf::pipeline_key(wgsl.as_deref()),
                            wgsl,
                            instances: vec![instance],
                            target: self.material_target(material),
                        });
                    }
                }
                DrawCommand::SdfBlend { .. } => {
                    self.flush_vector_run(&mut vector_run, packet, target, &mut draws);
                    if !quad_batch.is_empty() {
                        draws.push(Draw::Quads(std::mem::take(&mut quad_batch)));
                    }
                    if let Some((pair, material)) = crate::sdf::SdfInstance::from_blend(command) {
                        let wgsl = material.map(|m| m.wgsl.clone());
                        draws.push(Draw::SdfRun {
                            key: crate::sdf::pipeline_key(wgsl.as_deref()),
                            wgsl,
                            instances: pair.to_vec(),
                            target: self.material_target(material),
                        });
                    }
                }
                DrawCommand::SdfBackdrop { shape, backdrop } => {
                    self.flush_vector_run(&mut vector_run, packet, target, &mut draws);
                    if !quad_batch.is_empty() {
                        draws.push(Draw::Quads(std::mem::take(&mut quad_batch)));
                    }
                    if let Some(set) = layer_set.as_ref() {
                        if let Some(draw) = self.backdrop_draw(shape, backdrop, packet, set) {
                            draws.push(draw);
                        }
                    }
                }
                DrawCommand::Target {
                    id,
                    width: target_w,
                    height: target_h,
                    live,
                    commands,
                } => {
                    self.paint_target(id, (*target_w, *target_h), *live, commands, packet);
                }
                DrawCommand::Text { halo: None, .. } => {
                    self.flush_vector_run(&mut vector_run, packet, target, &mut draws);
                    if !quad_batch.is_empty() {
                        draws.push(Draw::Quads(std::mem::take(&mut quad_batch)));
                    }
                    match self.atlas_text(command, packet, target) {
                        Some(runs) => draws.extend(runs),
                        None => vector_run.push(command),
                    }
                }
                DrawCommand::Path { .. }
                | DrawCommand::PatternPath { .. }
                | DrawCommand::GradientPath { .. }
                | DrawCommand::Blurred { .. }
                | DrawCommand::Shadow { .. }
                | DrawCommand::ClippedPath { .. }
                | DrawCommand::Line { .. }
                | DrawCommand::Text { .. } => {
                    if !quad_batch.is_empty() {
                        draws.push(Draw::Quads(std::mem::take(&mut quad_batch)));
                    }
                    vector_run.push(command);
                }
                DrawCommand::AdditiveGroup { commands } => {
                    self.flush_vector_run(&mut vector_run, packet, target, &mut draws);
                    if !quad_batch.is_empty() {
                        draws.push(Draw::Quads(std::mem::take(&mut quad_batch)));
                    }
                    let mut layer: Vec<&DrawCommand> = commands.iter().collect();
                    self.flush_vector_run_with_blend(&mut layer, packet, target, &mut draws, true);
                }
                DrawCommand::ClipGroup { clip, commands } => {
                    self.flush_vector_run(&mut vector_run, packet, target, &mut draws);
                    if !quad_batch.is_empty() {
                        draws.push(Draw::Quads(std::mem::take(&mut quad_batch)));
                    }
                    if let Some(draw) = self.masked_group_draw(packet, target, clip, commands) {
                        draws.push(draw);
                    }
                }
                DrawCommand::TexturedTriangle {
                    texture,
                    points,
                    uv,
                    uv_in_pixels,
                    opacity,
                } => {
                    self.flush_vector_run(&mut vector_run, packet, target, &mut draws);
                    let Some((sheet_w, sheet_h)) = crate::asset::image_info(texture) else {
                        continue;
                    };
                    let pixel_uv = uv.map(|coords| {
                        if *uv_in_pixels {
                            coords
                        } else {
                            [coords[0] * sheet_w as f32, coords[1] * sheet_h as f32]
                        }
                    });
                    let area = (pixel_uv[1][0] - pixel_uv[0][0])
                        * (pixel_uv[2][1] - pixel_uv[0][1])
                        - (pixel_uv[1][1] - pixel_uv[0][1]) * (pixel_uv[2][0] - pixel_uv[0][0]);
                    if area.abs() < f32::EPSILON {
                        continue;
                    }
                    let mut target_scale = density;
                    for i in 0..3 {
                        let j = (i + 1) % 3;
                        let dst = (points[j].x - points[i].x).hypot(points[j].y - points[i].y);
                        let src = (pixel_uv[j][0] - pixel_uv[i][0])
                            .hypot(pixel_uv[j][1] - pixel_uv[i][1]);
                        if src > f32::EPSILON {
                            target_scale = target_scale.max(dst * density / src);
                        }
                    }
                    let Some((key, sprite)) = self.ensure_sprite(texture, target_scale) else {
                        continue;
                    };
                    if !quad_batch.is_empty() {
                        draws.push(Draw::Quads(std::mem::take(&mut quad_batch)));
                    }
                    let verts = textured_triangle_vertices(
                        *points,
                        *uv,
                        *uv_in_pixels,
                        sprite.width,
                        sprite.height,
                        *opacity,
                        width,
                        height,
                    );
                    draws.push(Draw::Sprite {
                        key,
                        verts: verts.to_vec(),
                        additive: false,
                    });
                }
                DrawCommand::Sprite {
                    texture,
                    src,
                    dst,
                    rotation,
                    opacity,
                    flip_x,
                    flip_y,
                } => {
                    self.flush_vector_run(&mut vector_run, packet, target, &mut draws);
                    if let Some(id) = texture.strip_prefix(crate::render_target::TARGET_PREFIX) {
                        let Some(read) = self.targets.read(id) else {
                            continue;
                        };
                        if !quad_batch.is_empty() {
                            draws.push(Draw::Quads(std::mem::take(&mut quad_batch)));
                        }
                        let whole = Rect {
                            x: 0.0,
                            y: 0.0,
                            w: read.width as f32,
                            h: read.height as f32,
                        };
                        let from = if src.w > 0.0 && src.h > 0.0 { *src } else { whole };
                        let verts = sprite_vertices(
                            &from, dst, read.width, read.height, *rotation, *opacity, *flip_x, *flip_y, width,
                            height,
                        );
                        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("render target sprite bind group"),
                            layout: &self.sprite_layout,
                            entries: &[
                                wgpu::BindGroupEntry {
                                    binding: 0,
                                    resource: wgpu::BindingResource::TextureView(&read.view),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 1,
                                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                                },
                            ],
                        });
                        draws.push(Draw::TargetSprite { bind_group, verts });
                        continue;
                    }
                    // Device pixels per sheet unit: the packet is laid out in
                    // logical units, the target is physical.
                    let target_scale = (dst.w / src.w.max(f32::EPSILON))
                        .max(dst.h / src.h.max(f32::EPSILON))
                        * density;
                    let Some((key, sprite)) = self.ensure_sprite(texture, target_scale) else {
                        continue; // asset in-flight/failed — skip, React parity
                    };
                    if !quad_batch.is_empty() {
                        draws.push(Draw::Quads(std::mem::take(&mut quad_batch)));
                    }
                    let verts = sprite_vertices(
                        src,
                        dst,
                        sprite.width,
                        sprite.height,
                        *rotation,
                        *opacity,
                        *flip_x,
                        *flip_y,
                        width,
                        height,
                    );
                    draws.push(Draw::Sprite {
                        key,
                        verts: verts.to_vec(),
                        additive: false,
                    });
                }
                _ => {}
            }
        }
        self.flush_vector_run(&mut vector_run, packet, target, &mut draws);
        if !quad_batch.is_empty() {
            draws.push(Draw::Quads(quad_batch));
        }
        let draws = self.finalize_sdf(draws, packet);
        let draws = self.finalize_glyphs(draws);

        let mut segment_start = 0;
        let mut first_segment = true;
        loop {
            let segment_end = draws[segment_start..]
                .iter()
                .position(|d| matches!(d, Draw::Backdrop(_)))
                .map_or(draws.len(), |at| segment_start + at);
            {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("game shared-device pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: if load_3d || !first_segment {
                            wgpu::LoadOp::Load
                        } else {
                            wgpu::LoadOp::Clear(clear_color)
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
            });
            first_segment = false;

            for draw in &draws[segment_start..segment_end] {
                match draw {
                    Draw::Particles(particles) => {
                        if let Some(pipeline) = self
                            .particles
                            .as_ref()
                            .and_then(|p| p.pipelines.get(&particles.stride))
                        {
                            pass.set_pipeline(pipeline);
                            pass.set_bind_group(0, &particles.bind_group, &[]);
                            pass.set_vertex_buffer(0, particles.vertices.slice(..));
                            pass.draw(0..6, 0..particles.count);
                            drew = true;
                        }
                    }
                    Draw::Quads(vertices) => {
                        let buffer =
                            self.device
                                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                                    label: Some("game quad batch"),
                                    contents: bytemuck::cast_slice(vertices),
                                    usage: wgpu::BufferUsages::VERTEX,
                                });
                        pass.set_pipeline(&self.pipeline);
                        pass.set_vertex_buffer(0, buffer.slice(..));
                        pass.draw(0..vertices.len() as u32, 0..1);
                        drew = true;
                    }
                    Draw::Sprite {
                        key,
                        verts,
                        additive,
                    } => {
                        let Some(sprite) = self.sprites.get(key) else {
                            continue;
                        };
                        let buffer =
                            self.device
                                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                                    label: Some("game sprite verts"),
                                    contents: bytemuck::cast_slice(verts),
                                    usage: wgpu::BufferUsages::VERTEX,
                                });
                        pass.set_pipeline(if *additive {
                            &self.additive_sprite_pipeline
                        } else {
                            &self.sprite_pipeline
                        });
                        pass.set_bind_group(0, &sprite.bind_group, &[]);
                        pass.set_vertex_buffer(0, buffer.slice(..));
                        pass.draw(0..verts.len() as u32, 0..1);
                        drew = true;
                    }
                    Draw::SdfRun { .. } | Draw::GlyphRun { .. } | Draw::Backdrop(_) => {}
                    Draw::TargetSprite { bind_group, verts } => {
                        let buffer =
                            self.device
                                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                                    label: Some("render target sprite verts"),
                                    contents: bytemuck::cast_slice(verts),
                                    usage: wgpu::BufferUsages::VERTEX,
                                });
                        pass.set_pipeline(&self.target_sprite_pipeline);
                        pass.set_bind_group(0, bind_group, &[]);
                        pass.set_vertex_buffer(0, buffer.slice(..));
                        pass.draw(0..6, 0..1);
                        drew = true;
                    }
                    Draw::Glyphs {
                        page,
                        buffer,
                        count,
                    } => {
                        let Some(glyphs) = self.glyphs.as_ref() else {
                            continue;
                        };
                        let Some(bind_group) = glyphs.bind_group(*page) else {
                            continue;
                        };
                        pass.set_pipeline(glyphs.pipeline());
                        pass.set_bind_group(0, bind_group, &[]);
                        pass.set_vertex_buffer(0, buffer.slice(..));
                        pass.draw(0..6, 0..*count);
                        drew = true;
                    }
                    Draw::Sdf {
                        key,
                        bind_group,
                        aux,
                        count,
                    } => {
                        let Some(pipeline) = self.sdf.as_ref().and_then(|p| p.pipeline(*key))
                        else {
                            continue;
                        };
                        pass.set_pipeline(pipeline);
                        pass.set_bind_group(0, bind_group, &[]);
                        pass.set_bind_group(1, aux, &[]);
                        pass.draw(0..6, 0..*count);
                        drew = true;
                    }
                    Draw::MaskedSprite { bind_group, verts } => {
                        let buffer =
                            self.device
                                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                                    label: Some("game clipped group vertices"),
                                    contents: bytemuck::cast_slice(verts),
                                    usage: wgpu::BufferUsages::VERTEX,
                                });
                        pass.set_pipeline(&self.masked_sprite_pipeline);
                        pass.set_bind_group(0, bind_group, &[]);
                        pass.set_vertex_buffer(0, buffer.slice(..));
                        pass.draw(0..6, 0..1);
                        drew = true;
                    }
                }
            }
            }
            if segment_end == draws.len() {
                break;
            }
            if let Draw::Backdrop(backdrop) = &draws[segment_end] {
                self.encode_backdrop(&mut encoder, backdrop);
                drew = true;
            }
            segment_start = segment_end + 1;
        }
        if let (Some(set), Some(blur)) = (layer_set.as_ref(), self.blur.as_ref()) {
            let out = final_target.create_view(&wgpu::TextureViewDescriptor::default());
            blur.blit(&self.device, &mut encoder, &set.frame, &out);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        self.render_depth -= 1;
        drew
    }
}

/// Build the 6 vertices (two triangles) for one sprite command: `src` in
/// pixels → UVs over the sheet; `dst` in pixels → NDC, rotated about its
/// center; `opacity` rides the vertex alpha; flips swap the UV extents.
#[allow(clippy::too_many_arguments)]
fn sprite_vertices(
    src: &Rect,
    dst: &Rect,
    sheet_w: u32,
    sheet_h: u32,
    rotation: f32,
    opacity: f32,
    flip_x: bool,
    flip_y: bool,
    target_w: u32,
    target_h: u32,
) -> [SpriteVertex; 6] {
    let tw = sheet_w.max(1) as f32;
    let th = sheet_h.max(1) as f32;
    let (mut u0, mut v0) = (src.x / tw, src.y / th);
    let (mut u1, mut v1) = ((src.x + src.w) / tw, (src.y + src.h) / th);
    if flip_x {
        std::mem::swap(&mut u0, &mut u1);
    }
    if flip_y {
        std::mem::swap(&mut v0, &mut v1);
    }
    let uvs = [[u0, v0], [u0, v1], [u1, v0], [u1, v1]];

    let cx = dst.x + dst.w / 2.0;
    let cy = dst.y + dst.h / 2.0;
    let (sin, cos) = rotation.sin_cos();
    let corner = |dx: f32, dy: f32| -> [f32; 2] {
        let px = dst.x + dx - cx;
        let py = dst.y + dy - cy;
        let rx = cx + px * cos - py * sin;
        let ry = cy + px * sin + py * cos;
        to_ndc(rx, ry, target_w, target_h)
    };
    let corners = [
        corner(0.0, 0.0),     // tl
        corner(0.0, dst.h),   // bl
        corner(dst.w, 0.0),   // tr
        corner(dst.w, dst.h), // br
    ];

    let color = [1.0, 1.0, 1.0, opacity.clamp(0.0, 1.0)];
    let v = |pos: [f32; 2], uv: [f32; 2]| SpriteVertex {
        position: pos,
        uv,
        color,
    };
    [
        v(corners[0], uvs[0]),
        v(corners[1], uvs[1]),
        v(corners[2], uvs[2]),
        v(corners[2], uvs[2]),
        v(corners[1], uvs[1]),
        v(corners[3], uvs[3]),
    ]
}

fn textured_triangle_vertices(
    points: [Vec2; 3],
    uv: [[f32; 2]; 3],
    uv_in_pixels: bool,
    sheet_w: u32,
    sheet_h: u32,
    opacity: f32,
    target_w: u32,
    target_h: u32,
) -> [SpriteVertex; 3] {
    let scale_u = if uv_in_pixels {
        sheet_w.max(1) as f32
    } else {
        1.0
    };
    let scale_v = if uv_in_pixels {
        sheet_h.max(1) as f32
    } else {
        1.0
    };
    std::array::from_fn(|i| SpriteVertex {
        position: to_ndc(points[i].x, points[i].y, target_w, target_h),
        uv: [uv[i][0] / scale_u, uv[i][1] / scale_v],
        color: [1.0, 1.0, 1.0, opacity.clamp(0.0, 1.0)],
    })
}

impl Canvas {
    /// Ensure a `wgpu` adapter, device, queue, and pipeline are available.
    fn ensure_device_resources(&mut self) -> Result<&DeviceResources> {
        if self.device_resources.is_none() {
            let shared = crate::device::acquire(&self.instance, self.surface.as_ref())?;
            if self.surface.is_none() {
                if let Some(instance) = &shared.instance {
                    self.instance = instance.clone();
                }
            }
            let device = shared.device.clone();
            let queue = shared.queue.clone();

            // Use the surface's preferred format when on-screen, fall back to Rgba8Unorm headless.
            let surface_format = match (self.surface.as_ref(), shared.adapter.as_ref()) {
                (Some(s), Some(adapter)) => {
                    // Packet colors are already sRGB-encoded (CSS values): an sRGB target would
                    // encode them a second time and wash every color out.
                    let formats = s.get_capabilities(adapter).formats;
                    formats
                        .iter()
                        .copied()
                        .find(|f| !f.is_srgb())
                        .or_else(|| formats.first().copied())
                        .unwrap_or(wgpu::TextureFormat::Rgba8Unorm)
                }
                (Some(_), None) => {
                    return Err(RendererError::Surface(
                        "a surface needs the adapter that opened the shared device".into(),
                    ))
                }
                (None, _) => wgpu::TextureFormat::Rgba8Unorm,
            };

            self.device_resources = Some(DeviceResources {
                adapter: shared.adapter,
                device: device.clone(),
                queue: queue.clone(),
                surface_format,
            });
        }

        Ok(self.device_resources.as_ref().expect("just initialized"))
    }

    /// Configure (or reconfigure) the surface for a given size.
    ///
    /// Must be called before the first `render_to_surface` and on every resize.
    pub(crate) fn configure_surface(&mut self, width: u32, height: u32) -> Result<()> {
        self.set_viewport(width, height);
        let resources = self.ensure_device_resources()?;
        let format = resources.surface_format;
        let device = resources.device.clone();
        let surface = self
            .surface
            .as_ref()
            .ok_or_else(|| RendererError::Surface("no surface".into()))?;
        surface.configure(
            &device,
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format,
                width: width.max(1),
                height: height.max(1),
                present_mode: wgpu::PresentMode::Fifo,
                desired_maximum_frame_latency: 2,
                alpha_mode: wgpu::CompositeAlphaMode::Auto,
                view_formats: vec![],
            },
        );
        Ok(())
    }

    /// Render one frame from `packet` to the on-screen surface and present it.
    pub(crate) fn render_to_surface(
        &mut self,
        packet: &crate::packet::GameRenderPacket,
    ) -> Result<()> {
        let surface = self
            .surface
            .as_ref()
            .ok_or_else(|| RendererError::Surface("no surface".into()))?;

        let frame = surface
            .get_current_texture()
            .map_err(|e| RendererError::Surface(format!("get_current_texture: {e:?}")))?;

        let resources = self.ensure_device_resources()?;
        let device = resources.device.clone();
        let queue = resources.queue.clone();
        let format = resources.surface_format;
        self.shared
            .get_or_insert_with(|| SharedDeviceRenderer::new(device, queue, format))
            .render_packet_to_texture(packet, &frame.texture);
        frame.present();
        Ok(())
    }

    /// Render the supplied packet into an RGBA pixel buffer.
    ///
    /// The returned buffer has length `width * height * 4` and is in
    /// top-to-bottom row order matching image conventions.
    pub fn render_packet_to_rgba(&mut self, packet: &GameRenderPacket) -> Result<Vec<u8>> {
        let resources = self.ensure_device_resources()?;
        let device = resources.device.clone();
        let queue = resources.queue.clone();

        // The readback is the host surface in physical pixels once a viewport is known
        // (packets are in logical units), else the packet's own size.
        let (width, height) = self
            .viewport
            .unwrap_or((packet.width.max(1.0) as u32, packet.height.max(1.0) as u32));

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("game render target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let shared = self.rgba_shared.get_or_insert_with(|| {
            SharedDeviceRenderer::new(
                device.clone(),
                queue.clone(),
                wgpu::TextureFormat::Rgba8Unorm,
            )
        });
        shared.render_packet_to_texture(packet, &texture);
        let encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("game readback encoder"),
        });
        copy_texture_to_rgba(&device, &queue, encoder, &texture, width, height)
    }
}

/// Record a texture→buffer copy into `encoder`, submit, and read the pixels
/// back as a tightly-packed RGBA8 buffer (top-to-bottom rows). Takes the
/// encoder by value (it finishes + submits it). Shared by the headless
/// readback path, tests of the shared-device renderer, and the
/// `dump-canvas-packet` example (fixture → PNG visual proof).
pub fn copy_texture_to_rgba(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    mut encoder: wgpu::CommandEncoder,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Result<Vec<u8>> {
    let bytes_per_row_unpadded = width * 4;
    let bytes_per_row_padded = align_up(
        bytes_per_row_unpadded as usize,
        wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize,
    );
    let buffer_size = bytes_per_row_padded * height as usize;

    let readback_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("game readback"),
        size: buffer_size as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback_buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row_padded as u32),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );

    queue.submit(std::iter::once(encoder.finish()));

    let slice = readback_buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    device.poll(wgpu::PollType::wait()).ok();
    rx.recv()
        .map_err(|_| RendererError::Surface("readback channel closed".into()))?
        .map_err(|e| RendererError::Surface(format!("buffer map failed: {e}")))?;

    let data = slice.get_mapped_range();
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height {
        let start = row as usize * bytes_per_row_padded;
        rgba.extend_from_slice(&data[start..start + bytes_per_row_unpadded as usize]);
    }
    drop(data);
    readback_buffer.unmap();

    Ok(rgba)
}

/// Convert pixel coordinates to Normalized Device Coordinates (-1..1).
fn to_ndc(x: f32, y: f32, width: u32, height: u32) -> [f32; 2] {
    [
        (x / width as f32) * 2.0 - 1.0,
        1.0 - (y / height as f32) * 2.0,
    ]
}

#[cfg(test)]
mod scene3d_canvas_tests {
    #[test]
    fn authored_3d_canvas_scene_paints_box_pixels_on_the_gpu() {
        let Some(mut renderer) = super::svg_sprite_tests::renderer() else {
            return;
        };
        let node = serde_json::json!({
            "type": "canvas", "mode": "3d", "backgroundColor": "#000000",
            "camera": {"pos": {"x": 0, "y": 5, "z": 2},
                "target": {"x": 0, "y": 0, "z": 0}, "fov": 60},
            "drawables": [{"type": "draw-mesh", "shape": "box",
                "position": {"x": 0, "y": 0, "z": 0},
                "material": {"kind": "basic", "side": "double", "color": "#ff0000"}}]
        });
        let mapped =
            crate::mappers::scene::map_scene(&node, Some((64.0, 64.0))).expect("authored 3D scene");
        let pixels =
            super::svg_sprite_tests::draw(&mut renderer, &mapped.packet, 64).expect("GPU readback");
        let red = pixels
            .chunks_exact(4)
            .filter(|pixel| pixel[0] > 150 && pixel[1] < 60 && pixel[2] < 60)
            .count();
        assert!(
            red > 20,
            "authored box must cover visible red pixels: {red}"
        );
    }
}

/// Push two triangles covering `rect` into the vertex buffer.
fn push_quad(vertices: &mut Vec<Vertex>, rect: &Rect, color: &[f32; 4], width: u32, height: u32) {
    let tl = to_ndc(rect.x, rect.y, width, height);
    let tr = to_ndc(rect.x + rect.w, rect.y, width, height);
    let bl = to_ndc(rect.x, rect.y + rect.h, width, height);
    let br = to_ndc(rect.x + rect.w, rect.y + rect.h, width, height);

    let c = *color;

    // First triangle: tl, bl, tr
    vertices.push(Vertex {
        position: tl,
        color: c,
    });
    vertices.push(Vertex {
        position: bl,
        color: c,
    });
    vertices.push(Vertex {
        position: tr,
        color: c,
    });

    // Second triangle: tr, bl, br
    vertices.push(Vertex {
        position: tr,
        color: c,
    });
    vertices.push(Vertex {
        position: bl,
        color: c,
    });
    vertices.push(Vertex {
        position: br,
        color: c,
    });
}

/// Push a triangle fan (p0, pi, pi+1) for a convex polygon into the vertex buffer.
fn push_poly_fan(
    vertices: &mut Vec<Vertex>,
    points: &[Vec2],
    color: &[f32; 4],
    width: u32,
    height: u32,
) {
    if points.len() < 3 {
        return;
    }
    let c = *color;
    let p0 = to_ndc(points[0].x, points[0].y, width, height);
    for i in 1..points.len() - 1 {
        vertices.push(Vertex {
            position: p0,
            color: c,
        });
        vertices.push(Vertex {
            position: to_ndc(points[i].x, points[i].y, width, height),
            color: c,
        });
        vertices.push(Vertex {
            position: to_ndc(points[i + 1].x, points[i + 1].y, width, height),
            color: c,
        });
    }
}

#[cfg(test)]
mod svg_sprite_tests {
    use super::*;
    use crate::packet::{DrawCommand, GameRenderPacket, Rect};

    /// Left half blue, right half transparent; 4x4 intrinsic.
    const HALF_BLUE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="2" height="4" fill="#0000ff"/></svg>"##;

    pub(super) fn renderer() -> Option<SharedDeviceRenderer> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            flags: wgpu::InstanceFlags::default(),
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            backend_options: wgpu::BackendOptions::default(),
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .ok()?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: wgpu::Features::empty(),
            required_limits:
                wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
            label: Some("svg sprite test"),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .ok()?;
        Some(SharedDeviceRenderer::new(
            Arc::new(device),
            Arc::new(queue),
            wgpu::TextureFormat::Rgba8Unorm,
        ))
    }

    pub(super) fn fixture(name: &str, contents: &[u8]) -> String {
        let dir =
            std::env::temp_dir().join(format!("orbital-canvas-svg-sprite-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("tempdir");
        let path = dir.join(name);
        std::fs::write(&path, contents).expect("write fixture");
        let url = format!("file://{}", path.display());
        for _ in 0..200 {
            if crate::asset::asset_bytes(&url).is_some() {
                return url;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("fixture must arrive within 2s");
    }

    fn sprite_packet(url: &str, logical: f32) -> GameRenderPacket {
        GameRenderPacket {
            pattern: "canvas-2d".into(),
            width: logical,
            height: logical,
            draw_commands: vec![DrawCommand::Sprite {
                texture: url.to_string(),
                src: Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 4.0,
                    h: 4.0,
                },
                dst: Rect {
                    x: 0.0,
                    y: 0.0,
                    w: logical,
                    h: logical,
                },
                rotation: 0.0,
                opacity: 1.0,
                flip_x: false,
                flip_y: false,
            }],
            ..Default::default()
        }
    }

    /// Render `packet` into a physical `px` x `px` target and read it back.
    pub(super) fn draw(
        r: &mut SharedDeviceRenderer,
        packet: &GameRenderPacket,
        px: u32,
    ) -> Option<Vec<u8>> {
        let target = r.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("svg sprite target"),
            size: wgpu::Extent3d {
                width: px,
                height: px,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        assert!(
            r.render_packet_to_texture(packet, &target),
            "sprite should draw"
        );
        let encoder = r
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("readback"),
            });
        copy_texture_to_rgba(r.device(), r.queue(), encoder, &target, px, px).ok()
    }

    pub(super) fn px(rgba: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * size + x) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    }

    #[test]
    fn svg_sprite_draws_through_the_textured_pipeline() {
        let Some(mut r) = renderer() else { return };
        let url = fixture("half-blue.svg", HALF_BLUE.as_bytes());
        let Some(rgba) = draw(&mut r, &sprite_packet(&url, 8.0), 8) else {
            return;
        };
        let left = px(&rgba, 8, 1, 4);
        assert!(
            left[2] > 200 && left[0] < 60 && left[3] > 200,
            "left half is the svg's blue, got {left:?}"
        );
        assert_eq!(
            px(&rgba, 8, 6, 4)[3],
            0,
            "right half stays transparent (svg alpha preserved)"
        );
    }

    #[test]
    fn svg_pattern_and_gradient_mask_reach_gpu_pixels() {
        let Some(mut renderer) = renderer() else {
            return;
        };
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16">
            <defs>
                <pattern id="tile" width="4" height="4" patternUnits="userSpaceOnUse"><rect width="2" height="4" fill="#ff0000"/></pattern>
                <linearGradient id="fade" x1="0%" y1="0%" x2="100%" y2="0%"><stop offset="0%" stop-color="white" stop-opacity="1"/><stop offset="100%" stop-color="white" stop-opacity="0"/></linearGradient>
                <mask id="mask"><rect width="100%" height="100%" fill="url(#fade)"/></mask>
            </defs>
            <rect width="100%" height="100%" fill="url(#tile)" mask="url(#mask)"/>
        </svg>"##;
        let url = fixture("pattern-mask.svg", svg.as_bytes());
        let packet = GameRenderPacket {
            pattern: "geometric-pattern-scene".into(),
            width: 16.0,
            height: 16.0,
            draw_commands: vec![DrawCommand::Sprite {
                texture: url,
                src: Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 16.0,
                    h: 16.0,
                },
                dst: Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 16.0,
                    h: 16.0,
                },
                rotation: 0.0,
                opacity: 1.0,
                flip_x: false,
                flip_y: false,
            }],
            ..Default::default()
        };
        let pixels = draw(&mut renderer, &packet, 16).expect("pattern mask GPU frame");
        let alphas = [1, 5, 9, 13].map(|x| px(&pixels, 16, x, 8)[3]);
        assert!(alphas[0] > 200, "visible edge: {alphas:?}");
        assert!(
            alphas[0] > alphas[1] && alphas[1] > alphas[2] && alphas[2] > alphas[3],
            "gradient fade: {alphas:?}"
        );
        assert!(alphas[3] < 80, "faded edge: {alphas:?}");
        for x in [2, 6, 10, 14] {
            assert!(
                px(&pixels, 16, x, 8)[3] < alphas[0] / 2,
                "tile gap at x={x}"
            );
        }
    }

    /// Density: a 3x target (24 physical for an 8-unit logical packet) gets a
    /// ~3x denser raster; the 1x control rasterizes at logical size.
    #[test]
    fn svg_texture_is_rasterized_at_logical_size_times_density() {
        let Some(mut r1) = renderer() else { return };
        let url = fixture("density.svg", HALF_BLUE.as_bytes());
        let Some(_) = draw(&mut r1, &sprite_packet(&url, 8.0), 8) else {
            return;
        };
        assert_eq!(
            r1.svg_texture_sizes(),
            vec![(8, 8)],
            "1x: 8 logical px / 4 units = scale 2 -> 8px texture"
        );

        let mut r3 = renderer().expect("adapter present");
        let Some(rgba) = draw(&mut r3, &sprite_packet(&url, 8.0), 24) else {
            return;
        };
        let sizes = r3.svg_texture_sizes();
        assert_eq!(sizes.len(), 1);
        assert!(
            sizes[0].0 >= 24 && sizes[0].0 < 24 * 12 / 10 + 2,
            "3x: >= physical 24px, bucketed, got {sizes:?}"
        );
        // Bucketed texture is up to ~19% over the screen size, so the edge is
        // bilinear-soft within a texel or two; the interiors are exact.
        let inside = px(&rgba, 24, 9, 12);
        let outside = px(&rgba, 24, 15, 12);
        assert!(
            inside[2] > 240 && outside[3] < 8,
            "3x: svg edge near physical midpoint: {inside:?} | {outside:?}"
        );
    }

    #[test]
    fn svg_raster_is_cached_per_size_and_reused_across_frames() {
        let Some(mut r) = renderer() else { return };
        let url = fixture("cache.svg", HALF_BLUE.as_bytes());
        let packet = sprite_packet(&url, 8.0);
        let Some(_) = draw(&mut r, &packet, 8) else {
            return;
        };
        let _ = draw(&mut r, &packet, 8);
        assert_eq!(
            r.svg_texture_sizes().len(),
            1,
            "repeat frame hits the cache"
        );
        assert_eq!(crate::svg::parse_count(&url), 1, "tree parsed once");
        // Same bucket (8.0 -> 8.2 stays under the next quarter-octave): still a hit.
        let _ = draw(&mut r, &sprite_packet(&url, 8.2), 8);
        assert_eq!(
            r.svg_texture_sizes().len(),
            1,
            "sub-bucket zoom reuses the texture"
        );
        // Control: a genuinely larger draw adds exactly one more texture.
        let _ = draw(&mut r, &sprite_packet(&url, 16.0), 16);
        assert_eq!(r.svg_texture_sizes().len(), 2);
        assert_eq!(crate::svg::parse_count(&url), 1, "still one parse");
    }

    #[test]
    fn svg_rasters_per_url_are_bounded_and_evict_least_recently_used() {
        let Some(mut r) = renderer() else { return };
        let url = fixture("lru.svg", HALF_BLUE.as_bytes());
        let other = fixture("lru-other.svg", HALF_BLUE.as_bytes());
        // Logical sizes 8,16,32,64 -> four distinct textures (the bound).
        for logical in [8.0f32, 16.0, 32.0, 64.0] {
            let px = logical as u32;
            let _ = draw(&mut r, &sprite_packet(&url, logical), px);
        }
        assert_eq!(r.svg_texture_sizes().len(), MAX_SVG_RASTERS_PER_URL);
        // Another URL has its own budget.
        let _ = draw(&mut r, &sprite_packet(&other, 8.0), 8);
        assert_eq!(r.svg_texture_sizes().len(), MAX_SVG_RASTERS_PER_URL + 1);
        // Touch the oldest (8) so 16 becomes least recently used, then add a fifth size.
        let _ = draw(&mut r, &sprite_packet(&url, 8.0), 8);
        let _ = draw(&mut r, &sprite_packet(&url, 128.0), 128);
        let mut mine: Vec<_> = r
            .sprites
            .keys()
            .filter(|k| k.url == url)
            .filter_map(|k| k.raster)
            .collect();
        mine.sort_unstable();
        assert_eq!(
            mine,
            vec![(8, 8), (32, 32), (64, 64), (128, 128)],
            "16 was least recently used (8 was touched), so it is the one evicted"
        );
        assert_eq!(
            r.sprites.keys().filter(|k| k.url == other).count(),
            1,
            "other URL untouched by this URL's eviction"
        );
        // The evicted size re-rasterizes on demand (eviction is a cache miss, not a failure).
        let rgba = draw(&mut r, &sprite_packet(&url, 16.0), 16).expect("re-rasterize");
        assert!(px(&rgba, 16, 2, 8)[2] > 200, "evicted size draws again");
    }

    /// Control: a PNG keeps its single native-size texture (no SVG path).
    #[test]
    fn png_sprite_still_decodes_once_at_native_size() {
        let Some(mut r) = renderer() else { return };
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 255, 255]))
            .write_to(&mut png, image::ImageFormat::Png)
            .expect("encode png");
        let url = fixture("native.png", png.get_ref());
        let Some(rgba) = draw(&mut r, &sprite_packet(&url, 8.0), 24) else {
            return;
        };
        assert!(r.svg_texture_sizes().is_empty(), "no svg raster for a png");
        assert_eq!(r.sprites.len(), 1);
        let c = px(&rgba, 24, 12, 12);
        assert!(c[2] > 200 && c[0] < 60, "png drawn, got {c:?}");
        let _ = draw(&mut r, &sprite_packet(&url, 8.0), 24);
        assert_eq!(r.sprites.len(), 1, "png cache hit on repeat");
    }
}

#[cfg(test)]
mod vector_layer_tests {
    use super::svg_sprite_tests::{draw, px, renderer};
    use super::*;
    use crate::packet::{PathSegment, Stroke, TextAlign};

    const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
    const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Vec<PathSegment> {
        vec![
            PathSegment::MoveTo { x, y },
            PathSegment::LineTo { x: x + w, y },
            PathSegment::LineTo { x: x + w, y: y + h },
            PathSegment::LineTo { x, y: y + h },
            PathSegment::Close,
        ]
    }

    fn packet(commands: Vec<DrawCommand>) -> GameRenderPacket {
        GameRenderPacket {
            pattern: "learning-canvas".into(),
            width: 16.0,
            height: 16.0,
            draw_commands: commands,
            ..Default::default()
        }
    }

    fn filled(x: f32, color: [f32; 4]) -> DrawCommand {
        DrawCommand::Path {
            segments: rect(x, 4.0, 8.0, 8.0),
            fill: Some(color),
            stroke: None,
        }
    }

    #[test]
    fn a_filled_path_reaches_the_target_at_its_logical_position() {
        let Some(mut r) = renderer() else { return };
        let Some(rgba) = draw(&mut r, &packet(vec![filled(4.0, RED)]), 16) else {
            return;
        };
        assert_eq!(px(&rgba, 16, 8, 8), [255, 0, 0, 255]);
        assert_eq!(
            px(&rgba, 16, 1, 1)[3],
            0,
            "control: outside the path stays clear"
        );
    }

    #[test]
    fn density_scales_the_vector_layer_with_the_target() {
        let Some(mut r) = renderer() else { return };
        let Some(rgba) = draw(&mut r, &packet(vec![filled(4.0, RED)]), 32) else {
            return;
        };
        assert_eq!(px(&rgba, 32, 16, 16), [255, 0, 0, 255]);
        assert_eq!(
            px(&rgba, 32, 4, 16)[3],
            0,
            "control: 2x target keeps logical extents"
        );
    }

    #[test]
    fn vector_commands_keep_z_order_with_quads() {
        let Some(mut r) = renderer() else { return };
        let quad = |color| DrawCommand::Quad {
            rect: Rect {
                x: 0.0,
                y: 0.0,
                w: 16.0,
                h: 16.0,
            },
            color,
        };
        let over = draw(&mut r, &packet(vec![quad(BLUE), filled(4.0, RED)]), 16).unwrap();
        assert_eq!(px(&over, 16, 8, 8), [255, 0, 0, 255], "path over quad");
        let under = draw(&mut r, &packet(vec![filled(4.0, RED), quad(BLUE)]), 16).unwrap();
        assert_eq!(
            px(&under, 16, 8, 8),
            [0, 0, 255, 255],
            "quad over path (control)"
        );
    }

    #[test]
    fn gpu_clip_group_masks_mixed_quad_and_vector_children() {
        let Some(mut renderer) = renderer() else {
            return;
        };
        let group = DrawCommand::ClipGroup {
            clip: rect(0.0, 0.0, 8.0, 16.0),
            commands: vec![
                DrawCommand::Quad {
                    rect: Rect {
                        x: 0.0,
                        y: 0.0,
                        w: 16.0,
                        h: 16.0,
                    },
                    color: BLUE,
                },
                filled(2.0, RED),
            ],
        };
        let Some(rgba) = draw(&mut renderer, &packet(vec![group]), 16) else {
            return;
        };
        assert_eq!(px(&rgba, 16, 4, 8), [255, 0, 0, 255]);
        assert_eq!(px(&rgba, 16, 1, 8), [0, 0, 255, 255]);
        assert_eq!(px(&rgba, 16, 12, 8)[3], 0);
    }

    #[test]
    fn authored_shape_pivot_and_group_stroke_width_reach_gpu_pixels() {
        let Some(mut renderer) = renderer() else {
            return;
        };
        let rotated = crate::mappers::game_canvas_2d::from_props(&serde_json::json!({
            "width": 64, "height": 64, "scale": 1.0, "projection": "free", "bgColor": "#00000000",
            "drawables": [{"type": "draw-shape", "shape": "rect",
                "position": {"x": 16, "y": 16}, "width": 32, "height": 16,
                "fill": "#00ff00", "rotate": 1.5707963267948966,
                "pivot": {"x": 0, "y": 0}}]
        }))
        .unwrap();
        let pixels = draw(&mut renderer, &rotated, 64).expect("rotated shape GPU frame");
        assert!(
            px(&pixels, 64, 8, 24)[1] > 200,
            "pivot rotates fill into left side"
        );
        assert_eq!(
            px(&pixels, 64, 24, 24)[3],
            0,
            "old unrotated area stays clear"
        );

        let stroked = crate::mappers::game_canvas_2d::from_props(&serde_json::json!({
            "width": 64, "height": 64, "scale": 1.0, "projection": "free", "bgColor": "#00000000",
            "drawables": [{"type": "draw-group", "position": {"x": 0, "y": 0}, "scale": 2,
                "items": [{"type": "draw-shape", "shape": "rect",
                    "position": {"x": 8, "y": 8}, "width": 16, "height": 16,
                    "stroke": "#ff0000", "strokeWidth": 2}]}]
        }))
        .unwrap();
        let pixels = draw(&mut renderer, &stroked, 64).expect("group stroke GPU frame");
        assert!(
            px(&pixels, 64, 16, 32)[0] > 200,
            "stroke reaches transformed edge"
        );
        assert_eq!(
            px(&pixels, 64, 20, 32)[3],
            0,
            "group scale leaves stroke width in pixels"
        );
    }

    #[test]
    fn authored_shape_blur_softens_edges_on_wgpu_target() {
        let Some(mut renderer) = renderer() else {
            return;
        };
        let packet = crate::mappers::game_canvas_2d::from_props(&serde_json::json!({
            "width": 64, "height": 64, "scale": 1.0,
            "projection": "free", "bgColor": "#00000000",
            "drawables": [{"type": "draw-shape", "shape": "rect",
                "position": {"x": 16, "y": 16}, "width": 16, "height": 16,
                "fill": "#ff0000", "blur": 4}]
        }))
        .unwrap();
        assert!(packet
            .draw_commands
            .iter()
            .any(|command| matches!(command, DrawCommand::Blurred { .. })));
        let pixels = draw(&mut renderer, &packet, 64).expect("blurred shape GPU frame");
        assert!(
            px(&pixels, 64, 13, 24)[3] > 0,
            "blur reaches beyond the rect edge"
        );
        assert!(px(&pixels, 64, 24, 24)[0] > 150, "the center remains red");
        assert_eq!(px(&pixels, 64, 3, 24)[3], 0, "far outside remains clear");
    }

    #[test]
    fn authored_lighter_shapes_add_color_on_wgpu_target() {
        let Some(mut renderer) = renderer() else {
            return;
        };
        let shape = serde_json::json!({"type": "draw-shape", "shape": "rect",
            "position": {"x": 16, "y": 16}, "width": 16, "height": 16,
            "fill": "#ff0000", "opacity": 0.5, "blendMode": "lighter"});
        let scene = |drawables| {
            crate::mappers::game_canvas_2d::from_props(&serde_json::json!({
                "width": 64, "height": 64, "scale": 1.0, "projection": "free",
                "bgColor": "#00000000", "drawables": drawables,
            }))
            .unwrap()
        };
        let one = draw(&mut renderer, &scene(vec![shape.clone()]), 64).unwrap();
        let two = draw(&mut renderer, &scene(vec![shape.clone(), shape]), 64).unwrap();
        let single = px(&one, 64, 24, 24);
        let doubled = px(&two, 64, 24, 24);
        assert!(
            doubled[0] > single[0] && doubled[3] > single[3],
            "lighter adds overlapping red and alpha: {single:?} -> {doubled:?}"
        );
    }

    #[test]
    fn skinned_triangle_samples_authored_uvs_on_gpu() {
        let Some(mut renderer) = renderer() else {
            return;
        };
        let mut pixels = image::RgbaImage::new(4, 4);
        for y in 0..4 {
            for x in 0..4 {
                pixels.put_pixel(
                    x,
                    y,
                    if x < 2 {
                        image::Rgba([255, 0, 0, 255])
                    } else {
                        image::Rgba([0, 0, 255, 255])
                    },
                );
            }
        }
        let mut png = std::io::Cursor::new(Vec::new());
        pixels.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let texture = super::svg_sprite_tests::fixture("skinned-uv.png", png.get_ref());
        let triangle = DrawCommand::TexturedTriangle {
            texture,
            points: [
                Vec2 { x: 0.0, y: 0.0 },
                Vec2 { x: 16.0, y: 0.0 },
                Vec2 { x: 0.0, y: 16.0 },
            ],
            uv: [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            uv_in_pixels: false,
            opacity: 1.0,
        };
        let Some(rgba) = draw(&mut renderer, &packet(vec![triangle]), 16) else {
            return;
        };
        assert!(px(&rgba, 16, 2, 2)[0] > 200, "left UV samples red");
        assert!(px(&rgba, 16, 10, 2)[2] > 200, "right UV samples blue");
        assert_eq!(px(&rgba, 16, 15, 15)[3], 0, "outside mesh stays clear");
    }

    #[test]
    fn an_identical_packet_reuses_the_cached_layer_and_a_changed_one_adds_one() {
        let Some(mut r) = renderer() else { return };
        let _font = crate::vector::FONT_GENERATION_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _ = draw(&mut r, &packet(vec![filled(4.0, RED)]), 16);
        let _ = crate::asset::asset_bytes(
            "data:text/plain,cache-generation-must-not-change-solid-path",
        );
        let _ = draw(&mut r, &packet(vec![filled(4.0, RED)]), 16);
        assert_eq!(r.vector_layer_count(), 1);
        let _ = draw(&mut r, &packet(vec![filled(2.0, RED)]), 16);
        assert_eq!(r.vector_layer_count(), 2);
    }

    #[test]
    fn vector_layers_are_bounded() {
        let Some(mut r) = renderer() else { return };
        for i in 0..(MAX_VECTOR_LAYERS + 4) {
            let _ = draw(&mut r, &packet(vec![filled(i as f32 * 0.25, RED)]), 16);
        }
        assert!(r.vector_layer_count() <= MAX_VECTOR_LAYERS);
    }

    const SIDE: u32 = 128;

    fn text_packet(commands: Vec<DrawCommand>) -> GameRenderPacket {
        GameRenderPacket {
            pattern: "learning-canvas".into(),
            width: SIDE as f32,
            height: SIDE as f32,
            draw_commands: commands,
            ..Default::default()
        }
    }

    fn word(text: &str, x: f32, y: f32, size: f32) -> DrawCommand {
        DrawCommand::Text {
            text: text.into(),
            pos: Vec2 { x, y },
            size,
            color: RED,
            align: TextAlign::Left,
            weight: 400,
            family: crate::packet::TextFamily::Sans,
            halo: None,
        }
    }

    fn alpha(rgba: &[u8]) -> Vec<u8> {
        rgba.chunks_exact(4).map(|p| p[3]).collect()
    }

    fn cpu_alpha(command: &DrawCommand) -> Vec<u8> {
        alpha(&crate::vector::rasterize(&[command], SIDE, SIDE, 1.0, 1.0).expect("cpu layer"))
    }

    fn gpu_alpha(r: &mut SharedDeviceRenderer, command: &DrawCommand) -> Option<Vec<u8>> {
        draw(r, &text_packet(vec![command.clone()]), SIDE).map(|rgba| alpha(&rgba))
    }

    fn mean_diff(a: &[u8], b: &[u8]) -> f64 {
        let total: u64 = a.iter().zip(b).map(|(x, y)| x.abs_diff(*y) as u64).sum();
        total as f64 / a.len() as f64
    }

    fn ink_sum(a: &[u8]) -> u64 {
        a.iter().map(|v| *v as u64).sum()
    }

    fn ink_width(a: &[u8]) -> u32 {
        let cols: Vec<u32> = (0..SIDE)
            .filter(|x| (0..SIDE).any(|y| a[(y * SIDE + x) as usize] > 40))
            .collect();
        match (cols.first(), cols.last()) {
            (Some(first), Some(last)) => last - first + 1,
            _ => 0,
        }
    }

    #[test]
    fn atlas_text_paints_the_same_pixels_as_the_cpu_path() {
        let Some(mut r) = renderer() else { return };
        let command = word("Hello Wg", 10.3, 40.7, 24.0);
        let cpu = cpu_alpha(&command);
        let Some(gpu) = gpu_alpha(&mut r, &command) else {
            return;
        };
        assert!(ink_sum(&cpu) > 0, "the CPU path inks");
        let diff = mean_diff(&cpu, &gpu);
        assert!(diff < 0.6, "mean alpha difference {diff}");
        let (c, g) = (ink_sum(&cpu) as f64, ink_sum(&gpu) as f64);
        assert!((c - g).abs() / c < 0.03, "ink {c} vs {g}");
        let shifted = cpu_alpha(&word("Hello Wg", 13.3, 40.7, 24.0));
        assert!(
            mean_diff(&shifted, &gpu) > diff * 4.0,
            "control: text three pixels over is far from the atlas pixels"
        );
    }

    #[test]
    fn atlas_text_draws_in_the_text_colour_and_keeps_z_order() {
        let Some(mut r) = renderer() else { return };
        let under = DrawCommand::Quad {
            rect: Rect {
                x: 0.0,
                y: 0.0,
                w: SIDE as f32,
                h: SIDE as f32,
            },
            color: [0.0, 0.0, 1.0, 1.0],
        };
        let text = word("WWW", 4.0, 40.0, 40.0);
        let Some(over) = draw(
            &mut r,
            &text_packet(vec![under.clone(), text.clone()]),
            SIDE,
        ) else {
            return;
        };
        assert!(
            over.chunks_exact(4).any(|p| p[0] > 240 && p[2] < 15),
            "red text over the blue quad"
        );
        let Some(hidden) = draw(&mut r, &text_packet(vec![text, under]), SIDE) else {
            return;
        };
        assert!(
            hidden.chunks_exact(4).all(|p| p[0] == 0 && p[2] == 255),
            "control: the quad last covers the text"
        );
    }

    #[test]
    fn arabic_through_the_atlas_is_drawn_and_joined() {
        let Some(mut r) = renderer() else { return };
        let joined = word("بيت", 20.0, 40.0, 28.0);
        let apart = word("ب ي ت", 20.0, 40.0, 28.0);
        let Some(gpu) = gpu_alpha(&mut r, &joined) else {
            return;
        };
        assert!(ink_sum(&gpu) > 0, "the joined word has coverage");
        let diff = mean_diff(&cpu_alpha(&joined), &gpu);
        assert!(diff < 0.6, "the atlas draws the CPU's joined glyphs: {diff}");
        let Some(spread) = gpu_alpha(&mut r, &apart) else {
            return;
        };
        assert!(ink_sum(&spread) > 0);
        assert!(
            ink_width(&gpu) < ink_width(&spread),
            "control: the letters set apart are wider than the joined word"
        );
    }

    fn rasterized(r: &SharedDeviceRenderer) -> u64 {
        r.glyphs.as_ref().map_or(0, |g| g.rasterized())
    }

    #[test]
    fn a_second_frame_of_the_same_text_rasterizes_no_new_glyphs() {
        let Some(mut r) = renderer() else { return };
        let command = word("Cache", 8.0, 40.0, 20.0);
        let Some(first) = gpu_alpha(&mut r, &command) else {
            return;
        };
        let after_first = rasterized(&r);
        assert!(after_first > 0);
        let Some(second) = gpu_alpha(&mut r, &command) else {
            return;
        };
        assert_eq!(rasterized(&r), after_first, "a cache hit");
        assert_eq!(first, second, "the same pixels");
        let _ = gpu_alpha(&mut r, &word("Xyz", 8.0, 40.0, 20.0));
        assert!(rasterized(&r) > after_first, "control: new letters rasterize");
    }

    #[test]
    fn a_full_atlas_evicts_a_page_and_still_draws() {
        let Some(mut r) = renderer() else { return };
        r.glyphs = Some(crate::glyph_atlas::GlyphPass::with_limits(
            r.device(),
            wgpu::TextureFormat::Rgba8Unorm,
            64,
            1,
        ));
        let first = word("WMNO", 2.0, 30.0, 24.0);
        let second = word("QG", 2.0, 30.0, 24.0);
        let Some(_) = gpu_alpha(&mut r, &first) else {
            return;
        };
        let after_first = rasterized(&r);
        let Some(drawn) = gpu_alpha(&mut r, &second) else {
            return;
        };
        assert!(ink_sum(&drawn) > 0, "the evicting frame still draws");
        let diff = mean_diff(&cpu_alpha(&second), &drawn);
        assert!(diff < 0.6, "and draws the CPU's pixels: {diff}");
        assert_eq!(r.glyphs.as_ref().map(|g| g.page_count()), Some(1));
        let before_again = rasterized(&r);
        assert!(before_again > after_first);
        let Some(_) = gpu_alpha(&mut r, &first) else {
            return;
        };
        assert!(
            rasterized(&r) > before_again,
            "the evicted letters rasterize again"
        );
        let Some(_) = gpu_alpha(&mut r, &first) else {
            return;
        };
        let settled = rasterized(&r);
        let _ = gpu_alpha(&mut r, &first);
        assert_eq!(rasterized(&r), settled, "control: resident letters do not");
    }

    #[test]
    fn one_frame_wider_than_the_budget_grows_then_trims_back() {
        let Some(mut r) = renderer() else { return };
        r.glyphs = Some(crate::glyph_atlas::GlyphPass::with_limits(
            r.device(),
            wgpu::TextureFormat::Rgba8Unorm,
            64,
            1,
        ));
        let wide = word("WMNOQGHKRS", 2.0, 30.0, 24.0);
        let Some(drawn) = gpu_alpha(&mut r, &wide) else {
            return;
        };
        assert!(ink_width(&drawn) > 100, "every glyph of the frame draws");
        assert!(r.glyphs.as_ref().is_some_and(|g| g.page_count() > 1));
        let _ = gpu_alpha(&mut r, &word("i", 2.0, 30.0, 24.0));
        assert_eq!(r.glyphs.as_ref().map(|g| g.page_count()), Some(1));
    }

    #[test]
    fn stroke_and_text_commands_draw() {
        let Some(mut r) = renderer() else { return };
        let stroke = DrawCommand::Path {
            segments: vec![
                PathSegment::MoveTo { x: 0.0, y: 8.0 },
                PathSegment::LineTo { x: 16.0, y: 8.0 },
            ],
            fill: None,
            stroke: Some(Stroke {
                color: BLUE,
                width: 2.0,
                dash: vec![],
                dash_offset: 0.0,
                cap: Default::default(),
                join: Default::default(),
            }),
        };
        let text = DrawCommand::Text {
            text: "W".into(),
            pos: Vec2 { x: 8.0, y: 8.0 },
            size: 12.0,
            color: RED,
            align: TextAlign::Center,
            weight: 400,
            family: crate::packet::TextFamily::Sans,
            halo: None,
        };
        let Some(rgba) = draw(&mut r, &packet(vec![stroke]), 16) else {
            return;
        };
        assert_eq!(px(&rgba, 16, 3, 8), [0, 0, 255, 255]);
        assert_eq!(px(&rgba, 16, 3, 2)[3], 0, "control: off the stroke");
        let with_text = draw(&mut r, &packet(vec![text]), 16).unwrap();
        assert!(
            with_text.chunks_exact(4).any(|p| p[3] > 200 && p[0] > 200),
            "text ink is drawn"
        );
    }
}

#[cfg(test)]
mod sdf_tests {
    use super::svg_sprite_tests::{draw, px, renderer};
    use super::*;
    use crate::packet::{SdfKind, SdfMaterial};

    const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

    fn shape(kind: SdfKind, rect: Rect) -> DrawCommand {
        DrawCommand::Sdf {
            kind,
            rect,
            corners: [0.0; 4],
            corner_kind: 0,
            arc: [0.0; 3],
            polygon: [0.0; 2],
            fill: RED,
            stroke: None,
            shadow: None,
            rotation: 0.0,
            opacity: 1.0,
            material: None,
        }
    }

    fn full() -> Rect {
        Rect {
            x: 0.0,
            y: 0.0,
            w: 64.0,
            h: 64.0,
        }
    }

    fn packet(commands: Vec<DrawCommand>, time_ms: f64, pointer: Option<Vec2>) -> GameRenderPacket {
        GameRenderPacket {
            pattern: "canvas-2d".into(),
            width: 64.0,
            height: 64.0,
            draw_commands: commands,
            time_ms,
            pointer,
            ..Default::default()
        }
    }

    fn painted(rgba: &[u8], x: u32, y: u32) -> bool {
        px(rgba, 64, x, y)[3] > 200
    }

    fn rounded(radius: f32, kind: u32) -> DrawCommand {
        let mut command = shape(
            SdfKind::Rect,
            Rect {
                x: 8.0,
                y: 8.0,
                w: 48.0,
                h: 48.0,
            },
        );
        if let DrawCommand::Sdf {
            corners,
            corner_kind,
            ..
        } = &mut command
        {
            *corners = [radius; 4];
            *corner_kind = kind;
        }
        command
    }

    fn arc(sweep: f32) -> DrawCommand {
        let mut command = shape(SdfKind::Arc, full());
        if let DrawCommand::Sdf { arc, .. } = &mut command {
            *arc = [0.0, sweep, 8.0];
        }
        command
    }

    fn polygon(sides: f32, ratio: f32) -> DrawCommand {
        let mut command = shape(SdfKind::Polygon, full());
        if let DrawCommand::Sdf { polygon, .. } = &mut command {
            *polygon = [sides, ratio];
        }
        command
    }

    fn with_material(wgsl: &str) -> DrawCommand {
        let mut command = shape(SdfKind::Rect, full());
        if let DrawCommand::Sdf { material, .. } = &mut command {
            *material = Some(SdfMaterial {
                wgsl: wgsl.into(),
                params: vec![],
                live: true,
                since: 0.0,
                targets: Vec::new(),
            });
        }
        command
    }

    #[test]
    fn a_rounded_rect_paints_inside_and_not_outside_its_corners() {
        let Some(mut r) = renderer() else { return };
        let Some(rgba) = draw(&mut r, &packet(vec![rounded(16.0, 0)], 0.0, None), 64) else {
            return;
        };
        assert!(painted(&rgba, 32, 32), "the middle");
        assert!(painted(&rgba, 32, 9), "along an edge");
        assert!(!painted(&rgba, 9, 9), "outside the rounded corner");
        let Some(square) = draw(&mut r, &packet(vec![rounded(0.0, 0)], 0.0, None), 64) else {
            return;
        };
        assert!(
            painted(&square, 9, 9),
            "control: no radius keeps the corner"
        );
    }

    #[test]
    fn a_bevel_corner_is_cut_straight_where_a_round_one_still_paints() {
        let Some(mut r) = renderer() else { return };
        let Some(bevel) = draw(&mut r, &packet(vec![rounded(16.0, 1)], 0.0, None), 64) else {
            return;
        };
        let Some(round) = draw(&mut r, &packet(vec![rounded(16.0, 0)], 0.0, None), 64) else {
            return;
        };
        assert!(!painted(&bevel, 13, 13), "the chamfer cuts it off");
        assert!(
            painted(&round, 13, 13),
            "control: the round corner keeps it"
        );
        assert!(painted(&bevel, 32, 32), "the middle");
    }

    #[test]
    fn an_arc_of_sweep_pi_paints_half_a_ring() {
        let Some(mut r) = renderer() else { return };
        let Some(half) = draw(
            &mut r,
            &packet(vec![arc(std::f32::consts::PI)], 0.0, None),
            64,
        ) else {
            return;
        };
        assert!(painted(&half, 32, 60), "the lower half of the ring");
        assert!(!painted(&half, 32, 3), "not the upper half");
        assert!(!painted(&half, 32, 32), "not the hole");
        let Some(full_ring) = draw(
            &mut r,
            &packet(vec![arc(std::f32::consts::TAU)], 0.0, None),
            64,
        ) else {
            return;
        };
        assert!(
            painted(&full_ring, 32, 3),
            "control: a full sweep has the top"
        );
    }

    #[test]
    fn an_eight_point_star_has_tips_and_notches_a_regular_polygon_does_not() {
        let Some(mut r) = renderer() else { return };
        let Some(star) = draw(&mut r, &packet(vec![polygon(8.0, 0.7)], 0.0, None), 64) else {
            return;
        };
        assert!(painted(&star, 32, 32), "the middle");
        assert!(painted(&star, 32, 6), "the upward tip");
        assert!(!painted(&star, 43, 5), "between two tips");
        let Some(octagon) = draw(&mut r, &packet(vec![polygon(8.0, 1.0)], 0.0, None), 64) else {
            return;
        };
        assert!(
            painted(&octagon, 43, 5),
            "control: the regular octagon fills it"
        );
    }

    #[test]
    fn a_material_reading_time_changes_pixels_between_two_times() {
        let Some(mut r) = renderer() else { return };
        let wgsl =
            "fn shade(px: Pixel) -> vec4<f32> { return vec4<f32>(fract(px.time), 0.0, 0.0, 1.0); }";
        let at = |r: &mut SharedDeviceRenderer, ms: f64| {
            draw(r, &packet(vec![with_material(wgsl)], ms, None), 64)
        };
        let (Some(start), Some(later)) = (at(&mut r, 0.0), at(&mut r, 500.0)) else {
            return;
        };
        assert!(px(&start, 64, 32, 32)[0] < 5);
        let red = px(&later, 64, 32, 32)[0];
        assert!((100..160).contains(&red), "{red}");
        let Some(again) = at(&mut r, 0.0) else { return };
        assert_eq!(
            px(&again, 64, 32, 32),
            px(&start, 64, 32, 32),
            "control: the same time paints the same pixel"
        );
    }

    #[test]
    fn a_material_reading_the_pointer_lights_only_when_it_is_over_the_shape() {
        let Some(mut r) = renderer() else { return };
        let wgsl = "fn shade(px: Pixel) -> vec4<f32> { let over = px.pointer.x > 0.0 && px.pointer.x < px.size.x; return vec4<f32>(select(0.0, 1.0, over), 0.0, 0.0, 1.0); }";
        let at = |r: &mut SharedDeviceRenderer, pointer: Option<Vec2>| {
            draw(r, &packet(vec![with_material(wgsl)], 0.0, pointer), 64)
        };
        let (Some(over), Some(away)) = (
            at(&mut r, Some(Vec2 { x: 10.0, y: 10.0 })),
            at(&mut r, None),
        ) else {
            return;
        };
        assert_eq!(px(&over, 64, 32, 32)[0], 255);
        assert_eq!(px(&away, 64, 32, 32)[0], 0, "control: no pointer");
    }

    #[test]
    fn sdf_shapes_keep_z_order_with_quads() {
        let Some(mut r) = renderer() else { return };
        let blue_quad = DrawCommand::Quad {
            rect: full(),
            color: [0.0, 0.0, 1.0, 1.0],
        };
        let Some(over) = draw(
            &mut r,
            &packet(vec![blue_quad.clone(), rounded(0.0, 0)], 0.0, None),
            64,
        ) else {
            return;
        };
        assert_eq!(px(&over, 64, 32, 32), [255, 0, 0, 255], "the shape on top");
        let Some(under) = draw(
            &mut r,
            &packet(vec![rounded(0.0, 0), blue_quad], 0.0, None),
            64,
        ) else {
            return;
        };
        assert_eq!(
            px(&under, 64, 32, 32),
            [0, 0, 255, 255],
            "control: quad last"
        );
    }

    fn clipped_scene(clip: Option<serde_json::Value>) -> GameRenderPacket {
        let mut group = serde_json::json!({"type": "draw-group", "position": {"x": 0, "y": 0}, "items": [
            {"type": "draw-shape", "shape": "rect", "anchor": "top-left", "position": {"x": 0, "y": 0},
             "width": 64, "height": 64, "fill": "#ff0000", "corners": {"radius": [0, 0, 0, 0]}}]});
        if let Some(clip) = clip {
            group["clip"] = clip;
        }
        crate::mappers::game_canvas_2d::from_props(&serde_json::json!({
            "width": 64, "height": 64, "scale": 1.0, "projection": "free", "bgColor": "#00000000",
            "drawables": [group]
        }))
        .unwrap()
    }

    #[test]
    fn a_rect_clip_cuts_an_sdf_shape_at_its_edges_and_rounds_its_corners() {
        let Some(mut r) = renderer() else { return };
        let clip = serde_json::json!({"x": 8, "y": 8, "width": 48, "height": 48, "radius": 16});
        let Some(clipped) = draw(&mut r, &clipped_scene(Some(clip)), 64) else {
            return;
        };
        assert!(painted(&clipped, 32, 32), "inside the clip the shape shows");
        assert!(!painted(&clipped, 4, 32), "left of the clip it is cut");
        assert!(!painted(&clipped, 60, 32), "right of the clip it is cut");
        assert!(painted(&clipped, 32, 10), "just inside the top edge it shows");
        assert!(!painted(&clipped, 9, 9), "the rounded corner of the clip cuts the shape");
        let Some(free) = draw(&mut r, &clipped_scene(None), 64) else {
            return;
        };
        assert!(painted(&free, 4, 32) && painted(&free, 9, 9), "control: no clip, no cut");
    }

    fn two_circles(radius: Option<f32>) -> GameRenderPacket {
        let circle = |x: f32, fill: [f32; 4]| DrawCommand::Sdf {
            kind: SdfKind::Ellipse,
            rect: Rect { x, y: 20.0, w: 24.0, h: 24.0 },
            corners: [0.0; 4],
            corner_kind: 0,
            arc: [0.0; 3],
            polygon: [0.0; 2],
            fill,
            stroke: None,
            shadow: None,
            rotation: 0.0,
            opacity: 1.0,
            material: None,
        };
        let (a, b) = (circle(4.0, RED), circle(36.0, [0.0, 0.0, 1.0, 1.0]));
        let commands = match radius {
            Some(radius) => vec![DrawCommand::SdfBlend { first: Box::new(a), second: Box::new(b), radius }],
            None => vec![a, b],
        };
        packet(commands, 0.0, None)
    }

    #[test]
    fn a_smooth_union_melts_two_apart_circles_into_one_neck_and_mixes_their_colours() {
        let Some(mut r) = renderer() else { return };
        let Some(blended) = draw(&mut r, &two_circles(Some(24.0)), 64) else {
            return;
        };
        assert!(painted(&blended, 32, 32), "the neck bridges the 8px gap");
        assert!(painted(&blended, 16, 32) && painted(&blended, 48, 32), "both shapes are there");
        assert!(!painted(&blended, 32, 8), "above the neck stays empty");
        let near_a = px(&blended, 64, 12, 32);
        let near_b = px(&blended, 64, 52, 32);
        assert!(near_a[0] > 200 && near_a[2] < 60, "the first circle is red: {near_a:?}");
        assert!(near_b[2] > 200 && near_b[0] < 60, "the second circle is blue: {near_b:?}");
        let neck = px(&blended, 64, 32, 32);
        assert!(neck[0] > 60 && neck[2] > 60, "the neck mixes both fills: {neck:?}");
        let Some(apart) = draw(&mut r, &two_circles(None), 64) else {
            return;
        };
        assert!(!painted(&apart, 32, 32), "control: two separate circles leave the gap empty");
        assert!(painted(&apart, 16, 32) && painted(&apart, 48, 32));
    }
}

#[cfg(test)]
mod layer_tests {
    use super::svg_sprite_tests::{px, renderer};
    use super::*;
    use crate::packet::{SdfBackdrop, SdfKind, SdfMaterial};

    const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
    const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
    const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
    const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
    const CLEAR: [f32; 4] = [0.0; 4];

    fn quad(x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) -> DrawCommand {
        DrawCommand::Quad { rect: Rect { x, y, w, h }, color }
    }

    fn sdf(rect: Rect, fill: [f32; 4], material: Option<SdfMaterial>) -> DrawCommand {
        DrawCommand::Sdf {
            kind: SdfKind::Rect,
            rect,
            corners: [0.0; 4],
            corner_kind: 0,
            arc: [0.0; 3],
            polygon: [0.0; 2],
            fill,
            stroke: None,
            shadow: None,
            rotation: 0.0,
            opacity: 1.0,
            material,
        }
    }

    fn glass(blur: f32, tint: Option<[f32; 4]>, saturate: f32, material: Option<SdfMaterial>) -> DrawCommand {
        DrawCommand::SdfBackdrop {
            shape: Box::new(sdf(Rect { x: 16.0, y: 16.0, w: 32.0, h: 32.0 }, CLEAR, material)),
            backdrop: SdfBackdrop { blur, tint, saturate },
        }
    }

    fn frame(commands: Vec<DrawCommand>) -> GameRenderPacket {
        GameRenderPacket {
            pattern: "canvas-2d".into(),
            width: 64.0,
            height: 64.0,
            draw_commands: commands,
            ..Default::default()
        }
    }

    fn halves() -> Vec<DrawCommand> {
        vec![quad(0.0, 0.0, 32.0, 64.0, WHITE), quad(32.0, 0.0, 32.0, 64.0, BLACK)]
    }

    /// Render into a physical `px` x `px` target and read it back; a frame may paint nothing, so no draw is asserted.
    fn draw(r: &mut SharedDeviceRenderer, packet: &GameRenderPacket, px: u32) -> Option<Vec<u8>> {
        let target = r.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("layer test target"),
            size: wgpu::Extent3d { width: px, height: px, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        r.render_packet_to_texture(packet, &target);
        let encoder = r
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("readback") });
        copy_texture_to_rgba(r.device(), r.queue(), encoder, &target, px, px).ok()
    }

    fn at(rgba: &[u8], x: u32, y: u32) -> [u8; 4] {
        px(rgba, 64, x, y)
    }

    fn material(wgsl: &str, targets: &[&str]) -> SdfMaterial {
        SdfMaterial {
            wgsl: wgsl.into(),
            params: vec![],
            live: false,
            since: 0.0,
            targets: targets.iter().map(|t| (*t).to_owned()).collect(),
        }
    }

    #[test]
    fn a_backdrop_blurs_what_is_behind_it_inside_the_shape_only() {
        let Some(mut r) = renderer() else { return };
        let mut commands = halves();
        commands.push(glass(8.0, None, 1.0, None));
        let Some(blurred) = draw(&mut r, &frame(commands), 64) else { return };
        let edge = at(&blurred, 31, 32);
        assert!(edge[0] > 60 && edge[0] < 200, "the hard edge is softened inside the glass: {edge:?}");
        let inner = at(&blurred, 22, 32);
        assert!(inner[0] > edge[0] + 30, "further from the edge it stays whiter: {inner:?} vs {edge:?}");
        assert_eq!(at(&blurred, 8, 32), [255, 255, 255, 255], "outside the shape is untouched");
        assert_eq!(at(&blurred, 56, 32), [0, 0, 0, 255], "outside the shape is untouched");
        assert_eq!(at(&blurred, 31, 10)[0], 255, "above the glass is untouched");
        let mut plain = halves();
        plain.push(sdf(Rect { x: 16.0, y: 16.0, w: 32.0, h: 32.0 }, CLEAR, None));
        let Some(sharp) = draw(&mut r, &frame(plain), 64) else { return };
        assert_eq!(at(&sharp, 31, 32), [255, 255, 255, 255], "control: a plain shape leaves the edge sharp");
        assert_eq!(at(&sharp, 32, 32), [0, 0, 0, 255], "control: a plain shape leaves the edge sharp");
    }

    #[test]
    fn a_backdrop_only_blurs_what_was_painted_before_it() {
        let Some(mut r) = renderer() else { return };
        let mut commands = halves();
        commands.push(glass(8.0, None, 1.0, None));
        commands.push(quad(20.0, 20.0, 8.0, 8.0, RED));
        let Some(pixels) = draw(&mut r, &frame(commands), 64) else { return };
        assert_eq!(at(&pixels, 24, 24), [255, 0, 0, 255], "a mark painted after the glass stays sharp on top");
        let mut before = halves();
        before.push(quad(20.0, 20.0, 8.0, 8.0, RED));
        before.push(glass(8.0, None, 1.0, None));
        let Some(under) = draw(&mut r, &frame(before), 64) else { return };
        let blurred = at(&under, 24, 24);
        assert!(blurred[0] > 100 && blurred[1] > 100, "control: a mark painted before the glass is blurred into the white behind it: {blurred:?}");
    }

    #[test]
    fn saturate_and_tint_shape_the_blurred_backdrop() {
        let Some(mut r) = renderer() else { return };
        let red = || vec![quad(0.0, 0.0, 64.0, 64.0, RED)];
        let with = |blur: f32, tint, saturate| {
            let mut commands = red();
            commands.push(glass(blur, tint, saturate, None));
            commands
        };
        let Some(gray) = draw(&mut r, &frame(with(0.0, None, 0.0)), 64) else { return };
        let g = at(&gray, 32, 32);
        assert!(g[0].abs_diff(g[1]) < 6 && g[1].abs_diff(g[2]) < 6, "saturate 0 is gray: {g:?}");
        assert!(g[0] > 40 && g[0] < 70, "at the red's luminance: {g:?}");
        let Some(kept) = draw(&mut r, &frame(with(0.0, None, 1.0)), 64) else { return };
        assert_eq!(at(&kept, 32, 32), [255, 0, 0, 255], "control: saturate 1 keeps the red");
        let Some(tinted) = draw(&mut r, &frame(with(0.0, Some([0.0, 0.0, 1.0, 1.0]), 1.0)), 64) else { return };
        assert_eq!(at(&tinted, 32, 32), [0, 0, 255, 255], "an opaque tint covers the backdrop");
        let Some(half) = draw(&mut r, &frame(with(0.0, Some([0.0, 0.0, 1.0, 0.5]), 1.0)), 64) else { return };
        let h = at(&half, 32, 32);
        assert!(h[0] > 100 && h[2] > 100 && h[1] < 10, "a half tint mixes: {h:?}");
        assert_eq!(at(&half, 4, 4), [255, 0, 0, 255], "outside the glass is untouched");
    }

    #[test]
    fn a_material_reads_behind_and_it_is_zero_without_a_backdrop() {
        let Some(mut r) = renderer() else { return };
        let wgsl = "fn shade(px: Pixel) -> vec4<f32> { return vec4<f32>(px.behind.rgb, 1.0); }";
        let mut over = vec![quad(0.0, 0.0, 64.0, 64.0, RED)];
        over.push(glass(0.0, None, 1.0, Some(material(wgsl, &[]))));
        let Some(with) = draw(&mut r, &frame(over), 64) else { return };
        assert_eq!(at(&with, 32, 32), [255, 0, 0, 255], "the material paints what is behind");
        let mut without = vec![quad(0.0, 0.0, 64.0, 64.0, RED)];
        without.push(sdf(Rect { x: 16.0, y: 16.0, w: 32.0, h: 32.0 }, CLEAR, Some(material(wgsl, &[]))));
        let Some(zero) = draw(&mut r, &frame(without), 64) else { return };
        assert_eq!(at(&zero, 32, 32), [0, 0, 0, 255], "control: no backdrop, behind is zero");
    }

    fn target(id: &str, color: [f32; 4]) -> DrawCommand {
        DrawCommand::Target {
            id: id.into(),
            width: 16,
            height: 16,
            live: false,
            commands: vec![quad(0.0, 0.0, 16.0, 16.0, color)],
        }
    }

    fn reader(id: &str) -> DrawCommand {
        DrawCommand::Sprite {
            texture: format!("target:{id}"),
            src: Rect::default(),
            dst: Rect { x: 8.0, y: 8.0, w: 16.0, h: 16.0 },
            rotation: 0.0,
            opacity: 1.0,
            flip_x: false,
            flip_y: false,
        }
    }

    #[test]
    fn a_target_paints_nothing_itself_and_a_sprite_paints_it() {
        let Some(mut r) = renderer() else { return };
        let Some(alone) = draw(&mut r, &frame(vec![target("t", GREEN)]), 64) else { return };
        assert_eq!(at(&alone, 16, 16)[3], 0, "control: a target alone paints nothing in the frame");
        let Some(shown) = draw(&mut r, &frame(vec![target("t", GREEN), reader("t")]), 64) else { return };
        assert_eq!(at(&shown, 16, 16), [0, 255, 0, 255], "the sprite paints the target");
        assert_eq!(at(&shown, 4, 4)[3], 0, "outside the sprite is empty");
        assert_eq!(at(&shown, 30, 30)[3], 0, "outside the sprite is empty");
    }

    #[test]
    fn a_reader_before_its_writer_reads_the_previous_frame() {
        let Some(mut r) = renderer() else { return };
        let packet = frame(vec![reader("t"), target("t", GREEN)]);
        let Some(first) = draw(&mut r, &packet, 64) else { return };
        assert_eq!(at(&first, 16, 16)[3], 0, "the first frame has no earlier texture to read");
        let Some(second) = draw(&mut r, &packet, 64) else { return };
        assert_eq!(at(&second, 16, 16), [0, 255, 0, 255], "the next frame reads what the writer left");
        let Some(ordered) = draw(&mut r, &frame(vec![target("u", GREEN), reader("u")]), 64) else { return };
        assert_eq!(at(&ordered, 16, 16), [0, 255, 0, 255], "control: a writer before its reader is read at once");
    }

    #[test]
    fn a_reader_before_its_writer_sees_the_old_content_while_the_writer_repaints() {
        let Some(mut r) = renderer() else { return };
        let Some(_) = draw(&mut r, &frame(vec![target("t", GREEN)]), 64) else { return };
        let Some(late) = draw(&mut r, &frame(vec![reader("t"), target("t", RED)]), 64) else { return };
        assert_eq!(at(&late, 16, 16), [0, 255, 0, 255], "the reader before the writer shows last frame's green");
        let Some(now) = draw(&mut r, &frame(vec![target("t", RED), reader("t")]), 64) else { return };
        assert_eq!(at(&now, 16, 16), [255, 0, 0, 255], "control: after the writer it shows the new red");
    }

    #[test]
    fn a_target_repaints_when_its_children_change_and_keeps_its_texture_when_they_do_not() {
        let Some(mut r) = renderer() else { return };
        let Some(green) = draw(&mut r, &frame(vec![target("t", GREEN), reader("t")]), 64) else { return };
        assert_eq!(at(&green, 16, 16), [0, 255, 0, 255]);
        let Some(again) = draw(&mut r, &frame(vec![target("t", GREEN), reader("t")]), 64) else { return };
        assert_eq!(at(&again, 16, 16), [0, 255, 0, 255], "unchanged children keep the texture");
        let Some(red) = draw(&mut r, &frame(vec![target("t", RED), reader("t")]), 64) else { return };
        assert_eq!(at(&red, 16, 16), [255, 0, 0, 255], "changed children repaint it");
    }

    #[test]
    fn a_material_samples_its_target_as_target_a() {
        let Some(mut r) = renderer() else { return };
        let wgsl = "fn shade(px: Pixel) -> vec4<f32> { return textureSampleLevel(target_a, target_sampler, px.uv, 0.0); }";
        let left = Rect { x: 0.0, y: 32.0, w: 16.0, h: 16.0 };
        let right = Rect { x: 32.0, y: 32.0, w: 16.0, h: 16.0 };
        let both = frame(vec![
            target("g", GREEN),
            target("r", RED),
            sdf(left, WHITE, Some(material(wgsl, &["g"]))),
            sdf(right, WHITE, Some(material(wgsl, &["r"]))),
        ]);
        let Some(pixels) = draw(&mut r, &both, 64) else { return };
        assert_eq!(at(&pixels, 8, 40), [0, 255, 0, 255], "the first shape samples the green target");
        assert_eq!(at(&pixels, 40, 40), [255, 0, 0, 255], "the next shape, same WGSL, samples the red one");
        let none = frame(vec![
            target("g", GREEN),
            sdf(left, WHITE, Some(material(wgsl, &[]))),
            sdf(right, WHITE, Some(material(wgsl, &["nope"]))),
        ]);
        let Some(empty) = draw(&mut r, &none, 64) else { return };
        assert_eq!(at(&empty, 8, 40)[3], 0, "control: no targets, nothing sampled");
        assert_eq!(at(&empty, 40, 40)[3], 0, "control: an unknown target, nothing sampled");
    }
}
