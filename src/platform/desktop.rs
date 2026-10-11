//! Desktop surface initialization using `winit` + `wgpu`.
//!
//! This module is only compiled when the `desktop` feature is enabled.

use crate::{GameRenderer, Result};

/// Initialize a renderer bound to a `winit` window.
///
/// # Safety
/// `window` must remain valid for the lifetime of the returned renderer.
pub unsafe fn init_from_window(
    _window: &winit::window::Window,
    _resource_base_url: &str,
) -> Result<GameRenderer> {
    // Phase 1: create wgpu surface from window. Phase 0 uses headless fallback.
    GameRenderer::new_headless()
}
