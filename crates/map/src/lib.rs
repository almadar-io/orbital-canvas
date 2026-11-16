//! orbital-canvas-map: a render-ui scene mapped to the draw-command packet, for every pattern that is not a subject
//! family: the 2D canvas and its drawables, layers, emitters and text reveals, and the 3D canvas. No GPU.
//!
//! Directories under `src/` are organizational only: every module is declared here at the crate root with its
//! `#[path]`, so public paths stay flat and this file is the one map of where each module lives.

// scene: what a mapped scene is and why one fails
#[path = "scene/error.rs"]
pub mod error;
#[path = "scene/scene.rs"]
pub mod scene;

// canvas2d: the 2D canvas and its drawables
#[path = "canvas2d/edit_layout.rs"]
pub mod edit_layout;
#[path = "canvas2d/emitter.rs"]
pub mod emitter;
#[path = "canvas2d/game_canvas_2d/mod.rs"]
pub mod game_canvas_2d;
#[path = "canvas2d/isometric_canvas.rs"]
pub mod isometric_canvas;
#[path = "canvas2d/layer_props.rs"]
pub mod layer_props;
#[path = "canvas2d/mini_map.rs"]
pub mod mini_map;
#[path = "canvas2d/platformer_canvas.rs"]
pub mod platformer_canvas;
#[path = "canvas2d/skinned_mesh_2d.rs"]
pub mod skinned_mesh_2d;
#[path = "canvas2d/text_reveal.rs"]
pub mod text_reveal;
#[path = "canvas2d/theme_refs.rs"]
pub mod theme_refs;

// canvas3d: the 3D canvas
#[path = "canvas3d/canvas3d_props.rs"]
pub mod canvas3d_props;
#[path = "canvas3d/canvas_scene3d/mod.rs"]
pub mod canvas_scene3d;
