//! The AVL state-machine diagram (`AvlStateMachine.tsx`) shared by `state-machine-view`,
//! `jazari-state-machine` (gear nodes) and `state-graph`: a trait's states and transitions laid
//! out left to right, themed by tone, with the execution position (active state, fired
//! transition, visited path). Layout is [`super::layered`] (the web uses ELK).

use super::draw::{mix, tint, with_alpha, Ink, Motion, Pen};
use super::effect_glyph;
use super::layered::{self, EdgeIn, Layout, Metrics, NodeIn};
use crate::mappers::learning::theme::Rgba;
use crate::mappers::scene::Emit;
use crate::packet::{Easing, Keyframe, TextAlign};
use crate::vector::{mono_text_width, text_width};
use orbital_core_base::schema::{format_sexpr, SExpression};
use serde_json::{json, Value};
use std::collections::HashMap;
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

fn source_names(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::String(from)) => vec![from.clone()],
        Some(Value::Array(froms)) => froms
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
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
                    .flat_map(|(i, t)| {
                        let froms = source_names(t.get("from"));
                        let to = t
                            .get("to")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        let event = t
                            .get("event")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        let guard = guard_of(t.get("guard"));
                        let effects: Vec<String> = t
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
                            .unwrap_or_default();
                        let index = t
                            .get("index")
                            .and_then(Value::as_u64)
                            .map_or(i, |n| n as usize);
                        froms.into_iter().map(move |from| TransitionInfo {
                            from,
                            to: to.clone(),
                            event: event.clone(),
                            guard: guard.clone(),
                            effects: effects.clone(),
                            index,
                        })
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
                    .flat_map(|(i, tr)| {
                        let froms = source_names(tr.get("from"));
                        let to = tr
                            .get("to")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        let event = tr
                            .get("event")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        let guard = guard_of(tr.get("guard"));
                        let effects: Vec<String> = tr
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
                            .unwrap_or_default();
                        froms.into_iter().map(move |from| TransitionInfo {
                            from,
                            to: to.clone(),
                            event: event.clone(),
                            guard: guard.clone(),
                            effects: effects.clone(),
                            index: i,
                        })
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

/// One state of a `MountedTrait`.
pub type MountedState = StateInfo;

#[derive(Debug, Clone)]
pub struct MountedTransition {
    pub index: usize,
    pub from: Vec<String>,
    pub to: String,
    pub event: String,
    pub guarded: bool,
    pub emits: Vec<String>,
}

/// `MountedTrait`: one trait of a mounted orbital (`program/self { mounted: true }`).
#[derive(Debug, Clone)]
pub struct MountedTrait {
    pub name: String,
    pub orbital: Option<String>,
    pub parent: Option<String>,
    pub depth: usize,
    pub embedded_in: Vec<String>,
    pub renders: bool,
    pub linked: bool,
    pub linked_entity: Option<String>,
    pub states: Vec<MountedState>,
    pub transitions: Vec<MountedTransition>,
    pub instances: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct MountedWire {
    pub from: String,
    pub to: String,
    pub event: String,
    pub triggers: Option<String>,
}

/// `MountedOrbital`: the `orbital` prop of `state-machine-view`.
#[derive(Debug, Clone, Default)]
pub struct Mounted {
    pub traits: Vec<MountedTrait>,
    pub wires: Vec<MountedWire>,
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

impl Mounted {
    /// `None` unless `v` is an orbital with at least one trait: the web falls back to the
    /// single-trait view for an absent, null or empty `orbital`.
    pub fn from_value(v: &Value) -> Option<Self> {
        let traits: Vec<MountedTrait> = v
            .get("traits")?
            .as_array()?
            .iter()
            .filter_map(|t| {
                Some(MountedTrait {
                    name: text(t, "trait")?,
                    orbital: text(t, "orbital"),
                    parent: text(t, "parent"),
                    depth: t.get("depth").and_then(Value::as_u64).unwrap_or(0) as usize,
                    embedded_in: strings(t.get("embeddedIn")),
                    renders: t.get("renders").and_then(Value::as_bool).unwrap_or(false),
                    linked: flag(t, "linked"),
                    linked_entity: text(t, "linkedEntity"),
                    states: t
                        .get("states")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .map(|s| StateInfo {
                                    name: text(s, "name").unwrap_or_default(),
                                    is_initial: flag(s, "isInitial"),
                                    is_terminal: flag(s, "isTerminal"),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    transitions: t
                        .get("transitions")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .enumerate()
                                .map(|(i, tr)| MountedTransition {
                                    index: tr
                                        .get("index")
                                        .and_then(Value::as_u64)
                                        .map_or(i, |n| n as usize),
                                    from: source_names(tr.get("from")),
                                    to: text(tr, "to").unwrap_or_default(),
                                    event: text(tr, "event").unwrap_or_default(),
                                    guarded: flag(tr, "guarded"),
                                    emits: strings(tr.get("emits")),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    instances: t
                        .get("instances")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(|i| text(i, "state")).collect())
                        .unwrap_or_default(),
                })
            })
            .collect();
        if traits.is_empty() {
            return None;
        }
        let wires = v
            .get("wires")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|w| {
                        Some(MountedWire {
                            from: text(w, "from")?,
                            to: text(w, "to")?,
                            event: text(w, "event")?,
                            triggers: text(w, "triggers"),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Self { traits, wires })
    }
}

/// The orbital-mode props of `state-machine-view` besides `orbital`.
#[derive(Debug, Clone)]
pub struct OrbitalOpts {
    pub active: Vec<(String, String)>,
    /// `route`, or `[firedTransition]` when no route is given: the transitions drawn as fired.
    pub steps: Vec<(String, usize)>,
    pub expand_depth: f64,
    pub selected: Option<String>,
    pub select_event: Option<String>,
    pub rtl: bool,
}

fn trait_steps(v: Option<&Value>) -> Vec<(String, usize)> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|s| {
                    Some((
                        text(s, "trait")?,
                        s.get("index").and_then(Value::as_u64)? as usize,
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

impl OrbitalOpts {
    pub fn from_props(props: &Value) -> Self {
        let route = trait_steps(props.get("route"));
        let fired = props
            .get("firedTransition")
            .filter(|f| f.is_object())
            .map(|f| trait_steps(Some(&Value::Array(vec![f.clone()]))))
            .unwrap_or_default();
        Self {
            active: props
                .get("activeStates")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|s| Some((text(s, "trait")?, text(s, "state")?)))
                        .collect()
                })
                .unwrap_or_default(),
            steps: if route.is_empty() { fired } else { route },
            expand_depth: props
                .get("expandDepth")
                .and_then(Value::as_f64)
                .unwrap_or(2.0),
            selected: text(props, "selectedTrait").filter(|t| !t.is_empty()),
            select_event: text(props, "selectEvent").filter(|t| !t.is_empty()),
            rtl: text(props, "direction").as_deref() == Some("rtl"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxKind {
    Machine,
    Collapsed,
    Chip,
}

#[derive(Debug, Clone)]
pub struct PlacedBox {
    pub trait_name: String,
    pub kind: BoxKind,
    pub lane: bool,
    /// The box this one sits inside; `None` for a root, a lane box and a linked-group member.
    pub parent: Option<String>,
    pub container: bool,
    pub rect: layered::Rect,
}

#[derive(Debug, Clone)]
pub struct PlacedState {
    pub trait_name: String,
    pub name: String,
    pub is_initial: bool,
    pub is_terminal: bool,
    pub rect: layered::Rect,
}

#[derive(Debug, Clone)]
pub struct PlacedEdge {
    pub trait_name: String,
    pub index: usize,
    pub from: String,
    pub to: String,
    pub event: String,
    pub guarded: bool,
    pub points: Vec<(f32, f32)>,
    pub label: layered::Rect,
}

/// Self-transitions of one state folded into a count chip on the state.
#[derive(Debug, Clone)]
pub struct PlacedLoop {
    pub trait_name: String,
    pub state: String,
    pub indexes: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct PlacedEmbed {
    pub trait_name: String,
    pub state: String,
    pub points: Vec<(f32, f32)>,
}

#[derive(Debug, Clone)]
pub struct PlacedWire {
    pub from: String,
    pub to: String,
    pub events: Vec<String>,
    pub points: Vec<(f32, f32)>,
    pub label: layered::Rect,
}

#[derive(Debug, Clone)]
pub struct PlacedGroup {
    pub orbital: String,
    pub rect: layered::Rect,
}

/// A laid-out whole orbital: its natural size and everything needed to paint it.
pub struct OrbitalDiagram<'a> {
    pub mounted: &'a Mounted,
    pub opts: &'a OrbitalOpts,
    pub boxes: Vec<PlacedBox>,
    pub states: Vec<PlacedState>,
    pub edges: Vec<PlacedEdge>,
    pub loops: Vec<PlacedLoop>,
    pub embeds: Vec<PlacedEmbed>,
    pub wires: Vec<PlacedWire>,
    pub groups: Vec<PlacedGroup>,
    pub lane_top: Option<f32>,
    pub group_top: Option<f32>,
    pub width: f32,
    pub height: f32,
}

const BOX_PAD: f32 = 16.0;
const BOX_HEAD_MACHINE: f32 = 46.0;
const BOX_HEAD_CONTAINER: f32 = 38.0;
const COLLAPSED_H: f32 = 56.0;
const CHIP_H: f32 = 28.0;
const HEAD_GLYPH: f32 = 40.0;
const HEAD_TOGGLE: f32 = 28.0;
const LOOP_FOLD: usize = 3;
const LOOP_LINE: f32 = 18.0;
const LANE_HEAD: f32 = 30.0;
const LANE_MARGIN: f32 = 36.0;
const LANE_MIN_WIDTH: f32 = 480.0;
const GLYPH_ROOM: f32 = 20.0;
const ORBITAL_ACTIVE_STROKE: f32 = 2.5;
const ORBITAL_WIRE_STROKE: f32 = 1.5;
const BACKGROUND_CAPTION: &str = "Runs in the background";
const LINKED_CAPTION: &str = "Other orbitals this page talks to";

struct EdgeMeta {
    from: String,
    to: String,
    event: String,
    index: usize,
    guarded: bool,
}

struct Named {
    name: String,
    kind: BoxKind,
    edges: Vec<EdgeMeta>,
    kids: Vec<Named>,
}

struct Ctx<'a> {
    o: &'a OrbitalOpts,
    children_of: HashMap<&'a str, Vec<&'a MountedTrait>>,
}

impl Ctx<'_> {
    fn kind(&self, t: &MountedTrait) -> BoxKind {
        if t.states.len() <= 1 {
            BoxKind::Chip
        } else if (t.depth as f64) < self.o.expand_depth {
            BoxKind::Machine
        } else {
            BoxKind::Collapsed
        }
    }

    fn compact(&self, t: &MountedTrait, kind: BoxKind) -> (f32, f32) {
        if kind == BoxKind::Chip {
            return (
                (t.name.chars().count() as f32 * BODY_CHAR + 28.0 + HEAD_GLYPH).round(),
                CHIP_H,
            );
        }
        let sub = t.linked_entity.as_deref().unwrap_or("").chars().count();
        let longest = t.name.chars().count().max(sub) as f32;
        (
            132.0f32.max((longest * BODY_CHAR + 64.0 + HEAD_GLYPH + HEAD_TOGGLE).round()),
            COLLAPSED_H,
        )
    }

    fn build(&self, t: &MountedTrait, loops: &mut Vec<PlacedLoop>) -> (layered::BoxIn, Named) {
        let kind = self.kind(t);
        let kids: Vec<&MountedTrait> = self
            .children_of
            .get(t.name.as_str())
            .cloned()
            .unwrap_or_default();
        if kind != BoxKind::Machine && kids.is_empty() {
            let (w, h) = self.compact(t, kind);
            return (
                layered::BoxIn {
                    leaf: (w, h),
                    head: 0.0,
                    pad: 0.0,
                    min: (0.0, 0.0),
                    machine: None,
                    children: Vec::new(),
                },
                Named {
                    name: t.name.clone(),
                    kind,
                    edges: Vec::new(),
                    kids: Vec::new(),
                },
            );
        }
        let head = if kind == BoxKind::Machine {
            BOX_HEAD_MACHINE
        } else {
            BOX_HEAD_CONTAINER
        };
        let sub = t.linked_entity.as_deref().unwrap_or("").chars().count();
        let title = (t.name.chars().count().max(sub) as f32 * BODY_CHAR
            + 32.0
            + HEAD_GLYPH
            + if kind == BoxKind::Chip {
                0.0
            } else {
                HEAD_TOGGLE
            })
        .round();
        let (kid_boxes, kid_names): (Vec<_>, Vec<_>) =
            kids.iter().map(|k| self.build(k, loops)).unzip();
        let mut out = layered::BoxIn {
            leaf: (0.0, 0.0),
            head,
            pad: BOX_PAD,
            min: (title, head + 20.0),
            machine: None,
            children: kid_boxes,
        };
        let mut named = Named {
            name: t.name.clone(),
            kind,
            edges: Vec::new(),
            kids: kid_names,
        };
        if kind != BoxKind::Machine {
            return (out, named);
        }
        let mut self_count: HashMap<&str, usize> = HashMap::new();
        for tr in &t.transitions {
            for from in tr.from.iter().filter(|f| **f == tr.to) {
                *self_count.entry(from.as_str()).or_default() += 1;
            }
        }
        let folded = |name: &str| self_count.get(name).copied().unwrap_or(0) >= LOOP_FOLD;
        for st in t.states.iter().filter(|s| folded(&s.name)) {
            loops.push(PlacedLoop {
                trait_name: t.name.clone(),
                state: st.name.clone(),
                indexes: t
                    .transitions
                    .iter()
                    .filter(|tr| tr.to == st.name && tr.from.contains(&st.name))
                    .map(|tr| tr.index)
                    .collect(),
            });
        }
        let nodes: Vec<NodeIn> = t
            .states
            .iter()
            .map(|s| {
                let (w, h) = node_size(&s.name, NodeShape::Pill);
                NodeIn {
                    w,
                    h: h + if folded(&s.name) { LOOP_LINE } else { 0.0 },
                    initial: s.is_initial,
                }
            })
            .collect();
        let index = |name: &str| t.states.iter().position(|s| s.name == name);
        let mut edges = Vec::new();
        for tr in &t.transitions {
            for from in &tr.from {
                if *from == tr.to && folded(from) {
                    continue;
                }
                let (Some(f), Some(to)) = (index(from), index(&tr.to)) else {
                    continue;
                };
                let (label_w, label_h) = label_size(&TransitionInfo {
                    from: from.clone(),
                    to: tr.to.clone(),
                    event: tr.event.clone(),
                    guard: None,
                    effects: Vec::new(),
                    index: tr.index,
                });
                edges.push(EdgeIn {
                    from: f,
                    to,
                    label_w,
                    label_h,
                });
                named.edges.push(EdgeMeta {
                    from: from.clone(),
                    to: tr.to.clone(),
                    event: tr.event.clone(),
                    index: tr.index,
                    guarded: tr.guarded,
                });
            }
        }
        out.machine = Some(layered::MachineIn {
            nodes,
            edges,
            metrics: Metrics {
                node_spacing: 40.0,
                layer_spacing: 88.0,
                rtl: self.o.rtl,
            },
        });
        (out, named)
    }
}

struct Collected {
    boxes: Vec<PlacedBox>,
    states: Vec<PlacedState>,
    edges: Vec<PlacedEdge>,
}

fn collect(
    bo: &layered::BoxOut,
    named: &Named,
    m: &Mounted,
    parent: Option<&str>,
    lane: bool,
    out: &mut Collected,
) {
    out.boxes.push(PlacedBox {
        trait_name: named.name.clone(),
        kind: named.kind,
        lane,
        parent: parent.map(str::to_string),
        container: !named.kids.is_empty(),
        rect: bo.rect,
    });
    if let (Some(l), Some(t)) = (&bo.machine, m.traits.iter().find(|t| t.name == named.name)) {
        for (i, s) in t.states.iter().enumerate() {
            out.states.push(PlacedState {
                trait_name: t.name.clone(),
                name: s.name.clone(),
                is_initial: s.is_initial,
                is_terminal: s.is_terminal,
                rect: l.nodes[i],
            });
        }
        for (k, e) in l.edges.iter().enumerate() {
            let meta = &named.edges[k];
            out.edges.push(PlacedEdge {
                trait_name: t.name.clone(),
                index: meta.index,
                from: meta.from.clone(),
                to: meta.to.clone(),
                event: meta.event.clone(),
                guarded: meta.guarded,
                points: e.points.clone(),
                label: e.label,
            });
        }
    }
    for (child, kid) in bo.children.iter().zip(&named.kids) {
        collect(child, kid, m, Some(&named.name), lane, out);
    }
}

fn box_wire_path(a: layered::Rect, b: layered::Rect) -> Vec<(f32, f32)> {
    let (ax, ay) = (a.x + a.w / 2.0, a.y + a.h / 2.0);
    let (bx, by) = (b.x + b.w / 2.0, b.y + b.h / 2.0);
    if b.x >= a.x + a.w {
        vec![(a.x + a.w, ay), (b.x, by)]
    } else if a.x >= b.x + b.w {
        vec![(a.x, ay), (b.x + b.w, by)]
    } else if by >= ay {
        vec![(ax, a.y + a.h), (bx, b.y)]
    } else {
        vec![(ax, a.y), (bx, b.y + b.h)]
    }
}

fn wire_label_size(text: &str) -> (f32, f32) {
    (
        (text.encode_utf16().count() as f32 * MONO_CHAR + 16.0 + GLYPH_ROOM).round(),
        PILL_H - 4.0,
    )
}

/// `computeOrbitalLayout`: every rendering trait a box nested in its nearest rendering ancestor,
/// a background lane for the traits that never paint, the linked orbitals' traits grouped below.
pub fn prepare_orbital<'a>(m: &'a Mounted, o: &'a OrbitalOpts) -> OrbitalDiagram<'a> {
    let by_name: HashMap<&str, &MountedTrait> =
        m.traits.iter().map(|t| (t.name.as_str(), t)).collect();
    let tree: Vec<&MountedTrait> = m.traits.iter().filter(|t| t.renders && !t.linked).collect();
    let lane: Vec<&MountedTrait> = m
        .traits
        .iter()
        .filter(|t| !t.renders && !t.linked)
        .collect();
    let mut linked: Vec<(String, Vec<&MountedTrait>)> = Vec::new();
    for t in m.traits.iter().filter(|t| t.linked) {
        let key = t.orbital.clone().unwrap_or_default();
        match linked.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => v.push(t),
            None => linked.push((key, vec![t])),
        }
    }
    let effective_parent = |t: &MountedTrait| -> Option<&str> {
        let mut cur = t.parent.as_deref().and_then(|p| by_name.get(p));
        let mut hops = 0;
        while let Some(c) = cur {
            if c.renders || hops > m.traits.len() {
                return c.renders.then_some(c.name.as_str());
            }
            cur = c.parent.as_deref().and_then(|p| by_name.get(p));
            hops += 1;
        }
        None
    };
    let mut children_of: HashMap<&str, Vec<&MountedTrait>> = HashMap::new();
    let mut roots: Vec<&MountedTrait> = Vec::new();
    for t in &tree {
        match effective_parent(t) {
            Some(p) => children_of.entry(p).or_default().push(t),
            None => roots.push(t),
        }
    }
    let cx = Ctx { o, children_of };
    let mut loops = Vec::new();
    let (root_boxes, root_names): (Vec<_>, Vec<_>) =
        roots.iter().map(|t| cx.build(t, &mut loops)).unzip();
    let main = layered::compound(&root_boxes, None);
    let mut found = Collected {
        boxes: Vec::new(),
        states: Vec::new(),
        edges: Vec::new(),
    };
    for (bo, named) in main.boxes.iter().zip(&root_names) {
        collect(bo, named, m, None, false, &mut found);
    }
    let (mut width, mut height) = (main.width, main.height);

    let mut lane_top = None;
    if !lane.is_empty() {
        let top = height + LANE_MARGIN;
        lane_top = Some(top);
        let kinds: Vec<BoxKind> = lane
            .iter()
            .map(|t| {
                if cx.kind(t) == BoxKind::Chip {
                    BoxKind::Chip
                } else {
                    BoxKind::Collapsed
                }
            })
            .collect();
        let leaves: Vec<layered::BoxIn> = lane
            .iter()
            .zip(&kinds)
            .map(|(t, k)| layered::BoxIn {
                leaf: cx.compact(t, *k),
                head: 0.0,
                pad: 0.0,
                min: (0.0, 0.0),
                machine: None,
                children: Vec::new(),
            })
            .collect();
        let packed = layered::compound(&leaves, Some(width.max(LANE_MIN_WIDTH)));
        for ((bo, t), kind) in packed.boxes.iter().zip(&lane).zip(&kinds) {
            let mut rect = bo.rect;
            rect.y += top + LANE_HEAD;
            found.boxes.push(PlacedBox {
                trait_name: t.name.clone(),
                kind: *kind,
                lane: true,
                parent: None,
                container: false,
                rect,
            });
        }
        width = width.max(packed.width);
        height = top + LANE_HEAD + packed.height;
    }

    let mut groups = Vec::new();
    let mut group_top = None;
    if !linked.is_empty() {
        let top = height + LANE_MARGIN;
        group_top = Some(top);
        let mut group_boxes = Vec::new();
        let mut group_names = Vec::new();
        for (name, traits) in &linked {
            let (kids, names): (Vec<_>, Vec<_>) =
                traits.iter().map(|t| cx.build(t, &mut loops)).unzip();
            group_boxes.push(layered::BoxIn {
                leaf: (0.0, 0.0),
                head: BOX_HEAD_CONTAINER,
                pad: BOX_PAD,
                min: (
                    (name.chars().count() as f32 * BODY_CHAR + 32.0 + HEAD_GLYPH).round(),
                    BOX_HEAD_CONTAINER + 20.0,
                ),
                machine: None,
                children: kids,
            });
            group_names.push(names);
        }
        let packed = layered::compound(&group_boxes, Some(width.max(LANE_MIN_WIDTH)));
        for ((bo, (name, _)), names) in packed.boxes.iter().zip(&linked).zip(&group_names) {
            let mut moved = bo.clone();
            layered::shift(&mut moved, 0.0, top + LANE_HEAD);
            groups.push(PlacedGroup {
                orbital: name.clone(),
                rect: moved.rect,
            });
            for (child, named) in moved.children.iter().zip(names) {
                collect(child, named, m, None, false, &mut found);
            }
        }
        width = width.max(packed.width);
        height = top + LANE_HEAD + packed.height;
    }

    let mut embeds = Vec::new();
    for t in &tree {
        let Some(parent) = effective_parent(t) else {
            continue;
        };
        let Some(child) = found.boxes.iter().find(|b| b.trait_name == t.name) else {
            continue;
        };
        for state in &t.embedded_in {
            if let Some(node) = found
                .states
                .iter()
                .find(|s| s.trait_name == parent && s.name == *state)
            {
                embeds.push(PlacedEmbed {
                    trait_name: t.name.clone(),
                    state: state.clone(),
                    points: box_wire_path(node.rect, child.rect),
                });
            }
        }
    }

    let mut pairs: Vec<(String, String, Vec<String>)> = Vec::new();
    for w in &m.wires {
        if !by_name.contains_key(w.from.as_str()) || !by_name.contains_key(w.to.as_str()) {
            continue;
        }
        match pairs
            .iter_mut()
            .find(|(f, t, _)| *f == w.from && *t == w.to)
        {
            Some((_, _, events)) => {
                if !events.contains(&w.event) {
                    events.push(w.event.clone());
                }
            }
            None => pairs.push((w.from.clone(), w.to.clone(), vec![w.event.clone()])),
        }
    }
    let mut wires = Vec::new();
    for (from, to, events) in pairs {
        let rect = |n: &str| {
            found
                .boxes
                .iter()
                .find(|b| b.trait_name == n)
                .map(|b| b.rect)
        };
        let (Some(a), Some(b)) = (rect(&from), rect(&to)) else {
            continue;
        };
        let points = box_wire_path(a, b);
        let (lw, lh) = wire_label_size(&events.join(", "));
        let mid = (
            (points[0].0 + points[1].0) / 2.0,
            (points[0].1 + points[1].1) / 2.0,
        );
        wires.push(PlacedWire {
            from,
            to,
            events,
            points,
            label: layered::Rect {
                x: mid.0 - lw / 2.0,
                y: mid.1 - lh / 2.0,
                w: lw,
                h: lh,
            },
        });
    }

    OrbitalDiagram {
        mounted: m,
        opts: o,
        boxes: found.boxes,
        states: found.states,
        edges: found.edges,
        loops,
        embeds,
        wires,
        groups,
        lane_top,
        group_top,
        width: width + 2.0 * MARGIN,
        height: height + 2.0 * MARGIN,
    }
}

impl OrbitalDiagram<'_> {
    fn actives<'t>(&'t self, t: &'t MountedTrait) -> Vec<&'t str> {
        let given: Vec<&str> = self
            .opts
            .active
            .iter()
            .filter(|(name, _)| *name == t.name)
            .map(|(_, s)| s.as_str())
            .collect();
        if given.is_empty() {
            t.instances.iter().map(String::as_str).collect()
        } else {
            given
        }
    }

    fn step_of(&self, trait_name: &str, index: usize) -> Option<usize> {
        self.opts
            .steps
            .iter()
            .enumerate()
            .filter(|(_, (t, i))| t == trait_name && *i == index)
            .map(|(n, _)| n + 1)
            .last()
    }

    fn first_step(&self, trait_name: &str) -> Option<usize> {
        self.opts.steps.iter().position(|(t, _)| t == trait_name)
    }

    fn wire_on_route(&self, w: &PlacedWire) -> bool {
        matches!((self.first_step(&w.from), self.first_step(&w.to)), (Some(a), Some(b)) if a < b)
    }

    fn depth_of(&self, b: &PlacedBox) -> usize {
        let mut depth = 0;
        let mut cur = b.parent.as_deref();
        while let Some(p) = cur {
            depth += 1;
            cur = self
                .boxes
                .iter()
                .find(|x| x.trait_name == p)
                .and_then(|x| x.parent.as_deref());
            if depth > self.boxes.len() {
                break;
            }
        }
        depth
    }

    /// Paint at the origin of the pen's current transform.
    pub fn paint(&self, pen: &mut Pen) {
        let ink = pen.tone("ink");
        let quiet = pen.tone("label");
        let line = pen.tone("grid");
        let surface = pen.card();
        let focus = pen.tone("primary");
        let wash = pen.muted();
        let ground = pen.ground();
        let o = self.opts;
        let m = self.mounted;
        let (ox, oy) = (MARGIN, MARGIN);
        let at = |p: (f32, f32)| (p.0 + ox, p.1 + oy);
        let trait_of = |name: &str| m.traits.iter().find(|t| t.name == name);

        if let Some(top) = self.lane_top {
            pen.text_middle(
                ox,
                oy + top + LANE_HEAD / 2.0,
                BACKGROUND_CAPTION,
                12.0,
                quiet,
                TextAlign::Left,
                400,
            );
        }
        if let Some(top) = self.group_top {
            pen.text_middle(
                ox,
                oy + top + LANE_HEAD / 2.0,
                LINKED_CAPTION,
                12.0,
                quiet,
                TextAlign::Left,
                400,
            );
        }
        for g in &self.groups {
            let r = g.rect;
            pen.rrect(
                ox + r.x,
                oy + r.y,
                r.w,
                r.h,
                12.0,
                Some(ground),
                Some(&Ink::new(line, 1.0).dashed(&[4.0, 3.0])),
            );
            pen.text_middle(
                ox + r.x + 12.0,
                oy + r.y + BOX_HEAD_CONTAINER / 2.0,
                &g.orbital,
                13.0,
                quiet,
                TextAlign::Left,
                600,
            );
        }

        for b in &self.boxes {
            let Some(t) = trait_of(&b.trait_name) else {
                continue;
            };
            let r = b.rect;
            let (x, y) = (ox + r.x, oy + r.y);
            let active = self.actives(t);
            let fired = self.first_step(&b.trait_name).is_some();
            let chosen = o.selected.as_deref() == Some(b.trait_name.as_str());
            let expanded = b.kind == BoxKind::Machine;
            let opens = b.kind != BoxKind::Chip;
            let head_h = if b.container || expanded {
                if expanded {
                    BOX_HEAD_MACHINE
                } else {
                    BOX_HEAD_CONTAINER
                }
            } else {
                r.h
            };
            if fired {
                pen.rrect(
                    x - 2.0,
                    y - 2.0,
                    r.w + 4.0,
                    r.h + 4.0,
                    14.0,
                    None,
                    Some(&Ink::new(tint(focus, 28.0), 4.0)),
                );
            }
            let fill = if self.depth_of(b) % 2 == 0 {
                surface
            } else {
                wash
            };
            let border = if fired || chosen { focus } else { line };
            pen.rrect(
                x,
                y,
                r.w,
                r.h,
                12.0,
                Some(fill),
                Some(&Ink::new(border, if chosen { 2.0 } else { 1.0 })),
            );
            let cy = y + head_h / 2.0;
            let mut tx = x + 8.0;
            if opens {
                let c = (tx + 6.0, cy);
                let chevron: [(f32, f32); 3] = if expanded {
                    [
                        (c.0 - 4.0, c.1 - 2.0),
                        (c.0, c.1 + 2.0),
                        (c.0 + 4.0, c.1 - 2.0),
                    ]
                } else if o.rtl {
                    [
                        (c.0 + 2.0, c.1 - 4.0),
                        (c.0 - 2.0, c.1),
                        (c.0 + 2.0, c.1 + 4.0),
                    ]
                } else {
                    [
                        (c.0 - 2.0, c.1 - 4.0),
                        (c.0 + 2.0, c.1),
                        (c.0 - 2.0, c.1 + 4.0),
                    ]
                };
                pen.polyline(&chevron, false, None, Some(&Ink::new(quiet, 1.5)));
                tx += HEAD_TOGGLE - 4.0;
            }
            pen.circle(tx + 6.0, cy, 5.0, None, Some(&Ink::new(quiet, 1.2)));
            tx += 20.0;
            let entity = t
                .linked_entity
                .as_deref()
                .filter(|_| b.kind != BoxKind::Chip);
            let name_y = if entity.is_some() { cy - 7.0 } else { cy };
            pen.text_middle(tx, name_y, &t.name, 13.0, ink, TextAlign::Left, 600);
            if let Some(e) = entity {
                pen.text_middle(tx, cy + 8.0, e, 11.0, quiet, TextAlign::Left, 400);
            }
            if b.kind == BoxKind::Collapsed && !active.is_empty() {
                let label = active.join(", ");
                let w = text_width(&label, 11.0) + 12.0;
                let bx = tx + text_width(&t.name, 13.0) + 8.0;
                pen.rrect(bx, name_y - 9.0, w, 18.0, 9.0, Some(focus), None);
                pen.text_middle(
                    bx + w / 2.0,
                    name_y,
                    &label,
                    11.0,
                    surface,
                    TextAlign::Center,
                    600,
                );
            }
            if let Some(ev) = &o.select_event {
                pen.hit(
                    x,
                    y,
                    r.w,
                    head_h,
                    vec![Emit {
                        event: ev.clone(),
                        arg: String::new(),
                        payload: json!({ "trait": b.trait_name }),
                    }],
                );
            }
        }

        for e in &self.embeds {
            let pts: Vec<(f32, f32)> = e.points.iter().map(|p| at(*p)).collect();
            pen.polyline(
                &pts,
                false,
                None,
                Some(&Ink::new(with_alpha(quiet, 0.6), 1.0).dashed(&[1.0, 4.0])),
            );
        }
        for s in self.states.iter().filter(|s| s.is_initial) {
            let r = s.rect;
            let cy = r.y + r.h / 2.0;
            let (dot, a, b) = if o.rtl {
                (r.x + r.w + 18.0, r.x + r.w + 13.0, r.x + r.w + 1.0)
            } else {
                (r.x - 18.0, r.x - 13.0, r.x - 1.0)
            };
            let c = at((dot, cy));
            pen.circle(c.0, c.1, 4.0, Some(ink), None);
            let (pa, pb) = (at((a, cy)), at((b, cy)));
            pen.line(pa, pb, &Ink::new(quiet, 1.5));
            arrow_head(pen, pa, pb, 7.0, 1.5, with_alpha(quiet, 0.8));
        }

        let is_next = |trait_name: &str, from: &str| {
            trait_of(trait_name).is_some_and(|t| self.actives(t).contains(&from))
        };
        for e in &self.edges {
            if e.points.len() < 2 {
                continue;
            }
            let lit = self.step_of(&e.trait_name, e.index).is_some();
            let next = !lit && is_next(&e.trait_name, &e.from);
            let color = if lit || next { focus } else { quiet };
            let alpha = if lit {
                1.0
            } else if next {
                0.5
            } else {
                0.3
            };
            let width = if lit {
                ORBITAL_ACTIVE_STROKE
            } else {
                ORBITAL_WIRE_STROKE
            };
            let pts: Vec<(f32, f32)> = e.points.iter().map(|p| at(*p)).collect();
            pen.polyline(
                &rounded(&pts, 10.0),
                false,
                None,
                Some(&Ink::new(with_alpha(color, alpha), width)),
            );
            let head = if lit {
                focus
            } else if next {
                with_alpha(focus, 0.55)
            } else {
                with_alpha(quiet, 0.8)
            };
            arrow_head(
                pen,
                pts[pts.len() - 2],
                pts[pts.len() - 1],
                if lit { 6.0 } else { 7.0 },
                width,
                head,
            );
        }

        let shown = |w: &&PlacedWire| {
            self.wire_on_route(w)
                || o.selected.as_deref() == Some(w.from.as_str())
                || o.selected.as_deref() == Some(w.to.as_str())
        };
        for w in self.wires.iter().filter(shown) {
            let lit = self.wire_on_route(w);
            let color = if lit { focus } else { with_alpha(quiet, 0.45) };
            let width = if lit {
                ORBITAL_ACTIVE_STROKE
            } else {
                ORBITAL_WIRE_STROKE
            };
            let pts: Vec<(f32, f32)> = w.points.iter().map(|p| at(*p)).collect();
            pen.polyline(
                &pts,
                false,
                None,
                Some(&Ink::new(color, width).dashed(&[6.0, 4.0])),
            );
            arrow_head(
                pen,
                pts[0],
                pts[pts.len() - 1],
                if lit { 6.0 } else { 7.0 },
                width,
                if lit { focus } else { with_alpha(quiet, 0.8) },
            );
        }

        for e in &self.edges {
            if e.label.w <= 0.0 {
                continue;
            }
            let step = self.step_of(&e.trait_name, e.index);
            let emphasis = step.is_some();
            let next = !emphasis && is_next(&e.trait_name, &e.from);
            let l = e.label;
            let (x, y) = (ox + l.x, oy + l.y);
            let border = if emphasis || next { focus } else { line };
            pen.rrect(
                x,
                y,
                l.w,
                l.h,
                6.0,
                Some(surface),
                Some(&Ink::new(border, 1.0)),
            );
            let color = if emphasis {
                focus
            } else if next {
                ink
            } else {
                quiet
            };
            pen.mono_text_middle(
                x + l.w / 2.0,
                y + l.h / 2.0,
                &e.event,
                12.0,
                color,
                TextAlign::Center,
                if emphasis { 600 } else { 400 },
            );
            if let Some(n) = step {
                pen.circle(x, y, 9.0, Some(focus), None);
                pen.text_middle(x, y, &n.to_string(), 10.0, surface, TextAlign::Center, 700);
            }
            if next && e.guarded {
                let (gx, gy) = (x + l.w + 10.0, y - 10.0 + 10.0);
                pen.polyline(
                    &[
                        (gx, gy - 5.0),
                        (gx + 5.0, gy),
                        (gx, gy + 5.0),
                        (gx - 5.0, gy),
                    ],
                    true,
                    Some(surface),
                    Some(&Ink::new(quiet, 1.2)),
                );
            }
        }
        for w in self.wires.iter().filter(shown) {
            let l = w.label;
            let (x, y) = (ox + l.x, oy + l.y);
            pen.rrect(
                x,
                y,
                l.w,
                l.h,
                6.0,
                Some(ground),
                Some(&Ink::new(line, 1.0).dashed(&[3.0, 2.0])),
            );
            pen.mono_text_middle(
                x + 8.0 + GLYPH_ROOM - 4.0,
                y + l.h / 2.0,
                &w.events.join(", "),
                11.0,
                quiet,
                TextAlign::Left,
                400,
            );
        }

        for s in &self.states {
            let Some(t) = trait_of(&s.trait_name) else {
                continue;
            };
            let r = s.rect;
            let (x, y) = (ox + r.x, oy + r.y);
            let active = self.actives(t).contains(&s.name.as_str());
            let looped = self
                .loops
                .iter()
                .find(|l| l.trait_name == s.trait_name && l.state == s.name);
            let extra = if looped.is_some() { LOOP_LINE } else { 0.0 };
            if s.is_terminal {
                pen.rrect(
                    x - 4.0,
                    y - 4.0,
                    r.w + 8.0,
                    r.h + 8.0,
                    12.0,
                    None,
                    Some(&Ink::new(line, 1.0)),
                );
            }
            let fill = if active {
                mix(focus, 10.0, surface)
            } else {
                surface
            };
            let border = if active { focus } else { line };
            pen.rrect(
                x,
                y,
                r.w,
                r.h,
                8.0,
                Some(fill),
                Some(&Ink::new(border, if active { 2.0 } else { 1.0 })),
            );
            pen.text_middle(
                x + r.w / 2.0,
                y + (r.h - extra) / 2.0,
                &s.name,
                13.0,
                if active { ink } else { quiet },
                TextAlign::Center,
                if active { 600 } else { 500 },
            );
            if let Some(l) = looped {
                let fired = l
                    .indexes
                    .iter()
                    .any(|i| self.step_of(&s.trait_name, *i).is_some());
                let label = l.indexes.len().to_string();
                let w = text_width(&label, 11.0) + 16.0;
                let (cx, cy) = (x + r.w / 2.0, y + r.h - LOOP_LINE / 2.0 - 2.0);
                pen.rrect(
                    cx - w / 2.0,
                    cy - 8.0,
                    w,
                    16.0,
                    8.0,
                    Some(if fired { focus } else { wash }),
                    None,
                );
                pen.text_middle(
                    cx,
                    cy,
                    &label,
                    11.0,
                    if fired { surface } else { quiet },
                    TextAlign::Center,
                    600,
                );
            }
        }
    }
}
