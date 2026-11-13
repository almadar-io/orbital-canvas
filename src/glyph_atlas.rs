//! The glyph atlas: plain text drawn on the GPU. Each glyph is rasterized once (tiny-skia, from its outline) into an
//! R8 coverage page and every run is instanced quads tinted by the text colour. Shaping, bidi and joining stay in
//! `vector.rs`; this file only places the shaped glyphs and caches their coverage.
//!
//! Pages are shelf packed. A glyph is keyed by (face, glyph id, size in quarter pixels, faux bold, origin quarter
//! pixel on each axis). When no page has room and the page budget is spent, the least recently used page that this
//! frame has not drawn from is cleared and refilled; when every page was drawn from this frame the pass grows past
//! the budget for that frame and trims back at the next one.

use crate::packet::DrawCommand;
use crate::vector::{self, FontId};
use std::collections::HashMap;
use wgpu::util::DeviceExt;

const SHADER: &str = include_str!("glyph_atlas.wgsl");
const PAGE_SIZE: u32 = 1024;
const MAX_PAGES: usize = 4;
const QUARTERS: f32 = 4.0;

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct GlyphInstance {
    rect: [f32; 4],
    uv: [f32; 4],
    color: [f32; 4],
}

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
struct GlyphKey {
    font: FontId,
    gid: u16,
    size: u32,
    faux_bold: bool,
    qx: u8,
    qy: u8,
}

/// Where a glyph's coverage sits; `page` is `None` for a glyph with no ink (a space).
#[derive(Copy, Clone)]
struct Entry {
    page: Option<u64>,
    x: u32,
    y: u32,
    left: i32,
    top: i32,
    width: u32,
    height: u32,
}

struct Shelf {
    y: u32,
    height: u32,
    x: u32,
}

struct Page {
    id: u64,
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    shelves: Vec<Shelf>,
    next_y: u32,
    last_used: u64,
}

impl Page {
    fn alloc(&mut self, size: u32, w: u32, h: u32) -> Option<(u32, u32)> {
        let fit = self
            .shelves
            .iter_mut()
            .filter(|s| s.height >= h && s.x + w <= size)
            .min_by_key(|s| s.height - h);
        if let Some(shelf) = fit {
            let at = (shelf.x, shelf.y);
            shelf.x += w;
            return Some(at);
        }
        if self.next_y + h > size {
            return None;
        }
        let y = self.next_y;
        self.next_y += h;
        self.shelves.push(Shelf { y, height: h, x: w });
        Some((0, y))
    }
}

pub(crate) struct GlyphPass {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    pages: Vec<Page>,
    entries: HashMap<GlyphKey, Entry>,
    page_size: u32,
    max_pages: usize,
    frame: u64,
    next_page: u64,
    rasterized: u64,
}

impl GlyphPass {
    pub(crate) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        Self::with_limits(device, format, PAGE_SIZE, MAX_PAGES)
    }

    pub(crate) fn with_limits(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        page_size: u32,
        max_pages: usize,
    ) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("glyph atlas layout"),
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
            label: Some("glyph atlas shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("glyph atlas pipeline layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("glyph atlas pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GlyphInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &attributes,
                }],
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
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("glyph atlas sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            pipeline,
            layout,
            sampler,
            pages: Vec::new(),
            entries: HashMap::new(),
            page_size,
            max_pages,
            frame: 1,
            next_page: 0,
            rasterized: 0,
        }
    }

    pub(crate) fn pipeline(&self) -> &wgpu::RenderPipeline {
        &self.pipeline
    }

    pub(crate) fn bind_group(&self, page: u64) -> Option<&wgpu::BindGroup> {
        self.pages
            .iter()
            .find(|p| p.id == page)
            .map(|p| &p.bind_group)
    }

    /// Glyphs rasterized since the pass was made (cache observability for tests).
    #[cfg(test)]
    pub(crate) fn rasterized(&self) -> u64 {
        self.rasterized
    }

    #[cfg(test)]
    pub(crate) fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Starts a frame: pages past the budget (grown for the last frame) go, least recently used first.
    pub(crate) fn begin_frame(&mut self) {
        self.frame += 1;
        while self.pages.len() > self.max_pages.max(1) {
            let Some(victim) = self
                .pages
                .iter()
                .enumerate()
                .min_by_key(|(_, p)| p.last_used)
                .map(|(i, _)| i)
            else {
                return;
            };
            let id = self.pages.remove(victim).id;
            self.entries.retain(|_, e| e.page != Some(id));
        }
    }

    fn new_page(&mut self, device: &wgpu::Device) -> usize {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph atlas page"),
            size: wgpu::Extent3d {
                width: self.page_size,
                height: self.page_size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("glyph atlas page"),
            layout: &self.layout,
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
        self.next_page += 1;
        self.pages.push(Page {
            id: self.next_page,
            texture,
            bind_group,
            shelves: Vec::new(),
            next_y: 0,
            last_used: self.frame,
        });
        self.pages.len() - 1
    }

    /// A `w` x `h` cell: in a page with room, else in a new page under the budget, else in the least recently used
    /// page this frame has not drawn from (cleared), else in a new page past the budget.
    fn allocate(&mut self, device: &wgpu::Device, w: u32, h: u32) -> Option<(usize, u32, u32)> {
        if w > self.page_size || h > self.page_size {
            return None;
        }
        let size = self.page_size;
        for (i, page) in self.pages.iter_mut().enumerate() {
            if let Some((x, y)) = page.alloc(size, w, h) {
                return Some((i, x, y));
            }
        }
        let index = if self.pages.len() < self.max_pages {
            self.new_page(device)
        } else {
            let frame = self.frame;
            let victim = self
                .pages
                .iter()
                .enumerate()
                .filter(|(_, p)| p.last_used < frame)
                .min_by_key(|(_, p)| p.last_used)
                .map(|(i, _)| i);
            match victim {
                Some(i) => {
                    let id = self.pages[i].id;
                    self.entries.retain(|_, e| e.page != Some(id));
                    self.pages[i].shelves.clear();
                    self.pages[i].next_y = 0;
                    i
                }
                None => self.new_page(device),
            }
        };
        let (x, y) = self.pages[index].alloc(size, w, h)?;
        Some((index, x, y))
    }

    /// The entry for `key`, rasterized into a page on a miss; `None` when its cell cannot fit a page.
    fn ensure(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: GlyphKey,
    ) -> Option<Entry> {
        if let Some(entry) = self.entries.get(&key).copied() {
            if let Some(id) = entry.page {
                let frame = self.frame;
                if let Some(page) = self.pages.iter_mut().find(|p| p.id == id) {
                    page.last_used = frame;
                }
            }
            return Some(entry);
        }
        let frac = (f32::from(key.qx) / QUARTERS, f32::from(key.qy) / QUARTERS);
        let bitmap = vector::rasterize_glyph(
            key.font,
            key.gid,
            key.size as f32 / QUARTERS,
            key.faux_bold,
            frac,
        );
        self.rasterized += 1;
        let Some(bitmap) = bitmap else {
            let entry = Entry {
                page: None,
                x: 0,
                y: 0,
                left: 0,
                top: 0,
                width: 0,
                height: 0,
            };
            self.entries.insert(key, entry);
            return Some(entry);
        };
        let (padded_w, padded_h) = (bitmap.width + 1, bitmap.height + 1);
        let (index, x, y) = self.allocate(device, padded_w, padded_h)?;
        let mut cell = vec![0u8; (padded_w * padded_h) as usize];
        for (row, src) in bitmap
            .coverage
            .chunks_exact(bitmap.width as usize)
            .enumerate()
        {
            let start = row * padded_w as usize;
            cell[start..start + src.len()].copy_from_slice(src);
        }
        let frame = self.frame;
        let page = &mut self.pages[index];
        page.last_used = frame;
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &page.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            &cell,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_w),
                rows_per_image: Some(padded_h),
            },
            wgpu::Extent3d {
                width: padded_w,
                height: padded_h,
                depth_or_array_layers: 1,
            },
        );
        let entry = Entry {
            page: Some(page.id),
            x,
            y,
            left: bitmap.left,
            top: bitmap.top,
            width: bitmap.width,
            height: bitmap.height,
        };
        self.entries.insert(key, entry);
        Some(entry)
    }

    /// The instances that draw `command` (a `Text` without a halo) onto a `target` px surface, `scale` px per
    /// packet unit, grouped by atlas page in first-use order. `None` when the command is not a plain text, or a
    /// glyph is too large for a page (the caller draws it on the CPU).
    pub(crate) fn place_text(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        command: &DrawCommand,
        scale: (f32, f32),
        target: (u32, u32),
    ) -> Option<Vec<(u64, Vec<GlyphInstance>)>> {
        let DrawCommand::Text {
            text,
            pos,
            size,
            color,
            align,
            weight,
            halo: None,
            family,
        } = command
        else {
            return None;
        };
        let bold = *weight >= 600;
        let layout = vector::text_layout(text, *pos, *size, *align, bold, *family)?;
        let px = *size * (scale.0 + scale.1) / 2.0;
        let size_q = (px * QUARTERS).round().max(1.0) as u32;
        let color = color.map(|c| c.clamp(0.0, 1.0));
        let (tw, th) = (target.0 as f32, target.1 as f32);
        let quarter = |v: f32| {
            let q = (v * QUARTERS).round() as i64;
            (q.div_euclid(4) as i32, q.rem_euclid(4) as u8)
        };
        let mut runs: Vec<(u64, Vec<GlyphInstance>)> = Vec::new();
        for g in &layout.shaped.glyphs {
            let (ix, qx) = quarter((layout.x0 + g.x) * scale.0);
            let (iy, qy) = quarter((layout.baseline + g.y) * scale.1);
            let key = GlyphKey {
                font: g.font,
                gid: g.gid,
                size: size_q,
                faux_bold: bold && g.font.faux_bold(),
                qx,
                qy,
            };
            let entry = self.ensure(device, queue, key)?;
            let Some(page) = entry.page else { continue };
            let left = (ix + entry.left) as f32;
            let top = (iy + entry.top) as f32;
            let ps = self.page_size as f32;
            let instance = GlyphInstance {
                rect: [
                    left / tw * 2.0 - 1.0,
                    1.0 - top / th * 2.0,
                    (left + entry.width as f32) / tw * 2.0 - 1.0,
                    1.0 - (top + entry.height as f32) / th * 2.0,
                ],
                uv: [
                    entry.x as f32 / ps,
                    entry.y as f32 / ps,
                    (entry.x + entry.width) as f32 / ps,
                    (entry.y + entry.height) as f32 / ps,
                ],
                color,
            };
            match runs.iter_mut().find(|(p, _)| *p == page) {
                Some((_, list)) => list.push(instance),
                None => runs.push((page, vec![instance])),
            }
        }
        Some(runs)
    }

    /// A vertex buffer of `instances` for one draw.
    pub(crate) fn instance_buffer(
        device: &wgpu::Device,
        instances: &[GlyphInstance],
    ) -> wgpu::Buffer {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("glyph instances"),
            contents: bytemuck::cast_slice(instances),
            usage: wgpu::BufferUsages::VERTEX,
        })
    }
}
