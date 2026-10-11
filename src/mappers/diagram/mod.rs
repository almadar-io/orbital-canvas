//! Diagram patterns on the Rust 2D renderer: the SVG-drawn `@almadar/ui` patterns that are not
//! learning canvases (`mappers::learning`) — graph and state-machine diagrams, the decorative
//! `svg-*` atoms, the orbital visualization, the positioned floor-plan canvas, the atlas
//! sprites and the scaled container that composes them.

#![allow(clippy::too_many_arguments)] // pen primitives mirror the SVG attribute lists they draw

pub mod atlas_sprites;
pub mod draw;
pub mod effect_glyph;
pub mod graph_layout;
pub mod graph_view;
pub mod layered;
pub mod mermaid;
pub mod orbital_viz;
pub mod positioned_canvas;
pub mod scaled_diagram;
pub mod state_machine;
pub mod state_views;
pub mod svg_atoms;

use super::game_canvas_2d::{get_f32, CanvasSceneError};
use super::scene::MappedScene;
use serde_json::Value;

/// Every diagram pattern that is not an `svg-*` atom ([`svg_atoms::ATOMS`]).
pub(crate) const PATTERNS: &[&str] = &[
    "atlas-image",
    "atlas-panel",
    "graph-view",
    "jazari-state-machine",
    "orbital-visualization",
    "positioned-canvas",
    "scaled-diagram",
    "state-graph",
    "state-machine-view",
];

/// Scenes no registry pattern names: a host composes them inside its own pattern (a ```` ```mermaid ````
/// fence in `markdown-content` / `segment-renderer`).
pub const SCENE_ONLY: &[&str] = &["mermaid"];

pub fn handles(pattern: &str) -> bool {
    svg_atoms::ATOMS.contains(&pattern)
        || PATTERNS.contains(&pattern)
        || SCENE_ONLY.contains(&pattern)
}

pub fn map(
    pattern: &str,
    props: &Value,
    viewport: Option<(f32, f32)>,
) -> Result<MappedScene, CanvasSceneError> {
    if svg_atoms::ATOMS.contains(&pattern) {
        return svg_atoms::map(pattern, props, viewport);
    }
    match pattern {
        "graph-view" => graph_view::map(props, viewport),
        "state-graph" => state_views::state_graph(props, viewport),
        "state-machine-view" => state_views::state_machine_view(props, viewport),
        "jazari-state-machine" => state_views::jazari(props, viewport),
        "mermaid" => mermaid::map(props, viewport),
        "orbital-visualization" => orbital_viz::map(props, viewport),
        "positioned-canvas" => positioned_canvas::map(props, viewport),
        "scaled-diagram" => scaled_diagram::map(props, viewport),
        "atlas-image" => atlas_sprites::image(props, viewport),
        "atlas-panel" => atlas_sprites::panel(props, viewport),
        other => unreachable!("diagram::handles admitted `{other}` with no mapper"),
    }
}

pub(crate) fn props_f32(props: &Value, key: &str, default: f32) -> f32 {
    get_f32(props, key, default)
}

pub(crate) fn props_str<'a>(props: &'a Value, key: &str) -> Option<&'a str> {
    props.get(key).and_then(Value::as_str)
}
