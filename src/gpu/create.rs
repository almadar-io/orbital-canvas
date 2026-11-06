//! Device, resource, shader, pipeline and bind-group creation.

use super::io::scoped;
use super::names::{self, BUFFER_USAGES, FEATURES, TEXTURE_USAGES};
use super::{
    arg, as_id, as_u32, as_u64, field, id_value, invalid, list, object, record, required, text,
    unsupported, GpuHost, Object, Pipe, Reply,
};
use orbital_core_base::os::OsError;
use orbital_core_base::value::Value;
use std::borrow::Cow;
use std::num::NonZeroU64;

/// The limits `gpu/device-open` reports, by contract name.
fn limits_record(limits: &wgpu::Limits) -> Result<Value, OsError> {
    let big = |n: u64| {
        i64::try_from(n)
            .map(Value::Int)
            .map_err(|_| super::io_error("limit out of range"))
    };
    Ok(record([
        (
            "maxTextureDimension1d",
            Value::Int(limits.max_texture_dimension_1d.into()),
        ),
        (
            "maxTextureDimension2d",
            Value::Int(limits.max_texture_dimension_2d.into()),
        ),
        (
            "maxTextureDimension3d",
            Value::Int(limits.max_texture_dimension_3d.into()),
        ),
        ("maxBindGroups", Value::Int(limits.max_bind_groups.into())),
        (
            "maxBindingsPerBindGroup",
            Value::Int(limits.max_bindings_per_bind_group.into()),
        ),
        (
            "maxSampledTexturesPerShaderStage",
            Value::Int(limits.max_sampled_textures_per_shader_stage.into()),
        ),
        (
            "maxSamplersPerShaderStage",
            Value::Int(limits.max_samplers_per_shader_stage.into()),
        ),
        (
            "maxStorageBuffersPerShaderStage",
            Value::Int(limits.max_storage_buffers_per_shader_stage.into()),
        ),
        (
            "maxStorageTexturesPerShaderStage",
            Value::Int(limits.max_storage_textures_per_shader_stage.into()),
        ),
        (
            "maxUniformBuffersPerShaderStage",
            Value::Int(limits.max_uniform_buffers_per_shader_stage.into()),
        ),
        (
            "maxUniformBufferBindingSize",
            Value::Int(limits.max_uniform_buffer_binding_size.into()),
        ),
        (
            "maxStorageBufferBindingSize",
            Value::Int(limits.max_storage_buffer_binding_size.into()),
        ),
        ("maxBufferSize", big(limits.max_buffer_size)?),
        (
            "maxComputeWorkgroupStorageSize",
            Value::Int(limits.max_compute_workgroup_storage_size.into()),
        ),
        (
            "maxComputeInvocationsPerWorkgroup",
            Value::Int(limits.max_compute_invocations_per_workgroup.into()),
        ),
        (
            "maxComputeWorkgroupSizeX",
            Value::Int(limits.max_compute_workgroup_size_x.into()),
        ),
        (
            "maxComputeWorkgroupSizeY",
            Value::Int(limits.max_compute_workgroup_size_y.into()),
        ),
        (
            "maxComputeWorkgroupSizeZ",
            Value::Int(limits.max_compute_workgroup_size_z.into()),
        ),
        (
            "maxComputeWorkgroupsPerDimension",
            Value::Int(limits.max_compute_workgroups_per_dimension.into()),
        ),
    ]))
}

impl GpuHost {
    /// `powerPreference` is validated but not applied: the process holds one device, opened by whoever came first.
    pub(super) async fn device_open(&self, args: &[Value]) -> Reply {
        let desc = object(arg(args, 0, "desc")?, "desc")?;
        if let Some(preference) = field(desc, "powerPreference") {
            match text(preference, "powerPreference")? {
                "low-power" | "high-performance" => {}
                other => return Err(invalid(format!("unknown powerPreference `{other}`"))),
            }
        }
        let shared = self.open_shared().await?;
        if let Some(requested) = field(desc, "features") {
            for item in list(requested, "features")? {
                let name = text(item, "feature")?;
                let (_, needs) = FEATURES
                    .iter()
                    .find(|(known, _)| *known == name)
                    .ok_or_else(|| invalid(format!("unknown feature `{name}`")))?;
                if !shared.features.contains(needs()) {
                    return Err(unsupported(format!("this device has no `{name}`")));
                }
            }
        }
        let features: Vec<Value> = FEATURES
            .iter()
            .filter(|(_, needs)| shared.features.contains(needs()))
            .map(|(name, _)| Value::string(name.to_string()))
            .collect();
        let id = self.insert_device();
        Ok(record([
            ("device", id_value(id)?),
            ("features", Value::array(features)),
            ("limits", limits_record(&shared.device.limits())?),
        ]))
    }

    pub(super) async fn buffer_create(&self, args: &[Value]) -> Reply {
        let device = as_id(arg(args, 0, "device")?, "device")?;
        let desc = object(arg(args, 1, "desc")?, "desc")?;
        let size = as_u64(required(desc, "size")?, "size")?;
        if size == 0 {
            return Err(invalid("buffer size must be positive"));
        }
        let usage = names::flags(
            required(desc, "usage")?,
            names::buffer_usage,
            BUFFER_USAGES,
            "buffer",
        )?;
        let ctx = self.context(device)?;
        let buffer = scoped(&ctx.device, || {
            ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size,
                usage,
                mapped_at_creation: false,
            })
        })
        .await?;
        let id = self.insert(
            device,
            Object::Buffer {
                buffer,
                size,
                usage,
            },
        );
        Ok(record([("buffer", id_value(id)?)]))
    }

    pub(super) async fn texture_create(&self, args: &[Value]) -> Reply {
        let device = as_id(arg(args, 0, "device")?, "device")?;
        let desc = object(arg(args, 1, "desc")?, "desc")?;
        let width = as_u32(required(desc, "width")?, "width")?;
        let height = as_u32(required(desc, "height")?, "height")?;
        let depth = field(desc, "depth")
            .map(|v| as_u32(v, "depth"))
            .transpose()?
            .unwrap_or(1);
        let mips = field(desc, "mips")
            .map(|v| as_u32(v, "mips"))
            .transpose()?
            .unwrap_or(1);
        if width == 0 || height == 0 || depth == 0 || mips == 0 {
            return Err(invalid(
                "texture width, height, depth and mips must be positive",
            ));
        }
        let format = names::parse_format(text(required(desc, "format")?, "format")?)?;
        let usage = names::flags(
            required(desc, "usage")?,
            names::texture_usage,
            TEXTURE_USAGES,
            "texture",
        )?;
        let ctx = self.context(device)?;
        let texture = scoped(&ctx.device, || {
            ctx.device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: depth,
                },
                mip_level_count: mips,
                sample_count: 1,
                dimension: if depth > 1 {
                    wgpu::TextureDimension::D3
                } else {
                    wgpu::TextureDimension::D2
                },
                format,
                usage,
                view_formats: &[],
            })
        })
        .await?;
        let id = self.insert(
            device,
            Object::Texture {
                texture,
                width,
                height,
                depth,
                format,
            },
        );
        Ok(record([("texture", id_value(id)?)]))
    }

    pub(super) async fn sampler_create(&self, args: &[Value]) -> Reply {
        let device = as_id(arg(args, 0, "device")?, "device")?;
        let desc = object(arg(args, 1, "desc")?, "desc")?;
        let filter = match field(desc, "filter")
            .map(|v| text(v, "filter"))
            .transpose()?
        {
            None | Some("nearest") => wgpu::FilterMode::Nearest,
            Some("linear") => wgpu::FilterMode::Linear,
            Some(other) => {
                return Err(invalid(format!(
                    "unknown filter `{other}`; one of nearest, linear"
                )))
            }
        };
        let address = match field(desc, "address")
            .map(|v| text(v, "address"))
            .transpose()?
        {
            None | Some("clamp") => wgpu::AddressMode::ClampToEdge,
            Some("repeat") => wgpu::AddressMode::Repeat,
            Some("mirror") => wgpu::AddressMode::MirrorRepeat,
            Some(other) => {
                return Err(invalid(format!(
                    "unknown address `{other}`; one of clamp, repeat, mirror"
                )))
            }
        };
        let ctx = self.context(device)?;
        let sampler = scoped(&ctx.device, || {
            ctx.device.create_sampler(&wgpu::SamplerDescriptor {
                label: None,
                address_mode_u: address,
                address_mode_v: address,
                address_mode_w: address,
                mag_filter: filter,
                min_filter: filter,
                mipmap_filter: filter,
                ..Default::default()
            })
        })
        .await?;
        let id = self.insert(device, Object::Sampler(sampler));
        Ok(record([("sampler", id_value(id)?)]))
    }

    pub(super) async fn query_create(&self, args: &[Value]) -> Reply {
        let device = as_id(arg(args, 0, "device")?, "device")?;
        let desc = object(arg(args, 1, "desc")?, "desc")?;
        let count = as_u32(required(desc, "count")?, "count")?;
        if count == 0 {
            return Err(invalid("query count must be positive"));
        }
        let ctx = self.context(device)?;
        if !ctx.features.contains(wgpu::Features::TIMESTAMP_QUERY) {
            return Err(unsupported("this device has no `timestamp-query`"));
        }
        let set = scoped(&ctx.device, || {
            ctx.device.create_query_set(&wgpu::QuerySetDescriptor {
                label: None,
                ty: wgpu::QueryType::Timestamp,
                count,
            })
        })
        .await?;
        let id = self.insert(device, Object::QuerySet { set, count });
        Ok(record([("querySet", id_value(id)?)]))
    }

    pub(super) async fn shader_create(&self, args: &[Value]) -> Reply {
        let device = as_id(arg(args, 0, "device")?, "device")?;
        let wgsl = text(arg(args, 1, "wgsl")?, "wgsl")?;
        let ctx = self.context(device)?;
        let module = scoped(&ctx.device, || {
            ctx.device
                .create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: None,
                    source: wgpu::ShaderSource::Wgsl(Cow::Owned(wgsl.to_string())),
                })
        })
        .await?;
        let id = self.insert(device, Object::Shader(module));
        Ok(record([("shader", id_value(id)?)]))
    }

    pub(super) async fn pipeline_create(&self, args: &[Value]) -> Reply {
        let device = as_id(arg(args, 0, "device")?, "device")?;
        let desc = object(arg(args, 1, "desc")?, "desc")?;
        let kind = text(required(desc, "kind")?, "kind")?;
        let shader_id = as_id(required(desc, "shader")?, "shader")?;
        let entry = text(required(desc, "entry")?, "entry")?;
        let fragment = field(desc, "fragment")
            .map(|v| text(v, "fragment"))
            .transpose()?;
        let targets = match field(desc, "targets") {
            Some(v) => list(v, "targets")?
                .iter()
                .map(|t| names::parse_format(text(t, "target")?))
                .collect::<Result<Vec<_>, _>>()?,
            None => Vec::new(),
        };
        let module = self.owned(shader_id, device, "shader", |e| match &e.object {
            Object::Shader(m) => Some(m.clone()),
            _ => None,
        })?;
        let ctx = self.context(device)?;
        let pipe = match kind {
            "compute" => {
                if fragment.is_some() || !targets.is_empty() {
                    return Err(invalid("a compute pipeline has no fragment or targets"));
                }
                scoped(&ctx.device, || {
                    Pipe::Compute(ctx.device.create_compute_pipeline(
                        &wgpu::ComputePipelineDescriptor {
                            label: None,
                            layout: None,
                            module: &module,
                            entry_point: Some(entry),
                            compilation_options: Default::default(),
                            cache: None,
                        },
                    ))
                })
                .await?
            }
            "render" => {
                if fragment.is_some() != !targets.is_empty() {
                    return Err(invalid(
                        "a render pipeline names `fragment` together with at least one target",
                    ));
                }
                let color_targets: Vec<Option<wgpu::ColorTargetState>> = targets
                    .iter()
                    .map(|format| {
                        Some(wgpu::ColorTargetState {
                            format: *format,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        })
                    })
                    .collect();
                scoped(&ctx.device, || {
                    Pipe::Render(ctx.device.create_render_pipeline(
                        &wgpu::RenderPipelineDescriptor {
                            label: None,
                            layout: None,
                            vertex: wgpu::VertexState {
                                module: &module,
                                entry_point: Some(entry),
                                compilation_options: Default::default(),
                                buffers: &[],
                            },
                            primitive: wgpu::PrimitiveState::default(),
                            depth_stencil: None,
                            multisample: wgpu::MultisampleState::default(),
                            fragment: fragment.map(|fs| wgpu::FragmentState {
                                module: &module,
                                entry_point: Some(fs),
                                compilation_options: Default::default(),
                                targets: &color_targets,
                            }),
                            multiview: None,
                            cache: None,
                        },
                    ))
                })
                .await?
            }
            other => {
                return Err(invalid(format!(
                    "unknown pipeline kind `{other}`; one of compute, render"
                )))
            }
        };
        let id = self.insert(device, Object::Pipeline(pipe));
        Ok(record([("pipeline", id_value(id)?)]))
    }

    pub(super) async fn bind_group_create(&self, args: &[Value]) -> Reply {
        enum Resource {
            Buffer(wgpu::Buffer, u64, Option<NonZeroU64>),
            View(wgpu::TextureView),
            Sampler(wgpu::Sampler),
        }
        let pipeline_id = as_id(arg(args, 0, "pipeline")?, "pipeline")?;
        let group = as_u32(arg(args, 1, "group")?, "group")?;
        let entries = list(arg(args, 2, "entries")?, "entries")?;
        let (pipe, device) = self.fetch(pipeline_id, "pipeline", |e| match &e.object {
            Object::Pipeline(Pipe::Compute(p)) => Some(Pipe::Compute(p.clone())),
            Object::Pipeline(Pipe::Render(p)) => Some(Pipe::Render(p.clone())),
            _ => None,
        })?;
        let mut resources: Vec<(u32, Resource)> = Vec::with_capacity(entries.len());
        for entry in entries {
            let map = object(entry, "binding entry")?;
            let binding = as_u32(required(map, "binding")?, "binding")?;
            let buffer = field(map, "buffer");
            let texture = field(map, "texture");
            let sampler = field(map, "sampler");
            let named = [buffer, texture, sampler]
                .iter()
                .filter(|v| v.is_some())
                .count();
            if named != 1 {
                return Err(invalid(format!(
                    "binding {binding} names exactly one of buffer, texture, sampler"
                )));
            }
            let resource = if let Some(id) = buffer {
                let id = as_id(id, "buffer")?;
                let buf = self.owned(id, device, "buffer", |e| match &e.object {
                    Object::Buffer { buffer, .. } => Some(buffer.clone()),
                    _ => None,
                })?;
                let offset = field(map, "offset")
                    .map(|v| as_u64(v, "offset"))
                    .transpose()?
                    .unwrap_or(0);
                let size = match field(map, "size") {
                    Some(v) => Some(
                        NonZeroU64::new(as_u64(v, "size")?)
                            .ok_or_else(|| invalid("binding size must be positive"))?,
                    ),
                    None => None,
                };
                Resource::Buffer(buf, offset, size)
            } else if let Some(id) = texture {
                let id = as_id(id, "texture")?;
                let tex = self.owned(id, device, "texture", |e| match &e.object {
                    Object::Texture { texture, .. } => Some(texture.clone()),
                    _ => None,
                })?;
                Resource::View(tex.create_view(&wgpu::TextureViewDescriptor::default()))
            } else if let Some(id) = sampler {
                let id = as_id(id, "sampler")?;
                Resource::Sampler(self.owned(id, device, "sampler", |e| match &e.object {
                    Object::Sampler(s) => Some(s.clone()),
                    _ => None,
                })?)
            } else {
                return Err(invalid("empty binding"));
            };
            resources.push((binding, resource));
        }
        let ctx = self.context(device)?;
        let bind_group = scoped(&ctx.device, || {
            let layout = match &pipe {
                Pipe::Compute(p) => p.get_bind_group_layout(group),
                Pipe::Render(p) => p.get_bind_group_layout(group),
            };
            let entries: Vec<wgpu::BindGroupEntry> = resources
                .iter()
                .map(|(binding, resource)| wgpu::BindGroupEntry {
                    binding: *binding,
                    resource: match resource {
                        Resource::Buffer(buffer, offset, size) => {
                            wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                buffer,
                                offset: *offset,
                                size: *size,
                            })
                        }
                        Resource::View(view) => wgpu::BindingResource::TextureView(view),
                        Resource::Sampler(sampler) => wgpu::BindingResource::Sampler(sampler),
                    },
                })
                .collect();
            ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &layout,
                entries: &entries,
            })
        })
        .await?;
        let id = self.insert(device, Object::BindGroup(bind_group));
        Ok(record([("bindGroup", id_value(id)?)]))
    }
}
