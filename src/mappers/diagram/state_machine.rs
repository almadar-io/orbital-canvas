//! The AVL state-machine diagram (`AvlStateMachine.tsx`) shared by `state-machine-view`,
//! `jazari-state-machine` (gear nodes) and `state-graph`: a trait's states and transitions laid
//! out left to right, themed by tone, with the execution position (active state, fired
//! transition, visited path). Layout is [`super::layered`] (the web uses ELK).

use super::draw::{tint, with_alpha, Ink, Motion, Pen};
use super::effect_glyph;
use super::layered::{self, EdgeIn, Layout, Metrics, NodeIn};
use crate::mappers::learning::theme::Rgba;
use crate::mappers::scene::Emit;
use crate::packet::{Easing, Keyframe, TextAlign};
use crate::vector::{mono_text_width, text_width};
use orbital_core::schema::{format_sexpr, SExpression};
use serde_json::{json, Value};
use std::f32::consts::TAU;

/// `strokeOpacity` of the active-state ring; its SMIL animation swings it 0.28 -> 0.06 -> 0.28.
const ACTIVE_RING_OPACITY: f32 = 0.22;

const LABEL_PX: f32 = 12.0;
/// `MONO_CHAR`: the web sizes a pill by its label's length in mono chars, not measured ink.
const MONO_CHAR: f32 = 7.3;

/// The web's `text.length * MONO_CHAR` (`length` counts UTF-16 units).
fn mono_chars(text: &str) -> f32 {
    text.encode_utf16().count() as f32 * MONO_CHAR
}
const BODY_CHAR: f32 = 7.6;
const NODE_H: f32 = 44.0;
const PILL_H: f32 = 24.0;
const GUARD_H: f32 = 20.0;
const EFFECT_CHIP: f32 = 14.0;
const EFFECT_GAP: f32 = 4.0;
pub const MARGIN: f32 = 28.0;
const HEADER_ROW: f32 = 24.0;
const HEADER_PAD: f32 = 12.0;
const RAIL_H: f32 = 20.0;

#[derive(Debug, Clone)]
pub struct StateInfo {
    pub name: String,
    pub is_initial: bool,
    pub is_terminal: bool,
}

#[derive(Debug, Clone)]
pub struct TransitionInfo {
    pub from: String,
    pub to: String,
    pub event: String,
    pub guard: Option<String>,
    pub effects: Vec<String>,
    pub index: usize,
}

/// `TraitLevelData`.
#[derive(Debug, Clone, Default)]
pub struct TraitData {
    pub name: String,
    pub linked_entity: String,
    pub states: Vec<StateInfo>,
    pub transitions: Vec<TransitionInfo>,
    pub emitted: Vec<String>,
    pub listened: Vec<String>,
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn flag(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn guard_of(v: Option<&Value>) -> Option<String> {
    v.filter(|g| !g.is_null())
        .and_then(|g| serde_json::from_value::<SExpression>(g.clone()).ok())
        .map(|g| format_sexpr(&g))
}

impl TraitData {
    /// The `trait` prop of `state-machine-view` / `avl-state-machine`: `TraitLevelData` JSON.
    pub fn from_level(v: &Value) -> Option<Self> {
        v.as_object()?;
        let states = v
            .get("states")?
            .as_array()?
            .iter()
            .map(|s| StateInfo {
                name: s
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                is_initial: flag(s, "isInitial"),
                is_terminal: flag(s, "isTerminal"),
            })
            .collect();
        let transitions = v
            .get("transitions")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .enumerate()
                    .map(|(i, t)| TransitionInfo {
                        from: t
                            .get("from")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        to: t
                            .get("to")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        event: t
                            .get("event")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        guard: guard_of(t.get("guard")),
                        effects: t
                            .get("effects")
                            .and_then(Value::as_array)
                            .map(|fx| {
                                fx.iter()
                                    .map(|e| {
                                        e.get("type")
                                            .and_then(Value::as_str)
                                            .unwrap_or("unknown")
                                            .to_string()
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                        index: t
                            .get("index")
                            .and_then(Value::as_u64)
                            .map_or(i, |n| n as usize),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Self {
            name: v
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            linked_entity: v
                .get("linkedEntity")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            states,
            transitions,
            emitted: strings(v.get("emittedEvents")),
            listened: strings(v.get("listenedEvents")),
        })
    }

    /// `traitLevelFromTrait`: one inline `Trait` JSON (`stateMachine`, `emits`, `listens`).
    pub fn from_trait(t: &Value, linked_entity: &str) -> Option<Self> {
        let sm = t.get("stateMachine").filter(|s| s.is_object())?;
        let states = sm
            .get("states")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|s| StateInfo {
                        name: s
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        is_initial: flag(s, "isInitial"),
                        is_terminal: flag(s, "isTerminal") || flag(s, "isFinal"),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let transitions = sm
            .get("transitions")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .enumerate()
                    .map(|(i, tr)| TransitionInfo {
                        from: tr
                            .get("from")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        to: tr
                            .get("to")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        event: tr
                            .get("event")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        guard: guard_of(tr.get("guard")),
                        effects: tr
                            .get("effects")
                            .and_then(Value::as_array)
                            .map(|fx| {
                                fx.iter()
                                    .map(|e| match e.as_array().and_then(|a| a.first()) {
                                        Some(head) => head
                                            .as_str()
                                            .map_or_else(|| head.to_string(), str::to_string),
                                        None => "unknown".into(),
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                        index: i,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let events = |key: &str| -> Vec<String> {
            t.get(key)
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|e| {
                            e.get("event")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string()
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        Some(Self {
            name: t
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            linked_entity: linked_entity.to_string(),
            states,
            transitions,
            emitted: events("emits"),
            listened: events("listens"),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeShape {
    Pill,
    Gear,
}

#[derive(Debug, Clone)]
pub struct Opts {
    pub node_shape: NodeShape,
    pub rtl: bool,
    pub show_header: bool,
    pub entity_fields: Vec<String>,
    pub active_state: Option<String>,
    pub active_transition: Option<usize>,
    pub visited: Vec<String>,
    pub selected_state: Option<String>,
    pub pending_source: Option<String>,
    pub state_click: Option<String>,
    pub transition_click: Option<String>,
    pub on_transition_click: Option<String>,
}

impl Opts {
    pub fn from_props(props: &Value) -> Self {
        let s = |k: &str| {
            props
                .get(k)
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
        };
        Self {
            node_shape: if s("nodeShape").as_deref() == Some("gear") {
                NodeShape::Gear
            } else {
                NodeShape::Pill
            },
            rtl: s("direction").as_deref() == Some("rtl"),
            show_header: props
                .get("showHeader")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            entity_fields: strings(props.get("entityFields")),
            active_state: s("activeState"),
            active_transition: props
                .get("activeTransition")
                .and_then(Value::as_u64)
                .map(|n| n as usize),
            visited: strings(props.get("visitedStates")),
            selected_state: s("selectedState"),
            pending_source: s("pendingSourceState"),
            state_click: s("stateClickEvent"),
            transition_click: s("transitionClickEvent"),
            on_transition_click: s("onTransitionClick"),
        }
    }
}

fn pill_width(t: &TransitionInfo) -> f32 {
    20.0 + if t.guard.is_some() { 16.0 } else { 0.0 }
        + mono_chars(&t.event)
        + t.effects.len() as f32 * (EFFECT_CHIP + EFFECT_GAP)
}

fn label_size(t: &TransitionInfo) -> (f32, f32) {
    match &t.guard {
        None => (pill_width(t), PILL_H),
        Some(g) => (
            pill_width(t).max(mono_chars(g) + 8.0),
            PILL_H + 2.0 * GUARD_H,
        ),
    }
}

fn gear_diameter(name: &str) -> f32 {
    96.0f32.max((name.chars().count() as f32 * BODY_CHAR + 40.0).round())
}

fn node_size(name: &str, shape: NodeShape) -> (f32, f32) {
    match shape {
        NodeShape::Gear => (gear_diameter(name), gear_diameter(name)),
        NodeShape::Pill => (
            112.0f32.max((name.chars().count() as f32 * BODY_CHAR + 44.0).round()),
            NODE_H,
        ),
    }
}

/// A laid-out diagram: its natural size and everything needed to paint it.
pub struct Diagram<'a> {
    pub trait_data: &'a TraitData,
    pub opts: &'a Opts,
    pub layout: Layout,
    pub width: f32,
    pub height: f32,
    header: Header,
}

#[derive(Default)]
struct Header {
    width: f32,
    height: f32,
    rails: Vec<(String, String)>,
    fields: Option<String>,
}

fn rail_width(kind: &str, event: &str) -> f32 {
    16.0 + mono_text_width(kind, LABEL_PX) + 4.0 + mono_text_width(event, LABEL_PX)
}

pub fn prepare<'a>(t: &'a TraitData, o: &'a Opts, scene_hint: f32) -> Diagram<'a> {
    let nodes: Vec<NodeIn> = t
        .states
        .iter()
        .map(|s| {
            let (w, h) = node_size(&s.name, o.node_shape);
            NodeIn {
                w,
                h,
                initial: s.is_initial,
            }
        })
        .collect();
    let index = |name: &str| t.states.iter().position(|s| s.name == name);
    let mut edges = Vec::new();
    let mut edge_of = Vec::new();
    for (i, tr) in t.transitions.iter().enumerate() {
        if let (Some(from), Some(to)) = (index(&tr.from), index(&tr.to)) {
            let (label_w, label_h) = label_size(tr);
            edges.push(EdgeIn {
                from,
                to,
                label_w,
                label_h,
            });
            edge_of.push(i);
        }
    }
    let mut layout = layered::layout(
        &nodes,
        &edges,
        Metrics {
            node_spacing: 40.0,
            layer_spacing: 88.0,
            rtl: o.rtl,
        },
    );
    let mut full_edges = vec![None; t.transitions.len()];
    for (k, e) in layout.edges.drain(..).enumerate() {
        full_edges[edge_of[k]] = Some(e);
    }
    layout.edges = full_edges
        .into_iter()
        .map(|e| {
            e.unwrap_or(layered::EdgeOut {
                points: Vec::new(),
                label: layered::Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 0.0,
                    h: 0.0,
                },
                is_self: false,
                is_backward: false,
            })
        })
        .collect();

    let mut header = Header::default();
    let body_w = layout.width + 2.0 * MARGIN;
    if o.show_header {
        let fields = (!o.entity_fields.is_empty())
            .then(|| format!("{} {{ {} }}", t.linked_entity, o.entity_fields.join(", ")));
        let mut rails: Vec<(String, String)> = t
            .listened
            .iter()
            .map(|e| ("listens".to_string(), e.clone()))
            .collect();
        rails.extend(t.emitted.iter().map(|e| ("emits".to_string(), e.clone())));
        let rail_total: f32 = rails
            .iter()
            .map(|(k, e)| rail_width(k, e) + 8.0)
            .sum::<f32>()
            + fields
                .as_ref()
                .map_or(0.0, |f| mono_text_width(f, LABEL_PX) + 8.0);
        let widest = rails
            .iter()
            .map(|(k, e)| rail_width(k, e))
            .chain(fields.as_ref().map(|f| mono_text_width(f, LABEL_PX)))
            .fold(0.0, f32::max);
        let width = body_w.max(scene_hint).max(widest);
        let title_w = text_width(&t.name, 16.0) + 12.0 + text_width(&caption(t), 12.0);
        let rows = if rail_total == 0.0 || title_w + 8.0 + rail_total <= width {
            1.0
        } else {
            1.0 + (rail_total / width).ceil()
        };
        header.width = width;
        header.height = HEADER_ROW + (rows - 1.0) * (RAIL_H + 4.0) + HEADER_PAD;
        header.rails = rails;
        header.fields = fields;
    }
    let width = body_w.max(scene_hint).max(header.width);
    let height = header.height + layout.height + 2.0 * MARGIN;
    Diagram {
        trait_data: t,
        opts: o,
        layout,
        width,
        height,
        header,
    }
}

/// One entry on the header's rails row: the entity-fields chip or a listens/emits rail.
enum HeaderItem {
    Fields(String, f32),
    Rail(String, String, f32),
}

impl HeaderItem {
    fn width(&self) -> f32 {
        match self {
            Self::Fields(_, w) | Self::Rail(_, _, w) => *w,
        }
    }
}

fn caption(t: &TraitData) -> String {
    format!(
        "on {} \u{b7} states: {} \u{b7} transitions: {}",
        t.linked_entity,
        t.states.len(),
        t.transitions.len()
    )
}

/// An orthogonal polyline with its corners rounded to `radius` (clamped to half of each leg).
fn rounded(points: &[(f32, f32)], radius: f32) -> Vec<(f32, f32)> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let mut out = vec![points[0]];
    for i in 1..points.len() - 1 {
        let (prev, cur, next) = (points[i - 1], points[i], points[i + 1]);
        let in_len = (cur.0 - prev.0).hypot(cur.1 - prev.1);
        let out_len = (next.0 - cur.0).hypot(next.1 - cur.1);
        let r = radius.min(in_len / 2.0).min(out_len / 2.0);
        if r <= 0.0 {
            out.push(cur);
            continue;
        }
        let a = (
            cur.0 - (cur.0 - prev.0) / in_len * r,
            cur.1 - (cur.1 - prev.1) / in_len * r,
        );
        let b = (
            cur.0 + (next.0 - cur.0) / out_len * r,
            cur.1 + (next.1 - cur.1) / out_len * r,
        );
        out.extend(Pen::quad_points(a, cur, b));
    }
    out.push(points[points.len() - 1]);
    out
}

pub(super) fn arrow_head(
    pen: &mut Pen,
    tip_from: (f32, f32),
    tip: (f32, f32),
    size: f32,
    stroke: f32,
    color: Rgba,
) {
    let theta = (tip.1 - tip_from.1).atan2(tip.0 - tip_from.0);
    let (sin, cos) = theta.sin_cos();
    let k = size * stroke / 10.0;
    let at = |mx: f32, my: f32| {
        let (dx, dy) = ((mx - 9.0) * k, (my - 5.0) * k);
        (tip.0 + dx * cos - dy * sin, tip.1 + dx * sin + dy * cos)
    };
    pen.polyline(
        &[at(0.0, 0.0), at(10.0, 5.0), at(0.0, 10.0)],
        true,
        Some(color),
        None,
    );
}

fn gear_points(cx: f32, cy: f32, inner: f32, outer: f32, teeth: usize) -> Vec<(f32, f32)> {
    let step = TAU / teeth as f32;
    let half = step * 0.25;
    let mut pts = Vec::with_capacity(teeth * 4);
    for i in 0..teeth {
        let base = i as f32 * step;
        for (r, a) in [
            (inner, base - half),
            (outer, base - half * 0.4),
            (outer, base + half * 0.4),
            (inner, base + half),
        ] {
            pts.push((cx + r * a.cos(), cy + r * a.sin()));
        }
    }
    pts
}

impl Diagram<'_> {
    /// Paint at the origin of the pen's current transform; returns the tappable regions' emits.
    pub fn paint(&self, pen: &mut Pen) {
        let t = self.trait_data;
        let o = self.opts;
        let ink = pen.tone("ink");
        let quiet = pen.tone("label");
        let line = pen.tone("grid");
        let surface = pen.card();
        let focus = pen.tone("primary");
        let wash = pen.muted();

        if t.states.is_empty() {
            self.paint_header(pen);
            let y = self.header.height;
            pen.rrect(
                0.0,
                y,
                self.width,
                64.0,
                6.0,
                None,
                Some(&Ink::new(line, 1.0).dashed(&[4.0, 3.0])),
            );
            pen.text(
                self.width / 2.0,
                y + 36.0,
                "No state machine",
                12.0,
                quiet,
                TextAlign::Center,
                400,
            );
            return;
        }
        self.paint_header(pen);
        let (ox, oy) = (MARGIN, self.header.height + MARGIN);
        let at = |p: (f32, f32)| (p.0 + ox, p.1 + oy);
        let idx = |name: &str| t.states.iter().position(|s| s.name == name);

        for (i, _) in t.states.iter().enumerate().filter(|(_, s)| s.is_initial) {
            let n = self.layout.nodes[i];
            let cy = n.y + n.h / 2.0;
            let (dot, a, b) = if o.rtl {
                (n.x + n.w + 18.0, n.x + n.w + 13.0, n.x + n.w + 1.0)
            } else {
                (n.x - 18.0, n.x - 13.0, n.x - 1.0)
            };
            let c = at((dot, cy));
            pen.circle(c.0, c.1, 5.0, Some(ink), None);
            let (pa, pb) = (at((a, cy)), at((b, cy)));
            pen.line(pa, pb, &Ink::new(quiet, 1.5));
            arrow_head(pen, pa, pb, 7.0, 1.5, with_alpha(quiet, 0.8));
        }
        for (ti, tr) in t.transitions.iter().enumerate() {
            let e = &self.layout.edges[ti];
            if e.points.len() < 2 {
                continue;
            }
            let fired = o.active_transition == Some(tr.index);
            let pts: Vec<(f32, f32)> = e.points.iter().map(|p| at(*p)).collect();
            let color = if fired {
                focus
            } else {
                with_alpha(quiet, 0.55)
            };
            let width = if fired { 2.5 } else { 1.5 };
            let mut stroke = Ink::new(color, width);
            if fired {
                stroke = stroke.dashed(&[8.0, 6.0]);
            } else if e.is_backward || e.is_self {
                stroke = stroke.dashed(&[5.0, 4.0]);
            }
            let wire = rounded(&pts, 10.0);
            if fired {
                let march = Motion::new(
                    800.0,
                    Easing::Linear,
                    vec![
                        Keyframe {
                            offset: 0.0,
                            dash_offset: Some(0.0),
                            ..Keyframe::default()
                        },
                        Keyframe {
                            offset: 1.0,
                            dash_offset: Some(-14.0),
                            ..Keyframe::default()
                        },
                    ],
                );
                pen.animated((0.0, 0.0), march, |pen| {
                    pen.polyline(&wire, false, None, Some(&stroke));
                });
            } else {
                pen.polyline(&wire, false, None, Some(&stroke));
            }
            let last = pts[pts.len() - 1];
            let before = pts[pts.len() - 2];
            if fired {
                arrow_head(pen, before, last, 6.0, width, focus);
            } else {
                arrow_head(pen, before, last, 7.0, width, with_alpha(quiet, 0.8));
            }
        }
        if let Some(i) = o.active_state.as_deref().and_then(idx) {
            let n = self.layout.nodes[i];
            let p = at((n.x, n.y));
            let breathe = {
                let at = |offset: f32, stroke_opacity: f32| Keyframe {
                    offset,
                    opacity: Some(stroke_opacity / ACTIVE_RING_OPACITY),
                    ..Keyframe::default()
                };
                Motion::new(
                    1600.0,
                    Easing::Linear,
                    vec![at(0.0, 0.28), at(0.5, 0.06), at(1.0, 0.28)],
                )
            };
            pen.animated((0.0, 0.0), breathe, |pen| {
                pen.rrect(
                    p.0 - 5.0,
                    p.1 - 5.0,
                    n.w + 10.0,
                    n.h + 10.0,
                    12.0,
                    None,
                    Some(&Ink::new(with_alpha(focus, ACTIVE_RING_OPACITY), 4.0)),
                );
            });
        }

        let next_from_active: Vec<usize> = t
            .transitions
            .iter()
            .filter(|tr| o.active_state.as_deref() == Some(tr.from.as_str()))
            .map(|tr| tr.index)
            .collect();
        let click_enabled = o.transition_click.is_some() || o.on_transition_click.is_some();
        for (ti, tr) in t.transitions.iter().enumerate() {
            let e = &self.layout.edges[ti];
            if e.label.w <= 0.0 {
                continue;
            }
            let fired = o.active_transition == Some(tr.index);
            let next = !fired && next_from_active.contains(&tr.index);
            let pw = pill_width(tr);
            let px = ox + e.label.x + (e.label.w - pw) / 2.0;
            let content_h = if tr.guard.is_some() {
                PILL_H + GUARD_H
            } else {
                PILL_H
            };
            let py = oy + e.label.y + (e.label.h - content_h) / 2.0;
            let border = if fired { focus } else { line };
            let mut b = Ink::new(border, 1.0);
            if next {
                b = b.dashed(&[3.0, 2.0]);
            }
            pen.rrect(px, py, pw, PILL_H, 6.0, Some(surface), Some(&b));
            let text_color = if fired { focus } else { ink };
            let mut tx = px + 8.0;
            if tr.guard.is_some() {
                let (gx, gy) = (tx + 6.0, py + PILL_H / 2.0);
                let g = if fired { focus } else { quiet };
                pen.polyline(
                    &[
                        (gx, gy - 5.0),
                        (gx + 5.0, gy),
                        (gx, gy + 5.0),
                        (gx - 5.0, gy),
                    ],
                    true,
                    Some(surface),
                    Some(&Ink::new(g, 1.2)),
                );
                tx += 16.0;
            }
            pen.mono_text_middle(
                tx,
                py + PILL_H / 2.0,
                &tr.event,
                12.0,
                text_color,
                TextAlign::Left,
                if fired { 600 } else { 400 },
            );
            let mut cx = tx + mono_text_width(&tr.event, LABEL_PX) + EFFECT_GAP;
            for fx in &tr.effects {
                let (mx, my) = (cx + EFFECT_CHIP / 2.0, py + PILL_H / 2.0);
                effect_glyph::chip(pen, fx, mx, my, EFFECT_CHIP);
                cx += EFFECT_CHIP + EFFECT_GAP;
            }
            if let Some(g) = &tr.guard {
                let color = if fired { focus } else { quiet };
                pen.mono_text_middle(
                    px + pw / 2.0,
                    py + PILL_H + GUARD_H / 2.0,
                    g,
                    12.0,
                    color,
                    TextAlign::Center,
                    400,
                );
            }
            if click_enabled {
                let payload =
                    json!({"index": tr.index, "event": tr.event, "from": tr.from, "to": tr.to});
                let mut emits = Vec::new();
                if let Some(ev) = &o.on_transition_click {
                    emits.push(Emit {
                        event: ev.clone(),
                        arg: "transition".into(),
                        payload: payload.clone(),
                    });
                }
                if let Some(ev) = &o.transition_click {
                    emits.push(Emit {
                        event: ev.clone(),
                        arg: String::new(),
                        payload,
                    });
                }
                pen.hit(ox + e.label.x, oy + e.label.y, e.label.w, e.label.h, emits);
            }
        }

        for (i, s) in t.states.iter().enumerate() {
            let n = self.layout.nodes[i];
            let (x, y) = (n.x + ox, n.y + oy);
            let active = o.active_state.as_deref() == Some(s.name.as_str());
            let selected = o.selected_state.as_deref() == Some(s.name.as_str());
            let pending = o.pending_source.as_deref() == Some(s.name.as_str());
            let visit = o.visited.iter().position(|v| *v == s.name).map(|p| p + 1);
            let idle = o.active_state.is_some() && !active && visit.is_none();
            let emphasis = active || selected || pending;
            let border = if emphasis { focus } else { line };
            match o.node_shape {
                NodeShape::Pill => {
                    if s.is_terminal {
                        pen.rrect(
                            x - 4.0,
                            y - 4.0,
                            n.w + 8.0,
                            n.h + 8.0,
                            12.0,
                            None,
                            Some(&Ink::new(line, 1.0)),
                        );
                    }
                    if selected {
                        pen.rrect(
                            x - 2.0,
                            y - 2.0,
                            n.w + 4.0,
                            n.h + 4.0,
                            10.0,
                            None,
                            Some(&Ink::new(tint(focus, 18.0), 4.0)),
                        );
                    }
                    let mut b = Ink::new(border, if emphasis { 2.0 } else { 1.0 });
                    if pending {
                        b = b.dashed(&[4.0, 3.0]);
                    }
                    pen.rrect(x, y, n.w, n.h, 8.0, Some(surface), Some(&b));
                }
                NodeShape::Gear => {
                    let (cx, cy) = (x + n.w / 2.0, y + n.h / 2.0);
                    let pts = gear_points(cx, cy, n.w / 2.0 - 8.0, n.w / 2.0 - 1.0, 12);
                    let mut b = Ink::new(border, if emphasis { 2.0 } else { 1.25 });
                    if pending || s.is_terminal {
                        b = b.dashed(&[4.0, 3.0]);
                    }
                    pen.polyline(&pts, true, Some(surface), Some(&b));
                    pen.circle(cx, cy, n.w / 2.0 - 14.0, None, Some(&Ink::new(line, 1.0)));
                }
            }
            let name_color = if idle { quiet } else { ink };
            pen.text_middle(
                x + n.w / 2.0,
                y + n.h / 2.0,
                &s.name,
                13.0,
                name_color,
                TextAlign::Center,
                if active { 600 } else { 500 },
            );
            if let (Some(v), false) = (visit, active) {
                pen.circle(x, y, 9.0, Some(wash), Some(&Ink::new(line, 1.0)));
                pen.text_middle(x, y, &v.to_string(), 11.0, quiet, TextAlign::Center, 600);
            }
            if active {
                let w = text_width("now", 11.0) + 12.0;
                let (bx, by) = (x + n.w - 8.0 - w, y - 10.0);
                pen.rrect(bx, by, w, 18.0, 9.0, Some(focus), None);
                pen.text_middle(
                    bx + w / 2.0,
                    by + 9.0,
                    "now",
                    11.0,
                    surface,
                    TextAlign::Center,
                    600,
                );
            }
            if o.state_click.is_some() {
                let emits = o
                    .state_click
                    .iter()
                    .map(|ev| Emit {
                        event: ev.clone(),
                        arg: String::new(),
                        payload: json!({ "stateId": s.name }),
                    })
                    .collect();
                pen.hit(x, y, n.w, n.h, emits);
            }
        }
    }

    fn paint_header(&self, pen: &mut Pen) {
        let t = self.trait_data;
        if !self.opts.show_header {
            return;
        }
        let ink = pen.tone("ink");
        let quiet = pen.tone("label");
        let line = pen.tone("grid");
        let wash = pen.muted();
        let title_w = text_width(&t.name, 16.0);
        pen.text_middle(
            0.0,
            HEADER_ROW / 2.0,
            &t.name,
            16.0,
            ink,
            TextAlign::Left,
            600,
        );
        pen.text_middle(
            title_w + 12.0,
            HEADER_ROW / 2.0,
            &caption(t),
            12.0,
            quiet,
            TextAlign::Left,
            400,
        );
        let mut items: Vec<HeaderItem> = Vec::new();
        if let Some(f) = &self.header.fields {
            items.push(HeaderItem::Fields(f.clone(), mono_text_width(f, LABEL_PX)));
        }
        for (k, e) in &self.header.rails {
            items.push(HeaderItem::Rail(k.clone(), e.clone(), rail_width(k, e)));
        }
        let total: f32 = items.iter().map(|i| i.width() + 8.0).sum::<f32>()
            - if items.is_empty() { 0.0 } else { 8.0 };
        let title_end = title_w + 12.0 + text_width(&caption(t), 12.0);
        let same_row = !items.is_empty() && title_end + 8.0 + total <= self.width;
        let (mut x, mut y) = if same_row {
            (self.width - total, HEADER_ROW / 2.0)
        } else {
            (0.0, HEADER_ROW + RAIL_H / 2.0 + 2.0)
        };
        for item in items {
            let w = item.width();
            if !same_row && x + w > self.width && x > 0.0 {
                x = 0.0;
                y += RAIL_H + 4.0;
            }
            match item {
                HeaderItem::Fields(text, _) => {
                    pen.mono_text_middle(x, y, &text, 12.0, quiet, TextAlign::Left, 400)
                }
                HeaderItem::Rail(kind, event, _) => {
                    pen.rrect(
                        x,
                        y - RAIL_H / 2.0,
                        w,
                        RAIL_H,
                        RAIL_H / 2.0,
                        Some(wash),
                        Some(&Ink::new(line, 1.0)),
                    );
                    pen.mono_text_middle(x + 8.0, y, &kind, 12.0, quiet, TextAlign::Left, 400);
                    pen.mono_text_middle(
                        x + 8.0 + mono_text_width(&kind, LABEL_PX) + 4.0,
                        y,
                        &event,
                        12.0,
                        ink,
                        TextAlign::Left,
                        600,
                    );
                }
            }
            x += w + 8.0;
        }
    }
}
