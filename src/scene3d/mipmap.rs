//! GPU mip chains, as three r160 has WebGL2 build them for every texture it samples (`generateMipmaps`, any size,
//! `LinearMipmapLinearFilter`): each level is a render pass that box-filters the level above it.
//!
//! Integration: create a texture with `mip_level_count(width, height)` levels and `TEXTURE_BINDING |
//! RENDER_ATTACHMENT | COPY_DST` usage, upload level 0, call [`MipmapGenerator::generate`] on the same encoder (or one
//! submitted after the upload), and sample it with `mipmap_filter: Linear`.

use std::collections::HashMap;
use std::sync::Arc;

/// The full chain down to 1x1: `floor(log2(max(width, height))) + 1`.
pub fn mip_level_count(width: u32, height: u32) -> u32 {
    32 - width.max(height).max(1).leading_zeros()
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MipmapError {
    #[error("the texture lacks {0:?} usage, which mip generation renders and samples through")]
    Usage(wgpu::TextureUsages),
    #[error("mip generation needs a single-sampled, single-layer 2D texture")]
    Shape,
}

pub struct MipmapGenerator {
    device: Arc<wgpu::Device>,
    shader: wgpu::ShaderModule,
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    sampler: wgpu::Sampler,
    pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
}

impl MipmapGenerator {
    pub fn new(device: Arc<wgpu::Device>) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene3d mipmap"),
            source: wgpu::ShaderSource::Wgsl(include_str!("mipmap.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene3d mipmap"),
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
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene3d mipmap"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("scene3d mipmap"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            device,
            shader,
            layout,
            pipeline_layout,
            sampler,
            pipelines: HashMap::new(),
        }
    }

    fn pipeline(&mut self, format: wgpu::TextureFormat) -> &wgpu::RenderPipeline {
        let (device, shader, layout) = (&self.device, &self.shader, &self.pipeline_layout);
        self.pipelines.entry(format).or_insert_with(|| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("scene3d mipmap"),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some("vs_mip"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some("fs_mip"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
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
            })
        })
    }

    /// Fills levels `1..texture.mip_level_count()` from level 0. The format must be filterable and renderable.
    pub fn generate(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        texture: &wgpu::Texture,
    ) -> Result<(), MipmapError> {
        let needed = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT;
        if !texture.usage().contains(needed) {
            return Err(MipmapError::Usage(needed - texture.usage()));
        }
        if texture.dimension() != wgpu::TextureDimension::D2
            || texture.depth_or_array_layers() != 1
            || texture.sample_count() != 1
        {
            return Err(MipmapError::Shape);
        }
        self.pipeline(texture.format());
        let pipeline = &self.pipelines[&texture.format()];
        let level_view = |level: u32| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("scene3d mip level"),
                base_mip_level: level,
                mip_level_count: Some(1),
                ..Default::default()
            })
        };
        for level in 1..texture.mip_level_count() {
            let source = level_view(level - 1);
            let destination = level_view(level);
            let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("scene3d mipmap"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&source),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene3d mipmap"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &destination,
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
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        Ok(())
    }
}

/// Readback and half-float helpers the scene3d GPU tests share.
#[cfg(test)]
pub(super) mod test_support {
    /// The texels of `level` (tightly packed rows), after submitting `encoder`.
    pub fn read_level(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mut encoder: wgpu::CommandEncoder,
        texture: &wgpu::Texture,
        level: u32,
    ) -> Vec<u8> {
        let texel = texture
            .format()
            .block_copy_size(None)
            .expect("a color format") as usize;
        let (width, height) = (
            (texture.width() >> level).max(1),
            (texture.height() >> level).max(1),
        );
        let row = width as usize * texel;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
        let padded = row.div_ceil(align) * align;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene3d test readback"),
            size: (padded * height as usize) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: level,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded as u32),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        device.poll(wgpu::PollType::wait()).expect("poll");
        rx.recv().expect("map callback").expect("map");
        let data = slice.get_mapped_range();
        let mut out = Vec::with_capacity(row * height as usize);
        for y in 0..height as usize {
            out.extend_from_slice(&data[y * padded..y * padded + row]);
        }
        out
    }

    pub fn f16_to_f32(bits: u16) -> f32 {
        let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
        let exponent = ((bits >> 10) & 0x1f) as i32;
        let mantissa = (bits & 0x3ff) as f32;
        match exponent {
            0 => sign * mantissa * 2f32.powi(-24),
            31 if mantissa == 0.0 => sign * f32::INFINITY,
            31 => f32::NAN,
            e => sign * (1.0 + mantissa / 1024.0) * 2f32.powi(e - 15),
        }
    }

    /// Exact for zero and for normal half floats whose mantissa fits ten bits (the values the tests write).
    pub fn f32_to_f16(value: f32) -> u16 {
        if value == 0.0 {
            return 0;
        }
        let bits = value.to_bits();
        let sign = ((bits >> 16) & 0x8000) as u16;
        let exponent = ((bits >> 23) & 0xff) as i32 - 127 + 15;
        assert!(
            (1..31).contains(&exponent),
            "{value} is not a normal half float"
        );
        assert_eq!(
            bits & 0x1fff,
            0,
            "{value} loses mantissa bits as a half float"
        );
        sign | ((exponent as u16) << 10) | ((bits >> 13) & 0x3ff) as u16
    }

    /// RGBA half floats of `level`, decoded.
    pub fn read_rgba16f(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: wgpu::CommandEncoder,
        texture: &wgpu::Texture,
        level: u32,
    ) -> Vec<[f32; 4]> {
        let bytes = read_level(device, queue, encoder, texture, level);
        bytes
            .chunks_exact(8)
            .map(|t| {
                let h = |i: usize| f16_to_f32(u16::from_le_bytes([t[i], t[i + 1]]));
                [h(0), h(2), h(4), h(6)]
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::read_level;
    use super::*;

    #[test]
    fn the_chain_runs_to_one_texel_like_gl() {
        assert_eq!(mip_level_count(1, 1), 1);
        assert_eq!(mip_level_count(4, 4), 3);
        assert_eq!(mip_level_count(5, 3), 3);
        assert_eq!(mip_level_count(256, 1), 9);
        assert_eq!(mip_level_count(300, 200), 9);
        assert_eq!(
            mip_level_count(0, 0),
            1,
            "edge: an empty size still has its base level"
        );
    }

    fn quarter_white(
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
    ) -> Option<(crate::device::SharedDevice, wgpu::Texture)> {
        let shared = crate::device::shared_device().ok()?;
        let size = wgpu::Extent3d {
            width: 4,
            height: 4,
            depth_or_array_layers: 1,
        };
        let texture = shared.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("mipmap test"),
            size,
            mip_level_count: mip_level_count(4, 4),
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        });
        let mut texels = vec![0u8; 4 * 4 * 4];
        for y in 0..4 {
            for x in 0..4 {
                let i = (y * 4 + x) * 4;
                let white = x < 2 && y < 2;
                texels[i..i + 4].copy_from_slice(if white { &[255; 4] } else { &[0, 0, 0, 255] });
            }
        }
        shared.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &texels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(16),
                rows_per_image: Some(4),
            },
            size,
        );
        Some((shared, texture))
    }

    const USAGE: wgpu::TextureUsages = wgpu::TextureUsages::TEXTURE_BINDING
        .union(wgpu::TextureUsages::RENDER_ATTACHMENT)
        .union(wgpu::TextureUsages::COPY_DST)
        .union(wgpu::TextureUsages::COPY_SRC);

    #[test]
    fn each_level_box_filters_the_one_above() {
        let Some((shared, texture)) = quarter_white(wgpu::TextureFormat::Rgba8Unorm, USAGE) else {
            return;
        };
        let mut generator = MipmapGenerator::new(shared.device.clone());
        let mut encoder = shared.device.create_command_encoder(&Default::default());
        generator
            .generate(&mut encoder, &texture)
            .expect("generate");
        shared.queue.submit([encoder.finish()]);

        let level1 = read_level(
            &shared.device,
            &shared.queue,
            shared.device.create_command_encoder(&Default::default()),
            &texture,
            1,
        );
        assert_eq!(
            &level1[0..4],
            &[255, 255, 255, 255],
            "the white quarter collapses to one white texel"
        );
        assert_eq!(
            &level1[4..8],
            &[0, 0, 0, 255],
            "control: its black neighbour stays black"
        );
        let level2 = read_level(
            &shared.device,
            &shared.queue,
            shared.device.create_command_encoder(&Default::default()),
            &texture,
            2,
        );
        for channel in &level2[0..3] {
            assert!(
                (63..=65).contains(channel),
                "a quarter of white averages to 64: {level2:?}"
            );
        }
    }

    #[test]
    fn an_srgb_texture_averages_in_linear_light() {
        let Some((shared, texture)) = quarter_white(wgpu::TextureFormat::Rgba8UnormSrgb, USAGE)
        else {
            return;
        };
        let mut generator = MipmapGenerator::new(shared.device.clone());
        let mut encoder = shared.device.create_command_encoder(&Default::default());
        generator
            .generate(&mut encoder, &texture)
            .expect("generate");
        let level2 = read_level(&shared.device, &shared.queue, encoder, &texture, 2);
        // linear 0.25 encodes to sRGB 0.537 = 137, where an encoded-space average would give 64
        for channel in &level2[0..3] {
            assert!((136..=138).contains(channel), "{level2:?}");
        }
    }

    #[test]
    fn a_texture_that_cannot_be_rendered_into_is_refused() {
        let usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
        let Some((shared, texture)) = quarter_white(wgpu::TextureFormat::Rgba8Unorm, usage) else {
            return;
        };
        let mut generator = MipmapGenerator::new(shared.device.clone());
        let mut encoder = shared.device.create_command_encoder(&Default::default());
        assert_eq!(
            generator.generate(&mut encoder, &texture),
            Err(MipmapError::Usage(wgpu::TextureUsages::RENDER_ATTACHMENT))
        );
    }
}
