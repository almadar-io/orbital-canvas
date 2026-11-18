//! Paragraph layout: lines broken to a width as the web breaks them, and ellipsis.

use unicode_segmentation::UnicodeSegmentation;

/// The lines of a paragraph and each line's advance width, in the units of `size`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Paragraph {
    pub lines: Vec<String>,
    /// The index in the text of each line's first character.
    pub starts: Vec<usize>,
    pub widths: Vec<f32>,
    /// A word wider than the box was broken across lines.
    pub broken: bool,
}

/// Lay `text` out in lines of at most `width`. Line breaks fall at the opportunities of [`break_segments`]; a
/// word wider than the line is broken at the last grapheme that fits (or cut with an ellipsis when `truncate`). `max_lines` keeps the
/// first lines, the last ending in an ellipsis when `truncate`. Widths come from the shaper's advances.
/// How a paragraph breaks: its line width and the wrap, line-cap and ellipsis rules.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ParagraphBox {
    pub width: f32,
    pub wrap: bool,
    pub max_lines: Option<usize>,
    pub truncate: bool,
}

type ParagraphKey = (
    String,
    u32,
    bool,
    orbital_canvas_base::packet::TextFamily,
    u32,
    bool,
    Option<usize>,
    bool,
);

const PARAGRAPH_CACHE_ENTRIES: usize = 16_384;

/// Laid-out paragraphs by their inputs, valid for one (font generation, asset generation).
#[derive(Default)]
struct ParagraphCache {
    generations: (u64, u64),
    entries: std::collections::HashMap<ParagraphKey, Paragraph>,
}

impl ParagraphCache {
    fn get(&mut self, generations: (u64, u64), key: &ParagraphKey) -> Option<Paragraph> {
        if self.generations != generations {
            self.entries.clear();
            self.generations = generations;
        }
        self.entries.get(key).cloned()
    }

    fn put(&mut self, generations: (u64, u64), key: ParagraphKey, paragraph: &Paragraph) {
        if self.generations != generations {
            return;
        }
        if self.entries.len() >= PARAGRAPH_CACHE_ENTRIES {
            self.entries.clear();
        }
        self.entries.insert(key, paragraph.clone());
    }
}

fn paragraph_cache() -> &'static std::sync::Mutex<ParagraphCache> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<ParagraphCache>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// How many paragraphs the cache holds.
#[cfg(test)]
pub(crate) fn paragraph_cache_len() -> usize {
    paragraph_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entries
        .len()
}

pub(crate) fn layout_paragraph(
    text: &str,
    size: f32,
    bold: bool,
    family: orbital_canvas_base::packet::TextFamily,
    frame: ParagraphBox,
) -> Paragraph {
    let generations = (
        orbital_canvas_paint::vector::font_generation(),
        orbital_canvas_paint::asset::generation(),
    );
    let key = (
        text.to_owned(),
        size.to_bits(),
        bold,
        family,
        frame.width.to_bits(),
        frame.wrap,
        frame.max_lines,
        frame.truncate,
    );
    let cached = paragraph_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(generations, &key);
    if let Some(paragraph) = cached {
        return paragraph;
    }
    let paragraph = layout_paragraph_uncached(text, size, bold, family, frame);
    paragraph_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .put(generations, key, &paragraph);
    paragraph
}

pub(super) fn layout_paragraph_uncached(
    text: &str,
    size: f32,
    bold: bool,
    family: orbital_canvas_base::packet::TextFamily,
    frame: ParagraphBox,
) -> Paragraph {
    let ParagraphBox {
        width,
        wrap,
        max_lines,
        truncate,
    } = frame;
    let measure = |s: &str| orbital_canvas_paint::vector::shape(s, size, bold, family).width;
    let mut lines: Vec<String> = Vec::new();
    let mut starts: Vec<usize> = Vec::new();
    let mut hard_start = 0;
    let mut broken = false;
    for hard in text.split('\n') {
        let hard_chars = hard.chars().count();
        if !wrap {
            lines.push(hard.trim_end().to_string());
            starts.push(hard_start);
            hard_start += hard_chars + 1;
            continue;
        }
        let mut line = String::new();
        let mut line_start = hard_start;
        let mut segment_start = hard_start;
        let mut pieces: Vec<String> = Vec::new();
        for segment in break_segments(hard) {
            let split = if truncate {
                None
            } else {
                split_wide_word(segment, width, &measure)
            };
            broken |= split.is_some();
            match split {
                Some(chunks) => pieces.extend(chunks),
                None => pieces.push(segment.to_string()),
            }
        }
        for segment in pieces.iter().map(String::as_str) {
            let candidate = format!("{line}{segment}");
            if line.is_empty() || measure(candidate.trim_end()) <= width {
                line = candidate;
            } else {
                lines.push(line.trim_end().to_string());
                starts.push(line_start);
                line = segment.to_string();
                line_start = segment_start;
            }
            segment_start += segment.chars().count();
        }
        lines.push(line.trim_end().to_string());
        starts.push(line_start);
        hard_start += hard_chars + 1;
    }
    let mut cut = false;
    if let Some(max) = max_lines {
        if lines.len() > max {
            lines.truncate(max);
            starts.truncate(max);
            cut = true;
        }
    }
    if truncate {
        let last = lines.len().saturating_sub(1);
        for (i, line) in lines.iter_mut().enumerate() {
            if (i == last && cut) || measure(line.as_str()) > width {
                *line = ellipsize(line, width, &measure);
            }
        }
    }
    let widths = lines.iter().map(|line| measure(line.as_str())).collect();
    Paragraph {
        lines,
        starts,
        widths,
        broken,
    }
}

/// `segment` cut into pieces that each fit `width` at the last grapheme that does, when it is wider on a line of its own.
fn split_wide_word(
    segment: &str,
    width: f32,
    measure: &dyn Fn(&str) -> f32,
) -> Option<Vec<String>> {
    let word = segment.trim_end();
    if measure(word) <= width {
        return None;
    }
    let tail = &segment[word.len()..];
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    for grapheme in word.graphemes(true) {
        if !current.is_empty() && measure(&format!("{current}{grapheme}")) > width {
            chunks.push(std::mem::take(&mut current));
        }
        current.push_str(grapheme);
    }
    current.push_str(tail);
    chunks.push(current);
    Some(chunks)
}

/// `line` with its tail replaced by an ellipsis so that it fits `width`.
pub(super) fn ellipsize(line: &str, width: f32, measure: &dyn Fn(&str) -> f32) -> String {
    let mut kept: Vec<char> = line.trim_end().chars().collect();
    loop {
        let mut candidate: String = kept.iter().collect();
        candidate.push('\u{2026}');
        if kept.is_empty() || measure(&candidate) <= width {
            return candidate;
        }
        kept.pop();
        while kept.last().is_some_and(|c| c.is_whitespace()) {
            kept.pop();
        }
    }
}

/// Split one hard line into pieces a line may break between: after spaces (UAX 14 LB18, none before a space or
/// around a no-break space), after a hyphen unless a digit follows (LB21, LB25) or a zero-width space, and between
/// ideographs, kana and Hangul syllables (LB31).
pub(super) fn break_segments(text: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    let mut start = 0;
    let mut previous: Option<char> = None;
    for (i, c) in text.char_indices() {
        if let Some(p) = previous {
            if breaks_between(p, c) {
                segments.push(&text[start..i]);
                start = i;
            }
        }
        previous = Some(c);
    }
    if start < text.len() {
        segments.push(&text[start..]);
    }
    segments
}

pub(super) fn is_ideographic(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0x20000..=0x2FFFF)
}

pub(super) fn breaks_between(previous: char, next: char) -> bool {
    if previous == '\u{00A0}' || next == '\u{00A0}' || next.is_whitespace() {
        return false;
    }
    if previous.is_whitespace() || previous == '\u{200B}' {
        return true;
    }
    if matches!(previous, '-' | '\u{2010}' | '\u{2013}') {
        return !next.is_ascii_digit();
    }
    is_ideographic(previous) || is_ideographic(next)
}
