//! `draw-group`: a group's transform, opacity, clip and smooth unions, and the bounds of what it paints.

use super::*;

pub(super) fn paint_group(
    node: &Value,
    projector: &Projector,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
    zoom: f32,
    epoch_now_ms: f64,
    gathered: &mut Gathered,
    out: &mut Vec<DrawCommand>,
) -> Result<(), CanvasSceneError> {
    let Some(position) = node.get("position").and_then(parse_scene_pos) else {
        gap_once("group-position", "draw-group without position");
        return Ok(());
    };
    let (gx, gy) = projector.project(position);
    let (zero_x, zero_y) = to_screen(0.0, 0.0);
    let animation = node
        .get("animation")
        .filter(|a| orbital_canvas_base::animate::plays_at(a, epoch_now_ms));
    let animated_number = |key: &str, default: f32| {
        animation
            .and_then(|animation| {
                orbital_canvas_base::animate::drawable_track(animation, epoch_now_ms, key)
            })
            .and_then(|value| value.as_f64())
            .map(|value| value as f32)
            .unwrap_or(default)
    };
    let (group_x, group_y) = to_screen(
        gx + animated_number("offsetX", 0.0) * projector.tile_width,
        gy + animated_number("offsetY", 0.0) * projector.tile_width,
    );
    let rotation_z = node
        .get("rotation")
        .and_then(Value::as_array)
        .and_then(|angles| angles.get(2))
        .and_then(Value::as_f64)
        .unwrap_or(0.0) as f32;
    let radians = get_f32(node, "rotate", 0.0) + rotation_z + animated_number("rotateZ", 0.0);
    let start = out.len();
    let (region_start, measured_start, hits_start) = (
        gathered.regions.len(),
        gathered.measured.len(),
        gathered.text_hits.len(),
    );
    let semantic_start = gathered.semantic.len();
    for child in items(node) {
        walk(
            child,
            projector,
            to_screen,
            zoom,
            epoch_now_ms,
            gathered,
            out,
        )?;
    }
    if let Some(spec) = node.get("target").filter(|t| !t.is_null()) {
        gathered.regions.truncate(region_start);
        gathered.text_hits.truncate(hits_start);
        return crate::layer_props::collect_target(spec, (zero_x, zero_y), out, start);
    }
    if radians != 0.0
        && out[start..]
            .iter()
            .any(|cmd| matches!(cmd, DrawCommand::Text { .. }))
    {
        out.truncate(start);
        gathered.regions.truncate(region_start);
        gathered.measured.truncate(measured_start);
        gathered.text_hits.truncate(hits_start);
        gap_once(
            "group-rotated-text",
            "draw-group rotating text needs a vector glyph path",
        );
        return Ok(());
    }
    if let Some(radius) = node.get("smoothUnion").filter(|r| !r.is_null()) {
        blend_group_shapes(radius, projector.tile_width * zoom, out, start)?;
    }
    let mut rect_clip = false;
    let clip = match node.get("clip").filter(|clip| !clip.is_null()) {
        Some(Value::String(clip)) if !clip.is_empty() => {
            let parsed = parse_path_data(
                clip,
                Xf {
                    s: projector.tile_width * zoom,
                    ox: zero_x,
                    oy: zero_y,
                },
            );
            if parsed.is_none() {
                out.truncate(start);
                gap_once("group-clip-path", "malformed draw-group SVG clip path");
                return Ok(());
            }
            parsed
        }
        Some(Value::Object(spec)) => {
            rect_clip = true;
            Some(rect_clip_segments(spec, projector, to_screen, zoom)?)
        }
        _ => None,
    };
    if let Some(segments) = clip {
        let commands = out.drain(start..).collect();
        out.push(DrawCommand::ClipGroup {
            clip: segments,
            commands,
        });
    }
    let origin = Vec2 {
        x: zero_x,
        y: zero_y,
    };
    let translation = Vec2 {
        x: group_x - zero_x,
        y: group_y - zero_y,
    };
    let scale = get_f32(node, "scale", 1.0) * animated_number("scale", 1.0);
    let opacity = get_f32(node, "opacity", 1.0) * animated_number("opacity", 1.0);
    orbital_canvas_base::animate::transform_group_commands(
        &mut out[start..],
        origin,
        translation,
        scale,
        radians,
        opacity,
    );
    // regions ride the group transform as sprites, which turn about their own centre like the group's marks
    let mut carriers: Vec<DrawCommand> = gathered.regions[region_start..]
        .iter()
        .map(|region| DrawCommand::Sprite {
            texture: String::new(),
            src: Rect::default(),
            dst: region.rect,
            rotation: region.rotation,
            opacity: 1.0,
            flip_x: false,
            flip_y: false,
        })
        .collect();
    orbital_canvas_base::animate::transform_group_commands(
        &mut carriers,
        origin,
        translation,
        scale,
        radians,
        1.0,
    );
    for (region, carrier) in gathered.regions[region_start..].iter_mut().zip(&carriers) {
        if let DrawCommand::Sprite { dst, rotation, .. } = carrier {
            region.rect = *dst;
            region.rotation = *rotation;
        }
    }
    // a semantic node's overlay rect rides the same transform as its hit region
    let mut boxes: Vec<DrawCommand> = gathered.semantic[semantic_start..]
        .iter()
        .map(|node| DrawCommand::Sprite {
            texture: String::new(),
            src: Rect::default(),
            dst: node.rect,
            rotation: 0.0,
            opacity: 1.0,
            flip_x: false,
            flip_y: false,
        })
        .collect();
    orbital_canvas_base::animate::transform_group_commands(
        &mut boxes,
        origin,
        translation,
        scale,
        radians,
        1.0,
    );
    for (node, carrier) in gathered.semantic[semantic_start..].iter_mut().zip(&boxes) {
        if let DrawCommand::Sprite { dst, .. } = carrier {
            node.rect = *dst;
        }
    }
    // a text hit's lines ride the same transform (text in a rotated group is refused above, so it only scales and moves)
    for hit in &mut gathered.text_hits[hits_start..] {
        for line in &mut hit.lines {
            let (x0, x1) = (
                line.edges.iter().copied().fold(f32::INFINITY, f32::min),
                line.edges.iter().copied().fold(f32::NEG_INFINITY, f32::max),
            );
            let before = Rect {
                x: x0,
                y: line.top,
                w: (x1 - x0).max(f32::EPSILON),
                h: (line.bottom - line.top).max(f32::EPSILON),
            };
            let mut carrier = vec![DrawCommand::Sprite {
                texture: String::new(),
                src: Rect::default(),
                dst: before,
                rotation: 0.0,
                opacity: 1.0,
                flip_x: false,
                flip_y: false,
            }];
            orbital_canvas_base::animate::transform_group_commands(
                &mut carrier,
                origin,
                translation,
                scale,
                radians,
                1.0,
            );
            if let Some(DrawCommand::Sprite { dst, .. }) = carrier.first() {
                for edge in &mut line.edges {
                    *edge = dst.x + (*edge - before.x) * dst.w / before.w;
                }
                line.top = dst.y;
                line.bottom = dst.y + dst.h;
            }
        }
    }
    for entry in &mut gathered.measured[measured_start..] {
        entry.width *= scale.abs();
        entry.height *= scale.abs();
    }
    if rect_clip {
        if let Some(DrawCommand::ClipGroup { clip, .. }) = out.get(start) {
            if let Some(bounds) = segments_bounds(clip) {
                cut_regions_to(&mut gathered.regions, region_start, bounds);
            }
        }
    }
    Ok(())
}

/// Replace the two SDF shapes a `smoothUnion` group painted by one [`DrawCommand::SdfBlend`] (contract 14.4).
pub(super) fn blend_group_shapes(
    radius: &Value,
    unit: f32,
    out: &mut Vec<DrawCommand>,
    start: usize,
) -> Result<(), CanvasSceneError> {
    let radius = radius
        .as_f64()
        .filter(|r| r.is_finite() && *r > 0.0)
        .ok_or_else(|| {
            invalid(
                "smoothUnion",
                "a smooth union radius is a number above zero",
            )
        })?;
    if !matches!(
        &out[start..],
        [DrawCommand::Sdf { .. }, DrawCommand::Sdf { .. }]
    ) {
        return Err(invalid(
            "smoothUnion",
            "a smooth union group holds exactly two SDF draw-shape children and nothing else",
        ));
    }
    let second = out.pop();
    let first = out.pop();
    let (Some(first), Some(second)) = (first, second) else {
        return Err(invalid(
            "smoothUnion",
            "a smooth union group holds two SDF shapes",
        ));
    };
    let compatible = match (&first, &second) {
        (
            DrawCommand::Sdf {
                stroke: first_stroke,
                opacity: first_opacity,
                material: first_material,
                ..
            },
            DrawCommand::Sdf {
                stroke,
                shadow,
                opacity,
                material,
                ..
            },
        ) => {
            if first_stroke.is_some_and(|s| s.dash != [0.0, 0.0]) {
                Err("a blended union draws a solid stroke, not a dashed one")
            } else if stroke.is_some() || shadow.is_some() {
                Err("the second shape carries no stroke and no shadow")
            } else if opacity != first_opacity {
                Err("both shapes carry the same opacity")
            } else if material != first_material {
                Err("both shapes carry the same material, or none")
            } else {
                Ok(())
            }
        }
        _ => Err("a smooth union group holds two SDF shapes"),
    };
    compatible.map_err(|detail| invalid("smoothUnion", detail))?;
    out.push(DrawCommand::SdfBlend {
        first: Box::new(first),
        second: Box::new(second),
        radius: radius as f32 * unit,
    });
    Ok(())
}

/// The box of an authored rect clip: `x`, `y`, `width`, `height` in group units, `radius` one number or four.
pub(super) fn rect_clip_segments(
    spec: &serde_json::Map<String, Value>,
    projector: &Projector,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
    zoom: f32,
) -> Result<Vec<PathSegment>, CanvasSceneError> {
    let number = |key: &str| {
        spec.get(key)
            .and_then(Value::as_f64)
            .filter(|v| v.is_finite())
    };
    let size = |key: &str| {
        number(key)
            .filter(|v| *v > 0.0)
            .ok_or_else(|| invalid("clip", format!("a rect clip needs `{key}` above zero")))
    };
    let (width, height) = (size("width")?, size("height")?);
    let origin = ScenePos {
        x: number("x").unwrap_or(0.0) as f32,
        y: number("y").unwrap_or(0.0) as f32,
    };
    let (px, py) = projector.project(origin);
    let (x, y) = to_screen(px, py);
    let unit = projector.tile_width * zoom;
    let (w, h) = (width as f32 * unit, height as f32 * unit);
    let radii = match spec.get("radius").filter(|r| !r.is_null()) {
        None => [0.0; 4],
        Some(Value::Number(n)) => [n.as_f64().unwrap_or(-1.0) as f32; 4],
        Some(Value::Array(items)) if items.len() == 4 => {
            let mut radii = [0.0; 4];
            for (slot, item) in radii.iter_mut().zip(items) {
                *slot = item.as_f64().unwrap_or(-1.0) as f32;
            }
            radii
        }
        Some(_) => {
            return Err(invalid(
                "clip",
                "`radius` is one number or four (tl, tr, br, bl)",
            ));
        }
    };
    if radii.iter().any(|r| !r.is_finite() || *r < 0.0) {
        return Err(invalid("clip", "`radius` values are zero or more"));
    }
    let limit = w.min(h) / 2.0;
    let [tl, tr, br, bl] = radii.map(|r| (r * unit).min(limit));
    const KAPPA: f32 = 0.552_284_8;
    let corner = |c1: (f32, f32), c2: (f32, f32), to: (f32, f32)| PathSegment::CubicTo {
        x1: c1.0,
        y1: c1.1,
        x2: c2.0,
        y2: c2.1,
        x: to.0,
        y: to.1,
    };
    let mut path = vec![PathSegment::MoveTo { x: x + tl, y }];
    path.push(PathSegment::LineTo { x: x + w - tr, y });
    if tr > 0.0 {
        path.push(corner(
            (x + w - tr + tr * KAPPA, y),
            (x + w, y + tr - tr * KAPPA),
            (x + w, y + tr),
        ));
    }
    path.push(PathSegment::LineTo {
        x: x + w,
        y: y + h - br,
    });
    if br > 0.0 {
        path.push(corner(
            (x + w, y + h - br + br * KAPPA),
            (x + w - br + br * KAPPA, y + h),
            (x + w - br, y + h),
        ));
    }
    path.push(PathSegment::LineTo {
        x: x + bl,
        y: y + h,
    });
    if bl > 0.0 {
        path.push(corner(
            (x + bl - bl * KAPPA, y + h),
            (x, y + h - bl + bl * KAPPA),
            (x, y + h - bl),
        ));
    }
    path.push(PathSegment::LineTo { x, y: y + tl });
    if tl > 0.0 {
        path.push(corner(
            (x, y + tl - tl * KAPPA),
            (x + tl - tl * KAPPA, y),
            (x + tl, y),
        ));
    }
    path.push(PathSegment::Close);
    Ok(path)
}

/// The box that holds every point of a path (curve control points included).
pub(super) fn segments_bounds(segments: &[PathSegment]) -> Option<Rect> {
    let mut bounds: Option<(f32, f32, f32, f32)> = None;
    let mut add = |x: f32, y: f32| {
        bounds = Some(match bounds {
            None => (x, y, x, y),
            Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
        });
    };
    for segment in segments {
        match *segment {
            PathSegment::MoveTo { x, y } | PathSegment::LineTo { x, y } => add(x, y),
            PathSegment::CubicTo {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => {
                add(x1, y1);
                add(x2, y2);
                add(x, y);
            }
            PathSegment::Arc { cx, cy, rx, ry, .. } => {
                add(cx - rx, cy - ry);
                add(cx + rx, cy + ry);
            }
            PathSegment::Close => {}
        }
    }
    bounds.map(|(x0, y0, x1, y1)| Rect {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    })
}

/// Cut the regions a clipped group gathered to the clip's bounds: one wholly outside is dropped, a straddling
/// one is intersected, a rotated one stays whole.
pub(super) fn cut_regions_to(regions: &mut Vec<Region>, from: usize, bounds: Rect) {
    let mut kept: Vec<Region> = Vec::new();
    for mut region in regions.drain(from..) {
        if region.rotation == 0.0 {
            let x0 = region.rect.x.max(bounds.x);
            let y0 = region.rect.y.max(bounds.y);
            let x1 = (region.rect.x + region.rect.w).min(bounds.x + bounds.w);
            let y1 = (region.rect.y + region.rect.h).min(bounds.y + bounds.h);
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            region.rect = Rect {
                x: x0,
                y: y0,
                w: x1 - x0,
                h: y1 - y0,
            };
        }
        kept.push(region);
    }
    regions.extend(kept);
}

/// Axis-aligned bounds of the marks a shape painted, in packet coordinates.
pub(super) fn commands_bounds(commands: &[DrawCommand]) -> Option<Rect> {
    let mut bounds: Option<(f32, f32, f32, f32)> = None;
    let mut add = |x0: f32, y0: f32, x1: f32, y1: f32| {
        bounds = Some(match bounds {
            None => (x0, y0, x1, y1),
            Some((a, b, c, d)) => (a.min(x0), b.min(y0), c.max(x1), d.max(y1)),
        });
    };
    fn walk_commands(commands: &[DrawCommand], add: &mut dyn FnMut(f32, f32, f32, f32)) {
        for command in commands {
            match command {
                DrawCommand::Quad { rect, .. } => {
                    add(rect.x, rect.y, rect.x + rect.w, rect.y + rect.h)
                }
                DrawCommand::Sdf { rect, .. } => {
                    add(rect.x, rect.y, rect.x + rect.w, rect.y + rect.h)
                }
                DrawCommand::SdfBackdrop { shape, .. } => {
                    walk_commands(std::slice::from_ref(shape.as_ref()), add)
                }
                DrawCommand::SdfBlend { first, second, .. } => {
                    walk_commands(std::slice::from_ref(first.as_ref()), add);
                    walk_commands(std::slice::from_ref(second.as_ref()), add);
                }
                DrawCommand::Poly { points, .. } => {
                    for p in points {
                        add(p.x, p.y, p.x, p.y);
                    }
                }
                DrawCommand::Path { segments, .. }
                | DrawCommand::GradientPath { segments, .. }
                | DrawCommand::PatternPath { segments, .. } => {
                    for segment in segments {
                        match segment {
                            PathSegment::MoveTo { x, y } | PathSegment::LineTo { x, y } => {
                                add(*x, *y, *x, *y)
                            }
                            PathSegment::Arc { cx, cy, rx, ry, .. } => {
                                add(cx - rx, cy - ry, cx + rx, cy + ry)
                            }
                            PathSegment::CubicTo {
                                x1,
                                y1,
                                x2,
                                y2,
                                x,
                                y,
                            } => {
                                add(*x1, *y1, *x1, *y1);
                                add(*x2, *y2, *x2, *y2);
                                add(*x, *y, *x, *y);
                            }
                            PathSegment::Close => {}
                        }
                    }
                }
                DrawCommand::Blurred { command, .. } => {
                    walk_commands(std::slice::from_ref(command.as_ref()), add)
                }
                DrawCommand::AdditiveGroup { commands } => walk_commands(commands, add),
                _ => {}
            }
        }
    }
    walk_commands(commands, &mut add);
    bounds.map(|(x0, y0, x1, y1)| Rect {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    })
}
