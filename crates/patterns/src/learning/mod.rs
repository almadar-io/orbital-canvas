//! Learning canvases on the Rust 2D renderer. Seven web patterns (math,
//! algorithm, physics, biology, chemistry, algo-graph, learning) derive a list
//! of `LearningShape`s and hand it to the `LearningCanvas` atom; here each has a
//! mapper that derives the same shapes and [`canvas`] is the atom.

pub mod canvas;
pub mod quadtree;
pub mod shape;
pub mod theme;

pub mod algo_graph_canvas;
pub mod algorithm_canvas;
pub mod biology_canvas;
pub mod chemistry_canvas;
pub mod graph_canvas;
pub mod learning_canvas;
pub mod math_canvas;
pub mod physics_canvas;

use crate::scene::MappedScene;
use orbital_canvas_map::error::CanvasSceneError;
use serde_json::Value;

pub(crate) const PATTERNS: &[&str] = &[
    "algo-graph-canvas",
    "algorithm-canvas",
    "biology-canvas",
    "chemistry-canvas",
    "graph-canvas",
    "learning-canvas",
    "math-canvas",
    "physics-canvas",
];

/// The molecules the web wraps in a card (title above the canvas): the learning atom and the
/// graph canvas draw without one.
pub const CARD_PATTERNS: &[&str] = &[
    "algo-graph-canvas",
    "algorithm-canvas",
    "biology-canvas",
    "chemistry-canvas",
    "math-canvas",
    "physics-canvas",
];

pub fn handles(pattern: &str) -> bool {
    PATTERNS.contains(&pattern)
}

pub fn map(
    pattern: &str,
    props: &Value,
    viewport: Option<(f32, f32)>,
) -> Result<MappedScene, CanvasSceneError> {
    match pattern {
        "algo-graph-canvas" => algo_graph_canvas::map(props, viewport),
        "algorithm-canvas" => algorithm_canvas::map(props, viewport),
        "biology-canvas" => biology_canvas::map(props, viewport),
        "chemistry-canvas" => chemistry_canvas::map(props, viewport),
        "graph-canvas" => graph_canvas::map(props, viewport),
        "learning-canvas" => learning_canvas::map(props, viewport),
        "math-canvas" => math_canvas::map(props, viewport),
        "physics-canvas" => physics_canvas::map(props, viewport),
        other => unreachable!("learning::handles admitted `{other}` with no mapper"),
    }
}
