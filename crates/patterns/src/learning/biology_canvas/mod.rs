//! `biology-canvas` — port of the 2D path of `BiologyCanvas.tsx`: bands,
//! compartments, a DNA helix ladder, edges, nodes and stage chips derived into
//! `LearningShape`s in canvas pixels, drawn by [`canvas`]. `mode: 3d` is a
//! three.js scene and is refused.

use super::canvas::{self, CanvasSpec};
use super::shape::{parse_shapes, Align, Dash, Shape, ShapeType};
use crate::scene::MappedScene;
use orbital_canvas_map::error::CanvasSceneError;
use orbital_canvas_map::game_canvas_2d::get_f32;
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
            let w = (js_len(&lab) as f32 * 6.0 + 10.0).clamp(26.0, 84.0);
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
    let shapes = derive_shapes(props, width, height);
    let spec = CanvasSpec::from_props("biology-canvas", props, 600.0, 400.0, shapes);
    Ok(canvas::render(&spec, &canvas::theme_for(props), viewport))
}

#[cfg(test)]
mod tests;
