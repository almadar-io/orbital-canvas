//! The glow pass, canvas `shadowBlur` under a flat 3D shape ([`super::model::MeshInstance::glow`]). Each glowing mesh
//! gets a tile of a per-frame atlas over its coverage padded by three sigmas, in its own local XY plane; its coverage
//! is drawn there and blurred there with its own Gaussian sigma, along x and then along y, so the blur follows the
//! shape's plane as the 2D painter's follows the screen. The lit pass then draws the tile as a quad in the glow color.

use super::model::MeshData;

/// Coverage lives in the alpha channel, with white color so the lit pass multiplies only alpha.
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Texels per local unit at most: the 2D painter's pixels per cell.
const DENSITY: f32 = crate::projector::TILE_WIDTH;
/// A tile's longest side and a blur's sigma stay within these, coarsening the tile's texels when they would not.
const MAX_TILE_TEXELS: f32 = 256.0;
const MAX_SIGMA_TEXELS: f32 = 16.0;
const GUTTER: u32 = 1;
/// The atlas fits the WebGL2 texture limit.
const ATLAS_SIDE: u32 = 2048;
const UNIFORM_STRIDE: u64 = 256;

/// One glow's tile: its local rect (min x, min y, width, height), its atlas texels (x, y, width, height) and its
/// sigma in texels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tile {
    pub rect: [f32; 4],
    pub texels: [u32; 4],
    pub sigma: f32,
}

impl Tile {
    /// The tile in normalized atlas coordinates: x, y, width, height.
    pub fn uv(&self, atlas: [u32; 2]) -> [f32; 4] {
        let [x, y, w, h] = self.texels.map(|t| t as f32);
        let (aw, ah) = (atlas[0] as f32, atlas[1] as f32);
        [x / aw, y / ah, w / aw, h / ah]
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum GlowError {
    #[error("glow {0}: {1}")]
    Invalid(usize, String),
    #[error("{0} glows do not fit a {ATLAS_SIDE} x {ATLAS_SIDE} atlas")]
    Full(usize),
}

/// Lay out one tile per glow (its mesh and its sigma in local units) and the atlas size that holds them, packed in
/// shelves left to right, top to bottom.
pub fn plan(glows: &[(&MeshData, f32)]) -> Result<(Vec<Tile>, [u32; 2]), GlowError> {
    let mut tiles = Vec::with_capacity(glows.len());
    let (mut x, mut y, mut row, mut width) = (0u32, 0u32, 0u32, 1u32);
    for (i, (mesh, sigma)) in glows.iter().enumerate() {
        if !(sigma.is_finite() && *sigma > 0.0) {
            return Err(GlowError::Invalid(
                i,
                format!("sigma {sigma} is not above zero"),
            ));
        }
        let mut lo = [f32::INFINITY; 2];
        let mut hi = [f32::NEG_INFINITY; 2];
        for p in &mesh.positions {
            for axis in 0..2 {
                lo[axis] = lo[axis].min(p[axis]);
                hi[axis] = hi[axis].max(p[axis]);
            }
        }
        if mesh.indices.is_empty()
            || !(lo[0].is_finite() && hi[0].is_finite() && lo[1].is_finite() && hi[1].is_finite())
        {
            return Err(GlowError::Invalid(i, "the mesh covers nothing".into()));
        }
        let pad = 3.0 * sigma;
        let (w, h) = (hi[0] - lo[0] + 2.0 * pad, hi[1] - lo[1] + 2.0 * pad);
        let density = DENSITY
            .min(MAX_TILE_TEXELS / w.max(h))
            .min(MAX_SIGMA_TEXELS / sigma);
        let (tw, th) = (
            (w * density).ceil().max(1.0) as u32,
            (h * density).ceil().max(1.0) as u32,
        );
        // the rect grows to whole texels about its center, so a texel is 1/density across both axes
        let (rw, rh) = (tw as f32 / density, th as f32 / density);
        let rect = [
            (lo[0] + hi[0]) / 2.0 - rw / 2.0,
            (lo[1] + hi[1]) / 2.0 - rh / 2.0,
            rw,
            rh,
        ];
        if x > 0 && x + tw > ATLAS_SIDE {
            (x, y, row) = (0, y + row + GUTTER, 0);
        }
        if x + tw > ATLAS_SIDE || y + th > ATLAS_SIDE {
            return Err(GlowError::Full(glows.len()));
        }
        tiles.push(Tile {
            rect,
            texels: [x, y, tw, th],
            sigma: sigma * density,
        });
        width = width.max(x + tw);
        row = row.max(th);
        x += tw + GUTTER;
    }
    Ok((tiles, [width, (y + row).max(1)]))
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct TileUniform {
    rect: [f32; 4],
    texels: [f32; 4],
    blur: [f32; 4],
}

struct Atlas {
    size: [u32; 2],
    result: wgpu::Texture,
    across: wgpu::Texture,
}

/// The coverage and blur pipelines and the atlas they draw into, kept across frames.
pub struct GlowPass {
    coverage: wgpu::RenderPipeline,
    blur: wgpu::RenderPipeline,
    tile_layout: wgpu::BindGroupLayout,
    source_layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    capacity: u64,
    atlas: Option<Atlas>,
}

impl GlowPass {
    pub fn new(device: &wgpu::Device, vertex_layout: wgpu::VertexBufferLayout<'static>) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene3d glow"),
            source: wgpu::ShaderSource::Wgsl(include_str!("glow.wgsl").into()),
        });
        let tile_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene3d glow tile"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<TileUniform>() as u64
                    ),
                },
                count: None,
            }],
        });
        let source_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene3d glow source"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let target = [Some(wgpu::ColorTargetState {
            format: FORMAT,
            blend: None,
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let pipeline = |label: &str,
                        layouts: &[&wgpu::BindGroupLayout],
                        vs: &str,
                        fs: &str,
                        buffers: &[wgpu::VertexBufferLayout<'_>]| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: layouts,
                push_constant_ranges: &[],
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    buffers,
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    targets: &target,
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview: None,
                cache: None,
            })
        };
        let coverage = pipeline(
            "scene3d glow coverage",
            &[&tile_layout],
            "vs_coverage",
            "fs_coverage",
            &[vertex_layout],
        );
        let blur = pipeline(
            "scene3d glow blur",
            &[&tile_layout, &source_layout],
            "vs_tile",
            "fs_blur",
            &[],
        );
        Self {
            coverage,
            blur,
            tile_layout,
            source_layout,
            uniforms: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("scene3d glow tiles"),
                size: UNIFORM_STRIDE * 3,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            capacity: 1,
            atlas: None,
        }
    }

    /// Draw each tile's coverage (its mesh's vertex buffer, index buffer and index count, in `tiles` order) and blur
    /// it in place. The returned texture holds every tile's glow in its alpha.
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        size: [u32; 2],
        tiles: &[Tile],
        meshes: &[(&wgpu::Buffer, &wgpu::Buffer, u32)],
    ) -> &wgpu::Texture {
        let fits = self
            .atlas
            .as_ref()
            .is_some_and(|a| a.size[0] >= size[0] && a.size[1] >= size[1]);
        if !fits {
            let grown = match &self.atlas {
                Some(a) => [a.size[0].max(size[0]), a.size[1].max(size[1])],
                None => size,
            };
            let texture = |label: &str| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: grown[0],
                        height: grown[1],
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
            };
            self.atlas = Some(Atlas {
                size: grown,
                result: texture("scene3d glow atlas"),
                across: texture("scene3d glow atlas across"),
            });
        }
        // one slot per tile for the coverage pass and one per blur direction
        let slots = tiles.len() as u64 * 3;
        if slots > self.capacity * 3 {
            self.capacity = (tiles.len() as u64).next_power_of_two();
            self.uniforms = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("scene3d glow tiles"),
                size: UNIFORM_STRIDE * self.capacity * 3,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        let mut bytes = vec![0u8; (UNIFORM_STRIDE * slots.max(1)) as usize];
        for (i, t) in tiles.iter().enumerate() {
            for pass in 0..3 {
                let uniform = TileUniform {
                    rect: t.rect,
                    texels: t.texels.map(|v| v as f32),
                    blur: [t.sigma, (pass == 2) as u32 as f32, 0.0, 0.0],
                };
                let at = ((i * 3 + pass) as u64 * UNIFORM_STRIDE) as usize;
                bytes[at..at + std::mem::size_of::<TileUniform>()]
                    .copy_from_slice(bytemuck::bytes_of(&uniform));
            }
        }
        queue.write_buffer(&self.uniforms, 0, &bytes);
        let tile_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene3d glow tile"),
            layout: &self.tile_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &self.uniforms,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<TileUniform>() as u64),
                }),
            }],
        });
        let atlas = self.atlas.as_ref().expect("just ensured");
        let result_view = atlas.result.create_view(&Default::default());
        let across_view = atlas.across.create_view(&Default::default());
        let source = |view: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("scene3d glow source"),
                layout: &self.source_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                }],
            })
        };
        let viewport = |pass: &mut wgpu::RenderPass<'_>, t: &Tile| {
            let [x, y, w, h] = t.texels;
            pass.set_viewport(x as f32, y as f32, w as f32, h as f32, 0.0, 1.0);
            pass.set_scissor_rect(x, y, w, h);
        };
        {
            let mut pass = begin(encoder, &result_view, "scene3d glow coverage");
            pass.set_pipeline(&self.coverage);
            for (i, (t, (vertices, indices, count))) in tiles.iter().zip(meshes).enumerate() {
                viewport(&mut pass, t);
                pass.set_bind_group(0, &tile_group, &[(i as u64 * 3 * UNIFORM_STRIDE) as u32]);
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..*count, 0, 0..1);
            }
        }
        for (direction, (from, to)) in [(&result_view, &across_view), (&across_view, &result_view)]
            .into_iter()
            .enumerate()
        {
            let from_group = source(from);
            let mut pass = begin(encoder, to, "scene3d glow blur");
            pass.set_pipeline(&self.blur);
            pass.set_bind_group(1, &from_group, &[]);
            for (i, t) in tiles.iter().enumerate() {
                viewport(&mut pass, t);
                let slot = i as u64 * 3 + 1 + direction as u64;
                pass.set_bind_group(0, &tile_group, &[(slot * UNIFORM_STRIDE) as u32]);
                pass.draw(0..3, 0..1);
            }
        }
        &atlas.result
    }

    /// The atlas size this frame's glows were drawn at.
    pub fn atlas_size(&self) -> Option<[u32; 2]> {
        self.atlas.as_ref().map(|a| a.size)
    }
}

/// A pass that clears `view` and draws into it.
fn begin<'a>(
    encoder: &'a mut wgpu::CommandEncoder,
    view: &'a wgpu::TextureView,
    label: &str,
) -> wgpu::RenderPass<'a> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(side: f32) -> MeshData {
        MeshData {
            positions: vec![
                [0.0, 0.0, 0.0],
                [side, 0.0, 0.0],
                [side, side, 0.0],
                [0.0, side, 0.0],
            ],
            normals: vec![[0.0, 0.0, 1.0]; 4],
            uvs: vec![[0.0, 0.0]; 4],
            indices: vec![0, 1, 2, 0, 2, 3],
        }
    }

    #[test]
    fn a_tile_covers_the_mesh_padded_by_three_sigmas_at_the_painter_density() {
        let mesh = square(1.0);
        let (tiles, atlas) = plan(&[(&mesh, 0.1)]).unwrap();
        let t = tiles[0];
        let side: f32 = 1.0 + 6.0 * 0.1;
        let density = DENSITY
            .min(MAX_TILE_TEXELS / side)
            .min(MAX_SIGMA_TEXELS / 0.1);
        assert_eq!(t.texels[2], (side * density).ceil() as u32);
        assert!(t.rect[0] <= -0.3 + 1e-4 && t.rect[0] + t.rect[2] >= 1.3 - 1e-4);
        assert!((t.sigma - 0.1 * density).abs() < 1e-3);
        assert_eq!(atlas, [t.texels[2], t.texels[3]]);
    }

    #[test]
    fn a_wide_blur_coarsens_its_tile_and_tiles_never_overlap() {
        let (small, big) = (square(0.2), square(4.0));
        let (tiles, _) = plan(&[(&small, 0.05), (&big, 2.0), (&small, 0.05)]).unwrap();
        assert!(
            tiles[1].sigma <= MAX_SIGMA_TEXELS + 1e-3,
            "{}",
            tiles[1].sigma
        );
        assert!(tiles[1].texels[2] as f32 <= MAX_TILE_TEXELS);
        for (i, a) in tiles.iter().enumerate() {
            for b in &tiles[i + 1..] {
                let apart = a.texels[0] + a.texels[2] <= b.texels[0]
                    || b.texels[0] + b.texels[2] <= a.texels[0]
                    || a.texels[1] + a.texels[3] <= b.texels[1]
                    || b.texels[1] + b.texels[3] <= a.texels[1];
                assert!(apart, "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn control_an_empty_mesh_or_no_sigma_is_refused() {
        assert!(matches!(
            plan(&[(&MeshData::default(), 0.1)]),
            Err(GlowError::Invalid(0, _))
        ));
        assert!(matches!(
            plan(&[(&square(1.0), 0.0)]),
            Err(GlowError::Invalid(0, _))
        ));
    }
}
