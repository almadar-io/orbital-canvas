//! The canvas post-processing stack (`post.bloom`, `post.vignette`), drawn as three.js draws it today:
//! @react-three/postprocessing 3.0.4's `EffectComposer` (half-float buffers) running postprocessing 6.39's
//! `BloomEffect` (mipmap blur) and `VignetteEffect` in one `EffectPass`.
//!
//! What three does while that composer is mounted, and so what the frame must do:
//! - `RenderPass` draws the scene into a half-float target, where three applies neither tone mapping nor the sRGB
//!   encode, and the composer forces `renderer.toneMapping = NoToneMapping` besides: a scene with post is never
//!   tone mapped, even when it asks for ACES.
//! - The `EffectPass` renders to the canvas and encodes sRGB last (`colorspace_fragment`), after bloom and vignette.
//!
//! Integration in `render.rs` (only when `bloom` or `vignette` is set; otherwise three mounts no composer):
//! 1. Render the lit pass into [`PostStack::scene_target`] (format [`HDR_FORMAT`]) instead of the frame's target:
//!    the MSAA color texture becomes `HDR_FORMAT` and resolves into that view, the pipelines are keyed by
//!    `HDR_FORMAT`, the frame's `options.y` (ACES) and `options.z` (encode) are 0, and the clear color is the linear
//!    background premultiplied by its alpha (three's `setClearColor`), not encoded.
//! 2. Call [`PostStack::apply`] on the same encoder with that view and the frame's target view; it encodes sRGB in the
//!    shader when the target's format is not an sRGB one, as `finish()` does.
//!
//! WebGL2 renders into `HDR_FORMAT` only with `EXT_color_buffer_float` (three's half-float composer needs the same).

use std::sync::Arc;

pub const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const LEVELS: usize = 8;
const RADIUS: f32 = 0.85;
const SLOT_LUMINANCE: usize = 0;
const SLOT_DOWN: usize = 1;
const SLOT_UP: usize = SLOT_DOWN + LEVELS;
const SLOT_COMPOSITE: usize = SLOT_UP + LEVELS - 1;
const SLOTS: usize = SLOT_COMPOSITE + 1;

/// `<Bloom intensity luminanceThreshold luminanceSmoothing />`; the defaults are Effects3D's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bloom {
    pub intensity: f32,
    pub threshold: f32,
    pub smoothing: f32,
}

impl Default for Bloom {
    fn default() -> Self {
        Self {
            intensity: 1.0,
            threshold: 0.9,
            smoothing: 0.3,
        }
    }
}

/// `<Vignette offset darkness />` (the default technique); the defaults are Effects3D's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vignette {
    pub offset: f32,
    pub darkness: f32,
}

impl Default for Vignette {
    fn default() -> Self {
        Self {
            offset: 0.3,
            darkness: 0.6,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    a: [f32; 4],
    b: [f32; 4],
}

/// MipmapBlurPass's level sizes for a `width` x `height` input: each half of the previous, rounded as JS's
/// `Math.round` rounds.
pub fn bloom_level_sizes(width: u32, height: u32) -> Vec<(u32, u32)> {
    let mut sizes = Vec::with_capacity(LEVELS);
    let (mut w, mut h) = (width, height);
    for _ in 0..LEVELS {
        w = w.div_ceil(2);
        h = h.div_ceil(2);
        sizes.push((w, h));
    }
    sizes
}

struct Level {
    size: (u32, u32),
    view: wgpu::TextureView,
}

struct Chain {
    size: (u32, u32),
    luminance: wgpu::TextureView,
    down: Vec<Level>,
    up: Vec<wgpu::TextureView>,
    down_groups: Vec<wgpu::BindGroup>,
    up_groups: Vec<wgpu::BindGroup>,
}

pub struct PostStack {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    format: wgpu::TextureFormat,
    layout: wgpu::BindGroupLayout,
    luminance: wgpu::RenderPipeline,
    downsample: wgpu::RenderPipeline,
    upsample: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    params: wgpu::Buffer,
    stride: u64,
    chain: Option<Chain>,
    scene: Option<((u32, u32), wgpu::TextureView)>,
}

fn hdr_texture(
    device: &wgpu::Device,
    (width, height): (u32, u32),
    label: &str,
) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

impl PostStack {
    /// A stack that writes into targets of `format` (the frame's target format).
    pub fn new(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        format: wgpu::TextureFormat,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene3d post"),
            source: wgpu::ShaderSource::Wgsl(include_str!("post.wgsl").into()),
        });
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let params_size = std::mem::size_of::<Params>() as u64;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene3d post"),
            entries: &[
                texture_entry(0),
                texture_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(params_size),
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene3d post"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = |entry: &str, target: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_post"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target,
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
        };
        let luminance = pipeline("fs_luminance", HDR_FORMAT);
        let downsample = pipeline("fs_downsample", HDR_FORMAT);
        let upsample = pipeline("fs_upsample", HDR_FORMAT);
        let composite = pipeline("fs_composite", format);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("scene3d post"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let align = device.limits().min_uniform_buffer_offset_alignment as u64;
        let stride = params_size.div_ceil(align) * align;
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene3d post params"),
            size: stride * SLOTS as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            device,
            queue,
            format,
            layout,
            luminance,
            downsample,
            upsample,
            composite,
            sampler,
            params,
            stride,
            chain: None,
            scene: None,
        }
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// The half-float view the lit pass resolves into for a `width` x `height` frame (kept until the size changes).
    pub fn scene_target(&mut self, width: u32, height: u32) -> wgpu::TextureView {
        let size = (width, height);
        if self.scene.as_ref().map(|(s, _)| *s) != Some(size) {
            self.scene = Some((size, hdr_texture(&self.device, size, "scene3d post scene")));
        }
        self.scene
            .as_ref()
            .map(|(_, view)| view.clone())
            .expect("just set")
    }

    fn group(&self, a: &wgpu::TextureView, b: &wgpu::TextureView, slot: usize) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene3d post"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(a),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(b),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.params,
                        offset: slot as u64 * self.stride,
                        size: wgpu::BufferSize::new(std::mem::size_of::<Params>() as u64),
                    }),
                },
            ],
        })
    }

    fn ensure_chain(&mut self, size: (u32, u32)) {
        if self.chain.as_ref().is_some_and(|c| c.size == size) {
            return;
        }
        let luminance = hdr_texture(&self.device, size, "scene3d bloom luminance");
        let down: Vec<Level> = bloom_level_sizes(size.0, size.1)
            .into_iter()
            .map(|s| Level {
                size: s,
                view: hdr_texture(&self.device, s, "scene3d bloom down"),
            })
            .collect();
        let up: Vec<wgpu::TextureView> = down[..LEVELS - 1]
            .iter()
            .map(|level| hdr_texture(&self.device, level.size, "scene3d bloom up"))
            .collect();
        let down_groups = (0..LEVELS)
            .map(|i| {
                let input = if i == 0 {
                    &luminance
                } else {
                    &down[i - 1].view
                };
                self.group(input, input, SLOT_DOWN + i)
            })
            .collect();
        let up_groups = (0..LEVELS - 1)
            .map(|i| {
                let input = if i == LEVELS - 2 {
                    &down[LEVELS - 1].view
                } else {
                    &up[i + 1]
                };
                self.group(input, &down[i].view, SLOT_UP + i)
            })
            .collect();
        self.chain = Some(Chain {
            size,
            luminance,
            down,
            up,
            down_groups,
            up_groups,
        });
    }

    fn write(&self, slot: usize, a: [f32; 4], b: [f32; 4]) {
        self.queue.write_buffer(
            &self.params,
            slot as u64 * self.stride,
            bytemuck::bytes_of(&Params { a, b }),
        );
    }

    /// Bloom, then vignette, then the sRGB encode, from `scene` (a [`HDR_FORMAT`] view of `size`) into `target`
    /// (a view of this stack's format and the same size). With neither effect it only encodes.
    pub fn apply(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        scene: &wgpu::TextureView,
        size: (u32, u32),
        target: &wgpu::TextureView,
        bloom: Option<Bloom>,
        vignette: Option<Vignette>,
    ) {
        let texel = |(w, h): (u32, u32)| [1.0 / w as f32, 1.0 / h as f32];
        let mut bloom_view = None;
        if let Some(b) = bloom {
            self.ensure_chain(size);
            let chain = self.chain.as_ref().expect("just ensured");
            self.write(
                SLOT_LUMINANCE,
                [b.threshold, b.smoothing, 0.0, 0.0],
                [0.0; 4],
            );
            let mut input = size;
            for (i, level) in chain.down.iter().enumerate() {
                let [x, y] = texel(input);
                self.write(SLOT_DOWN + i, [x, y, 0.0, 0.0], [0.0; 4]);
                input = level.size;
            }
            for i in (0..LEVELS - 1).rev() {
                let [x, y] = texel(input);
                self.write(SLOT_UP + i, [x, y, RADIUS, 0.0], [0.0; 4]);
                input = chain.down[i].size;
            }
            let luminance_group = self.group(scene, scene, SLOT_LUMINANCE);
            run(encoder, &chain.luminance, &self.luminance, &luminance_group);
            for (level, group) in chain.down.iter().zip(&chain.down_groups) {
                run(encoder, &level.view, &self.downsample, group);
            }
            for i in (0..LEVELS - 1).rev() {
                run(encoder, &chain.up[i], &self.upsample, &chain.up_groups[i]);
            }
            bloom_view = Some(chain.up[0].clone());
        }
        let b = bloom.unwrap_or_default();
        let v = vignette.unwrap_or_default();
        self.write(
            SLOT_COMPOSITE,
            [
                b.intensity,
                bloom.is_some() as u32 as f32,
                v.offset,
                v.darkness,
            ],
            [
                vignette.is_some() as u32 as f32,
                (!self.format.is_srgb()) as u32 as f32,
                0.0,
                0.0,
            ],
        );
        let group = self.group(scene, bloom_view.as_ref().unwrap_or(scene), SLOT_COMPOSITE);
        run(encoder, target, &self.composite, &group);
    }
}

fn run(
    encoder: &mut wgpu::CommandEncoder,
    target: &wgpu::TextureView,
    pipeline: &wgpu::RenderPipeline,
    group: &wgpu::BindGroup,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("scene3d post"),
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
        timestamp_writes: None,
        occlusion_query_set: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, group, &[]);
    pass.draw(0..3, 0..1);
}

#[cfg(test)]
mod tests {
    use super::super::mipmap::test_support::f32_to_f16;
    use super::*;

    const SIZE: u32 = 64;

    #[test]
    fn bloom_levels_halve_with_js_rounding() {
        assert_eq!(
            bloom_level_sizes(64, 64),
            vec![
                (32, 32),
                (16, 16),
                (8, 8),
                (4, 4),
                (2, 2),
                (1, 1),
                (1, 1),
                (1, 1)
            ]
        );
        assert_eq!(
            bloom_level_sizes(1920, 1080),
            vec![
                (960, 540),
                (480, 270),
                (240, 135),
                (120, 68),
                (60, 34),
                (30, 17),
                (15, 9),
                (8, 5)
            ]
        );
    }

    fn oetf(c: f32) -> f32 {
        if c <= 0.0031308 {
            c * 12.92
        } else {
            c.powf(0.41666) * 1.055 - 0.055
        }
    }

    fn hermite(low: f32, high: f32, x: f32) -> f32 {
        let t = ((x - low) / (high - low)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    /// Runs the stack on an HDR frame whose texel at (x, y) is `fill(x, y)` (gray, alpha 1) into an 8-bit target.
    fn post(
        format: wgpu::TextureFormat,
        fill: impl Fn(u32, u32) -> f32,
        bloom: Option<Bloom>,
        vignette: Option<Vignette>,
    ) -> Option<Vec<u8>> {
        let shared = crate::device::shared_device().ok()?;
        let (device, queue) = (&shared.device, &shared.queue);
        let extent = wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        };
        let scene = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("post test scene"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let mut texels = Vec::with_capacity((SIZE * SIZE * 4) as usize);
        for y in 0..SIZE {
            for x in 0..SIZE {
                let v = f32_to_f16(fill(x, y));
                texels.extend([v, v, v, f32_to_f16(1.0)]);
            }
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &scene,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&texels),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(SIZE * 8),
                rows_per_image: Some(SIZE),
            },
            extent,
        );
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("post test target"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let mut stack = PostStack::new(device.clone(), queue.clone(), format);
        let mut encoder = device.create_command_encoder(&Default::default());
        stack.apply(
            &mut encoder,
            &scene.create_view(&Default::default()),
            (SIZE, SIZE),
            &target.create_view(&Default::default()),
            bloom,
            vignette,
        );
        crate::renderer::copy_texture_to_rgba(device, queue, encoder, &target, SIZE, SIZE).ok()
    }

    fn at(rgba: &[u8], x: u32, y: u32) -> [u8; 4] {
        let i = ((y * SIZE + x) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    }

    fn close(actual: u8, expected: f32) -> bool {
        (actual as f32 - expected * 255.0).abs() <= 1.0
    }

    #[test]
    fn with_no_effect_the_pass_only_encodes_srgb_in_the_shader_or_the_target() {
        let expected = oetf(0.5);
        for format in [
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureFormat::Rgba8UnormSrgb,
        ] {
            let Some(rgba) = post(format, |_, _| 0.5, None, None) else {
                return;
            };
            let p = at(&rgba, 10, 20);
            assert!(close(p[0], expected), "{format:?}: {p:?}");
            assert_eq!(p[3], 255);
        }
    }

    #[test]
    fn bloom_spreads_light_above_the_threshold_and_control_below_it_does_not() {
        let block = |x: u32, y: u32| {
            if (28..36).contains(&x) && (28..36).contains(&y) {
                4.0
            } else {
                0.0
            }
        };
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let Some(plain) = post(format, block, None, None) else {
            return;
        };
        let glowing = post(format, block, Some(Bloom::default()), None).expect("device");
        let above = post(
            format,
            block,
            Some(Bloom {
                threshold: 10.0,
                ..Bloom::default()
            }),
            None,
        )
        .expect("device");
        let outside = (24, 32);
        assert_eq!(
            at(&plain, outside.0, outside.1)[0],
            0,
            "control: no bloom, no glow"
        );
        assert!(
            at(&glowing, outside.0, outside.1)[0] > 20,
            "{:?}",
            at(&glowing, outside.0, outside.1)
        );
        assert_eq!(
            at(&above, outside.0, outside.1)[0],
            0,
            "control: everything under the threshold"
        );
        assert!(
            at(&glowing, 2, 2)[0] < at(&glowing, outside.0, outside.1)[0],
            "the glow falls off with distance"
        );
    }

    #[test]
    fn the_vignette_darkens_corners_by_postprocessings_curve_and_keeps_the_center() {
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let v = Vignette::default();
        let Some(rgba) = post(format, |_, _| 0.5, None, Some(v)) else {
            return;
        };
        let expect = |x: u32, y: u32| {
            let (u, w) = (
                (x as f32 + 0.5) / SIZE as f32,
                (y as f32 + 0.5) / SIZE as f32,
            );
            let d = ((u - 0.5).powi(2) + (w - 0.5).powi(2)).sqrt();
            oetf(0.5 * hermite(0.8, v.offset * 0.799, d * (v.darkness + v.offset)))
        };
        for (x, y) in [(0, 0), (63, 63), (0, 32), (32, 32)] {
            let p = at(&rgba, x, y);
            assert!(
                close(p[0], expect(x, y)),
                "({x}, {y}): {p:?} vs {}",
                expect(x, y) * 255.0
            );
        }
        assert!(at(&rgba, 0, 0)[0] + 40 < at(&rgba, 32, 32)[0]);
    }
}
