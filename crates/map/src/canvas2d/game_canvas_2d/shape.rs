//! `draw-shape`: vector paths, SDF shapes with materials, gradients, fill patterns and strokes.

use super::*;

/// Segments used to approximate an ellipse fill with a polygon fan.
pub(super) const ELLIPSE_SEGMENTS: usize = 24;

/// The transform a `path` shape's `d` is drawn with: its projected position plus `offsetX`/`offsetY`, scaled by the
/// tile width, the zoom and its `scale`.
pub(super) fn path_xf(
    node: &Value,
    projector: &Projector,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
    zoom: f32,
) -> Xf {
    let tw = projector.tile_width;
    let (bx, by) = node
        .get("position")
        .and_then(parse_scene_pos)
        .map_or((0.0, 0.0), |p| projector.project(p));
    let (ox, oy) = to_screen(
        bx + get_f32(node, "offsetX", 0.0) * tw,
        by + get_f32(node, "offsetY", 0.0) * tw,
    );
    Xf {
        s: tw * zoom * get_f32(node, "scale", 1.0),
        ox,
        oy,
    }
}

/// Screen px per authored dash unit: the zoom, and on a `path` shape also its `scale`, so a dash sized to an outline
/// stays on that outline at every size.
pub(super) fn dash_unit(node: &Value, zoom: f32) -> f32 {
    if node.get("shape").and_then(Value::as_str) == Some("path") {
        zoom * get_f32(node, "scale", 1.0)
    } else {
        zoom
    }
}

pub(super) fn animated_shape(node: &Value, epoch_now_ms: f64) -> Option<Value> {
    let animation = node.get("animation")?;
    if !orbital_canvas_base::animate::plays_at(animation, epoch_now_ms) {
        return None;
    }
    let mut animated = node.clone();
    let object = animated.as_object_mut()?;
    for key in [
        "offsetX",
        "offsetY",
        "rotate",
        "opacity",
        "radiusX",
        "radiusY",
        "width",
        "height",
        "strokeWidth",
        "strokeDashOffset",
        "blur",
        "fill",
        "stroke",
    ] {
        if let Some(value) =
            orbital_canvas_base::animate::drawable_track(animation, epoch_now_ms, key)
        {
            object.insert(key.to_owned(), value);
        }
    }
    if let Some(color) =
        orbital_canvas_base::animate::drawable_track(animation, epoch_now_ms, "shadow.color")
    {
        let shadow = object
            .entry("shadow")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(shadow) = shadow.as_object_mut() {
            shadow.insert("color".into(), color);
        }
    }
    if let Some(blur) =
        orbital_canvas_base::animate::drawable_track(animation, epoch_now_ms, "shadow.blur")
    {
        let shadow = object
            .entry("shadow")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(shadow) = shadow.as_object_mut() {
            shadow.insert("blur".into(), blur);
        }
    }
    Some(animated)
}

pub(super) fn shape_gradient(
    source: &Value,
    position: ScenePos,
    projector: &Projector,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
    zoom: f32,
    opacity: f32,
) -> Option<Gradient> {
    let stops = source
        .get("stops")?
        .as_array()?
        .iter()
        .map(|stop| {
            let offset = stop.get("offset")?.as_f64()? as f32;
            let mut color = parse_css_color(stop.get("color")?.as_str()?)?;
            color[3] *= opacity;
            Some(GradientStop { offset, color })
        })
        .collect::<Option<Vec<_>>>()?;
    if stops.is_empty() {
        return None;
    }
    let (origin_x, origin_y) = projector.project(position);
    let point = |key: &str| -> Option<Vec2> {
        let value = source.get(key)?;
        let x = value.get("x")?.as_f64()? as f32;
        let y = value.get("y")?.as_f64()? as f32;
        let (x, y) = to_screen(
            origin_x + x * projector.tile_width,
            origin_y + y * projector.tile_width,
        );
        Some(Vec2 { x, y })
    };
    match source.get("kind")?.as_str()? {
        "linear" => Some(Gradient::Linear {
            from: point("from")?,
            to: point("to")?,
            stops,
        }),
        "radial" => Some(Gradient::Radial {
            center: point("center")?,
            radius: source.get("radius")?.as_f64()? as f32 * projector.tile_width * zoom,
            stops,
        }),
        "conic" => Some(Gradient::Conic {
            center: point("center")?,
            angle: source.get("angle").and_then(Value::as_f64).unwrap_or(0.0) as f32,
            stops,
        }),
        _ => None,
    }
}

pub(super) fn polygon_segments(points: &[Vec2]) -> Vec<PathSegment> {
    let Some(first) = points.first() else {
        return Vec::new();
    };
    let mut segments = vec![PathSegment::MoveTo {
        x: first.x,
        y: first.y,
    }];
    segments.extend(points.iter().skip(1).map(|point| PathSegment::LineTo {
        x: point.x,
        y: point.y,
    }));
    segments.push(PathSegment::Close);
    segments
}

pub(super) fn shape_fill_pattern(source: &Value) -> Option<FillPattern> {
    match source.get("kind")?.as_str()? {
        "noise" => Some(FillPattern::Noise {
            scale: get_f32(source, "scale", 2.0),
            alpha: get_f32(source, "alpha", 0.12),
            color: source
                .get("color")
                .and_then(Value::as_str)
                .and_then(parse_css_color)
                .unwrap_or([0.0, 0.0, 0.0, 1.0]),
        }),
        "image" => Some(FillPattern::Image {
            url: source.get("url")?.as_str()?.to_string(),
            scale: get_f32(source, "scale", 1.0),
        }),
        _ => None,
    }
}

/// How a styled path is painted: its fills, strokes, gradients and pattern, and whether they paint as separate
/// commands (a blurred shape's).
pub(super) struct PathPaint {
    pub(super) fill: Option<[f32; 4]>,
    pub(super) stroke: Option<Stroke>,
    pub(super) fill_gradient: Option<Gradient>,
    pub(super) stroke_gradient: Option<Gradient>,
    pub(super) pattern: Option<FillPattern>,
    pub(super) opacity: f32,
    pub(super) separate_paint: bool,
}

pub(super) fn push_styled_path(
    segments: Vec<PathSegment>,
    paint: PathPaint,
    out: &mut Vec<DrawCommand>,
) {
    let PathPaint {
        fill,
        stroke,
        fill_gradient,
        stroke_gradient,
        pattern,
        opacity,
        separate_paint,
    } = paint;
    if pattern.is_some() || separate_paint {
        if fill.is_some() || fill_gradient.is_some() {
            out.push(DrawCommand::GradientPath {
                segments: segments.clone(),
                fill,
                stroke: None,
                fill_gradient,
                stroke_gradient: None,
            });
        }
        if let Some(pattern) = pattern {
            out.push(DrawCommand::PatternPath {
                segments: segments.clone(),
                pattern,
                opacity,
            });
        }
        if stroke.is_some() || stroke_gradient.is_some() {
            out.push(DrawCommand::GradientPath {
                segments,
                fill: None,
                stroke,
                fill_gradient: None,
                stroke_gradient,
            });
        }
    } else {
        out.push(DrawCommand::GradientPath {
            segments,
            fill,
            stroke,
            fill_gradient,
            stroke_gradient,
        });
    }
}

/// Whether a draw-shape is drawn by the SDF pass: a shape only it draws, or `corners` or a `material`.
pub(super) fn is_sdf_shape(node: &Value, shape: &str) -> bool {
    matches!(shape, "capsule" | "arc" | "polygon")
        || ["corners", "material", "backdrop"]
            .iter()
            .any(|key| node.get(*key).is_some_and(|v| !v.is_null()))
}

pub(super) fn number_array(value: &Value, field: &str) -> Result<Vec<f32>, CanvasSceneError> {
    let items = value
        .as_array()
        .ok_or_else(|| invalid(field, "must be an array of numbers"))?;
    items
        .iter()
        .map(|v| {
            v.as_f64()
                .filter(|f| f.is_finite())
                .map(|f| f as f32)
                .ok_or_else(|| invalid(field, "entries must be finite numbers"))
        })
        .collect()
}

/// The inner outline of a shape with an `inset`: the same shape `gap` px inside, stroke only, radii reduced by the gap.
fn inset_outline(node: &Value, inset: &Value) -> Result<Option<Value>, CanvasSceneError> {
    let stroke = inset
        .get("stroke")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("inset.stroke", "needs a colour"))?;
    let width = inset
        .get("strokeWidth")
        .and_then(Value::as_f64)
        .filter(|w| w.is_finite() && *w >= 0.0)
        .ok_or_else(|| invalid("inset.strokeWidth", "needs a width of zero or more"))?;
    let gap = inset
        .get("gap")
        .and_then(Value::as_f64)
        .filter(|g| g.is_finite() && *g > 0.0)
        .ok_or_else(|| invalid("inset.gap", "needs a gap above zero"))?;
    let shape = node
        .get("shape")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut inner = node.clone();
    let Some(fields) = inner.as_object_mut() else {
        return Ok(None);
    };
    for key in [
        "inset",
        "fill",
        "gradient",
        "strokeGradient",
        "fillPattern",
        "material",
        "shadow",
        "backdrop",
        "blur",
        "hit",
        "strokeDash",
        "strokeDashOffset",
        "animation",
    ] {
        fields.remove(key);
    }
    fields.insert("stroke".into(), Value::String(stroke.to_owned()));
    fields.insert("strokeWidth".into(), width.into());
    let number = |key: &str| node.get(key).and_then(Value::as_f64).unwrap_or(0.0);
    match shape {
        "rect" | "capsule" => {
            let (w, h) = (number("width") - 2.0 * gap, number("height") - 2.0 * gap);
            if w <= 0.0 || h <= 0.0 {
                return Ok(None);
            }
            fields.insert("offsetX".into(), (number("offsetX") + gap).into());
            fields.insert("offsetY".into(), (number("offsetY") + gap).into());
            fields.insert("width".into(), w.into());
            fields.insert("height".into(), h.into());
            if let Some(radius) = node
                .get("corners")
                .and_then(|c| c.get("radius"))
                .and_then(Value::as_array)
            {
                let reduced: Vec<Value> = radius
                    .iter()
                    .map(|r| {
                        r.as_f64()
                            .map_or(Value::Null, |r| (r - gap).max(0.0).into())
                    })
                    .collect();
                if let Some(corners) = fields.get_mut("corners").and_then(Value::as_object_mut) {
                    corners.insert("radius".into(), Value::Array(reduced));
                }
            }
        }
        "ellipse" => {
            let rx = number("radiusX") - gap;
            let ry = node
                .get("radiusY")
                .and_then(Value::as_f64)
                .unwrap_or(number("radiusX"))
                - gap;
            if rx <= 0.0 || ry <= 0.0 {
                return Ok(None);
            }
            fields.insert("radiusX".into(), rx.into());
            fields.insert("radiusY".into(), ry.into());
        }
        other => {
            return Err(invalid(
                "inset",
                format!("`{other}` has no inset (rect, capsule, ellipse)"),
            ))
        }
    }
    Ok(Some(inner))
}

/// Project one draw-shape: the SDF pass takes the shapes that need it, every other shape keeps the vector path.
pub(super) fn paint_shape(
    node: &Value,
    projector: &Projector,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
    zoom: f32,
    epoch_now_ms: f64,
    gathered: &mut Gathered,
    out: &mut Vec<DrawCommand>,
) -> Result<(), CanvasSceneError> {
    if let Some(inset) = node.get("inset").filter(|v| !v.is_null()) {
        let animated = animated_shape(node, epoch_now_ms);
        let resolved = animated.as_ref().unwrap_or(node);
        let inner = inset_outline(resolved, inset)?;
        let mut outer = resolved.clone();
        if let Some(fields) = outer.as_object_mut() {
            fields.remove("inset");
            fields.remove("animation");
        }
        paint_shape(
            &outer,
            projector,
            to_screen,
            zoom,
            epoch_now_ms,
            gathered,
            out,
        )?;
        if let Some(inner) = inner {
            paint_shape(
                &inner,
                projector,
                to_screen,
                zoom,
                epoch_now_ms,
                gathered,
                out,
            )?;
        }
        return Ok(());
    }
    let sdf = node
        .get("shape")
        .and_then(Value::as_str)
        .is_some_and(|shape| is_sdf_shape(node, shape));
    if !sdf {
        paint_vector_shape(
            node,
            projector,
            to_screen,
            zoom,
            epoch_now_ms,
            gathered,
            out,
        );
        return Ok(());
    }
    let animated = animated_shape(node, epoch_now_ms);
    paint_sdf_shape(
        animated.as_ref().unwrap_or(node),
        projector,
        to_screen,
        zoom,
        epoch_now_ms,
        gathered,
        out,
    )
}

pub(super) fn paint_sdf_shape(
    node: &Value,
    projector: &Projector,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
    zoom: f32,
    epoch_now_ms: f64,
    gathered: &mut Gathered,
    out: &mut Vec<DrawCommand>,
) -> Result<(), CanvasSceneError> {
    let shape = node
        .get("shape")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some(position) = node.get("position").and_then(parse_scene_pos) else {
        gap_once("shape-position", "draw-shape without position");
        return Ok(());
    };
    if ["gradient", "strokeGradient", "fillPattern"]
        .iter()
        .any(|key| node.get(*key).is_some_and(|v| !v.is_null()))
        || get_f32(node, "blur", 0.0) > 0.0
        || node
            .get("blendMode")
            .and_then(Value::as_str)
            .is_some_and(|mode| mode != "source-over")
    {
        gap_once(
            "sdf-style",
            "an SDF draw-shape carries no gradient, fillPattern, blur or blendMode",
        );
        return Ok(());
    }
    let kind = match shape {
        "rect" => SdfKind::Rect,
        "capsule" => SdfKind::Capsule,
        "arc" => SdfKind::Arc,
        "polygon" => SdfKind::Polygon,
        "ellipse" => SdfKind::Ellipse,
        other => {
            return Err(invalid(
                "shape",
                format!("`{other}` has no SDF form (rect, capsule, arc, polygon, ellipse)"),
            ))
        }
    };
    let tw = projector.tile_width;
    let opacity = get_f32(node, "opacity", 1.0);
    let mut rect = if kind == SdfKind::Ellipse {
        let anchor = node
            .get("anchor")
            .and_then(Value::as_str)
            .and_then(Anchor::parse)
            .unwrap_or(Anchor::Ground);
        let (px, py) = projector.anchor_point(position, anchor);
        let cx = px + get_f32(node, "offsetX", 0.0) * tw;
        let cy = py + get_f32(node, "offsetY", 0.0) * tw;
        let rx = get_f32(node, "radiusX", 0.0) * tw;
        let ry = node
            .get("radiusY")
            .and_then(Value::as_f64)
            .map(|v| v as f32 * tw)
            .unwrap_or(rx);
        let (cx, cy) = to_screen(cx, cy);
        Rect {
            x: cx - rx * zoom,
            y: cy - ry * zoom,
            w: 2.0 * rx * zoom,
            h: 2.0 * ry * zoom,
        }
    } else {
        let anchor = node
            .get("anchor")
            .and_then(Value::as_str)
            .and_then(Anchor::parse)
            .unwrap_or(Anchor::TopLeft);
        let (px, py) = projector.anchor_point(position, anchor);
        let x = px + get_f32(node, "offsetX", 0.0) * tw;
        let y = py + get_f32(node, "offsetY", 0.0) * tw;
        let (sx, sy) = to_screen(x, y);
        Rect {
            x: sx,
            y: sy,
            w: get_f32(node, "width", 0.0) * tw * zoom,
            h: get_f32(node, "height", 0.0) * tw * zoom,
        }
    };

    let (corners, corner_kind) = match node.get("corners").filter(|c| !c.is_null()) {
        None => ([0.0; 4], 0),
        Some(spec) => {
            if kind != SdfKind::Rect {
                return Err(invalid("corners", "corners apply to rect shapes only"));
            }
            let radius = number_array(
                spec.get("radius")
                    .ok_or_else(|| invalid("corners.radius", "needs four radii"))?,
                "corners.radius",
            )?;
            if radius.len() != 4 || radius.iter().any(|r| *r < 0.0) {
                return Err(invalid(
                    "corners.radius",
                    "needs four radii of zero or more (top-left, top-right, bottom-right, bottom-left)",
                ));
            }
            let corner_kind = match spec.get("kind").and_then(Value::as_str).unwrap_or("round") {
                "round" => 0,
                "bevel" => 1,
                "notch" => 2,
                "scoop" => 3,
                other => {
                    return Err(invalid(
                        "corners.kind",
                        format!("`{other}` is not round, bevel, notch or scoop"),
                    ))
                }
            };
            (
                [
                    radius[0] * zoom,
                    radius[1] * zoom,
                    radius[2] * zoom,
                    radius[3] * zoom,
                ],
                corner_kind,
            )
        }
    };
    let arc = if kind == SdfKind::Arc {
        let thickness = node
            .get("thickness")
            .and_then(Value::as_f64)
            .filter(|t| t.is_finite() && *t > 0.0)
            .ok_or_else(|| invalid("thickness", "an arc needs a thickness in px above zero"))?;
        [
            get_f32(node, "startAngle", 0.0),
            get_f32(node, "sweep", std::f32::consts::TAU),
            thickness as f32 * zoom,
        ]
    } else {
        [0.0; 3]
    };
    let polygon = if kind == SdfKind::Polygon {
        let sides = node
            .get("sides")
            .and_then(Value::as_f64)
            .filter(|s| s.fract() == 0.0 && (3.0..=64.0).contains(s))
            .ok_or_else(|| invalid("sides", "a polygon needs a whole number of 3 to 64 sides"))?;
        let ratio = get_f32(node, "starRatio", 1.0);
        if !(ratio > 0.0 && ratio <= 1.0) {
            return Err(invalid("starRatio", "must be above 0 and at most 1"));
        }
        [sides as f32, ratio]
    } else {
        [0.0; 2]
    };

    let material = match node.get("material").filter(|m| !m.is_null()) {
        None => None,
        Some(spec) => {
            let wgsl = spec
                .get("wgsl")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("material.wgsl", "needs the program's WGSL text"))?;
            let params = match spec.get("params").filter(|p| !p.is_null()) {
                None => Vec::new(),
                Some(params) => number_array(params, "material.params")?,
            };
            if params.len() > 8 {
                return Err(invalid("material.params", "at most 8 parameters"));
            }
            orbital_canvas_base::shaders::check_material(wgsl)
                .map_err(|detail| invalid("material.wgsl", detail))?;
            let anchor_ms = match spec.get("anchor").filter(|a| !a.is_null()) {
                None => None,
                Some(anchor) => {
                    Some(anchor.as_f64().filter(|a| a.is_finite()).ok_or_else(|| {
                        invalid(
                            "material.anchor",
                            "must be a finite number of epoch milliseconds",
                        )
                    })?)
                }
            };
            let since = match anchor_ms {
                Some(anchor) if epoch_now_ms > 0.0 => ((epoch_now_ms - anchor) / 1000.0) as f32,
                _ => 0.0,
            };
            Some(SdfMaterial {
                wgsl: wgsl.to_owned(),
                params,
                live: spec.get("live").and_then(Value::as_bool).unwrap_or(false),
                since,
                anchor_ms,
                targets: crate::layer_props::material_targets(spec)?,
            })
        }
    };

    let fill = node
        .get("fill")
        .and_then(Value::as_str)
        .and_then(parse_css_color);
    let stroke = node
        .get("stroke")
        .and_then(Value::as_str)
        .and_then(parse_css_color)
        .map(|color| {
            let dash = node
                .get("strokeDash")
                .and_then(Value::as_array)
                .map(|dash| dash.iter().filter_map(Value::as_f64).collect::<Vec<_>>())
                .unwrap_or_default();
            let dash = match dash.as_slice() {
                [] => [0.0, 0.0],
                [one] => [*one as f32 * zoom, *one as f32 * zoom],
                [on, off, ..] => [*on as f32 * zoom, *off as f32 * zoom],
            };
            SdfStroke {
                color,
                width: get_f32(node, "strokeWidth", 1.0) * zoom,
                dash,
                dash_offset: get_f32(node, "strokeDashOffset", 0.0) * zoom,
            }
        });
    let shadow = match node.get("shadow").filter(|s| !s.is_null()) {
        None => None,
        Some(spec) => {
            let color = spec
                .get("color")
                .and_then(Value::as_str)
                .and_then(parse_css_color);
            match (color, spec.get("blur").and_then(Value::as_f64)) {
                (Some(color), Some(blur)) if blur.is_finite() && blur >= 0.0 => {
                    let offset = Vec2 {
                        x: get_f32(spec, "offsetX", 0.0) * tw * zoom,
                        y: get_f32(spec, "offsetY", 0.0) * tw * zoom,
                    };
                    let moved = offset.x != 0.0 || offset.y != 0.0;
                    // a hard offset shadow still needs a sigma: half a pixel keeps its edge crisp
                    ((blur > 0.0 || moved) && color[3] > 0.0).then(|| SdfShadow {
                        color,
                        sigma: (blur as f32 * tw * zoom / 2.0).max(if moved { 0.5 } else { 0.0 }),
                        offset,
                    })
                }
                _ => {
                    return Err(invalid(
                        "shadow",
                        "needs a css `color` and a finite `blur` of zero or more",
                    ))
                }
            }
        }
    };
    if fill.is_none()
        && stroke.is_none()
        && material.is_none()
        && !crate::layer_props::has_backdrop(node)
    {
        return Ok(());
    }

    let rotation = get_f32(node, "rotate", 0.0);
    if rotation != 0.0 {
        let pivot = node.get("pivot");
        let (origin_x, origin_y) = projector.project(position);
        let (pivot_x, pivot_y) = to_screen(
            origin_x
                + pivot
                    .and_then(|p| p.get("x"))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.5) as f32
                    * tw,
            origin_y
                + pivot
                    .and_then(|p| p.get("y"))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.5) as f32
                    * tw,
        );
        let (dx, dy) = (
            rect.x + rect.w / 2.0 - pivot_x,
            rect.y + rect.h / 2.0 - pivot_y,
        );
        let (cs, sn) = (rotation.cos(), rotation.sin());
        let (cx, cy) = (pivot_x + dx * cs - dy * sn, pivot_y + dx * sn + dy * cs);
        rect.x = cx - rect.w / 2.0;
        rect.y = cy - rect.h / 2.0;
    }

    if let Some(id) = hit_id(node) {
        gathered.regions.push(Region {
            id: id.to_string(),
            rect,
            rotation,
            wheel: takes_wheel(node),
            fixed: false,
            drag: takes_drag(node),
        });
    }
    out.push(DrawCommand::Sdf {
        kind,
        rect,
        corners,
        corner_kind,
        arc,
        polygon,
        fill: fill.unwrap_or([0.0; 4]),
        stroke,
        shadow,
        rotation,
        opacity,
        material,
    });
    crate::layer_props::wrap_backdrop(node, zoom, out)
}

/// Project one world shape through the shared Canvas vector commands.
pub(super) fn paint_vector_shape(
    node: &Value,
    projector: &Projector,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
    zoom: f32,
    epoch_now_ms: f64,
    gathered: &mut Gathered,
    out: &mut Vec<DrawCommand>,
) {
    let animated = animated_shape(node, epoch_now_ms);
    let node = animated.as_ref().unwrap_or(node);
    let Some(shape) = node.get("shape").and_then(Value::as_str) else {
        gap_once("shape-kind", "draw-shape without shape");
        return;
    };
    let Some(position) = node.get("position").and_then(parse_scene_pos) else {
        gap_once("shape-position", "draw-shape without position");
        return;
    };
    let blend_mode = node.get("blendMode").and_then(Value::as_str);
    if blend_mode.is_some_and(|mode| !matches!(mode, "source-over" | "lighter")) {
        gap_once(
            "shape-blend",
            "draw-shape blendMode needs scoped GPU compositing",
        );
        return;
    }
    let blur = get_f32(node, "blur", 0.0).max(0.0);
    let additive = blend_mode == Some("lighter");
    let shadow = node.get("shadow").filter(|shadow| !shadow.is_null());
    let shadow = match shadow {
        None => None,
        Some(shadow) => {
            let color = shadow
                .get("color")
                .and_then(Value::as_str)
                .and_then(parse_css_color);
            let blur = shadow.get("blur").and_then(Value::as_f64);
            match (color, blur) {
                (Some(color), Some(blur)) if blur.is_finite() && blur >= 0.0 => {
                    (blur > 0.0 && color[3] > 0.0).then_some((color, blur as f32))
                }
                _ => {
                    gap_once(
                        "shape-shadow",
                        "draw-shape shadow needs a css `color` and a finite `blur` of zero or more",
                    );
                    return;
                }
            }
        }
    };
    let opacity = get_f32(node, "opacity", 1.0);
    let fill_gradient = node
        .get("gradient")
        .and_then(|source| shape_gradient(source, position, projector, to_screen, zoom, opacity));
    let stroke_gradient = node
        .get("strokeGradient")
        .and_then(|source| shape_gradient(source, position, projector, to_screen, zoom, opacity));
    let pattern = node.get("fillPattern").and_then(shape_fill_pattern);
    let uses_vector_style = fill_gradient.is_some()
        || stroke_gradient.is_some()
        || pattern.is_some()
        || blur > 0.0
        || additive
        || shadow.is_some();
    let fill = node
        .get("fill")
        .and_then(Value::as_str)
        .and_then(parse_css_color)
        .map(|mut color| {
            color[3] *= opacity;
            color
        });
    let stroke = node
        .get("stroke")
        .and_then(Value::as_str)
        .and_then(parse_css_color)
        .or_else(|| stroke_gradient.as_ref().map(|_| [1.0, 1.0, 1.0, 1.0]))
        .map(|mut color| {
            color[3] *= opacity;
            Stroke {
                color,
                width: get_f32(node, "strokeWidth", 1.0) * zoom,
                dash: node
                    .get("strokeDash")
                    .and_then(Value::as_array)
                    .map(|dash| {
                        dash.iter()
                            .filter_map(Value::as_f64)
                            .map(|length| length as f32 * dash_unit(node, zoom))
                            .collect()
                    })
                    .unwrap_or_default(),
                dash_offset: get_f32(node, "strokeDashOffset", 0.0) * dash_unit(node, zoom),
                cap: LineCap::Butt,
                join: LineJoin::Miter,
            }
        });
    if fill.is_none()
        && stroke.is_none()
        && fill_gradient.is_none()
        && pattern.is_none()
        && hit_id(node).is_none()
    {
        return;
    }

    let start = out.len();
    // A hit area may paint nothing (no fill, no stroke); its region is its geometry.
    let mut geometry_bounds: Option<Rect> = None;
    let tw = projector.tile_width;
    match shape {
        "cell" => {
            let pts = projector.cell_path(position);
            let points: Vec<Vec2> = pts
                .iter()
                .map(|&(x, y)| {
                    let (sx, sy) = to_screen(x, y);
                    Vec2 { x: sx, y: sy }
                })
                .collect();
            if uses_vector_style {
                push_styled_path(
                    polygon_segments(&points),
                    PathPaint {
                        fill,
                        stroke,
                        fill_gradient,
                        stroke_gradient,
                        pattern,
                        opacity,
                        separate_paint: blur > 0.0,
                    },
                    out,
                );
            } else {
                if let Some(color) = fill {
                    out.push(DrawCommand::Poly {
                        points: points.clone(),
                        color,
                    });
                }
                push_shape_stroke(&points, stroke, out);
            }
        }
        "rect" => {
            let anchor = node
                .get("anchor")
                .and_then(Value::as_str)
                .and_then(Anchor::parse)
                .unwrap_or(Anchor::TopLeft);
            let (px, py) = projector.anchor_point(position, anchor);
            let x = px + get_f32(node, "offsetX", 0.0) * tw;
            let y = py + get_f32(node, "offsetY", 0.0) * tw;
            let w = get_f32(node, "width", 0.0) * tw;
            let h = get_f32(node, "height", 0.0) * tw;
            let (sx, sy) = to_screen(x, y);
            let rect = Rect {
                x: sx,
                y: sy,
                w: w * zoom,
                h: h * zoom,
            };
            geometry_bounds = Some(rect);
            let points = [
                Vec2 {
                    x: rect.x,
                    y: rect.y,
                },
                Vec2 {
                    x: rect.x + rect.w,
                    y: rect.y,
                },
                Vec2 {
                    x: rect.x + rect.w,
                    y: rect.y + rect.h,
                },
                Vec2 {
                    x: rect.x,
                    y: rect.y + rect.h,
                },
            ];
            if uses_vector_style {
                push_styled_path(
                    polygon_segments(&points),
                    PathPaint {
                        fill,
                        stroke,
                        fill_gradient,
                        stroke_gradient,
                        pattern,
                        opacity,
                        separate_paint: blur > 0.0,
                    },
                    out,
                );
            } else {
                if let Some(color) = fill {
                    out.push(DrawCommand::Quad { rect, color });
                }
                push_shape_stroke(&points, stroke, out);
            }
        }
        "ellipse" => {
            let anchor = node
                .get("anchor")
                .and_then(Value::as_str)
                .and_then(Anchor::parse)
                .unwrap_or(Anchor::Ground);
            let (px, py) = projector.anchor_point(position, anchor);
            let cx = px + get_f32(node, "offsetX", 0.0) * tw;
            let cy = py + get_f32(node, "offsetY", 0.0) * tw;
            let rx = get_f32(node, "radiusX", 0.0) * tw;
            // Parity with DrawShape.tsx: radiusY defaults to radiusX (a circle) in
            // WORLD units — the old post-scale fallback double-multiplied by tw.
            let ry = node
                .get("radiusY")
                .and_then(Value::as_f64)
                .map(|v| v as f32 * tw)
                .unwrap_or(rx);
            let mut points = Vec::with_capacity(ELLIPSE_SEGMENTS);
            for i in 0..ELLIPSE_SEGMENTS {
                let a = (i as f32 / ELLIPSE_SEGMENTS as f32) * std::f32::consts::TAU;
                let (sx, sy) = to_screen(cx + rx * a.cos(), cy + ry * a.sin());
                points.push(Vec2 { x: sx, y: sy });
            }
            let (cx, cy) = to_screen(cx, cy);
            let arc = vec![
                PathSegment::Arc {
                    cx,
                    cy,
                    rx: rx * zoom,
                    ry: ry * zoom,
                    start: 0.0,
                    end: std::f32::consts::TAU,
                },
                PathSegment::Close,
            ];
            if uses_vector_style {
                push_styled_path(
                    arc,
                    PathPaint {
                        fill,
                        stroke,
                        fill_gradient,
                        stroke_gradient,
                        pattern,
                        opacity,
                        separate_paint: blur > 0.0,
                    },
                    out,
                );
            } else {
                if let Some(color) = fill {
                    out.push(DrawCommand::Poly { points, color });
                }
                if let Some(stroke) = stroke {
                    out.push(DrawCommand::Path {
                        segments: arc,
                        fill: None,
                        stroke: Some(stroke),
                    });
                }
            }
        }
        "poly" => {
            let (bx, by) = projector.project(position);
            let offset_x = get_f32(node, "offsetX", 0.0);
            let offset_y = get_f32(node, "offsetY", 0.0);
            let points: Vec<Vec2> = node
                .get("points")
                .and_then(Value::as_array)
                .map(|pts| {
                    pts.iter()
                        .filter_map(|pt| {
                            let x = pt.get("x").and_then(Value::as_f64)? as f32;
                            let y = pt.get("y").and_then(Value::as_f64)? as f32;
                            let (sx, sy) =
                                to_screen(bx + (offset_x + x) * tw, by + (offset_y + y) * tw);
                            Some(Vec2 { x: sx, y: sy })
                        })
                        .collect()
                })
                .unwrap_or_default();
            if uses_vector_style {
                push_styled_path(
                    polygon_segments(&points),
                    PathPaint {
                        fill,
                        stroke,
                        fill_gradient,
                        stroke_gradient,
                        pattern,
                        opacity,
                        separate_paint: blur > 0.0,
                    },
                    out,
                );
            } else {
                if let Some(color) = fill {
                    out.push(DrawCommand::Poly {
                        points: points.clone(),
                        color,
                    });
                }
                push_shape_stroke(&points, stroke, out);
            }
        }
        "path" => {
            let Some(d) = node.get("d").and_then(Value::as_str) else {
                gap_once("shape-path", "draw-shape path without d");
                return;
            };
            let Some(segments) = parse_path_data(d, path_xf(node, projector, to_screen, zoom))
            else {
                gap_once("shape-path-data", "malformed draw-shape SVG path");
                return;
            };
            if uses_vector_style {
                push_styled_path(
                    segments,
                    PathPaint {
                        fill,
                        stroke,
                        fill_gradient,
                        stroke_gradient,
                        pattern,
                        opacity,
                        separate_paint: blur > 0.0,
                    },
                    out,
                );
            } else {
                out.push(DrawCommand::Path {
                    segments,
                    fill,
                    stroke,
                });
            }
        }
        _ => gap_once("shape-kind-unknown", "unknown draw-shape kind"),
    }
    if blur > 0.0 {
        let filtered: Vec<_> = out
            .drain(start..)
            .map(|command| DrawCommand::Blurred {
                command: Box::new(command),
                radius: blur,
            })
            .collect();
        out.extend(filtered);
    }
    let radians = get_f32(node, "rotate", 0.0);
    if radians != 0.0 {
        let pivot = node.get("pivot");
        let (origin_x, origin_y) = projector.project(position);
        let (x, y) = to_screen(
            origin_x
                + pivot
                    .and_then(|p| p.get("x"))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.5) as f32
                    * tw,
            origin_y
                + pivot
                    .and_then(|p| p.get("y"))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.5) as f32
                    * tw,
        );
        orbital_canvas_base::animate::transform_group_commands(
            &mut out[start..],
            Vec2 { x, y },
            Vec2 { x: 0.0, y: 0.0 },
            1.0,
            radians,
            1.0,
        );
    }
    if let Some((color, blur)) = shadow.filter(|_| out.len() > start) {
        // canvas shadowBlur is twice the Gaussian sigma; `blur` is in world units
        let commands = out[start..].to_vec();
        let sigma = blur * projector.tile_width * zoom / 2.0;
        out.insert(
            start,
            DrawCommand::Shadow {
                commands,
                color,
                sigma,
            },
        );
    }
    if additive && out.len() > start {
        let commands = out.drain(start..).collect();
        out.push(DrawCommand::AdditiveGroup { commands });
    }
    if let Some(id) = hit_id(node) {
        if let Some(rect) = commands_bounds(&out[start..]).or(geometry_bounds) {
            gathered.regions.push(Region {
                id: id.to_string(),
                rect,
                rotation: 0.0,
                wheel: takes_wheel(node),
                fixed: false,
                drag: takes_drag(node),
            });
        }
    }
}

pub(super) fn push_shape_stroke(
    points: &[Vec2],
    stroke: Option<Stroke>,
    out: &mut Vec<DrawCommand>,
) {
    let Some(stroke) = stroke else { return };
    let Some(first) = points.first() else { return };
    let mut segments = vec![PathSegment::MoveTo {
        x: first.x,
        y: first.y,
    }];
    segments.extend(
        points
            .iter()
            .skip(1)
            .map(|p| PathSegment::LineTo { x: p.x, y: p.y }),
    );
    segments.push(PathSegment::Close);
    out.push(DrawCommand::Path {
        segments,
        fill: None,
        stroke: Some(stroke),
    });
}
