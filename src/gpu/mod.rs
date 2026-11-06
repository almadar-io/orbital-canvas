//! The `gpu/*` answerer: a handle table over the one shared wgpu device ([`crate::device`]).
//!
//! The kernel hands handles as plain ids and has already checked grants and budget; every id and shape this module
//! relies on is checked again and answered as a typed [`OsError`]. The core is async so a browser host can await it
//! ([`GpuHost::answer_async`]); the native [`GpuAnswerer`] drives the same code with `pollster`.
//!
//! Vocabularies (see `names.rs`): buffer usages `copy-src copy-dst storage uniform vertex index map-read`;
//! texture usages `copy-src copy-dst sampled storage render-target`; features `timestamp-query float32-filterable`.
//! A texture with `depth > 1` is 3D. Texture<->buffer copies cover the whole texture (mip 0) and need a row of
//! `width * texel bytes` that is a multiple of 256.

mod create;
mod io;
mod names;
mod signal;
mod submit;
#[cfg(test)]
mod tests;

use crate::device::{shared_device_async, SharedDevice};
#[cfg(not(target_arch = "wasm32"))]
use orbital_core_base::gpu::GpuAnswerer;
use orbital_core_base::os::{OsError, OsErrorKind, OsOp};
use orbital_core_base::value::{Value, ValueMap};
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

pub(super) type Reply = Result<Value, OsError>;

pub(super) fn invalid(message: impl Into<String>) -> OsError {
    OsError::new(OsErrorKind::Invalid, message)
}

pub(super) fn closed(message: impl Into<String>) -> OsError {
    OsError::new(OsErrorKind::Closed, message)
}

pub(super) fn io_error(message: impl Into<String>) -> OsError {
    OsError::new(OsErrorKind::Io, message)
}

pub(super) fn unsupported(message: impl Into<String>) -> OsError {
    OsError::new(OsErrorKind::Unsupported, message)
}

pub(super) fn record<const N: usize>(pairs: [(&str, Value); N]) -> Value {
    let mut map = ValueMap::default();
    for (key, value) in pairs {
        map.insert(key.to_string().into(), value);
    }
    Value::object(map)
}

pub(super) fn id_value(id: u64) -> Result<Value, OsError> {
    i64::try_from(id)
        .map(Value::Int)
        .map_err(|_| io_error("handle id out of range"))
}

pub(super) fn arg<'a>(args: &'a [Value], index: usize, name: &str) -> Result<&'a Value, OsError> {
    args.get(index)
        .ok_or_else(|| invalid(format!("missing argument `{name}`")))
}

pub(super) fn as_id(value: &Value, name: &str) -> Result<u64, OsError> {
    value
        .as_int()
        .and_then(|n| u64::try_from(n).ok())
        .filter(|n| *n > 0)
        .ok_or_else(|| invalid(format!("`{name}` must be a handle id")))
}

pub(super) fn as_u64(value: &Value, name: &str) -> Result<u64, OsError> {
    value
        .as_int()
        .and_then(|n| u64::try_from(n).ok())
        .ok_or_else(|| invalid(format!("`{name}` must be a non-negative int")))
}

pub(super) fn as_u32(value: &Value, name: &str) -> Result<u32, OsError> {
    as_u64(value, name)
        .and_then(|n| u32::try_from(n).map_err(|_| invalid(format!("`{name}` is too large"))))
}

pub(super) fn object<'a>(value: &'a Value, name: &str) -> Result<&'a ValueMap, OsError> {
    value
        .as_object()
        .ok_or_else(|| invalid(format!("`{name}` must be a record")))
}

pub(super) fn field<'a>(map: &'a ValueMap, key: &str) -> Option<&'a Value> {
    map.get(key).filter(|v| !v.is_null())
}

pub(super) fn required<'a>(map: &'a ValueMap, key: &str) -> Result<&'a Value, OsError> {
    field(map, key).ok_or_else(|| invalid(format!("missing field `{key}`")))
}

pub(super) fn text<'a>(value: &'a Value, name: &str) -> Result<&'a str, OsError> {
    value
        .as_str()
        .ok_or_else(|| invalid(format!("`{name}` must be text")))
}

pub(super) fn list<'a>(value: &'a Value, name: &str) -> Result<&'a Vec<Value>, OsError> {
    value
        .as_array()
        .ok_or_else(|| invalid(format!("`{name}` must be a list")))
}

pub(super) enum Pipe {
    Compute(wgpu::ComputePipeline),
    Render(wgpu::RenderPipeline),
}

pub(super) enum Object {
    Device,
    Buffer {
        buffer: wgpu::Buffer,
        size: u64,
        usage: wgpu::BufferUsages,
    },
    Texture {
        texture: wgpu::Texture,
        width: u32,
        height: u32,
        depth: u32,
        format: wgpu::TextureFormat,
    },
    Sampler(wgpu::Sampler),
    QuerySet {
        set: wgpu::QuerySet,
        count: u32,
    },
    Shader(wgpu::ShaderModule),
    Pipeline(Pipe),
    BindGroup(wgpu::BindGroup),
}

pub(super) struct Entry {
    pub device: u64,
    pub object: Object,
}

struct Table {
    next: u64,
    entries: HashMap<u64, Entry>,
}

/// One program instance's gpu objects over the process-wide device.
pub struct GpuHost {
    table: Mutex<Table>,
    shared: Mutex<Option<SharedDevice>>,
    /// The map-read buffer a read of a copy-src buffer copies through, kept between reads. A read takes it out of the
    /// slot, so concurrent reads never share one; it never outgrows the largest read, which fits in a program buffer.
    staging: Mutex<Option<wgpu::Buffer>>,
}

impl Default for GpuHost {
    fn default() -> Self {
        Self::new()
    }
}

impl GpuHost {
    pub fn new() -> Self {
        Self {
            table: Mutex::new(Table {
                next: 1,
                entries: HashMap::new(),
            }),
            shared: Mutex::new(None),
            staging: Mutex::new(None),
        }
    }

    /// A map-read staging buffer of at least `length` bytes: the kept one when it is large enough, else a new one.
    pub(super) fn take_staging(&self, device: &wgpu::Device, length: u64) -> wgpu::Buffer {
        let kept = self
            .staging
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        match kept {
            Some(buffer) if buffer.size() >= length => buffer,
            _ => device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: length,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
        }
    }

    /// Keep an unmapped staging buffer for the next read; of two, the larger stays.
    pub(super) fn keep_staging(&self, buffer: wgpu::Buffer) {
        let mut slot = self.staging.lock().unwrap_or_else(|p| p.into_inner());
        if slot.as_ref().is_none_or(|kept| kept.size() < buffer.size()) {
            *slot = Some(buffer);
        }
    }

    fn table(&self) -> MutexGuard<'_, Table> {
        self.table.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub(super) fn insert(&self, device: u64, object: Object) -> u64 {
        let mut table = self.table();
        let id = table.next;
        table.next += 1;
        table.entries.insert(id, Entry { device, object });
        id
    }

    pub(super) fn insert_device(&self) -> u64 {
        let mut table = self.table();
        let id = table.next;
        table.next += 1;
        table.entries.insert(
            id,
            Entry {
                device: id,
                object: Object::Device,
            },
        );
        id
    }

    /// `pick` reads what it needs out of the entry; a missing id is `closed` when it was issued, else `invalid`.
    pub(super) fn fetch<T>(
        &self,
        id: u64,
        want: &str,
        pick: impl FnOnce(&Entry) -> Option<T>,
    ) -> Result<(T, u64), OsError> {
        let table = self.table();
        match table.entries.get(&id) {
            None if id < table.next => Err(closed(format!("handle {id} is closed"))),
            None => Err(invalid(format!("no handle {id}"))),
            Some(entry) => pick(entry)
                .map(|picked| (picked, entry.device))
                .ok_or_else(|| invalid(format!("handle {id} is not a {want}"))),
        }
    }

    /// Like [`fetch`](Self::fetch) for an object that must belong to `device`.
    pub(super) fn owned<T>(
        &self,
        id: u64,
        device: u64,
        want: &str,
        pick: impl FnOnce(&Entry) -> Option<T>,
    ) -> Result<T, OsError> {
        let (picked, owner) = self.fetch(id, want, pick)?;
        if owner == device {
            Ok(picked)
        } else {
            Err(invalid(format!("handle {id} belongs to another device")))
        }
    }

    /// The shared device behind an open device handle.
    pub(super) fn context(&self, device: u64) -> Result<SharedDevice, OsError> {
        self.fetch(device, "device", |e| {
            matches!(e.object, Object::Device).then_some(())
        })?;
        self.shared
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .ok_or_else(|| io_error("device handle without a device"))
    }

    pub(super) async fn open_shared(&self) -> Result<SharedDevice, OsError> {
        let held = self
            .shared
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        if let Some(existing) = held {
            return Ok(existing);
        }
        let opened = shared_device_async()
            .await
            .map_err(|e| unsupported(e.to_string()))?;
        let mut shared = self.shared.lock().unwrap_or_else(|p| p.into_inner());
        Ok(shared.get_or_insert(opened).clone())
    }

    pub(super) fn close_device(&self, device: u64) -> Result<(), OsError> {
        self.fetch(device, "device", |e| {
            matches!(e.object, Object::Device).then_some(())
        })?;
        self.table()
            .entries
            .retain(|id, entry| *id != device && entry.device != device);
        self.drop_staging();
        Ok(())
    }

    pub(super) fn release(&self, id: u64) -> Result<(), OsError> {
        let mut table = self.table();
        match table.entries.get(&id) {
            None if id < table.next && id > 0 => Err(closed(format!("handle {id} is closed"))),
            None => Err(invalid(format!("no handle {id}"))),
            Some(Entry {
                object: Object::Device,
                ..
            }) => Err(invalid("a device is closed with gpu/device-close")),
            Some(_) => {
                if let Some(Entry { object, .. }) = table.entries.remove(&id) {
                    match object {
                        Object::Buffer { buffer, .. } => buffer.destroy(),
                        Object::Texture { texture, .. } => texture.destroy(),
                        _ => {}
                    }
                }
                Ok(())
            }
        }
    }

    fn drop_staging(&self) {
        if let Some(buffer) = self
            .staging
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            buffer.destroy();
        }
    }

    /// Drop every object this host holds (host teardown).
    pub fn close_all(&self) {
        self.table().entries.clear();
        self.drop_staging();
    }

    /// Answer one `gpu/*` ask. The browser host awaits this; the native [`GpuAnswerer`] blocks on it.
    pub async fn answer_async(&self, op: OsOp, args: &[Value]) -> Reply {
        match op {
            OsOp::GpuDeviceOpen => self.device_open(args).await,
            OsOp::GpuDeviceClose => {
                let device = as_id(arg(args, 0, "device")?, "device")?;
                self.close_device(device)?;
                Ok(record([]))
            }
            OsOp::GpuBufferCreate => self.buffer_create(args).await,
            OsOp::GpuTextureCreate => self.texture_create(args).await,
            OsOp::GpuSamplerCreate => self.sampler_create(args).await,
            OsOp::GpuQueryCreate => self.query_create(args).await,
            OsOp::GpuShaderCreate => self.shader_create(args).await,
            OsOp::GpuPipelineCreate => self.pipeline_create(args).await,
            OsOp::GpuBindGroupCreate => self.bind_group_create(args).await,
            OsOp::GpuSubmit => self.submit(args).await,
            OsOp::GpuRelease => {
                let handle = as_id(arg(args, 0, "handle")?, "handle")?;
                self.release(handle)?;
                Ok(record([]))
            }
            other => Err(unsupported(format!(
                "{} is not a gpu operation",
                other.name()
            ))),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl GpuAnswerer for GpuHost {
    fn answer(&self, op: OsOp, args: &[Value]) -> Reply {
        pollster::block_on(self.answer_async(op, args))
    }

    fn close_all(&self) {
        GpuHost::close_all(self);
    }
}
