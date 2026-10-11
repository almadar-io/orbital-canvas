//! GPU pipeline and offscreen rendering for [`GameRenderer`].

use crate::packet::{DrawCommand, GameRenderPacket, Rect, Vec2};
use crate::{GameRenderer, RendererError, Result};
use std::sync::Arc;
use wgpu::util::DeviceExt;

fn align_up(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}

/// Lazily-initialized GPU resources needed for offscreen rendering.
pub(crate) struct DeviceResources {
    #[allow(dead_code)]
    pub adapter: wgpu::Adapter,
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
/// format. Shared by [`GameRenderer`] (own-device headless/surface paths) and
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

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("game sprite shader"),
        source: wgpu::ShaderSource::Wgsl(SPRITE_SHADER_SOURCE.into()),
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("game sprite pipeline layout"),
        bind_group_layouts: &[&layout],
        push_constant_ranges: &[],
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("game sprite pipeline"),
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
    });

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
    Sprite {
        key: SpriteKey,
        verts: [SpriteVertex; 6],
    },
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
/// custom paint): same display-list pipeline as [`GameRenderer`], but built on
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
    sprite_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    sprites: std::collections::HashMap<SpriteKey, GpuSprite>,
    /// Bumped on every sprite request; stamps `GpuSprite::last_used`.
    use_clock: u64,
    /// `use_clock` when the current frame began: sprites stamped after it are
    /// referenced by this frame's draw list and must not be evicted.
    frame_epoch: u64,
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
            sprite_layout,
            sampler,
            sprites: std::collections::HashMap::new(),
            use_clock: 0,
            frame_epoch: 0,
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
        // A host face added since the layer was drawn changes how its text shapes.
        std::hash::Hash::hash(&crate::vector::font_generation(), &mut hasher);
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
        draws.push(Draw::Sprite { key, verts });
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

    /// Upload straight-alpha RGBA8 as a sampled texture with its bind group.
    fn upload_rgba(&self, rgba: &[u8], width: u32, height: u32, now: u64) -> GpuSprite {
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
    pub fn render_packet_to_texture(
        &mut self,
        packet: &GameRenderPacket,
        target: &wgpu::Texture,
    ) -> bool {
        self.frame_epoch = self.use_clock;
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
                DrawCommand::Poly { points, color } => {
                    self.flush_vector_run(&mut vector_run, packet, target, &mut draws);
                    push_poly_fan(&mut quad_batch, points, color, width, height);
                }
                DrawCommand::Path { .. }
                | DrawCommand::ClippedPath { .. }
                | DrawCommand::Line { .. }
                | DrawCommand::Text { .. } => {
                    if !quad_batch.is_empty() {
                        draws.push(Draw::Quads(std::mem::take(&mut quad_batch)));
                    }
                    vector_run.push(command);
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
                    draws.push(Draw::Sprite { key, verts });
                }
                _ => {}
            }
        }
        self.flush_vector_run(&mut vector_run, packet, target, &mut draws);
        if !quad_batch.is_empty() {
            draws.push(Draw::Quads(quad_batch));
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("game shared-device pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear_color),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
            });

            for draw in &draws {
                match draw {
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
                    Draw::Sprite { key, verts } => {
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
                        pass.set_pipeline(&self.sprite_pipeline);
                        pass.set_bind_group(0, &sprite.bind_group, &[]);
                        pass.set_vertex_buffer(0, buffer.slice(..));
                        pass.draw(0..6, 0..1);
                        drew = true;
                    }
                }
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
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

impl GameRenderer {
    /// Ensure a `wgpu` adapter, device, queue, and pipeline are available.
    fn ensure_device_resources(&mut self) -> Result<&DeviceResources> {
        if self.device_resources.is_none() {
            let adapter =
                pollster::block_on(self.instance.request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    compatible_surface: self.surface.as_ref(),
                    force_fallback_adapter: false,
                }))
                .map_err(|_| RendererError::NoAdapter)?;

            let (device, queue) = pollster::block_on(
                adapter.request_device(&wgpu::DeviceDescriptor {
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                        .using_resolution(adapter.limits()),
                    label: Some("orbital-game-renderer"),
                    memory_hints: wgpu::MemoryHints::Performance,
                    trace: wgpu::Trace::Off,
                }),
            )
            .map_err(|e| RendererError::DeviceRequestFailed(e.to_string()))?;

            let device = Arc::new(device);
            let queue = Arc::new(queue);

            // Use the surface's preferred format when on-screen, fall back to Rgba8Unorm headless.
            let surface_format = self
                .surface
                .as_ref()
                .and_then(|s| {
                    // Packet colors are already sRGB-encoded (CSS values): an sRGB target would
                    // encode them a second time and wash every color out.
                    let formats = s.get_capabilities(&adapter).formats;
                    formats
                        .iter()
                        .copied()
                        .find(|f| !f.is_srgb())
                        .or_else(|| formats.first().copied())
                })
                .unwrap_or(wgpu::TextureFormat::Rgba8Unorm);

            self.device_resources = Some(DeviceResources {
                adapter,
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
    /// top-to-bottom row order matching `egui`/image conventions.
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

    fn fixture(name: &str, contents: &[u8]) -> String {
        let dir = std::env::temp_dir().join(format!(
            "orbital-game-renderer-svg-sprite-{}",
            std::process::id()
        ));
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
    fn an_identical_packet_reuses_the_cached_layer_and_a_changed_one_adds_one() {
        let Some(mut r) = renderer() else { return };
        let _font = crate::vector::FONT_GENERATION_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _ = draw(&mut r, &packet(vec![filled(4.0, RED)]), 16);
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
            mono: false,
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
