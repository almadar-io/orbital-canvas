//! A ```` ```mermaid ```` fence (`MermaidDiagram`, markdown / segment renderer) on the Rust
//! renderer: flowcharts (`graph` / `flowchart`) and state diagrams (`stateDiagram`), laid out by
//! [`layered`] (transposed for top-down) and drawn in the theme's mermaid variables
//! (`lib/mermaidTheme.ts`). Every other mermaid kind is refused with a declared error, so the host
//! keeps showing the fence as code.

use super::draw::{frame, Ink, Pen};
use super::layered::{self, EdgeIn, Metrics, NodeIn};
use super::state_machine::arrow_head;
use crate::mappers::game_canvas_2d::{get_string, CanvasSceneError};
use crate::mappers::learning::canvas::theme_for;
use crate::mappers::learning::gap_once;
use crate::mappers::scene::MappedScene;
use crate::packet::TextAlign;
use crate::vector::text_width;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Direction {
    TopDown,
    BottomUp,
    LeftRight,
    RightLeft,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    Rect,
    Round,
    Stadium,
    Circle,
    Diamond,
    Hexagon,
    /// `stateDiagram` `[*]` as a source.
    Start,
    /// `stateDiagram` `[*]` as a target.
    End,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Line {
    Solid,
    Dotted,
    Thick,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: String,
    pub label: String,
    pub shape: Shape,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub label: Option<String>,
    pub line: Line,
    pub arrow: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Diagram {
    pub direction: Direction,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

#[derive(Default)]
struct Builder {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
}

impl Builder {
    /// The node `id`, declared on first sight; a later declaration with a shape updates it.
    fn node(&mut self, id: &str, label: Option<String>, shape: Option<Shape>) -> usize {
        if let Some(i) = self.nodes.iter().position(|n| n.id == id) {
            if let Some(label) = label {
                self.nodes[i].label = label;
            }
            if let Some(shape) = shape {
                self.nodes[i].shape = shape;
            }
            return i;
        }
        self.nodes.push(Node {
            id: id.to_string(),
            label: label.unwrap_or_else(|| id.to_string()),
            shape: shape.unwrap_or(Shape::Rect),
        });
        self.nodes.len() - 1
    }
}

/// Parse the supported mermaid subset; `Err` names what is not supported.
pub fn parse(code: &str) -> Result<Diagram, String> {
    let mut lines = code
        .lines()
        .flat_map(|l| l.split(';'))
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("%%"));
    let header = lines.next().ok_or("empty diagram")?;
    let mut words = header.split_whitespace();
    let kind = words.next().unwrap_or_default();
    let mut b = Builder::default();
    match kind {
        "graph" | "flowchart" => {
            let direction = match words.next().unwrap_or("TD") {
                "TD" | "TB" => Direction::TopDown,
                "BT" => Direction::BottomUp,
                "LR" => Direction::LeftRight,
                "RL" => Direction::RightLeft,
                other => return Err(format!("unknown flowchart direction `{other}`")),
            };
            for line in lines {
                flow_line(&mut b, line)?;
            }
            Ok(Diagram {
                direction,
                nodes: b.nodes,
                edges: b.edges,
            })
        }
        "stateDiagram" | "stateDiagram-v2" => {
            let mut direction = Direction::TopDown;
            for line in lines {
                if let Some(d) = line.strip_prefix("direction ") {
                    direction = match d.trim() {
                        "LR" => Direction::LeftRight,
                        "RL" => Direction::RightLeft,
                        "BT" => Direction::BottomUp,
                        _ => Direction::TopDown,
                    };
                    continue;
                }
                state_line(&mut b, line)?;
            }
            Ok(Diagram {
                direction,
                nodes: b.nodes,
                edges: b.edges,
            })
        }
        other => Err(format!("mermaid `{other}` diagrams are not drawn natively")),
    }
}

const IGNORED_FLOW: [&str; 6] = [
    "classDef ",
    "class ",
    "style ",
    "linkStyle ",
    "click ",
    "accTitle",
];

fn flow_line(b: &mut Builder, line: &str) -> Result<(), String> {
    if IGNORED_FLOW.iter().any(|p| line.starts_with(p)) {
        gap_once(
            "mermaid-styling",
            "mermaid class/style/click directives are not drawn natively",
        );
        return Ok(());
    }
    if line.starts_with("subgraph") || line == "end" || line.starts_with("direction ") {
        gap_once(
            "mermaid-subgraph",
            "mermaid subgraph boxes are not drawn natively; their nodes are",
        );
        return Ok(());
    }
    let (mut sources, mut rest) = node_group(b, line)?;
    loop {
        let pending = rest.trim_start();
        if pending.is_empty() {
            return Ok(());
        }
        let (line_kind, arrow, label, next) = link(pending)?;
        let (targets, after) = node_group(b, next.trim_start())?;
        for &from in &sources {
            for &to in &targets {
                b.edges.push(Edge {
                    from,
                    to,
                    label: label.clone(),
                    line: line_kind,
                    arrow,
                });
            }
        }
        sources = targets;
        rest = after;
    }
}

/// `A`, `A[label]`, `A & B`: the nodes and the text after them.
fn node_group<'a>(b: &mut Builder, s: &'a str) -> Result<(Vec<usize>, &'a str), String> {
    let mut out = Vec::new();
    let mut rest = s;
    loop {
        let (i, after) = node_ref(b, rest.trim_start())?;
        out.push(i);
        let after = after.trim_start();
        match after.strip_prefix('&') {
            Some(more) => rest = more,
            None => return Ok((out, after)),
        }
    }
}

const SHAPES: [(&str, &str, Shape); 9] = [
    ("((", "))", Shape::Circle),
    ("([", "])", Shape::Stadium),
    ("[[", "]]", Shape::Rect),
    ("[(", ")]", Shape::Rect),
    ("{{", "}}", Shape::Hexagon),
    ("[", "]", Shape::Rect),
    ("(", ")", Shape::Round),
    ("{", "}", Shape::Diamond),
    (">", "]", Shape::Rect),
];

/// One node reference: an id, then an optional shape bracket with its label.
fn node_ref<'a>(b: &mut Builder, s: &'a str) -> Result<(usize, &'a str), String> {
    let id_len = s
        .char_indices()
        .find(|(_, c)| !(c.is_alphanumeric() || *c == '_'))
        .map_or(s.len(), |(i, _)| i);
    if id_len == 0 {
        return Err(format!("expected a node id at `{s}`"));
    }
    let (id, after) = s.split_at(id_len);
    for (open, close, shape) in SHAPES {
        if let Some(inner) = after.strip_prefix(open) {
            let end = inner
                .find(close)
                .ok_or_else(|| format!("unclosed `{open}` after `{id}`"))?;
            let label = unquote(inner[..end].trim());
            return Ok((
                b.node(id, Some(label), Some(shape)),
                &inner[end + close.len()..],
            ));
        }
    }
    Ok((b.node(id, None, None), after))
}

fn unquote(s: &str) -> String {
    s.strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .unwrap_or(s)
        .to_string()
}

/// A link and the text after it: `-->`, `---`, `-.->`, `==>`, `-->|label|`, `-- label -->`.
fn link(s: &str) -> Result<(Line, bool, Option<String>, &str), String> {
    const OPS: [(&str, Line, bool); 8] = [
        ("<-->", Line::Solid, true),
        ("-.->", Line::Dotted, true),
        ("-.-", Line::Dotted, false),
        ("==>", Line::Thick, true),
        ("===", Line::Thick, false),
        ("-->", Line::Solid, true),
        ("---", Line::Solid, false),
        ("--o", Line::Solid, false),
    ];
    for (op, line, arrow) in OPS {
        if let Some(after) = s.strip_prefix(op) {
            let after = after.trim_start();
            if let Some(piped) = after.strip_prefix('|') {
                let end = piped.find('|').ok_or("unclosed `|` link label")?;
                return Ok((
                    line,
                    arrow,
                    Some(unquote(piped[..end].trim())),
                    &piped[end + 1..],
                ));
            }
            return Ok((line, arrow, None, after));
        }
    }
    for (open, close, line) in [
        ("--", "-->", Line::Solid),
        ("-.", ".->", Line::Dotted),
        ("==", "==>", Line::Thick),
    ] {
        if let Some(after) = s.strip_prefix(open) {
            if let Some(end) = after.find(close) {
                let label = unquote(after[..end].trim());
                return Ok((line, true, Some(label), &after[end + close.len()..]));
            }
        }
    }
    Err(format!("expected a link at `{s}`"))
}

fn state_line(b: &mut Builder, line: &str) -> Result<(), String> {
    if let Some(decl) = line.strip_prefix("state ") {
        if let Some((name, id)) = decl.split_once(" as ") {
            b.node(id.trim(), Some(unquote(name.trim())), Some(Shape::Round));
            return Ok(());
        }
        gap_once(
            "mermaid-state-composite",
            "composite and fork/join states are not drawn natively",
        );
        return Ok(());
    }
    if line == "}" || line.starts_with("note ") || line == "end note" {
        return Ok(());
    }
    let Some((lhs, rhs)) = line.split_once("-->") else {
        if let Some((id, _description)) = line.split_once(':') {
            b.node(id.trim(), None, Some(Shape::Round));
        } else {
            b.node(line, None, Some(Shape::Round));
        }
        return Ok(());
    };
    let (rhs, label) = match rhs.split_once(':') {
        Some((target, label)) => (target, Some(label.trim().to_string())),
        None => (rhs, None),
    };
    let state = |b: &mut Builder, name: &str, start: bool| {
        let name = name.trim();
        if name == "[*]" {
            let (id, shape) = if start {
                ("[*]start", Shape::Start)
            } else {
                ("[*]end", Shape::End)
            };
            b.node(id, Some(String::new()), Some(shape))
        } else {
            let existing = b.nodes.iter().position(|n| n.id == name);
            existing.unwrap_or_else(|| b.node(name, None, Some(Shape::Round)))
        }
    };
    let from = state(b, lhs, true);
    let to = state(b, rhs, false);
    b.edges.push(Edge {
        from,
        to,
        label,
        line: Line::Solid,
        arrow: true,
    });
    Ok(())
}

const FONT_PX: f32 = 14.0;
const PAD_X: f32 = 15.0;
const PAD_Y: f32 = 10.0;

fn node_size(n: &Node) -> (f32, f32) {
    let w = text_width(&n.label, FONT_PX) + PAD_X * 2.0;
    let h = FONT_PX * 1.4 + PAD_Y * 2.0;
    match n.shape {
        Shape::Start | Shape::End => (14.0, 14.0),
        Shape::Circle => {
            let d = w.max(h);
            (d, d)
        }
        Shape::Diamond => (w * 1.5, h * 1.6),
        Shape::Hexagon => (w + h / 2.0, h),
        _ => (w, h),
    }
}

/// Map a `mermaid` scene (`{ code }`).
pub fn map(props: &Value, viewport: Option<(f32, f32)>) -> Result<MappedScene, CanvasSceneError> {
    let code = get_string(props, "code").unwrap_or_default();
    let d = parse(&code).map_err(CanvasSceneError::UnsupportedMode)?;
    if d.nodes.is_empty() {
        return Err(CanvasSceneError::NothingToDraw("mermaid".into()));
    }
    let vertical = matches!(d.direction, Direction::TopDown | Direction::BottomUp);
    let sizes: Vec<(f32, f32)> = d.nodes.iter().map(node_size).collect();
    let nodes_in: Vec<NodeIn> = sizes
        .iter()
        .map(|&(w, h)| {
            let (w, h) = if vertical { (h, w) } else { (w, h) };
            NodeIn {
                w,
                h,
                initial: false,
            }
        })
        .collect();
    let edges_in: Vec<EdgeIn> = d
        .edges
        .iter()
        .map(|e| {
            let lw = e
                .label
                .as_deref()
                .map_or(0.0, |l| text_width(l, FONT_PX) + 8.0);
            let lh = if e.label.is_some() {
                FONT_PX * 1.4
            } else {
                0.0
            };
            let (label_w, label_h) = if vertical { (lh, lw) } else { (lw, lh) };
            EdgeIn {
                from: e.from,
                to: e.to,
                label_w,
                label_h,
            }
        })
        .collect();
    let layout = layered::layout(
        &nodes_in,
        &edges_in,
        Metrics {
            node_spacing: 40.0,
            layer_spacing: 60.0,
            rtl: matches!(d.direction, Direction::RightLeft),
        },
    );
    let flip_y = matches!(d.direction, Direction::BottomUp);
    let (lw, lh) = if vertical {
        (layout.height, layout.width)
    } else {
        (layout.width, layout.height)
    };
    let place = |x: f32, y: f32| -> (f32, f32) {
        let (px, py) = if vertical { (y, x) } else { (x, y) };
        (px + 20.0, if flip_y { lh - py + 20.0 } else { py + 20.0 })
    };
    let (width, height) = (lw + 40.0, lh + 40.0);
    let theme = theme_for(props);
    let (xf, pw, ph) = frame(width, height, viewport);
    let mut pen = Pen::new(&theme, xf);
    let ground = pen.ground();
    pen.clear(ground);
    let (fill, ink, label, line) = (
        pen.tone("fill"),
        pen.tone("ink"),
        pen.tone("label"),
        pen.tone("axis"),
    );

    for (e, out) in d.edges.iter().zip(&layout.edges) {
        let pts: Vec<(f32, f32)> = out.points.iter().map(|&(x, y)| place(x, y)).collect();
        let width = if e.line == Line::Thick { 3.0 } else { 1.5 };
        let mut stroke = Ink::new(line, width);
        if e.line == Line::Dotted {
            stroke = stroke.dashed(&[3.0, 3.0]);
        }
        pen.polyline(&pts, false, None, Some(&stroke));
        if e.arrow && pts.len() >= 2 {
            arrow_head(
                &mut pen,
                pts[pts.len() - 2],
                pts[pts.len() - 1],
                8.0,
                width.max(1.5),
                line,
            );
        }
        if let Some(text) = &e.label {
            let r = out.label;
            let (cx, cy) = place(r.x + r.w / 2.0, r.y + r.h / 2.0);
            let tw = text_width(text, FONT_PX) + 8.0;
            pen.rrect(
                cx - tw / 2.0,
                cy - FONT_PX * 0.7,
                tw,
                FONT_PX * 1.4,
                2.0,
                Some(ground),
                None,
            );
            pen.text_middle(cx, cy, text, FONT_PX, label, TextAlign::Center, 400);
        }
    }

    for ((n, r), &(w, h)) in d.nodes.iter().zip(&layout.nodes).zip(&sizes) {
        let (cx, cy) = place(r.x + r.w / 2.0, r.y + r.h / 2.0);
        let (x, y) = (cx - w / 2.0, cy - h / 2.0);
        let border = Ink::new(ink, 1.0);
        match n.shape {
            Shape::Rect => pen.rrect(x, y, w, h, 0.0, Some(fill), Some(&border)),
            Shape::Round => pen.rrect(x, y, w, h, 5.0, Some(fill), Some(&border)),
            Shape::Stadium => pen.rrect(x, y, w, h, h / 2.0, Some(fill), Some(&border)),
            Shape::Circle => pen.circle(cx, cy, w / 2.0, Some(fill), Some(&border)),
            Shape::Diamond => pen.polyline(
                &[(cx, y), (x + w, cy), (cx, y + h), (x, cy)],
                true,
                Some(fill),
                Some(&border),
            ),
            Shape::Hexagon => {
                let k = h / 4.0;
                pen.polyline(
                    &[
                        (x + k, y),
                        (x + w - k, y),
                        (x + w, cy),
                        (x + w - k, y + h),
                        (x + k, y + h),
                        (x, cy),
                    ],
                    true,
                    Some(fill),
                    Some(&border),
                );
            }
            Shape::Start => pen.circle(cx, cy, 7.0, Some(ink), None),
            Shape::End => {
                pen.circle(cx, cy, 7.0, None, Some(&Ink::new(ink, 1.5)));
                pen.circle(cx, cy, 4.0, Some(ink), None);
            }
        }
        if !matches!(n.shape, Shape::Start | Shape::End) {
            pen.text_middle(cx, cy, &n.label, FONT_PX, label, TextAlign::Center, 400);
        }
    }
    Ok(pen.finish("mermaid", pw, ph))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::DrawCommand;
    use serde_json::json;

    fn texts(s: &MappedScene) -> Vec<String> {
        s.packet
            .draw_commands
            .iter()
            .filter_map(|c| match c {
                DrawCommand::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_flowchart_parses_shapes_labels_chains_and_fan_out() {
        let d = parse(
            "graph TD\n  A[Start] --> B{Ok?}\n  B -->|yes| C((Done)) --> D\n  B -- no --> E & F",
        )
        .unwrap();
        assert_eq!(d.direction, Direction::TopDown);
        let ids: Vec<&str> = d.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["A", "B", "C", "D", "E", "F"]);
        assert_eq!(d.nodes[1].shape, Shape::Diamond);
        assert_eq!(d.nodes[2].shape, Shape::Circle);
        assert_eq!(d.nodes[2].label, "Done");
        let labels: Vec<Option<&str>> = d.edges.iter().map(|e| e.label.as_deref()).collect();
        assert_eq!(
            labels,
            vec![None, Some("yes"), None, Some("no"), Some("no")]
        );
    }

    #[test]
    fn link_styles_are_read() {
        let d = parse("flowchart LR\n A -.-> B\n B ==> C\n C --- D").unwrap();
        let kinds: Vec<(Line, bool)> = d.edges.iter().map(|e| (e.line, e.arrow)).collect();
        assert_eq!(
            kinds,
            vec![
                (Line::Dotted, true),
                (Line::Thick, true),
                (Line::Solid, false)
            ]
        );
    }

    #[test]
    fn a_state_diagram_has_start_and_end_marks_and_transition_labels() {
        let d =
            parse("stateDiagram-v2\n [*] --> Idle\n Idle --> Running : start\n Running --> [*]")
                .unwrap();
        assert_eq!(d.nodes.first().map(|n| n.shape), Some(Shape::Start));
        assert!(d.nodes.iter().any(|n| n.shape == Shape::End));
        assert_eq!(d.edges[1].label.as_deref(), Some("start"));
    }

    #[test]
    fn control_other_diagram_kinds_and_broken_syntax_are_refused() {
        assert!(parse("sequenceDiagram\n A->>B: hi")
            .unwrap_err()
            .contains("sequenceDiagram"));
        assert!(parse("graph TD\n A[open --> B").is_err());
        assert!(parse("").is_err());
        assert!(matches!(
            map(&json!({"code": "pie\n \"a\": 1"}), None),
            Err(CanvasSceneError::UnsupportedMode(_))
        ));
    }

    #[test]
    fn top_down_stacks_and_left_right_lines_up() {
        let pos = |code: &str| {
            let s = map(&json!({"code": code}), None).unwrap();
            let at = |label: &str| {
                s.packet.draw_commands.iter().find_map(|c| match c {
                    DrawCommand::Text { text, pos, .. } if text == label => Some((pos.x, pos.y)),
                    _ => None,
                })
            };
            (at("One").unwrap(), at("Two").unwrap())
        };
        let (a, b) = pos("graph TD\n A[One] --> B[Two]");
        assert!(
            b.1 > a.1 + 20.0 && (b.0 - a.0).abs() < 1.0,
            "TD: {a:?} {b:?}"
        );
        let (a, b) = pos("graph LR\n A[One] --> B[Two]");
        assert!(
            b.0 > a.0 + 20.0 && (b.1 - a.1).abs() < 1.0,
            "LR: {a:?} {b:?}"
        );
        let (a, b) = pos("graph BT\n A[One] --> B[Two]");
        assert!(b.1 < a.1 - 20.0, "BT: {a:?} {b:?}");
    }

    #[test]
    fn the_scene_draws_every_label_once() {
        let s = map(
            &json!({"code": "graph TD\n A[Alpha] -->|go| B[Beta]"}),
            None,
        )
        .unwrap();
        let t = texts(&s);
        for label in ["Alpha", "Beta", "go"] {
            assert_eq!(
                t.iter().filter(|x| *x == label).count(),
                1,
                "{label}: {t:?}"
            );
        }
    }
}
