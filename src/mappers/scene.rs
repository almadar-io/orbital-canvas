//! One render-ui scene mapped for native: the display-list packet plus what the
//! host needs to turn a tap into an event.

use super::diagram;
use super::game_canvas_2d::{self, CanvasSceneError};
use super::learning;
use crate::packet::{GameRenderPacket, Rect};
use serde_json::Value;
use std::collections::HashMap;

/// The web graph header's view buttons: zoom about the canvas origin by 1.2 (offset kept, within
/// 0.3..3), or reset to zoom 1 with no offset.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ViewCommand {
    ZoomIn,
    ZoomOut,
    Reset,
}

impl ViewCommand {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "zoom-in" => Some(Self::ZoomIn),
            "zoom-out" => Some(Self::ZoomOut),
            "reset" => Some(Self::Reset),
            _ => None,
        }
    }
}

/// What a renderer keeps for its scene between re-maps, the state the web component holds:
/// a graph's settled node positions (`nodesRef`), whether it has laid out once, and its view
/// (zoom and offset, auto-fit first and then the user's pan and pinch). Gestures queue raw
/// deltas here in packet points; the mapper turns them into view changes.
#[derive(Debug, Clone, Default)]
pub struct SceneMemory {
    /// The pattern the memory belongs to; a scene of another pattern starts afresh.
    pub pattern: Option<String>,
    pub positions: HashMap<String, (f64, f64)>,
    pub laid_out: bool,
    /// `(zoom, offset x, offset y)` in the scene's own units.
    pub view: Option<(f64, f64, f64)>,
    pub pending_pan: (f32, f32),
    /// `(factor, focus x, focus y)` in packet points.
    pub pending_zoom: Option<(f32, f32, f32)>,
    /// A zoom-button press waiting to be applied (the web header's zoom out / zoom in / reset).
    pub pending_view: Option<ViewCommand>,
    /// A long press waiting to be resolved to the element under it (packet points), and the
    /// element it resolved to: the touch stand-in for the web's hover.
    pub pending_press: Option<(f32, f32)>,
    pub pressed: Option<String>,
}

/// An event a tap emits. `payload` is the value the web component passes to the callback; the
/// runtime delivers it keyed by the callback's declared argument name (`wrapCallbackForEvent`),
/// so `arg` names that key (`payload` for `onShapeClick`, `node` for graph callbacks). An empty
/// `arg` is an event-ref whose payload object is delivered as is.
#[derive(Debug, Clone, PartialEq)]
pub struct Emit {
    pub event: String,
    pub arg: String,
    pub payload: Value,
}

impl Emit {
    /// The event payload the trait receives.
    pub fn delivered(&self) -> Value {
        if self.arg.is_empty() {
            self.payload.clone()
        } else {
            serde_json::json!({ self.arg.as_str(): self.payload })
        }
    }
}

/// A tappable region in packet (logical) coordinates.
#[derive(Debug, Clone)]
pub struct Hit {
    pub rect: Rect,
    pub emits: Vec<Emit>,
}

/// A draggable region: a touch that starts on it and travels past the tap slop moves it, and the
/// renderer queues `moves` with the new scene-unit top-left each time the pointer moves.
#[derive(Debug, Clone)]
pub struct Drag {
    pub rect: Rect,
    /// Scene-to-packet transform of the scene this region lives in.
    pub scene_scale: f32,
    pub scene_origin: (f32, f32),
    /// The region's own size in scene units and the largest top-left it may take.
    pub max: (f32, f32),
    pub id: String,
    /// Events queued per move; the payload is `{id, x, y}` in scene units, delivered flat.
    pub moves: Vec<String>,
    /// Moving the region pins the scene element `id` at the pointer ([`SceneMemory::positions`])
    /// and re-maps, like the web's local node drag, instead of queueing events.
    pub pin: bool,
}

#[derive(Debug, Clone, Default)]
pub struct MappedScene {
    pub packet: GameRenderPacket,
    /// In paint order; the topmost (last) region containing a point wins.
    pub hits: Vec<Hit>,
    /// Draggable regions in paint order; the topmost wins.
    pub drags: Vec<Drag>,
    /// Regions a double tap fires (`onNodeDoubleClick`); the host's recognizer reports the gesture.
    pub double_hits: Vec<Hit>,
    /// A one-finger drag that starts off every drag region pans the view, and a pinch zooms it.
    pub pannable: bool,
}

impl MappedScene {
    pub fn drawn(packet: GameRenderPacket) -> Self {
        Self {
            packet,
            ..Self::default()
        }
    }

    /// The events a tap at (`x`, `y`) emits: the topmost region's, or none.
    pub fn emits_at(&self, x: f32, y: f32) -> &[Emit] {
        emits_in(&self.hits, x, y)
    }
}

/// The topmost (last in paint order) region's events under a point.
pub fn emits_in(hits: &[Hit], x: f32, y: f32) -> &[Emit] {
    hits.iter()
        .rev()
        .find(|h| {
            x >= h.rect.x && x <= h.rect.x + h.rect.w && y >= h.rect.y && y <= h.rect.y + h.rect.h
        })
        .map_or(&[], |h| h.emits.as_slice())
}

/// Patterns the scene mapper draws itself: the learning canvases, the diagram patterns and the
/// game canvases.
pub fn is_scene_pattern(pattern: &str) -> bool {
    learning::handles(pattern)
        || diagram::handles(pattern)
        || matches!(pattern, "canvas" | "canvas-2d")
}

/// Every registry pattern a host draws as a Rust scene: the learning canvases, the diagrams, the
/// `svg-*` atoms and the game `canvas`. A [`diagram::SCENE_ONLY`] scene names no registry pattern.
pub fn scene_patterns() -> impl Iterator<Item = &'static str> {
    learning::PATTERNS
        .iter()
        .chain(diagram::PATTERNS)
        .chain(diagram::svg_atoms::ATOMS)
        .chain(&["canvas", "canvas-2d"])
        .copied()
}

/// The node a host hands a scene: the pattern's props under its `type`, plus the kernel's
/// `canvasTheme` when the host has one (absent, scenes paint with their built-in palette).
pub fn host_node(pattern: &str, props: Value, canvas_theme: Option<&Value>) -> Value {
    let mut fields = match props {
        Value::Object(fields) => fields,
        _ => serde_json::Map::new(),
    };
    fields.insert("type".to_string(), Value::String(pattern.to_string()));
    if let Some(theme) = canvas_theme {
        fields.insert("canvasTheme".to_string(), theme.clone());
    }
    Value::Object(fields)
}

/// Map a render-ui node to its scene. The node's `type` names the pattern: the
/// learning canvases go to [`learning::map`], everything else is the game
/// `canvas` host ([`game_canvas_2d`]). `viewport` is the host surface in
/// logical points; absent, scenes use their own authored size.
pub fn map_scene(
    node: &Value,
    viewport: Option<(f32, f32)>,
) -> Result<MappedScene, CanvasSceneError> {
    let obj = node.as_object().ok_or(CanvasSceneError::NotAnObject)?;
    let fields = match obj.get("props") {
        Some(Value::Object(nested)) => nested,
        _ => obj,
    };
    let pattern = obj
        .get("type")
        .or_else(|| fields.get("type"))
        .and_then(Value::as_str);
    if let Some(pattern) = pattern.filter(|p| learning::handles(p)) {
        return learning::map(pattern, &Value::Object(fields.clone()), viewport);
    }
    if let Some(pattern) = pattern.filter(|p| diagram::handles(p)) {
        return diagram::map(pattern, &Value::Object(fields.clone()), viewport);
    }
    let packet = match viewport {
        Some((w, h)) => game_canvas_2d::from_canvas_node_in_viewport(node, w, h)?,
        None => game_canvas_2d::from_canvas_node(node)?,
    };
    Ok(MappedScene::drawn(packet))
}

/// [`map_scene`] with the renderer's memory of this scene: a graph keeps its settled layout and
/// view across re-maps, and applies the pan and zoom gestures queued on `memory`.
pub fn map_scene_with(
    node: &Value,
    viewport: Option<(f32, f32)>,
    memory: &mut SceneMemory,
) -> Result<MappedScene, CanvasSceneError> {
    let obj = node.as_object().ok_or(CanvasSceneError::NotAnObject)?;
    let fields = match obj.get("props") {
        Some(Value::Object(nested)) => nested,
        _ => obj,
    };
    let pattern = obj
        .get("type")
        .or_else(|| fields.get("type"))
        .and_then(Value::as_str);
    if memory.pattern.as_deref() != pattern {
        *memory = SceneMemory {
            pattern: pattern.map(str::to_string),
            ..SceneMemory::default()
        };
    }
    if pattern == Some("graph-canvas") {
        return learning::graph_canvas::map_with(&Value::Object(fields.clone()), viewport, memory);
    }
    map_scene(node, viewport)
}

/// The scene's own size in logical units (what the host sizes its surface to by aspect ratio):
/// the authored `width` x `height` of a learning canvas, the default board of a game canvas.
pub fn scene_size(node: &Value) -> Result<(f32, f32), CanvasSceneError> {
    let mapped = map_scene(node, None)?;
    Ok((mapped.packet.width, mapped.packet.height))
}

#[cfg(test)]
mod tests {
    #[test]
    fn scene_patterns_are_the_registry_names_every_host_draws_as_a_scene() {
        let names: Vec<&str> = super::scene_patterns().collect();
        for name in [
            "graph-canvas",
            "state-machine-view",
            "svg-flow",
            "canvas",
            "canvas-2d",
        ] {
            assert!(names.contains(&name), "{name}");
        }
        assert!(names.iter().all(|n| super::is_scene_pattern(n)));
        assert!(
            !names.contains(&"mermaid"),
            "control: a scene-only kind names no pattern"
        );
        assert!(
            !names.contains(&"button"),
            "control: a component is not a scene"
        );
    }

    use super::*;
    use serde_json::json;

    #[test]
    fn a_callback_payload_is_delivered_under_its_declared_argument_name() {
        let wrapped = Emit {
            event: "E".into(),
            arg: "payload".into(),
            payload: json!({"id": "a"}),
        };
        assert_eq!(wrapped.delivered(), json!({"payload": {"id": "a"}}));
        let flat = Emit {
            event: "E".into(),
            arg: String::new(),
            payload: json!({"id": "a"}),
        };
        assert_eq!(
            flat.delivered(),
            json!({"id": "a"}),
            "control: an event-ref payload is not wrapped"
        );
    }

    #[test]
    fn a_learning_scene_reports_its_authored_size_and_a_control_prop_changes_it() {
        let node = json!({"type": "learning-canvas", "width": 600, "height": 380});
        assert_eq!(scene_size(&node).unwrap(), (600.0, 380.0));
        let taller = json!({"type": "learning-canvas", "width": 600, "height": 500});
        assert_eq!(scene_size(&taller).unwrap(), (600.0, 500.0));
        let nested = json!({"type": "math-canvas", "props": {"width": 300, "height": 200}});
        assert_eq!(scene_size(&nested).unwrap(), (300.0, 200.0));
    }

    #[test]
    fn the_type_picks_the_mapper_and_an_unmapped_pattern_is_refused() {
        let learning = map_scene(&json!({"type": "learning-canvas"}), None).unwrap();
        assert_eq!(learning.packet.pattern, "learning-canvas");
        let game = map_scene(&json!({"type": "canvas-2d"}), None).unwrap();
        assert_eq!(game.packet.pattern, "canvas-2d");
        assert_eq!(
            map_scene(&json!(3), None).unwrap_err(),
            CanvasSceneError::NotAnObject
        );
    }
}
