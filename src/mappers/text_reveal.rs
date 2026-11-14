//! `draw-text` `reveal`: glyphs that appear one after another on the paint clock, each a pure function of the clock.

use crate::packet::{DrawCommand, TextAlign, TextFamily, Vec2};
use serde_json::Value;

const DEFAULT_OFFSET_Y: f32 = 6.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Reveal {
    start_ms: f64,
    per_glyph_ms: f64,
    duration_ms: f64,
    offset_y: f32,
}

/// The `reveal` of a `draw-text` node: `Ok(None)` when it has none, `Err(detail)` when it is malformed.
pub(crate) fn parse(node: &Value) -> Result<Option<Reveal>, String> {
    let Some(reveal) = node.get("reveal").filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let Some(object) = reveal.as_object() else {
        return Err("must be an object".to_owned());
    };
    let number = |key: &str, required: bool, non_negative: bool| -> Result<Option<f64>, String> {
        match object.get(key).filter(|v| !v.is_null()) {
            None if required => Err(format!("{key} is required")),
            None => Ok(None),
            Some(v) => match v.as_f64() {
                Some(n) if n.is_finite() && (!non_negative || n >= 0.0) => Ok(Some(n)),
                _ => Err(format!(
                    "{key} must be a {}finite number",
                    if non_negative { "non-negative " } else { "" }
                )),
            },
        }
    };
    let start_ms = number("startMs", true, true)?.unwrap_or_default();
    let per_glyph_ms = number("perGlyphMs", true, true)?.unwrap_or_default();
    let duration_ms = number("durationMs", true, true)?.unwrap_or_default();
    let offset_y = number("offsetY", false, false)?.map_or(DEFAULT_OFFSET_Y, |n| n as f32);
    Ok(Some(Reveal {
        start_ms,
        per_glyph_ms,
        duration_ms,
        offset_y,
    }))
}

impl Reveal {
    fn end_ms(&self, glyphs: usize) -> f64 {
        self.start_ms + glyphs.saturating_sub(1) as f64 * self.per_glyph_ms + self.duration_ms
    }

    /// Opacity factor and downward offset of glyph `index` at `now_ms`, eased out.
    fn progress(&self, index: usize, now_ms: f64) -> (f32, f32) {
        let begin = self.start_ms + index as f64 * self.per_glyph_ms;
        if now_ms < begin {
            return (0.0, self.offset_y);
        }
        let t = if self.duration_ms > 0.0 {
            ((now_ms - begin) / self.duration_ms).min(1.0)
        } else {
            1.0
        };
        let eased = (1.0 - (1.0 - t).powi(3)) as f32;
        (eased, self.offset_y * (1.0 - eased))
    }
}

fn reveals(node: &Value, f: &mut dyn FnMut(&Value)) {
    if node.get("type").and_then(Value::as_str) == Some("draw-text") {
        f(node);
    }
    for key in ["props", "drawables", "children", "items"] {
        match node.get(key) {
            Some(Value::Array(items)) => items.iter().for_each(|i| reveals(i, f)),
            Some(child @ Value::Object(_)) => reveals(child, f),
            _ => {}
        }
    }
}

pub(crate) fn has_reveal(node: &Value) -> bool {
    let mut found = false;
    reveals(node, &mut |t| found |= matches!(parse(t), Ok(Some(_))));
    found
}

/// Whether any revealing text under `node` still has a glyph to show or finish at the clock. A text has at most as
/// many glyphs as characters, so the end it reads is never early.
pub(crate) fn any_live(node: &Value, epoch_now_ms: f64) -> bool {
    let mut live = false;
    reveals(node, &mut |t| {
        if let Ok(Some(reveal)) = parse(t) {
            let chars = t
                .get("text")
                .and_then(Value::as_str)
                .map_or(0, |s| s.chars().count());
            live |= chars > 0 && epoch_now_ms < reveal.end_ms(chars);
        }
    });
    live
}

/// One line to paint: its text, the left edge it starts at and the vertical middle of its em box.
pub(crate) struct Line<'a> {
    pub text: &'a str,
    pub left: f32,
    pub y: f32,
}

/// How a line of text is aligned against the box it is painted in: the left edge from the shaped width.
pub(crate) fn left_edge(align: TextAlign, anchor_x: f32, box_width: f32, width: f32) -> f32 {
    match align {
        TextAlign::Left => anchor_x,
        TextAlign::Center => anchor_x + (box_width - width) / 2.0,
        TextAlign::Right => anchor_x + box_width - width,
    }
}

struct Unit {
    bytes: std::ops::Range<usize>,
    x0: f32,
    first: usize,
    rtl: bool,
    joined: bool,
}

/// The reveal units of one shaped line, in visual order, each timed by its place in reading (logical) order. A cluster is a unit of its own when it shapes the same alone
/// as inside the line; clusters that do not (a joined Arabic run) are drawn together, so their forms stay joined.
fn units(
    line: &str,
    shaped: &crate::vector::Shaped,
    bold: bool,
    size: f32,
    family: TextFamily,
    base: usize,
) -> Vec<Unit> {
    let mut order: Vec<usize> = (0..shaped.clusters.len()).collect();
    order.sort_by_key(|k| shaped.clusters[*k].bytes.start);
    let mut rank = vec![0; order.len()];
    for (r, k) in order.into_iter().enumerate() {
        rank[k] = r;
    }
    let mut out: Vec<Unit> = Vec::new();
    for (k, cluster) in shaped.clusters.iter().enumerate() {
        let alone = crate::vector::shape(&line[cluster.bytes.clone()], size, bold, family);
        let inside = &shaped.glyphs[cluster.glyphs.clone()];
        let separable = alone.glyphs.len() == inside.len()
            && alone
                .glyphs
                .iter()
                .zip(inside)
                .all(|(a, b)| a.gid == b.gid && a.font == b.font);
        match out.last_mut() {
            Some(unit) if !separable && unit.joined && unit.rtl == cluster.rtl => {
                unit.bytes.start = unit.bytes.start.min(cluster.bytes.start);
                unit.bytes.end = unit.bytes.end.max(cluster.bytes.end);
                unit.x0 = unit.x0.min(cluster.x0);
                unit.first = unit.first.min(base + rank[k]);
            }
            _ => out.push(Unit {
                bytes: cluster.bytes.clone(),
                x0: cluster.x0,
                first: base + rank[k],
                rtl: cluster.rtl,
                joined: !separable,
            }),
        }
    }
    out
}

/// The commands that paint `lines` revealed at `now_ms`, glyph indices running on across lines in reading order; `None`
/// when no clock is running or every glyph is done, where the text paints as it does without `reveal`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint(
    reveal: &Reveal,
    now_ms: f64,
    lines: &[Line],
    size: f32,
    bold: bool,
    family: TextFamily,
    color: [f32; 4],
    weight: u16,
) -> Option<Vec<DrawCommand>> {
    if now_ms <= 0.0 {
        return None;
    }
    let shaped: Vec<crate::vector::Shaped> = lines
        .iter()
        .map(|line| crate::vector::shape(line.text, size, bold, family))
        .collect();
    let total: usize = shaped.iter().map(|s| s.clusters.len()).sum();
    if now_ms >= reveal.end_ms(total) {
        return None;
    }
    let mut out = Vec::new();
    let mut base = 0;
    for (line, shaped) in lines.iter().zip(&shaped) {
        for unit in units(line.text, shaped, bold, size, family, base) {
            let text = &line.text[unit.bytes];
            let (factor, lift) = reveal.progress(unit.first, now_ms);
            if factor <= 0.0 || text.trim().is_empty() {
                continue;
            }
            let mut tint = color;
            tint[3] *= factor;
            out.push(DrawCommand::Text {
                text: text.to_owned(),
                pos: Vec2 {
                    x: line.left + unit.x0,
                    y: line.y + lift,
                },
                size,
                color: tint,
                align: TextAlign::Left,
                weight,
                halo: None,
                family,
            });
        }
        base += shaped.clusters.len();
    }
    Some(out)
}
