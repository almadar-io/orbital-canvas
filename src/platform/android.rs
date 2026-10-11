//! Android surface initialization using `ANativeWindow*` via raw-window-handle + wgpu Vulkan.

use crate::{Canvas, RendererError, Result};
use std::ptr::NonNull;

/// Initialize a renderer bound to a raw `ANativeWindow*`.
///
/// Builds a wgpu Vulkan surface via `raw-window-handle`'s `AndroidNdkWindowHandle`,
/// configures it for `width × height`, and returns the renderer ready for
/// `orbital_canvas_render` calls.
///
/// # Safety
/// `window` must be a valid, non-null `ANativeWindow*` that remains alive for the
/// lifetime of the returned renderer.
pub unsafe fn init_from_native_window(
    window: *mut std::ffi::c_void,
    width: u32,
    height: u32,
    _resource_base_url: &str,
) -> Result<Canvas> {
    let nn = NonNull::new(window)
        .ok_or_else(|| RendererError::Surface("ANativeWindow pointer is null".into()))?;

    let raw_window = raw_window_handle::AndroidNdkWindowHandle::new(nn).into();
    let raw_display = raw_window_handle::AndroidDisplayHandle::new().into();

    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        flags: wgpu::InstanceFlags::default(),
        memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
        backend_options: wgpu::BackendOptions::default(),
    });

    // SAFETY: `window` (ANativeWindow*) is valid and outlives the surface per the caller contract.
    let surface = unsafe {
        instance
            .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: raw_display,
                raw_window_handle: raw_window,
            })
            .map_err(|e| RendererError::Surface(format!("create_surface: {e}")))?
    };

    let mut renderer = Canvas::new_with_surface(instance, surface)?;
    renderer.configure_surface(width, height)?;
    Ok(renderer)
}
