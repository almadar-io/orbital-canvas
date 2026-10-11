//! `biology-canvas` — port of the 2D path of `BiologyCanvas.tsx`: bands,
//! compartments, a DNA helix ladder, edges, nodes and stage chips derived into
//! `LearningShape`s in canvas pixels, drawn by [`canvas`]. `mode: 3d` is a
//! three.js scene and is refused.

use super::canvas::{self, CanvasSpec};
use super::gap_once;
use super::shape::{parse_shapes, Align, Dash, Shape, ShapeType};
use crate::mappers::game_canvas_2d::{get_f32, CanvasSceneError};
use crate::mappers::scene::MappedScene;
use serde_json::Value;

const NODE_RADIUS: f32 = 16.0;

fn list<'a>(props: &'a Value, key: &str) -> &'a [Value] {
    props
        .get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn num(v: &Value, key: &str) -> Option<f32> {
    v.get(key).and_then(Value::as_f64).map(|n| n as f32)
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

/// JS truthiness of an optional label: absent and empty draw nothing.
fn label(v: &Value, key: &str) -> Option<String> {
    text(v, key).filter(|s| !s.is_empty())
}

fn series_tone(i: usize) -> String {
    super::theme::DiagramTheme::series_tone(i)
}

fn stage_fill(state: &str) -> &'static str {
    match state {
        "active" => "primary",
        "done" => "muted",
        _ => "fill",
    }
}

fn stage_text(state: &str) -> &'static str {
    match state {
        "active" | "done" => "ink",
        _ => "label",
    }
}

fn unit(dx: f32, dy: f32) -> (f32, f32) {
    let dist = dx.hypot(dy).max(1e-6);
    (dx / dist, dy / dist)
}

fn push_bands(props: &Value, width: f32, height: f32, out: &mut Vec<Shape>) {
    let bands = list(props, "bands");
    let count = bands.len() as f32;
    for (i, band) in bands.iter().enumerate() {
        let color = text(band, "color").unwrap_or_else(|| series_tone(i));
        let y = i as f32 * height / count;
        out.push(
            Shape::rect(0.0, y, width, height / count)
                .color(color.clone())
                .fill(color)
                .fill_style("tint")
                .stroke_weight("thin")
                .opacity(0.45),
        );
        if let Some(l) = label(band, "label") {
            out.push(Shape::text(8.0, y + 14.0, l).tone("label").text_size("xs"));
        }
    }
}

fn push_compartments(props: &Value, out: &mut Vec<Shape>) {
    for c in list(props, "compartments") {
        let color = text(c, "color").unwrap_or_else(|| "ink".into());
        let (x, y) = (num(c, "x").unwrap_or(0.0), num(c, "y").unwrap_or(0.0));
        let (w, h) = (
            num(c, "width").unwrap_or(0.0),
            num(c, "height").unwrap_or(0.0),
        );
        let mut ellipse = Shape::ellipse(x, y, w, h)
            .color(color.clone())
            .fill(text(c, "fill").unwrap_or(color))
            .fill_style("tint");
        match num(c, "lineWidth") {
            Some(lw) => ellipse.line_width = Some(lw),
            None => ellipse = ellipse.stroke_weight("normal"),
        }
        ellipse.dash = match c.get("dash").and_then(Value::as_str) {
            Some("dashed") => Some(Dash::Dashed),
            Some("dotted") => Some(Dash::Dotted),
            _ => None,
        };
        out.push(ellipse);
        if let Some(l) = label(c, "label") {
            out.push(
                Shape::text(x, y - h / 2.0 + 14.0, l)
                    .tone("ink")
                    .text_size("sm")
                    .align(Align::Center),
            );
        }
    }
}

fn push_helix(props: &Value, width: f32, height: f32, out: &mut Vec<Shape>) {
    let Some(helix) = props.get("helix").filter(|h| h.is_object()) else {
        return;
    };
    let hx = num(helix, "x").unwrap_or(24.0);
    let hy = num(helix, "y").unwrap_or(height * 0.25);
    let hw = num(helix, "width").unwrap_or(width - 48.0);
    let hh = num(helix, "height").unwrap_or(height * 0.5);
    let rungs = list(helix, "rungs");
    let n = rungs.len();
    let cy = hy + hh / 2.0;
    let color_a = text(helix, "colorA").unwrap_or_else(|| "series-1".into());
    let color_b = text(helix, "colorB").unwrap_or_else(|| "series-2".into());
    let rung_color = text(helix, "rungColor").unwrap_or_else(|| "muted".into());
    let fork = num(helix, "fork").unwrap_or(0.0);
    let max_sep = (hh - 8.0).min(96.0);

    struct Geom<'a> {
        rx: f32,
        sep: f32,
        rung: &'a Value,
        paired: bool,
    }
    let geoms: Vec<Geom> = rungs
        .iter()
        .enumerate()
        .map(|(i, rung)| {
            let rx = hx + (i as f32 + 0.5) * hw / n as f32;
            let t = (i as f32 + 0.5) / n as f32;
            let paired = t >= fork;
            let sep = if paired {
                28.0
            } else {
                28.0 + (max_sep - 28.0) * ((fork - t) / fork)
            };
            Geom {
                rx,
                sep,
                rung,
                paired,
            }
        })
        .collect();

    for (color, dir) in [(&color_a, -1.0), (&color_b, 1.0)] {
        for w in geoms.windows(2) {
            let (p, q) = (&w[0], &w[1]);
            out.push(
                Shape::line(p.rx, cy + dir * p.sep / 2.0, q.rx, cy + dir * q.sep / 2.0)
                    .color(color.clone())
                    .stroke_weight("bold"),
            );
        }
    }
    let base = |x: f32, y: f32, s: String| {
        Shape::text(x, y, s)
            .text_size("xs")
            .align(Align::Center)
            .tone("label")
    };
    for g in &geoms {
        let r_color = text(g.rung, "color").unwrap_or_else(|| {
            if text(g.rung, "state").as_deref() == Some("new") {
                "accent".into()
            } else {
                rung_color.clone()
            }
        });
        let top = cy - g.sep / 2.0;
        let bottom = cy + g.sep / 2.0;
        let (a, b) = (label(g.rung, "a"), label(g.rung, "b"));
        if g.paired {
            out.push(
                Shape::line(g.rx, top, g.rx, bottom)
                    .color(r_color)
                    .stroke_weight("thin"),
            );
            if let Some(a) = a {
                out.push(base(g.rx, cy - g.sep / 4.0, a));
            }
            if let Some(b) = b {
                out.push(base(g.rx, cy + g.sep / 4.0, b));
            }
        } else {
            let stub_top = top + 8.0;
            let stub_bottom = bottom - 8.0;
            out.push(
                Shape::line(g.rx, top, g.rx, stub_top)
                    .color(r_color.clone())
                    .stroke_weight("thin"),
            );
            out.push(
                Shape::line(g.rx, bottom, g.rx, stub_bottom)
                    .color(r_color)
                    .stroke_weight("thin"),
            );
            if let Some(a) = a {
                out.push(base(g.rx, stub_top + 6.0, a));
            }
            if let Some(b) = b {
                out.push(base(g.rx, stub_bottom - 6.0, b));
            }
        }
    }
}

fn push_edges(props: &Value, out: &mut Vec<Shape>) {
    let nodes = list(props, "nodes");
    let find = |id: &str| nodes.iter().find(|n| text(n, "id").as_deref() == Some(id));
    let xy = |n: &Value| (num(n, "x").unwrap_or(0.0), num(n, "y").unwrap_or(0.0));
    for e in list(props, "edges") {
        let (Some(a), Some(b)) = (
            text(e, "from").and_then(|id| find(&id)),
            text(e, "to").and_then(|id| find(&id)),
        ) else {
            continue;
        };
        let ((ax, ay), (bx, by)) = (xy(a), xy(b));
        let color = text(e, "color").unwrap_or_else(|| "ink".into());
        if e.get("directed").and_then(Value::as_bool).unwrap_or(false) {
            let (ra, rb) = (
                num(a, "radius").unwrap_or(NODE_RADIUS),
                num(b, "radius").unwrap_or(NODE_RADIUS),
            );
            let (ux, uy) = unit(bx - ax, by - ay);
            out.push(
                Shape::arrow(ax + ux * ra, ay + uy * ra, bx - ux * rb, by - uy * rb)
                    .color(color)
                    .stroke_weight("normal"),
            );
        } else {
            out.push(
                Shape::line(ax, ay, bx, by)
                    .color(color)
                    .stroke_weight("normal"),
            );
        }
        if let Some(l) = label(e, "label") {
            out.push(
                Shape::text((ax + bx) / 2.0 + 4.0, (ay + by) / 2.0 - 4.0, l)
                    .tone("label")
                    .text_size("sm"),
            );
        }
    }
}

fn push_nodes(props: &Value, out: &mut Vec<Shape>) {
    for (ni, n) in list(props, "nodes").iter().enumerate() {
        let state = text(n, "state").unwrap_or_else(|| "default".into());
        let muted = state == "muted";
        let color = text(n, "color").unwrap_or_else(|| series_tone(ni));
        let (x, y) = (num(n, "x").unwrap_or(0.0), num(n, "y").unwrap_or(0.0));
        let radius = num(n, "radius").unwrap_or(NODE_RADIUS);
        let mut circle = Shape::circle(x, y, radius)
            .color(color.clone())
            .fill(color)
            .fill_style("tint")
            .stroke_weight("normal");
        circle.id = text(n, "id");
        if muted {
            circle = circle.opacity(0.35);
        }
        out.push(circle);
        if state == "highlight" {
            out.push(
                Shape::circle(x, y, radius + 4.0)
                    .tone("highlight")
                    .stroke_weight("normal"),
            );
        }
        if let Some(l) = label(n, "label") {
            let mut t = Shape::text(x, y + radius + 14.0, l)
                .tone("ink")
                .text_size("sm")
                .align(Align::Center);
            if muted {
                t = t.opacity(0.35);
            }
            out.push(t);
        }
    }
}

/// JS `string.length` (UTF-16 code units).
fn js_len(s: &str) -> usize {
    s.encode_utf16().count()
}

fn push_stages(props: &Value, width: f32, height: f32, out: &mut Vec<Shape>) {
    let stages = list(props, "stages");
    let count = stages.len();
    if count == 0 {
        return;
    }
    let state_of = |s: &Value| text(s, "state").unwrap_or_else(|| "pending".into());
    let fill_of =
        |s: &Value, state: &str| text(s, "color").unwrap_or_else(|| stage_fill(state).into());
    let stage_label = |s: &Value| text(s, "label").unwrap_or_default();

    if props.get("stageStyle").and_then(Value::as_str) == Some("ring") {
        let (cx, cy) = (width / 2.0, height / 2.0);
        let r = width.min(height) / 2.0 - 48.0;
        let points: Vec<(f32, f32)> = (0..count)
            .map(|i| {
                let a = (-90.0 + 360.0 * i as f32 / count as f32).to_radians();
                (cx + r * a.cos(), cy + r * a.sin())
            })
            .collect();
        for w in points.windows(2) {
            let ((x1, y1), (x2, y2)) = (w[0], w[1]);
            let (ux, uy) = unit(x2 - x1, y2 - y1);
            out.push(
                Shape::arrow(
                    x1 + ux * 46.0,
                    y1 + uy * 46.0,
                    x2 - ux * 46.0,
                    y2 - uy * 46.0,
                )
                .tone("guide")
                .stroke_weight("normal"),
            );
        }
        for (stage, (px, py)) in stages.iter().zip(&points) {
            let state = state_of(stage);
            let fill = fill_of(stage, &state);
            let lab = stage_label(stage);
            let w = (js_len(&lab) as f32 * 6.0 + 10.0).min(84.0).max(26.0);
            let h = 18.0;
            out.push(
                Shape::rect(px - w / 2.0, py - h / 2.0, w, h)
                    .color(fill.clone())
                    .fill(fill)
                    .fill_style("tint")
                    .stroke_weight("normal"),
            );
            out.push(
                Shape::text(*px, *py, lab)
                    .tone(stage_text(&state))
                    .text_size("xs")
                    .align(Align::Center),
            );
        }
    } else {
        let strip_y = height - 32.0;
        let slot_w = (width - 16.0) / count as f32;
        let chips: Vec<(f32, f32)> = (0..count)
            .map(|i| (8.0 + i as f32 * slot_w + 5.0, slot_w - 10.0))
            .collect();
        let mid_y = strip_y + 13.0;
        for w in chips.windows(2) {
            out.push(
                Shape::arrow(w[0].0 + w[0].1, mid_y, w[1].0, mid_y)
                    .tone("guide")
                    .stroke_weight("normal"),
            );
        }
        for (stage, (x, w)) in stages.iter().zip(&chips) {
            let state = state_of(stage);
            let fill = fill_of(stage, &state);
            out.push(
                Shape::rect(*x, strip_y, *w, 26.0)
                    .color(fill.clone())
                    .fill(fill)
                    .fill_style("tint")
                    .stroke_weight("normal"),
            );
            out.push(
                Shape::text(x + w / 2.0, strip_y + 13.0, stage_label(stage))
                    .tone(stage_text(&state))
                    .text_size("xs")
                    .align(Align::Center),
            );
        }
    }
}

/// The shapes `BiologyCanvas` derives in 2D, in its paint order.
pub fn derive_shapes(props: &Value, width: f32, height: f32) -> Vec<Shape> {
    let mut out = Vec::new();
    push_bands(props, width, height, &mut out);
    push_compartments(props, &mut out);
    push_helix(props, width, height, &mut out);
    push_edges(props, &mut out);
    push_nodes(props, &mut out);
    push_stages(props, width, height, &mut out);
    out.extend(parse_shapes(props.get("shapes")));
    debug_assert!(out.iter().all(|s| s.ty != ShapeType::VennRegion));
    out
}

pub fn map(props: &Value, viewport: Option<(f32, f32)>) -> Result<MappedScene, CanvasSceneError> {
    if props.get("mode").and_then(Value::as_str) == Some("3d") {
        return Err(CanvasSceneError::UnsupportedMode("3d".into()));
    }
    let width = get_f32(props, "width", 600.0).max(1.0);
    let height = get_f32(props, "height", 400.0).max(1.0);
    if props
        .get("title")
        .and_then(Value::as_str)
        .is_some_and(|t| !t.is_empty())
    {
        gap_once(
            "biology-title",
            "the canvas card title (Typography h4) is not painted natively",
        );
    }
    let shapes = derive_shapes(props, width, height);
    let spec = CanvasSpec::from_props("biology-canvas", props, 600.0, 400.0, shapes);
    Ok(canvas::render(&spec, &canvas::theme_for(props), viewport))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mappers::learning::theme::{DiagramTheme, Weight};
    use crate::packet::DrawCommand;
    use serde_json::json;

    fn close(a: Option<f32>, b: f32) -> bool {
        a.is_some_and(|a| (a - b).abs() < 0.01)
    }

    fn of(shapes: &[Shape], ty: ShapeType) -> Vec<&Shape> {
        shapes.iter().filter(|s| s.ty == ty).collect()
    }

    /// `std-bio-foodweb` at its config defaults (600x400): five species become
    /// series circles sized by `clamp(population*0.4, 6, 55)` with "Label (pop)"
    /// captions, `eats` edges are directed guide arrows, four trophic bands.
    fn foodweb() -> Value {
        let species = [
            ("grass", 300, 300, "series-1", "Grass", 120),
            ("rabbit", 180, 230, "series-2", "Rabbit", 40),
            ("insect", 420, 230, "series-3", "Insect", 55),
            ("fox", 220, 110, "series-4", "Fox", 18),
            ("hawk", 400, 80, "series-5", "Hawk", 10),
        ];
        let nodes: Vec<Value> = species
            .iter()
            .map(|(id, x, y, c, l, p)| {
                let r = (*p as f64 * 0.4).clamp(6.0, 55.0);
                json!({"id": id, "x": x, "y": y, "radius": r, "color": c, "label": format!("{l} ({p})"), "kind": "organism"})
            })
            .collect();
        let edges: Vec<Value> = [("rabbit", "grass"), ("insect", "grass"), ("fox", "rabbit"), ("fox", "insect"), ("hawk", "fox")]
            .iter()
            .map(|(p, q)| json!({"from": p, "to": q, "directed": true, "color": "guide", "label": "eats"}))
            .collect();
        json!({
            "type": "biology-canvas", "width": 600, "height": 400, "backgroundColor": "",
            "nodes": nodes, "edges": edges,
            "bands": [{"label": "Producers"}, {"label": "Herbivores"}, {"label": "Carnivores"}, {"label": "Apex"}],
            "traces": [{"series": [{"label": "Grass", "samples": [{"x": 0, "y": 120}]}]}]
        })
    }

    #[test]
    fn foodweb_bands_stack_in_series_tones_with_labels_drawn_first() {
        let shapes = derive_shapes(&foodweb(), 600.0, 400.0);
        let bands = of(&shapes[..8], ShapeType::Rect);
        assert_eq!(bands.len(), 4);
        for (i, b) in bands.iter().enumerate() {
            assert_eq!(b.y, Some(i as f32 * 100.0));
            assert_eq!(
                (b.x, b.width, b.height),
                (Some(0.0), Some(600.0), Some(100.0))
            );
            assert_eq!(
                b.color.as_deref(),
                Some(format!("series-{}", i + 1).as_str())
            );
            assert_eq!(b.fill_style.as_deref(), Some("tint"));
            assert_eq!(b.stroke.as_deref(), Some("thin"));
            assert_eq!(b.opacity, Some(0.45));
        }
        let first_label = &shapes[1];
        assert_eq!(first_label.text.as_deref(), Some("Producers"));
        assert_eq!((first_label.x, first_label.y), (Some(8.0), Some(14.0)));
        assert_eq!(first_label.tone.as_deref(), Some("label"));
        assert_eq!(first_label.text_size.as_deref(), Some("xs"));
    }

    #[test]
    fn foodweb_without_bands_has_no_background_rects() {
        let mut props = foodweb();
        props["bands"] = json!([]);
        let shapes = derive_shapes(&props, 600.0, 400.0);
        assert!(
            of(&shapes, ShapeType::Rect).is_empty(),
            "control: bands are the only rects"
        );
    }

    #[test]
    fn foodweb_band_color_overrides_the_cycled_series_tone() {
        let mut props = foodweb();
        props["bands"] = json!([{"color": "#336699"}, {}]);
        let shapes = derive_shapes(&props, 600.0, 400.0);
        assert_eq!(shapes[0].color.as_deref(), Some("#336699"));
        assert_eq!(shapes[1].color.as_deref(), Some("series-2"));
        assert_eq!(shapes[1].y, Some(200.0), "two bands split the 400px height");
    }

    #[test]
    fn foodweb_nodes_are_tinted_circles_with_centered_labels_below() {
        let shapes = derive_shapes(&foodweb(), 600.0, 400.0);
        let circles = of(&shapes, ShapeType::Circle);
        assert_eq!(circles.len(), 5);
        let grass = circles[0];
        assert_eq!(
            (grass.x, grass.y, grass.radius),
            (Some(300.0), Some(300.0), Some(48.0))
        );
        assert_eq!(grass.id.as_deref(), Some("grass"));
        assert_eq!(grass.fill.as_deref(), Some("series-1"));
        assert_eq!(grass.fill_style.as_deref(), Some("tint"));
        assert_eq!(grass.stroke.as_deref(), Some("normal"));
        assert_eq!(grass.opacity, None, "default state is opaque");
        let label = shapes
            .iter()
            .find(|s| s.text.as_deref() == Some("Hawk (10)"))
            .unwrap();
        assert_eq!((label.x, label.y), (Some(400.0), Some(80.0 + 6.0 + 14.0)));
        assert_eq!(label.align, Some(Align::Center));
        assert_eq!(label.tone.as_deref(), Some("ink"));
        assert_eq!(label.text_size.as_deref(), Some("sm"));
    }

    #[test]
    fn node_without_a_color_cycles_the_series_by_index_and_radius_defaults_to_16() {
        let props = json!({"nodes": [{"x": 10, "y": 10}, {"x": 50, "y": 10, "label": ""}]});
        let shapes = derive_shapes(&props, 600.0, 400.0);
        assert_eq!(shapes.len(), 2, "an empty label draws no text");
        assert_eq!(shapes[0].color.as_deref(), Some("series-1"));
        assert_eq!(shapes[1].color.as_deref(), Some("series-2"));
        assert_eq!(shapes[0].radius, Some(16.0));
        assert_eq!(shapes[0].id, None);
    }

    #[test]
    fn foodweb_directed_edge_is_a_rim_shortened_guide_arrow_with_a_midpoint_label() {
        let shapes = derive_shapes(&foodweb(), 600.0, 400.0);
        let arrows = of(&shapes, ShapeType::Arrow);
        assert_eq!(arrows.len(), 5);
        let a = arrows[0];
        let (dx, dy) = (120.0_f32, 70.0_f32);
        let d = dx.hypot(dy);
        assert!(
            close(a.x1, 180.0 + dx / d * 16.0) && close(a.y1, 230.0 + dy / d * 16.0),
            "starts on the rabbit rim (r=16)"
        );
        assert!(
            close(a.x2, 300.0 - dx / d * 48.0) && close(a.y2, 300.0 - dy / d * 48.0),
            "ends on the grass rim (r=48)"
        );
        assert_eq!(a.color.as_deref(), Some("guide"));
        assert_eq!(a.stroke.as_deref(), Some("normal"));
        let eats = shapes
            .iter()
            .find(|s| s.text.as_deref() == Some("eats"))
            .unwrap();
        assert_eq!((eats.x, eats.y), (Some(244.0), Some(261.0)));
        assert_eq!(eats.tone.as_deref(), Some("label"));
    }

    #[test]
    fn an_undirected_edge_is_a_center_to_center_line_in_ink_and_a_dangling_edge_is_skipped() {
        let mut props = foodweb();
        props["edges"] =
            json!([{"from": "rabbit", "to": "grass"}, {"from": "rabbit", "to": "nobody"}]);
        let shapes = derive_shapes(&props, 600.0, 400.0);
        assert!(of(&shapes, ShapeType::Arrow).is_empty());
        let lines = of(&shapes, ShapeType::Line);
        assert_eq!(lines.len(), 1, "the edge naming an unknown node is skipped");
        let l = lines[0];
        assert_eq!(
            (l.x1, l.y1, l.x2, l.y2),
            (Some(180.0), Some(230.0), Some(300.0), Some(300.0))
        );
        assert_eq!(l.color.as_deref(), Some("ink"));
    }

    #[test]
    fn the_packet_draws_the_guide_arrow_stroke_and_reads_traces_from_the_fixture() {
        let scene = map(&foodweb(), None).unwrap();
        let theme = DiagramTheme::heritage(false);
        let guide = theme.tone("guide").unwrap();
        let guide_strokes = scene
            .packet
            .draw_commands
            .iter()
            .filter(|c| matches!(c, DrawCommand::Path { stroke: Some(s), fill: None, .. } if s.color == guide && s.width == theme.stroke_width(Weight::Normal)))
            .count();
        assert!(guide_strokes >= 5, "five arrow shafts: {guide_strokes}");
        let mut bare = foodweb();
        bare["traces"] = json!([]);
        assert!(
            scene.packet.draw_commands.len() > map(&bare, None).unwrap().packet.draw_commands.len(),
            "control: traces add marks"
        );
    }

    /// `std-bio-cell` (coreOrganelles) with the nucleus selected: the membrane
    /// becomes a compartment, the nucleus a 1.4x highlighted node.
    fn cell_selected_nucleus() -> Value {
        json!({
            "type": "biology-canvas", "width": 600, "height": 400,
            "nodes": [
                {"id": "nucleus", "x": 320, "y": 180, "radius": 64.4, "color": "series-2", "label": "Nucleus", "kind": "cell", "state": "highlight"},
                {"id": "ribosomes", "x": 425, "y": 145, "radius": 9, "color": "series-4", "label": "Ribosomes", "kind": "organelle", "state": "default"}
            ],
            "compartments": [{"x": 300, "y": 200, "width": 320, "height": 320, "label": "Cell Membrane", "color": "ink", "stroke": "normal"}]
        })
    }

    #[test]
    fn cell_membrane_compartment_is_a_tinted_ink_ellipse_with_a_top_label() {
        let shapes = derive_shapes(&cell_selected_nucleus(), 600.0, 400.0);
        let e = &shapes[0];
        assert_eq!(e.ty, ShapeType::Ellipse);
        assert_eq!(
            (e.x, e.y, e.width, e.height),
            (Some(300.0), Some(200.0), Some(320.0), Some(320.0))
        );
        assert_eq!(
            (
                e.color.as_deref(),
                e.fill.as_deref(),
                e.fill_style.as_deref()
            ),
            (Some("ink"), Some("ink"), Some("tint"))
        );
        assert_eq!(e.stroke.as_deref(), Some("normal"));
        assert_eq!(e.dash, None);
        let t = &shapes[1];
        assert_eq!(
            (t.text.as_deref(), t.x, t.y),
            (Some("Cell Membrane"), Some(300.0), Some(54.0))
        );
        assert_eq!(
            (t.tone.as_deref(), t.text_size.as_deref(), t.align),
            (Some("ink"), Some("sm"), Some(Align::Center))
        );
    }

    #[test]
    fn compartment_defaults_and_overrides_diverge_on_color_fill_dash_and_line_width() {
        let props = json!({"compartments": [{"x": 0, "y": 0, "width": 10, "height": 10}, {"x": 0, "y": 0, "width": 10, "height": 10, "color": "series-3", "fill": "series-1", "dash": "dotted", "lineWidth": 4}]});
        let shapes = derive_shapes(&props, 600.0, 400.0);
        assert_eq!(
            (shapes[0].color.as_deref(), shapes[0].fill.as_deref()),
            (Some("ink"), Some("ink"))
        );
        assert_eq!(
            (shapes[0].stroke.as_deref(), shapes[0].line_width),
            (Some("normal"), None)
        );
        assert_eq!(
            (shapes[1].color.as_deref(), shapes[1].fill.as_deref()),
            (Some("series-3"), Some("series-1"))
        );
        assert_eq!(
            (shapes[1].stroke.as_deref(), shapes[1].line_width),
            (None, Some(4.0)),
            "lineWidth replaces the stroke role"
        );
        assert_eq!(shapes[1].dash, Some(Dash::Dotted));
    }

    #[test]
    fn highlight_adds_a_ring_four_px_outside_and_muted_fades_node_and_label() {
        let shapes = derive_shapes(&cell_selected_nucleus(), 600.0, 400.0);
        let circles = of(&shapes, ShapeType::Circle);
        assert_eq!(circles.len(), 3, "two nodes + one highlight ring");
        let ring = circles[1];
        assert_eq!(ring.radius, Some(68.4));
        assert_eq!(ring.tone.as_deref(), Some("highlight"));
        assert!(ring.fill.is_none() && ring.fill_style.is_none());

        let mut muted = cell_selected_nucleus();
        muted["nodes"][0]["state"] = json!("muted");
        let shapes = derive_shapes(&muted, 600.0, 400.0);
        assert_eq!(
            of(&shapes, ShapeType::Circle).len(),
            2,
            "control: no ring when muted"
        );
        let node = of(&shapes, ShapeType::Circle)[0];
        assert_eq!(node.opacity, Some(0.35));
        let label = shapes
            .iter()
            .find(|s| s.text.as_deref() == Some("Nucleus"))
            .unwrap();
        assert_eq!(label.opacity, Some(0.35));
        let other = shapes
            .iter()
            .find(|s| s.text.as_deref() == Some("Ribosomes"))
            .unwrap();
        assert_eq!(other.opacity, None);
    }

    /// `std-bio-mitosis` prophase scene (config `phaseScenes[1]`): the dashed
    /// `dotted` nucleus compartment inside the cell, chromosome nodes with empty
    /// labels, and the five-phase timeline at step 1.
    fn mitosis(step: usize) -> Value {
        let labels = [
            "Interphase",
            "Prophase",
            "Metaphase",
            "Anaphase",
            "Telophase",
        ];
        let stages: Vec<Value> = labels
            .iter()
            .enumerate()
            .map(|(i, l)| json!({"label": l, "state": if i < step { "done" } else if i == step { "active" } else { "pending" }}))
            .collect();
        json!({
            "type": "biology-canvas", "width": 600, "height": 400,
            "nodes": [
                {"id": "c1", "x": 270, "y": 180, "radius": 10, "color": "series-2", "label": "", "kind": "organelle"},
                {"id": "c2", "x": 330, "y": 180, "radius": 10, "color": "series-2", "label": "", "kind": "organelle"}
            ],
            "compartments": [
                {"x": 300, "y": 200, "width": 340, "height": 280, "label": "cell", "color": "ink"},
                {"x": 300, "y": 200, "width": 140, "height": 120, "label": "nucleus", "color": "series-3", "dash": "dotted"}
            ],
            "stages": stages
        })
    }

    #[test]
    fn mitosis_timeline_places_five_chips_on_the_bottom_strip_with_arrows_between() {
        let shapes = derive_shapes(&mitosis(1), 600.0, 400.0);
        let chips = of(&shapes, ShapeType::Rect);
        assert_eq!(chips.len(), 5);
        let slot = 584.0_f32 / 5.0;
        for (i, c) in chips.iter().enumerate() {
            assert!(
                close(c.x, 8.0 + i as f32 * slot + 5.0) && close(c.width, slot - 10.0),
                "chip {i}"
            );
            assert_eq!((c.y, c.height), (Some(368.0), Some(26.0)));
        }
        let arrows = of(&shapes, ShapeType::Arrow);
        assert_eq!(arrows.len(), 4);
        assert!(close(arrows[0].x1, 13.0 + slot - 10.0) && close(arrows[0].x2, 8.0 + slot + 5.0));
        assert_eq!(
            (arrows[0].y1, arrows[0].tone.as_deref()),
            (Some(381.0), Some("guide"))
        );
        let colors: Vec<_> = chips.iter().map(|c| c.color.as_deref().unwrap()).collect();
        assert_eq!(
            colors,
            ["muted", "primary", "fill", "fill", "fill"],
            "done / active / pending"
        );
        let label = shapes
            .iter()
            .find(|s| s.text.as_deref() == Some("Prophase"))
            .unwrap();
        assert_eq!(
            (label.tone.as_deref(), label.y, label.text_size.as_deref()),
            (Some("ink"), Some(381.0), Some("xs"))
        );
        let pending = shapes
            .iter()
            .find(|s| s.text.as_deref() == Some("Telophase"))
            .unwrap();
        assert_eq!(
            pending.tone.as_deref(),
            Some("label"),
            "pending text is the label tone"
        );
    }

    #[test]
    fn mitosis_stage_color_overrides_the_state_fill_and_a_later_step_changes_the_states() {
        let mut props = mitosis(4);
        props["stages"][0]["color"] = json!("series-6");
        let shapes = derive_shapes(&props, 600.0, 400.0);
        let chips = of(&shapes, ShapeType::Rect);
        let colors: Vec<_> = chips.iter().map(|c| c.color.as_deref().unwrap()).collect();
        assert_eq!(colors, ["series-6", "muted", "muted", "muted", "primary"]);
        let before = derive_shapes(&mitosis(1), 600.0, 400.0);
        assert_ne!(
            of(&before, ShapeType::Rect)[4].color,
            chips[4].color,
            "control: step 1 leaves Telophase pending"
        );
    }

    #[test]
    fn a_single_stage_has_no_connecting_arrow_and_no_stages_draws_no_strip() {
        let mut props = mitosis(0);
        props["stages"] = json!([{"label": "Only"}]);
        let shapes = derive_shapes(&props, 600.0, 400.0);
        assert!(of(&shapes, ShapeType::Arrow).is_empty());
        assert_eq!(of(&shapes, ShapeType::Rect).len(), 1);
        props["stages"] = json!([]);
        assert!(of(&derive_shapes(&props, 600.0, 400.0), ShapeType::Rect).is_empty());
    }

    #[test]
    fn mitosis_compartments_carry_dashes_and_empty_labels_draw_no_text() {
        let shapes = derive_shapes(&mitosis(1), 600.0, 400.0);
        let ellipses = of(&shapes, ShapeType::Ellipse);
        assert_eq!(
            (ellipses[0].dash, ellipses[1].dash),
            (None, Some(Dash::Dotted))
        );
        let texts: Vec<_> = shapes.iter().filter_map(|s| s.text.as_deref()).collect();
        assert_eq!(&texts[..2], ["cell", "nucleus"]);
        assert!(!texts.contains(&""), "chromosome nodes have label \"\"");
    }

    /// `std-bio-phasecycle` ring overview (ringWidth 480 x ringHeight 260) with
    /// four phases at step 1: Prophase done, Metaphase active.
    fn phase_ring() -> Value {
        json!({
            "type": "biology-canvas", "width": 480, "height": 260, "stageStyle": "ring",
            "stages": [
                {"label": "Prophase", "state": "done"}, {"label": "Metaphase", "state": "active"},
                {"label": "Anaphase", "state": "pending"}, {"label": "Telophase", "state": "pending"}
            ]
        })
    }

    #[test]
    fn phase_ring_places_chips_clockwise_from_the_top_on_a_radius_of_min_side_half_minus_48() {
        let shapes = derive_shapes(&phase_ring(), 480.0, 260.0);
        let chips = of(&shapes, ShapeType::Rect);
        let centers: Vec<(f32, f32)> = chips
            .iter()
            .map(|c| {
                (
                    c.x.unwrap() + c.width.unwrap() / 2.0,
                    c.y.unwrap() + c.height.unwrap() / 2.0,
                )
            })
            .collect();
        let want = [
            (240.0, 48.0),
            (322.0, 130.0),
            (240.0, 212.0),
            (158.0, 130.0),
        ];
        for (got, want) in centers.iter().zip(want) {
            assert!(
                (got.0 - want.0).abs() < 0.01 && (got.1 - want.1).abs() < 0.01,
                "{got:?} vs {want:?}"
            );
        }
        assert_eq!(chips[0].width, Some(58.0), "8 chars * 6 + 10");
        assert_eq!(chips[1].width, Some(64.0));
        assert_eq!(chips[0].height, Some(18.0));
        let colors: Vec<_> = chips.iter().map(|c| c.color.as_deref().unwrap()).collect();
        assert_eq!(colors, ["muted", "primary", "fill", "fill"]);
    }

    #[test]
    fn phase_ring_arrows_stop_46px_short_of_each_chip_centre_and_do_not_close_the_loop() {
        let shapes = derive_shapes(&phase_ring(), 480.0, 260.0);
        let arrows = of(&shapes, ShapeType::Arrow);
        assert_eq!(arrows.len(), 3);
        let u = std::f32::consts::FRAC_1_SQRT_2;
        assert!(close(arrows[0].x1, 240.0 + u * 46.0) && close(arrows[0].y1, 48.0 + u * 46.0));
        assert!(close(arrows[0].x2, 322.0 - u * 46.0) && close(arrows[0].y2, 130.0 - u * 46.0));
        assert_eq!(arrows[0].tone.as_deref(), Some("guide"));
        let mut timeline = phase_ring();
        timeline["stageStyle"] = json!("timeline");
        let t = derive_shapes(&timeline, 480.0, 260.0);
        assert_ne!(
            of(&t, ShapeType::Rect)[0].y,
            of(&shapes, ShapeType::Rect)[0].y,
            "control: stageStyle switches layout"
        );
    }

    #[test]
    fn long_ring_labels_clamp_the_chip_width_to_84_and_short_ones_to_26() {
        let props = json!({"width": 480, "height": 260, "stageStyle": "ring", "stages": [{"label": "Extraordinarily long"}, {"label": "A"}]});
        let chips: Vec<_> = derive_shapes(&props, 480.0, 260.0)
            .into_iter()
            .filter(|s| s.ty == ShapeType::Rect)
            .collect();
        assert_eq!((chips[0].width, chips[1].width), (Some(84.0), Some(26.0)));
    }

    /// `std-bio-dna-replication` (sequence ATGCCTGA, 600x400) after two steps:
    /// fork 2/8, rungs 0-1 new, rung 2 open, the rest paired.
    fn replication_helix(fork_index: usize) -> Value {
        let bases = ["A", "T", "G", "C", "C", "T", "G", "A"];
        let comps = ["T", "A", "C", "G", "G", "A", "C", "T"];
        let rungs: Vec<Value> = (0..8)
            .map(|i| json!({"a": bases[i], "b": comps[i], "state": if i < fork_index { "new" } else if i == fork_index { "open" } else { "paired" }}))
            .collect();
        json!({"type": "biology-canvas", "width": 600, "height": 400, "helix": {"fork": fork_index as f64 / 8.0, "rungs": rungs}})
    }

    #[test]
    fn replication_helix_unzips_the_rungs_behind_the_fork_and_stubs_them() {
        let shapes = derive_shapes(&replication_helix(2), 600.0, 400.0);
        let lines = of(&shapes, ShapeType::Line);
        assert_eq!(
            shapes.len(),
            14 + 8 + 18,
            "2 strands x 7 segments, 2 unzipped rungs, 6 paired rungs"
        );
        let bold: Vec<_> = lines
            .iter()
            .filter(|l| l.stroke.as_deref() == Some("bold"))
            .collect();
        assert_eq!(bold.len(), 14);
        assert_eq!(bold[0].color.as_deref(), Some("series-1"));
        assert_eq!(bold[7].color.as_deref(), Some("series-2"));
        assert!(
            close(bold[0].x1, 24.0 + 0.5 * 69.0) && close(bold[0].y1, 200.0 - 79.0 / 2.0),
            "rung 0 separation 28 + 68*0.75"
        );
        assert!(
            close(bold[0].y2, 200.0 - 45.0 / 2.0),
            "rung 1 separation 28 + 68*0.25"
        );
        let thin: Vec<_> = lines
            .iter()
            .filter(|l| l.stroke.as_deref() == Some("thin"))
            .collect();
        assert_eq!(
            thin[0].color.as_deref(),
            Some("accent"),
            "new rungs are accent"
        );
        assert!(
            close(thin[0].y1, 160.5) && close(thin[0].y2, 168.5),
            "an unzipped rung is an 8px stub from the strand"
        );
        let a = shapes
            .iter()
            .find(|s| {
                s.text.as_deref() == Some("A") && s.y.is_some_and(|y| (y - 174.5).abs() < 0.01)
            })
            .unwrap();
        assert_eq!(
            (a.align, a.tone.as_deref(), a.text_size.as_deref()),
            (Some(Align::Center), Some("label"), Some("xs"))
        );
        let open_rung = thin
            .iter()
            .find(|l| l.color.as_deref() == Some("muted"))
            .unwrap();
        assert!(
            close(open_rung.y1, 186.0) && close(open_rung.y2, 214.0),
            "paired rungs span the 28px separation"
        );
    }

    #[test]
    fn a_fully_paired_helix_has_no_stubs_and_the_fork_moves_the_split() {
        let closed = derive_shapes(&replication_helix(0), 600.0, 400.0);
        assert!(
            of(&closed, ShapeType::Line)
                .iter()
                .all(|l| l.color.as_deref() != Some("accent")),
            "no new rungs at fork 0"
        );
        let stubs = |s: &[Shape]| {
            of(s, ShapeType::Line)
                .iter()
                .filter(|l| {
                    l.stroke.as_deref() == Some("thin")
                        && (l.y2.unwrap() - l.y1.unwrap()).abs() < 9.0
                })
                .count()
        };
        assert_eq!(stubs(&closed), 0);
        assert_eq!(
            stubs(&derive_shapes(&replication_helix(2), 600.0, 400.0)),
            4
        );
    }

    #[test]
    fn helix_geometry_color_and_rung_overrides_apply() {
        let props = json!({"helix": {"x": 10, "y": 20, "width": 100, "height": 60, "colorA": "#112233", "colorB": "series-5", "rungColor": "series-7",
            "rungs": [{"a": "G"}, {"b": "C", "color": "series-8"}]}});
        let shapes = derive_shapes(&props, 600.0, 400.0);
        let strands: Vec<_> = of(&shapes, ShapeType::Line)
            .into_iter()
            .filter(|l| l.stroke.as_deref() == Some("bold"))
            .collect();
        assert_eq!(
            (strands[0].color.as_deref(), strands[1].color.as_deref()),
            (Some("#112233"), Some("series-5"))
        );
        assert!(close(strands[0].x1, 10.0 + 25.0) && close(strands[0].x2, 10.0 + 75.0));
        assert!(close(strands[0].y1, 50.0 - 14.0), "cy = 20 + 30, sep 28");
        let rungs: Vec<_> = of(&shapes, ShapeType::Line)
            .into_iter()
            .filter(|l| l.stroke.as_deref() == Some("thin"))
            .collect();
        assert_eq!(
            (rungs[0].color.as_deref(), rungs[1].color.as_deref()),
            (Some("series-7"), Some("series-8"))
        );
        let texts: Vec<_> = shapes.iter().filter_map(|s| s.text.as_deref()).collect();
        assert_eq!(texts, ["G", "C"], "a missing base letter draws nothing");
    }

    #[test]
    fn authored_shapes_are_appended_last_and_hits_index_into_the_derived_list() {
        let mut props = cell_selected_nucleus();
        props["shapes"] =
            json!([{"type": "line", "x1": 0, "y1": 0, "x2": 5, "y2": 5, "id": "extra"}]);
        let shapes = derive_shapes(&props, 600.0, 400.0);
        assert_eq!(shapes.last().unwrap().id.as_deref(), Some("extra"));
        let mut interactive = props.clone();
        interactive["interactive"] = json!(true);
        interactive["onShapeClick"] = json!("PICK");
        let scene = map(&interactive, None).unwrap();
        let hit = scene.emits_at(425.0, 145.0);
        assert_eq!(
            (hit[0].event.as_str(), &hit[0].payload),
            (
                "PICK",
                &json!({"id": "ribosomes", "type": "circle", "index": 5})
            )
        );
        let ring = scene.emits_at(320.0, 180.0);
        assert_eq!(
            ring[0].payload,
            json!({"type": "circle", "index": 3}),
            "the unnamed highlight ring is topmost over the nucleus, as on the web"
        );
        assert!(
            map(&props, None).unwrap().hits.is_empty(),
            "control: not interactive"
        );
    }

    /// `std-bio-dna-3d` and `std-learn-tour-3d` render `mode: "3d"` with
    /// `helix3d`/camera props: a three.js scene the 2D renderer refuses.
    #[test]
    fn three_d_mode_is_refused_and_two_d_is_not() {
        let mut props = json!({"type": "biology-canvas", "mode": "3d", "helix3d": {"count": 10}, "width": 600, "height": 400});
        assert!(
            matches!(map(&props, None), Err(CanvasSceneError::UnsupportedMode(m)) if m == "3d")
        );
        props["mode"] = json!("2d");
        assert!(map(&props, None).is_ok());
    }
}
