//! orbital-canvas-paint: the CPU side of drawing: text (bundled faces, shaping, layout), vector and SVG rasters,
//! and loading assets (images, sprite atlases, glTF models) for every host. No GPU.
//!
//! Directories under `src/` are organizational only: every module is declared here at the crate root with its
//! `#[path]`, so public paths stay flat and this file is the one map of where each module lives.

// raster: pixels on the CPU
#[path = "raster/vector.rs"]
pub mod vector;
#[path = "raster/svg.rs"]
pub mod svg;

// load: bytes a scene names, fetched and decoded
#[path = "load/asset.rs"]
pub mod asset;
#[path = "load/atlas.rs"]
pub mod atlas;

// text: faces the bundle lacks, from the system
#[cfg(feature = "system-fonts")]
#[path = "text/system_fonts.rs"]
pub mod system_fonts;

// scene3d: what a 3D scene loads and labels
#[path = "scene3d/mod.rs"]
pub mod scene3d;
