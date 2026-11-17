//! Clip groups: a group painted into a layer and drawn over its clip's box, through a mask unless the clip is a
//! rectangle on whole pixels.

use super::*;

/// A clip group's layer, the size of the target, kept for the frames that use it.
pub(super) struct ClipLayer {
    key: (u32, u32, wgpu::TextureFormat),
    texture: wgpu::Texture,
    used_in: u64,
}

/// A clip path's coverage over its own box.
pub(super) struct ClipMask {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    used_in: u64,
}

/// Frames a clip mask is kept without being drawn.
const CLIP_MASK_FRAMES: u64 = 120;

/// A clip mask covers its clip's whole box (so it is reused as the clip scrolls across the edge) while that box is at
/// most this many targets in area.
const WHOLE_CLIP_TARGETS: u64 = 2;

impl SharedDeviceRenderer {
    /// Destroy clip layers no frame drew into since the last one and masks not drawn for [`CLIP_MASK_FRAMES`].
    pub(super) fn prune_clip_surfaces(&mut self) {
        let now = self.frame_count;
        self.clip_layers.retain(|layer| {
            let keep = layer.used_in + 1 >= now;
            if !keep {
                layer.texture.destroy();
            }
            keep
        });
        self.clip_masks.retain(|_, mask| {
            let keep = mask.used_in + CLIP_MASK_FRAMES >= now;
            if !keep {
                mask.texture.destroy();
            }
            keep
        });
    }

    fn clip_layer(&mut self, key: (u32, u32, wgpu::TextureFormat)) -> wgpu::Texture {
        let now = self.frame_count;
        if let Some(layer) = self
            .clip_layers
            .iter_mut()
            .find(|layer| layer.key == key && layer.used_in != now)
        {
            layer.used_in = now;
            return layer.texture.clone();
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("game clipped group surface"),
            size: wgpu::Extent3d {
                width: key.0,
                height: key.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: key.2,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        self.clip_layers.push(ClipLayer {
            key,
            texture: texture.clone(),
            used_in: now,
        });
        texture
    }

    /// Paint a clip group's commands into a layer and queue the layer over the clip's box: a rectangle on whole
    /// pixels needs nothing more, any other clip draws through its coverage mask, cached by its shape relative to
    /// its box so a scrolled or moved clip keeps it.
    pub(super) fn masked_group_draw(
        &mut self,
        packet: &GameRenderPacket,
        target: &wgpu::Texture,
        clip: &[orbital_canvas_base::packet::PathSegment],
        commands: &[DrawCommand],
    ) -> Option<Draw> {
        let (tw, th) = (target.width(), target.height());
        let max_dim = self.device.limits().max_texture_dimension_2d;
        if tw > max_dim || th > max_dim {
            return None;
        }
        let sx = tw as f32 / packet.width.max(1.0);
        let sy = th as f32 / packet.height.max(1.0);
        let bounds = orbital_canvas_base::path_data::path_bounds(clip)?;
        let whole = (
            (bounds.x * sx).floor() as i64,
            (bounds.y * sy).floor() as i64,
            ((bounds.x + bounds.w) * sx).ceil() as i64,
            ((bounds.y + bounds.h) * sy).ceil() as i64,
        );
        let shown = (
            whole.0.clamp(0, tw as i64),
            whole.1.clamp(0, th as i64),
            whole.2.clamp(0, tw as i64),
            whole.3.clamp(0, th as i64),
        );
        if shown.2 <= shown.0 || shown.3 <= shown.1 {
            return None;
        }
        let layer = self.clip_layer((tw, th, target.format()));
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
        let layer_view = layer.create_view(&wgpu::TextureViewDescriptor::default());
        let width = packet.width.max(1.0) as u32;
        let height = packet.height.max(1.0) as u32;
        let src = Rect {
            x: shown.0 as f32,
            y: shown.1 as f32,
            w: (shown.2 - shown.0) as f32,
            h: (shown.3 - shown.1) as f32,
        };
        let dst = Rect {
            x: src.x / sx,
            y: src.y / sy,
            w: src.w / sx,
            h: src.h / sy,
        };
        let mut verts = sprite_vertices(
            &src,
            &dst,
            (tw, th),
            0.0,
            1.0,
            (false, false),
            None,
            (width, height),
        );
        let whole_pixels = |v: f32| v.fract() == 0.0;
        let exact_rect = orbital_canvas_base::path_data::axis_rect(clip).is_some_and(|r| {
            whole_pixels(r.x * sx)
                && whole_pixels(r.y * sy)
                && whole_pixels((r.x + r.w) * sx)
                && whole_pixels((r.y + r.h) * sy)
        });
        if exact_rect {
            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("game clipped group bind group"),
                layout: &self.sprite_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&layer_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            return Some(Draw::TargetSprite { bind_group, verts });
        }
        let (mw, mh) = (whole.2 - whole.0, whole.3 - whole.1);
        let fits = mw <= max_dim as i64
            && mh <= max_dim as i64
            && (mw as u64) * (mh as u64) <= WHOLE_CLIP_TARGETS * tw as u64 * th as u64;
        let mask_box = if fits { whole } else { shown };
        let (mw, mh) = (
            (mask_box.2 - mask_box.0) as u32,
            (mask_box.3 - mask_box.1) as u32,
        );
        let (ox, oy) = (mask_box.0 as f32 / sx, mask_box.1 as f32 / sy);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(&(mw, mh, sx.to_bits(), sy.to_bits()), &mut hasher);
        let outline = DrawCommand::Path {
            segments: clip.to_vec(),
            fill: None,
            stroke: None,
        };
        orbital_canvas_base::targets::hash_commands_from(
            [&outline],
            Vec2 { x: ox, y: oy },
            &mut hasher,
        )
        .ok()?;
        let key = std::hash::Hasher::finish(&hasher);
        let now = self.frame_count;
        if !self.clip_masks.contains_key(&key) {
            let rgba =
                orbital_canvas_paint::vector::rasterize_clip_mask(clip, mw, mh, sx, sy, ox, oy)?;
            let texture = self.upload_rgba_texture(&rgba, mw, mh);
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.clip_masks.insert(
                key,
                ClipMask {
                    texture,
                    view,
                    used_in: now,
                },
            );
        }
        let mask = self.clip_masks.get_mut(&key)?;
        mask.used_in = now;
        // The masked shader reads the mask's coordinates from the vertex colour.
        for vert in &mut verts {
            let (x, y) = (vert.uv[0] * tw as f32, vert.uv[1] * th as f32);
            vert.color = [
                (x - mask_box.0 as f32) / mw as f32,
                (y - mask_box.1 as f32) / mh as f32,
                0.0,
                1.0,
            ];
        }
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
                    resource: wgpu::BindingResource::TextureView(&mask.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        Some(Draw::MaskedSprite { bind_group, verts })
    }

    /// Clip masks currently cached (cache observability for tests).
    #[cfg(test)]
    pub(super) fn clip_mask_count(&self) -> usize {
        self.clip_masks.len()
    }

    /// Clip layers currently kept (cache observability for tests).
    #[cfg(test)]
    pub(super) fn clip_layer_count(&self) -> usize {
        self.clip_layers.len()
    }
}
