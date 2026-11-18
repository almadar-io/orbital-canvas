//! Backdrop blurs: what lies behind a shape, blurred and tinted, under it.

use super::*;

/// One backdrop shape resolved for the pass: the blur job and the bind groups of its three draws.
pub(super) struct BackdropDraw {
    pub(super) job: crate::backdrop::BackdropJob,
    pub(super) key: u64,
    pub(super) bind_group: wgpu::BindGroup,
    pub(super) aux: wgpu::BindGroup,
}

impl SharedDeviceRenderer {
    /// Resolve a backdrop shape into its draw; `None` when it is off the frame or its material does not compile.
    pub(super) fn backdrop_draw(
        &mut self,
        shape: &DrawCommand,
        backdrop: &orbital_canvas_base::packet::SdfBackdrop,
        packet: &GameRenderPacket,
        set: &crate::backdrop::LayerSet,
    ) -> Option<Draw> {
        let DrawCommand::Sdf { rect, rotation, .. } = shape else {
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
        let (device, queue, format) = (self.device.clone(), self.queue.clone(), self.format);
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
            &queue,
            &[instance],
            (packet.width, packet.height),
            packet.time_ms,
            packet.pointer,
            packet.scroll,
        );
        let behind = set.b.create_view(&wgpu::TextureViewDescriptor::default());
        let aux = pass.aux_group(&device, Some(&behind), target.as_ref().map(|t| &t.view));
        Some(Draw::Backdrop(Box::new(BackdropDraw {
            job: crate::backdrop::BackdropJob {
                frame: set.frame.clone(),
                a: set.a.clone(),
                b: set.b.clone(),
                sigma: backdrop.blur * 0.5 * scale.0,
                reach: crate::backdrop::BACKDROP_REACH,
                saturate: backdrop.saturate,
                tint: backdrop.tint,
                region,
                straight_output: false,
            },
            key,
            bind_group,
            aux,
        })))
    }

    /// Blur the frame under a backdrop shape, then clear the frame where it covers, add the blur there and paint
    /// the shape over it.
    pub(super) fn encode_backdrop(&self, encoder: &mut wgpu::CommandEncoder, draw: &BackdropDraw) {
        let Some(blur) = self.blur.as_ref() else {
            return;
        };
        let Some(sdf) = self.sdf.as_ref() else {
            return;
        };
        let (Some((cut, add)), Some(fill)) = (sdf.backdrop_pipelines(), sdf.pipeline(draw.key))
        else {
            return;
        };
        let mut kept = Vec::new();
        blur.encode(&self.device, encoder, &draw.job, &mut kept);
        for buffer in &kept {
            self.keep(buffer);
        }
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
}
