//! Caret and selection geometry of one painted line of editable text, from the shaped, bidi-reordered line.
//!
//! Each character gets the visual interval of its shaper cluster (a cluster of several characters, such as a lam-alef
//! ligature, is divided equally in logical order along the run's direction). A caret index sits on one edge of one
//! neighbouring character: where the neighbours' bidi levels match, the trailing edge of the previous character;
//! where they differ, the facing edge of the neighbour with the lower level (the one closer to the paragraph
//! direction); at the line's start and end, the edge of the first or last character when its level is the paragraph
//! level, else the paragraph direction's start or end of the line.

use crate::packet::TextFamily;
use unicode_bidi::BidiInfo;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EditLayout {
    pub start: usize,
    pub width: f32,
    /// The caret x of each logical index `0..=chars`, relative to the line's left edge.
    pub edges: Vec<f32>,
    /// The visual interval `(left, right)` of each character, relative to the line's left edge.
    pub spans: Vec<(f32, f32)>,
}

impl EditLayout {
    /// The merged visual intervals, relative to the line's left edge, of the characters `from..to` (indices local to the line).
    pub fn ranges(&self, from: usize, to: usize) -> Vec<(f32, f32)> {
        let to = to.min(self.spans.len());
        let mut spans: Vec<(f32, f32)> = self.spans[from.min(to)..to].to_vec();
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut merged: Vec<(f32, f32)> = Vec::new();
        for (x0, x1) in spans {
            match merged.last_mut() {
                Some(last) if x0 <= last.1 + 1e-3 => last.1 = last.1.max(x1),
                _ => merged.push((x0, x1)),
            }
        }
        merged
    }
}

/// The layout of `line`, whose first character is index `start` of the text. `None` when a character has no shaped glyph.
pub(crate) fn layout_line(
    line: &str,
    start: usize,
    size: f32,
    bold: bool,
    family: TextFamily,
) -> Option<EditLayout> {
    if line.is_empty() {
        return Some(EditLayout {
            start,
            width: 0.0,
            edges: vec![0.0],
            spans: Vec::new(),
        });
    }
    let shaped = crate::vector::shape(line, size, bold, family);
    let offsets: Vec<usize> = line.char_indices().map(|(i, _)| i).collect();
    let n = offsets.len();
    let mut spans: Vec<Option<(f32, f32)>> = vec![None; n];
    for cluster in &shaped.clusters {
        let first = offsets.partition_point(|o| *o < cluster.bytes.start);
        let last = offsets.partition_point(|o| *o < cluster.bytes.end);
        let m = last - first;
        let step = (cluster.x1 - cluster.x0) / m.max(1) as f32;
        for (j, slot) in spans[first..last].iter_mut().enumerate() {
            let k = if cluster.rtl { m - 1 - j } else { j };
            *slot = Some((
                cluster.x0 + step * k as f32,
                cluster.x0 + step * (k + 1) as f32,
            ));
        }
    }
    let spans: Vec<(f32, f32)> = spans.into_iter().collect::<Option<_>>()?;
    let info = BidiInfo::new(line, None);
    let level_of = |k: usize| info.levels[offsets[k]];
    let first_para = info.paragraphs.first()?.level;
    let last_para = info.paragraphs.last()?.level;
    let width = shaped.width;
    let trailing = |k: usize| {
        if level_of(k).is_rtl() {
            spans[k].0
        } else {
            spans[k].1
        }
    };
    let leading = |k: usize| {
        if level_of(k).is_rtl() {
            spans[k].1
        } else {
            spans[k].0
        }
    };
    let edges = (0..=n)
        .map(|i| match (i.checked_sub(1), (i < n).then_some(i)) {
            (Some(l), Some(r)) => {
                if level_of(l) <= level_of(r) {
                    trailing(l)
                } else {
                    leading(r)
                }
            }
            (None, Some(r)) => {
                if level_of(r) == first_para {
                    leading(r)
                } else if first_para.is_rtl() {
                    width
                } else {
                    0.0
                }
            }
            (Some(l), None) => {
                if level_of(l) == last_para {
                    trailing(l)
                } else if last_para.is_rtl() {
                    0.0
                } else {
                    width
                }
            }
            (None, None) => 0.0,
        })
        .collect();
    Some(EditLayout {
        start,
        width,
        edges,
        spans,
    })
}
