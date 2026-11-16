//! The gpu objects a canvas may draw from: every live buffer and texture, process-wide, keyed by its handle id. Ids
//! are unique across every `GpuHost` in the process, so a handle a program puts in a drawable (`{ id, device }`)
//! names exactly one object; the renderer checks it is live and on the named device before binding it.

use super::Object;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

static NEXT: AtomicU64 = AtomicU64::new(1);

pub(super) fn next_id() -> u64 {
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// A buffer or texture a drawable can name.
#[derive(Clone, Debug)]
pub enum DrawableGpu {
    Buffer {
        buffer: wgpu::Buffer,
        size: u64,
        usage: wgpu::BufferUsages,
    },
    Texture {
        texture: wgpu::Texture,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    },
}

struct Live {
    device: u64,
    object: DrawableGpu,
}

static LIVE: Mutex<Option<HashMap<u64, Live>>> = Mutex::new(None);

fn live() -> MutexGuard<'static, Option<HashMap<u64, Live>>> {
    LIVE.lock().unwrap_or_else(|p| p.into_inner())
}

pub(super) fn register(id: u64, device: u64, object: &Object) {
    let drawable = match object {
        Object::Buffer {
            buffer,
            size,
            usage,
        } => DrawableGpu::Buffer {
            buffer: buffer.clone(),
            size: *size,
            usage: *usage,
        },
        Object::Texture {
            texture,
            width,
            height,
            format,
            ..
        } => DrawableGpu::Texture {
            texture: texture.clone(),
            width: *width,
            height: *height,
            format: *format,
        },
        _ => return,
    };
    live().get_or_insert_with(HashMap::new).insert(
        id,
        Live {
            device,
            object: drawable,
        },
    );
}

pub(super) fn unregister(id: u64) {
    if let Some(map) = live().as_mut() {
        map.remove(&id);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrawableGpuError {
    /// No live buffer or texture has this id (never made, released, or its device closed).
    NotLive { id: u64 },
    /// The handle names a device other than the one that owns the object.
    WrongDevice { id: u64, device: u64 },
}

impl std::fmt::Display for DrawableGpuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotLive { id } => write!(f, "gpu handle {id} is not a live buffer or texture"),
            Self::WrongDevice { id, device } => {
                write!(f, "gpu handle {id} does not belong to device {device}")
            }
        }
    }
}

/// The live object behind a drawable's `{ id, device }` handle.
pub fn resolve(id: u64, device: u64) -> Result<DrawableGpu, DrawableGpuError> {
    let guard = live();
    let entry = guard
        .as_ref()
        .and_then(|map| map.get(&id))
        .ok_or(DrawableGpuError::NotLive { id })?;
    if entry.device != device {
        return Err(DrawableGpuError::WrongDevice { id, device });
    }
    Ok(entry.object.clone())
}
