//! MeshPhysicalMaterial `transmission`/`ior` as three r160 draws it (`renderTransmissionPass`): before the frame's
//! own pass, the opaque meshes render (no tone mapping, linear) into a 4x multisampled half-float target the size of
//! the drawing buffer, cleared to the clear color, or to white at alpha 0.5 when the clear alpha is below 1; the
//! resolve gets a full mip chain (`LinearMipmapLinearFilter`). Transmissive meshes then sample it through
//! `getIBLVolumeRefraction`, blurred by roughness through the mips. Double-sided transmissive meshes first draw their
//! back faces into the same target (sampling the opaque-only resolve) and the target is resolved and mipmapped again.
//!
//! Integration in `render.rs` (when any mesh has a physical material with `transmission > 0`):
//! 1. Lists as three's `WebGLRenderList`: transmission > 0 is "transmissive" whatever its opacity; opaque, then
//!    transmissive (sorted like opaque), then transparent with the grid.
//! 2. Shader: append [`WGSL`] after `environment::IBL_WGSL`; add [`Transmission::layout_entries`]`(5)` to the frame
//!    layout. Object uniform gains transmission, ior and thickness. In `fs_main` for kind 2 with transmission > 0:
//!    keep direct and indirect diffuse apart from specular, then
//!    `let t = transmission_volume_refraction(n, v, roughness, s.diffuse, specular.rgb, specular.w, in.world,
//!    object.model, frame.view_proj, ior, thickness, vec3(1.0), 0.0, false);`
//!    `total_diffuse = mix(total_diffuse, t.rgb, transmission)`, and the output alpha is
//!    `base.a * mix(1.0, t.a, transmission)`.
//! 3. Frame: [`Transmission::begin_opaque`], draw the opaque meshes with pipelines keyed by ([`FORMAT`], [`SAMPLES`]),
//!    `options.y = 0`, `options.z = 0`, the frame bind group binding [`Transmission::placeholder_view`] at 5; then
//!    [`Transmission::end_opaque`]. If any transmissive mesh is `Side::Double`: [`Transmission::begin_back_faces`],
//!    draw those meshes with `Side::Back` pipelines and a frame bind group binding [`Transmission::opaque_view`], then
//!    [`Transmission::end_back_faces`]. The lit pass binds [`Transmission::view`].
//!
//! WebGL2 renders into [`FORMAT`] only with `EXT_color_buffer_float`; three falls back to 8-bit without it.

use super::mipmap::{mip_level_count, MipmapError, MipmapGenerator};
use std::sync::Arc;

pub const WGSL: &str = include_str!("transmission.wgsl");
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub const SAMPLES: u32 = 4;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

struct Targets {
    size: (u32, u32),
    msaa: wgpu::TextureView,
    depth: wgpu::TextureView,
    opaque: wgpu::Texture,
    back: wgpu::Texture,
}

pub struct Transmission {
    device: Arc<wgpu::Device>,
    mips: MipmapGenerator,
    sampler: wgpu::Sampler,
    placeholder: wgpu::TextureView,
    targets: Option<Targets>,
    with_back_faces: bool,
}

fn level0(texture: &wgpu::Texture) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("scene3d transmission resolve"),
        base_mip_level: 0,
        mip_level_count: Some(1),
        ..Default::default()
    })
}

impl Transmission {
    pub fn new(device: Arc<wgpu::Device>) -> Self {
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("scene3d transmission"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let placeholder = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("scene3d no transmission"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        Self {
            mips: MipmapGenerator::new(device.clone()),
            device,
            sampler,
            placeholder,
            targets: None,
            with_back_faces: false,
        }
    }

    /// The color the target clears to for a scene `background` (linear RGBA): three's clear color, premultiplied as
    /// WebGL clears, or white at alpha 0.5 when the alpha is below 1.
    pub fn clear_color(background: [f32; 4]) -> wgpu::Color {
        let [r, g, b, a] = if background[3] < 1.0 {
            [1.0, 1.0, 1.0, 0.5]
        } else {
            background
        };
        wgpu::Color {
            r: (r * a) as f64,
            g: (g * a) as f64,
            b: (b * a) as f64,
            a: a as f64,
        }
    }

    fn ensure(&mut self, width: u32, height: u32) {
        let size = (width.max(1), height.max(1));
        if self.targets.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let extent = wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        };
        let attachment = |label: &str, format: wgpu::TextureFormat| {
            self.device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: extent,
                    mip_level_count: 1,
                    sample_count: SAMPLES,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let resolved = |label: &str| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: mip_level_count(size.0, size.1),
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        };
        self.targets = Some(Targets {
            size,
            msaa: attachment("scene3d transmission msaa", FORMAT),
            depth: attachment("scene3d transmission depth", DEPTH),
            opaque: resolved("scene3d transmission opaque"),
            back: resolved("scene3d transmission back faces"),
        });
    }

    /// Clears the `width` x `height` target and opens the pass the opaque meshes draw in (color [`FORMAT`] at
    /// [`SAMPLES`] samples, depth `Depth32Float`).
    pub fn begin_opaque<'e>(
        &mut self,
        encoder: &'e mut wgpu::CommandEncoder,
        width: u32,
        height: u32,
        background: [f32; 4],
    ) -> wgpu::RenderPass<'e> {
        self.ensure(width, height);
        self.with_back_faces = false;
        let targets = self.targets.as_ref().expect("just ensured");
        let resolve = level0(&targets.opaque);
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene3d transmission opaque"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &targets.msaa,
                depth_slice: None,
                resolve_target: Some(&resolve),
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(Self::clear_color(background)),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &targets.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        })
    }

    /// After the opaque pass has ended: the resolve's mip chain.
    pub fn end_opaque(&mut self, encoder: &mut wgpu::CommandEncoder) -> Result<(), MipmapError> {
        let targets = self.targets.as_ref().expect("begin_opaque first");
        self.mips.generate(encoder, &targets.opaque)
    }

    /// Reopens the multisampled target (color and depth kept) for double-sided transmissive meshes' back faces, which
    /// sample [`Transmission::opaque_view`]; it resolves into a second texture so that one stays readable.
    pub fn begin_back_faces<'e>(
        &mut self,
        encoder: &'e mut wgpu::CommandEncoder,
    ) -> wgpu::RenderPass<'e> {
        self.with_back_faces = true;
        let targets = self.targets.as_ref().expect("begin_opaque first");
        let resolve = level0(&targets.back);
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene3d transmission back faces"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &targets.msaa,
                depth_slice: None,
                resolve_target: Some(&resolve),
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &targets.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        })
    }

    pub fn end_back_faces(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<(), MipmapError> {
        let targets = self.targets.as_ref().expect("begin_opaque first");
        self.mips.generate(encoder, &targets.back)
    }

    /// The opaque-only resolve, all mips.
    pub fn opaque_view(&self) -> wgpu::TextureView {
        let targets = self.targets.as_ref().expect("begin_opaque first");
        targets.opaque.create_view(&Default::default())
    }

    /// What this frame's transmissive meshes sample: the back-face resolve when back faces were drawn, else the
    /// opaque one.
    pub fn view(&self) -> wgpu::TextureView {
        let targets = self.targets.as_ref().expect("begin_opaque first");
        let texture = if self.with_back_faces {
            &targets.back
        } else {
            &targets.opaque
        };
        texture.create_view(&Default::default())
    }

    /// Bound in place of the target while drawing into it, and when no mesh transmits.
    pub fn placeholder_view(&self) -> &wgpu::TextureView {
        &self.placeholder
    }

    pub fn sampler(&self) -> &wgpu::Sampler {
        &self.sampler
    }

    /// The target (`first`) and its trilinear sampler (`first + 1`) for a fragment-stage bind group layout.
    pub fn layout_entries(first: u32) -> [wgpu::BindGroupLayoutEntry; 2] {
        [
            wgpu::BindGroupLayoutEntry {
                binding: first,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: first + 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::super::mipmap::test_support::read_rgba16f;
    use super::*;

    #[test]
    fn an_opaque_background_clears_premultiplied_and_a_see_through_one_clears_half_white() {
        let opaque = Transmission::clear_color([0.2, 0.4, 0.6, 1.0]);
        assert_eq!((opaque.r, opaque.a), (0.2f32 as f64, 1.0));
        for alpha in [0.0, 0.5, 0.99] {
            let c = Transmission::clear_color([0.2, 0.4, 0.6, alpha]);
            assert_eq!([c.r, c.g, c.b, c.a], [0.5; 4], "alpha {alpha}");
        }
    }

    #[test]
    fn the_target_resolves_and_mipmaps_what_the_opaque_pass_drew() {
        let Some(shared) = crate::device::shared_device().ok() else {
            return;
        };
        let (device, queue) = (&shared.device, &shared.queue);
        let mut t = Transmission::new(device.clone());
        let mut encoder = device.create_command_encoder(&Default::default());
        drop(t.begin_opaque(&mut encoder, 64, 48, [0.25, 0.5, 0.75, 1.0]));
        t.end_opaque(&mut encoder).expect("mips");
        let opaque = t.targets.as_ref().expect("targets").opaque.clone();
        assert_eq!(opaque.mip_level_count(), 7);
        assert_eq!(opaque.format(), FORMAT);
        let top = read_rgba16f(device, queue, encoder, &opaque, 6);
        assert_eq!(
            top,
            vec![[0.25, 0.5, 0.75, 1.0]],
            "the 1x1 level averages the clear"
        );
        assert!(
            !t.with_back_faces,
            "control: without back faces the lit pass samples the opaque resolve"
        );

        let mut encoder = device.create_command_encoder(&Default::default());
        drop(t.begin_back_faces(&mut encoder));
        t.end_back_faces(&mut encoder).expect("mips");
        let back = t.targets.as_ref().expect("targets").back.clone();
        let top = read_rgba16f(device, queue, encoder, &back, 6);
        assert_eq!(
            top,
            vec![[0.25, 0.5, 0.75, 1.0]],
            "the back-face pass keeps the opaque content"
        );
        assert!(t.with_back_faces);
    }

    #[test]
    fn the_snippet_compiles_into_the_lit_shader_with_the_ibl_one() {
        let Some(shared) = crate::device::shared_device().ok() else {
            return;
        };
        let device = &shared.device;
        let probe = r#"
@fragment
fn fs_transmission_probe(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
  let n = vec3<f32>(0.0, 0.0, 1.0);
  let roughness = ibl_material_roughness(0.5, n);
  let specular = ibl_specular(vec3<f32>(1.0), 0.0, 1.5, true);
  return transmission_volume_refraction(n, n, roughness, vec3<f32>(1.0), specular.rgb, specular.w, at.xyz,
    mat4x4<f32>(), mat4x4<f32>(), 1.5, 0.0, vec3<f32>(1.0), 0.0, false);
}
"#;
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let _module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("transmission probe"),
            source: wgpu::ShaderSource::Wgsl(
                [super::super::render::lit_source().as_str(), probe]
                    .concat()
                    .into(),
            ),
        });
        let errors = pollster::block_on(device.pop_error_scope());
        assert!(errors.is_none(), "{errors:?}");
    }
}
