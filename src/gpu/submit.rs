//! Writes, command submission and readback.

use super::io::{drive, scoped};
use super::signal::signal;
use super::{
    arg, as_id, as_u32, as_u64, field, invalid, io_error, list, object, record, required, GpuHost,
    Object, Pipe, Reply,
};
use orbital_core_base::os::OsError;
use orbital_core_base::value::{Value, ValueMap};

const ROW_ALIGN: u32 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

enum Endpoint {
    Buffer {
        buffer: wgpu::Buffer,
        size: u64,
    },
    Texture {
        texture: wgpu::Texture,
        width: u32,
        height: u32,
        depth: u32,
        format: wgpu::TextureFormat,
    },
}

enum Cmd {
    Compute {
        pipeline: wgpu::ComputePipeline,
        groups: Vec<(u32, wgpu::BindGroup)>,
        dispatch: [u32; 3],
    },
    Render {
        pipeline: wgpu::RenderPipeline,
        groups: Vec<(u32, wgpu::BindGroup)>,
        view: wgpu::TextureView,
        vertices: u32,
        instances: u32,
        clear: Option<wgpu::Color>,
    },
    BufferToBuffer {
        from: wgpu::Buffer,
        from_offset: u64,
        to: wgpu::Buffer,
        to_offset: u64,
        size: u64,
    },
    BufferToTexture {
        buffer: wgpu::Buffer,
        offset: u64,
        texture: wgpu::Texture,
        extent: wgpu::Extent3d,
        row: u32,
    },
    TextureToBuffer {
        texture: wgpu::Texture,
        buffer: wgpu::Buffer,
        offset: u64,
        extent: wgpu::Extent3d,
        row: u32,
    },
    TextureToTexture {
        from: wgpu::Texture,
        to: wgpu::Texture,
        extent: wgpu::Extent3d,
    },
    Clear {
        buffer: wgpu::Buffer,
        offset: u64,
        size: Option<u64>,
    },
    Stamp {
        set: wgpu::QuerySet,
        index: u32,
    },
    WriteBuffer {
        buffer: wgpu::Buffer,
        offset: u64,
        data: Value,
    },
    WriteTexture {
        texture: wgpu::Texture,
        extent: wgpu::Extent3d,
        row: u32,
        data: Value,
    },
    /// `direct` for a map-read buffer: mapped after the whole list instead of copied out at its place in it.
    Read {
        buffer: wgpu::Buffer,
        offset: u64,
        length: u64,
        direct: bool,
    },
}

impl Cmd {
    /// Whether this command can change `buffer`'s contents.
    fn writes(&self, buffer: &wgpu::Buffer) -> bool {
        match self {
            Cmd::WriteBuffer { buffer: to, .. }
            | Cmd::BufferToBuffer { to, .. }
            | Cmd::TextureToBuffer { buffer: to, .. }
            | Cmd::Clear { buffer: to, .. } => to == buffer,
            _ => false,
        }
    }
}

fn bytes_of(data: &Value) -> &[u8] {
    data.as_bytes().unwrap_or_default()
}

fn align(at: u64, to: u64) -> u64 {
    at.div_ceil(to) * to
}

fn extent(width: u32, height: u32, depth: u32) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: depth,
    }
}

fn row_bytes(width: u32, format: wgpu::TextureFormat) -> Result<u32, OsError> {
    let texel = format
        .block_copy_size(None)
        .ok_or_else(|| invalid("this texture format has no plain texel size"))?;
    width
        .checked_mul(texel)
        .ok_or_else(|| invalid("texture row is too large"))
}

fn aligned_row(width: u32, format: wgpu::TextureFormat) -> Result<u32, OsError> {
    let row = row_bytes(width, format)?;
    if row % ROW_ALIGN != 0 {
        return Err(invalid(format!(
            "a texture<->buffer copy needs rows of a multiple of {ROW_ALIGN} bytes; this one is {row}"
        )));
    }
    Ok(row)
}

fn zero_offset(map: &ValueMap, key: &str) -> Result<(), OsError> {
    match field(map, key) {
        Some(v) if as_u64(v, key)? != 0 => {
            Err(invalid(format!("`{key}` is not used on a texture side")))
        }
        _ => Ok(()),
    }
}

fn optional_size_equals(map: &ValueMap, whole: u64) -> Result<(), OsError> {
    match field(map, "size") {
        Some(v) if as_u64(v, "size")? != whole => Err(invalid(format!(
            "a copy with a texture side moves the whole texture ({whole} bytes)"
        ))),
        _ => Ok(()),
    }
}

impl GpuHost {
    fn endpoint(&self, id: u64, device: u64) -> Result<Endpoint, OsError> {
        self.owned(id, device, "buffer or texture", |e| match &e.object {
            Object::Buffer { buffer, size, .. } => Some(Endpoint::Buffer {
                buffer: buffer.clone(),
                size: *size,
            }),
            Object::Texture {
                texture,
                width,
                height,
                depth,
                format,
            } => Some(Endpoint::Texture {
                texture: texture.clone(),
                width: *width,
                height: *height,
                depth: *depth,
                format: *format,
            }),
            _ => None,
        })
    }

    fn bind_groups(
        &self,
        value: &Value,
        device: u64,
    ) -> Result<Vec<(u32, wgpu::BindGroup)>, OsError> {
        list(value, "bindGroups")?
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let id = as_id(item, "bind group")?;
                let group = self.owned(id, device, "bind group", |e| match &e.object {
                    Object::BindGroup(g) => Some(g.clone()),
                    _ => None,
                })?;
                let slot = u32::try_from(index).map_err(|_| invalid("too many bind groups"))?;
                Ok((slot, group))
            })
            .collect()
    }

    fn parse_command(&self, value: &Value, device: u64, timestamps: bool) -> Result<Cmd, OsError> {
        let record = object(value, "command")?;
        if record.len() != 1 {
            return Err(invalid(
                "a command holds exactly one of write, compute, render, copy, clear, timestamp, read",
            ));
        }
        let (kind, body) = record
            .iter()
            .next()
            .ok_or_else(|| invalid("empty command"))?;
        let body = object(body, kind)?;
        match &***kind {
            "compute" => {
                let id = as_id(required(body, "pipeline")?, "pipeline")?;
                let pipeline = self.owned(id, device, "compute pipeline", |e| match &e.object {
                    Object::Pipeline(Pipe::Compute(p)) => Some(p.clone()),
                    _ => None,
                })?;
                let groups = self.bind_groups(required(body, "bindGroups")?, device)?;
                let dims = list(required(body, "dispatch")?, "dispatch")?;
                let [x, y, z] = dims.as_slice() else {
                    return Err(invalid("dispatch is [x, y, z]"));
                };
                Ok(Cmd::Compute {
                    pipeline,
                    groups,
                    dispatch: [
                        as_u32(x, "dispatch x")?,
                        as_u32(y, "dispatch y")?,
                        as_u32(z, "dispatch z")?,
                    ],
                })
            }
            "render" => {
                let id = as_id(required(body, "pipeline")?, "pipeline")?;
                let pipeline = self.owned(id, device, "render pipeline", |e| match &e.object {
                    Object::Pipeline(Pipe::Render(p)) => Some(p.clone()),
                    _ => None,
                })?;
                let groups = self.bind_groups(required(body, "bindGroups")?, device)?;
                let target = as_id(required(body, "target")?, "target")?;
                let texture = self.owned(target, device, "texture", |e| match &e.object {
                    Object::Texture { texture, .. } => Some(texture.clone()),
                    _ => None,
                })?;
                let vertices = as_u32(required(body, "vertices")?, "vertices")?;
                let instances = field(body, "instances")
                    .map(|v| as_u32(v, "instances"))
                    .transpose()?
                    .unwrap_or(1);
                let clear = match field(body, "clear") {
                    None => None,
                    Some(v) => {
                        let parts = list(v, "clear")?;
                        let numbers: Option<Vec<f64>> =
                            parts.iter().map(|p| p.as_number()).collect();
                        match numbers.as_deref() {
                            Some([r, g, b, a]) => Some(wgpu::Color {
                                r: *r,
                                g: *g,
                                b: *b,
                                a: *a,
                            }),
                            _ => return Err(invalid("clear is [r, g, b, a] numbers")),
                        }
                    }
                };
                Ok(Cmd::Render {
                    pipeline,
                    groups,
                    view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
                    vertices,
                    instances,
                    clear,
                })
            }
            "copy" => self.parse_copy(body, device),
            "write" => self.parse_write(body, device),
            "read" => {
                let id = as_id(required(body, "buffer")?, "buffer")?;
                let (buffer, size, usage) =
                    self.owned(id, device, "buffer", |e| match &e.object {
                        Object::Buffer {
                            buffer,
                            size,
                            usage,
                        } => Some((buffer.clone(), *size, *usage)),
                        _ => None,
                    })?;
                let offset = as_u64(required(body, "offset")?, "offset")?;
                let length = as_u64(required(body, "length")?, "length")?;
                if length == 0 {
                    return Err(invalid("nothing to read"));
                }
                if offset.checked_add(length).is_none_or(|end| end > size) {
                    return Err(invalid("the read runs past the end of the buffer"));
                }
                let direct = usage.contains(wgpu::BufferUsages::MAP_READ);
                if direct {
                    if offset % wgpu::MAP_ALIGNMENT != 0
                        || length % wgpu::COPY_BUFFER_ALIGNMENT != 0
                    {
                        return Err(invalid("a read of a map-read buffer starts at a multiple of 8 and is a multiple of 4 long"));
                    }
                } else if !usage.contains(wgpu::BufferUsages::COPY_SRC) {
                    return Err(invalid("a buffer is read through copy-src or map-read"));
                } else if offset % wgpu::COPY_BUFFER_ALIGNMENT != 0
                    || length % wgpu::COPY_BUFFER_ALIGNMENT != 0
                {
                    return Err(invalid("a read starts and ends at a multiple of 4"));
                }
                Ok(Cmd::Read {
                    buffer,
                    offset,
                    length,
                    direct,
                })
            }
            "clear" => {
                let id = as_id(required(body, "buffer")?, "buffer")?;
                let buffer = self.owned(id, device, "buffer", |e| match &e.object {
                    Object::Buffer { buffer, .. } => Some(buffer.clone()),
                    _ => None,
                })?;
                Ok(Cmd::Clear {
                    buffer,
                    offset: field(body, "offset")
                        .map(|v| as_u64(v, "offset"))
                        .transpose()?
                        .unwrap_or(0),
                    size: field(body, "size").map(|v| as_u64(v, "size")).transpose()?,
                })
            }
            "timestamp" => {
                if !timestamps {
                    return Err(super::unsupported("this device has no `timestamp-query`"));
                }
                let id = as_id(required(body, "querySet")?, "querySet")?;
                let index = as_u32(required(body, "index")?, "index")?;
                let (set, count) = self.owned(id, device, "query set", |e| match &e.object {
                    Object::QuerySet { set, count } => Some((set.clone(), *count)),
                    _ => None,
                })?;
                if index >= count {
                    return Err(invalid(format!(
                        "query index {index} is outside the {count} queries"
                    )));
                }
                Ok(Cmd::Stamp { set, index })
            }
            other => Err(invalid(format!("unknown command `{other}`"))),
        }
    }

    fn parse_copy(&self, body: &ValueMap, device: u64) -> Result<Cmd, OsError> {
        let from = self.endpoint(as_id(required(body, "from")?, "from")?, device)?;
        let to = self.endpoint(as_id(required(body, "to")?, "to")?, device)?;
        let offset = |key: &str| -> Result<u64, OsError> {
            Ok(field(body, key)
                .map(|v| as_u64(v, key))
                .transpose()?
                .unwrap_or(0))
        };
        match (from, to) {
            (
                Endpoint::Buffer {
                    buffer: from,
                    size: from_size,
                },
                Endpoint::Buffer {
                    buffer: to,
                    size: to_size,
                },
            ) => {
                let from_offset = offset("fromOffset")?;
                let to_offset = offset("toOffset")?;
                let size = match field(body, "size") {
                    Some(v) => as_u64(v, "size")?,
                    None => from_size.checked_sub(from_offset).ok_or_else(|| {
                        invalid("fromOffset is past the end of the source buffer")
                    })?,
                };
                let fits =
                    |at: u64, total: u64| at.checked_add(size).is_some_and(|end| end <= total);
                if !fits(from_offset, from_size) || !fits(to_offset, to_size) {
                    return Err(invalid("the copy runs past the end of a buffer"));
                }
                Ok(Cmd::BufferToBuffer {
                    from,
                    from_offset,
                    to,
                    to_offset,
                    size,
                })
            }
            (
                Endpoint::Buffer { buffer, .. },
                Endpoint::Texture {
                    texture,
                    width,
                    height,
                    depth,
                    format,
                },
            ) => {
                zero_offset(body, "toOffset")?;
                let row = aligned_row(width, format)?;
                optional_size_equals(body, u64::from(row) * u64::from(height) * u64::from(depth))?;
                Ok(Cmd::BufferToTexture {
                    buffer,
                    offset: offset("fromOffset")?,
                    texture,
                    extent: extent(width, height, depth),
                    row,
                })
            }
            (
                Endpoint::Texture {
                    texture,
                    width,
                    height,
                    depth,
                    format,
                },
                Endpoint::Buffer { buffer, .. },
            ) => {
                zero_offset(body, "fromOffset")?;
                let row = aligned_row(width, format)?;
                optional_size_equals(body, u64::from(row) * u64::from(height) * u64::from(depth))?;
                Ok(Cmd::TextureToBuffer {
                    texture,
                    buffer,
                    offset: offset("toOffset")?,
                    extent: extent(width, height, depth),
                    row,
                })
            }
            (
                Endpoint::Texture {
                    texture: from,
                    width,
                    height,
                    depth,
                    ..
                },
                Endpoint::Texture { texture: to, .. },
            ) => {
                zero_offset(body, "fromOffset")?;
                zero_offset(body, "toOffset")?;
                if field(body, "size").is_some() {
                    return Err(invalid("a texture to texture copy moves the whole texture"));
                }
                Ok(Cmd::TextureToTexture {
                    from,
                    to,
                    extent: extent(width, height, depth),
                })
            }
        }
    }

    fn parse_write(&self, body: &ValueMap, device: u64) -> Result<Cmd, OsError> {
        let data = required(body, "data")?;
        let bytes = data
            .bytes_view()
            .map_err(|e| invalid(format!("data: {e}")))?;
        if bytes.is_empty() {
            return Err(invalid("nothing to write"));
        }
        let length = bytes.len() as u64;
        let data = match data {
            Value::Bytes(_) => data.clone(),
            _ => Value::bytes(bytes.into_owned()),
        };
        match (field(body, "buffer"), field(body, "texture")) {
            (Some(id), None) => {
                let id = as_id(id, "buffer")?;
                let (buffer, size) = self.owned(id, device, "buffer", |e| match &e.object {
                    Object::Buffer { buffer, size, .. } => Some((buffer.clone(), *size)),
                    _ => None,
                })?;
                let offset = as_u64(required(body, "offset")?, "offset")?;
                if offset.checked_add(length).is_none_or(|end| end > size) {
                    return Err(invalid("the write runs past the end of the buffer"));
                }
                if offset % wgpu::COPY_BUFFER_ALIGNMENT != 0
                    || length % wgpu::COPY_BUFFER_ALIGNMENT != 0
                {
                    return Err(invalid(
                        "a buffer write starts at a multiple of 4 and is a multiple of 4 long",
                    ));
                }
                Ok(Cmd::WriteBuffer {
                    buffer,
                    offset,
                    data,
                })
            }
            (None, Some(id)) => {
                let id = as_id(id, "texture")?;
                let (texture, width, height, depth, format) =
                    self.owned(id, device, "texture", |e| match &e.object {
                        Object::Texture {
                            texture,
                            width,
                            height,
                            depth,
                            format,
                        } => Some((texture.clone(), *width, *height, *depth, *format)),
                        _ => None,
                    })?;
                let row = row_bytes(width, format)?;
                let whole = u64::from(row) * u64::from(height) * u64::from(depth);
                if length != whole {
                    return Err(invalid(format!(
                        "a texture write carries the whole texture: {whole} bytes"
                    )));
                }
                Ok(Cmd::WriteTexture {
                    texture,
                    extent: extent(width, height, depth),
                    row,
                    data,
                })
            }
            _ => Err(invalid(
                "a write names exactly one of `buffer` or `texture`",
            )),
        }
    }

    pub(super) async fn submit(&self, args: &[Value]) -> Reply {
        let device = as_id(arg(args, 0, "device")?, "device")?;
        let ctx = self.context(device)?;
        let timestamps = ctx
            .features
            .contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS);
        let commands = list(arg(args, 1, "commands")?, "commands")?
            .iter()
            .map(|c| self.parse_command(c, device, timestamps))
            .collect::<Result<Vec<_>, _>>()?;
        if commands.is_empty() {
            return Err(invalid("no commands to submit"));
        }
        for (at, command) in commands.iter().enumerate() {
            if let Cmd::Read {
                buffer,
                direct: true,
                ..
            } = command
            {
                if commands[at + 1..].iter().any(|later| later.writes(buffer)) {
                    return Err(invalid(
                        "a map-read buffer is read as it stands after the list, so no later command may write it",
                    ));
                }
            }
        }
        // Where each write's bytes sit in the one upload buffer, and each copied read in the one readback buffer.
        let (mut uploads, mut reads) = (0u64, 0u64);
        let slots: Vec<u64> = commands
            .iter()
            .map(|command| match command {
                Cmd::WriteBuffer { data, .. } => {
                    let at = align(uploads, wgpu::COPY_BUFFER_ALIGNMENT);
                    uploads = at + bytes_of(data).len() as u64;
                    at
                }
                Cmd::WriteTexture { extent, row, .. } => {
                    let at = align(uploads, u64::from(ROW_ALIGN));
                    let padded = u64::from(row.div_ceil(ROW_ALIGN) * ROW_ALIGN);
                    uploads = at
                        + padded
                            * u64::from(extent.height)
                            * u64::from(extent.depth_or_array_layers);
                    at
                }
                Cmd::Read {
                    length,
                    direct: false,
                    ..
                } => {
                    let at = align(reads, wgpu::MAP_ALIGNMENT);
                    reads = at + length;
                    at
                }
                _ => 0,
            })
            .collect();
        let readback = (reads > 0).then(|| self.take_staging(&ctx.device, reads));
        scoped(&ctx.device, || {
            let upload = (uploads > 0).then(|| {
                let upload = ctx.device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size: align(uploads, wgpu::COPY_BUFFER_ALIGNMENT),
                    usage: wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: true,
                });
                {
                    let mut view = upload.slice(..).get_mapped_range_mut();
                    for (command, at) in commands.iter().zip(&slots) {
                        let at = *at as usize;
                        match command {
                            Cmd::WriteBuffer { data, .. } => {
                                let bytes = bytes_of(data);
                                view[at..at + bytes.len()].copy_from_slice(bytes);
                            }
                            Cmd::WriteTexture {
                                extent, row, data, ..
                            } => {
                                let (row, padded) = (
                                    *row as usize,
                                    (row.div_ceil(ROW_ALIGN) * ROW_ALIGN) as usize,
                                );
                                let rows = (extent.height * extent.depth_or_array_layers) as usize;
                                for (index, line) in
                                    bytes_of(data).chunks(row).take(rows).enumerate()
                                {
                                    let start = at + index * padded;
                                    view[start..start + row].copy_from_slice(line);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                upload.unmap();
                upload
            });
            let mut encoder = ctx
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            for (command, at) in commands.iter().zip(&slots) {
                match command {
                    Cmd::WriteBuffer {
                        buffer,
                        offset,
                        data,
                    } => {
                        if let Some(upload) = &upload {
                            encoder.copy_buffer_to_buffer(
                                upload,
                                *at,
                                buffer,
                                *offset,
                                bytes_of(data).len() as u64,
                            );
                        }
                    }
                    Cmd::WriteTexture {
                        texture,
                        extent,
                        row,
                        ..
                    } => {
                        if let Some(upload) = &upload {
                            encoder.copy_buffer_to_texture(
                                wgpu::TexelCopyBufferInfo {
                                    buffer: upload,
                                    layout: wgpu::TexelCopyBufferLayout {
                                        offset: *at,
                                        bytes_per_row: Some(row.div_ceil(ROW_ALIGN) * ROW_ALIGN),
                                        rows_per_image: Some(extent.height),
                                    },
                                },
                                wgpu::TexelCopyTextureInfo {
                                    texture,
                                    mip_level: 0,
                                    origin: wgpu::Origin3d::ZERO,
                                    aspect: wgpu::TextureAspect::All,
                                },
                                *extent,
                            );
                        }
                    }
                    Cmd::Read {
                        buffer,
                        offset,
                        length,
                        direct,
                    } => {
                        if let (false, Some(staging)) = (direct, &readback) {
                            encoder.copy_buffer_to_buffer(buffer, *offset, staging, *at, *length);
                        }
                    }
                    Cmd::Compute {
                        pipeline,
                        groups,
                        dispatch,
                    } => {
                        let mut pass =
                            encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
                        pass.set_pipeline(pipeline);
                        for (slot, group) in groups {
                            pass.set_bind_group(*slot, group, &[]);
                        }
                        pass.dispatch_workgroups(dispatch[0], dispatch[1], dispatch[2]);
                    }
                    Cmd::Render {
                        pipeline,
                        groups,
                        view,
                        vertices,
                        instances,
                        clear,
                    } => {
                        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: None,
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view,
                                depth_slice: None,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: clear.map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear),
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            depth_stencil_attachment: None,
                            timestamp_writes: None,
                            occlusion_query_set: None,
                        });
                        pass.set_pipeline(pipeline);
                        for (slot, group) in groups {
                            pass.set_bind_group(*slot, group, &[]);
                        }
                        pass.draw(0..*vertices, 0..*instances);
                    }
                    Cmd::BufferToBuffer {
                        from,
                        from_offset,
                        to,
                        to_offset,
                        size,
                    } => encoder.copy_buffer_to_buffer(from, *from_offset, to, *to_offset, *size),
                    Cmd::BufferToTexture {
                        buffer,
                        offset,
                        texture,
                        extent,
                        row,
                    } => encoder.copy_buffer_to_texture(
                        wgpu::TexelCopyBufferInfo {
                            buffer,
                            layout: wgpu::TexelCopyBufferLayout {
                                offset: *offset,
                                bytes_per_row: Some(*row),
                                rows_per_image: Some(extent.height),
                            },
                        },
                        wgpu::TexelCopyTextureInfo {
                            texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        *extent,
                    ),
                    Cmd::TextureToBuffer {
                        texture,
                        buffer,
                        offset,
                        extent,
                        row,
                    } => encoder.copy_texture_to_buffer(
                        wgpu::TexelCopyTextureInfo {
                            texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        wgpu::TexelCopyBufferInfo {
                            buffer,
                            layout: wgpu::TexelCopyBufferLayout {
                                offset: *offset,
                                bytes_per_row: Some(*row),
                                rows_per_image: Some(extent.height),
                            },
                        },
                        *extent,
                    ),
                    Cmd::TextureToTexture { from, to, extent } => encoder.copy_texture_to_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: from,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        wgpu::TexelCopyTextureInfo {
                            texture: to,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        *extent,
                    ),
                    Cmd::Clear {
                        buffer,
                        offset,
                        size,
                    } => encoder.clear_buffer(buffer, *offset, *size),
                    Cmd::Stamp { set, index } => encoder.write_timestamp(set, *index),
                }
            }
            ctx.queue.submit(std::iter::once(encoder.finish()));
        })
        .await?;
        // Each map-read buffer is mapped once over every range the list reads from it.
        let mut direct: Vec<(wgpu::Buffer, u64, u64)> = Vec::new();
        for command in &commands {
            if let Cmd::Read {
                buffer,
                offset,
                length,
                direct: true,
            } = command
            {
                match direct.iter_mut().find(|(b, ..)| b == buffer) {
                    Some((_, start, end)) => {
                        *start = (*start).min(*offset);
                        *end = (*end).max(offset + length);
                    }
                    None => direct.push((buffer.clone(), *offset, offset + length)),
                }
            }
        }
        let mut maps: Vec<(wgpu::Buffer, u64, u64)> = direct.clone();
        if let Some(staging) = &readback {
            maps.push((staging.clone(), 0, reads));
        }
        if maps.is_empty() {
            let (tx, rx) = signal();
            ctx.queue.on_submitted_work_done(move || tx.send(()));
            drive(&ctx.device, &rx)?;
            rx.await
                .map_err(|_| io_error("the queue ended without finishing the work"))?;
            return Ok(record([("reads", Value::array(Vec::new()))]));
        }
        let mut waits = Vec::with_capacity(maps.len());
        for (buffer, start, end) in &maps {
            let (tx, rx) = signal();
            buffer
                .slice(*start..*end)
                .map_async(wgpu::MapMode::Read, move |result| tx.send(result));
            waits.push(rx);
        }
        let mut mapped = Ok(());
        let mut held = Vec::with_capacity(maps.len());
        for (rx, (buffer, ..)) in waits.into_iter().zip(&maps) {
            drive(&ctx.device, &rx)?;
            let outcome = rx
                .await
                .map_err(|_| io_error("the buffer map never answered"))
                .and_then(|r| r.map_err(|e| io_error(format!("buffer map failed: {e}"))));
            if outcome.is_ok() {
                held.push(buffer.clone());
            }
            mapped = mapped.and(outcome);
        }
        let out = mapped.map(|()| {
            commands
                .iter()
                .zip(&slots)
                .filter_map(|(command, at)| match command {
                    Cmd::Read {
                        buffer,
                        offset,
                        length,
                        direct: true,
                    } => Some(Value::bytes(
                        buffer
                            .slice(*offset..offset + length)
                            .get_mapped_range()
                            .to_vec(),
                    )),
                    Cmd::Read { length, .. } => readback.as_ref().map(|staging| {
                        Value::bytes(staging.slice(*at..at + length).get_mapped_range().to_vec())
                    }),
                    _ => None,
                })
                .collect::<Vec<_>>()
        });
        for buffer in held {
            buffer.unmap();
        }
        if let Some(staging) = readback {
            self.keep_staging(staging);
        }
        Ok(record([("reads", Value::array(out?))]))
    }
}
