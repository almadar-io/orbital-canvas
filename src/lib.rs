//! Shared `wgpu`-based game renderer for Orbital shells.
//!
//! Implements one GPU renderer that runs on Rust desktop, Android, and iOS
//! through a tiny C ABI. The server/runtime sends a JSON `GameRenderPacket`
//! (display list); this crate turns it into draw calls.

pub use orbital_canvas_base::PORTABLE_MAX_TEXTURE_SIDE;
pub use orbital_canvas_base::{
    animate, color, contrast, diagnostics, exact_ints, fx_projection, morph, packet, path_data, projector,
    shader_check, shaders, skinning_2d, spring, targets, transform,
};
#[cfg(feature = "system-fonts")]
pub use orbital_canvas_paint::system_fonts;
pub use orbital_canvas_paint::{asset, atlas, svg, vector};

pub use orbital_canvas_render::{
    backdrop, device, glyph_atlas, gpu, particles, render_target, renderer, sdf, texture,
    RendererError, Result,
};

pub mod c_api;
mod host;
pub mod mappers;
pub mod platform;
pub mod scene3d;
#[cfg(target_arch = "wasm32")]
pub mod web2d;
#[cfg(target_arch = "wasm32")]
pub mod web3d;

#[cfg(not(target_arch = "wasm32"))]
pub use device::shared_device;
pub use device::{register_device, shared_device_async};
pub use gpu::GpuHost;
pub use renderer::SharedDeviceRenderer;

pub use host::{
    AccessAction, Canvas, ContrastFinding, ContrastProbe, InputEvent, KeyMods, SceneHost, TAP_SLOP,
};
