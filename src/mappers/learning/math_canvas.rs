//! `math-canvas` — port of `MathCanvas.tsx`: a coordinate plane (grid, axes,
//! tick labels) with regions, bars, guides, curves, hops, angle arcs, points and
//! vectors derived into `LearningShape`s in canvas pixels, drawn by [`canvas`].

use super::canvas::{self, CanvasSpec};
use super::shape::{parse_shapes, Dash, Point, Shape, ShapeType};
use super::theme::{DiagramTheme, Rgba};
use crate::mappers::game_canvas_2d::{get_f32, get_string, paint_drawables};
use crate::mappers::scene::MappedScene;
use crate::projector::{PlotAxes, Projector};
use serde_json::Value;

use super::shape::Align;

/// `plotTransform`: world to canvas mapping of the plot box.
#[derive(Debug, Copy, Clone)]
pub struct Plot {
    pub sx: f32,
    pub sy: f32,
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
    x_min: f32,
    y_min: f32,
}

impl Plot {
    pub fn new(
        width: f32,
        height: f32,
        (x_min, x_max): (f32, f32),
        (y_min, y_max): (f32, f32),
        equal: bool,
    ) -> Self {
        let margin = 24.0;
        let fit_w = width - margin * 2.0;
        let fit_h = height - margin * 2.0;
        let fit_sx = fit_w / (x_max - x_min);
        let fit_sy = fit_h / (y_max - y_min);
        let sx = if equal { fit_sx.min(fit_sy) } else { fit_sx };
        let sy = if equal { fit_sx.min(fit_sy) } else { fit_sy };
        let left = margin + (fit_w - sx * (x_max - x_min)) / 2.0;
        let bottom = height - margin - (fit_h - sy * (y_max - y_min)) / 2.0;
        Self {
            sx,
            sy,
            left,
            right: left + sx * (x_max - x_min),
            top: bottom - sy * (y_max - y_min),
            bottom,
            x_min,
            y_min,
        }
    }

    pub fn map_x(&self, x: f32) -> f32 {
        self.left + (x - self.x_min) * self.sx
    }

    pub fn map_y(&self, y: f32) -> f32 {
        self.bottom - (y - self.y_min) * self.sy
    }

    pub fn plot_h(&self) -> f32 {
        self.bottom - self.top
    }
}

/// `formatTick`.
fn format_tick(v: f32) -> String {
    if v.fract() == 0.0 {
        return format!("{}", v as i64);
    }
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn dash_of(v: &Value) -> Option<Dash> {
    match v.get("dash").and_then(Value::as_str)? {
        "dashed" => Some(Dash::Dashed),
        "dotted" => Some(Dash::Dotted),
        "solid" => Some(Dash::Solid),
        _ => None,
    }
}

fn f(v: &Value, key: &str) -> Option<f32> {
    v.get(key).and_then(Value::as_f64).map(|n| n as f32)
}

fn samples(v: Option<&Value>) -> Vec<Point> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|p| {
                    Some(Point {
                        x: f(p, "x")?,
                        y: f(p, "y")?,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn list<'a>(props: &'a Value, key: &str) -> &'a [Value] {
    props
        .get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// Grid positions `ceil(min/step)*step ..= max` stepping by `step`, accumulated
/// like the web loop.
fn steps(min: f32, max: f32, step: f32) -> Vec<f32> {
    let mut out = Vec::new();
    if step <= 0.0 {
        return out;
    }
    let mut v = (min / step).ceil() * step;
    while v <= max {
        out.push(v);
        v += step;
    }
    out
}

fn label_every(span: f32, step: f32, extent: f32, per_label: f32) -> usize {
    let slots = (extent / per_label).floor();
    if slots < 1.0 {
        return usize::MAX;
    }
    ((span / step) / slots).ceil().max(1.0) as usize
}

/// The shapes `MathCanvas` derives, in its paint order.
/// The plot box `MathCanvas` lays its world axes out in.
fn plot_of(props: &Value, width: f32, height: f32) -> Plot {
    let x = (get_f32(props, "xMin", -10.0), get_f32(props, "xMax", 10.0));
    let y = (get_f32(props, "yMin", -10.0), get_f32(props, "yMax", 10.0));
    let equal = props.get("aspect").and_then(Value::as_str) == Some("equal");
    Plot::new(width, height, x, y, equal)
}

pub fn derive_shapes(props: &Value, width: f32, height: f32) -> Vec<Shape> {
    let x_min = get_f32(props, "xMin", -10.0);
    let x_max = get_f32(props, "xMax", 10.0);
    let y_min = get_f32(props, "yMin", -10.0);
    let y_max = get_f32(props, "yMax", 10.0);
    let show_axes = props
        .get("showAxes")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let show_grid = props
        .get("showGrid")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let grid_step = get_f32(props, "gridStep", 1.0);
    let grid_color = get_string(props, "gridColor").unwrap_or_else(|| "grid".into());
    let axis_color = get_string(props, "axisColor").unwrap_or_else(|| "axis".into());
    let show_tick_labels = props
        .get("showTickLabels")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let tick_px = props
        .get("tickLabelFontSize")
        .and_then(Value::as_f64)
        .map(|n| n as f32);
    let label_px = props
        .get("labelFontSize")
        .and_then(Value::as_f64)
        .map(|n| n as f32);
    let show_curve_labels = props
        .get("showCurveLabels")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let plot = plot_of(props, width, height);
    let (sx, sy) = (plot.sx, plot.sy);
    let (left, right, top, bottom) = (plot.left, plot.right, plot.top, plot.bottom);
    let plot_w = right - left;
    let plot_h = plot.plot_h();
    let x_axis_y = top.max(bottom.min(plot.map_y(0.0)));
    let y_axis_x = left.max(right.min(plot.map_x(0.0)));

    let mut out: Vec<Shape> = Vec::new();

    if show_grid {
        for x in steps(x_min, x_max, grid_step) {
            let px = plot.map_x(x);
            out.push(
                Shape::line(px, top, px, bottom)
                    .color(grid_color.clone())
                    .stroke_weight("thin"),
            );
        }
        for y in steps(y_min, y_max, grid_step) {
            let py = plot.map_y(y);
            out.push(
                Shape::line(left, py, right, py)
                    .color(grid_color.clone())
                    .stroke_weight("thin"),
            );
        }
    }

    if show_tick_labels {
        let tick = |x: f32, y: f32, v: f32, align: Align| {
            Shape::text(x, y, format_tick(v))
                .verbatim()
                .tone("label")
                .text_size("xs")
                .font_size(tick_px)
                .align(align)
        };
        let every_x = label_every(x_max - x_min, grid_step, plot_w, 40.0);
        for (k, x) in steps(x_min, x_max, grid_step).into_iter().enumerate() {
            if k % every_x == 0 && x != 0.0 {
                out.push(tick(plot.map_x(x), x_axis_y + 12.0, x, Align::Center));
            }
        }
        let every_y = label_every(y_max - y_min, grid_step, plot_h, 28.0);
        for (k, y) in steps(y_min, y_max, grid_step).into_iter().enumerate() {
            if k % every_y == 0 && y != 0.0 {
                out.push(tick(y_axis_x - 6.0, plot.map_y(y), y, Align::Right));
            }
        }
        if x_min <= 0.0 && x_max >= 0.0 && y_min <= 0.0 && y_max >= 0.0 {
            out.push(tick(y_axis_x - 6.0, x_axis_y + 12.0, 0.0, Align::Right));
        }
    }

    for region in list(props, "regions") {
        let upper_src = samples(region.get("samples"));
        if upper_src.is_empty() {
            continue;
        }
        let baseline = f(region, "baseline").unwrap_or(0.0);
        let clamped = |p: &Point| Point {
            x: plot.map_x(p.x.clamp(x_min, x_max)),
            y: plot.map_y(p.y.clamp(y_min, y_max)),
        };
        let first = upper_src[0];
        let last = upper_src[upper_src.len() - 1];
        let lower_src = samples(region.get("samples2"));
        let closing: Vec<Point> = if !lower_src.is_empty() {
            lower_src.iter().rev().map(clamped).collect()
        } else {
            vec![
                clamped(&Point {
                    x: last.x,
                    y: baseline,
                }),
                clamped(&Point {
                    x: first.x,
                    y: baseline,
                }),
            ]
        };
        let color = get_string(region, "color").unwrap_or_else(|| "series-1".into());
        let mut pts: Vec<Point> = upper_src.iter().map(clamped).collect();
        pts.extend(closing);
        let mut poly = Shape::polygon(pts)
            .fill(color.clone())
            .color(color.clone())
            .fill_style("tint")
            .stroke_weight("thin");
        poly.opacity = f(region, "opacity");
        out.push(poly);
        if let Some(label) = get_string(region, "label") {
            let mid = upper_src.len() / 2;
            out.push(
                Shape::text(
                    plot.map_x((first.x + last.x) / 2.0),
                    (plot.map_y(upper_src[mid].y) + plot.map_y(baseline)) / 2.0,
                    label,
                )
                .verbatim()
                .color(color)
                .font_size(label_px),
            );
        }
    }

    for bar in list(props, "bars") {
        let (Some(bx), Some(bw), Some(y1)) = (f(bar, "x"), f(bar, "width"), f(bar, "y1")) else {
            continue;
        };
        if bx + bw < x_min || bx > x_max {
            continue;
        }
        let y0 = f(bar, "y0").unwrap_or(0.0);
        let color = get_string(bar, "color").unwrap_or_else(|| "series-2".into());
        let mut rect = Shape::rect(
            plot.map_x(bx),
            plot.map_y(y0.max(y1)),
            plot.map_x(bx + bw) - plot.map_x(bx),
            (plot.map_y(y1) - plot.map_y(y0)).abs(),
        )
        .color(color.clone())
        .fill(color)
        .fill_style("tint")
        .stroke_weight("thin");
        rect.opacity = f(bar, "opacity");
        out.push(rect);
    }

    if show_axes {
        out.push(
            Shape::line(left, x_axis_y, right, x_axis_y)
                .color(axis_color.clone())
                .stroke_weight("normal"),
        );
        out.push(
            Shape::line(y_axis_x, top, y_axis_x, bottom)
                .color(axis_color)
                .stroke_weight("normal"),
        );
    }

    for guide in list(props, "guides") {
        let (Some(kind), Some(at)) = (guide.get("kind").and_then(Value::as_str), f(guide, "at"))
        else {
            continue;
        };
        let color = get_string(guide, "color").unwrap_or_else(|| "guide".into());
        let dash = dash_of(guide).or(Some(Dash::Dashed));
        let label = get_string(guide, "label");
        if kind == "vline" {
            if at < x_min || at > x_max {
                continue;
            }
            let px = plot.map_x(at);
            out.push(
                Shape::line(px, top, px, bottom)
                    .color(color.clone())
                    .dash(dash)
                    .stroke_weight("thin"),
            );
            if let Some(label) = label {
                out.push(
                    Shape::text(px + 4.0, top + 10.0, label)
                        .verbatim()
                        .color(color)
                        .font_size(label_px),
                );
            }
        } else {
            if at < y_min || at > y_max {
                continue;
            }
            let py = plot.map_y(at);
            out.push(
                Shape::line(left, py, right, py)
                    .color(color.clone())
                    .dash(dash)
                    .stroke_weight("thin"),
            );
            if let Some(label) = label {
                out.push(
                    Shape::text(right - 4.0, py - 8.0, label)
                        .verbatim()
                        .color(color)
                        .font_size(label_px)
                        .align(Align::Right),
                );
            }
        }
    }

    for (curve_index, curve) in list(props, "curves").iter().enumerate() {
        let pts = samples(curve.get("samples"));
        if pts.len() < 2 {
            continue;
        }
        let color =
            get_string(curve, "color").unwrap_or_else(|| DiagramTheme::series_tone(curve_index));
        let mut d = String::new();
        let mut pen_down = false;
        let mut last_in_range: Option<Point> = None;
        for pair in pts.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if (a.x < x_min && b.x < x_min) || (a.x > x_max && b.x > x_max) {
                pen_down = false;
                continue;
            }
            let clip = |p: Point, q: Point, lim: f32| {
                let t = if q.x == p.x {
                    0.0
                } else {
                    (lim - p.x) / (q.x - p.x)
                };
                Point {
                    x: lim,
                    y: p.y + t * (q.y - p.y),
                }
            };
            let clip_to = |p: Point, q: Point| {
                if p.x < x_min {
                    clip(p, q, x_min)
                } else if p.x > x_max {
                    clip(p, q, x_max)
                } else {
                    p
                }
            };
            let ca = clip_to(a, b);
            let cb = if b.x < x_min {
                clip(a, b, x_min)
            } else if b.x > x_max {
                clip(a, b, x_max)
            } else {
                b
            };
            d.push_str(&format!(
                "{} {} {} L {} {} ",
                if pen_down { "L" } else { "M" },
                plot.map_x(ca.x),
                plot.map_y(ca.y),
                plot.map_x(cb.x),
                plot.map_y(cb.y)
            ));
            pen_down = true;
            if b.x >= x_min && b.x <= x_max {
                last_in_range = Some(b);
            }
        }
        if d.is_empty() {
            continue;
        }
        out.push(
            Shape::path(d)
                .color(color.clone())
                .stroke_weight("bold")
                .dash(dash_of(curve)),
        );
        if let (true, Some(label), Some(p)) =
            (show_curve_labels, get_string(curve, "label"), last_in_range)
        {
            out.push(
                Shape::text(plot.map_x(p.x) + 6.0, plot.map_y(p.y) - 6.0, label)
                    .verbatim()
                    .color(color)
                    .font_size(label_px),
            );
        }
    }

    for hop in list(props, "hops") {
        let (Some(from), Some(to)) = (f(hop, "from"), f(hop, "to")) else {
            continue;
        };
        let (x1, x2) = (plot.map_x(from), plot.map_x(to));
        let peak = 36.0f32.min(plot_h * 0.3);
        let color = get_string(hop, "color").unwrap_or_else(|| "series-3".into());
        out.push(
            Shape::ellipse((x1 + x2) / 2.0, x_axis_y, (x2 - x1).abs(), 2.0 * peak)
                .color(color.clone())
                .angles(180.0, 360.0),
        );
        let s = (to - from).signum();
        out.push(
            Shape::polygon(vec![
                Point { x: x2, y: x_axis_y },
                Point {
                    x: x2 - 4.0 * s,
                    y: x_axis_y - 7.0,
                },
                Point {
                    x: x2 + 2.0 * s,
                    y: x_axis_y - 7.0,
                },
            ])
            .fill(color.clone())
            .color(color.clone()),
        );
        if let Some(label) = get_string(hop, "label") {
            out.push(
                Shape::text((x1 + x2) / 2.0, x_axis_y - peak - 8.0, label)
                    .verbatim()
                    .color(color)
                    .font_size(label_px)
                    .align(Align::Center),
            );
        }
    }

    for angle in list(props, "angles") {
        let (Some(ax), Some(ay), Some(from), Some(to)) = (
            f(angle, "x"),
            f(angle, "y"),
            f(angle, "from"),
            f(angle, "to"),
        ) else {
            continue;
        };
        let radius = f(angle, "radius").unwrap_or(0.8);
        let color = get_string(angle, "color").unwrap_or_else(|| "series-4".into());
        out.push(
            Shape::ellipse(
                plot.map_x(ax),
                plot.map_y(ay),
                2.0 * radius * sx,
                2.0 * radius * sy,
            )
            .color(color.clone())
            .angles(-to, -from),
        );
        if let Some(label) = get_string(angle, "label") {
            let rad = ((from + to) / 2.0).to_radians();
            out.push(
                Shape::text(
                    plot.map_x(ax + 1.35 * radius * rad.cos()),
                    plot.map_y(ay + 1.35 * radius * rad.sin()),
                    label,
                )
                .verbatim()
                .color(color)
                .font_size(label_px)
                .align(Align::Center),
            );
        }
    }

    for p in list(props, "points") {
        let (Some(px), Some(py)) = (f(p, "x"), f(p, "y")) else {
            continue;
        };
        if px < x_min || px > x_max || py < y_min || py > y_max {
            continue;
        }
        let open = p.get("style").and_then(Value::as_str) == Some("open");
        let color = get_string(p, "color").unwrap_or_else(|| "highlight".into());
        out.push(
            Shape::circle(
                plot.map_x(px),
                plot.map_y(py),
                f(p, "radius").unwrap_or(4.0),
            )
            .color(color.clone())
            .fill(color)
            .fill_style(if open { "outline" } else { "solid" })
            .stroke_weight("normal"),
        );
        if let Some(label) = get_string(p, "label") {
            out.push(
                Shape::text(plot.map_x(px) + 8.0, plot.map_y(py) - 8.0, label)
                    .verbatim()
                    .tone("ink")
                    .font_size(label_px),
            );
        }
    }

    for v in list(props, "vectors") {
        let (Some(vx0), Some(vy0), Some(dx), Some(dy)) =
            (f(v, "x"), f(v, "y"), f(v, "vx"), f(v, "vy"))
        else {
            continue;
        };
        if vx0 < x_min || vx0 > x_max || vy0 < y_min || vy0 > y_max {
            continue;
        }
        let (x1, y1) = (plot.map_x(vx0), plot.map_y(vy0));
        let (x2, y2) = (plot.map_x(vx0 + dx), plot.map_y(vy0 + dy));
        let color = get_string(v, "color").unwrap_or_else(|| "series-5".into());
        out.push(
            Shape::arrow(x1, y1, x2, y2)
                .color(color.clone())
                .stroke_weight("bold"),
        );
        if let Some(label) = get_string(v, "label") {
            out.push(
                Shape::text(x2 + 6.0, y2 - 6.0, label)
                    .verbatim()
                    .color(color)
                    .font_size(label_px),
            );
        }
    }

    out.extend(parse_shapes(props.get("shapes")));
    debug_assert!(out
        .iter()
        .all(|s| s.ty != ShapeType::VennRegion || s.inside.is_some()));
    out
}

pub fn map(props: &Value, viewport: Option<(f32, f32)>) -> MappedScene {
    let width = get_f32(props, "width", 600.0).max(1.0);
    let height = get_f32(props, "height", 400.0).max(1.0);
    let shapes = derive_shapes(props, width, height);
    let spec = CanvasSpec::from_props("math-canvas", props, 600.0, 400.0, shapes);
    let theme = canvas::theme_for(props);
    let mut scene = canvas::render(&spec, &theme, viewport);
    let drawables = props.get("drawables").and_then(Value::as_array);
    if let Some(drawables) = drawables.filter(|d| !d.is_empty()) {
        let plot = plot_of(props, spec.width, spec.height);
        let projector = Projector::plot(PlotAxes {
            ox: plot.map_x(0.0),
            oy: plot.map_y(0.0),
            sx: plot.sx,
            sy: plot.sy,
        });
        let (xf, _, _) = canvas::fit(spec.width, spec.height, viewport);
        let resolve = |c: &str| css_of(theme.mark_color(Some(c), "ink"));
        scene.packet.draw_commands.extend(paint_drawables(
            drawables,
            &projector,
            &resolve,
            &|x, y| xf.p(x, y),
            xf.s,
        ));
    }
    scene
}

fn css_of(c: Rgba) -> String {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "rgba({}, {}, {}, {})",
        byte(c[0]),
        byte(c[1]),
        byte(c[2]),
        c[3]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::{DrawCommand, PathSegment};
    use serde_json::json;

    /// `std-math-sinewave` at its config defaults (amplitude 5, frequency 1,
    /// 41 samples over -9..9, 600x400, y -6..6): the resolved `math-canvas`
    /// props its `MathSineWaveSim` renders.
    fn sinewave() -> Value {
        let xs: Vec<f64> = (0..41).map(|i| -9.0 + i as f64 * (18.0 / 40.0)).collect();
        let samples: Vec<Value> = xs
            .iter()
            .map(|x| json!({"x": x, "y": 5.0 * x.sin()}))
            .collect();
        json!({
            "type": "math-canvas", "width": 600, "height": 400,
            "xMin": -9, "xMax": 9, "yMin": -6, "yMax": 6, "showTickLabels": true,
            "curves": [
                {"label": "wave", "color": "series-1", "samples": samples},
                {"label": "reference", "color": "guide", "dash": "dashed", "samples": samples}
            ]
        })
    }

    fn count(shapes: &[Shape], ty: ShapeType) -> usize {
        shapes.iter().filter(|s| s.ty == ty).count()
    }

    #[test]
    fn drawables_paint_over_the_plot_at_their_world_cell_in_the_theme_tone() {
        let props = json!({
            "xMin": -9, "xMax": 9, "yMin": -6, "yMax": 6,
            "drawables": [{"type": "draw-shape", "shape": "cell", "position": {"x": 0, "y": 0}, "fill": "series-1"}]
        });
        let scene = map(&props, None);
        let Some(DrawCommand::Poly { points, color }) = scene.packet.draw_commands.last() else {
            panic!(
                "the drawable paints last: {:?}",
                scene.packet.draw_commands.last()
            )
        };
        let plot = Plot::new(600.0, 400.0, (-9.0, 9.0), (-6.0, 6.0), false);
        assert!(
            (points[0].x - plot.map_x(0.0)).abs() < 1e-3
                && (points[0].y - plot.map_y(0.0)).abs() < 1e-3
        );
        assert!(
            (points[1].x - points[0].x - plot.sx).abs() < 1e-3,
            "a cell is one world-x unit wide"
        );
        let tone = canvas::theme_for(&props).tone("series-1").unwrap();
        assert!(
            color.iter().zip(tone).all(|(a, b)| (a - b).abs() < 0.01),
            "{color:?} vs {tone:?}"
        );
    }

    #[test]
    fn control_no_drawables_adds_nothing_after_the_plot() {
        let with = map(&json!({"drawables": []}), None)
            .packet
            .draw_commands
            .len();
        let without = map(&json!({}), None).packet.draw_commands.len();
        assert_eq!(with, without);
    }

    #[test]
    fn plot_box_is_the_canvas_minus_a_24px_margin_and_maps_world_corners() {
        let p = Plot::new(600.0, 400.0, (-9.0, 9.0), (-6.0, 6.0), false);
        assert_eq!(
            (p.left, p.right, p.top, p.bottom),
            (24.0, 576.0, 24.0, 376.0)
        );
        assert_eq!((p.map_x(-9.0), p.map_x(9.0)), (24.0, 576.0));
        assert_eq!((p.map_y(6.0), p.map_y(-6.0)), (24.0, 376.0));
        assert!((p.map_x(0.0) - 300.0).abs() < 1e-3);
    }

    #[test]
    fn equal_aspect_uses_one_scale_and_centers_the_plot() {
        let fit = Plot::new(600.0, 400.0, (-9.0, 9.0), (-6.0, 6.0), false);
        let eq = Plot::new(600.0, 400.0, (-9.0, 9.0), (-6.0, 6.0), true);
        assert_ne!(fit.sx, fit.sy, "control: fit stretches each axis");
        assert_eq!(eq.sx, eq.sy);
        assert_eq!(eq.sx, fit.sx.min(fit.sy));
        assert!(
            (eq.left - 24.0).abs() > 0.0 || (eq.top - 24.0).abs() > 0.0,
            "letterboxed inside the margin"
        );
    }

    #[test]
    fn the_sinewave_fixture_derives_grid_axes_ticks_and_both_curves() {
        let shapes = derive_shapes(&sinewave(), 600.0, 400.0);
        // grid: 19 vertical + 13 horizontal thin lines, then 2 axis lines
        assert_eq!(count(&shapes, ShapeType::Line), 19 + 13 + 2);
        assert_eq!(count(&shapes, ShapeType::Path), 2);
        assert!(
            count(&shapes, ShapeType::Text) > 0,
            "showTickLabels adds tick text"
        );
        let curves: Vec<&Shape> = shapes.iter().filter(|s| s.ty == ShapeType::Path).collect();
        assert_eq!(curves[0].color.as_deref(), Some("series-1"));
        assert_eq!(curves[1].color.as_deref(), Some("guide"));
        assert_eq!(curves[0].dash, None);
        assert_eq!(curves[1].dash, Some(Dash::Dashed));
        assert_eq!(curves[0].stroke.as_deref(), Some("bold"));
    }

    #[test]
    fn grid_axes_and_tick_labels_each_switch_off_independently() {
        let mut props = sinewave();
        props["showGrid"] = json!(false);
        let shapes = derive_shapes(&props, 600.0, 400.0);
        assert_eq!(count(&shapes, ShapeType::Line), 2, "only the axes remain");
        props["showAxes"] = json!(false);
        assert_eq!(
            count(&derive_shapes(&props, 600.0, 400.0), ShapeType::Line),
            0
        );
        props["showTickLabels"] = json!(false);
        assert_eq!(
            count(&derive_shapes(&props, 600.0, 400.0), ShapeType::Text),
            0
        );
    }

    #[test]
    fn tick_labels_skip_zero_and_format_like_the_web() {
        assert_eq!(format_tick(3.0), "3");
        assert_eq!(format_tick(-2.0), "-2");
        assert_eq!(format_tick(0.5), "0.5");
        assert_eq!(format_tick(1.25), "1.25");
        assert_eq!(format_tick(0.1 + 0.2), "0.3");
        let shapes = derive_shapes(&sinewave(), 600.0, 400.0);
        let labels: Vec<&str> = shapes
            .iter()
            .filter(|s| s.ty == ShapeType::Text)
            .filter_map(|s| s.text.as_deref())
            .collect();
        assert!(labels.contains(&"-6") && labels.contains(&"6") && labels.contains(&"0"));
        assert_eq!(
            labels.iter().filter(|l| **l == "0").count(),
            1,
            "the origin label is added once, not per axis"
        );
    }

    #[test]
    fn a_curve_leaving_the_window_is_clipped_at_the_edge_not_dropped() {
        let props = json!({
            "xMin": 0, "xMax": 10, "yMin": 0, "yMax": 10, "showGrid": false, "showAxes": false,
            "curves": [{"samples": [{"x": -5, "y": 0}, {"x": 5, "y": 10}, {"x": 15, "y": 0}]}]
        });
        let shapes = derive_shapes(&props, 600.0, 400.0);
        let d = shapes[0].path.as_deref().unwrap();
        let p = Plot::new(600.0, 400.0, (0.0, 10.0), (0.0, 10.0), false);
        assert!(
            d.starts_with(&format!("M {} {}", p.map_x(0.0), p.map_y(5.0))),
            "{d}"
        );
        assert!(
            d.contains(&format!("L {} {}", p.map_x(10.0), p.map_y(5.0))),
            "{d}"
        );
        let off = json!({"xMin": 0, "xMax": 10, "yMin": 0, "yMax": 10, "curves": [{"samples": [{"x": 20, "y": 1}, {"x": 30, "y": 2}]}]});
        assert_eq!(
            count(&derive_shapes(&off, 600.0, 400.0), ShapeType::Path),
            0,
            "wholly outside draws nothing"
        );
    }

    #[test]
    fn points_vectors_regions_bars_guides_hops_and_angles_derive_their_marks() {
        let props = json!({
            "xMin": -5, "xMax": 5, "yMin": -5, "yMax": 5, "showGrid": false, "showAxes": false,
            "points": [
                {"x": 1, "y": 1, "label": "A"},
                {"x": 2, "y": 2, "style": "open", "color": "series-3"},
                {"x": 99, "y": 0}
            ],
            "vectors": [{"x": 0, "y": 0, "vx": 2, "vy": 1, "label": "v"}],
            "regions": [{"samples": [{"x": 0, "y": 1}, {"x": 2, "y": 3}], "label": "area"}],
            "bars": [{"x": 0, "width": 1, "y1": 2}, {"x": 50, "width": 1, "y1": 2}],
            "guides": [{"kind": "vline", "at": 1, "label": "x=1"}, {"kind": "hline", "at": 40}],
            "hops": [{"from": 0, "to": 2, "label": "+2"}],
            "angles": [{"x": 0, "y": 0, "from": 0, "to": 45, "label": "θ"}]
        });
        let shapes = derive_shapes(&props, 600.0, 400.0);
        let circles: Vec<&Shape> = shapes
            .iter()
            .filter(|s| s.ty == ShapeType::Circle)
            .collect();
        assert_eq!(circles.len(), 2, "the out-of-range point is dropped");
        assert_eq!(circles[0].fill_style.as_deref(), Some("solid"));
        assert_eq!(
            circles[1].fill_style.as_deref(),
            Some("outline"),
            "open style = hollow ring"
        );
        assert_eq!(count(&shapes, ShapeType::Arrow), 1);
        assert_eq!(
            count(&shapes, ShapeType::Rect),
            1,
            "the off-window bar is dropped"
        );
        assert_eq!(
            count(&shapes, ShapeType::Polygon),
            2,
            "region + hop arrowhead"
        );
        assert_eq!(count(&shapes, ShapeType::Ellipse), 2, "hop arc + angle arc");
        let guide = shapes.iter().find(|s| s.ty == ShapeType::Line).unwrap();
        assert_eq!(guide.dash, Some(Dash::Dashed), "guides default to dashed");
        assert_eq!(
            count(&shapes, ShapeType::Line),
            1,
            "the out-of-range hline is dropped"
        );
        let labels: Vec<&str> = shapes
            .iter()
            .filter(|s| s.ty == ShapeType::Text)
            .filter_map(|s| s.text.as_deref())
            .collect();
        for l in ["A", "v", "area", "x=1", "+2", "θ"] {
            assert!(labels.contains(&l), "{l} in {labels:?}");
        }
        let arcs: Vec<&Shape> = shapes
            .iter()
            .filter(|s| s.ty == ShapeType::Ellipse)
            .collect();
        assert_eq!(
            (arcs[0].start_angle, arcs[0].end_angle),
            (Some(180.0), Some(360.0)),
            "hop"
        );
        assert_eq!(
            (arcs[1].start_angle, arcs[1].end_angle),
            (Some(-45.0), Some(0.0)),
            "angle sweeps are negated for the y-down canvas"
        );
    }

    #[test]
    fn region_closes_to_the_baseline_and_a_second_boundary_replaces_it() {
        let one = json!({"showGrid": false, "showAxes": false, "regions": [{"samples": [{"x": 0, "y": 2}, {"x": 4, "y": 2}], "baseline": 1}]});
        let two = json!({"showGrid": false, "showAxes": false, "regions": [{"samples": [{"x": 0, "y": 2}, {"x": 4, "y": 2}], "samples2": [{"x": 0, "y": 0}, {"x": 4, "y": 0}, {"x": 5, "y": 0}]}]});
        let n = |p: &Value| {
            derive_shapes(p, 600.0, 400.0)[0]
                .points
                .as_ref()
                .unwrap()
                .len()
        };
        assert_eq!(n(&one), 4);
        assert_eq!(n(&two), 5);
    }

    #[test]
    fn the_scene_packet_draws_each_curve_with_its_color_dash_and_width() {
        let scene = map(&sinewave(), None);
        let theme = DiagramTheme::heritage(false);
        let strokes: Vec<_> = scene
            .packet
            .draw_commands
            .iter()
            .filter_map(|c| match c {
                DrawCommand::Path {
                    stroke: Some(s),
                    fill: None,
                    segments,
                } if segments.len() > 20 => Some(s),
                _ => None,
            })
            .collect();
        assert_eq!(strokes.len(), 2);
        assert_eq!(strokes[0].color, theme.tone("series-1").unwrap());
        assert_eq!(strokes[0].width, 2.5);
        assert!(strokes[0].dash.is_empty());
        assert_eq!(strokes[1].color, theme.tone("guide").unwrap());
        assert_eq!(
            strokes[1].dash,
            vec![6.0, 4.0],
            "control: the reference curve is dashed"
        );
        assert_eq!((scene.packet.width, scene.packet.height), (600.0, 400.0));
    }

    #[test]
    fn the_packet_follows_the_viewport_and_its_curves_move_with_the_axis_range() {
        let wide = map(&sinewave(), Some((300.0, 200.0)));
        assert_eq!((wide.packet.width, wide.packet.height), (300.0, 200.0));
        let end_x = |scene: &MappedScene| {
            let curve = scene
                .packet
                .draw_commands
                .iter()
                .find_map(|c| match c {
                    DrawCommand::Path {
                        segments,
                        stroke: Some(s),
                        ..
                    } if segments.len() > 20 && s.dash.is_empty() => Some(segments),
                    _ => None,
                })
                .unwrap();
            match curve.last() {
                Some(PathSegment::LineTo { x, .. }) => *x,
                other => panic!("{other:?}"),
            }
        };
        let half = end_x(&wide);
        let full = end_x(&map(&sinewave(), None));
        assert!(
            (full / half - 2.0).abs() < 0.01,
            "300px viewport halves the 600px scene: {full} vs {half}"
        );
    }

    #[test]
    fn interactive_shapes_emit_the_authored_event_with_the_derived_shape_index() {
        let mut props = json!({
            "xMin": -5, "xMax": 5, "yMin": -5, "yMax": 5, "showGrid": false, "showAxes": false,
            "interactive": true, "onShapeClick": "POINT_PICKED",
            "points": [{"x": 0, "y": 0}]
        });
        let scene = map(&props, None);
        let p = Plot::new(600.0, 400.0, (-5.0, 5.0), (-5.0, 5.0), false);
        let emits = scene.emits_at(p.map_x(0.0), p.map_y(0.0));
        assert_eq!(emits.len(), 1);
        assert_eq!(emits[0].event, "POINT_PICKED");
        assert_eq!(emits[0].payload, json!({"type": "circle", "index": 0}));
        props["interactive"] = json!(false);
        assert!(
            map(&props, None).hits.is_empty(),
            "control: not interactive"
        );
    }

    #[test]
    fn series_override_recolors_series_tones() {
        let mut props = sinewave();
        props["series"] = json!(["#ff0000"]);
        let scene = map(&props, None);
        let first = scene.packet.draw_commands.iter().find_map(|c| match c {
            DrawCommand::Path {
                stroke: Some(s),
                segments,
                ..
            } if segments.len() > 20 => Some(s.color),
            _ => None,
        });
        assert_eq!(first, Some([1.0, 0.0, 0.0, 1.0]));
    }
}
