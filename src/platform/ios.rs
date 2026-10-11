//! iOS surface initialization using `CAMetalLayer*` via wgpu Metal backend.
//!
//! This module is only compiled on `target_os = "ios"` (see `platform/mod.rs`).
//! On iOS wgpu's Metal backend is always present, so `SurfaceTargetUnsafe::CoreAnimationLayer`
//! is unconditionally available.

use crate::{Canvas, RendererError, Result};

/// Initialize a renderer bound to a raw `CAMetalLayer*`.
///
/// Builds a wgpu Metal surface from the layer pointer via
/// `SurfaceTargetUnsafe::CoreAnimationLayer`, configures it for `width × height`, and
/// returns a renderer ready for `orbital_canvas_render` calls.
///
/// # Safety
/// `layer` must be a valid, non-null `CAMetalLayer*` that remains alive for the lifetime of
/// the returned renderer.
pub unsafe fn init_from_metal_layer(
    layer: *mut std::ffi::c_void,
    width: u32,
    height: u32,
    _resource_base_url: &str,
) -> Result<Canvas> {
    if layer.is_null() {
        return Err(RendererError::Surface(
            "CAMetalLayer pointer is null".into(),
        ));
    }

    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::METAL,
        flags: wgpu::InstanceFlags::default(),
        memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
        backend_options: wgpu::BackendOptions::default(),
    });

    // SAFETY: `layer` (CAMetalLayer*) is valid and outlives the surface per caller contract.
    let surface = unsafe {
        instance
            .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(layer))
            .map_err(|e| RendererError::Surface(format!("create_surface: {e}")))?
    };

    let mut renderer = Canvas::new_with_surface(instance, surface)?;
    renderer.configure_surface(width, height)?;
    Ok(renderer)
}
