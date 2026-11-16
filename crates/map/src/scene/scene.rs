//! One render-ui scene mapped for native: the display-list packet plus what the
//! host needs to turn a tap into an event.

use orbital_canvas_base::packet::{GameRenderPacket, Rect};
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

/// The events a pointer region sends, each naming the trait event it dispatches (the canvas's `pointer*Event` props).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PointerEvents {
    pub down: Option<String>,
    pub up: Option<String>,
    pub enter: Option<String>,
    pub leave: Option<String>,
    pub moved: Option<String>,
}

/// A drawable with `id` and `hit: true`: its painted bounds in packet coordinates and its rotation in radians about
/// their centre.
#[derive(Debug, Clone, PartialEq)]
pub struct PointerRegion {
    pub id: String,
    pub rect: Rect,
    pub rotation: f32,
    pub events: PointerEvents,
    /// The drawable declared `wheel: true`.
    pub wheel: bool,
    /// The drawable, or a group holding it, declared `fixed: true`: it stays put when the page scrolls.
    pub fixed: bool,
    /// The drawable declared `drag: true`: a press on it keeps the pointer's travel, the page does not scroll.
    pub drag: bool,
}

/// One node of a canvas's semantic tree: a drawable with `semantic` and `id`, at its pointer region
/// (or its painted bounds when it has no `hit`), in packet units.
#[derive(Debug, Clone, PartialEq)]
pub struct SemanticNode {
    pub id: String,
    pub role: String,
    pub name: String,
    pub value: Option<String>,
    pub checked: Option<bool>,
    pub disabled: bool,
    pub focused: bool,
    pub caret: Option<usize>,
    pub selection_start: Option<usize>,
    pub selection_end: Option<usize>,
    pub multiline: bool,
    pub order: f32,
    pub rect: Rect,
    /// The node stays put when the page scrolls.
    pub fixed: bool,
}

/// One laid-out line of a hit `draw-text`: its vertical band and the left edge of each caret position, so a press
/// reports the caret index nearest it (contract §11). `start` is the line's first character index in the text.
#[derive(Debug, Clone, PartialEq)]
pub struct TextHitLine {
    pub top: f32,
    pub bottom: f32,
    pub start: usize,
    pub edges: Vec<f32>,
}

/// A hit `draw-text`'s caret positions, in packet units.
#[derive(Debug, Clone, PartialEq)]
pub struct TextHit {
    pub id: String,
    pub fixed: bool,
    pub lines: Vec<TextHitLine>,
}

impl TextHit {
    /// The caret index nearest the point: the line whose band holds `y` (else the nearest line), then its nearest edge.
    pub fn index_at(&self, x: f32, y: f32) -> Option<usize> {
        let line = self.lines.iter().min_by(|a, b| {
            let dist = |l: &TextHitLine| if y < l.top { l.top - y } else if y > l.bottom { y - l.bottom } else { 0.0 };
            dist(a).total_cmp(&dist(b))
        })?;
        let (i, _) = line.edges.iter().enumerate().min_by(|(_, a), (_, b)| (*a - x).abs().total_cmp(&(*b - x).abs()))?;
        Some(line.start + i)
    }
}

/// The size of a `draw-text` with `measure: true`, in packet units.
#[derive(Debug, Clone, PartialEq)]
pub struct Measured {
    pub id: String,
    pub width: f32,
    pub height: f32,
    pub lines: u32,
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
    /// Pointer regions in paint order; the topmost (last) region containing a point wins.
    pub pointer_regions: Vec<PointerRegion>,
    /// Every `draw-text` with `measure: true` and an `id`, in paint order.
    pub measured: Vec<Measured>,
    /// The caret positions of every hit `draw-text`, in paint order.
    pub text_hits: Vec<TextHit>,
    /// The semantic tree, in reading order (`order`, then paint order).
    pub semantic_nodes: Vec<SemanticNode>,
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

