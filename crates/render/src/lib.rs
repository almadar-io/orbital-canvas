//! orbital-canvas-render: the GPU side of drawing on wgpu: the shared device, the frame painter (quads, sprites,
//! vector layers, SDF shapes and materials, glyph runs, backdrops, render targets, particles), the 3D renderer and
//! the `gpu/*` operator host.
//!
//! Directories under `src/` are organizational only: every module is declared here at the crate root with its
//! `#[path]`, so public paths stay flat and this file is the one map of where each module lives.

#[path = "error.rs"]
pub mod error;
pub use error::{RendererError, Result};

// device: the one device every painter shares
#[path = "device/device.rs"]
pub mod device;

// frame: one packet painted into a target
#[path = "frame/backdrop.rs"]
pub mod backdrop;
#[path = "frame/glyph_atlas.rs"]
pub mod glyph_atlas;
#[path = "frame/particles.rs"]
pub mod particles;
#[path = "frame/render_target.rs"]
pub mod render_target;
#[path = "frame/renderer/mod.rs"]
pub mod renderer;
#[path = "frame/sdf.rs"]
pub mod sdf;
#[path = "frame/texture.rs"]
pub mod texture;

// scene3d: the 3D renderer
#[path = "scene3d/mod.rs"]
pub mod scene3d;

// gpu: the gpu/* operator host
#[path = "gpu/mod.rs"]
pub mod gpu;
