//! `graph-canvas` — the Rust twin of `GraphCanvas.tsx`: a node-link graph laid
//! out by a port of its d3-force simulation, drawn with the learning `Shape`
//! painter in the web's own paint order under the web's auto-fit view.

use super::canvas::{fit, theme_for};
use super::quadtree::{Kind, Quadtree};
use super::shape::{self, Align, Dash, PaintCtx, Shape};
use super::theme::{DiagramTheme, Rgba};
use super::{gap_once, Xf};
use crate::mappers::game_canvas_2d::{get_string, CanvasSceneError};
use crate::mappers::scene::{Drag, Emit, Hit, MappedScene, SceneMemory, ViewCommand};
use crate::packet::{DrawCommand, GameRenderPacket, Halo, PathSegment, Rect, TextAlign, Vec2};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::f64::consts::PI;

const MAX_LABEL_CHARS: usize = 22;
const GROUP_GRAVITY: f64 = 0.3;
const UNGROUPED_GRAVITY: f64 = 0.02;
const DEFAULT_WIDTH: f64 = 800.0;
const LABEL_GAP: f64 = 12.0;
const LABEL_H: f64 = 16.0;

#[derive(Debug, Clone, PartialEq)]
pub enum Mark {
    Suggested,
    Proposed { tint: Option<String> },
}

#[derive(Debug, Clone)]
pub struct Node {
    pub id: String,
    pub label: Option<String>,
    pub group: Option<String>,
    pub color: Option<String>,
    pub size: f64,
    pub badge: Option<f64>,
    pub mark: Option<Mark>,
    pub given: Option<(f64, f64)>,
    pub raw: Value,
    pub x: f64,
    pub y: f64,
    pub vx: f64,
    pub vy: f64,
    pub label_w: f64,
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub source: String,
    pub target: String,
    pub label: Option<String>,
    pub weight: Option<f64>,
    pub color: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Similarity {
    pub source: String,
    pub target: String,
    pub weight: f64,
}

#[derive(Debug, Clone)]
pub struct LayoutOpts {
    pub width: f64,
    pub height: f64,
    pub layout: String,
    pub show_labels: bool,
    pub repulsion: f64,
    pub link_distance: f64,
    pub node_spacing: f64,
}

/// The auto-fit view the web applies after a force layout (`zoom`, `offset`).
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct View {
    pub zoom: f64,
    pub ox: f64,
    pub oy: f64,
}

fn non_empty(v: &Value, key: &str) -> Option<String> {
    get_string(v, key).filter(|s| !s.is_empty())
}

fn num(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(Value::as_f64)
}

fn prop_num(props: &Value, key: &str, default: f64) -> f64 {
    num(props, key).unwrap_or(default)
}

fn prop_bool(props: &Value, key: &str, default: bool) -> bool {
    props.get(key).and_then(Value::as_bool).unwrap_or(default)
}

pub fn parse_nodes(v: Option<&Value>) -> Vec<Node> {
    v.and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|n| {
                    let Some(id) = get_string(n, "id") else {
                        gap_once("graph-node-id", "a node without a string `id` was skipped");
                        return None;
                    };
                    let size = num(n, "size")
                        .filter(|s| *s != 0.0 && !s.is_nan())
                        .unwrap_or(8.0);
                    let mark =
                        n.get("mark")
                            .and_then(|m| match m.get("kind").and_then(Value::as_str) {
                                Some("suggested") => Some(Mark::Suggested),
                                Some("proposed") => Some(Mark::Proposed {
                                    tint: get_string(m, "tint"),
                                }),
                                _ => None,
                            });
                    let given = match (num(n, "x"), num(n, "y")) {
                        (Some(x), Some(y)) if x != 0.0 && y != 0.0 => Some((x, y)),
                        _ => None,
                    };
                    Some(Node {
                        id,
                        label: non_empty(n, "label"),
                        group: non_empty(n, "group"),
                        color: non_empty(n, "color"),
                        size,
                        badge: num(n, "badge").filter(|b| *b != 0.0),
                        mark,
                        given,
                        raw: n.clone(),
                        x: 0.0,
                        y: 0.0,
                        vx: 0.0,
                        vy: 0.0,
                        label_w: 0.0,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn parse_edges(v: Option<&Value>) -> Vec<Edge> {
    v.and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|e| {
                    Some(Edge {
                        source: get_string(e, "source")?,
                        target: get_string(e, "target")?,
                        label: non_empty(e, "label"),
                        weight: num(e, "weight"),
                        color: non_empty(e, "color"),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_similarity(v: Option<&Value>) -> Vec<Similarity> {
    v.and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|s| {
                    Some(Similarity {
                        source: get_string(s, "source")?,
                        target: get_string(s, "target")?,
                        weight: num(s, "weight")?,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `hashSeed`: the 32-bit string hash that seeds a node's initial position.
pub fn hash_seed(s: &str) -> u32 {
    let units: Vec<u16> = s.encode_utf16().collect();
    let mut h = 1779033703u32 ^ units.len() as u32;
    for u in units {
        h = (h ^ u as u32).wrapping_mul(3432918353);
        h = h.rotate_left(13);
    }
    h ^= h >> 16;
    h
}

/// `mulberry32`.
pub fn mulberry32(seed: u32) -> impl FnMut() -> f64 {
    let mut a = seed;
    move || {
        a = a.wrapping_add(0x6d2b79f5);
        let mut t = (a ^ (a >> 15)).wrapping_mul(1 | a);
        t = t.wrapping_add((t ^ (t >> 7)).wrapping_mul(61 | t)) ^ t;
        (t ^ (t >> 14)) as f64 / 4294967296.0
    }
}

/// `truncateLabel`.
pub fn truncate_label(label: &str) -> String {
    if label.chars().count() > MAX_LABEL_CHARS {
        let head: String = label.chars().take(MAX_LABEL_CHARS - 1).collect();
        format!("{head}\u{2026}")
    } else {
        label.to_string()
    }
}

/// `edgeStroke`: weight sets width between the thin and normal strokes, never opacity.
pub fn edge_stroke(weight: Option<f64>, link_opacity: f64, thin: f64, normal: f64) -> (f64, f64) {
    match weight {
        None => (link_opacity, thin),
        Some(w) => (link_opacity, thin + (normal - thin) * w.clamp(0.0, 1.0)),
    }
}

/// d3-force's seeded LCG (`jiggle` source).
struct Lcg(u64);

impl Lcg {
    fn jiggle(&mut self) -> f64 {
        self.0 = (1664525 * self.0 + 1013904223) % 4294967296;
        (self.0 as f64 / 4294967296.0 - 0.5) * 1e-6
    }
}

struct Link {
    s: usize,
    t: usize,
    dist: f64,
    bias: f64,
}

fn links(raw: &[(usize, usize, f64)], n: usize, dist: impl Fn(f64) -> f64) -> Vec<Link> {
    let mut count = vec![0.0f64; n];
    for (s, t, _) in raw {
        count[*s] += 1.0;
        count[*t] += 1.0;
    }
    raw.iter()
        .map(|(s, t, w)| Link {
            s: *s,
            t: *t,
            dist: dist(*w),
            bias: count[*s] / (count[*s] + count[*t]),
        })
        .collect()
}

/// `GraphCanvas`'s d3 simulation run synchronously: the edge and similarity links, the charge,
/// collision and per-node x/y gravity, in that order, for 300 ticks at d3's default decay.
fn settle(
    nodes: &mut [Node],
    edge_links: &[Link],
    sim_links: &[Link],
    charge: f64,
    radii: &[f64],
    goals: &[((f64, f64), f64)],
) {
    let mut rng = Lcg(1);
    let alpha_decay = 1.0 - 0.001f64.powf(1.0 / 300.0);
    let mut alpha = 1.0f64;
    for _ in 0..300 {
        alpha += (0.0 - alpha) * alpha_decay;
        link_force(nodes, edge_links, 0.9, alpha, &mut rng);
        link_force(nodes, sim_links, 0.03, alpha, &mut rng);
        many_body(nodes, charge, alpha, &mut rng);
        collide(nodes, radii, &mut rng);
        for (n, ((tx, ty), g)) in nodes.iter_mut().zip(goals) {
            n.vx += (tx - n.x) * g * alpha;
            n.vy += (ty - n.y) * g * alpha;
        }
        for n in nodes.iter_mut() {
            n.vx *= 0.6;
            n.x += n.vx;
            n.vy *= 0.6;
            n.y += n.vy;
        }
    }
}

fn link_force(nodes: &mut [Node], links: &[Link], strength: f64, alpha: f64, rng: &mut Lcg) {
    for l in links {
        let (s, t) = (&nodes[l.s], &nodes[l.t]);
        let mut x = t.x + t.vx - s.x - s.vx;
        if x == 0.0 || x.is_nan() {
            x = rng.jiggle();
        }
        let mut y = t.y + t.vy - s.y - s.vy;
        if y == 0.0 || y.is_nan() {
            y = rng.jiggle();
        }
        let len = (x * x + y * y).sqrt();
        let k = (len - l.dist) / len * alpha * strength;
        let (x, y) = (x * k, y * k);
        nodes[l.t].vx -= x * l.bias;
        nodes[l.t].vy -= y * l.bias;
        nodes[l.s].vx += x * (1.0 - l.bias);
        nodes[l.s].vy += y * (1.0 - l.bias);
    }
}

/// d3 `forceManyBody` at its defaults (theta 0.9, distanceMin 1, no distanceMax): Barnes-Hut over
/// the d3 quadtree, each far quad acting as one body at its strength-weighted centroid.
fn many_body(nodes: &mut [Node], strength: f64, alpha: f64, rng: &mut Lcg) {
    const THETA2: f64 = 0.81;
    const DISTANCE_MIN2: f64 = 1.0;
    let points: Vec<(f64, f64)> = nodes.iter().map(|n| (n.x, n.y)).collect();
    let mut tree = Quadtree::build(&points);
    tree.visit_after(|t, q| {
        let (mut value, mut weight, mut x, mut y) = (0.0, 0.0, 0.0, 0.0);
        match t.quads[q].kind {
            Kind::Internal(children) => {
                for c in children.into_iter().flatten() {
                    let v = t.quads[c].value;
                    if v != 0.0 {
                        value += v;
                        weight += v.abs();
                        x += v.abs() * t.quads[c].x;
                        y += v.abs() * t.quads[c].y;
                    }
                }
                x /= weight;
                y /= weight;
            }
            Kind::Leaf { data, .. } => {
                (x, y) = points[data];
                let mut link = Some(q);
                while let Some(l) = link {
                    value += strength;
                    link = match t.quads[l].kind {
                        Kind::Leaf { next, .. } => next,
                        Kind::Internal(_) => None,
                    };
                }
            }
        }
        (t.quads[q].value, t.quads[q].x, t.quads[q].y) = (value, x, y);
    });
    for (i, node) in nodes.iter_mut().enumerate() {
        let (nx, ny) = (node.x, node.y);
        tree.visit(|q, x1, _, x2, _| {
            let quad = &tree.quads[q];
            if quad.value == 0.0 {
                return true;
            }
            let (mut x, mut y) = (quad.x - nx, quad.y - ny);
            let w = x2 - x1;
            let mut l = x * x + y * y;
            if w * w / THETA2 < l {
                if x == 0.0 {
                    x = rng.jiggle();
                    l += x * x;
                }
                if y == 0.0 {
                    y = rng.jiggle();
                    l += y * y;
                }
                if l < DISTANCE_MIN2 {
                    l = (DISTANCE_MIN2 * l).sqrt();
                }
                node.vx += x * quad.value * alpha / l;
                node.vy += y * quad.value * alpha / l;
                return true;
            }
            let Kind::Leaf { data, next } = quad.kind else {
                return false;
            };
            if data != i || next.is_some() {
                if x == 0.0 {
                    x = rng.jiggle();
                    l += x * x;
                }
                if y == 0.0 {
                    y = rng.jiggle();
                    l += y * y;
                }
                if l < DISTANCE_MIN2 {
                    l = (DISTANCE_MIN2 * l).sqrt();
                }
            }
            let mut link = Some(q);
            while let Some(c) = link {
                let Kind::Leaf { data, next } = tree.quads[c].kind else {
                    break;
                };
                if data != i {
                    let w = strength * alpha / l;
                    node.vx += x * w;
                    node.vy += y * w;
                }
                link = next;
            }
            false
        });
    }
}

/// d3 `forceCollide` (strength 0.9, one iteration): each node against the quads its predicted
/// circle reaches, over a quadtree of predicted positions (`x + vx`), pairs taken once (`j > i`).
fn collide(nodes: &mut [Node], radii: &[f64], rng: &mut Lcg) {
    const STRENGTH: f64 = 0.9;
    let points: Vec<(f64, f64)> = nodes.iter().map(|n| (n.x + n.vx, n.y + n.vy)).collect();
    let mut tree = Quadtree::build(&points);
    tree.visit_after(|t, q| {
        let r = match t.quads[q].kind {
            Kind::Leaf { data, .. } => radii[data],
            Kind::Internal(children) => children
                .into_iter()
                .flatten()
                .map(|c| t.quads[c].value)
                .fold(0.0, f64::max),
        };
        t.quads[q].value = r;
    });
    for i in 0..nodes.len() {
        let ri = radii[i];
        let ri2 = ri * ri;
        let xi = nodes[i].x + nodes[i].vx;
        let yi = nodes[i].y + nodes[i].vy;
        tree.visit(|q, x0, y0, x1, y1| {
            let quad = &tree.quads[q];
            let rj = quad.value;
            let r = ri + rj;
            let Kind::Leaf { data: j, .. } = quad.kind else {
                return x0 > xi + r || x1 < xi - r || y0 > yi + r || y1 < yi - r;
            };
            if j > i {
                let mut x = xi - nodes[j].x - nodes[j].vx;
                let mut y = yi - nodes[j].y - nodes[j].vy;
                let mut l = x * x + y * y;
                if l < r * r {
                    if x == 0.0 {
                        x = rng.jiggle();
                        l += x * x;
                    }
                    if y == 0.0 {
                        y = rng.jiggle();
                        l += y * y;
                    }
                    let len = l.sqrt();
                    let l = (r - len) / len * STRENGTH;
                    let (x, y) = (x * l, y * l);
                    let rj2 = rj * rj;
                    let share = rj2 / (ri2 + rj2);
                    nodes[i].vx += x * share;
                    nodes[i].vy += y * share;
                    let rest = 1.0 - share;
                    nodes[j].vx -= x * rest;
                    nodes[j].vy -= y * rest;
                }
            }
            false
        });
    }
}

type Rect4 = [f64; 4];

fn sep(r1: Rect4, r2: Rect4) -> Option<(bool, f64, f64)> {
    let ox = r1[2].min(r2[2]) - r1[0].max(r2[0]);
    let oy = r1[3].min(r2[3]) - r1[1].max(r2[1]);
    if ox <= 0.0 || oy <= 0.0 {
        return None;
    }
    if ox <= oy {
        Some((
            true,
            ox,
            if r1[0] + r1[2] < r2[0] + r2[2] {
                1.0
            } else {
                -1.0
            },
        ))
    } else {
        Some((
            false,
            oy,
            if r1[1] + r1[3] < r2[1] + r2[3] {
                1.0
            } else {
                -1.0
            },
        ))
    }
}

fn rects_of(n: &Node, pad: f64, show_labels: bool) -> (Rect4, Rect4) {
    let r = n.size;
    let lw = if show_labels { n.label_w } else { 0.0 };
    (
        [n.x - r - pad, n.y - r - pad, n.x + r + pad, n.y + r + pad],
        [
            n.x - lw / 2.0 - pad,
            n.y + r + LABEL_GAP - 8.0,
            n.x + lw / 2.0 + pad,
            n.y + r + LABEL_GAP + LABEL_H,
        ],
    )
}

/// The web layout effect: seeded/circular/grid placement, then for `force` the
/// 300-tick d3-force settle, the label-rectangle cleanup and the auto-fit view.
/// Node `label_w` must already be set. Circular/grid keep the identity view.
pub fn layout_nodes(
    nodes: &mut [Node],
    edges: &[Edge],
    similarity: &[Similarity],
    o: &LayoutOpts,
) -> View {
    layout_nodes_in(nodes, edges, similarity, o, &mut SceneMemory::default())
}

/// [`layout_nodes`] against the renderer's memory, as `GraphCanvas` lays out against its own
/// state: a node keeps its settled position, and the force simulation and auto-fit run only on
/// the first layout or when fewer than half the nodes are the ones laid out before (the web's
/// `fullRelayout`). The view stays the remembered one otherwise.
fn layout_nodes_in(
    nodes: &mut [Node],
    edges: &[Edge],
    similarity: &[Similarity],
    o: &LayoutOpts,
    mem: &mut SceneMemory,
) -> View {
    let view = place_nodes(nodes, edges, similarity, o, mem);
    mem.positions = nodes.iter().map(|n| (n.id.clone(), (n.x, n.y))).collect();
    mem.laid_out = true;
    view
}

fn place_nodes(
    nodes: &mut [Node],
    edges: &[Edge],
    similarity: &[Similarity],
    o: &LayoutOpts,
    mem: &mut SceneMemory,
) -> View {
    let prev = std::mem::take(&mut mem.positions);
    let kept = nodes.iter().filter(|n| prev.contains_key(&n.id)).count();
    let overlap = if prev.is_empty() {
        0.0
    } else {
        kept as f64 / prev.len().max(nodes.len()) as f64
    };
    let full_relayout = !mem.laid_out || overlap < 0.5;
    let remembered = mem.view.map_or(
        View {
            zoom: 1.0,
            ox: 0.0,
            oy: 0.0,
        },
        |(zoom, ox, oy)| View { zoom, ox, oy },
    );
    let count = nodes.len();
    let scale = (count as f64 / 12.0).sqrt().min(3.0).max(1.0);
    let (w, h) = (o.width * scale, o.height * scale);
    for (idx, n) in nodes.iter_mut().enumerate() {
        let (x, y) = match prev.get(&n.id).copied().or(n.given) {
            Some(p) => p,
            None => match o.layout.as_str() {
                "circular" => {
                    let angle = idx as f64 / count as f64 * 2.0 * PI;
                    let radius = w.min(h) * 0.35;
                    (
                        w / 2.0 + radius * angle.cos(),
                        h / 2.0 + radius * angle.sin(),
                    )
                }
                "grid" => {
                    let cols = (count as f64).sqrt().ceil();
                    let gap_x = w / (cols + 1.0);
                    let gap_y = h / ((count as f64 / cols).ceil() + 1.0);
                    (
                        gap_x * ((idx as f64 % cols) + 1.0),
                        gap_y * ((idx as f64 / cols).floor() + 1.0),
                    )
                }
                _ => {
                    let mut rand = mulberry32(hash_seed(&n.id));
                    let x = w * 0.2 + rand() * w * 0.6;
                    let y = h * 0.2 + rand() * h * 0.6;
                    (x, y)
                }
            },
        };
        n.x = x;
        n.y = y;
        n.vx = 0.0;
        n.vy = 0.0;
    }
    if o.layout != "force" || !full_relayout {
        return remembered;
    }

    let key = |a: &str, b: &str| {
        if a < b {
            (a.to_string(), b.to_string())
        } else {
            (b.to_string(), a.to_string())
        }
    };
    let drawn: HashSet<(String, String)> =
        edges.iter().map(|e| key(&e.source, &e.target)).collect();
    let mut by_source: Vec<(String, Vec<&Similarity>)> = Vec::new();
    for p in similarity {
        if p.weight < 0.25 || drawn.contains(&key(&p.source, &p.target)) {
            continue;
        }
        match by_source.iter_mut().find(|(s, _)| *s == p.source) {
            Some((_, v)) => v.push(p),
            None => by_source.push((p.source.clone(), vec![p])),
        }
    }
    let mut neighbors: HashMap<String, HashSet<String>> = HashMap::new();
    for (id, mut arr) in by_source {
        arr.sort_by(|a, b| {
            b.weight
                .partial_cmp(&a.weight)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        neighbors.insert(id, arr.iter().take(5).map(|p| p.target.clone()).collect());
    }
    let active: Vec<&Similarity> = similarity
        .iter()
        .filter(|p| {
            if drawn.contains(&key(&p.source, &p.target)) {
                return false;
            }
            neighbors
                .get(&p.source)
                .is_some_and(|s| s.contains(&p.target))
                || neighbors
                    .get(&p.target)
                    .is_some_and(|s| s.contains(&p.source))
        })
        .collect();

    let mut groups: Vec<String> = Vec::new();
    for n in nodes.iter() {
        if let Some(g) = &n.group {
            if !groups.contains(g) {
                groups.push(g.clone());
            }
        }
    }
    let multi = groups.len() > 1;
    let mut target: HashMap<&str, (f64, f64)> = HashMap::new();
    if multi {
        let ring = w.min(h) * 0.34;
        for (i, g) in groups.iter().enumerate() {
            let a = i as f64 / groups.len() as f64 * 2.0 * PI;
            target.insert(
                g.as_str(),
                (w / 2.0 + ring * a.cos(), h / 2.0 + ring * a.sin()),
            );
        }
    }

    let mut index: HashMap<&str, usize> = HashMap::new();
    for (i, n) in nodes.iter().enumerate() {
        index.insert(n.id.as_str(), i);
    }
    let resolve = |a: &str, b: &str| Some((*index.get(a)?, *index.get(b)?));
    let edge_raw: Vec<(usize, usize, f64)> = edges
        .iter()
        .filter_map(|e| {
            resolve(&e.source, &e.target)
                .map(|(s, t)| (s, t, e.weight.unwrap_or(1.0).clamp(0.0, 1.0)))
        })
        .collect();
    let sim_raw: Vec<(usize, usize, f64)> = active
        .iter()
        .filter_map(|p| {
            resolve(&p.source, &p.target).map(|(s, t)| (s, t, p.weight.clamp(0.0, 1.0)))
        })
        .collect();
    let ld = o.link_distance;
    let edge_links = links(&edge_raw, count, |wt| ld * (0.35 + (1.0 - wt) * 0.3));
    let sim_links = links(&sim_raw, count, |wt| ld * (1.35 - 0.55 * wt));
    let radii: Vec<f64> = nodes
        .iter()
        .map(|n| n.size.max(n.label_w / 2.0) + o.node_spacing / 2.0)
        .collect();
    let goals: Vec<((f64, f64), f64)> = nodes
        .iter()
        .map(|n| {
            let t = n
                .group
                .as_deref()
                .and_then(|g| target.get(g))
                .copied()
                .unwrap_or((w / 2.0, h / 2.0));
            (
                t,
                if multi && n.group.is_some() {
                    GROUP_GRAVITY
                } else {
                    UNGROUPED_GRAVITY
                },
            )
        })
        .collect();

    settle(
        nodes,
        &edge_links,
        &sim_links,
        -o.repulsion * 0.5,
        &radii,
        &goals,
    );

    let pad = o.node_spacing / 2.0;
    for _ in 0..24 {
        let mut moved = false;
        for i in 0..count {
            for j in (i + 1)..count {
                let (ca, la) = rects_of(&nodes[i], pad, o.show_labels);
                let (cb, lb) = rects_of(&nodes[j], pad, o.show_labels);
                let mut best: Option<(bool, f64, f64)> = None;
                for (r1, r2) in [(ca, cb), (ca, lb), (la, cb), (la, lb)] {
                    if let Some(s) = sep(r1, r2) {
                        if best.is_none_or(|b| s.1 < b.1) {
                            best = Some(s);
                        }
                    }
                }
                if let Some((horizontal, depth, sign)) = best {
                    moved = true;
                    let push = depth / 2.0;
                    if horizontal {
                        nodes[i].x = (nodes[i].x - push * sign).min(w - 30.0).max(30.0);
                        nodes[j].x = (nodes[j].x + push * sign).min(w - 30.0).max(30.0);
                    } else {
                        nodes[i].y = (nodes[i].y - push * sign).min(h - 30.0).max(30.0);
                        nodes[j].y = (nodes[j].y + push * sign).min(h - 30.0).max(30.0);
                    }
                }
            }
        }
        if !moved {
            break;
        }
    }

    let fit_pad = 40.0;
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    for n in nodes.iter() {
        let lw = if o.show_labels { n.label_w } else { 0.0 };
        min_x = min_x.min(n.x - n.size).min(n.x - lw / 2.0);
        min_y = min_y.min(n.y - n.size);
        max_x = max_x.max(n.x + n.size).max(n.x + lw / 2.0);
        max_y = max_y.max(n.y + n.size + LABEL_GAP + LABEL_H);
    }
    let bbox_w = (max_x - min_x + fit_pad * 2.0).max(1.0);
    let bbox_h = (max_y - min_y + fit_pad * 2.0).max(1.0);
    let zoom = 1.0f64.min(o.width / bbox_w).min(o.height / bbox_h);
    let fitted = View {
        zoom,
        ox: fit_pad - min_x * zoom,
        oy: fit_pad - min_y * zoom,
    };
    mem.view = Some((fitted.zoom, fitted.ox, fitted.oy));
    fitted
}

pub struct Graph {
    pub width: f64,
    pub height: f64,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub proposed: Vec<Edge>,
    pub groups: Vec<String>,
    pub show_labels: bool,
    pub selected: Option<String>,
    pub link_opacity: f64,
    pub view: View,
}

/// One drawn element in web paint order. `Dot` is the fill-only suggestion
/// indicator, which no stroked `Shape` can express.
#[derive(Debug, Clone)]
pub enum Item {
    Shape(Shape),
    Dot {
        x: f32,
        y: f32,
        r: f32,
        color: Rgba,
    },
    /// Centered text with an explicit weight (selected-node label 700, badge count 600) and an optional halo.
    Text {
        x: f32,
        y: f32,
        text: String,
        size: &'static str,
        color: Rgba,
        weight: u16,
        halo: bool,
    },
}

pub fn build(props: &Value, viewport: Option<(f32, f32)>) -> Graph {
    build_in(props, viewport, &mut SceneMemory::default())
}

fn build_in(props: &Value, viewport: Option<(f32, f32)>, mem: &mut SceneMemory) -> Graph {
    let width = viewport.map_or(DEFAULT_WIDTH, |(w, _)| w as f64);
    let height = prop_num(props, "height", 400.0);
    let mut nodes = parse_nodes(props.get("nodes"));
    let edges = parse_edges(props.get("edges"));
    let show_labels = prop_bool(props, "showLabels", true);
    let opts = LayoutOpts {
        width,
        height,
        layout: get_string(props, "layout").unwrap_or_else(|| "force".into()),
        show_labels,
        repulsion: prop_num(props, "repulsion", 800.0),
        link_distance: prop_num(props, "linkDistance", 100.0),
        node_spacing: prop_num(props, "nodeSpacing", 28.0),
    };
    for n in nodes.iter_mut() {
        n.label_w = match (&n.label, show_labels) {
            (Some(l), true) => crate::vector::text_width(&truncate_label(l), 12.0) as f64 + 4.0,
            _ => 0.0,
        };
    }
    let view = layout_nodes_in(
        &mut nodes,
        &edges,
        &parse_similarity(props.get("similarity")),
        &opts,
        mem,
    );
    let mut groups: Vec<String> = Vec::new();
    for n in &nodes {
        if let Some(g) = &n.group {
            if !groups.contains(g) {
                groups.push(g.clone());
            }
        }
    }
    Graph {
        width,
        height,
        nodes,
        edges,
        proposed: parse_edges(props.get("proposedEdges")),
        groups,
        show_labels,
        selected: get_string(props, "selectedNodeId"),
        link_opacity: prop_num(props, "linkOpacity", 0.18),
        view,
    }
}

/// An exact color as a literal the shape painter parses back.
fn css(c: Rgba) -> String {
    format!(
        "rgba({},{},{},{})",
        c[0] * 255.0,
        c[1] * 255.0,
        c[2] * 255.0,
        c[3]
    )
}

fn node_color(g: &Graph, n: &Node) -> String {
    if let Some(c) = &n.color {
        return c.clone();
    }
    match &n.group {
        None if g.groups.is_empty() => "series-1".into(),
        None => "muted".into(),
        Some(group) => {
            let idx = g.groups.iter().position(|x| x == group).unwrap_or(0);
            DiagramTheme::series_tone(idx)
        }
    }
}

fn line_shape(
    a: &Node,
    b: &Node,
    color: &str,
    opacity: f64,
    width: f64,
    dash: Option<Dash>,
) -> Shape {
    let mut s = Shape::line(a.x as f32, a.y as f32, b.x as f32, b.y as f32)
        .color(color)
        .opacity(opacity as f32)
        .dash(dash);
    s.line_width = Some(width as f32);
    s
}

fn ring(x: f64, y: f64, r: f64, color: &str, width: f32, opacity: f32) -> Shape {
    let mut s = Shape::circle(x as f32, y as f32, r as f32)
        .color(color)
        .opacity(opacity);
    s.line_width = Some(width);
    s
}

/// Everything the web draws on the canvas, in its paint order, in graph space.
pub fn derive_items(g: &Graph, theme: &DiagramTheme) -> Vec<Item> {
    let mut out = Vec::new();
    let [thin, normal, bold] = theme.strokes;
    let guide = css(theme.tone("guide").unwrap_or(theme.ground));
    let accent = css(theme.tone("highlight").unwrap_or(theme.ground));
    let ground = css(theme.ground);
    let ink = theme.tone("ink").unwrap_or(theme.ground);
    let ring_color = css([ink[0], ink[1], ink[2], 0.13]);
    let find = |id: &str| g.nodes.iter().find(|n| n.id == id);
    let edge_color = |e: &Edge| css(theme.mark_color(e.color.as_deref(), "guide"));

    for e in &g.edges {
        let (Some(a), Some(b)) = (find(&e.source), find(&e.target)) else {
            continue;
        };
        let (alpha, width) = edge_stroke(e.weight, g.link_opacity, thin as f64, normal as f64);
        let color = if e.color.is_some() {
            edge_color(e)
        } else {
            guide.clone()
        };
        out.push(Item::Shape(line_shape(a, b, &color, alpha, width, None)));
        if let (Some(label), true) = (&e.label, g.show_labels) {
            out.push(Item::Shape(
                Shape::text(
                    ((a.x + b.x) / 2.0) as f32,
                    ((a.y + b.y) / 2.0) as f32 - 4.0,
                    label.clone(),
                )
                .color(guide.clone())
                .text_size("xs")
                .align(Align::Center)
                .opacity(alpha as f32),
            ));
        }
    }
    for e in &g.proposed {
        let (Some(a), Some(b)) = (find(&e.source), find(&e.target)) else {
            continue;
        };
        let color = if e.color.is_some() {
            edge_color(e)
        } else {
            guide.clone()
        };
        out.push(Item::Shape(line_shape(
            a,
            b,
            &color,
            0.4,
            thin as f64,
            Some(Dash::Dashed),
        )));
    }

    for n in &g.nodes {
        let selected = g.selected.as_deref() == Some(n.id.as_str());
        let radius = if selected { n.size + 4.0 } else { n.size };
        let proposed = matches!(n.mark, Some(Mark::Proposed { .. }));
        let alpha = if proposed { 0.55 } else { 1.0 };
        let fill = if proposed {
            guide.clone()
        } else {
            css(theme.mark_color(Some(&node_color(g, n)), "series-1"))
        };
        let tint = match &n.mark {
            Some(Mark::Proposed { tint }) => {
                Some(css(theme.mark_color(tint.as_deref(), "warning")))
            }
            _ => None,
        };
        let (stroke, width, dash) = match (&tint, selected) {
            (Some(t), _) => (t.clone(), normal, Some(Dash::Dotted)),
            (None, true) => (accent.clone(), bold, None),
            (None, false) => (ring_color.clone(), thin, None),
        };
        let mut circle = Shape::circle(n.x as f32, n.y as f32, radius as f32)
            .fill(fill)
            .fill_style("solid")
            .color(stroke)
            .opacity(alpha)
            .dash(dash);
        circle.line_width = Some(width);
        out.push(Item::Shape(circle));

        match (&n.mark, &tint) {
            (Some(Mark::Suggested), _) => {
                out.push(Item::Shape(ring(
                    n.x,
                    n.y,
                    radius + 3.0,
                    &accent,
                    normal,
                    1.0,
                )));
                out.push(Item::Dot {
                    x: (n.x + radius * 0.7) as f32,
                    y: (n.y - radius * 0.7) as f32,
                    r: 2.5,
                    color: theme.tone("highlight").unwrap_or(theme.ground),
                });
            }
            (Some(Mark::Proposed { .. }), Some(t)) => {
                out.push(Item::Shape(ring(
                    n.x,
                    n.y,
                    radius + 6.0,
                    t,
                    normal,
                    0.55 * 0.4,
                )));
            }
            _ => {}
        }

        if let (Some(label), true) = (&n.label, g.show_labels) {
            let y = (n.y + radius + 14.0) as f32;
            let text = truncate_label(label);
            if selected {
                let text = if theme.label_uppercase {
                    text.to_uppercase()
                } else {
                    text
                };
                let mut color = ink;
                color[3] *= alpha;
                out.push(Item::Text {
                    x: n.x as f32,
                    y,
                    text,
                    size: "sm",
                    color,
                    weight: 700,
                    halo: true,
                });
            } else {
                out.push(Item::Shape(
                    Shape::text(n.x as f32, y, text)
                        .color(css(ink))
                        .text_size("sm")
                        .align(Align::Center)
                        .opacity(alpha),
                ));
            }
        }

        if let Some(badge) = n.badge.filter(|b| *b > 1.0) {
            let (bx, by, br) = badge_geometry(n, radius);
            let mut disc = Shape::circle(bx as f32, by as f32, br as f32)
                .fill(accent.clone())
                .fill_style("solid")
                .color(ground.clone())
                .opacity(alpha);
            disc.line_width = Some(normal);
            out.push(Item::Shape(disc));
            let mut color = theme.ground;
            color[3] *= alpha;
            out.push(Item::Text {
                x: bx as f32,
                y: (by + 0.5) as f32,
                text: badge.to_string(),
                size: "xs",
                color,
                weight: 600,
                halo: false,
            });
        }
    }
    out
}

fn badge_geometry(n: &Node, radius: f64) -> (f64, f64, f64) {
    (
        n.x + radius * 0.7,
        n.y - radius * 0.7,
        (radius * 0.45).min(11.0).max(7.0),
    )
}

fn node_payload(n: &Node) -> Value {
    let mut v = n.raw.clone();
    v["x"] = json!(n.x);
    v["y"] = json!(n.y);
    v["vx"] = json!(n.vx);
    v["vy"] = json!(n.vy);
    v
}

struct Handlers {
    node_click_event: Option<String>,
    on_node_click: Option<String>,
    on_mark_click: Option<String>,
    on_badge_click: Option<String>,
}

fn rect_around(xf: Xf, x: f64, y: f64, r: f64) -> (f32, f32, f32, f32) {
    let (px, py) = xf.p(x as f32, y as f32);
    let rr = r as f32 * xf.s;
    (px - rr, py - rr, px + rr, py + rr)
}

fn hits(g: &Graph, xf: Xf, h: &Handlers) -> Vec<Hit> {
    let mut out = Vec::new();
    for n in g.nodes.iter().rev() {
        let mut emits = Vec::new();
        if let (Some(ev), Some(Mark::Suggested | Mark::Proposed { .. })) =
            (&h.on_mark_click, &n.mark)
        {
            emits.push(Emit {
                event: ev.clone(),
                arg: "node".into(),
                payload: node_payload(n),
            });
        } else {
            if let Some(ev) = &h.node_click_event {
                emits.push(Emit {
                    event: ev.clone(),
                    arg: String::new(),
                    payload: json!({ "id": n.id, "row": node_payload(n) }),
                });
            }
            if let Some(ev) = &h.on_node_click {
                emits.push(Emit {
                    event: ev.clone(),
                    arg: "node".into(),
                    payload: node_payload(n),
                });
            }
        }
        let body = rect_around(xf, n.x, n.y, n.size + 4.0);
        if !emits.is_empty() {
            out.push(Hit {
                rect: Rect {
                    x: body.0,
                    y: body.1,
                    w: body.2 - body.0,
                    h: body.3 - body.1,
                },
                emits,
            });
        }
        if let (Some(ev), true) = (&h.on_badge_click, n.badge.is_some_and(|b| b > 1.0)) {
            let selected = g.selected.as_deref() == Some(n.id.as_str());
            let r = n.size + if selected { 4.0 } else { 0.0 };
            let (bx, by, br) = badge_geometry(n, r);
            let badge = rect_around(xf, bx, by, br + 3.0);
            let (x0, y0) = (badge.0.max(body.0), badge.1.max(body.1));
            let (x1, y1) = (badge.2.min(body.2), badge.3.min(body.3));
            if x1 > x0 && y1 > y0 {
                out.push(Hit {
                    rect: Rect {
                        x: x0,
                        y: y0,
                        w: x1 - x0,
                        h: y1 - y0,
                    },
                    emits: vec![Emit {
                        event: ev.clone(),
                        arg: "node".into(),
                        payload: node_payload(n),
                    }],
                });
            }
        }
    }
    out
}

/// The full-label tooltip the web shows on hover over a truncated label (`bg-primary
/// text-primary-foreground px-3 py-2 text-sm max-w-xs`, 10px above the node's center), here on a
/// long press. A label that fits draws nothing, as on the web.
fn label_tooltip(cmds: &mut Vec<DrawCommand>, n: &Node, xf: Xf, theme: &DiagramTheme) {
    let Some(label) = n.label.as_deref().filter(|l| truncate_label(l) != *l) else {
        return;
    };
    const PX: f32 = 14.0;
    const LINE: f32 = 20.0;
    let max_w = 320.0 - 24.0;
    let mut lines: Vec<String> = Vec::new();
    for word in label.split_whitespace() {
        match lines.last_mut() {
            Some(line) if crate::vector::text_width(&format!("{line} {word}"), PX) <= max_w => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    let text_w = lines
        .iter()
        .map(|l| crate::vector::text_width(l, PX))
        .fold(0.0, f32::max);
    let (w, h) = (text_w + 24.0, lines.len() as f32 * LINE + 16.0);
    let (cx, cy) = xf.p(n.x as f32, n.y as f32);
    let (x, y) = (cx - w / 2.0, cy - 10.0 - h);
    cmds.push(DrawCommand::Path {
        segments: shape::rounded_rect(x, y, w, h, 8.0, Xf::identity()),
        fill: theme.tone("primary"),
        stroke: None,
    });
    for (i, line) in lines.iter().enumerate() {
        cmds.push(DrawCommand::Text {
            text: line.clone(),
            pos: Vec2 {
                x: cx,
                y: y + 8.0 + LINE * (i as f32 + 0.5),
            },
            size: PX,
            color: theme.surface_on_primary,
            align: TextAlign::Center,
            weight: 400,
            halo: None,
            mono: false,
        });
    }
}

fn composed(viewfit: Xf, v: View) -> Xf {
    Xf {
        s: viewfit.s * v.zoom as f32,
        ox: viewfit.ox + viewfit.s * v.ox as f32,
        oy: viewfit.oy + viewfit.s * v.oy as f32,
    }
}

fn dot_command(x: f32, y: f32, r: f32, color: Rgba, xf: Xf) -> DrawCommand {
    let (cx, cy) = xf.p(x, y);
    let rr = r * xf.s;
    DrawCommand::Path {
        segments: vec![PathSegment::Arc {
            cx,
            cy,
            rx: rr,
            ry: rr,
            start: 0.0,
            end: std::f32::consts::TAU,
        }],
        fill: Some(color),
        stroke: None,
    }
}

pub fn map(props: &Value, viewport: Option<(f32, f32)>) -> Result<MappedScene, CanvasSceneError> {
    map_with(props, viewport, &mut SceneMemory::default())
}

/// `applyPanDelta` / `applyZoom`: the gestures queued on `mem`, in packet points, moved into the
/// view (zoom kept within 0.3..3, the pinch focus held still).
fn apply_gestures(view: &mut View, viewfit: Xf, mem: &mut SceneMemory) {
    let (dx, dy) = std::mem::take(&mut mem.pending_pan);
    view.ox += f64::from(dx / viewfit.s);
    view.oy += f64::from(dy / viewfit.s);
    if let Some((factor, fx, fy)) = mem.pending_zoom.take() {
        let old = view.zoom;
        let new = (old * f64::from(factor)).clamp(0.3, 3.0);
        if new != old {
            let cx = f64::from((fx - viewfit.ox) / viewfit.s);
            let cy = f64::from((fy - viewfit.oy) / viewfit.s);
            view.ox = cx - (cx - view.ox) * (new / old);
            view.oy = cy - (cy - view.oy) * (new / old);
            view.zoom = new;
        }
    }
    match mem.pending_view.take() {
        Some(ViewCommand::ZoomIn) => view.zoom = (view.zoom * 1.2).min(3.0),
        Some(ViewCommand::ZoomOut) => view.zoom = (view.zoom / 1.2).max(0.3),
        Some(ViewCommand::Reset) => (view.zoom, view.ox, view.oy) = (1.0, 0.0, 0.0),
        None => {}
    }
    mem.view = Some((view.zoom, view.ox, view.oy));
}

/// [`map`] with the renderer's [`SceneMemory`]: the layout and view persist, gestures apply.
pub fn map_with(
    props: &Value,
    viewport: Option<(f32, f32)>,
    mem: &mut SceneMemory,
) -> Result<MappedScene, CanvasSceneError> {
    let theme = theme_for(props);
    let mut g = build_in(props, viewport, mem);
    let (viewfit, pw, ph) = fit(g.width as f32, g.height as f32, viewport);
    let interactive = prop_bool(props, "interactive", true);
    if interactive {
        apply_gestures(&mut g.view, viewfit, mem);
    } else {
        (mem.pending_pan, mem.pending_zoom, mem.pending_view) = ((0.0, 0.0), None, None);
    }
    let mut cmds = vec![DrawCommand::Clear {
        color: theme.ground,
    }];

    let chrome = props.get("title").is_some()
        || props.get("actions").is_some()
        || prop_bool(props, "interactive", true);
    if chrome {
        gap_once(
            "graph-canvas-chrome",
            "title, zoom buttons and action badges are DOM chrome around the canvas; host layout",
        );
    }
    if g.groups.len() > 1 {
        gap_once(
            "graph-canvas-legend",
            "the group legend row is DOM below the canvas; host layout",
        );
    }
    if props.get("isLoading").and_then(Value::as_bool) == Some(true)
        || props.get("error").is_some_and(|e| !e.is_null())
        || g.nodes.is_empty()
    {
        gap_once(
            "graph-canvas-state",
            "loading/error/empty states are DOM; the canvas is left blank",
        );
        return Ok(MappedScene {
            packet: GameRenderPacket {
                pattern: "graph-canvas".into(),
                width: pw,
                height: ph,
                draw_commands: cmds,
                ..Default::default()
            },
            hits: Vec::new(),
            ..MappedScene::default()
        });
    }

    let xf = composed(viewfit, g.view);
    let ctx = PaintCtx {
        theme: &theme,
        xf,
        siblings: &[],
        width: g.width as f32,
        height: g.height as f32,
    };
    for item in derive_items(&g, &theme) {
        match item {
            Item::Shape(s) => shape::paint(&s, &ctx, &mut cmds),
            Item::Dot { x, y, r, color } => cmds.push(dot_command(x, y, r, color, xf)),
            Item::Text {
                x,
                y,
                text,
                size,
                color,
                weight,
                halo,
            } => {
                let (tx, ty) = xf.p(x, y);
                cmds.push(DrawCommand::Text {
                    text,
                    pos: Vec2 { x: tx, y: ty },
                    size: theme.text_px(size) * xf.s,
                    color,
                    align: TextAlign::Center,
                    weight,
                    mono: false,
                    halo: halo.then(|| Halo {
                        color: theme.ground,
                        width: (theme.text_px("xs") * 0.3).max(2.0) * xf.s,
                    }),
                });
            }
        }
    }

    if let Some((px, py)) = mem.pending_press.take() {
        let (gx, gy) = (
            f64::from((px - xf.ox) / xf.s),
            f64::from((py - xf.oy) / xf.s),
        );
        mem.pressed = g
            .nodes
            .iter()
            .rev()
            .find(|n| (n.x - gx).hypot(n.y - gy) <= n.size + 4.0)
            .map(|n| n.id.clone());
    }
    if let Some(n) = mem
        .pressed
        .as_ref()
        .and_then(|id| g.nodes.iter().find(|n| &n.id == id))
    {
        label_tooltip(&mut cmds, n, xf, &theme);
    }

    let handlers = Handlers {
        node_click_event: get_string(props, "nodeClickEvent"),
        on_node_click: get_string(props, "onNodeClick"),
        on_mark_click: get_string(props, "onMarkClick"),
        on_badge_click: get_string(props, "onBadgeClick"),
    };
    let draggable = prop_bool(props, "draggable", true);
    let tappable = interactive || draggable;
    let hit_list = if tappable {
        hits(&g, xf, &handlers)
    } else {
        Vec::new()
    };
    let node_rect = |n: &Node| {
        let (x0, y0, x1, y1) = rect_around(xf, n.x, n.y, n.size + 4.0);
        Rect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        }
    };
    let drags = if draggable {
        g.nodes
            .iter()
            .map(|n| Drag {
                rect: node_rect(n),
                scene_scale: xf.s,
                scene_origin: (xf.ox, xf.oy),
                max: (f32::MAX, f32::MAX),
                id: n.id.clone(),
                moves: Vec::new(),
                pin: true,
            })
            .collect()
    } else {
        Vec::new()
    };
    let double_hits = match get_string(props, "onNodeDoubleClick") {
        Some(event) if tappable => g
            .nodes
            .iter()
            .map(|n| Hit {
                rect: node_rect(n),
                emits: vec![Emit {
                    event: event.clone(),
                    arg: "node".into(),
                    payload: node_payload(n),
                }],
            })
            .collect(),
        _ => Vec::new(),
    };
    Ok(MappedScene {
        packet: GameRenderPacket {
            pattern: "graph-canvas".into(),
            width: pw,
            height: ph,
            draw_commands: cmds,
            ..Default::default()
        },
        hits: hit_list,
        drags,
        double_hits,
        pannable: interactive,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path_of(scene: &MappedScene, i: usize) -> &DrawCommand {
        &scene.packet.draw_commands[i]
    }

    #[test]
    fn the_layout_seed_matches_the_web_hash_and_prng() {
        // values from running GraphCanvas.tsx hashSeed/mulberry32 in node
        assert_eq!(hash_seed("alpha"), 3867295292);
        let mut r = mulberry32(hash_seed("alpha"));
        assert!((r() - 0.25601538829505444).abs() < 1e-12);
        assert!((r() - 0.930887047899887).abs() < 1e-12);
    }

    #[test]
    fn labels_truncate_at_22_chars_with_an_ellipsis_and_short_ones_pass() {
        assert_eq!(truncate_label("short"), "short");
        let t = truncate_label("abcdefghijklmnopqrstuvwxyz");
        assert_eq!(t.chars().count(), 22);
        assert!(t.ends_with('\u{2026}'));
        assert_eq!(truncate_label(&"x".repeat(22)).chars().count(), 22);
    }

    #[test]
    fn edge_weight_sets_width_between_thin_and_normal_never_opacity() {
        assert_eq!(edge_stroke(None, 0.18, 1.0, 1.5), (0.18, 1.0));
        assert_eq!(edge_stroke(Some(1.0), 0.18, 1.0, 1.5), (0.18, 1.5));
        assert_eq!(edge_stroke(Some(0.0), 0.18, 1.0, 1.5), (0.18, 1.0));
        assert_eq!(edge_stroke(Some(9.0), 0.18, 1.0, 1.5), (0.18, 1.5));
    }

    fn opts(layout: &str) -> LayoutOpts {
        LayoutOpts {
            width: 800.0,
            height: 400.0,
            layout: layout.into(),
            show_labels: true,
            repulsion: 800.0,
            link_distance: 100.0,
            node_spacing: 28.0,
        }
    }

    fn nodes_of(ids: &[&str]) -> Vec<Node> {
        let v = json!(ids
            .iter()
            .map(|i| json!({"id": i, "label": i}))
            .collect::<Vec<_>>());
        let mut n = parse_nodes(Some(&v));
        for x in n.iter_mut() {
            x.label_w = 40.0;
        }
        n
    }

    #[test]
    fn circular_and_grid_place_nodes_on_the_web_formulas_without_a_view() {
        let mut n = nodes_of(&["a", "b", "c", "d"]);
        let v = layout_nodes(&mut n, &[], &[], &opts("circular"));
        assert_eq!(
            v,
            View {
                zoom: 1.0,
                ox: 0.0,
                oy: 0.0
            }
        );
        assert!((n[0].x - (400.0 + 140.0)).abs() < 1e-9 && (n[0].y - 200.0).abs() < 1e-9);
        assert!((n[1].x - 400.0).abs() < 1e-9 && (n[1].y - (200.0 + 140.0)).abs() < 1e-9);
        let mut g = nodes_of(&["a", "b", "c", "d"]);
        layout_nodes(&mut g, &[], &[], &opts("grid"));
        assert_eq!((g[0].x, g[0].y), (800.0 / 3.0, 400.0 / 3.0));
        assert_eq!((g[3].x, g[3].y), (800.0 / 3.0 * 2.0, 400.0 / 3.0 * 2.0));
    }

    fn node_at(x: f64, y: f64) -> Node {
        let mut n = parse_nodes(Some(&json!([{"id": "n"}]))).remove(0);
        (n.x, n.y, n.vx, n.vy) = (x, y, 0.0, 0.0);
        n
    }

    fn force_cases() -> Vec<Value> {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/graph-force/cases.json"
        );
        serde_json::from_str(&std::fs::read_to_string(path).expect("graph-force fixture")).unwrap()
    }

    /// Max distance between the native and the d3 settled positions of one fixture case.
    fn force_drift(case: &Value) -> f64 {
        let f = |v: &Value| v.as_f64().unwrap();
        let mut nodes: Vec<Node> = case["start"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| node_at(f(&p["x"]), f(&p["y"])))
            .collect();
        let radii: Vec<f64> = case["start"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| f(&p["radius"]))
            .collect();
        let raw = |key: &str| -> Vec<(usize, usize, f64)> {
            case[key]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| (f(&e["s"]) as usize, f(&e["t"]) as usize, f(&e["weight"])))
                .collect()
        };
        let ld = f(&case["linkDistance"]);
        let count = nodes.len();
        let edge_links = links(&raw("edges"), count, |w| ld * (0.35 + (1.0 - w) * 0.3));
        let sim_links = links(&raw("sims"), count, |w| ld * (1.35 - 0.55 * w));
        let goals: Vec<((f64, f64), f64)> = case["goals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| ((f(&g["x"]), f(&g["y"])), f(&g["strength"])))
            .collect();
        settle(
            &mut nodes,
            &edge_links,
            &sim_links,
            -f(&case["repulsion"]) * 0.5,
            &radii,
            &goals,
        );
        nodes
            .iter()
            .zip(case["settled"].as_array().unwrap())
            .map(|(n, p)| ((n.x - f(&p["x"])).powi(2) + (n.y - f(&p["y"])).powi(2)).sqrt())
            .fold(0.0, f64::max)
    }

    #[test]
    fn the_force_layout_settles_where_d3_force_settles() {
        for case in force_cases() {
            let drift = force_drift(&case);
            assert!(
                drift < 1e-6,
                "{}: max drift {drift} px from d3",
                case["name"]
            );
        }
    }

    #[test]
    fn control_a_perturbed_start_drifts_from_the_d3_fixture() {
        let mut case = force_cases().remove(0);
        case["start"][3]["x"] = json!(case["start"][3]["x"].as_f64().unwrap() + 40.0);
        assert!(force_drift(&case) > 1.0);
    }

    #[test]
    fn force_layout_is_deterministic_and_separates_nodes() {
        let ids = ["a", "b", "c", "d", "e", "f"];
        let edges = parse_edges(Some(
            &json!([{"source":"a","target":"b"},{"source":"b","target":"c"},{"source":"d","target":"e"}]),
        ));
        let mut one = nodes_of(&ids);
        let mut two = nodes_of(&ids);
        let v1 = layout_nodes(&mut one, &edges, &[], &opts("force"));
        let v2 = layout_nodes(&mut two, &edges, &[], &opts("force"));
        assert_eq!(v1, v2);
        for (a, b) in one.iter().zip(&two) {
            assert_eq!((a.x, a.y), (b.x, b.y));
        }
        assert!(v1.zoom > 0.0 && v1.zoom <= 1.0);
        for i in 0..one.len() {
            for j in (i + 1)..one.len() {
                let d = ((one[i].x - one[j].x).powi(2) + (one[i].y - one[j].y).powi(2)).sqrt();
                assert!(d > 16.0, "{} and {} overlap at {d}", ids[i], ids[j]);
            }
        }
        let mut other = nodes_of(&ids);
        let v3 = layout_nodes(&mut other, &[], &[], &opts("force"));
        assert!(
            one.iter().zip(&other).any(|(a, b)| (a.x - b.x).abs() > 1.0),
            "control: edges change the layout, {v3:?}"
        );
    }

    #[test]
    fn the_settled_force_layout_matches_d3_force_run_on_the_same_graph() {
        // positions from the web's d3-force setup (300 ticks, labelW 40) run in node; the port is exact pairwise where d3 uses Barnes-Hut, so a few px apart
        let want = [
            ("limits", 458.68, 180.79),
            ("derivatives", 350.27, 216.11),
            ("integrals", 415.94, 233.69),
            ("series", 317.95, 133.45),
            ("a", 483.13, 244.23),
            ("b", 367.91, 281.78),
            ("c", 392.99, 163.16),
            ("d", 525.82, 191.36),
            ("e", 440.39, 297.14),
        ];
        let ids: Vec<&str> = want.iter().map(|w| w.0).collect();
        let groups = ["L1", "L2", "L2", "L3", "L1", "L2", "L3", "L1", "L2"];
        let v = json!(ids
            .iter()
            .zip(groups)
            .map(|(i, g)| json!({"id": i, "group": g, "label": i}))
            .collect::<Vec<_>>());
        let mut nodes = parse_nodes(Some(&v));
        for n in nodes.iter_mut() {
            n.label_w = 40.0;
        }
        let pairs = [
            ("limits", "derivatives"),
            ("limits", "integrals"),
            ("derivatives", "series"),
            ("a", "b"),
            ("b", "c"),
            ("c", "d"),
            ("d", "e"),
            ("a", "limits"),
        ];
        let edges = parse_edges(Some(&json!(pairs
            .iter()
            .map(|(s, t)| json!({"source": s, "target": t}))
            .collect::<Vec<_>>())));
        layout_nodes(&mut nodes, &edges, &[], &opts("force"));
        for (n, w) in nodes.iter().zip(want) {
            assert!(
                (n.x - w.1).abs() < 6.0 && (n.y - w.2).abs() < 6.0,
                "{} at ({:.2},{:.2}) vs d3 ({},{})",
                w.0,
                n.x,
                n.y,
                w.1,
                w.2
            );
        }
    }

    #[test]
    fn a_node_with_both_coordinates_keeps_them_as_the_start_and_a_zero_coordinate_does_not() {
        let v = json!([{"id":"a","x":100,"y":50},{"id":"b","x":0,"y":50}]);
        let n = parse_nodes(Some(&v));
        assert_eq!(n[0].given, Some((100.0, 50.0)));
        assert_eq!(n[1].given, None);
    }

    // std fixture: kflow-path `graph-canvas` (concept map: id/label/group levels, parent edges)
    fn path_props() -> Value {
        json!({
            "type": "graph-canvas", "height": 420, "showLabels": true, "interactive": true, "draggable": true,
            "nodeClickEvent": "GRAPH_NODE_CLICK",
            "nodes": [
                {"id":"limits","label":"Limits","group":"Level 1"},
                {"id":"derivatives","label":"Derivatives","group":"Level 2"},
                {"id":"integrals","label":"Integrals","group":"Level 2"},
                {"id":"series","label":"Series and sequences","group":"Level 3"}
            ],
            "edges": [
                {"source":"limits","target":"derivatives"},
                {"source":"limits","target":"integrals"},
                {"source":"derivatives","target":"series"}
            ]
        })
    }

    // std fixture: studio-deploy topology graph (tone colors, size 10, selected unit, edge labels)
    fn deploy_props() -> Value {
        json!({
            "type": "graph-canvas", "height": 320, "showLabels": true, "interactive": true, "draggable": true,
            "nodeClickEvent": "UNIT_CLICK", "selectedNodeId": "api",
            "nodes": [
                {"id":"api","label":"api","group":"compute","color":"success","size":10},
                {"id":"db","label":"db","group":"data","color":"warning","size":10},
                {"id":"cache","label":"cache","group":"data","color":"","size":10}
            ],
            "edges": [{"source":"api","target":"db","label":"sql"},{"source":"api","target":"cache","label":"get"}]
        })
    }

    fn circles(items: &[Item]) -> Vec<&Shape> {
        items
            .iter()
            .filter_map(|i| match i {
                Item::Shape(s) if s.ty == shape::ShapeType::Circle => Some(s),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn groups_take_series_tones_in_first_seen_order_and_ungrouped_is_muted_in_a_mixed_graph() {
        let g = build(&path_props(), Some((390.0, 300.0)));
        assert_eq!(g.groups, vec!["Level 1", "Level 2", "Level 3"]);
        assert_eq!(node_color(&g, &g.nodes[0]), "series-1");
        assert_eq!(node_color(&g, &g.nodes[1]), "series-2");
        assert_eq!(node_color(&g, &g.nodes[2]), "series-2");
        assert_eq!(node_color(&g, &g.nodes[3]), "series-3");
        let mixed = build(&json!({"nodes":[{"id":"a","group":"g"},{"id":"b"}]}), None);
        assert_eq!(node_color(&mixed, &mixed.nodes[1]), "muted");
        let none = build(&json!({"nodes":[{"id":"a"},{"id":"b"}]}), None);
        assert_eq!(node_color(&none, &none.nodes[0]), "series-1");
    }

    #[test]
    fn an_explicit_node_color_wins_and_an_empty_one_falls_to_the_group_color() {
        let g = build(&deploy_props(), None);
        assert_eq!(node_color(&g, &g.nodes[0]), "success");
        assert_eq!(node_color(&g, &g.nodes[2]), "series-2");
    }

    #[test]
    fn the_selected_node_is_larger_with_a_bold_highlight_ring_and_others_are_not() {
        let theme = DiagramTheme::heritage(false);
        let g = build(&deploy_props(), None);
        let items = derive_items(&g, &theme);
        let c = circles(&items);
        assert_eq!(c[0].radius, Some(14.0));
        assert_eq!(c[0].line_width, Some(theme.strokes[2]));
        assert_eq!(c[1].radius, Some(10.0));
        assert_eq!(c[1].line_width, Some(theme.strokes[0]));
        let mut p = deploy_props();
        p.as_object_mut().unwrap().remove("selectedNodeId");
        let g2 = build(&p, None);
        let items2 = derive_items(&g2, &theme);
        assert_eq!(
            circles(&items2)[0].radius,
            Some(10.0),
            "control: nothing selected"
        );
    }

    #[test]
    fn edges_draw_first_faint_with_labels_at_the_midpoint_and_at_the_link_opacity() {
        let theme = DiagramTheme::heritage(false);
        let g = build(&deploy_props(), None);
        let items = derive_items(&g, &theme);
        let Item::Shape(first) = &items[0] else {
            panic!()
        };
        assert_eq!(first.ty, shape::ShapeType::Line);
        assert_eq!(first.opacity, Some(0.18));
        assert_eq!(first.line_width, Some(theme.strokes[0]));
        let Item::Shape(label) = &items[1] else {
            panic!()
        };
        assert_eq!(label.text.as_deref(), Some("sql"));
        let (a, b) = (&g.nodes[0], &g.nodes[1]);
        assert!((label.x.unwrap() as f64 - (a.x + b.x) / 2.0).abs() < 1e-3);
        assert!((label.y.unwrap() as f64 - ((a.y + b.y) / 2.0 - 4.0)).abs() < 1e-3);
        let mut p = deploy_props();
        p["showLabels"] = json!(false);
        let off = derive_items(&build(&p, None), &theme);
        assert!(
            !off.iter()
                .any(|i| matches!(i, Item::Shape(s) if s.ty == shape::ShapeType::Text)),
            "control: showLabels off"
        );
    }

    #[test]
    fn proposed_nodes_are_muted_dotted_with_a_pulse_ring_and_suggested_get_a_ring_and_dot() {
        let theme = DiagramTheme::heritage(false);
        let props = json!({"nodes":[
            {"id":"p","label":"p","mark":{"kind":"proposed","suggestionId":"s1"}},
            {"id":"s","label":"s","mark":{"kind":"suggested","suggestionId":"s2"}},
            {"id":"n","label":"n"}
        ], "layout":"grid", "proposedEdges":[{"source":"p","target":"s"}]});
        let g = build(&props, None);
        let items = derive_items(&g, &theme);
        let Item::Shape(proposed_edge) = &items[0] else {
            panic!()
        };
        assert_eq!(
            (proposed_edge.dash, proposed_edge.opacity),
            (Some(Dash::Dashed), Some(0.4))
        );
        let c = circles(&items);
        assert_eq!(c[0].dash, Some(Dash::Dotted));
        assert_eq!(c[0].opacity, Some(0.55));
        assert_eq!(c[1].radius, Some(14.0), "pulse ring at radius + 6, phase 0");
        assert!((c[1].opacity.unwrap() - 0.22).abs() < 1e-6);
        assert_eq!(c[2].radius, Some(8.0));
        assert_eq!(c[3].radius, Some(11.0), "suggested ring at radius + 3");
        assert_eq!(
            items
                .iter()
                .filter(|i| matches!(i, Item::Dot { .. }))
                .count(),
            1
        );
        let plain = circles(&items).len();
        assert_eq!(plain, 5, "p circle+ring, s circle+ring, n circle");
    }

    #[test]
    fn a_badge_over_one_draws_a_disc_and_a_haloless_600_count_and_one_does_not() {
        let props =
            json!({"nodes":[{"id":"a","label":"a","badge":5,"size":20}],"interactive":false});
        let theme = DiagramTheme::heritage(false);
        let with = derive_items(&build(&props, None), &theme);
        assert_eq!(
            circles(&with)[1].radius,
            Some(9.0),
            "badge radius clamp(7, 11, 0.45 * 20)"
        );
        let cmds = map(&props, None).unwrap().packet.draw_commands;
        let texts: Vec<_> = cmds
            .iter()
            .filter_map(|c| match c {
                DrawCommand::Text {
                    text, weight, halo, ..
                } => Some((text.clone(), *weight, halo.is_some())),
                _ => None,
            })
            .collect();
        assert_eq!(
            texts,
            vec![("a".to_string(), 600, true), ("5".to_string(), 600, false)]
        );
        let one = derive_items(
            &build(&json!({"nodes":[{"id":"a","label":"a","badge":1}]}), None),
            &theme,
        );
        assert_eq!(circles(&one).len(), 1, "control: badge 1 draws nothing");
    }

    #[test]
    fn a_selected_node_label_is_weight_700_and_an_unselected_one_keeps_the_theme_weight() {
        let weights = |p: &Value| -> Vec<u16> {
            map(p, None)
                .unwrap()
                .packet
                .draw_commands
                .iter()
                .filter_map(|c| match c {
                    DrawCommand::Text { weight, .. } => Some(*weight),
                    _ => None,
                })
                .collect()
        };
        let mut p = json!({"nodes":[{"id":"a","label":"a"}],"selectedNodeId":"a"});
        assert_eq!(weights(&p), vec![700]);
        p["selectedNodeId"] = json!("zzz");
        assert_eq!(weights(&p), vec![600]);
    }

    #[test]
    fn the_scene_clears_to_ground_paints_in_order_and_fits_the_viewport() {
        let scene = map(&path_props(), Some((390.0, 300.0))).unwrap();
        assert_eq!(scene.packet.pattern, "graph-canvas");
        assert_eq!((scene.packet.width, scene.packet.height), (390.0, 300.0));
        let DrawCommand::Clear { color } = path_of(&scene, 0) else {
            panic!()
        };
        assert_eq!(*color, DiagramTheme::heritage(false).ground);
        let n = scene.packet.draw_commands.len();
        assert!(
            matches!(scene.packet.draw_commands[n - 1], DrawCommand::Text { .. }),
            "last node label is last"
        );
    }

    #[test]
    fn a_tap_on_a_node_emits_the_node_click_event_with_id_and_row_and_empty_space_emits_nothing() {
        let scene = map(&path_props(), Some((390.0, 300.0))).unwrap();
        let g = build(&path_props(), Some((390.0, 300.0)));
        let (vf, ..) = fit(g.width as f32, g.height as f32, Some((390.0, 300.0)));
        let xf = composed(vf, g.view);
        let n = &g.nodes[1];
        let (px, py) = xf.p(n.x as f32, n.y as f32);
        let emits = scene.emits_at(px, py);
        assert_eq!(emits.len(), 1);
        assert_eq!(emits[0].event, "GRAPH_NODE_CLICK");
        assert_eq!(emits[0].payload["id"], "derivatives");
        assert_eq!(emits[0].payload["row"]["label"], "Derivatives");
        assert!(scene.emits_at(0.5, 0.5).is_empty(), "control: empty corner");
        let mut quiet = path_props();
        quiet.as_object_mut().unwrap().remove("nodeClickEvent");
        assert!(
            map(&quiet, Some((390.0, 300.0))).unwrap().hits.is_empty(),
            "control: no handler authored"
        );
        let mut off = path_props();
        off["interactive"] = json!(false);
        off["draggable"] = json!(false);
        assert!(
            map(&off, Some((390.0, 300.0))).unwrap().hits.is_empty(),
            "control: not interactive"
        );
    }

    #[test]
    fn mark_click_takes_priority_over_node_click_and_badge_click_over_both() {
        let props = json!({
            "layout":"grid", "nodeClickEvent":"N", "onNodeClick":"NC", "onMarkClick":"M", "onBadgeClick":"B",
            "nodes":[{"id":"m","mark":{"kind":"suggested","suggestionId":"s"},"size":20},{"id":"b","badge":3,"size":20},{"id":"p","size":20}],
        });
        let g = build(&props, None);
        let scene = map(&props, None).unwrap();
        let xf = composed(fit(g.width as f32, g.height as f32, None).0, g.view);
        let at = |i: usize| xf.p(g.nodes[i].x as f32, g.nodes[i].y as f32);
        let (mx, my) = at(0);
        let e = scene.emits_at(mx, my);
        assert_eq!(
            e.iter().map(|e| e.event.as_str()).collect::<Vec<_>>(),
            vec!["M"]
        );
        let (px, py) = at(2);
        let e = scene.emits_at(px, py);
        assert_eq!(
            e.iter().map(|e| e.event.as_str()).collect::<Vec<_>>(),
            vec!["N", "NC"]
        );
        let (bx, by) = at(1);
        let (bbx, bby, _) = badge_geometry(&g.nodes[1], 20.0);
        let (sx, sy) = xf.p(bbx as f32, bby as f32);
        assert_eq!(scene.emits_at(sx, sy)[0].event, "B");
        assert_eq!(
            scene
                .emits_at(bx, by)
                .iter()
                .map(|e| e.event.as_str())
                .collect::<Vec<_>>(),
            vec!["N", "NC"],
            "control: badge body away from the badge"
        );
    }

    #[test]
    fn an_empty_graph_loading_or_error_leaves_a_blank_canvas() {
        for p in [
            json!({}),
            json!({"isLoading": true, "nodes":[{"id":"a"}]}),
            json!({"error":{"message":"x"},"nodes":[{"id":"a"}]}),
        ] {
            let s = map(&p, Some((100.0, 100.0))).unwrap();
            assert_eq!(s.packet.draw_commands.len(), 1);
        }
        assert!(
            map(&json!({"nodes":[{"id":"a"}]}), None)
                .unwrap()
                .packet
                .draw_commands
                .len()
                > 1,
            "control"
        );
    }

    #[test]
    fn scene_width_defaults_to_800_without_a_viewport_and_height_is_the_prop() {
        let s = map(&path_props(), None).unwrap();
        assert_eq!((s.packet.width, s.packet.height), (800.0, 420.0));
    }
}
