//! orbital-canvas-patterns: the subject families' scenes mapped to the draw-command packet: the learning canvases
//! (algorithms, graphs, maths, physics, chemistry, biology) and the diagrams (state machines, graphs, mermaid, SVG
//! atoms), and the dispatcher that sends each render-ui pattern to its mapper. No GPU.
//!
//! Directories under `src/` are organizational only: every module is declared here at the crate root with its
//! `#[path]`, so public paths stay flat and this file is the one map of where each module lives.

#[path = "diagram/mod.rs"]
pub mod diagram;
#[path = "learning/mod.rs"]
pub mod learning;

#[path = "scene3d/learning_overlay3d.rs"]
pub mod learning_overlay3d;
#[path = "scene3d/learning_scene3d/mod.rs"]
pub mod learning_scene3d;

// scene: the dispatcher every host maps through
#[path = "scene/scene.rs"]
pub mod scene;
