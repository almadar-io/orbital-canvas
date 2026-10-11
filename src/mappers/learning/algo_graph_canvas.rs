//! `algo-graph-canvas` — port of `AlgoGraphCanvas.tsx`: nodes and edges placed
//! by one of four deterministic layouts and derived into `LearningShape`s,
//! drawn by [`canvas`].

use super::canvas::{self, CanvasSpec};
use super::gap_once;
use super::shape::{parse_shapes, Align, Shape, ShapeType};
use crate::mappers::game_canvas_2d::{get_string, CanvasSceneError};
use crate::mappers::scene::{Emit, MappedScene};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};

const DEFAULT_NODE_RADIUS: f64 = 18.0;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct Badge {
    text: String,
    color: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct Node {
    id: String,
    x: Option<f64>,
    y: Option<f64>,
    label: Option<String>,
    state: Option<String>,
    color: Option<String>,
    radius: Option<f64>,
    badge: Option<Badge>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct Edge {
    from: String,
    to: String,
    directed: Option<bool>,
    weight: Option<f64>,
    label: Option<String>,
    state: Option<String>,
    color: Option<String>,
}

type Positions = HashMap<String, (f64, f64)>;

fn parse_list<T: for<'de> Deserialize<'de>>(props: &Value, key: &str) -> Vec<T> {
    let Some(items) = props.get(key).and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match serde_json::from_value::<T>(item.clone()) {
            Ok(v) => Some(v),
            Err(err) => {
                gap_once("algo-graph-invalid", &format!("{key} entry skipped: {err}"));
                None
            }
        })
        .collect()
}

fn node_state_color(state: &str) -> Option<&'static str> {
    match state {
        "unvisited" => Some("ink"),
        "frontier" => Some("highlight"),
        "current" => Some("primary"),
        "visited" => Some("muted"),
        "goal" => Some("success"),
        "path" => Some("accent"),
        _ => None,
    }
}

fn edge_state_color(state: &str) -> Option<&'static str> {
    match state {
        "default" => Some("ink"),
        "tree" => Some("muted"),
        "relaxed" => Some("primary"),
        "candidate" => Some("highlight"),
        "path" => Some("accent"),
        _ => None,
    }
}

fn layout_circle(nodes: &[Node], width: f64, height: f64) -> Positions {
    let (cx, cy) = (width / 2.0, height / 2.0);
    let radius = 10f64.max(cx.min(cy) - 40.0);
    let n = nodes.len().max(1) as f64;
    nodes
        .iter()
        .enumerate()
        .map(|(i, node)| {
            let angle = 2.0 * std::f64::consts::PI * i as f64 / n - std::f64::consts::FRAC_PI_2;
            (
                node.id.clone(),
                (cx + radius * angle.cos(), cy + radius * angle.sin()),
            )
        })
        .collect()
}

fn assign_x_slot(
    u: &str,
    tree_children: &HashMap<String, Vec<String>>,
    leaf: &mut usize,
    slots: &mut HashMap<String, f64>,
) -> f64 {
    let children = tree_children.get(u).map(Vec::as_slice).unwrap_or(&[]);
    if children.is_empty() {
        let slot = *leaf as f64;
        *leaf += 1;
        slots.insert(u.to_string(), slot);
        return slot;
    }
    let sum: f64 = children
        .iter()
        .map(|c| assign_x_slot(c, tree_children, leaf, slots))
        .sum();
    let avg = sum / children.len() as f64;
    slots.insert(u.to_string(), avg);
    avg
}

fn layout_tree(
    nodes: &[Node],
    edges: &[Edge],
    root: Option<&str>,
    width: f64,
    height: f64,
) -> Positions {
    let node_ids: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    let id_set: HashSet<&str> = node_ids.iter().copied().collect();
    let mut children_of: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut has_incoming: HashSet<&str> = HashSet::new();
    for e in edges {
        if !id_set.contains(e.from.as_str()) || !id_set.contains(e.to.as_str()) {
            continue;
        }
        children_of
            .entry(e.from.as_str())
            .or_default()
            .push(e.to.as_str());
        has_incoming.insert(e.to.as_str());
    }

    let mut depth: HashMap<String, usize> = HashMap::new();
    let mut tree_children: HashMap<String, Vec<String>> = HashMap::new();
    let mut visited: HashSet<String> = HashSet::new();

    let mut bfs_from = |start: &str| -> bool {
        if visited.contains(start) {
            return false;
        }
        visited.insert(start.to_string());
        depth.insert(start.to_string(), 0);
        let mut queue: VecDeque<String> = VecDeque::from([start.to_string()]);
        while let Some(u) = queue.pop_front() {
            for &v in children_of
                .get(u.as_str())
                .map(Vec::as_slice)
                .unwrap_or(&[])
            {
                if visited.contains(v) {
                    continue;
                }
                visited.insert(v.to_string());
                let d = depth.get(&u).copied().unwrap_or(0) + 1;
                depth.insert(v.to_string(), d);
                tree_children
                    .entry(u.clone())
                    .or_default()
                    .push(v.to_string());
                queue.push_back(v.to_string());
            }
        }
        true
    };

    let primary = match root {
        Some(r) if !r.is_empty() && id_set.contains(r) => Some(r),
        _ => node_ids
            .iter()
            .copied()
            .find(|id| !has_incoming.contains(id))
            .or_else(|| node_ids.first().copied()),
    };
    let mut roots_order: Vec<&str> = Vec::new();
    if let Some(p) = primary {
        bfs_from(p);
        roots_order.push(p);
    }
    for &id in &node_ids {
        if bfs_from(id) {
            roots_order.push(id);
        }
    }

    let mut leaf = 0usize;
    let mut x_slot: HashMap<String, f64> = HashMap::new();
    for r in roots_order {
        assign_x_slot(r, &tree_children, &mut leaf, &mut x_slot);
    }
    let max_depth = depth.values().copied().max().unwrap_or(0);
    let col_width = width / leaf.max(1) as f64;
    let row_height = height / (max_depth + 1) as f64;

    node_ids
        .iter()
        .map(|&id| {
            let slot = x_slot.get(id).copied().unwrap_or(0.0);
            let d = depth.get(id).copied().unwrap_or(0) as f64;
            (
                id.to_string(),
                (
                    slot * col_width + col_width / 2.0,
                    d * row_height + row_height / 2.0,
                ),
            )
        })
        .collect()
}

fn layout_layered(nodes: &[Node], edges: &[Edge], width: f64, height: f64) -> Positions {
    let node_ids: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    let id_set: HashSet<&str> = node_ids.iter().copied().collect();
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut remaining: HashMap<&str, i64> = node_ids.iter().map(|&id| (id, 0)).collect();
    for e in edges {
        if !id_set.contains(e.from.as_str()) || !id_set.contains(e.to.as_str()) {
            continue;
        }
        adj.entry(e.from.as_str()).or_default().push(e.to.as_str());
        *remaining.entry(e.to.as_str()).or_insert(0) += 1;
    }

    let mut layer: HashMap<&str, usize> = HashMap::new();
    let mut dequeued: HashSet<&str> = HashSet::new();
    let mut queue: VecDeque<&str> = VecDeque::new();
    for &id in &node_ids {
        if remaining.get(id).copied().unwrap_or(0) == 0 {
            layer.insert(id, 0);
            queue.push_back(id);
        }
    }
    while let Some(u) = queue.pop_front() {
        dequeued.insert(u);
        for &v in adj.get(u).map(Vec::as_slice).unwrap_or(&[]) {
            let candidate = layer.get(u).copied().unwrap_or(0) + 1;
            let cur = layer.get(v).copied().unwrap_or(0);
            layer.insert(v, cur.max(candidate));
            let r = remaining.get(v).copied().unwrap_or(0) - 1;
            remaining.insert(v, r);
            if r == 0 && !dequeued.contains(v) {
                queue.push_back(v);
            }
        }
    }

    let base_max = node_ids
        .iter()
        .filter(|id| dequeued.contains(*id))
        .map(|id| layer.get(id).copied().unwrap_or(0))
        .max()
        .unwrap_or(0);
    let cycle_layer = base_max + 1;
    let mut max_layer = base_max;
    for &id in &node_ids {
        if !dequeued.contains(id) {
            layer.insert(id, cycle_layer);
            max_layer = cycle_layer;
        }
    }

    let col_width = width / (max_layer + 1) as f64;
    let mut by_layer: Vec<(usize, Vec<&str>)> = Vec::new();
    for &id in &node_ids {
        let l = layer.get(id).copied().unwrap_or(0);
        match by_layer.iter_mut().find(|(k, _)| *k == l) {
            Some((_, ids)) => ids.push(id),
            None => by_layer.push((l, vec![id])),
        }
    }
    let mut positions = Positions::new();
    for (l, ids) in by_layer {
        let row_height = height / ids.len() as f64;
        for (i, id) in ids.iter().enumerate() {
            positions.insert(
                id.to_string(),
                (
                    l as f64 * col_width + col_width / 2.0,
                    i as f64 * row_height + row_height / 2.0,
                ),
            );
        }
    }
    positions
}

fn compute_positions(
    nodes: &[Node],
    edges: &[Edge],
    layout: &str,
    root: Option<&str>,
    width: f64,
    height: f64,
) -> Positions {
    match layout {
        "circle" => layout_circle(nodes, width, height),
        "tree" => layout_tree(nodes, edges, root, width, height),
        "layered" => layout_layered(nodes, edges, width, height),
        _ => nodes
            .iter()
            .map(|n| (n.id.clone(), (n.x.unwrap_or(0.0), n.y.unwrap_or(0.0))))
            .collect(),
    }
}

fn js_number_text(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

struct EdgeGeom {
    directed: bool,
    p1: (f64, f64),
    p2: (f64, f64),
    color: Option<String>,
    label: Option<String>,
}

struct NodeGeom<'a> {
    node: &'a Node,
    x: f64,
    y: f64,
    radius: f64,
    color: Option<String>,
}

/// The derived shapes in web order: edges, edge labels, node circles, badge
/// pills, badge texts, node labels, then the authored `shapes`.
pub fn derive_shapes(props: &Value, width: f32, height: f32) -> Vec<Shape> {
    let nodes: Vec<Node> = parse_list(props, "nodes");
    let edges: Vec<Edge> = parse_list(props, "edges");
    let layout = get_string(props, "layout").unwrap_or_else(|| "manual".into());
    let root = get_string(props, "root");
    let positions = compute_positions(
        &nodes,
        &edges,
        &layout,
        root.as_deref(),
        f64::from(width),
        f64::from(height),
    );

    let mut node_by_id: HashMap<&str, &Node> = HashMap::new();
    for n in &nodes {
        node_by_id.insert(n.id.as_str(), n);
    }

    let mut edge_geoms = Vec::new();
    for e in &edges {
        let (Some(a), Some(b)) = (
            node_by_id.get(e.from.as_str()),
            node_by_id.get(e.to.as_str()),
        ) else {
            continue;
        };
        let (Some(&pa), Some(&pb)) = (positions.get(&e.from), positions.get(&e.to)) else {
            continue;
        };
        let (ra, rb) = (
            a.radius.unwrap_or(DEFAULT_NODE_RADIUS),
            b.radius.unwrap_or(DEFAULT_NODE_RADIUS),
        );
        let (dx, dy) = (pb.0 - pa.0, pb.1 - pa.1);
        let dist = 1e-6f64.max(dx.hypot(dy));
        let (ux, uy) = (dx / dist, dy / dist);
        let color = e.color.clone().or_else(|| {
            edge_state_color(e.state.as_deref().unwrap_or("default")).map(str::to_string)
        });
        edge_geoms.push(EdgeGeom {
            directed: e.directed.unwrap_or(false),
            p1: (pa.0 + ux * ra, pa.1 + uy * ra),
            p2: (pb.0 - ux * rb, pb.1 - uy * rb),
            color,
            label: e.label.clone().or_else(|| e.weight.map(js_number_text)),
        });
    }

    let mut out = Vec::new();
    for g in &edge_geoms {
        let (x1, y1, x2, y2) = (g.p1.0 as f32, g.p1.1 as f32, g.p2.0 as f32, g.p2.1 as f32);
        let mut s = if g.directed {
            Shape::arrow(x1, y1, x2, y2)
        } else {
            Shape::line(x1, y1, x2, y2)
        }
        .stroke_weight("normal");
        s.color = g.color.clone();
        out.push(s);
    }
    for g in &edge_geoms {
        let Some(label) = &g.label else { continue };
        let (mx, my) = ((g.p1.0 + g.p2.0) / 2.0, (g.p1.1 + g.p2.1) / 2.0);
        let (dx, dy) = (g.p2.0 - g.p1.0, g.p2.1 - g.p1.1);
        let dist = 1e-6f64.max(dx.hypot(dy));
        let (perp_x, perp_y) = (-(dy / dist), dx / dist);
        out.push(
            Shape::text(
                (mx + perp_x * 10.0) as f32,
                (my + perp_y * 10.0) as f32,
                label.clone(),
            )
            .text_size("sm")
            .align(Align::Center)
            .tone("label"),
        );
    }

    let mut node_geoms = Vec::new();
    for n in &nodes {
        let Some(&pos) = positions.get(&n.id) else {
            continue;
        };
        node_geoms.push(NodeGeom {
            node: n,
            x: pos.0,
            y: pos.1,
            radius: n.radius.unwrap_or(DEFAULT_NODE_RADIUS),
            color: n.color.clone().or_else(|| {
                node_state_color(n.state.as_deref().unwrap_or("unvisited")).map(str::to_string)
            }),
        });
    }
    for g in &node_geoms {
        let mut s = Shape::circle(g.x as f32, g.y as f32, g.radius as f32)
            .id(g.node.id.clone())
            .fill_style("tint")
            .stroke_weight("normal");
        s.color = g.color.clone();
        s.fill = g.color.clone();
        out.push(s);
    }

    let badges: Vec<(f64, f64, f64, &str, &str)> = node_geoms
        .iter()
        .filter_map(|g| {
            let badge = g.node.badge.as_ref().filter(|b| !b.text.is_empty())?;
            let w = 42f64.min(18f64.max(badge.text.encode_utf16().count() as f64 * 6.0 + 10.0));
            Some((
                g.x + g.radius * 0.75,
                g.y - g.radius * 0.75,
                w,
                badge.color.as_deref().unwrap_or("ink"),
                badge.text.as_str(),
            ))
        })
        .collect();
    for &(cx, cy, w, color, _) in &badges {
        let mut s = Shape::rect((cx - w / 2.0) as f32, (cy - 7.0) as f32, w as f32, 14.0)
            .color(color)
            .fill_style("solid")
            .stroke_weight("thin");
        s.fill_tone = Some("fill".into());
        out.push(s);
    }
    for &(cx, cy, _, _, text) in &badges {
        out.push(
            Shape::text(cx as f32, cy as f32, text)
                .text_size("xs")
                .align(Align::Center)
                .tone("ink"),
        );
    }

    for g in &node_geoms {
        let Some(label) = &g.node.label else { continue };
        out.push(
            Shape::text(g.x as f32, (g.y + g.radius + 14.0) as f32, label.clone())
                .text_size("sm")
                .align(Align::Center)
                .tone("ink"),
        );
    }

    out.extend(parse_shapes(props.get("shapes")));
    out
}

pub fn map(props: &Value, viewport: Option<(f32, f32)>) -> Result<MappedScene, CanvasSceneError> {
    if get_string(props, "title").is_some_and(|t| !t.is_empty()) {
        gap_once(
            "algo-graph-title",
            "the title is a Typography above the canvas, outside the drawn scene",
        );
    }
    let mut spec = CanvasSpec::from_props("algo-graph-canvas", props, 600.0, 400.0, Vec::new());
    spec.shapes = derive_shapes(props, spec.width, spec.height);
    let on_node_click = get_string(props, "onNodeClick");
    let theme = canvas::theme_for(props);
    let mut scene = canvas::render(&spec, &theme, viewport);

    if let Some(event) = on_node_click {
        let nodes: Vec<Node> = parse_list(props, "nodes");
        let mut index_by_id: HashMap<&str, usize> = HashMap::new();
        for (i, n) in nodes.iter().enumerate() {
            index_by_id.insert(n.id.as_str(), i);
        }
        let hit_shapes = spec
            .shapes
            .iter()
            .filter(|s| super::shape::bounds(s).is_some());
        for (hit, shape) in scene.hits.iter_mut().zip(hit_shapes) {
            let (ShapeType::Circle, Some(id)) = (shape.ty, shape.id.as_deref()) else {
                continue;
            };
            let Some(&index) = index_by_id.get(id) else {
                continue;
            };
            let Some(node) = nodes.get(index) else {
                continue;
            };
            let mut payload = json!({ "id": node.id, "index": index });
            if let Some(label) = &node.label {
                payload["label"] = Value::String(label.clone());
            }
            hit.emits.insert(
                0,
                Emit {
                    event: event.clone(),
                    arg: "payload".into(),
                    payload,
                },
            );
        }
    }
    Ok(scene)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::{DrawCommand, PathSegment};

    fn bfs_nodes() -> Value {
        json!(["A", "B", "C", "D", "E", "F", "G"]
            .map(|id| json!({"id": id, "label": id, "state": "unvisited"})))
    }

    fn bfs_edges() -> Value {
        json!([
            ["A", "B"],
            ["A", "C"],
            ["B", "D"],
            ["B", "E"],
            ["C", "F"],
            ["C", "G"],
            ["E", "F"]
        ]
        .map(
            |[f, t]| json!({"from": f, "to": t, "directed": false, "label": "", "state": "default"})
        ))
    }

    /// `std-algo-bfs` at its config defaults (INIT): circle layout, A frontier.
    fn bfs_scene() -> Value {
        let mut nodes = bfs_nodes();
        nodes[0]["state"] = json!("frontier");
        json!({"type": "algo-graph-canvas", "width": 600, "height": 300, "backgroundColor": "", "layout": "circle", "nodes": nodes, "edges": bfs_edges()})
    }

    fn near(a: Option<f32>, b: f32) -> bool {
        a.is_some_and(|a| (a - b).abs() < 0.01)
    }

    fn of(shapes: &[Shape], ty: ShapeType) -> Vec<&Shape> {
        shapes.iter().filter(|s| s.ty == ty).collect()
    }

    #[test]
    fn bfs_circle_layout_puts_node_a_at_the_top_of_a_ring_of_radius_110() {
        let shapes = derive_shapes(&bfs_scene(), 600.0, 300.0);
        let circles = of(&shapes, ShapeType::Circle);
        assert_eq!(circles.len(), 7);
        assert!(
            near(circles[0].x, 300.0) && near(circles[0].y, 40.0),
            "{:?}",
            (circles[0].x, circles[0].y)
        );
        let angle = 2.0 * std::f32::consts::PI / 7.0 - std::f32::consts::FRAC_PI_2;
        assert!(
            near(circles[1].x, 300.0 + 110.0 * angle.cos())
                && near(circles[1].y, 150.0 + 110.0 * angle.sin())
        );
    }

    #[test]
    fn bfs_states_pick_the_tone_and_edges_shorten_to_the_rim() {
        let shapes = derive_shapes(&bfs_scene(), 600.0, 300.0);
        let circles = of(&shapes, ShapeType::Circle);
        assert_eq!(circles[0].color.as_deref(), Some("highlight"));
        assert_eq!(circles[0].fill.as_deref(), Some("highlight"));
        assert_eq!(circles[0].fill_style.as_deref(), Some("tint"));
        assert_eq!(circles[1].color.as_deref(), Some("ink"));
        let lines = of(&shapes, ShapeType::Line);
        assert_eq!(lines.len(), 7);
        assert!(
            of(&shapes, ShapeType::Arrow).is_empty(),
            "undirected edges are lines"
        );
        let (a, b) = (circles[0], circles[1]);
        let d1 = ((lines[0].x1.unwrap_or(0.0) - a.x.unwrap_or(0.0)).powi(2)
            + (lines[0].y1.unwrap_or(0.0) - a.y.unwrap_or(0.0)).powi(2))
        .sqrt();
        let d2 = ((lines[0].x2.unwrap_or(0.0) - b.x.unwrap_or(0.0)).powi(2)
            + (lines[0].y2.unwrap_or(0.0) - b.y.unwrap_or(0.0)).powi(2))
        .sqrt();
        assert!((d1 - 18.0).abs() < 0.01 && (d2 - 18.0).abs() < 0.01);
        assert_eq!(lines[0].color.as_deref(), Some("ink"));
    }

    #[test]
    fn a_tree_state_edge_is_muted_and_a_control_default_edge_is_ink() {
        let mut props = bfs_scene();
        props["edges"][0]["state"] = json!("tree");
        let shapes = derive_shapes(&props, 600.0, 300.0);
        let lines = of(&shapes, ShapeType::Line);
        assert_eq!(lines[0].color.as_deref(), Some("muted"));
        assert_eq!(lines[1].color.as_deref(), Some("ink"));
    }

    #[test]
    fn node_labels_sit_below_the_circle_in_sm_ink_and_empty_edge_labels_draw_nothing() {
        let shapes = derive_shapes(&bfs_scene(), 600.0, 300.0);
        let texts = of(&shapes, ShapeType::Text);
        assert_eq!(
            texts.len(),
            14,
            "7 empty edge labels + 7 node labels (empty ones are painted as nothing)"
        );
        let label_a = texts
            .iter()
            .find(|t| t.text.as_deref() == Some("A"))
            .map(|t| (t.y, t.tone.clone(), t.text_size.clone()));
        assert_eq!(
            label_a,
            Some((
                Some(40.0 + 18.0 + 14.0),
                Some("ink".into()),
                Some("sm".into())
            ))
        );
        let scene = map(&bfs_scene(), None)
            .map(|s| s.packet.draw_commands)
            .unwrap_or_default();
        let painted = scene
            .iter()
            .filter(|c| matches!(c, DrawCommand::Text { .. }))
            .count();
        assert_eq!(painted, 7);
    }

    /// `std-learn-tree` after inserting 50,30,70,20,40: tree layout, root n0.
    fn tree_scene() -> Value {
        let nodes: Vec<Value> = ["50", "30", "70", "20", "40"]
            .iter()
            .enumerate()
            .map(|(i, v)| json!({"id": format!("n{i}"), "label": v, "state": "visited"}))
            .collect();
        let edge = |f: &str, t: &str, side: &str| json!({"from": f, "to": t, "directed": true, "label": side, "state": "tree"});
        json!({"type": "algo-graph-canvas", "width": 600, "height": 300, "layout": "tree", "root": "n0", "nodes": nodes,
               "edges": [edge("n0", "n1", "L"), edge("n0", "n2", "R"), edge("n1", "n3", "L"), edge("n1", "n4", "R")]})
    }

    #[test]
    fn tree_layout_centres_parents_over_their_children_and_rows_by_depth() {
        let shapes = derive_shapes(&tree_scene(), 600.0, 300.0);
        let c = of(&shapes, ShapeType::Circle);
        // leaves n3,n4,n2 take slots 0,1,2 of 3 (col 200); n1 = 0.5; n0 = 1.25 ; rows of 100
        let at = |i: usize| (c[i].x.unwrap_or(f32::NAN), c[i].y.unwrap_or(f32::NAN));
        assert_eq!(at(3), (100.0, 250.0));
        assert_eq!(at(4), (300.0, 250.0));
        assert_eq!(at(1), (200.0, 150.0));
        assert_eq!(at(2), (500.0, 150.0));
        assert_eq!(at(0), (350.0, 50.0));
        assert_eq!(of(&shapes, ShapeType::Arrow).len(), 4);
        assert_eq!(c[0].color.as_deref(), Some("muted"));
    }

    fn tree_scene_y0() -> Option<f32> {
        derive_shapes(&tree_scene(), 600.0, 300.0)
            .iter()
            .find(|s| s.ty == ShapeType::Circle)
            .and_then(|s| s.y)
    }

    #[test]
    fn tree_root_prop_changes_the_layout_and_a_cycle_does_not_loop() {
        let mut props = tree_scene();
        props["root"] = json!("n1");
        let shapes = derive_shapes(&props, 600.0, 300.0);
        let c = of(&shapes, ShapeType::Circle);
        // n1 roots first (n3,n4 leaves); n0 then roots its own tree with n2: depth 1 => 150px rows
        assert_eq!(c[1].y, Some(75.0), "n1 is the explicit root, at depth 0");
        assert_eq!(c[0].y, Some(75.0), "n0 is a second root, also depth 0");
        assert_eq!(
            tree_scene_y0(),
            Some(50.0),
            "control: root n0 gives 100px rows"
        );
        let mut cyc = tree_scene();
        cyc["edges"]
            .as_array_mut()
            .map(|e| e.push(json!({"from": "n4", "to": "n0"})));
        assert_eq!(
            of(&derive_shapes(&cyc, 600.0, 300.0), ShapeType::Circle).len(),
            5
        );
    }

    /// `std-algo-toposort` INIT: layered layout, directed edges, in-degree badges.
    fn topo_scene() -> Value {
        let degs = [
            ("A", "0"),
            ("B", "0"),
            ("C", "2"),
            ("D", "1"),
            ("E", "1"),
            ("F", "2"),
        ];
        let nodes: Vec<Value> = degs
            .iter()
            .map(|(id, d)| json!({"id": id, "label": id, "state": if *d == "0" { "frontier" } else { "unvisited" }, "badge": {"text": d}}))
            .collect();
        let edges: Vec<Value> = [
            ("A", "C"),
            ("B", "C"),
            ("C", "D"),
            ("C", "E"),
            ("D", "F"),
            ("E", "F"),
        ]
        .iter()
        .map(
            |(f, t)| json!({"from": f, "to": t, "directed": true, "label": "", "state": "default"}),
        )
        .collect();
        json!({"type": "algo-graph-canvas", "width": 600, "height": 300, "layout": "layered", "nodes": nodes, "edges": edges})
    }

    #[test]
    fn layered_layout_stacks_dag_layers_left_to_right() {
        let shapes = derive_shapes(&topo_scene(), 600.0, 300.0);
        let c = of(&shapes, ShapeType::Circle);
        let at = |i: usize| (c[i].x.unwrap_or(f32::NAN), c[i].y.unwrap_or(f32::NAN));
        // layers: A,B=0  C=1  D,E=2  F=3 ; col 150
        assert_eq!(at(0), (75.0, 75.0));
        assert_eq!(at(1), (75.0, 225.0));
        assert_eq!(at(2), (225.0, 150.0));
        assert_eq!(at(3), (375.0, 75.0));
        assert_eq!(at(4), (375.0, 225.0));
        assert_eq!(at(5), (525.0, 150.0));
        assert_eq!(of(&shapes, ShapeType::Arrow).len(), 6);
    }

    #[test]
    fn layered_cycle_nodes_park_one_layer_past_the_dag() {
        let props = json!({"layout": "layered", "nodes": [{"id": "a"}, {"id": "b"}, {"id": "c"}],
            "edges": [{"from": "a", "to": "b"}, {"from": "b", "to": "c"}, {"from": "c", "to": "b"}]});
        let shapes = derive_shapes(&props, 600.0, 400.0);
        let c = of(&shapes, ShapeType::Circle);
        assert_eq!(c[0].x, Some(150.0), "a alone in layer 0 of 2 columns");
        assert_eq!(c[1].x, Some(450.0));
        assert_eq!(c[2].x, Some(450.0));
        assert_eq!((c[1].y, c[2].y), (Some(100.0), Some(300.0)));
    }

    #[test]
    fn badges_draw_a_solid_pill_and_xs_text_only_when_non_empty() {
        let shapes = derive_shapes(&topo_scene(), 600.0, 300.0);
        let rects = of(&shapes, ShapeType::Rect);
        assert_eq!(rects.len(), 6);
        let r = rects[0];
        assert_eq!((r.width, r.height), (Some(18.0), Some(14.0)));
        assert_eq!(r.fill_tone.as_deref(), Some("fill"));
        assert_eq!(r.fill_style.as_deref(), Some("solid"));
        assert_eq!(r.stroke.as_deref(), Some("thin"));
        assert_eq!(r.color.as_deref(), Some("ink"));
        assert!(near(r.x, 75.0 + 13.5 - 9.0) && near(r.y, 75.0 - 13.5 - 7.0));
        assert!(of(&shapes, ShapeType::Text)
            .iter()
            .any(|t| t.text_size.as_deref() == Some("xs") && t.text.as_deref() == Some("2")));

        let mut empty = topo_scene();
        empty["nodes"][0]["badge"] = json!({"text": ""});
        assert_eq!(
            of(&derive_shapes(&empty, 600.0, 300.0), ShapeType::Rect).len(),
            5
        );
        let mut long = topo_scene();
        long["nodes"][0]["badge"] = json!({"text": "123456789", "color": "primary"});
        let wide = derive_shapes(&long, 600.0, 300.0);
        assert_eq!(of(&wide, ShapeType::Rect)[0].width, Some(42.0));
        assert_eq!(
            of(&wide, ShapeType::Rect)[0].color.as_deref(),
            Some("primary")
        );
    }

    #[test]
    fn manual_layout_uses_authored_xy_and_weight_becomes_the_edge_label() {
        let props = json!({"nodes": [{"id": "a", "x": 100, "y": 100, "radius": 10}, {"id": "b", "x": 300, "y": 100}],
            "edges": [{"from": "a", "to": "b", "weight": 7, "state": "relaxed"}]});
        let shapes = derive_shapes(&props, 600.0, 400.0);
        let line = of(&shapes, ShapeType::Line)[0];
        assert_eq!((line.x1, line.x2), (Some(110.0), Some(282.0)));
        assert_eq!(line.color.as_deref(), Some("primary"));
        let label = of(&shapes, ShapeType::Text)[0];
        assert_eq!(label.text.as_deref(), Some("7"));
        assert!(
            near(label.x, 196.0) && near(label.y, 110.0),
            "perpendicular offset of 10 below a rightward edge"
        );
        let mut no_weight = props.clone();
        no_weight["edges"][0]["weight"] = Value::Null;
        assert!(of(&derive_shapes(&no_weight, 600.0, 400.0), ShapeType::Text).is_empty());
    }

    #[test]
    fn authored_shapes_follow_the_derived_ones() {
        let mut props = bfs_scene();
        let base = derive_shapes(&props, 600.0, 300.0).len();
        props["shapes"] = json!([{"type": "rect", "x": 1, "y": 1, "width": 5, "height": 5}]);
        let shapes = derive_shapes(&props, 600.0, 300.0);
        assert_eq!(shapes.len(), base + 1);
        assert_eq!(shapes[base].ty, ShapeType::Rect);
    }

    fn interactive(mut props: Value) -> Value {
        props["interactive"] = json!(true);
        props
    }

    #[test]
    fn tapping_a_node_emits_shape_click_then_node_click_with_the_web_payloads() {
        let mut props = interactive(bfs_scene());
        props["onShapeClick"] = json!("PICK_SHAPE");
        props["onNodeClick"] = json!("PICK_NODE");
        let scene = map(&props, None).expect("scene");
        let emits = scene.emits_at(300.0, 40.0);
        assert_eq!(emits.len(), 2);
        assert_eq!(emits[0].event, "PICK_NODE");
        assert_eq!(
            emits[0].payload,
            json!({"id": "A", "label": "A", "index": 0})
        );
        assert_eq!(emits[1].event, "PICK_SHAPE");
        assert_eq!(emits[1].payload["id"], "A");
        assert_eq!(emits[1].payload["type"], "circle");
        assert_eq!(
            emits[1].payload["index"], 14,
            "7 edges + 7 edge-label texts precede the first circle"
        );
    }

    #[test]
    fn without_onnodeclick_a_node_tap_emits_only_the_shape_event() {
        let mut props = interactive(bfs_scene());
        props["onShapeClick"] = json!("PICK_SHAPE");
        let scene = map(&props, None).expect("scene");
        let emits = scene.emits_at(300.0, 40.0);
        assert_eq!(emits.len(), 1);
        assert_eq!(emits[0].event, "PICK_SHAPE");
    }

    #[test]
    fn only_onnodeclick_emits_for_nodes_and_ignores_non_node_shapes() {
        let mut props = interactive(bfs_scene());
        props["onNodeClick"] = json!("PICK_NODE");
        let scene = map(&props, None).expect("scene");
        let emits = scene.emits_at(300.0, 40.0);
        assert_eq!(emits.len(), 1);
        assert_eq!(emits[0].event, "PICK_NODE");
        let label = scene.emits_at(300.0, 40.0 + 18.0 + 14.0 - 6.0);
        assert!(
            label.is_empty(),
            "a non-node shape emits nothing without onShapeClick"
        );
        let control = map(&interactive(bfs_scene()), None).expect("scene");
        assert!(
            control.emits_at(300.0, 40.0).is_empty(),
            "control: neither handler authored, no events"
        );
    }

    #[test]
    fn a_non_interactive_graph_has_no_hits_and_the_viewport_fit_keeps_node_taps() {
        let mut props = bfs_scene();
        props["onNodeClick"] = json!("PICK_NODE");
        assert!(map(&props, None).expect("scene").hits.is_empty());
        let scene = map(&interactive(props), Some((300.0, 150.0))).expect("scene");
        assert_eq!(scene.emits_at(150.0, 20.0)[0].payload["id"], "A");
    }

    #[test]
    fn node_circles_paint_as_tint_filled_arcs_in_the_state_tone() {
        let scene = map(&bfs_scene(), None).expect("scene");
        let theme = canvas::theme_for(&bfs_scene());
        let arcs: Vec<_> = scene
            .packet
            .draw_commands
            .iter()
            .filter_map(|c| match c {
                DrawCommand::Path {
                    segments,
                    fill: Some(f),
                    stroke: Some(s),
                    ..
                } if matches!(segments[0], PathSegment::Arc { .. }) => Some((*f, s.color)),
                _ => None,
            })
            .collect();
        assert_eq!(arcs.len(), 7);
        assert_eq!(Some(arcs[0].1), theme.tone("highlight"));
        assert_eq!(Some(arcs[1].1), theme.tone("ink"));
        assert!(arcs[0].0[3] < 1.0, "tint fill is translucent");
    }
}
