//! A `Blurred` or `Shadow` run drawn as its own layer: the CPU rasterizes only the unblurred coverage and the
//! backdrop blur pass blurs it on the GPU, once per cached layer.

use super::*;

impl SharedDeviceRenderer {
    /// The blurred layer of one `Blurred` or `Shadow` command over its `lw` x `lh` box; `None` when a texture
    /// cannot be made or `command` is not a blur.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn blurred_layer(
        &mut self,
        command: &DrawCommand,
        lw: u32,
        lh: u32,
        scale: (f32, f32),
        origin: (f32, f32),
        now: u64,
    ) -> Option<GpuSprite> {
        let input = orbital_canvas_paint::vector::rasterize_blur_input(
            command, lw, lh, scale.0, scale.1, origin.0, origin.1,
        )?;
        let device = self.device.clone();
        let format = self.format;
        let source = self.upload_rgba_texture(&input.rgba, lw, lh);
        let target = |label| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: lw,
                    height: lh,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };
        let scratch = match self.blur_scratch.take() {
            Some(held) if (held.width(), held.height()) == (lw, lh) => held,
            Some(held) => {
                held.destroy();
                target("blur layer scratch")
            }
            None => target("blur layer scratch"),
        };
        let out = target("blur layer");
        self.blur
            .get_or_insert_with(|| crate::backdrop::BlurPass::new(&device, format));
        let blur = self.blur.as_ref()?;
        let job = crate::backdrop::BackdropJob {
            frame: source.clone(),
            a: scratch.clone(),
            b: out.clone(),
            sigma: input.sigma,
            reach: orbital_canvas_paint::vector::BLUR_REACH,
            saturate: 1.0,
            tint: None,
            region: [0, 0, lw, lh],
            straight_output: true,
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("blur layer"),
        });
        let mut kept = Vec::new();
        blur.encode(&device, &mut encoder, &job, &mut kept);
        self.queue.submit(std::iter::once(encoder.finish()));
        for buffer in &kept {
            self.keep(buffer);
        }
        source.destroy();
        self.blur_scratch = Some(scratch);
        #[cfg(test)]
        {
            self.gpu_blurred_layers += 1;
        }
        Some(self.sprite_from_texture(out, lw, lh, now))
    }

    /// GPU-blurred vector layers made so far (cache observability for tests).
    #[cfg(test)]
    pub(super) fn gpu_blurred_layer_count(&self) -> u64 {
        self.gpu_blurred_layers
    }
}
