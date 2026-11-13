//! Backdrop blur passes (rasm floor contract 14.6). A frame that holds a backdrop shape is painted into an internal
//! frame texture instead of the host's target, so the pass can read what is painted so far (a host target need not
//! be readable). For each backdrop shape the frame is blurred horizontally into `a`, then vertically into `b` where
//! saturate and tint are applied; the shape then clears the frame under its coverage and adds `b` there. The frame
//! texture is copied into the host target at the end. Render passes only, so it runs on WebGL2.

use crate::packet::Rect;
use wgpu::util::DeviceExt;

const BLUR_SHADER: &str = r#"
struct Params {
  a: vec4<f32>,
  b: vec4<f32>,
  tint: vec4<f32>,
}
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var smp: sampler;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
  var corners = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
  return vec4<f32>(corners[vi], 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
  let uv = pos.xy / vec2<f32>(textureDimensions(src));
  let sigma = max(p.a.z, 0.0001);
  let taps = i32(p.a.w);
  var sum = vec4<f32>(0.0);
  var total = 0.0;
  for (var i = -taps; i <= taps; i = i + 1) {
    let off = f32(i) * p.b.x;
    let w = exp(-(off * off) / (2.0 * sigma * sigma));
    sum = sum + textureSampleLevel(src, smp, uv + p.a.xy * off, 0.0) * w;
    total = total + w;
  }
  var c = sum / total;
  if (p.b.z > 0.5) {
    var straight = vec4<f32>(0.0);
    if (c.a > 0.0) {
      straight = vec4<f32>(c.rgb / c.a, c.a);
    }
    let lum = dot(straight.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    let rgb = clamp(mix(vec3<f32>(lum), straight.rgb, p.b.y), vec3<f32>(0.0), vec3<f32>(1.0));
    c = vec4<f32>(rgb * straight.a, straight.a);
    c = vec4<f32>(p.tint.rgb * p.tint.a, p.tint.a) + c * (1.0 - p.tint.a);
  }
  return c;
}
"#;

const BLIT_SHADER: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
  var corners = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
  return vec4<f32>(corners[vi], 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
  return textureLoad(src, vec2<i32>(pos.xy), 0);
}
"#;

/// The most taps either side of a pixel in one blur pass; a wider kernel widens the stride between taps.
const MAX_TAPS: f32 = 32.0;

/// Taps per side and the stride between them (texels) for a Gaussian of `sigma` physical pixels.
pub(crate) fn kernel(sigma: f32) -> (u32, f32) {
    let radius = (3.0 * sigma.max(0.0)).ceil();
    if radius > MAX_TAPS {
        (MAX_TAPS as u32, radius / MAX_TAPS)
    } else {
        (radius as u32, 1.0)
    }
}

/// The physical pixel box `[x, y, w, h]` of a rect turned about its centre, padded by `pad` logical pixels and
/// clamped to a `size` target; `None` when nothing of it is on the target.
pub(crate) fn region(
    rect: Rect,
    rotation: f32,
    pad: f32,
    scale: (f32, f32),
    size: (u32, u32),
) -> Option<[u32; 4]> {
    let (hw, hh) = (rect.w * 0.5, rect.h * 0.5);
    let (cs, sn) = (rotation.cos().abs(), rotation.sin().abs());
    let ext = (hw * cs + hh * sn + pad, hw * sn + hh * cs + pad);
    let (cx, cy) = (rect.x + hw, rect.y + hh);
    let x0 = ((cx - ext.0) * scale.0).floor().max(0.0);
    let y0 = ((cy - ext.1) * scale.1).floor().max(0.0);
    let x1 = ((cx + ext.0) * scale.0).ceil().min(size.0 as f32);
    let y1 = ((cy + ext.1) * scale.1).ceil().min(size.1 as f32);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some([x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32])
}

/// What one backdrop shape needs: the frame to read and the two scratch textures, with the look it gives them.
pub(crate) struct BackdropJob {
    pub(crate) frame: wgpu::Texture,
    pub(crate) a: wgpu::Texture,
    pub(crate) b: wgpu::Texture,
    pub(crate) sigma: f32,
    pub(crate) saturate: f32,
    pub(crate) tint: Option<[f32; 4]>,
    pub(crate) region: [u32; 4],
}

/// The frame texture and the two blur textures of one size and format, reused across frames.
#[derive(Clone)]
pub(crate) struct LayerSet {
    pub(crate) frame: wgpu::Texture,
    pub(crate) a: wgpu::Texture,
    pub(crate) b: wgpu::Texture,
}

#[derive(Default)]
pub(crate) struct LayerPool {
    held: Option<((u32, u32, wgpu::TextureFormat), LayerSet)>,
}

impl LayerPool {
    pub(crate) fn get(
        &mut self,
        device: &wgpu::Device,
        size: (u32, u32),
        format: wgpu::TextureFormat,
    ) -> LayerSet {
        let key = (size.0, size.1, format);
        if let Some((held, set)) = &self.held {
            if *held == key {
                return set.clone();
            }
        }
        let make = |label| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
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
        let set = LayerSet {
            frame: make("backdrop frame"),
            a: make("backdrop blur a"),
            b: make("backdrop blur b"),
        };
        self.held = Some((key, set.clone()));
        set
    }
}

pub(crate) struct BlurPass {
    blur: wgpu::RenderPipeline,
    blur_layout: wgpu::BindGroupLayout,
    blit: wgpu::RenderPipeline,
    blit_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

fn fullscreen_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
    source: &str,
) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("backdrop shader"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("backdrop pipeline layout"),
        bind_group_layouts: &[layout],
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("backdrop pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

fn texture_entry(binding: u32, filterable: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

impl BlurPass {
    pub(crate) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let blur_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("backdrop blur layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                texture_entry(1, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("backdrop blit layout"),
            entries: &[texture_entry(0, false)],
        });
        Self {
            blur: fullscreen_pipeline(device, &blur_layout, format, BLUR_SHADER),
            blit: fullscreen_pipeline(device, &blit_layout, format, BLIT_SHADER),
            blur_layout,
            blit_layout,
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("backdrop sampler"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn blur_pass(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::Texture,
        dst: &wgpu::Texture,
        direction: (f32, f32),
        job: &BackdropJob,
        last: bool,
        scissor: [u32; 4],
    ) {
        let (taps, stride) = kernel(job.sigma);
        let tint = job.tint.unwrap_or([0.0; 4]);
        let params: [f32; 12] = [
            direction.0 / src.width() as f32,
            direction.1 / src.height() as f32,
            job.sigma,
            taps as f32,
            stride,
            job.saturate,
            if last { 1.0 } else { 0.0 },
            0.0,
            tint[0],
            tint[1],
            tint[2],
            tint[3],
        ];
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("backdrop blur params"),
            contents: bytemuck::cast_slice(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let src_view = src.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("backdrop blur bind group"),
            layout: &self.blur_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&src_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("backdrop blur"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &dst_view,
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
        pass.set_pipeline(&self.blur);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.set_scissor_rect(scissor[0], scissor[1], scissor[2], scissor[3]);
        pass.draw(0..3, 0..1);
    }

    /// Blur the frame under `job.region` into `job.b`: horizontally into `a` over the rows the vertical pass will
    /// read, then vertically (with saturate and tint) into `b`.
    pub(crate) fn encode(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        job: &BackdropJob,
    ) {
        let (taps, stride) = kernel(job.sigma);
        let reach = (taps as f32 * stride).ceil() as u32 + 1;
        let [x, y, w, h] = job.region;
        let top = y.saturating_sub(reach);
        let bottom = (y + h + reach).min(job.frame.height());
        self.blur_pass(
            device,
            encoder,
            &job.frame,
            &job.a,
            (1.0, 0.0),
            job,
            false,
            [x, top, w, bottom - top],
        );
        self.blur_pass(
            device,
            encoder,
            &job.a,
            &job.b,
            (0.0, 1.0),
            job,
            true,
            job.region,
        );
    }

    /// Copy the internal frame into the host target, replacing whatever it held.
    pub(crate) fn blit(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        frame: &wgpu::Texture,
        target: &wgpu::TextureView,
    ) {
        let frame_view = frame.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("backdrop blit bind group"),
            layout: &self.blit_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&frame_view),
            }],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("backdrop blit"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            occlusion_query_set: None,
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.blit);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: fn(f32, f32, f32, f32) -> Rect = |x, y, w, h| Rect { x, y, w, h };

    #[test]
    fn the_kernel_reaches_three_sigma_and_a_wide_one_strides_instead_of_adding_taps() {
        assert_eq!(kernel(0.0), (0, 1.0), "no blur is one tap");
        assert_eq!(kernel(4.0), (12, 1.0));
        let (taps, stride) = kernel(100.0);
        assert_eq!(taps, 32);
        assert!(
            (taps as f32 * stride - 300.0).abs() < 0.01,
            "the kernel still spans three sigma"
        );
    }

    #[test]
    fn a_region_pads_scales_clamps_and_vanishes_off_target() {
        assert_eq!(
            region(R(10.0, 10.0, 20.0, 20.0), 0.0, 2.0, (1.0, 1.0), (64, 64)),
            Some([8, 8, 24, 24])
        );
        assert_eq!(
            region(R(10.0, 10.0, 20.0, 20.0), 0.0, 0.0, (2.0, 2.0), (64, 64)),
            Some([20, 20, 40, 40]),
            "density scales"
        );
        assert_eq!(
            region(R(-10.0, -10.0, 20.0, 20.0), 0.0, 0.0, (1.0, 1.0), (64, 64)),
            Some([0, 0, 10, 10]),
            "clamped"
        );
        assert_eq!(
            region(R(100.0, 0.0, 20.0, 20.0), 0.0, 0.0, (1.0, 1.0), (64, 64)),
            None,
            "wholly off target"
        );
        let turned = region(
            R(20.0, 28.0, 24.0, 8.0),
            std::f32::consts::FRAC_PI_2,
            0.0,
            (1.0, 1.0),
            (64, 64),
        )
        .unwrap();
        assert_eq!(
            (turned[2], turned[3]),
            (8, 24),
            "a quarter turn swaps the box"
        );
    }
}
