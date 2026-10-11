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

use super::game_canvas_2d::CanvasSceneError;
use super::scene::MappedScene;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::{OnceLock, RwLock};

const NS: &str = "almadar::game_renderer::mapper";

static GAPS: OnceLock<RwLock<HashSet<String>>> = OnceLock::new();

/// Log a mapper gap once per `key`.
pub fn gap_once(key: &str, detail: &str) {
    let mut set = GAPS
        .get_or_init(Default::default)
        .write()
        .expect("gap log poisoned");
    if set.insert(key.to_string()) {
        tracing::warn!(target: NS, gap = key, detail = %detail, "mapper gap");
    }
}

/// Scene-to-packet transform: scene units are scaled by `s` and offset by
/// (`ox`, `oy`) into the host viewport (contain-fit, centered).
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Xf {
    pub s: f32,
    pub ox: f32,
    pub oy: f32,
}

impl Xf {
    pub fn identity() -> Self {
        Self {
            s: 1.0,
            ox: 0.0,
            oy: 0.0,
        }
    }

    pub fn p(&self, x: f32, y: f32) -> (f32, f32) {
        (self.ox + x * self.s, self.oy + y * self.s)
    }
}

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
        "learning-canvas" => Ok(learning_canvas::map(props, viewport)),
        "math-canvas" => Ok(math_canvas::map(props, viewport)),
        "physics-canvas" => physics_canvas::map(props, viewport),
        other => unreachable!("learning::handles admitted `{other}` with no mapper"),
    }
}
