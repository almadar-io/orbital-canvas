//! The one process-wide wgpu device: canvas painting and the `gpu/*` answerer share it.

use crate::{RendererError, Result};
use std::sync::{Arc, Mutex, MutexGuard};

/// A device, its queue and the features it was opened with.
#[derive(Clone)]
pub struct SharedDevice {
    /// The instance that opened the device; `None` when a host registered a device it already owned.
    pub instance: Option<wgpu::Instance>,
    /// The adapter that opened the device; `None` for a registered device.
    pub adapter: Option<wgpu::Adapter>,
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
    pub features: wgpu::Features,
}

static SLOT: Mutex<Option<SharedDevice>> = Mutex::new(None);

fn slot() -> MutexGuard<'static, Option<SharedDevice>> {
    SLOT.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Features canvas asks for when the adapter has them; everything else stays off.
fn wanted_features(adapter: &wgpu::Adapter) -> wgpu::Features {
    adapter.features()
        & (wgpu::Features::TIMESTAMP_QUERY
            | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS
            | wgpu::Features::FLOAT32_FILTERABLE)
}

fn new_instance() -> wgpu::Instance {
    // the shared slot is WebGPU in a browser; a page without it gets a WebGL2 device per canvas instead (web3d)
    #[cfg(target_arch = "wasm32")]
    let backends = wgpu::Backends::BROWSER_WEBGPU;
    #[cfg(not(target_arch = "wasm32"))]
    let backends = wgpu::Backends::all();
    wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends,
        flags: wgpu::InstanceFlags::default(),
        memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
        backend_options: wgpu::BackendOptions::default(),
    })
}

async fn open_async(
    instance: &wgpu::Instance,
    surface: Option<&wgpu::Surface<'static>>,
) -> Result<SharedDevice> {
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: surface,
            force_fallback_adapter: false,
        })
        .await
        .map_err(|_| RendererError::NoAdapter)?;

    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            required_features: wanted_features(&adapter),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            label: Some("orbital-canvas"),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        })
        .await
        .map_err(|e| RendererError::DeviceRequestFailed(e.to_string()))?;

    let features = device.features();
    Ok(SharedDevice {
        instance: Some(instance.clone()),
        adapter: Some(adapter),
        device: Arc::new(device),
        queue: Arc::new(queue),
        features,
    })
}

fn open(
    instance: &wgpu::Instance,
    surface: Option<&wgpu::Surface<'static>>,
) -> Result<SharedDevice> {
    pollster::block_on(open_async(instance, surface))
}

/// Hand the slot a device the host already owns (the desktop window). Fails when the slot is taken.
pub fn register_device(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>) -> Result<()> {
    let mut guard = slot();
    if guard.is_some() {
        return Err(RendererError::DeviceRequestFailed(
            "a shared device is already in place".into(),
        ));
    }
    let features = device.features();
    *guard = Some(SharedDevice {
        instance: None,
        adapter: None,
        device,
        queue,
        features,
    });
    Ok(())
}

/// The registered device, or the one canvas opens now (compute-capable limits, adapter texture resolution).
#[cfg(not(target_arch = "wasm32"))]
pub fn shared_device() -> Result<SharedDevice> {
    pollster::block_on(shared_device_async())
}

/// [`shared_device`] for a host that cannot block (a browser awaits the adapter and device requests).
pub async fn shared_device_async() -> Result<SharedDevice> {
    if let Some(existing) = slot().as_ref() {
        return Ok(existing.clone());
    }
    let opened = open_async(&new_instance(), None).await?;
    let mut guard = slot();
    Ok(guard.get_or_insert(opened).clone())
}

/// The instance a platform surface must be created on: the shared slot's own when the slot is open (its adapter
/// belongs to that instance and no other), else `fresh` (the platform's backend choice), which then opens the slot.
pub(crate) fn instance_for_surface(fresh: impl FnOnce() -> wgpu::Instance) -> wgpu::Instance {
    match slot().as_ref().and_then(|shared| shared.instance.clone()) {
        Some(instance) => instance,
        None => fresh(),
    }
}

/// The slot for a `Canvas` that owns `instance` (and maybe a surface): reuse the slot, or open on that instance so
/// the adapter is compatible with the surface. The slot outlives the Canvas, so mobile surface recreation keeps it.
pub(crate) fn acquire(
    instance: &wgpu::Instance,
    surface: Option<&wgpu::Surface<'static>>,
) -> Result<SharedDevice> {
    let mut guard = slot();
    if let Some(existing) = guard.as_ref() {
        return Ok(existing.clone());
    }
    let opened = open(instance, surface)?;
    *guard = Some(opened.clone());
    Ok(opened)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn a_surface_is_created_on_the_open_slot_instance_and_opens_a_fresh_one_only_when_the_slot_is_empty() {
        let Ok(opened) = shared_device() else {
            eprintln!("no GPU adapter on this host; the slot cannot open");
            return;
        };
        let mut fresh_called = false;
        let _instance = instance_for_surface(|| {
            fresh_called = true;
            new_instance()
        });
        assert_eq!(
            fresh_called,
            opened.instance.is_none(),
            "an open slot with an instance hands that instance out, never a second one"
        );
    }
}
