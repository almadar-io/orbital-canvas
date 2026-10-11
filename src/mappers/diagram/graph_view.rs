//! `graph-view` — the Rust twin of `GraphView.tsx`: nodes and edges laid out by the force
//! simulation or a static flow / tree / radial layout, drawn as circles and lines under the web's
//! zoom-to-fit view box. A tap runs the same pair of callbacks the web's touch path runs:
//! `onNodeHover(node)` then `onNodeClick(node)`.

use super::draw::{frame, with_alpha, Ink, Pen};
use super::graph_layout::{force_layout, static_layout, LayoutMode, Point};
use super::{props_f32, props_str};
use crate::mappers::game_canvas_2d::CanvasSceneError;
use crate::mappers::learning::canvas::theme_for;
use crate::mappers::learning::Xf;
use crate::mappers::scene::{Emit, MappedScene};
use crate::packet::TextAlign;
use serde_json::{json, Value};

const DEFAULT_NODE_SIZE: f32 = 8.0;
const DEFAULT_NODE_COLOR: &str = "var(--color-muted-foreground)";
const DEFAULT_EDGE_COLOR: &str = "var(--color-border)";
const ZOOM_PAD: f32 = 60.0;
const SERIES: usize = 8;

struct GNode {
    id: String,
    label: Option<String>,
    color: String,
    size: f32,
    group: Option<String>,
    at: Point,
}

struct GEdge {
    source: usize,
    target: usize,
    color: Option<String>,
    label: Option<String>,
}

fn opt_string(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn node_payload(n: &GNode) -> Value {
    let mut v = json!({ "id": n.id, "color": n.color, "size": n.size });
    if let Some(l) = &n.label {
        v["label"] = json!(l);
    }
    if let Some(g) = &n.group {
        v["group"] = json!(g);
    }
    v
}

pub fn map(props: &Value, viewport: Option<(f32, f32)>) -> Result<MappedScene, CanvasSceneError> {
    let theme = theme_for(props);
    let w = props_f32(props, "width", 600.0).max(1.0);
    let h = props_f32(props, "height", 400.0).max(1.0);
    let (outer, pw, ph) = frame(w, h, viewport);
    let mut pen = Pen::new(&theme, outer);
    pen.clear(theme.ground);

    let raw_nodes: Vec<&Value> = props
        .get("nodes")
        .and_then(Value::as_array)
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    let ids: Vec<String> = raw_nodes
        .iter()
        .filter_map(|n| opt_string(n, "id"))
        .collect();
    if ids.is_empty() {
        let ink = pen.tone("label");
        pen.text(
            w / 2.0,
            h / 2.0 + 5.0,
            "No graph data",
            14.0,
            ink,
            TextAlign::Center,
            400,
        );
        return Ok(pen.finish("graph-view", pw, ph));
    }

    let mut groups: Vec<String> = Vec::new();
    for n in &raw_nodes {
        if let Some(g) = opt_string(n, "group") {
            if !groups.contains(&g) {
                groups.push(g);
            }
        }
    }
    let raw_edges: Vec<(String, String, Option<String>, Option<String>)> = props
        .get("edges")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|e| {
                    Some((
                        opt_string(e, "source")?,
                        opt_string(e, "target")?,
                        opt_string(e, "color"),
                        opt_string(e, "label"),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let pairs: Vec<(String, String)> = raw_edges
        .iter()
        .map(|(s, t, ..)| (s.clone(), t.clone()))
        .collect();
    let layout = props_str(props, "layout")
        .and_then(LayoutMode::parse)
        .unwrap_or(LayoutMode::Force);
    let points = if layout == LayoutMode::Force {
        force_layout(&ids, &pairs, w, h)
    } else {
        static_layout(layout, &ids, &pairs, w, h)
    };

    let nodes: Vec<GNode> = raw_nodes
        .iter()
        .filter(|n| opt_string(n, "id").is_some())
        .zip(points)
        .map(|(n, at)| {
            let group = opt_string(n, "group");
            let color = opt_string(n, "color")
                .or_else(|| {
                    group.as_ref().and_then(|g| {
                        groups
                            .iter()
                            .position(|x| x == g)
                            .map(|i| format!("var(--color-series-{})", i % SERIES + 1))
                    })
                })
                .unwrap_or_else(|| DEFAULT_NODE_COLOR.to_string());
            GNode {
                id: opt_string(n, "id").unwrap_or_default(),
                label: opt_string(n, "label"),
                color,
                size: n
                    .get("size")
                    .and_then(Value::as_f64)
                    .map_or(DEFAULT_NODE_SIZE, |s| s as f32),
                group,
                at,
            }
        })
        .collect();
    let edges: Vec<GEdge> = raw_edges
        .into_iter()
        .filter_map(|(s, t, color, label)| {
            Some(GEdge {
                source: nodes.iter().position(|n| n.id == s)?,
                target: nodes.iter().position(|n| n.id == t)?,
                color,
                label,
            })
        })
        .collect();

    let zoom = props
        .get("zoomToFit")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let (vx, vy, vw, vh) = if zoom {
        let min_x = nodes
            .iter()
            .map(|n| n.at.x - n.size)
            .fold(f32::INFINITY, f32::min);
        let min_y = nodes
            .iter()
            .map(|n| n.at.y - n.size)
            .fold(f32::INFINITY, f32::min);
        let max_x = nodes
            .iter()
            .map(|n| n.at.x + n.size)
            .fold(f32::NEG_INFINITY, f32::max);
        let max_y = nodes
            .iter()
            .map(|n| n.at.y + n.size)
            .fold(f32::NEG_INFINITY, f32::max);
        (
            min_x - ZOOM_PAD,
            min_y - ZOOM_PAD,
            max_x - min_x + ZOOM_PAD * 2.0,
            max_y - min_y + ZOOM_PAD * 2.0,
        )
    } else {
        (0.0, 0.0, w, h)
    };
    let s_in = (w / vw).min(h / vh);
    let inner = Xf {
        s: s_in,
        ox: (w - vw * s_in) / 2.0 - vx * s_in,
        oy: (h - vh * s_in) / 2.0 - vy * s_in,
    };
    pen.xf = Xf {
        s: outer.s * inner.s,
        ox: outer.ox + outer.s * inner.ox,
        oy: outer.oy + outer.s * inner.oy,
    };

    let show_labels = props
        .get("showLabels")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let directed = layout != LayoutMode::Force;
    let label_ink = pen.tone("label");
    for e in &edges {
        let (a, b) = (&nodes[e.source], &nodes[e.target]);
        let color = with_alpha(
            pen.paint(
                Some(e.color.as_deref().unwrap_or(DEFAULT_EDGE_COLOR)),
                "grid",
            ),
            0.8,
        );
        pen.line((a.at.x, a.at.y), (b.at.x, b.at.y), &Ink::new(color, 1.5));
        if directed {
            let sw = 1.5;
            let theta = (b.at.y - a.at.y).atan2(b.at.x - a.at.x);
            let (sin, cos) = theta.sin_cos();
            let at = |mx: f32, my: f32| {
                let (dx, dy) = ((mx - 9.0) * 0.6 * sw, (my - 5.0) * 0.6 * sw);
                (b.at.x + dx * cos - dy * sin, b.at.y + dx * sin + dy * cos)
            };
            pen.polyline(
                &[at(0.0, 0.0), at(10.0, 5.0), at(0.0, 10.0)],
                true,
                Some(color),
                None,
            );
        }
        if let (true, Some(label)) = (show_labels, &e.label) {
            pen.text(
                (a.at.x + b.at.x) / 2.0,
                (a.at.y + b.at.y) / 2.0 - 6.0,
                label,
                9.0,
                with_alpha(label_ink, 0.9),
                TextAlign::Center,
                400,
            );
        }
    }

    let ink = pen.tone("ink");
    let tap_event = |name: &str| {
        props_str(props, name)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let (on_hover, on_click) = (tap_event("onNodeHover"), tap_event("onNodeClick"));
    for n in &nodes {
        pen.circle(
            n.at.x,
            n.at.y,
            n.size,
            Some(pen.paint(Some(&n.color), "label")),
            None,
        );
        if show_labels {
            pen.text(
                n.at.x,
                n.at.y + n.size + 12.0,
                n.label.as_deref().unwrap_or(&n.id),
                11.0,
                with_alpha(ink, 0.9),
                TextAlign::Center,
                400,
            );
        }
        let payload = node_payload(n);
        let emits: Vec<Emit> = [&on_hover, &on_click]
            .into_iter()
            .flatten()
            .map(|event| Emit {
                event: event.clone(),
                arg: "node".into(),
                payload: payload.clone(),
            })
            .collect();
        let r = n.size + 4.0;
        pen.hit(n.at.x - r, n.at.y - r, r * 2.0, r * 2.0, emits);
    }
    Ok(pen.finish("graph-view", pw, ph))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::DrawCommand;

    /// `learning-diagram` (mode `flow`, step 1 active): the active node carries the literal amber
    /// colour at size 12, the rest are grouped nodes at size 8.
    fn learning_diagram() -> Value {
        json!({
            "type": "graph-view", "layout": "flow", "height": 300, "showLabels": true,
            "nodes": [
                {"id": "in", "label": "Input", "group": "io", "size": 8},
                {"id": "proc", "label": "Process", "group": "core", "color": "#f59e0b", "size": 12},
                {"id": "out", "label": "Output", "group": "io", "size": 8}
            ],
            "edges": [
                {"source": "in", "target": "proc", "label": "parse"},
                {"source": "proc", "target": "out"}
            ]
        })
    }

    fn circles(scene: &MappedScene) -> Vec<([f32; 4], f32)> {
        scene
            .packet
            .draw_commands
            .iter()
            .filter_map(|c| match c {
                DrawCommand::Path {
                    segments,
                    fill: Some(f),
                    stroke: None,
                } => match segments[0] {
                    crate::packet::PathSegment::Arc { rx, .. } => Some((*f, rx)),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    #[test]
    fn draws_one_circle_per_node_in_group_series_and_literal_colours() {
        let scene = map(&learning_diagram(), None).unwrap();
        let theme = theme_for(&learning_diagram());
        let c = circles(&scene);
        let node_circles: Vec<_> = c.iter().filter(|(_, r)| *r >= 7.0).collect();
        assert_eq!(node_circles.len(), 3);
        assert_eq!(
            node_circles[0].0,
            theme.tone("series-1").unwrap(),
            "first group takes series-1"
        );
        assert_eq!(
            node_circles[1].0,
            crate::mappers::game_canvas_2d::parse_css_color("#f59e0b").unwrap(),
            "an authored literal wins over the group"
        );
        assert_eq!(
            node_circles[2].0,
            theme.tone("series-1").unwrap(),
            "the same group shares its series tone"
        );
        assert!(
            node_circles[1].1 > node_circles[0].1,
            "size 12 draws larger than size 8 (control)"
        );
    }

    #[test]
    fn a_tap_runs_hover_then_click_with_the_web_node_payload_and_a_miss_emits_nothing() {
        let mut props = learning_diagram();
        props["onNodeClick"] = json!("NODE_CLICK");
        props["onNodeHover"] = json!("NODE_HOVER");
        let scene = map(&props, None).unwrap();
        let hit = scene
            .hits
            .iter()
            .find(|h| h.emits[0].payload["id"] == "proc")
            .unwrap();
        let (x, y) = (hit.rect.x + hit.rect.w / 2.0, hit.rect.y + hit.rect.h / 2.0);
        let emits = scene.emits_at(x, y);
        let events: Vec<&str> = emits.iter().map(|e| e.event.as_str()).collect();
        assert_eq!(events, ["NODE_HOVER", "NODE_CLICK"]);
        assert_eq!(
            emits[1].delivered(),
            json!({"node": {"id": "proc", "label": "Process", "group": "core", "color": "#f59e0b", "size": 12.0}})
        );
        assert!(
            scene.emits_at(1.0, 1.0).is_empty(),
            "control: empty canvas emits nothing"
        );
        let silent = map(&learning_diagram(), None).unwrap();
        assert!(
            silent.hits.is_empty(),
            "control: no authored callback, no regions"
        );
    }

    #[test]
    fn static_layouts_draw_arrow_heads_and_force_does_not() {
        let flow = map(&learning_diagram(), None).unwrap();
        let mut force_props = learning_diagram();
        force_props["layout"] = json!("force");
        let force = map(&force_props, None).unwrap();
        let heads = |s: &MappedScene| {
            s.packet.draw_commands.iter().filter(|c| matches!(c, DrawCommand::Path { segments, stroke: None, .. } if segments.len() == 4)).count()
        };
        assert_eq!(heads(&flow), 2);
        assert_eq!(heads(&force), 0);
    }

    #[test]
    fn zoom_to_fit_changes_the_view_and_hide_labels_removes_text() {
        let base = map(&learning_diagram(), None).unwrap();
        let mut fixed = learning_diagram();
        fixed["zoomToFit"] = json!(false);
        let unzoomed = map(&fixed, None).unwrap();
        let first = |s: &MappedScene| {
            s.packet.draw_commands.iter().find_map(|c| match c {
                DrawCommand::Path {
                    segments,
                    stroke: Some(_),
                    ..
                } => Some(format!("{:?}", segments[0])),
                _ => None,
            })
        };
        assert_ne!(first(&base), first(&unzoomed));
        let texts = |s: &MappedScene| {
            s.packet
                .draw_commands
                .iter()
                .filter(|c| matches!(c, DrawCommand::Text { .. }))
                .count()
        };
        assert_eq!(texts(&base), 4, "three node labels plus the one edge label");
        let mut quiet = learning_diagram();
        quiet["showLabels"] = json!(false);
        assert_eq!(texts(&map(&quiet, None).unwrap()), 0);
    }

    #[test]
    fn no_nodes_draws_the_empty_message_and_an_unknown_edge_endpoint_is_skipped() {
        let empty = map(&json!({"type": "graph-view", "nodes": []}), None).unwrap();
        assert!(empty
            .packet
            .draw_commands
            .iter()
            .any(|c| matches!(c, DrawCommand::Text { text, .. } if text == "No graph data")));
        let mut props = learning_diagram();
        props["edges"] = json!([{"source": "in", "target": "ghost"}]);
        let scene = map(&props, None).unwrap();
        let lines = scene
            .packet
            .draw_commands
            .iter()
            .filter(|c| {
                matches!(
                    c,
                    DrawCommand::Path {
                        stroke: Some(_),
                        fill: None,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(lines, 0);
    }
}
