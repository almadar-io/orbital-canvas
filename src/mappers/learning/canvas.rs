//! The `LearningCanvas` atom: shapes (plus the readout chips and trace panels
//! it derives itself) laid out in a `width` x `height` scene and fitted into the
//! host viewport. Every learning molecule renders through this.

use super::shape::{self, PaintCtx, Point, Shape, ShapeType};
use super::theme::DiagramTheme;
use super::Xf;
use crate::mappers::game_canvas_2d::{get_f32, get_string};
use crate::mappers::scene::{Emit, Hit, MappedScene};
use crate::packet::{DrawCommand, GameRenderPacket, Rect};
use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub struct Readout {
    pub label: String,
    pub value: String,
    pub color: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TraceSeries {
    pub samples: Vec<Point>,
    pub color: Option<String>,
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TracePanel {
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub series: Vec<TraceSeries>,
    pub x_label: Option<String>,
    pub y_label: Option<String>,
    pub frame_color: Option<String>,
    pub background_color: Option<String>,
    pub background_opacity: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct CanvasSpec {
    pub pattern: &'static str,
    pub width: f32,
    pub height: f32,
    pub background: Option<String>,
    pub shapes: Vec<Shape>,
    pub readouts: Vec<Readout>,
    pub traces: Vec<TracePanel>,
    pub interactive: bool,
    pub on_shape_click: Option<String>,
}

fn num_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n
            .as_i64()
            .map(|i| i.to_string())
            .or_else(|| n.as_f64().map(|f| f.to_string()))
            .unwrap_or_default(),
        Value::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

pub fn parse_readouts(v: Option<&Value>) -> Vec<Readout> {
    v.and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|r| {
                    Some(Readout {
                        label: get_string(r, "label")?,
                        value: num_text(r.get("value")?),
                        color: get_string(r, "color"),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_points(v: Option<&Value>) -> Vec<Point> {
    v.and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|p| {
                    Some(Point {
                        x: p.get("x")?.as_f64()? as f32,
                        y: p.get("y")?.as_f64()? as f32,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn opt_f32(v: &Value, key: &str) -> Option<f32> {
    v.get(key).and_then(Value::as_f64).map(|f| f as f32)
}

pub fn parse_traces(v: Option<&Value>) -> Vec<TracePanel> {
    v.and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|t| TracePanel {
                    x: opt_f32(t, "x"),
                    y: opt_f32(t, "y"),
                    width: opt_f32(t, "width"),
                    height: opt_f32(t, "height"),
                    series: t
                        .get("series")
                        .and_then(Value::as_array)
                        .map(|ss| {
                            ss.iter()
                                .map(|s| TraceSeries {
                                    samples: parse_points(s.get("samples")),
                                    color: get_string(s, "color"),
                                    label: get_string(s, "label"),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    x_label: get_string(t, "xLabel"),
                    y_label: get_string(t, "yLabel"),
                    frame_color: get_string(t, "frameColor"),
                    background_color: get_string(t, "backgroundColor"),
                    background_opacity: opt_f32(t, "backgroundOpacity"),
                })
                .collect()
        })
        .unwrap_or_default()
}

impl CanvasSpec {
    /// The atom-level props every molecule forwards: size, ground, chips,
    /// sparklines and the click wiring.
    pub fn from_props(
        pattern: &'static str,
        props: &Value,
        default_w: f32,
        default_h: f32,
        shapes: Vec<Shape>,
    ) -> Self {
        Self {
            pattern,
            width: get_f32(props, "width", default_w).max(1.0),
            height: get_f32(props, "height", default_h).max(1.0),
            background: get_string(props, "backgroundColor").filter(|c| !c.is_empty()),
            shapes,
            readouts: parse_readouts(props.get("readouts")),
            traces: parse_traces(props.get("traces")),
            interactive: props
                .get("interactive")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            on_shape_click: get_string(props, "onShapeClick"),
        }
    }
}

/// The theme a scene draws with: heritage for its color scheme, an optional
/// resolved `canvasTheme`, then the molecule's `series` palette override.
pub fn theme_for(props: &Value) -> DiagramTheme {
    let series: Vec<String> = props
        .get("series")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    DiagramTheme::from_scene(props).with_series(&series)
}

/// `readoutShapes`: status chips along the top-right edge.
pub fn readout_shapes(readouts: &[Readout], width: f32, theme: &DiagramTheme) -> Vec<Shape> {
    let mut out = Vec::new();
    let px = theme.text_px("xs");
    let chip_h = (px * 1.9).round();
    let pad = (px * 0.75).round();
    let dot = (px * 0.3).round().max(3.0);
    let gap = 6.0;
    let char_w = px * 0.62;
    let mut right_edge = width - 8.0;
    let mut row_y = 8.0;
    for r in readouts {
        let text = format!("{} {}", r.label, r.value);
        let chip_w = (text.chars().count() as f32 * char_w + pad * 2.0 + dot * 3.0)
            .round()
            .max(40.0);
        let mut chip_x = right_edge - chip_w;
        if chip_x < 8.0 {
            row_y += chip_h + 4.0;
            right_edge = width - 8.0;
            chip_x = right_edge - chip_w;
        }
        let ground = rgba_css(theme.ground);
        out.push(
            Shape::rect(chip_x, row_y, chip_w, chip_h)
                .tone("grid")
                .fill(ground)
                .fill_style("solid")
                .stroke_weight("thin")
                .opacity(0.94),
        );
        let accent = r.color.clone().unwrap_or_else(|| "highlight".into());
        out.push(
            Shape::circle(chip_x + pad + dot, row_y + chip_h / 2.0, dot)
                .color(accent.clone())
                .fill(accent)
                .fill_style("solid")
                .stroke_weight("thin"),
        );
        out.push(
            Shape::text(chip_x + pad + dot * 3.0, row_y + chip_h / 2.0, text)
                .tone("ink")
                .verbatim()
                .text_size("xs"),
        );
        right_edge = chip_x - gap;
    }
    out
}

/// A color as a literal the shape painter parses back.
fn rgba_css(c: [f32; 4]) -> String {
    format!(
        "rgba({},{},{},{})",
        (c[0] * 255.0).round(),
        (c[1] * 255.0).round(),
        (c[2] * 255.0).round(),
        c[3]
    )
}

/// `traceShapes`: one sparkline inset.
pub fn trace_shapes(
    panel: &TracePanel,
    k: usize,
    width: f32,
    height: f32,
    theme: &DiagramTheme,
) -> Vec<Shape> {
    let w = panel.width.unwrap_or_else(|| (width * 0.32).round());
    let h = panel.height.unwrap_or_else(|| (height * 0.28).round());
    let x = panel.x.unwrap_or(width - w - 8.0);
    let y = panel.y.unwrap_or(height - h - 8.0 - k as f32 * (h + 8.0));
    let all: Vec<&Point> = panel.series.iter().flat_map(|s| s.samples.iter()).collect();
    if all.is_empty() {
        return Vec::new();
    }
    let fold = |f: fn(&Point) -> f32| {
        let lo = all.iter().map(|p| f(p)).fold(f32::INFINITY, f32::min);
        let hi = all.iter().map(|p| f(p)).fold(f32::NEG_INFINITY, f32::max);
        if lo == hi {
            (lo - 1.0, hi + 1.0)
        } else {
            (lo, hi)
        }
    };
    let (x_lo, x_hi) = fold(|p| p.x);
    let (y_lo, y_hi) = fold(|p| p.y);

    let mut out = Vec::new();
    let ground = panel
        .background_color
        .clone()
        .unwrap_or_else(|| rgba_css(theme.ground));
    out.push(
        Shape::rect(x, y, w, h)
            .color(panel.frame_color.clone().unwrap_or_else(|| "grid".into()))
            .fill(ground)
            .fill_style("solid")
            .stroke_weight("thin")
            .opacity(panel.background_opacity.unwrap_or(0.92)),
    );
    let show_legend = panel.series.len() > 1 || panel.y_label.is_none();
    for (j, series) in panel.series.iter().enumerate() {
        let color = series
            .color
            .clone()
            .unwrap_or_else(|| DiagramTheme::series_tone(j));
        let mapped: Vec<(f32, f32)> = series
            .samples
            .iter()
            .map(|p| {
                (
                    x + 4.0 + (p.x - x_lo) / (x_hi - x_lo) * (w - 8.0),
                    y + h - 4.0 - (p.y - y_lo) / (y_hi - y_lo) * (h - 8.0),
                )
            })
            .collect();
        for pair in mapped.windows(2) {
            out.push(
                Shape::line(pair[0].0, pair[0].1, pair[1].0, pair[1].1)
                    .color(color.clone())
                    .stroke_weight("normal"),
            );
        }
        if let Some(last) = mapped.last() {
            out.push(
                Shape::circle(last.0, last.1, 2.5)
                    .color(color.clone())
                    .fill(color.clone())
                    .fill_style("solid")
                    .stroke_weight("thin"),
            );
        }
        if let (Some(label), true) = (&series.label, show_legend) {
            let ly = y + 10.0 + 13.0 * j as f32;
            out.push(
                Shape::line(x + 6.0, ly, x + 16.0, ly)
                    .id(format!("trace-swatch-{j}"))
                    .color(color)
                    .stroke_weight("bold"),
            );
            out.push(
                Shape::text(x + 20.0, ly, label.clone())
                    .tone("label")
                    .verbatim()
                    .text_size("xs"),
            );
        }
    }
    if let Some(label) = &panel.y_label {
        out.push(
            Shape::text(x + w - 6.0, y + 10.0, label.clone())
                .tone("label")
                .verbatim()
                .text_size("xs")
                .align(shape::Align::Right),
        );
    }
    if let Some(label) = &panel.x_label {
        out.push(
            Shape::text(x + w - 6.0, y + h - 8.0, label.clone())
                .tone("label")
                .verbatim()
                .text_size("xs")
                .align(shape::Align::Right),
        );
    }
    out
}

fn type_name(ty: ShapeType) -> &'static str {
    match ty {
        ShapeType::Line => "line",
        ShapeType::Arrow => "arrow",
        ShapeType::Circle => "circle",
        ShapeType::Ellipse => "ellipse",
        ShapeType::Rect => "rect",
        ShapeType::Polygon => "polygon",
        ShapeType::Path => "path",
        ShapeType::Text => "text",
        ShapeType::Axis => "axis",
        ShapeType::Grid => "grid",
        ShapeType::VennRegion => "venn-region",
    }
}

/// `{ id?, type, index }` — the web `onShapeClick` payload.
pub fn shape_payload(shape: &Shape, index: usize) -> Value {
    let mut payload = json!({ "type": type_name(shape.ty), "index": index });
    if let Some(id) = &shape.id {
        payload["id"] = Value::String(id.clone());
    }
    payload
}

/// Lay the scene out in the viewport (contain-fit, centered) or at its own size.
pub fn fit(width: f32, height: f32, viewport: Option<(f32, f32)>) -> (Xf, f32, f32) {
    match viewport {
        Some((vw, vh)) => {
            let s = (vw / width).min(vh / height);
            (
                Xf {
                    s,
                    ox: (vw - width * s) / 2.0,
                    oy: (vh - height * s) / 2.0,
                },
                vw,
                vh,
            )
        }
        None => (Xf::identity(), width, height),
    }
}

pub fn render(
    spec: &CanvasSpec,
    theme: &DiagramTheme,
    viewport: Option<(f32, f32)>,
) -> MappedScene {
    let (xf, pw, ph) = fit(spec.width, spec.height, viewport);
    let mut derived: Vec<Shape> = spec.shapes.clone();
    for (k, panel) in spec.traces.iter().enumerate() {
        derived.extend(trace_shapes(panel, k, spec.width, spec.height, theme));
    }
    derived.extend(readout_shapes(&spec.readouts, spec.width, theme));
    let ctx = PaintCtx {
        theme,
        xf,
        siblings: &derived,
        width: spec.width,
        height: spec.height,
    };

    let ground = theme.mark_color(spec.background.as_deref(), "fill");
    let ground = if spec.background.is_some() {
        ground
    } else {
        theme.ground
    };
    let mut draw_commands = vec![DrawCommand::Clear { color: ground }];
    for shape in derived.iter().filter(|s| s.ty != ShapeType::Text) {
        shape::paint(shape, &ctx, &mut draw_commands);
    }
    for shape in derived.iter().filter(|s| s.ty == ShapeType::Text) {
        shape::paint(shape, &ctx, &mut draw_commands);
    }

    let mut hits = Vec::new();
    if spec.interactive {
        for (index, s) in spec.shapes.iter().enumerate() {
            if let Some(b) = shape::bounds(s) {
                let (x, y) = xf.p(b.x, b.y);
                hits.push(Hit {
                    rect: Rect {
                        x,
                        y,
                        w: b.w * xf.s,
                        h: b.h * xf.s,
                    },
                    emits: spec
                        .on_shape_click
                        .iter()
                        .map(|event| Emit {
                            event: event.clone(),
                            arg: "payload".into(),
                            payload: shape_payload(s, index),
                        })
                        .collect(),
                });
            }
        }
    }

    GameRenderPacket {
        pattern: spec.pattern.into(),
        width: pw,
        height: ph,
        draw_commands,
        ..Default::default()
    }
    .into_scene(hits)
}

trait IntoScene {
    fn into_scene(self, hits: Vec<Hit>) -> MappedScene;
}

impl IntoScene for GameRenderPacket {
    fn into_scene(self, hits: Vec<Hit>) -> MappedScene {
        MappedScene {
            packet: self,
            hits,
            ..MappedScene::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(shapes: Vec<Shape>) -> CanvasSpec {
        CanvasSpec {
            pattern: "learning-canvas",
            width: 300.0,
            height: 200.0,
            background: None,
            shapes,
            readouts: vec![],
            traces: vec![],
            interactive: false,
            on_shape_click: None,
        }
    }

    fn theme() -> DiagramTheme {
        DiagramTheme::heritage(false)
    }

    #[test]
    fn fit_letterboxes_by_the_tighter_axis_and_centers() {
        let (xf, w, h) = fit(300.0, 200.0, Some((600.0, 800.0)));
        assert_eq!((xf.s, xf.ox, xf.oy, w, h), (2.0, 0.0, 200.0, 600.0, 800.0));
        let (xf, ..) = fit(300.0, 200.0, Some((900.0, 200.0)));
        assert_eq!((xf.s, xf.ox, xf.oy), (1.0, 300.0, 0.0));
        let (xf, w, h) = fit(300.0, 200.0, None);
        assert_eq!((xf, w, h), (Xf::identity(), 300.0, 200.0));
    }

    #[test]
    fn the_packet_clears_to_the_ground_and_a_background_overrides_it() {
        let scene = render(&spec(vec![]), &theme(), None);
        let DrawCommand::Clear { color } = scene.packet.draw_commands[0] else {
            panic!()
        };
        assert_eq!(color, theme().ground);
        let mut s = spec(vec![]);
        s.background = Some("#102030".into());
        let scene = render(&s, &theme(), None);
        let DrawCommand::Clear { color } = scene.packet.draw_commands[0] else {
            panic!()
        };
        assert_ne!(color, theme().ground);
    }

    #[test]
    fn text_paints_after_every_other_shape_regardless_of_order() {
        let shapes = vec![Shape::text(5.0, 5.0, "a"), Shape::circle(10.0, 10.0, 4.0)];
        let cmds = render(&spec(shapes), &theme(), None).packet.draw_commands;
        assert!(matches!(cmds[1], DrawCommand::Path { .. }));
        assert!(matches!(cmds[2], DrawCommand::Text { .. }));
    }

    #[test]
    fn readouts_add_a_chip_dot_and_label_each_and_none_adds_nothing() {
        let t = theme();
        assert!(readout_shapes(&[], 300.0, &t).is_empty());
        let chips = readout_shapes(
            &[Readout {
                label: "t".into(),
                value: "1.5".into(),
                color: None,
            }],
            300.0,
            &t,
        );
        let types: Vec<_> = chips.iter().map(|s| s.ty).collect();
        assert_eq!(
            types,
            vec![ShapeType::Rect, ShapeType::Circle, ShapeType::Text]
        );
        assert!(
            chips[0].x.unwrap() + chips[0].width.unwrap() <= 300.0 - 8.0 + 0.5,
            "right-aligned"
        );
        assert_eq!(chips[2].text.as_deref(), Some("t 1.5"));
    }

    #[test]
    fn many_readouts_wrap_to_a_second_row() {
        let t = theme();
        let r = |l: &str| Readout {
            label: l.into(),
            value: "123456".into(),
            color: None,
        };
        let chips = readout_shapes(
            &[r("aaaaaaaa"), r("bbbbbbbb"), r("cccccccc"), r("dddddddd")],
            200.0,
            &t,
        );
        let ys: Vec<f32> = chips
            .iter()
            .filter(|s| s.ty == ShapeType::Rect)
            .map(|s| s.y.unwrap())
            .collect();
        assert!(
            ys.iter().any(|y| *y > ys[0]),
            "a later chip drops a row: {ys:?}"
        );
    }

    #[test]
    fn a_trace_panel_scales_samples_into_its_frame_and_legends_multi_series() {
        let t = theme();
        let series = |label: &str| TraceSeries {
            samples: vec![Point { x: 0.0, y: 0.0 }, Point { x: 10.0, y: 5.0 }],
            color: None,
            label: Some(label.into()),
        };
        let panel = TracePanel {
            x: Some(100.0),
            y: Some(100.0),
            width: Some(80.0),
            height: Some(40.0),
            series: vec![series("a"), series("b")],
            x_label: None,
            y_label: None,
            frame_color: None,
            background_color: None,
            background_opacity: None,
        };
        let shapes = trace_shapes(&panel, 0, 300.0, 200.0, &t);
        let line = shapes
            .iter()
            .find(|s| s.ty == ShapeType::Line && s.id.is_none())
            .unwrap();
        assert_eq!((line.x1, line.x2), (Some(104.0), Some(176.0)));
        assert_eq!((line.y1, line.y2), (Some(136.0), Some(104.0)));
        assert_eq!(shapes.iter().filter(|s| s.ty == ShapeType::Text).count(), 2);
        let single = TracePanel {
            series: vec![series("only")],
            y_label: Some("v".into()),
            ..panel.clone()
        };
        let texts: Vec<_> = trace_shapes(&single, 0, 300.0, 200.0, &t)
            .into_iter()
            .filter(|s| s.ty == ShapeType::Text)
            .collect();
        assert_eq!(texts.len(), 1, "yLabel suppresses the single-series legend");
        assert!(trace_shapes(
            &TracePanel {
                series: vec![],
                ..panel
            },
            0,
            300.0,
            200.0,
            &t
        )
        .is_empty());
    }

    #[test]
    fn taps_resolve_to_the_topmost_shape_with_the_web_payload() {
        let mut s = spec(vec![
            Shape::circle(50.0, 50.0, 10.0).id("a"),
            Shape::circle(52.0, 50.0, 10.0).id("b"),
        ]);
        s.interactive = true;
        s.on_shape_click = Some("PICK".into());
        let scene = render(&s, &theme(), None);
        let emits = scene.emits_at(51.0, 50.0);
        assert_eq!(emits.len(), 1);
        assert_eq!(emits[0].event, "PICK");
        assert_eq!(
            emits[0].payload,
            json!({"id": "b", "type": "circle", "index": 1})
        );
        assert!(
            scene.emits_at(250.0, 150.0).is_empty(),
            "control: a tap on empty space emits nothing"
        );
        s.interactive = false;
        assert!(
            render(&s, &theme(), None).hits.is_empty(),
            "control: not interactive"
        );
        s.interactive = true;
        s.on_shape_click = None;
        let silent = render(&s, &theme(), None);
        assert_eq!(
            silent.hits.len(),
            2,
            "regions still exist for mappers that add their own events"
        );
        assert!(
            silent.emits_at(50.0, 50.0).is_empty(),
            "control: no authored event, nothing to emit"
        );
    }

    #[test]
    fn hit_regions_follow_the_viewport_fit() {
        let mut s = spec(vec![Shape::circle(150.0, 100.0, 10.0)]);
        s.interactive = true;
        s.on_shape_click = Some("TAP".into());
        let scene = render(&s, &theme(), Some((600.0, 800.0)));
        assert_eq!(
            scene.emits_at(300.0, 400.0).len(),
            1,
            "scene center is the viewport center at 2x"
        );
        assert!(
            scene.emits_at(150.0, 100.0).is_empty(),
            "control: unscaled coordinates miss"
        );
    }
}
