//! orbital-canvas-base: what a canvas draws, as data and pure maths: the draw-command packet, motion, projection,
//! the 3D scene model and its camera, colours, shader checks and mapper diagnostics. No GPU and no fonts.
//!
//! Directories under `src/` are organizational only: every module is declared here at the crate root with its
//! `#[path]`, so public paths stay flat and this file is the one map of where each module lives.

/// The largest texture side every host can allocate (WebGL2's downlevel limit); a canvas sizes its own rasters within it.
pub const PORTABLE_MAX_TEXTURE_SIDE: u32 = 2048;

// scene: what one frame draws
#[path = "scene/fx_projection.rs"]
pub mod fx_projection;
#[path = "scene/packet.rs"]
pub mod packet;
#[path = "scene/path_data.rs"]
pub mod path_data;
#[path = "scene/targets.rs"]
pub mod targets;

// motion: how drawables change over time
#[path = "motion/animate/mod.rs"]
pub mod animate;
#[path = "motion/morph.rs"]
pub mod morph;
#[path = "motion/spring.rs"]
pub mod spring;

// geometry: where things are
#[path = "geometry/projector.rs"]
pub mod projector;
#[path = "geometry/skinning_2d.rs"]
pub mod skinning_2d;
#[path = "geometry/transform.rs"]
pub mod transform;

// scene3d: the 3D scene as data
#[path = "scene3d/mod.rs"]
pub mod scene3d;

// shaders: WGSL source assembled from parts
#[path = "shaders/mod.rs"]
pub mod shaders;

// support: shared by every layer above
#[path = "support/color.rs"]
pub mod color;
#[path = "support/diagnostics.rs"]
pub mod diagnostics;
#[path = "support/shader_check.rs"]
pub mod shader_check;
