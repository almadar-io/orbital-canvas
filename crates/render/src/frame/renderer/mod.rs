//! The shared-device painter: one packet drawn into any target texture of a device, and the device resources a
//! host canvas renders with.

use orbital_canvas_base::packet::{DrawCommand, GameRenderPacket, Rect, Vec2};
use std::sync::Arc;
use wgpu::util::DeviceExt;

/// Lazily-initialized GPU resources needed for offscreen rendering.
pub struct DeviceResources {
    #[allow(dead_code)]
    pub adapter: Option<wgpu::Adapter>,
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
    /// Format used by the render pipeline (Rgba8Unorm headless; surface format on-screen).
    pub surface_format: wgpu::TextureFormat,
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
    texture: wgpu::Texture,
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
        tinted: bool,
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

/// Renderer for hosts that own the wgpu device (e.g. dioxus-native/Blitz
/// custom paint): same display-list pipeline as [`Canvas`], but built on
/// the host's shared device/queue and drawing into a caller-provided texture
/// instead of an owned surface or a readback buffer. Handles the full 2D
/// command set: `Clear` + `Quad` through the vertex-colored pipeline and
/// `Sprite` through the textured pipeline (assets arrive via
/// [`orbital_canvas_paint::asset`]; not-yet-loaded sprites are skipped — React parity).
pub struct SharedDeviceRenderer {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    pipeline: wgpu::RenderPipeline,
    sprite_pipeline: wgpu::RenderPipeline,
    additive_sprite_pipeline: wgpu::RenderPipeline,
    tinted_sprite_pipeline: wgpu::RenderPipeline,
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
    /// The horizontal-pass texture of the last GPU-blurred vector layer, reused while the next one has its size.
    blur_scratch: Option<wgpu::Texture>,
    /// Vector layers blurred on the GPU so far (cache observability for tests).
    #[cfg(test)]
    gpu_blurred_layers: u64,
    targets: crate::render_target::TargetStore,
    target_sprite_pipeline: wgpu::RenderPipeline,
    /// Layers clip groups paint into, kept across frames; one per clip group of a frame.
    clip_layers: Vec<clip::ClipLayer>,
    /// Clip masks by shape relative to their box, kept while drawn.
    clip_masks: std::collections::HashMap<u64, clip::ClipMask>,
    /// Top-level frames rendered; stamps what a frame used.
    frame_count: u64,
    /// Buffers made for one frame's draws; destroyed after that frame's submit (wgpu's web backend never frees on drop).
    transient: std::sync::Mutex<Vec<wgpu::Buffer>>,
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
        let tinted_sprite_pipeline = textured_pipeline(
            &device,
            format,
            &sprite_layout,
            TINTED_SPRITE_SHADER_SOURCE,
            wgpu::BlendState::ALPHA_BLENDING,
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
            tinted_sprite_pipeline,
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
            blur_scratch: None,
            #[cfg(test)]
            gpu_blurred_layers: 0,
            targets: Default::default(),
            target_sprite_pipeline,
            clip_layers: Vec::new(),
            clip_masks: std::collections::HashMap::new(),
            frame_count: 0,
            transient: Default::default(),
        }
    }

    fn keep(&self, buffer: &wgpu::Buffer) {
        self.transient
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(buffer.clone());
    }

    fn transient_len(&self) -> usize {
        self.transient
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// Destroy the buffers kept since `mark`; their frame has been submitted.
    fn release_transient(&self, mark: usize) {
        let mut kept = self
            .transient
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for buffer in kept.drain(mark..) {
            buffer.destroy();
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

    /// Draw `packet` into a new `width` x `height` `Rgba8Unorm` texture and read it back (top row first); the
    /// renderer must have been made for that format.
    pub async fn render_packet_to_rgba_async(
        &mut self,
        packet: &GameRenderPacket,
        width: u32,
        height: u32,
    ) -> crate::Result<Vec<u8>> {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
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
        self.render_packet_to_texture(packet, &texture);
        let encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("game readback encoder"),
            });
        crate::texture::copy_texture_to_rgba_async(
            &self.device,
            &self.queue,
            encoder,
            &texture,
            width,
            height,
        )
        .await
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
        let has_area = packet.width > 0.0 && packet.height > 0.0;
        if !has_area && !packet.draw_commands.is_empty() {
            let blank = GameRenderPacket {
                draw_commands: Vec::new(),
                ..packet.clone()
            };
            return self.render_packet_to_texture(&blank, target);
        }
        if self.render_depth == 0 {
            self.frame_epoch = self.use_clock;
            self.frame_count += 1;
            self.prune_clip_surfaces();
            if let Some(glyphs) = self.glyphs.as_mut() {
                glyphs.begin_frame();
            }
            self.targets.begin_frame();
            if let Some(sdf) = self.sdf.as_mut() {
                sdf.begin_frame();
            }
        }
        self.render_depth += 1;
        let transient_mark = self.transient_len();
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

        let mut pending = packet.draw_commands.iter().peekable();
        while let Some(command) = pending.next() {
            match command {
                DrawCommand::Faded {
                    opacity,
                    scale,
                    origin,
                    offset,
                    commands,
                } => {
                    self.flush_vector_run(&mut vector_run, packet, target, &mut draws);
                    if !quad_batch.is_empty() {
                        draws.push(Draw::Quads(std::mem::take(&mut quad_batch)));
                    }
                    let mut layer: Vec<&DrawCommand> = commands.iter().collect();
                    while let Some(DrawCommand::Faded {
                        opacity: next,
                        scale: next_scale,
                        origin: next_origin,
                        offset: next_offset,
                        commands,
                    }) = pending.peek()
                    {
                        if next != opacity
                            || next_scale != scale
                            || next_origin != origin
                            || next_offset != offset
                        {
                            break;
                        }
                        layer.extend(commands.iter());
                        pending.next();
                    }
                    if *opacity > 0.0 {
                        if *scale == 1.0 && *offset == Vec2::default() {
                            self.flush_vector_run_faded(
                                &mut layer, packet, target, &mut draws, false, *opacity,
                            );
                        } else {
                            self.flush_placed_layer(
                                &layer,
                                packet,
                                target,
                                &mut draws,
                                *opacity,
                                (*scale, *origin, *offset),
                            );
                        }
                    }
                }
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
                        Ok(prepared) => {
                            for buffer in &prepared.transient {
                                self.keep(buffer);
                            }
                            draws.push(Draw::Particles(prepared))
                        }
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
                    self.paint_target(id, (*target_w, *target_h), density, *live, commands, packet);
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
                    let Some((sheet_w, sheet_h)) = orbital_canvas_paint::asset::image_info(texture)
                    else {
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
                        (sprite.width, sprite.height),
                        *opacity,
                        (width, height),
                    );
                    draws.push(Draw::Sprite {
                        key,
                        verts: verts.to_vec(),
                        additive: false,
                        tinted: false,
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
                    tint,
                } => {
                    self.flush_vector_run(&mut vector_run, packet, target, &mut draws);
                    if let Some(id) =
                        texture.strip_prefix(orbital_canvas_base::targets::TARGET_PREFIX)
                    {
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
                        let from = if src.w > 0.0 && src.h > 0.0 {
                            let sx = read.width as f32 / read.points.0.max(1) as f32;
                            let sy = read.height as f32 / read.points.1.max(1) as f32;
                            Rect {
                                x: src.x * sx,
                                y: src.y * sy,
                                w: src.w * sx,
                                h: src.h * sy,
                            }
                        } else {
                            whole
                        };
                        let verts = sprite_vertices(
                            &from,
                            dst,
                            (read.width, read.height),
                            *rotation,
                            *opacity,
                            (*flip_x, *flip_y),
                            None,
                            (width, height),
                        );
                        let bind_group =
                            self.device.create_bind_group(&wgpu::BindGroupDescriptor {
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
                        (sprite.width, sprite.height),
                        *rotation,
                        *opacity,
                        (*flip_x, *flip_y),
                        *tint,
                        (width, height),
                    );
                    draws.push(Draw::Sprite {
                        key,
                        verts: verts.to_vec(),
                        additive: false,
                        tinted: tint.is_some(),
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
                            self.keep(&buffer);
                            pass.set_pipeline(&self.pipeline);
                            pass.set_vertex_buffer(0, buffer.slice(..));
                            pass.draw(0..vertices.len() as u32, 0..1);
                            drew = true;
                        }
                        Draw::Sprite {
                            key,
                            verts,
                            additive,
                            tinted,
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
                            self.keep(&buffer);
                            pass.set_pipeline(if *additive {
                                &self.additive_sprite_pipeline
                            } else if *tinted {
                                &self.tinted_sprite_pipeline
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
                            self.keep(&buffer);
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
                            self.keep(&buffer);
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
        self.release_transient(transient_mark);
        self.render_depth -= 1;
        drew
    }
}

pub use crate::texture::copy_texture_to_rgba;

mod backdrops;
mod blur_layers;
mod clip;
mod glyphs;
mod pipelines;
mod sdf_draws;
mod sprites;
mod targets;
mod vector_layers;
mod vertices;
use self::backdrops::*;
pub(crate) use self::pipelines::*;
use self::vertices::*;

#[cfg(test)]
mod scene3d_canvas_tests;

#[cfg(test)]
mod svg_sprite_tests;

#[cfg(test)]
mod vector_layer_tests;

#[cfg(test)]
mod sdf_tests;

#[cfg(test)]
mod layer_tests;
