//! `physics-canvas` — port of `PhysicsCanvas.tsx` (2D): bodies, constraints,
//! velocity/force arrows, scene fixtures, trails, vectors, angle arcs, a field
//! lattice and meters derived into `LearningShape`s in canvas pixels, drawn by
//! [`canvas`]. `mode: "3d"` is a three.js scene and is refused.

use super::canvas::{self, CanvasSpec};
use super::shape::{parse_shapes, Align, Dash, Point, Shape};
use super::theme::DiagramTheme;
use crate::scene::MappedScene;
use orbital_canvas_base::diagnostics::gap_once;
use orbital_canvas_map::error::CanvasSceneError;
use orbital_canvas_map::game_canvas_2d::{get_f32, get_string};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct Body {
    id: Option<String>,
    x: f32,
    y: f32,
    radius: Option<f32>,
    color: Option<String>,
    label: Option<String>,
    vx: Option<f32>,
    vy: Option<f32>,
    fx: Option<f32>,
    fy: Option<f32>,
}

#[derive(Debug, Clone, Deserialize)]
struct Constraint {
    from: String,
    to: String,
    color: Option<String>,
    kind: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct SceneObject {
    kind: String,
    x: Option<f32>,
    y: Option<f32>,
    x1: Option<f32>,
    y1: Option<f32>,
    x2: Option<f32>,
    y2: Option<f32>,
    width: Option<f32>,
    height: Option<f32>,
    color: Option<String>,
    fill: Option<String>,
    label: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Trail {
    #[serde(default)]
    points: Vec<Point>,
    color: Option<String>,
    width: Option<f32>,
    opacity: Option<f32>,
    fade: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct Vector {
    x: Option<f32>,
    y: Option<f32>,
    body: Option<String>,
    dx: f32,
    dy: f32,
    scale: Option<f32>,
    color: Option<String>,
    label: Option<String>,
    dash: Option<Dash>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct AngleMarker {
    x: f32,
    y: f32,
    from: f32,
    to: f32,
    radius: Option<f32>,
    color: Option<String>,
    label: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct Field {
    kind: String,
    spacing: Option<f32>,
    angle: Option<f32>,
    size: Option<f32>,
    color: Option<String>,
    x: Option<f32>,
    y: Option<f32>,
    width: Option<f32>,
    height: Option<f32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct Meter {
    label: String,
    value: f64,
    max: Option<f64>,
    color: Option<String>,
}

fn list<T: for<'de> Deserialize<'de>>(props: &Value, key: &'static str) -> Vec<T> {
    let Some(items) = props.get(key).and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(
            |item| match orbital_core_base::json_value::from_json_value::<T>(item) {
                Ok(v) => Some(v),
                Err(err) => {
                    gap_once(
                        "physics-entry-invalid",
                        &format!("{key} entry skipped: {err}"),
                    );
                    None
                }
            },
        )
        .collect()
}

fn flag(props: &Value, key: &str, default: bool) -> bool {
    props.get(key).and_then(Value::as_bool).unwrap_or(default)
}

/// JS truthiness of an optional string: absent and `""` are both falsy.
fn present(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|s| !s.is_empty())
}

fn color_or(c: &Option<String>, default: &str) -> String {
    c.clone().unwrap_or_else(|| default.to_string())
}

fn label_text(x: f32, y: f32, text: &str, align: Align) -> Shape {
    Shape::text(x, y, text)
        .tone("label")
        .text_size("sm")
        .align(align)
}

fn scene_object_shapes(obj: &SceneObject, canvas_w: f32, canvas_h: f32) -> Vec<Shape> {
    let mut out = Vec::new();
    let color = color_or(&obj.color, "ink");
    let label = present(&obj.label);
    match obj.kind.as_str() {
        "ground" => {
            let x_start = obj.x1.unwrap_or(0.0);
            let x_end = obj.x2.unwrap_or(canvas_w);
            let y = obj.y.unwrap_or(0.0);
            out.push(
                Shape::line(x_start, y, x_end, y)
                    .color(&color)
                    .stroke_weight("normal"),
            );
            let mut hx = f64::from(x_start) + 7.0;
            while hx <= f64::from(x_end) {
                let hx32 = hx as f32;
                out.push(
                    Shape::line(hx32, y, hx32 - 7.0, y + 7.0)
                        .color(&color)
                        .stroke_weight("thin"),
                );
                hx += 14.0;
            }
            if let Some(l) = label {
                out.push(label_text(
                    (x_start + x_end) / 2.0,
                    y - 10.0,
                    l,
                    Align::Center,
                ));
            }
        }
        "wall" => {
            let y_start = obj.y1.unwrap_or(0.0);
            let y_end = obj.y2.unwrap_or(canvas_h);
            let x = obj.x.unwrap_or(0.0);
            out.push(
                Shape::line(x, y_start, x, y_end)
                    .color(&color)
                    .stroke_weight("normal"),
            );
            let mut hy = f64::from(y_start) + 7.0;
            while hy <= f64::from(y_end) {
                let hy32 = hy as f32;
                out.push(
                    Shape::line(x, hy32, x - 7.0, hy32 + 7.0)
                        .color(&color)
                        .stroke_weight("thin"),
                );
                hy += 14.0;
            }
            if let Some(l) = label {
                out.push(label_text(
                    x + 12.0,
                    (y_start + y_end) / 2.0,
                    l,
                    Align::Left,
                ));
            }
        }
        "ramp" => {
            let x1 = obj.x1.unwrap_or(0.0);
            let y1 = obj.y1.unwrap_or(0.0);
            let x2 = obj.x2.unwrap_or(canvas_w);
            let y2 = obj.y2.unwrap_or(canvas_h);
            out.push(
                Shape::polygon(vec![
                    Point { x: x1, y: y1 },
                    Point { x: x2, y: y2 },
                    Point { x: x1, y: y2 },
                ])
                .color(&color)
                .fill(color_or(&obj.fill, "fill"))
                .stroke_weight("normal"),
            );
            if let Some(l) = label {
                out.push(label_text(
                    (2.0 * x1 + x2) / 3.0,
                    (y1 + 2.0 * y2) / 3.0,
                    l,
                    Align::Center,
                ));
            }
        }
        "box" => {
            let x = obj.x.unwrap_or(0.0);
            let y = obj.y.unwrap_or(0.0);
            let w = obj.width.unwrap_or(40.0);
            let h = obj.height.unwrap_or(40.0);
            let mut rect = Shape::rect(x, y, w, h)
                .color(&color)
                .stroke_weight("normal");
            rect.fill = obj.fill.clone();
            out.push(rect);
            if let Some(l) = label {
                out.push(label_text(x + w / 2.0, y + h / 2.0, l, Align::Center));
            }
        }
        "pivot" => {
            let x = obj.x.unwrap_or(0.0);
            let y = obj.y.unwrap_or(0.0);
            out.push(
                Shape::circle(x, y, 5.0)
                    .color(&color)
                    .fill(&color)
                    .fill_style("solid"),
            );
            out.push(
                Shape::line(x - 14.0, y - 8.0, x + 14.0, y - 8.0)
                    .color(&color)
                    .stroke_weight("thin"),
            );
            for k in 0..5 {
                let hx = x - 14.0 + 7.0 * k as f32;
                out.push(
                    Shape::line(hx, y - 8.0, hx - 6.0, y - 14.0)
                        .color(&color)
                        .stroke_weight("thin"),
                );
            }
            if let Some(l) = label {
                out.push(label_text(x, y - 30.0, l, Align::Center));
            }
        }
        other => gap_once(
            "physics-scene-object-kind",
            &format!("sceneObjects kind {other:?} draws nothing on the web either"),
        ),
    }
    out
}

fn trail_shapes(trail: &Trail) -> Vec<Shape> {
    let n = trail.points.len();
    if n < 2 {
        return Vec::new();
    }
    let color = color_or(&trail.color, "guide");
    let fade = trail.fade.unwrap_or(true);
    let global = trail.opacity.unwrap_or(1.0);
    (0..n - 1)
        .map(|i| {
            let (a, b) = (trail.points[i], trail.points[i + 1]);
            let segment = if fade {
                0.12 + (0.68 * i as f32) / (n - 1) as f32
            } else {
                0.6
            };
            let mut line = Shape::line(a.x, a.y, b.x, b.y)
                .color(&color)
                .stroke_weight("normal")
                .opacity(segment * global);
            line.line_width = trail.width;
            line
        })
        .collect()
}

fn constraint_shapes(c: &Constraint, a: &Body, b: &Body) -> Vec<Shape> {
    let color = color_or(&c.color, "ink");
    match c.kind.as_deref().unwrap_or("rod") {
        "string" => vec![Shape::line(a.x, a.y, b.x, b.y)
            .color(&color)
            .stroke_weight("normal")
            .dash(Some(Dash::Dashed))],
        "spring" => {
            const COILS: usize = 8;
            const AMP: f32 = 7.0;
            const LEAD: f32 = 10.0;
            let (dx, dy) = (b.x - a.x, b.y - a.y);
            let dist = dx.hypot(dy).max(1e-6);
            let (ux, uy) = (dx / dist, dy / dist);
            let (perp_x, perp_y) = (-uy, ux);
            let a_prime = (a.x + LEAD * ux, a.y + LEAD * uy);
            let b_prime = (b.x - LEAD * ux, b.y - LEAD * uy);
            let m = 2 * COILS;
            let mut poly = vec![(a.x, a.y), a_prime];
            for j in 1..=m {
                let t = j as f32 / (m + 1) as f32;
                let base_x = a_prime.0 + t * (b_prime.0 - a_prime.0);
                let base_y = a_prime.1 + t * (b_prime.1 - a_prime.1);
                let sign = if j % 2 == 0 { 1.0 } else { -1.0 };
                poly.push((base_x + sign * AMP * perp_x, base_y + sign * AMP * perp_y));
            }
            poly.push(b_prime);
            poly.push((b.x, b.y));
            poly.windows(2)
                .map(|w| {
                    Shape::line(w[0].0, w[0].1, w[1].0, w[1].1)
                        .color(&color)
                        .stroke_weight("normal")
                })
                .collect()
        }
        "rod" => vec![Shape::line(a.x, a.y, b.x, b.y)
            .color(&color)
            .stroke_weight("normal")],
        other => {
            gap_once(
                "physics-constraint-kind",
                &format!("constraint kind {other:?} is not rod/spring/string; drawn as a rod"),
            );
            vec![Shape::line(a.x, a.y, b.x, b.y)
                .color(&color)
                .stroke_weight("normal")]
        }
    }
}

fn vector_shapes(v: &Vector, bodies: &HashMap<String, Body>) -> Vec<Shape> {
    let (ax, ay, anchor_radius) = match present(&v.body) {
        Some(id) => match bodies.get(id) {
            Some(b) => (b.x, b.y, b.radius.unwrap_or(12.0)),
            None => return Vec::new(),
        },
        None => (v.x.unwrap_or(0.0), v.y.unwrap_or(0.0), 0.0),
    };
    let scale = v.scale.unwrap_or(1.0);
    let color = color_or(&v.color, "series-5");
    let tx = ax + v.dx * scale;
    let ty = ay + v.dy * scale;
    let mut out = vec![Shape::arrow(ax, ay, tx, ty)
        .color(&color)
        .stroke_weight("bold")
        .dash(v.dash)];
    if let Some(label) = present(&v.label) {
        let len = (tx - ax).hypot(ty - ay);
        let (ux, uy) = if len > 1e-6 {
            ((tx - ax) / len, (ty - ay) / len)
        } else {
            (0.0, -1.0)
        };
        let reach = len.max(anchor_radius) + 8.0;
        out.push(
            Shape::text(ax + reach * ux, ay + reach * uy, label)
                .verbatim()
                .color(&color)
                .text_size("sm")
                .align(Align::Center),
        );
    }
    out
}

fn angle_marker_shapes(a: &AngleMarker) -> Vec<Shape> {
    let radius = a.radius.unwrap_or(26.0);
    let color = color_or(&a.color, "series-4");
    let mut out = vec![Shape::ellipse(a.x, a.y, radius * 2.0, radius * 2.0)
        .angles(a.from, a.to)
        .color(&color)
        .stroke_weight("normal")];
    if let Some(label) = present(&a.label) {
        let mid = ((a.from + a.to) / 2.0).to_radians();
        out.push(
            Shape::text(
                a.x + (radius + 13.0) * mid.cos(),
                a.y + (radius + 13.0) * mid.sin(),
                label,
            )
            .verbatim()
            .color(&color)
            .text_size("sm")
            .align(Align::Center),
        );
    }
    out
}

fn field_shapes(field: &Field, canvas_w: f32, canvas_h: f32) -> Vec<Shape> {
    let spacing = field.spacing.unwrap_or(48.0);
    let size = field.size.unwrap_or(14.0);
    let color = color_or(&field.color, "guide");
    let region_x = f64::from(field.x.unwrap_or(0.0));
    let region_y = f64::from(field.y.unwrap_or(0.0));
    let region_w = f64::from(field.width.unwrap_or(canvas_w));
    let region_h = f64::from(field.height.unwrap_or(canvas_h));
    if spacing <= 0.0 {
        gap_once(
            "physics-field-spacing",
            "field.spacing <= 0 would never advance the lattice; field skipped",
        );
        return Vec::new();
    }
    if !matches!(field.kind.as_str(), "arrows" | "into" | "out") {
        gap_once(
            "physics-field-kind",
            &format!(
                "field kind {:?} is not arrows/into/out; drawn as out",
                field.kind
            ),
        );
    }
    let spacing = f64::from(spacing);
    let mut out = Vec::new();
    let mut gx = region_x + spacing / 2.0;
    while gx < region_x + region_w {
        let mut gy = region_y + spacing / 2.0;
        while gy < region_y + region_h {
            let (x, y) = (gx as f32, gy as f32);
            match field.kind.as_str() {
                "arrows" => {
                    let rad = field.angle.unwrap_or(0.0).to_radians();
                    let hx = rad.cos() * size / 2.0;
                    let hy = rad.sin() * size / 2.0;
                    out.push(
                        Shape::arrow(x - hx, y - hy, x + hx, y + hy)
                            .color(&color)
                            .stroke_weight("normal"),
                    );
                }
                "into" => {
                    let r = size / 3.0;
                    let d = 0.6 * r * std::f32::consts::FRAC_1_SQRT_2;
                    out.push(Shape::circle(x, y, r).color(&color));
                    out.push(
                        Shape::line(x - d, y - d, x + d, y + d)
                            .color(&color)
                            .stroke_weight("thin"),
                    );
                    out.push(
                        Shape::line(x - d, y + d, x + d, y - d)
                            .color(&color)
                            .stroke_weight("thin"),
                    );
                }
                _ => {
                    out.push(Shape::circle(x, y, size / 3.0).color(&color));
                    out.push(
                        Shape::circle(x, y, 1.5)
                            .color(&color)
                            .fill(&color)
                            .fill_style("solid"),
                    );
                }
            }
            gy += spacing;
        }
        gx += spacing;
    }
    out
}

/// `formatMeterValue`: integers as-is, otherwise rounded to two decimals with trailing zeros dropped.
fn format_meter_value(v: f64) -> String {
    if v.fract() == 0.0 {
        return format!("{v}");
    }
    let rounded: f64 = format!("{v:.2}").parse().unwrap_or(v);
    format!("{rounded}")
}

fn meter_shapes(meters: &[Meter], canvas_h: f32) -> Vec<Shape> {
    let n = meters.len();
    let shared_max = meters.iter().map(|m| m.value).fold(1e-6_f64, f64::max);
    let mut out = Vec::new();
    for (i, meter) in meters.iter().enumerate() {
        let row_y = canvas_h - 10.0 - 16.0 * (n - i) as f32;
        let color = color_or(&meter.color, &DiagramTheme::series_tone(i));
        let max = meter.max.unwrap_or(shared_max);
        let w = ((meter.value / max).clamp(0.0, 1.0) * 110.0).round() as f32;
        out.push(
            Shape::text(8.0, row_y + 8.0, meter.label.as_str())
                .tone("label")
                .text_size("xs"),
        );
        out.push(
            Shape::rect(52.0, row_y, w, 10.0)
                .color(&color)
                .fill(&color)
                .fill_style("solid"),
        );
        out.push(
            Shape::text(166.0, row_y + 8.0, format_meter_value(meter.value))
                .tone("label")
                .text_size("xs"),
        );
    }
    out
}

/// `derivedShapes` in `PhysicsCanvas.tsx`, in the web's draw order.
pub fn derive_shapes(props: &Value, width: f32, height: f32) -> Vec<Shape> {
    let bodies: Vec<Body> = list(props, "bodies");
    let constraints: Vec<Constraint> = list(props, "constraints");
    let scene_objects: Vec<SceneObject> = list(props, "sceneObjects");
    let trails: Vec<Trail> = list(props, "trails");
    let vectors: Vec<Vector> = list(props, "vectors");
    let angles: Vec<AngleMarker> = list(props, "angles");
    let meters: Vec<Meter> = list(props, "meters");
    let show_velocity = flag(props, "showVelocity", true);
    let show_forces = flag(props, "showForces", false);
    let velocity_scale = get_f32(props, "velocityScale", 20.0);
    let force_scale = get_f32(props, "forceScale", 20.0);

    let mut by_id: HashMap<String, Body> = HashMap::new();
    for b in &bodies {
        if let Some(id) = present(&b.id) {
            by_id.insert(id.to_string(), b.clone());
        }
    }

    let mut out = Vec::new();
    if let Some(field) = props.get("field").filter(|f| !f.is_null()) {
        match orbital_core_base::json_value::from_json_value::<Field>(field) {
            Ok(f) => out.extend(field_shapes(&f, width, height)),
            Err(err) => gap_once("physics-field-invalid", &format!("field skipped: {err}")),
        }
    }
    for obj in &scene_objects {
        out.extend(scene_object_shapes(obj, width, height));
    }
    for trail in &trails {
        out.extend(trail_shapes(trail));
    }
    for c in &constraints {
        if let (Some(a), Some(b)) = (by_id.get(&c.from), by_id.get(&c.to)) {
            out.extend(constraint_shapes(c, a, b));
        }
    }
    for (bi, b) in bodies.iter().enumerate() {
        let body_color = b
            .color
            .clone()
            .unwrap_or_else(|| DiagramTheme::series_tone(bi));
        let radius = b.radius.unwrap_or(12.0);
        let mut circle = Shape::circle(b.x, b.y, radius)
            .color(&body_color)
            .fill(&body_color)
            .fill_style("solid");
        circle.id = b.id.clone();
        out.push(circle);
        if let Some(label) = present(&b.label) {
            out.push(
                Shape::text(b.x + radius + 6.0, b.y - radius - 6.0, label)
                    .tone("ink")
                    .text_size("sm"),
            );
        }
        if let (true, Some(vx), Some(vy)) = (show_velocity, b.vx, b.vy) {
            if vx != 0.0 || vy != 0.0 {
                out.push(
                    Shape::arrow(
                        b.x,
                        b.y,
                        b.x + vx * velocity_scale,
                        b.y + vy * velocity_scale,
                    )
                    .color("series-3")
                    .stroke_weight("bold"),
                );
            }
        }
        if let (true, Some(fx), Some(fy)) = (show_forces, b.fx, b.fy) {
            if fx != 0.0 || fy != 0.0 {
                out.push(
                    Shape::arrow(b.x, b.y, b.x + fx * force_scale, b.y + fy * force_scale)
                        .color("series-5")
                        .stroke_weight("bold"),
                );
            }
        }
    }
    for v in &vectors {
        out.extend(vector_shapes(v, &by_id));
    }
    for a in &angles {
        out.extend(angle_marker_shapes(a));
    }
    if !meters.is_empty() {
        out.extend(meter_shapes(&meters, height));
    }
    out.extend(parse_shapes(props.get("shapes")));
    out
}

pub fn map(props: &Value, viewport: Option<(f32, f32)>) -> Result<MappedScene, CanvasSceneError> {
    if get_string(props, "mode").as_deref() == Some("3d") {
        return Err(CanvasSceneError::UnsupportedMode("3d".into()));
    }
    let width = get_f32(props, "width", 600.0).max(1.0);
    let height = get_f32(props, "height", 400.0).max(1.0);
    let shapes = derive_shapes(props, width, height);
    let mut spec = CanvasSpec::from_props("physics-canvas", props, 600.0, 400.0, shapes);
    spec.particles = canvas::parse_particles(props.get("particles"))?;
    Ok(canvas::render(&spec, &canvas::theme_for(props), viewport))
}

#[cfg(test)]
mod particle_tests;

#[cfg(test)]
mod tests;
