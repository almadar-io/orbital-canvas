//! The instanced particle pass: each packed word is a per-instance vertex attribute, so a GPU buffer the program's
//! compute work fills draws as it lies, and a CPU copy uploads once per frame.

use crate::packet::{ParticleSource, Vec2};

pub const MAX_PALETTE: usize = 256;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ParticleUniform {
    viewport: [f32; 2],
    half: f32,
    scale: f32,
    origin: [f32; 2],
    palette_len: f32,
    _pad: f32,
    palette: [[f32; 4]; MAX_PALETTE],
}

const SHADER: &str = r#"
struct Params {
  viewport: vec2<f32>,
  half: f32,
  scale: f32,
  origin: vec2<f32>,
  palette_len: f32,
  pad: f32,
  palette: array<vec4<f32>, 256>,
};
@group(0) @binding(0) var<uniform> p: Params;

struct Out {
  @builtin(position) clip: vec4<f32>,
  @location(0) color: vec4<f32>,
};

@vertex
fn vs(@builtin(vertex_index) corner_index: u32, @location(0) word: u32) -> Out {
  var corners = array<vec2<f32>, 6>(
    vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
    vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0));
  let scene = vec2<f32>(f32(word & 4095u), f32((word >> 12u) & 4095u));
  let at = p.origin + scene * p.scale + corners[corner_index] * p.half * p.scale;
  let index = min(word >> 24u, u32(p.palette_len) - 1u);
  var out: Out;
  out.clip = vec4<f32>(at.x / p.viewport.x * 2.0 - 1.0, 1.0 - at.y / p.viewport.y * 2.0, 0.0, 1.0);
  out.color = p.palette[index];
  return out;
}

@fragment
fn fs(in: Out) -> @location(0) vec4<f32> {
  return in.color;
}
"#;

pub(crate) struct ParticlePass {
    pub(crate) pipelines: std::collections::HashMap<u32, wgpu::RenderPipeline>,
    shader: wgpu::ShaderModule,
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
}

/// One particle draw, ready for the pass.
pub(crate) struct ParticleDraw {
    pub stride: u32,
    pub vertices: wgpu::Buffer,
    pub count: u32,
    pub bind_group: wgpu::BindGroup,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ParticleError {
    #[error("particles: stride {0} is not a multiple of 4 between 4 and 2048")]
    Stride(u32),
    #[error("particles: the palette is empty or longer than {MAX_PALETTE}")]
    Palette,
    #[error("particles: {0}")]
    Gpu(String),
    #[error(
        "particles: the gpu buffer {0} was not created with `vertex` usage, so it cannot be drawn"
    )]
    NotVertex(u64),
}

impl ParticlePass {
    pub(crate) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("particles"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("particles"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<ParticleUniform>() as u64
                    ),
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("particles"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        Self {
            pipelines: Default::default(),
            shader,
            layout,
            pipeline_layout,
            format,
        }
    }

    /// The pipeline for one word stride (the stride is part of the vertex layout).
    pub(crate) fn pipeline(&mut self, device: &wgpu::Device, stride: u32) -> &wgpu::RenderPipeline {
        let (shader, layout, format) = (&self.shader, &self.pipeline_layout, self.format);
        self.pipelines.entry(stride).or_insert_with(|| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("particles"),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some("vs"),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: stride as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &[wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Uint32,
                            offset: 0,
                            shader_location: 0,
                        }],
                    }],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some("fs"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
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

    /// Resolve one `Particles` command: its words' buffer and its parameters, for a `viewport` in packet units.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        source: &ParticleSource,
        stride: u32,
        radius: f32,
        palette: &[[f32; 4]],
        scale: f32,
        origin: Vec2,
        viewport: (f32, f32),
    ) -> Result<ParticleDraw, ParticleError> {
        use wgpu::util::DeviceExt;
        if stride < 4 || stride % 4 != 0 || stride > 2048 {
            return Err(ParticleError::Stride(stride));
        }
        if palette.is_empty() || palette.len() > MAX_PALETTE {
            return Err(ParticleError::Palette);
        }
        let (vertices, bytes) = match source {
            ParticleSource::Bytes { data } => (
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("particle words"),
                    contents: if data.is_empty() { &[0, 0, 0, 0] } else { data },
                    usage: wgpu::BufferUsages::VERTEX,
                }),
                data.len() as u64,
            ),
            ParticleSource::Gpu { id, device: owner } => {
                match crate::gpu::drawable::resolve(*id, *owner) {
                    Ok(crate::gpu::drawable::DrawableGpu::Buffer {
                        buffer,
                        size,
                        usage,
                    }) => {
                        if !usage.contains(wgpu::BufferUsages::VERTEX) {
                            return Err(ParticleError::NotVertex(*id));
                        }
                        (buffer, size)
                    }
                    Ok(crate::gpu::drawable::DrawableGpu::Texture { .. }) => {
                        return Err(ParticleError::Gpu(format!(
                            "handle {id} is a texture, not a buffer"
                        )))
                    }
                    Err(error) => return Err(ParticleError::Gpu(error.to_string())),
                }
            }
        };
        let count = ((bytes + (stride as u64 - 4)) / stride as u64) as u32;
        let mut uniform = ParticleUniform {
            viewport: [viewport.0.max(1.0), viewport.1.max(1.0)],
            half: radius,
            scale,
            origin: [origin.x, origin.y],
            palette_len: palette.len() as f32,
            _pad: 0.0,
            palette: [[0.0; 4]; MAX_PALETTE],
        };
        uniform.palette[..palette.len()].copy_from_slice(palette);
        let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("particle params"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("particles"),
            layout: &self.layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: params.as_entire_binding(),
            }],
        });
        self.pipeline(device, stride);
        Ok(ParticleDraw {
            stride,
            vertices,
            count,
            bind_group,
        })
    }
}
