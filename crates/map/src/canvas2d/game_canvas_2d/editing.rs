//! An edited `draw-text`: its caret and selection over the laid-out lines.

use super::*;

/// The canvas's 2D text painter maps through the same vector font pipeline as
/// other Canvas labels. Unsupported CSS fonts/baselines are reported rather
/// than silently given a different face or anchor. With a `width` the text is a
/// paragraph laid out from the top-left of `position` (see [`layout_paragraph`]).
/// A `draw-text`'s editing decorations (contract §11): caret, selection band and the input method's preedit.
pub(super) struct TextEditing {
    pub(super) caret: Option<usize>,
    pub(super) selection: Option<(usize, usize)>,
    pub(super) preedit: String,
    pub(super) caret_color: Option<[f32; 4]>,
    pub(super) selection_color: Option<[f32; 4]>,
}

impl TextEditing {
    pub(super) fn read(node: &Value) -> Result<Self, CanvasSceneError> {
        let index = |field: &str| -> Result<Option<usize>, CanvasSceneError> {
            match node.get(field) {
                None | Some(Value::Null) => Ok(None),
                Some(value) => match value.as_f64() {
                    Some(v) if v.is_finite() && v < 0.0 => Ok(None),
                    Some(v) if v.is_finite() && v.fract() == 0.0 => Ok(Some(v as usize)),
                    _ => Err(invalid(field, "must be a whole number")),
                },
            }
        };
        let color = |field: &str| {
            node.get(field)
                .and_then(Value::as_str)
                .and_then(parse_css_color)
        };
        let selection = match (index("selectionStart")?, index("selectionEnd")?) {
            (Some(a), Some(b)) if a != b => Some((a.min(b), a.max(b))),
            _ => None,
        };
        Ok(Self {
            caret: index("caret")?,
            selection,
            preedit: node
                .get("preedit")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            caret_color: color("caretColor"),
            selection_color: color("selectionColor"),
        })
    }

    pub(super) fn active(&self) -> bool {
        self.caret.is_some() || self.selection.is_some()
    }

    /// The text as painted: the preedit is shown at the caret, though it is not part of the text.
    pub(super) fn shown(&self, text: &str) -> String {
        match self.caret.filter(|_| !self.preedit.is_empty()) {
            Some(caret) => {
                let at = text
                    .char_indices()
                    .nth(caret)
                    .map_or(text.len(), |(i, _)| i);
                format!("{}{}{}", &text[..at], self.preedit, &text[at..])
            }
            None => text.to_string(),
        }
    }

    /// A text index as an index of the shown text.
    pub(super) fn shown_index(&self, index: usize) -> usize {
        match self.caret {
            Some(caret) if index >= caret => index + self.preedit.chars().count(),
            _ => index,
        }
    }
}

/// The layout of each painted line of an editable text: its first character index in `shown`, width, caret edges and
/// per-character intervals from the shaped, bidi-reordered line. `None` when the lines do not join back into `shown`
/// exactly or a character has no shaped glyph.
pub(super) fn caret_positions(
    lines: &[String],
    shown: &str,
    size: f32,
    bold: bool,
    family: orbital_canvas_base::packet::TextFamily,
) -> Option<Vec<crate::edit_layout::EditLayout>> {
    if lines.concat() != shown {
        return None;
    }
    let mut start = 0;
    lines
        .iter()
        .map(|line| {
            let laid = crate::edit_layout::layout_line(line, start, size, bold, family);
            start += line.chars().count();
            laid
        })
        .collect()
}

/// One painted line of an editable text, in screen units: left edge, band top and height, first index, edges.
pub(super) struct EditLine {
    pub(super) left: f32,
    pub(super) top: f32,
    pub(super) height: f32,
    pub(super) start: usize,
    pub(super) edges: Vec<f32>,
    pub(super) spans: Vec<(f32, f32)>,
}

/// The merged visual intervals of the characters `from..to` of `line`, relative to its left edge.
pub(super) fn visual_ranges(line: &EditLine, from: usize, to: usize) -> Vec<(f32, f32)> {
    crate::edit_layout::EditLayout {
        start: line.start,
        width: 0.0,
        edges: Vec::new(),
        spans: line.spans.clone(),
    }
    .ranges(from, to)
}

/// Paint `editing`'s selection bands behind the text (inserted at `text_start`) and its preedit underline and caret
/// over it. The caret blinks on the scene clock (530 ms on, 530 ms off); a scene drawn with no clock shows it steady.
pub(super) fn paint_editing(
    editing: &TextEditing,
    lines: &[EditLine],
    text_color: [f32; 4],
    size: f32,
    epoch_now_ms: f64,
    text_start: usize,
    out: &mut Vec<DrawCommand>,
) {
    let x_of = |line: &EditLine, index: usize| {
        line.left + line.edges[(index - line.start).min(line.edges.len() - 1)]
    };
    let holds = |line: &EditLine, index: usize| {
        index >= line.start && index < line.start + line.edges.len()
    };
    if let Some((from, to)) = editing.selection {
        let (from, to) = (editing.shown_index(from), editing.shown_index(to));
        let band = editing.selection_color.unwrap_or([
            text_color[0],
            text_color[1],
            text_color[2],
            text_color[3] * 0.3,
        ]);
        let bands: Vec<DrawCommand> = lines
            .iter()
            .flat_map(|line| {
                let end = line.start + line.edges.len() - 1;
                let (a, b) = (from.max(line.start), to.min(end));
                let ranges = if a < b {
                    visual_ranges(line, a - line.start, b - line.start)
                } else {
                    Vec::new()
                };
                ranges.into_iter().map(move |(x0, x1)| DrawCommand::Quad {
                    rect: Rect {
                        x: line.left + x0,
                        y: line.top,
                        w: x1 - x0,
                        h: line.height,
                    },
                    color: band,
                })
            })
            .collect();
        out.splice(text_start..text_start, bands);
    }
    let Some(caret) = editing.caret else { return };
    let preedit = editing.preedit.chars().count();
    let at = editing.shown_index(caret);
    let line = lines
        .iter()
        .rev()
        .find(|line| holds(line, at))
        .or_else(|| lines.last());
    let Some(line) = line else { return };
    if preedit > 0 {
        for (x0, x1) in visual_ranges(
            line,
            caret.saturating_sub(line.start),
            (caret + preedit).saturating_sub(line.start),
        ) {
            out.push(DrawCommand::Quad {
                rect: Rect {
                    x: line.left + x0,
                    y: line.top + line.height - (size * 0.08).max(1.0),
                    w: x1 - x0,
                    h: (size * 0.06).max(1.0),
                },
                color: text_color,
            });
        }
    }
    let on = epoch_now_ms <= 0.0 || (epoch_now_ms / 530.0).floor() as i64 % 2 == 0;
    if on {
        let w = (size * 0.07).max(1.0);
        out.push(DrawCommand::Quad {
            rect: Rect {
                x: x_of(line, at) - w / 2.0,
                y: line.top,
                w,
                h: line.height,
            },
            color: editing.caret_color.unwrap_or(text_color),
        });
    }
}
