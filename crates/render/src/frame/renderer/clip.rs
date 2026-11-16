//! Clip groups: a group painted into a layer and drawn through its clip mask.

use super::*;

impl SharedDeviceRenderer {
    /// Render one packet into `target`. The texture must be created on the
    /// same device with `RENDER_ATTACHMENT` usage and the format passed to
    /// [`Self::new`]. Draw commands execute in packet order (z-order); quads
    /// batch between sprites. Returns `false` when nothing drawable was
    /// emitted (the target is still cleared).
    pub(super) fn masked_group_draw(
        &mut self,
        packet: &GameRenderPacket,
        target: &wgpu::Texture,
        clip: &[orbital_canvas_base::packet::PathSegment],
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
        let mask = orbital_canvas_paint::vector::rasterize_clip_mask(clip, tw, th, sx, sy)?;
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
        let verts = sprite_vertices(
            &rect,
            &dst,
            (tw, th),
            0.0,
            1.0,
            (false, false),
            (width, height),
        );
        Some(Draw::MaskedSprite { bind_group, verts })
    }
}
