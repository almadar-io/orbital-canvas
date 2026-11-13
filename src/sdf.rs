//! The SDF pass: rounded rects, capsules, arcs, polygons, stars and ellipses drawn as instanced quads whose
//! fragment stage evaluates the shape's signed distance. Instances ride one uniform array, so the pass needs no
//! compute and no storage buffers and runs on WebGL2. A shape's fill is the program's `shade` (2D materials).

use crate::packet::{DrawCommand, SdfKind, SdfMaterial, Vec2};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use wgpu::util::DeviceExt;

const PRELUDE: &str = include_str!("sdf.wgsl");
const PLAIN_SHADE: &str = "\nfn shade(px: Pixel) -> vec4<f32> { return px.color; }\n";

/// Shapes per uniform buffer; the WGSL array holds `MAX_INSTANCES * FIELDS` vec4 slots.
pub(crate) const MAX_INSTANCES: usize = 64;
const FIELDS: usize = 12;
const HEADER_FLOATS: usize = 8;
const _: () = assert!(MAX_INSTANCES * FIELDS == 768);

/// The pipeline key of the plain (no material) pass.
pub(crate) const PLAIN_KEY: u64 = 0;

/// One shape's packed uniform slots.
#[derive(Debug, Copy, Clone, PartialEq)]
pub(crate) struct SdfInstance(pub(crate) [[f32; 4]; FIELDS]);

impl SdfInstance {
    /// The instance and material of an [`DrawCommand::Sdf`]; `None` for any other command.
    pub(crate) fn from_command(command: &DrawCommand) -> Option<(Self, Option<&SdfMaterial>)> {
        let DrawCommand::Sdf {
            kind,
            rect,
            corners,
            corner_kind,
            arc,
            polygon,
            fill,
            stroke,
            shadow,
            rotation,
            opacity,
            material,
        } = command
        else {
            return None;
        };
        let shape = match kind {
            SdfKind::Rect => 0.0,
            SdfKind::Capsule => 1.0,
            SdfKind::Arc => 2.0,
            SdfKind::Polygon => 3.0,
            SdfKind::Ellipse => 4.0,
        };
        let (stroke_color, stroke_width, dash, dash_offset) = match stroke {
            Some(s) => (s.color, s.width, s.dash, s.dash_offset),
            None => ([0.0; 4], 0.0, [0.0; 2], 0.0),
        };
        let (shadow_color, sigma, offset) = match shadow {
            Some(s) => (s.color, s.sigma, s.offset),
            None => ([0.0; 4], 0.0, Vec2::default()),
        };
        let mut a = [0.0; 4];
        let mut b = [0.0; 4];
        let since = material.as_ref().map_or(0.0, |m| m.since);
        if let Some(m) = material {
            for (i, v) in m.params.iter().take(8).enumerate() {
                if i < 4 {
                    a[i] = *v;
                } else {
                    b[i - 4] = *v;
                }
            }
        }
        let slots = [
            [rect.x, rect.y, rect.w, rect.h],
            *corners,
            [shape, *corner_kind as f32, *rotation, *opacity],
            [arc[0], arc[1], arc[2], polygon[0]],
            [polygon[1], stroke_width, dash[0], dash[1]],
            [dash_offset, sigma, offset.x, offset.y],
            *fill,
            stroke_color,
            shadow_color,
            a,
            b,
            [since, 0.0, 0.0, 0.0],
        ];
        Some((Self(slots), material.as_ref()))
    }
}

impl SdfInstance {
    /// The two instances of an [`DrawCommand::SdfBlend`] and the first one's material: the lead carries the blend
    /// radius in slot 11, the partner (the very next instance) is marked so its own quad is not drawn.
    pub(crate) fn from_blend(command: &DrawCommand) -> Option<([Self; 2], Option<&SdfMaterial>)> {
        let DrawCommand::SdfBlend {
            first,
            second,
            radius,
        } = command
        else {
            return None;
        };
        let (mut lead, material) = Self::from_command(first)?;
        let (mut partner, _) = Self::from_command(second)?;
        lead.0[11][1] = *radius;
        partner.0[11][2] = 1.0;
        Some(([lead, partner], material))
    }

    /// Whether this instance leads a blended pair.
    fn leads_pair(&self) -> bool {
        self.0[11][1] > 0.0
    }
}

/// Split instances into uniform-buffer batches of at most [`MAX_INSTANCES`], never between a blended pair's lead
/// and its partner.
pub(crate) fn batches(instances: &[SdfInstance]) -> Vec<&[SdfInstance]> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < instances.len() {
        let mut end = (start + MAX_INSTANCES).min(instances.len());
        if end < instances.len() && instances[end - 1].leads_pair() {
            end -= 1;
        }
        out.push(&instances[start..end]);
        start = end;
    }
    out
}

/// The pipeline key of a material's WGSL; the plain pass is [`PLAIN_KEY`].
pub(crate) fn pipeline_key(wgsl: Option<&str>) -> u64 {
    match wgsl {
        None => PLAIN_KEY,
        Some(wgsl) => {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            wgsl.hash(&mut hasher);
            hasher.finish()
        }
    }
}

/// A material's whole module: the prelude and the program's WGSL that declares `shade`.
pub(crate) fn material_source(wgsl: &str) -> String {
    [PRELUDE, wgsl].concat()
}

fn plain_source() -> String {
    [PRELUDE, PLAIN_SHADE].concat()
}

/// Whether a material's WGSL makes a valid module, kept by the WGSL's hash since a canvas maps its scene every frame.
pub(crate) fn check_material(wgsl: &str) -> Result<(), String> {
    use std::sync::{Mutex, OnceLock};
    static CHECKED: OnceLock<Mutex<HashMap<u64, Result<(), String>>>> = OnceLock::new();
    let key = pipeline_key(Some(wgsl));
    let checked = CHECKED.get_or_init(Default::default);
    if let Some(known) = checked.lock().unwrap_or_else(|p| p.into_inner()).get(&key) {
        return known.clone();
    }
    let source = material_source(wgsl);
    let answer = check_source(&source);
    checked
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(key, answer.clone());
    answer
}

fn check_source(source: &str) -> Result<(), String> {
    wgpu::naga::front::wgsl::parse_str(source)
        .map_err(|e| e.emit_to_string(source))
        .and_then(|module| {
            wgpu::naga::valid::Validator::new(
                wgpu::naga::valid::ValidationFlags::all(),
                wgpu::naga::valid::Capabilities::empty(),
            )
            .validate(&module)
            .map(|_| ())
            .map_err(|e| e.emit_to_string(source))
        })
}

fn build_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    source: &str,
) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("sdf shader"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sdf pipeline"),
        layout: Some(layout),
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
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

/// The pipelines of the pass: the plain one and one per material WGSL.
pub(crate) struct SdfPass {
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    pipelines: HashMap<u64, wgpu::RenderPipeline>,
}

impl SdfPass {
    pub(crate) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sdf bind group layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sdf pipeline layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let mut pipelines = HashMap::new();
        pipelines.insert(
            PLAIN_KEY,
            build_pipeline(device, &pipeline_layout, format, &plain_source()),
        );
        Self {
            layout,
            pipeline_layout,
            format,
            pipelines,
        }
    }

    /// Compile the material's pipeline once; a WGSL that does not check is an error naming the reason.
    pub(crate) fn ensure(
        &mut self,
        device: &wgpu::Device,
        key: u64,
        wgsl: Option<&str>,
    ) -> Result<(), String> {
        if self.pipelines.contains_key(&key) {
            return Ok(());
        }
        let wgsl = wgsl.ok_or_else(|| "sdf pipeline has no WGSL".to_string())?;
        check_material(wgsl)?;
        let pipeline = build_pipeline(
            device,
            &self.pipeline_layout,
            self.format,
            &material_source(wgsl),
        );
        self.pipelines.insert(key, pipeline);
        Ok(())
    }

    pub(crate) fn pipeline(&self, key: u64) -> Option<&wgpu::RenderPipeline> {
        self.pipelines.get(&key)
    }

    /// A uniform buffer holding the frame (packet size, clock, pointer) and up to [`MAX_INSTANCES`] shapes.
    pub(crate) fn bind_group(
        &self,
        device: &wgpu::Device,
        instances: &[SdfInstance],
        packet_size: (f32, f32),
        time_ms: f64,
        pointer: Option<Vec2>,
    ) -> wgpu::BindGroup {
        let mut floats = vec![0.0f32; HEADER_FLOATS + MAX_INSTANCES * FIELDS * 4];
        floats[0] = packet_size.0.max(1.0);
        floats[1] = packet_size.1.max(1.0);
        floats[2] = (time_ms / 1000.0) as f32;
        if let Some(p) = pointer {
            floats[3] = 1.0;
            floats[4] = p.x;
            floats[5] = p.y;
        }
        for (n, instance) in instances.iter().take(MAX_INSTANCES).enumerate() {
            for (k, slot) in instance.0.iter().enumerate() {
                let at = HEADER_FLOATS + (n * FIELDS + k) * 4;
                floats[at..at + 4].copy_from_slice(slot);
            }
        }
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sdf instances"),
            contents: bytemuck::cast_slice(&floats),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sdf bind group"),
            layout: &self.layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::SdfKind;

    #[test]
    fn the_plain_module_checks_and_a_missing_shade_does_not() {
        assert_eq!(check_source(&plain_source()), Ok(()));
        let err = check_material("fn other() -> f32 { return 1.0; }").unwrap_err();
        assert!(err.contains("shade"), "{err}");
    }

    #[test]
    fn a_material_reading_time_and_pointer_checks() {
        let wgsl = "fn shade(px: Pixel) -> vec4<f32> { return vec4<f32>(px.uv, sin(px.time) + px.pointer.x * 0.0, 1.0); }";
        assert_eq!(check_material(wgsl), Ok(()));
    }

    fn shape(kind: SdfKind, x: f32) -> DrawCommand {
        DrawCommand::Sdf {
            kind,
            rect: crate::packet::Rect { x, y: 0.0, w: 10.0, h: 10.0 },
            corners: [0.0; 4],
            corner_kind: 0,
            arc: [0.0; 3],
            polygon: [0.0; 2],
            fill: [1.0, 0.0, 0.0, 1.0],
            stroke: None,
            shadow: None,
            rotation: 0.0,
            opacity: 1.0,
            material: None,
        }
    }

    fn blend(radius: f32) -> DrawCommand {
        DrawCommand::SdfBlend {
            first: Box::new(shape(SdfKind::Ellipse, 0.0)),
            second: Box::new(shape(SdfKind::Ellipse, 20.0)),
            radius,
        }
    }

    #[test]
    fn a_blend_packs_a_lead_with_the_radius_and_a_marked_partner() {
        let command = blend(6.0);
        let ([lead, partner], material) = SdfInstance::from_blend(&command).expect("a blend");
        assert!(material.is_none());
        assert_eq!(lead.0[11][1], 6.0, "the lead carries the radius");
        assert_eq!(lead.0[11][2], 0.0);
        assert_eq!(partner.0[11][2], 1.0, "the partner is marked");
        assert_eq!(partner.0[11][1], 0.0);
        assert_eq!(lead.0[0][0], 0.0);
        assert_eq!(partner.0[0][0], 20.0, "the partner keeps its own rect");
        let plain_shape = shape(SdfKind::Rect, 0.0);
        assert!(SdfInstance::from_blend(&plain_shape).is_none(), "control: a plain shape is no blend");
    }

    #[test]
    fn batches_never_split_a_blended_pair() {
        let ([lead, partner], _) = SdfInstance::from_blend(&blend(4.0)).unwrap();
        let (plain, _) = SdfInstance::from_command(&shape(SdfKind::Rect, 0.0)).unwrap();
        let mut all = vec![plain; MAX_INSTANCES - 1];
        all.push(lead);
        all.push(partner);
        let split = batches(&all);
        assert_eq!(split.iter().map(|b| b.len()).collect::<Vec<_>>(), vec![MAX_INSTANCES - 1, 2]);
        assert!(split[1][0].leads_pair(), "the pair starts the next batch whole");
        let plains = vec![plain; MAX_INSTANCES + 1];
        assert_eq!(
            batches(&plains).iter().map(|b| b.len()).collect::<Vec<_>>(),
            vec![MAX_INSTANCES, 1],
            "control: plain shapes fill a batch"
        );
    }

    #[test]
    fn keys_are_stable_per_wgsl_and_plain_is_apart() {
        assert_eq!(pipeline_key(Some("a")), pipeline_key(Some("a")));
        assert_ne!(pipeline_key(Some("a")), pipeline_key(Some("b")));
        assert_eq!(pipeline_key(None), PLAIN_KEY);
    }
}
