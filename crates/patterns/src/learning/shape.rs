//! `LearningShape` and its painter — the Rust twin of `LearningCanvas.tsx`
//! `drawShape` (line, arrow, circle, ellipse, rect, polygon, path, text, axis,
//! grid). Every learning molecule derives a `Vec<Shape>` in its own mapper and
//! this module is the one place a shape becomes draw commands.
//!
//! The one shape the vector layer cannot paint is `venn-region` (boolean
//! circle regions need a clip/composite primitive); it is skipped and logged.

use super::theme::{DiagramTheme, FillStyle, Marker, Rgba, Weight};
use orbital_canvas_base::diagnostics::gap_once;
use orbital_canvas_base::packet::{
    DrawCommand, Halo, LineCap, LineJoin, PathSegment, Rect, Stroke, TextAlign, Vec2,
};
use orbital_canvas_base::path_data::parse_path_data;
use orbital_canvas_base::transform::Xf;
use serde::Deserialize;
use std::f32::consts::{FRAC_PI_2, PI, TAU};

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShapeType {
    #[default]
    Line,
    Arrow,
    Circle,
    Ellipse,
    Rect,
    Polygon,
    Path,
    Text,
    Axis,
    Grid,
    VennRegion,
}

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dash {
    #[default]
    Solid,
    Dashed,
    Dotted,
}

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Debug, Copy, Clone, PartialEq, Deserialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

/// A drawn mark. Style comes from the theme by role (`tone`, `stroke`,
/// `textSize`, `fillStyle`); a literal field on the same mark (`color`,
/// `lineWidth`, `fontSize`) overrides its role.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Shape {
    #[serde(rename = "type")]
    pub ty: ShapeType,
    pub id: Option<String>,
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub x1: Option<f32>,
    pub y1: Option<f32>,
    pub x2: Option<f32>,
    pub y2: Option<f32>,
    pub radius: Option<f32>,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub points: Option<Vec<Point>>,
    pub path: Option<String>,
    pub text: Option<String>,
    pub font_size: Option<f32>,
    pub align: Option<Align>,
    pub axis: Option<String>,
    pub step: Option<f32>,
    pub color: Option<String>,
    pub fill: Option<String>,
    pub line_width: Option<f32>,
    pub opacity: Option<f32>,
    pub start_angle: Option<f32>,
    pub end_angle: Option<f32>,
    pub dash: Option<Dash>,
    pub inside: Option<Vec<String>>,
    pub outside: Option<Vec<String>>,
    pub tone: Option<String>,
    pub fill_tone: Option<String>,
    pub stroke: Option<String>,
    pub text_size: Option<String>,
    pub text_case: Option<String>,
    pub fill_style: Option<String>,
}

impl Shape {
    pub fn new(ty: ShapeType) -> Self {
        Self {
            ty,
            ..Self::default()
        }
    }

    pub fn line(x1: f32, y1: f32, x2: f32, y2: f32) -> Self {
        Self {
            x1: Some(x1),
            y1: Some(y1),
            x2: Some(x2),
            y2: Some(y2),
            ..Self::new(ShapeType::Line)
        }
    }

    pub fn arrow(x1: f32, y1: f32, x2: f32, y2: f32) -> Self {
        Self {
            ty: ShapeType::Arrow,
            ..Self::line(x1, y1, x2, y2)
        }
    }

    pub fn circle(x: f32, y: f32, radius: f32) -> Self {
        Self {
            x: Some(x),
            y: Some(y),
            radius: Some(radius),
            ..Self::new(ShapeType::Circle)
        }
    }

    pub fn ellipse(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x: Some(x),
            y: Some(y),
            width: Some(width),
            height: Some(height),
            ..Self::new(ShapeType::Ellipse)
        }
    }

    pub fn rect(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x: Some(x),
            y: Some(y),
            width: Some(width),
            height: Some(height),
            ..Self::new(ShapeType::Rect)
        }
    }

    pub fn polygon(points: Vec<Point>) -> Self {
        Self {
            points: Some(points),
            ..Self::new(ShapeType::Polygon)
        }
    }

    pub fn path(d: String) -> Self {
        Self {
            path: Some(d),
            ..Self::new(ShapeType::Path)
        }
    }

    pub fn text(x: f32, y: f32, text: impl Into<String>) -> Self {
        Self {
            x: Some(x),
            y: Some(y),
            text: Some(text.into()),
            ..Self::new(ShapeType::Text)
        }
    }

    pub fn color(mut self, c: impl Into<String>) -> Self {
        self.color = Some(c.into());
        self
    }

    pub fn tone(mut self, t: impl Into<String>) -> Self {
        self.tone = Some(t.into());
        self
    }

    pub fn fill(mut self, c: impl Into<String>) -> Self {
        self.fill = Some(c.into());
        self
    }

    pub fn fill_style(mut self, s: &str) -> Self {
        self.fill_style = Some(s.to_string());
        self
    }

    pub fn stroke_weight(mut self, s: &str) -> Self {
        self.stroke = Some(s.to_string());
        self
    }

    pub fn opacity(mut self, o: f32) -> Self {
        self.opacity = Some(o);
        self
    }

    pub fn dash(mut self, d: Option<Dash>) -> Self {
        self.dash = d;
        self
    }

    pub fn align(mut self, a: Align) -> Self {
        self.align = Some(a);
        self
    }

    pub fn font_size(mut self, px: Option<f32>) -> Self {
        self.font_size = px;
        self
    }

    pub fn text_size(mut self, s: &str) -> Self {
        self.text_size = Some(s.to_string());
        self
    }

    pub fn verbatim(mut self) -> Self {
        self.text_case = Some("verbatim".into());
        self
    }

    pub fn angles(mut self, start: f32, end: f32) -> Self {
        self.start_angle = Some(start);
        self.end_angle = Some(end);
        self
    }

    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }
}

/// Parse a `shapes` array tolerantly: an element that is not a valid shape is
/// skipped and logged, never fatal to the canvas.
pub fn parse_shapes(value: Option<&serde_json::Value>) -> Vec<Shape> {
    let Some(items) = value.and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(
            |item| match orbital_core_base::json_value::from_json_value::<Shape>(item) {
                Ok(shape) => Some(shape),
                Err(err) => {
                    gap_once("shape-invalid", &format!("shape skipped: {err}"));
                    None
                }
            },
        )
        .collect()
}

pub struct PaintCtx<'a> {
    pub theme: &'a DiagramTheme,
    pub xf: Xf,
    /// Every shape of the scene: a `venn-region` resolves its `inside`/`outside` circles here.
    pub siblings: &'a [Shape],
    /// Scene extent (grid and axis span it).
    pub width: f32,
    pub height: f32,
}

fn is_literal_color(v: Option<&str>) -> bool {
    match v {
        None | Some("") => false,
        Some(s) if s.starts_with("var(") => false,
        Some(s) => {
            let b = s.as_bytes();
            let word = b.iter().take_while(|c| c.is_ascii_lowercase()).count();
            let tone_like = word > 0
                && (word == b.len()
                    || (word + 2 == b.len() && b[word] == b'-' && b[word + 1].is_ascii_digit()));
            !tone_like
        }
    }
}

fn with_alpha(mut c: Rgba, a: f32) -> Rgba {
    c[3] *= a;
    c
}

fn default_tone(ty: ShapeType) -> &'static str {
    match ty {
        ShapeType::Grid => "grid",
        ShapeType::Axis => "axis",
        ShapeType::Text => "label",
        _ => "ink",
    }
}

fn dash_list(theme: &DiagramTheme, dash: Option<Dash>, scale: f32) -> Vec<f32> {
    let base = match dash {
        Some(Dash::Dashed) => &theme.dash,
        Some(Dash::Dotted) => &theme.dot,
        _ => return Vec::new(),
    };
    base.iter().map(|d| d * scale).collect()
}

/// Resolved look of one mark (`drawShape` head).
struct Look {
    fill: Option<Rgba>,
    /// `fillStyle: hatch`: `fill` is drawn as clipped diagonal lines, not a flat fill.
    hatch: bool,
    stroke: Rgba,
    width: f32,
}

fn fill_style_of(shape: &Shape, theme: &DiagramTheme) -> FillStyle {
    shape
        .fill_style
        .as_deref()
        .and_then(FillStyle::parse)
        .unwrap_or(if is_literal_color(shape.fill.as_deref()) {
            FillStyle::Solid
        } else {
            theme.fill_style
        })
}

fn look(shape: &Shape, theme: &DiagramTheme) -> Look {
    let opacity = shape.opacity.unwrap_or(1.0);
    let fill_value = shape.fill.as_deref().or(shape.fill_tone.as_deref());
    let fill = fill_value.map(|v| theme.mark_color(Some(v), "fill"));
    let style = fill_style_of(shape, theme);
    let stroke_color = theme.mark_color(
        shape.color.as_deref().or(shape.tone.as_deref()),
        default_tone(shape.ty),
    );
    let carries_fill = style == FillStyle::Outline && fill.is_some() && fill_value != Some("fill");
    let stroke = if carries_fill {
        fill.unwrap_or(stroke_color)
    } else {
        stroke_color
    };
    let role =
        shape
            .stroke
            .as_deref()
            .and_then(Weight::parse)
            .unwrap_or(if shape.ty == ShapeType::Grid {
                Weight::Thin
            } else {
                Weight::Normal
            });
    let declared = shape.line_width.unwrap_or_else(|| theme.stroke_width(role));
    let width = if carries_fill {
        declared.max(theme.stroke_width(Weight::Bold))
    } else {
        declared
    };
    let fill = fill.and_then(|c| match style {
        FillStyle::Outline => None,
        FillStyle::Solid | FillStyle::Hatch => Some(with_alpha(c, opacity)),
        FillStyle::Tint => Some(with_alpha(c, opacity * theme.fill_opacity)),
    });
    Look {
        fill,
        hatch: style == FillStyle::Hatch,
        stroke: with_alpha(stroke, opacity),
        width,
    }
}

fn poly_segments(points: &[Point], closed: bool, xf: Xf) -> Vec<PathSegment> {
    let mut out = Vec::with_capacity(points.len() + 1);
    for (i, p) in points.iter().enumerate() {
        let (x, y) = xf.p(p.x, p.y);
        out.push(if i == 0 {
            PathSegment::MoveTo { x, y }
        } else {
            PathSegment::LineTo { x, y }
        });
    }
    if closed {
        out.push(PathSegment::Close);
    }
    out
}

pub(crate) fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32, xf: Xf) -> Vec<PathSegment> {
    let (r, s) = (r.min(w / 2.0).min(h / 2.0), xf.s);
    let line = |px: f32, py: f32| {
        let (x, y) = xf.p(px, py);
        PathSegment::LineTo { x, y }
    };
    let arc = |cx: f32, cy: f32, start: f32| {
        let (cx, cy) = xf.p(cx, cy);
        PathSegment::Arc {
            cx,
            cy,
            rx: r * s,
            ry: r * s,
            start,
            end: start + FRAC_PI_2,
        }
    };
    let (sx, sy) = xf.p(x + r, y);
    vec![
        PathSegment::MoveTo { x: sx, y: sy },
        line(x + w - r, y),
        arc(x + w - r, y + r, -FRAC_PI_2),
        line(x + w, y + h - r),
        arc(x + w - r, y + h - r, 0.0),
        line(x + r, y + h),
        arc(x + r, y + h - r, FRAC_PI_2),
        line(x, y + r),
        arc(x + r, y + r, PI),
        PathSegment::Close,
    ]
}

/// Hatch line spacing in scene units (`fillMark`'s 6).
const HATCH_SPACING: f32 = 6.0;

fn push_path(
    out: &mut Vec<DrawCommand>,
    segments: Vec<PathSegment>,
    look: &Look,
    shape: &Shape,
    ctx: &PaintCtx,
    fill: bool,
) {
    let theme = ctx.theme;
    let stroke = Some(Stroke {
        color: look.stroke,
        width: look.width * ctx.xf.s,
        dash: dash_list(theme, shape.dash, ctx.xf.s),
        dash_offset: 0.0,
        cap: theme.line_cap,
        join: theme.line_join,
    });
    match look.fill.filter(|_| fill) {
        Some(color) if look.hatch => {
            out.push(hatch_cmd(segments.clone(), color, ctx));
            out.push(DrawCommand::Path {
                segments,
                fill: None,
                stroke,
            });
        }
        fill => out.push(DrawCommand::Path {
            segments,
            fill,
            stroke,
        }),
    }
}

/// `fillMark` hatch: 45-degree lines at [`HATCH_SPACING`] in the theme's thin stroke, clipped
/// to the mark.
fn hatch_cmd(region: Vec<PathSegment>, color: Rgba, ctx: &PaintCtx) -> DrawCommand {
    let span = ctx.width + ctx.height;
    let mut lines = Vec::new();
    let mut d = -span;
    while d < span {
        let (a, b) = (ctx.xf.p(d, 0.0), ctx.xf.p(d + span, span));
        lines.push(PathSegment::MoveTo { x: a.0, y: a.1 });
        lines.push(PathSegment::LineTo { x: b.0, y: b.1 });
        d += HATCH_SPACING;
    }
    DrawCommand::ClippedPath {
        segments: lines,
        fill: None,
        stroke: Some(Stroke {
            color,
            width: ctx.theme.stroke_width(Weight::Thin) * ctx.xf.s,
            dash: Vec::new(),
            dash_offset: 0.0,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
        }),
        clip: vec![region],
        cut: Vec::new(),
    }
}

fn marker_commands(
    theme: &DiagramTheme,
    xf: Xf,
    (x1, y1, x2, y2): (f32, f32, f32, f32),
    color: Rgba,
    width: f32,
    out: &mut Vec<DrawCommand>,
) {
    let size = (width * 4.0).max(8.0);
    let angle = (y2 - y1).atan2(x2 - x1);
    let at = |a: f32| (x2 - size * a.cos(), y2 - size * a.sin());
    let left = at(angle - PI / 6.0);
    let right = at(angle + PI / 6.0);
    let pt = |(x, y): (f32, f32)| {
        let (x, y) = xf.p(x, y);
        (x, y)
    };
    let stroke = Stroke {
        color,
        width: width * xf.s,
        dash: Vec::new(),
        dash_offset: 0.0,
        cap: theme.line_cap,
        join: theme.line_join,
    };
    let tip = pt((x2, y2));
    let (lx, ly) = pt(left);
    let (rx, ry) = pt(right);
    match theme.marker {
        Marker::Triangle => out.push(DrawCommand::Path {
            segments: vec![
                PathSegment::MoveTo { x: tip.0, y: tip.1 },
                PathSegment::LineTo { x: lx, y: ly },
                PathSegment::LineTo { x: rx, y: ry },
                PathSegment::Close,
            ],
            fill: Some(color),
            stroke: None,
        }),
        Marker::Open | Marker::Line => {
            let mut segments = vec![
                PathSegment::MoveTo { x: lx, y: ly },
                PathSegment::LineTo { x: tip.0, y: tip.1 },
                PathSegment::LineTo { x: rx, y: ry },
            ];
            if theme.marker == Marker::Line {
                let half = size * 0.5;
                let (ax, ay) = pt((
                    x2 - half * (angle + FRAC_PI_2).cos(),
                    y2 - half * (angle + FRAC_PI_2).sin(),
                ));
                let (bx, by) = pt((
                    x2 + half * (angle + FRAC_PI_2).cos(),
                    y2 + half * (angle + FRAC_PI_2).sin(),
                ));
                segments.push(PathSegment::MoveTo { x: ax, y: ay });
                segments.push(PathSegment::LineTo { x: bx, y: by });
            }
            out.push(DrawCommand::Path {
                segments,
                fill: None,
                stroke: Some(stroke),
            });
        }
        Marker::Dot => {
            let r = (width * 1.5).max(2.5) * xf.s;
            out.push(DrawCommand::Path {
                segments: vec![PathSegment::Arc {
                    cx: tip.0,
                    cy: tip.1,
                    rx: r,
                    ry: r,
                    start: 0.0,
                    end: TAU,
                }],
                fill: Some(color),
                stroke: None,
            });
        }
    }
}

/// Paint one shape into `out`.
pub fn paint(shape: &Shape, ctx: &PaintCtx, out: &mut Vec<DrawCommand>) {
    let theme = ctx.theme;
    let xf = ctx.xf;
    let look = look(shape, theme);
    match shape.ty {
        ShapeType::Grid => {
            let step = shape.step.unwrap_or(40.0).max(1.0);
            let mut segments = Vec::new();
            let mut x = 0.0;
            while x <= ctx.width {
                let (a, b) = (xf.p(x, 0.0), xf.p(x, ctx.height));
                segments.push(PathSegment::MoveTo { x: a.0, y: a.1 });
                segments.push(PathSegment::LineTo { x: b.0, y: b.1 });
                x += step;
            }
            let mut y = 0.0;
            while y <= ctx.height {
                let (a, b) = (xf.p(0.0, y), xf.p(ctx.width, y));
                segments.push(PathSegment::MoveTo { x: a.0, y: a.1 });
                segments.push(PathSegment::LineTo { x: b.0, y: b.1 });
                y += step;
            }
            push_path(out, segments, &look, shape, ctx, false);
        }
        ShapeType::Axis => {
            let horizontal = shape.axis.as_deref().unwrap_or("x") == "x";
            let ((x1, y1), (x2, y2)) = if horizontal {
                (
                    xf.p(0.0, ctx.height / 2.0),
                    xf.p(ctx.width, ctx.height / 2.0),
                )
            } else {
                (
                    xf.p(ctx.width / 2.0, 0.0),
                    xf.p(ctx.width / 2.0, ctx.height),
                )
            };
            push_path(
                out,
                vec![
                    PathSegment::MoveTo { x: x1, y: y1 },
                    PathSegment::LineTo { x: x2, y: y2 },
                ],
                &look,
                shape,
                ctx,
                false,
            );
        }
        ShapeType::Line | ShapeType::Arrow => {
            let (Some(x1), Some(y1), Some(x2), Some(y2)) = (shape.x1, shape.y1, shape.x2, shape.y2)
            else {
                return;
            };
            let (a, b) = (xf.p(x1, y1), xf.p(x2, y2));
            push_path(
                out,
                vec![
                    PathSegment::MoveTo { x: a.0, y: a.1 },
                    PathSegment::LineTo { x: b.0, y: b.1 },
                ],
                &look,
                shape,
                ctx,
                false,
            );
            if shape.ty == ShapeType::Arrow {
                marker_commands(theme, xf, (x1, y1, x2, y2), look.stroke, look.width, out);
            }
        }
        ShapeType::Circle => {
            let (Some(x), Some(y), Some(r)) = (shape.x, shape.y, shape.radius) else {
                return;
            };
            let (cx, cy) = xf.p(x, y);
            let seg = PathSegment::Arc {
                cx,
                cy,
                rx: r * xf.s,
                ry: r * xf.s,
                start: 0.0,
                end: TAU,
            };
            push_path(out, vec![seg], &look, shape, ctx, true);
        }
        ShapeType::Ellipse => {
            let (Some(x), Some(y), Some(w), Some(h)) =
                (shape.x, shape.y, shape.width, shape.height)
            else {
                return;
            };
            let (cx, cy) = xf.p(x, y);
            let start = shape.start_angle.unwrap_or(0.0).to_radians();
            let end = shape.end_angle.unwrap_or(360.0).to_radians();
            let seg = PathSegment::Arc {
                cx,
                cy,
                rx: w / 2.0 * xf.s,
                ry: h / 2.0 * xf.s,
                start,
                end,
            };
            push_path(out, vec![seg], &look, shape, ctx, true);
        }
        ShapeType::Rect => {
            let (Some(x), Some(y), Some(w), Some(h)) =
                (shape.x, shape.y, shape.width, shape.height)
            else {
                return;
            };
            let segments = if theme.corner > 0.0 {
                rounded_rect(x, y, w, h, theme.corner, xf)
            } else {
                poly_segments(
                    &[
                        Point { x, y },
                        Point { x: x + w, y },
                        Point { x: x + w, y: y + h },
                        Point { x, y: y + h },
                    ],
                    true,
                    xf,
                )
            };
            push_path(out, segments, &look, shape, ctx, true);
        }
        ShapeType::Polygon => {
            let Some(points) = shape.points.as_deref().filter(|p| p.len() >= 2) else {
                return;
            };
            push_path(
                out,
                poly_segments(points, true, xf),
                &look,
                shape,
                ctx,
                true,
            );
        }
        ShapeType::Path => {
            let Some(d) = shape.path.as_deref() else {
                return;
            };
            match parse_path_data(d, xf) {
                Some(segments) => push_path(out, segments, &look, shape, ctx, true),
                None => gap_once("path-data", "malformed path data — skipped"),
            }
        }
        ShapeType::Text => {
            let (Some(x), Some(y), Some(text)) = (shape.x, shape.y, shape.text.as_deref()) else {
                return;
            };
            if text.is_empty() {
                return;
            }
            let verbatim = shape.text_case.as_deref() == Some("verbatim");
            let text = if theme.label_uppercase && !verbatim {
                text.to_uppercase()
            } else {
                text.to_string()
            };
            let px = shape.font_size.unwrap_or_else(|| {
                theme.text_px(shape.text_size.as_deref().unwrap_or(&theme.label_size))
            });
            let (tx, ty) = xf.p(x, y);
            out.push(DrawCommand::Text {
                color_runs: Vec::new(),
                rotation: 0.0,
                text,
                pos: Vec2 { x: tx, y: ty },
                size: px * xf.s,
                color: look.stroke,
                align: match shape.align.unwrap_or_default() {
                    Align::Left => TextAlign::Left,
                    Align::Center => TextAlign::Center,
                    Align::Right => TextAlign::Right,
                },
                weight: theme.label_weight,
                family: orbital_canvas_base::packet::TextFamily::Sans,
                halo: Some(Halo {
                    color: theme.ground,
                    width: (theme.text_px("xs") * 0.3).max(2.0) * xf.s,
                }),
            });
        }
        ShapeType::VennRegion => {
            if let Some(cmd) = venn_region(shape, ctx) {
                out.push(cmd);
            }
        }
    }
}

/// The intersection of the `inside` circles minus the union of the `outside` circles, filled
/// with the region's fill (or its stroke color), as `LearningCanvas` composites it.
fn venn_region(shape: &Shape, ctx: &PaintCtx) -> Option<DrawCommand> {
    let circles = |ids: &Option<Vec<String>>| -> Vec<Vec<PathSegment>> {
        ids.iter()
            .flatten()
            .filter_map(|id| {
                let c = ctx
                    .siblings
                    .iter()
                    .find(|s| s.ty == ShapeType::Circle && s.id.as_deref() == Some(id.as_str()))?;
                let (x, y) = ctx.xf.p(c.x?, c.y?);
                let r = c.radius? * ctx.xf.s;
                Some(vec![PathSegment::Arc {
                    cx: x,
                    cy: y,
                    rx: r,
                    ry: r,
                    start: 0.0,
                    end: TAU,
                }])
            })
            .collect()
    };
    let inside = circles(&shape.inside);
    if inside.is_empty() {
        return None;
    }
    let theme = ctx.theme;
    let opacity = shape.opacity.unwrap_or(1.0);
    let fill_value = shape.fill.as_deref().or(shape.fill_tone.as_deref());
    let color = match fill_value {
        Some(v) => theme.mark_color(Some(v), "fill"),
        None => theme.mark_color(
            shape.color.as_deref().or(shape.tone.as_deref()),
            default_tone(shape.ty),
        ),
    };
    let style = fill_style_of(shape, theme);
    let alpha = if style == FillStyle::Tint {
        opacity * theme.fill_opacity * 2.0
    } else {
        opacity
    };
    let (x0, y0) = ctx.xf.p(0.0, 0.0);
    let (x1, y1) = ctx.xf.p(ctx.width, ctx.height);
    Some(DrawCommand::ClippedPath {
        segments: vec![
            PathSegment::MoveTo { x: x0, y: y0 },
            PathSegment::LineTo { x: x1, y: y0 },
            PathSegment::LineTo { x: x1, y: y1 },
            PathSegment::LineTo { x: x0, y: y1 },
            PathSegment::Close,
        ],
        fill: Some(with_alpha(color, alpha)),
        stroke: None,
        clip: inside,
        cut: circles(&shape.outside),
    })
}

/// `shapeBounds`: the hit box of a shape in scene units, with the web's slop.
pub fn bounds(shape: &Shape) -> Option<Rect> {
    match shape.ty {
        ShapeType::Line | ShapeType::Arrow => {
            let (x1, y1, x2, y2) = (shape.x1?, shape.y1?, shape.x2?, shape.y2?);
            Some(Rect {
                x: x1.min(x2) - 6.0,
                y: y1.min(y2) - 6.0,
                w: (x2 - x1).abs() + 12.0,
                h: (y2 - y1).abs() + 12.0,
            })
        }
        ShapeType::Circle => {
            let (x, y, r) = (shape.x?, shape.y?, shape.radius?);
            Some(Rect {
                x: x - r - 4.0,
                y: y - r - 4.0,
                w: r * 2.0 + 8.0,
                h: r * 2.0 + 8.0,
            })
        }
        ShapeType::Ellipse => {
            let (x, y, w, h) = (shape.x?, shape.y?, shape.width?, shape.height?);
            Some(Rect {
                x: x - w / 2.0 - 4.0,
                y: y - h / 2.0 - 4.0,
                w: w + 8.0,
                h: h + 8.0,
            })
        }
        ShapeType::Rect => {
            let (x, y, w, h) = (shape.x?, shape.y?, shape.width?, shape.height?);
            Some(Rect {
                x: x - 4.0,
                y: y - 4.0,
                w: w + 8.0,
                h: h + 8.0,
            })
        }
        ShapeType::Polygon => {
            let pts = shape.points.as_deref().filter(|p| !p.is_empty())?;
            let min_x = pts.iter().map(|p| p.x).fold(f32::INFINITY, f32::min);
            let max_x = pts.iter().map(|p| p.x).fold(f32::NEG_INFINITY, f32::max);
            let min_y = pts.iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
            let max_y = pts.iter().map(|p| p.y).fold(f32::NEG_INFINITY, f32::max);
            Some(Rect {
                x: min_x - 4.0,
                y: min_y - 4.0,
                w: max_x - min_x + 8.0,
                h: max_y - min_y + 8.0,
            })
        }
        ShapeType::Text => {
            let (x, y) = (shape.x?, shape.y?);
            let fs = shape.font_size.unwrap_or(14.0);
            Some(Rect {
                x: x - 4.0,
                y: y - fs - 4.0,
                w: 120.0,
                h: fs + 8.0,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx(theme: &DiagramTheme) -> PaintCtx<'_> {
        PaintCtx {
            theme,
            xf: Xf::identity(),
            siblings: &[],
            width: 200.0,
            height: 100.0,
        }
    }

    fn paint_one(shape: &Shape) -> Vec<DrawCommand> {
        let theme = DiagramTheme::heritage(false);
        let mut out = Vec::new();
        paint(shape, &ctx(&theme), &mut out);
        out
    }

    #[test]
    fn a_line_strokes_one_path_in_the_ink_tone_at_normal_weight() {
        let cmds = paint_one(&Shape::line(0.0, 0.0, 10.0, 0.0));
        let [DrawCommand::Path {
            fill,
            stroke: Some(s),
            ..
        }] = cmds.as_slice()
        else {
            panic!("{cmds:?}")
        };
        let theme = DiagramTheme::heritage(false);
        assert!(fill.is_none());
        assert_eq!(s.color, theme.tone("ink").unwrap());
        assert_eq!(s.width, 1.5);
        assert!(s.dash.is_empty());
    }

    #[test]
    fn color_stroke_role_and_dash_each_change_the_stroke() {
        let theme = DiagramTheme::heritage(false);
        let cmds = paint_one(
            &Shape::line(0.0, 0.0, 10.0, 0.0)
                .color("series-2")
                .stroke_weight("bold")
                .dash(Some(Dash::Dashed)),
        );
        let DrawCommand::Path {
            stroke: Some(s), ..
        } = &cmds[0]
        else {
            panic!()
        };
        assert_eq!(s.color, theme.tone("series-2").unwrap());
        assert_eq!(s.width, 2.5);
        assert_eq!(s.dash, vec![6.0, 4.0]);
        let dotted = paint_one(&Shape::line(0.0, 0.0, 10.0, 0.0).dash(Some(Dash::Dotted)));
        let DrawCommand::Path {
            stroke: Some(d), ..
        } = &dotted[0]
        else {
            panic!()
        };
        assert_eq!(d.dash, vec![1.0, 4.0]);
    }

    #[test]
    fn a_literal_fill_is_solid_a_tone_fill_is_a_tint() {
        let theme = DiagramTheme::heritage(false);
        let solid = paint_one(&Shape::circle(5.0, 5.0, 3.0).fill("#ff0000"));
        let DrawCommand::Path { fill: Some(f), .. } = &solid[0] else {
            panic!()
        };
        assert_eq!(f[3], 1.0);
        let tint = paint_one(&Shape::circle(5.0, 5.0, 3.0).fill("series-1"));
        let DrawCommand::Path { fill: Some(f), .. } = &tint[0] else {
            panic!()
        };
        assert!(
            (f[3] - theme.fill_opacity).abs() < 1e-6,
            "tint alpha {}",
            f[3]
        );
        let outline = paint_one(
            &Shape::circle(5.0, 5.0, 3.0)
                .fill("series-1")
                .fill_style("outline"),
        );
        let DrawCommand::Path {
            fill,
            stroke: Some(s),
            ..
        } = &outline[0]
        else {
            panic!()
        };
        assert!(fill.is_none(), "outline style paints no fill");
        assert_eq!(
            s.color,
            theme.tone("series-1").unwrap(),
            "outline stroke takes the fill's color"
        );
        assert_eq!(s.width, 2.5, "and is drawn bold");
    }

    #[test]
    fn opacity_scales_stroke_and_fill_alpha() {
        let cmds = paint_one(&Shape::circle(5.0, 5.0, 3.0).fill("#ff0000").opacity(0.5));
        let DrawCommand::Path {
            fill: Some(f),
            stroke: Some(s),
            ..
        } = &cmds[0]
        else {
            panic!()
        };
        assert_eq!(f[3], 0.5);
        assert_eq!(s.color[3], 0.5);
    }

    #[test]
    fn an_arrow_adds_a_filled_triangle_head_at_its_tip() {
        let cmds = paint_one(&Shape::arrow(0.0, 0.0, 50.0, 0.0));
        assert_eq!(cmds.len(), 2);
        let DrawCommand::Path {
            segments,
            fill: Some(_),
            ..
        } = &cmds[1]
        else {
            panic!("{:?}", cmds[1])
        };
        let PathSegment::MoveTo { x, y } = segments[0] else {
            panic!()
        };
        assert_eq!((x, y), (50.0, 0.0));
        assert_eq!(segments.len(), 4);
        let plain = paint_one(&Shape::line(0.0, 0.0, 50.0, 0.0));
        assert_eq!(plain.len(), 1, "control: a line has no head");
    }

    #[test]
    fn missing_geometry_paints_nothing() {
        assert!(paint_one(&Shape::new(ShapeType::Circle)).is_empty());
        assert!(paint_one(&Shape::new(ShapeType::Rect)).is_empty());
        assert!(paint_one(&Shape::polygon(vec![Point { x: 0.0, y: 0.0 }])).is_empty());
        assert!(paint_one(&Shape::new(ShapeType::Text)).is_empty());
    }

    #[test]
    fn grid_and_axis_span_the_scene_and_axis_follows_its_direction() {
        let grid = paint_one(&Shape {
            step: Some(50.0),
            ..Shape::new(ShapeType::Grid)
        });
        let DrawCommand::Path {
            segments,
            stroke: Some(s),
            ..
        } = &grid[0]
        else {
            panic!()
        };
        assert_eq!(
            segments.len(),
            2 * (5 + 3),
            "5 vertical + 3 horizontal lines"
        );
        assert_eq!(s.width, 1.0, "grid draws thin");
        let x = paint_one(&Shape::new(ShapeType::Axis));
        let DrawCommand::Path { segments, .. } = &x[0] else {
            panic!()
        };
        assert!(matches!(segments[1], PathSegment::LineTo { x, y } if x == 200.0 && y == 50.0));
        let y = paint_one(&Shape {
            axis: Some("y".into()),
            ..Shape::new(ShapeType::Axis)
        });
        let DrawCommand::Path { segments, .. } = &y[0] else {
            panic!()
        };
        assert!(matches!(segments[1], PathSegment::LineTo { x, y } if x == 100.0 && y == 100.0));
    }

    #[test]
    fn text_carries_alignment_label_color_halo_and_case() {
        let cmds = paint_one(&Shape::text(10.0, 20.0, "mg").align(Align::Right).verbatim());
        let DrawCommand::Text {
            text,
            pos,
            size,
            align,
            weight,
            halo,
            color,
            ..
        } = &cmds[0]
        else {
            panic!()
        };
        let theme = DiagramTheme::heritage(false);
        assert_eq!(
            (text.as_str(), pos.x, pos.y, *size),
            ("mg", 10.0, 20.0, 14.0)
        );
        assert_eq!(*align, TextAlign::Right);
        assert_eq!(*weight, 600);
        assert_eq!(*color, theme.tone("label").unwrap());
        assert_eq!(halo.unwrap().color, theme.ground);
        let sized = paint_one(&Shape::text(0.0, 0.0, "x").text_size("xs"));
        let DrawCommand::Text { size, .. } = &sized[0] else {
            panic!()
        };
        assert_eq!(*size, 12.0);
        let literal = paint_one(&Shape::text(0.0, 0.0, "x").font_size(Some(9.0)));
        let DrawCommand::Text { size, .. } = &literal[0] else {
            panic!()
        };
        assert_eq!(*size, 9.0);
        let mut upper = DiagramTheme::heritage(false);
        upper.label_uppercase = true;
        let mut out = Vec::new();
        paint(&Shape::text(0.0, 0.0, "ab"), &ctx(&upper), &mut out);
        paint(
            &Shape::text(0.0, 0.0, "ab").verbatim(),
            &ctx(&upper),
            &mut out,
        );
        let texts: Vec<_> = out
            .iter()
            .map(|c| match c {
                DrawCommand::Text { text, .. } => text.clone(),
                _ => String::new(),
            })
            .collect();
        assert_eq!(texts, vec!["AB".to_string(), "ab".to_string()]);
    }

    fn venn(inside: &[&str], outside: &[&str]) -> Shape {
        let mut v: Shape = orbital_core_base::json_value::from_json_value(
            &(json!({"type": "venn-region", "fill": "series-2", "fillStyle": "solid"})),
        )
        .unwrap();
        v.inside = Some(inside.iter().map(|s| s.to_string()).collect());
        v.outside = Some(outside.iter().map(|s| s.to_string()).collect());
        v
    }

    #[test]
    fn a_venn_region_clips_to_its_inside_circles_and_cuts_its_outside_circles() {
        let theme = DiagramTheme::heritage(false);
        let siblings = vec![
            Shape::circle(40.0, 50.0, 30.0).id("a"),
            Shape::circle(80.0, 50.0, 30.0).id("b"),
            Shape::circle(120.0, 50.0, 30.0).id("c"),
        ];
        let ctx = PaintCtx {
            siblings: &siblings,
            ..ctx(&theme)
        };
        let mut out = Vec::new();
        paint(&venn(&["a", "b"], &["c"]), &ctx, &mut out);
        let [DrawCommand::ClippedPath {
            clip, cut, fill, ..
        }] = out.as_slice()
        else {
            panic!("one clipped region: {out:?}")
        };
        assert_eq!((clip.len(), cut.len()), (2, 1));
        assert_eq!(*fill, Some(theme.tone("series-2").unwrap()));
    }

    #[test]
    fn control_a_venn_region_naming_no_circle_draws_nothing() {
        let theme = DiagramTheme::heritage(false);
        let siblings = vec![Shape::rect(0.0, 0.0, 10.0, 10.0).id("a")];
        let ctx = PaintCtx {
            siblings: &siblings,
            ..ctx(&theme)
        };
        let mut out = Vec::new();
        paint(&venn(&["a", "missing"], &[]), &ctx, &mut out);
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn a_hatch_fill_is_clipped_lines_plus_the_outline_and_a_tint_is_one_filled_path() {
        let theme = DiagramTheme::heritage(false);
        let mut out = Vec::new();
        paint(
            &Shape::rect(10.0, 10.0, 40.0, 20.0)
                .fill("series-1")
                .fill_style("hatch"),
            &ctx(&theme),
            &mut out,
        );
        assert!(
            matches!(out.as_slice(), [DrawCommand::ClippedPath { clip, fill: None, stroke: Some(_), .. }, DrawCommand::Path { fill: None, .. }] if clip.len() == 1),
            "{out:?}"
        );
        let mut tint = Vec::new();
        paint(
            &Shape::rect(10.0, 10.0, 40.0, 20.0)
                .fill("series-1")
                .fill_style("tint"),
            &ctx(&theme),
            &mut tint,
        );
        assert!(
            matches!(tint.as_slice(), [DrawCommand::Path { fill: Some(_), .. }]),
            "{tint:?}"
        );
    }

    #[test]
    fn the_scene_transform_scales_geometry_widths_and_text() {
        let theme = DiagramTheme::heritage(false);
        let ctx = PaintCtx {
            theme: &theme,
            siblings: &[],
            xf: Xf {
                s: 2.0,
                ox: 10.0,
                oy: 0.0,
            },
            width: 200.0,
            height: 100.0,
        };
        let mut out = Vec::new();
        paint(&Shape::line(0.0, 0.0, 5.0, 0.0), &ctx, &mut out);
        paint(&Shape::text(1.0, 1.0, "a"), &ctx, &mut out);
        let DrawCommand::Path {
            segments,
            stroke: Some(s),
            ..
        } = &out[0]
        else {
            panic!()
        };
        assert!(matches!(segments[1], PathSegment::LineTo { x, .. } if x == 20.0));
        assert_eq!(s.width, 3.0);
        let DrawCommand::Text { pos, size, .. } = &out[1] else {
            panic!()
        };
        assert_eq!((pos.x, size), (12.0, &28.0));
    }

    #[test]
    fn shapes_deserialize_from_web_json_and_invalid_entries_are_dropped() {
        let shapes = parse_shapes(Some(&json!([
            {"type": "circle", "x": 1, "y": 2, "radius": 3, "fill": "series-1", "fillStyle": "solid", "lineWidth": 2},
            {"type": "no-such-shape"},
            {"type": "text", "x": 0, "y": 0, "text": "θ", "textSize": "xs", "textCase": "verbatim"},
        ])));
        assert_eq!(shapes.len(), 2);
        assert_eq!(shapes[0].fill_style.as_deref(), Some("solid"));
        assert_eq!(shapes[0].line_width, Some(2.0));
        assert_eq!(shapes[1].text_case.as_deref(), Some("verbatim"));
        assert!(parse_shapes(None).is_empty());
    }

    #[test]
    fn hit_bounds_match_the_web_slop() {
        let b = bounds(&Shape::circle(10.0, 10.0, 5.0)).unwrap();
        assert_eq!((b.x, b.y, b.w, b.h), (1.0, 1.0, 18.0, 18.0));
        let l = bounds(&Shape::line(0.0, 0.0, 10.0, 4.0)).unwrap();
        assert_eq!((l.x, l.y, l.w, l.h), (-6.0, -6.0, 22.0, 16.0));
        assert!(bounds(&Shape::new(ShapeType::Axis)).is_none());
    }
}
