//! `draw-text`: a text's font, colour runs, reveal and paint.

use super::*;

/// The text with its keyframe `animation` resolved at the paint clock: `opacity`, `offsetX` and `offsetY`.
pub(super) fn animated_text(node: &Value, epoch_now_ms: f64) -> Option<Value> {
    let animation = node.get("animation")?;
    if !orbital_canvas_base::animate::plays_at(animation, epoch_now_ms) {
        return None;
    }
    let mut animated = node.clone();
    let object = animated.as_object_mut()?;
    for key in ["opacity", "offsetX", "offsetY"] {
        if let Some(value) =
            orbital_canvas_base::animate::drawable_track(animation, epoch_now_ms, key)
        {
            object.insert(key.to_owned(), value);
        }
    }
    Some(animated)
}

/// A `draw-text`'s `colorRuns`: character ranges `[from, to)` of `text`, sorted and apart, each with its own colour
/// (its alpha scaled by `opacity`, as the node's own colour is).
pub(super) fn read_color_runs(
    node: &Value,
    text: &str,
    opacity: f32,
) -> Result<Vec<ColorRun>, CanvasSceneError> {
    let items = match node.get("colorRuns") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(items)) => items,
        Some(_) => {
            return Err(invalid(
                "colorRuns",
                "must be a list of { from, to, color }",
            ))
        }
    };
    let length = text.chars().count();
    let mut runs: Vec<ColorRun> = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let index = |field: &str| match item.get(field).and_then(Value::as_f64) {
            Some(v) if v.is_finite() && v >= 0.0 && v.fract() == 0.0 => Ok(v as usize),
            _ => Err(invalid(
                "colorRuns",
                format!("run {i}: `{field}` must be a whole number of zero or more"),
            )),
        };
        let (from, to) = (index("from")?, index("to")?);
        if from >= to {
            return Err(invalid(
                "colorRuns",
                format!("run {i}: `from` ({from}) must be below `to` ({to})"),
            ));
        }
        if to > length {
            return Err(invalid(
                "colorRuns",
                format!("run {i}: `to` ({to}) is past the text's {length} characters"),
            ));
        }
        if let Some(previous) = runs.last() {
            if from < previous.from {
                return Err(invalid(
                    "colorRuns",
                    format!("run {i}: runs must be sorted by `from`"),
                ));
            }
            if from < previous.to {
                return Err(invalid(
                    "colorRuns",
                    format!("run {i}: overlaps the run before it"),
                ));
            }
        }
        let Some(mut color) = item
            .get("color")
            .and_then(Value::as_str)
            .and_then(parse_css_color)
        else {
            return Err(invalid(
                "colorRuns",
                format!("run {i}: `color` is not a supported CSS color"),
            ));
        };
        color[3] *= opacity;
        runs.push(ColorRun { from, to, color });
    }
    Ok(runs)
}

/// The runs that fall on the line of `len` characters starting at index `start` of the text, as indices of that line.
pub(super) fn runs_on_line(runs: &[ColorRun], start: usize, len: usize) -> Vec<ColorRun> {
    runs.iter()
        .filter_map(|run| {
            let (from, to) = (run.from.max(start), run.to.min(start + len));
            (from < to).then_some(ColorRun {
                from: from - start,
                to: to - start,
                color: run.color,
            })
        })
        .collect()
}

pub(super) fn paint_text(
    node: &Value,
    projector: &Projector,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
    zoom: f32,
    epoch_now_ms: f64,
    gathered: &mut Gathered,
    out: &mut Vec<DrawCommand>,
) -> Result<(), CanvasSceneError> {
    let animated = animated_text(node, epoch_now_ms);
    let node = animated.as_ref().unwrap_or(node);
    let reveal = crate::text_reveal::parse(node).map_err(|detail| invalid("reveal", detail))?;
    let Some(position) = node.get("position").and_then(parse_scene_pos) else {
        gap_once("text-position", "draw-text without position");
        return Ok(());
    };
    let Some(text) = node.get("text").and_then(Value::as_str) else {
        gap_once("text-value", "draw-text without text");
        return Ok(());
    };
    let Some(mut color) = node
        .get("color")
        .and_then(Value::as_str)
        .and_then(parse_css_color)
    else {
        gap_once("text-color", "draw-text color is not a supported CSS color");
        return Ok(());
    };
    let (size, weight, family) = match parse_text_font(node.get("font").and_then(Value::as_str)) {
        Some(font) => font,
        None => {
            gap_once(
                "text-font",
                "draw-text CSS font is not carried by the native font pack",
            );
            return Ok(());
        }
    };
    let align = match node.get("align").and_then(Value::as_str).unwrap_or("left") {
        "left" | "start" => TextAlign::Left,
        "center" => TextAlign::Center,
        "right" | "end" => TextAlign::Right,
        _ => {
            gap_once("text-align", "draw-text alignment is not supported");
            return Ok(());
        }
    };
    let size = size * zoom;
    color[3] *= get_f32(node, "opacity", 1.0);
    let line_height = match node.get("lineHeight") {
        None | Some(Value::Null) => 1.3,
        Some(value) => match value.as_f64() {
            Some(v) if v.is_finite() && v > 0.0 => v as f32,
            _ => return Err(invalid("lineHeight", "must be a number above zero")),
        },
    };
    let bold = weight >= 600;
    let editing = TextEditing::read(node)?;
    let color_runs: Vec<ColorRun> = read_color_runs(node, text, get_f32(node, "opacity", 1.0))?
        .into_iter()
        .map(|run| ColorRun {
            from: editing.shown_index(run.from),
            to: editing.shown_index(run.to),
            color: run.color,
        })
        .collect();
    if reveal.is_some() && !color_runs.is_empty() {
        return Err(invalid(
            "colorRuns",
            "a text with a reveal paints one colour; colorRuns cannot combine with reveal",
        ));
    }
    let original = text;
    let shown = editing.shown(text);
    let text = shown.as_str();
    let max_width = match node.get("maxWidth") {
        None | Some(Value::Null) => None,
        Some(value) => match value.as_f64() {
            Some(w) if w.is_finite() && w > 0.0 => Some(w as f32 * zoom),
            _ => return Err(invalid("maxWidth", "must be a number above zero")),
        },
    };
    if max_width.is_some() && node.get("width").is_some_and(|w| !w.is_null()) {
        return Err(invalid(
            "maxWidth",
            "a paragraph with a width wraps or truncates in its own box; maxWidth is for one line",
        ));
    }
    // like Canvas2D fillText's maxWidth: one line wider than it is drawn smaller, so it fits
    let size = match max_width {
        Some(limit) => {
            let natural = orbital_canvas_paint::vector::shape(text, size, bold, family).width;
            if natural > limit {
                size * limit / natural
            } else {
                size
            }
        }
        None => size,
    };
    let fit = read_fit(node, zoom)?;
    let id = node.get("id").and_then(Value::as_str);
    let measure = node
        .get("measure")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    if let Some(width) = node.get("width").filter(|w| !w.is_null()) {
        let width = match width.as_f64() {
            Some(w) if w.is_finite() && w > 0.0 => w as f32 * zoom,
            _ => return Err(invalid("width", "must be a number above zero")),
        };
        let max_lines = match node.get("maxLines") {
            None | Some(Value::Null) => None,
            Some(value) => match value.as_u64() {
                Some(n) if n >= 1 => Some(n as usize),
                _ => return Err(invalid("maxLines", "must be a whole number of one or more")),
            },
        };
        let wrap = node.get("wrap").and_then(Value::as_bool).unwrap_or(true);
        let truncate = node
            .get("truncate")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let (x, y) = projector.anchor_point(position, Anchor::TopLeft);
        let (sx, sy) = to_screen(
            x + get_f32(node, "offsetX", 0.0),
            y + get_f32(node, "offsetY", 0.0),
        );
        let frame = ParagraphBox {
            width,
            wrap,
            max_lines,
            truncate,
        };
        let (size, paragraph) = match fit {
            Some(fit) => fit_paragraph(text, size, bold, family, frame, line_height, fit),
            None => (size, layout_paragraph(text, size, bold, family, frame)),
        };
        let line_px = size * line_height;
        let text_start = out.len();
        let revealed = reveal.as_ref().and_then(|reveal| {
            let lines: Vec<crate::text_reveal::Line> = paragraph
                .lines
                .iter()
                .enumerate()
                .filter(|(_, line)| !line.is_empty())
                .map(|(i, line)| {
                    let w = orbital_canvas_paint::vector::shape(line, size, bold, family).width;
                    crate::text_reveal::Line {
                        text: line,
                        left: crate::text_reveal::left_edge(align, sx, width, w),
                        y: sy + (i as f32 + 0.5) * line_px,
                    }
                })
                .collect();
            crate::text_reveal::paint(
                reveal,
                epoch_now_ms,
                &lines,
                size,
                bold,
                family,
                color,
                weight,
            )
        });
        if let Some(commands) = revealed {
            out.extend(commands);
        } else {
            for (i, line) in paragraph.lines.iter().enumerate() {
                if line.is_empty() {
                    continue;
                }
                let x = match align {
                    TextAlign::Left => sx,
                    TextAlign::Center => sx + width / 2.0,
                    TextAlign::Right => sx + width,
                };
                out.push(DrawCommand::Text {
                    text: line.clone(),
                    pos: Vec2 {
                        x,
                        y: sy + (i as f32 + 0.5) * line_px,
                    },
                    size,
                    color,
                    align,
                    weight,
                    halo: None,
                    family,
                    color_runs: runs_on_line(
                        &color_runs,
                        paragraph.starts[i],
                        line.chars().count(),
                    ),
                    rotation: 0.0,
                });
            }
        }
        if let Some(positions) = editing
            .active()
            .then(|| caret_positions(&paragraph.lines, text, size, bold, family))
        {
            match positions {
                Some(positions) => {
                    let edit_lines: Vec<EditLine> = positions
                        .into_iter()
                        .enumerate()
                        .map(|(i, laid)| {
                            let w = laid.width;
                            let left = match align {
                                TextAlign::Left => sx,
                                TextAlign::Center => sx + (width - w) / 2.0,
                                TextAlign::Right => sx + width - w,
                            };
                            EditLine { left, top: sy + i as f32 * line_px, height: line_px, start: laid.start, edges: laid.edges, spans: laid.spans }
                        })
                        .collect();
                    paint_editing(&editing, &edit_lines, color, size, epoch_now_ms, text_start, out);
                }
                None => gap_once("text-caret-layout", "draw-text caret and selection need lines that join back into the text and shape every character"),
            }
        }
        let lines = paragraph.lines.len();
        let height = lines as f32 * line_px;
        if let Some(id) = id {
            if hit_id(node).is_some() {
                gathered.regions.push(Region {
                    id: id.to_string(),
                    rect: Rect {
                        x: sx,
                        y: sy,
                        w: width,
                        h: height,
                    },
                    rotation: 0.0,
                    wheel: takes_wheel(node),
                    fixed: false,
                    drag: takes_drag(node),
                });
                if text == original {
                    if let Some(positions) =
                        caret_positions(&paragraph.lines, text, size, bold, family)
                    {
                        gathered.text_hits.push(crate::scene::TextHit {
                            id: id.to_string(),
                            fixed: false,
                            lines: positions
                                .into_iter()
                                .enumerate()
                                .map(|(i, laid)| {
                                    let (start, w) = (laid.start, laid.width);
                                    let edges = laid.edges;
                                    let left = match align {
                                        TextAlign::Left => sx,
                                        TextAlign::Center => sx + (width - w) / 2.0,
                                        TextAlign::Right => sx + width - w,
                                    };
                                    crate::scene::TextHitLine {
                                        top: sy + i as f32 * line_px,
                                        bottom: sy + (i + 1) as f32 * line_px,
                                        start,
                                        edges: edges.into_iter().map(|e| left + e).collect(),
                                    }
                                })
                                .collect(),
                        });
                    }
                }
            }
            if measure {
                gathered.measured.push(Measured {
                    id: id.to_string(),
                    width: paragraph.widths.iter().copied().fold(0.0, f32::max),
                    height,
                    lines: lines as u32,
                });
            }
        }
        return Ok(());
    }

    let anchor = node
        .get("anchor")
        .and_then(Value::as_str)
        .and_then(Anchor::parse)
        .unwrap_or(Anchor::Center);
    let (x, y) = projector.anchor_point(position, anchor);
    let x = x + get_f32(node, "offsetX", 0.0);
    let y = y + get_f32(node, "offsetY", 0.0);
    let (sx, sy) = to_screen(x, y);
    let Some((asc, desc)) = orbital_canvas_paint::vector::text_font_metrics(size) else {
        gap_once("text-face", "native text face is unavailable");
        return Ok(());
    };
    let mid = (asc + desc) / 2.0;
    let sy = match node
        .get("baseline")
        .and_then(Value::as_str)
        .unwrap_or("alphabetic")
    {
        "alphabetic" => sy - mid,
        "middle" => sy,
        "top" => sy + asc - mid,
        "bottom" => sy + desc - mid,
        _ => {
            gap_once(
                "text-baseline",
                "draw-text baseline is not carried by the native font pack",
            );
            return Ok(());
        }
    };
    if let Some(id) = id {
        let tracked = hit_id(node).is_some();
        if tracked || measure {
            let w = orbital_canvas_paint::vector::shape(text, size, bold, family).width;
            let h = size * line_height;
            let left = match align {
                TextAlign::Left => sx,
                TextAlign::Center => sx - w / 2.0,
                TextAlign::Right => sx - w,
            };
            if tracked {
                gathered.regions.push(Region {
                    id: id.to_string(),
                    rect: Rect {
                        x: left,
                        y: sy - h / 2.0,
                        w,
                        h,
                    },
                    rotation: 0.0,
                    wheel: takes_wheel(node),
                    fixed: false,
                    drag: takes_drag(node),
                });
                if text == original {
                    if let Some(mut positions) =
                        caret_positions(&[text.to_string()], text, size, bold, family)
                    {
                        let laid = positions.remove(0);
                        let (start, edges) = (laid.start, laid.edges);
                        gathered.text_hits.push(crate::scene::TextHit {
                            id: id.to_string(),
                            fixed: false,
                            lines: vec![crate::scene::TextHitLine {
                                top: sy - h / 2.0,
                                bottom: sy + h / 2.0,
                                start,
                                edges: edges.into_iter().map(|e| left + e).collect(),
                            }],
                        });
                    }
                }
            }
            if measure {
                gathered.measured.push(Measured {
                    id: id.to_string(),
                    width: w,
                    height: h,
                    lines: 1,
                });
            }
        }
    }
    let text_start = out.len();
    let revealed = reveal.as_ref().and_then(|reveal| {
        let w = orbital_canvas_paint::vector::shape(text, size, bold, family).width;
        let line = crate::text_reveal::Line {
            text,
            left: crate::text_reveal::left_edge(align, sx, 0.0, w),
            y: sy,
        };
        crate::text_reveal::paint(
            reveal,
            epoch_now_ms,
            &[line],
            size,
            bold,
            family,
            color,
            weight,
        )
    });
    if let Some(commands) = revealed {
        out.extend(commands);
    } else {
        out.push(DrawCommand::Text {
            text: text.to_owned(),
            pos: Vec2 { x: sx, y: sy },
            size,
            color,
            align,
            weight,
            halo: None,
            family,
            color_runs: color_runs.clone(),
            rotation: 0.0,
        });
    }
    if editing.active() {
        match caret_positions(&[text.to_string()], text, size, bold, family) {
            Some(mut positions) => {
                let laid = positions.remove(0);
                let w = laid.width;
                let left = match align {
                    TextAlign::Left => sx,
                    TextAlign::Center => sx - w / 2.0,
                    TextAlign::Right => sx - w,
                };
                let h = size * line_height;
                let line = EditLine { left, top: sy - h / 2.0, height: h, start: laid.start, edges: laid.edges, spans: laid.spans };
                paint_editing(&editing, &[line], color, size, epoch_now_ms, text_start, out);
            }
            None => gap_once("text-caret-layout", "draw-text caret and selection need lines that join back into the text and shape every character"),
        }
    }
    Ok(())
}

/// A paragraph's `fit: "shrink"` box: its height and the smallest font size, in screen units.
#[derive(Debug, Clone, Copy)]
pub(super) struct Fit {
    height: f32,
    min_size: f32,
}

/// The `fit`, `height` and `minSize` of a `draw-text`: `None` for `fit: "none"` (the default), an error for a
/// value that is not understood or a `shrink` without the `width` and `height` that make its box.
pub(super) fn read_fit(node: &Value, zoom: f32) -> Result<Option<Fit>, CanvasSceneError> {
    let positive = |field: &str| match node.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => match value.as_f64() {
            Some(v) if v.is_finite() && v > 0.0 => Ok(Some(v as f32 * zoom)),
            _ => Err(invalid(field, "must be a number above zero")),
        },
    };
    let height = positive("height")?;
    let min_size = positive("minSize")?;
    match node.get("fit") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(mode)) if mode == "none" => Ok(None),
        Some(Value::String(mode)) if mode == "shrink" => {
            let has_width = node.get("width").is_some_and(|w| !w.is_null());
            match (height, has_width) {
                (Some(height), true) => Ok(Some(Fit {
                    height,
                    min_size: min_size.unwrap_or(10.0 * zoom),
                })),
                _ => Err(invalid("fit", "shrink needs both a width and a height")),
            }
        }
        _ => Err(invalid("fit", "must be \"none\" or \"shrink\"")),
    }
}

/// The paragraph at the largest font size, between `fit.min_size` and `size`, whose lines fit `frame.width` and
/// stack within `fit.height`; at `fit.min_size` it is the paragraph as it falls, overflowing. The size is found by
/// bisection on the same layout the paint uses.
pub(super) fn fit_paragraph(
    text: &str,
    size: f32,
    bold: bool,
    family: orbital_canvas_base::packet::TextFamily,
    frame: ParagraphBox,
    line_height: f32,
    fit: Fit,
) -> (f32, Paragraph) {
    let laid = |s: f32| layout_paragraph(text, s, bold, family, frame);
    let fits = |s: f32, p: &Paragraph| {
        p.lines.len() as f32 * s * line_height <= fit.height
            && p.widths.iter().all(|w| *w <= frame.width + 1e-3)
    };
    let first = laid(size);
    let floor = fit.min_size.min(size);
    if fits(size, &first) || floor >= size {
        return (size, first);
    }
    let at_floor = laid(floor);
    if !fits(floor, &at_floor) {
        return (floor, at_floor);
    }
    let (mut low, mut high) = (floor, size);
    let mut best = (floor, at_floor);
    for _ in 0..24 {
        let mid = (low + high) / 2.0;
        let paragraph = laid(mid);
        if fits(mid, &paragraph) {
            low = mid;
            best = (mid, paragraph);
        } else {
            high = mid;
        }
    }
    best
}

pub(crate) fn parse_text_font(
    font: Option<&str>,
) -> Option<(f32, u16, orbital_canvas_base::packet::TextFamily)> {
    use orbital_canvas_base::packet::TextFamily;
    let Some(font) = font else {
        return Some((10.0, 400, TextFamily::Sans));
    };
    let mut tokens = font.split_whitespace();
    let first = tokens.next()?;
    let (weight, size) = match first {
        "bold" => (700, tokens.next()?),
        "normal" => (400, tokens.next()?),
        numeric if numeric.bytes().all(|b| b.is_ascii_digit()) => {
            let weight = numeric
                .parse::<u16>()
                .ok()
                .filter(|w| (1..=1000).contains(w))?;
            (weight, tokens.next()?)
        }
        _ => (400, first),
    };
    let size = size.strip_suffix("px")?.parse::<f32>().ok()?;
    if !size.is_finite() || size <= 0.0 {
        return None;
    }
    let list = tokens.collect::<Vec<_>>().join(" ");
    list.split(',')
        .find_map(|name| named_family(name.trim()))
        .map(|family| (size, weight, family))
}

/// The family of the pack that a CSS family name is, or `None` for a name the pack does not carry.
pub(super) fn named_family(name: &str) -> Option<orbital_canvas_base::packet::TextFamily> {
    use orbital_canvas_base::packet::TextFamily;
    let name = name
        .trim_matches(|c| c == '\'' || c == '"')
        .to_ascii_lowercase();
    match name.as_str() {
        "sans-serif" | "system-ui" => Some(TextFamily::Sans),
        "monospace" => Some(TextFamily::Mono),
        "serif" => Some(TextFamily::Serif),
        "reem kufi" => Some(TextFamily::Display),
        "cairo" => Some(TextFamily::Body),
        "display" | "body" => Some(TextFamily::Sans),
        "mono" => Some(TextFamily::Mono),
        other => orbital_canvas_base::faces::face_named(other),
    }
}
