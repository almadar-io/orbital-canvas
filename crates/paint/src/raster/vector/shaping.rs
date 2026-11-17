//! Text shaped into placed glyphs (joining, ligatures, marks and bidi order), and its widths and metrics.

use super::*;

/// One shaped glyph: `x` is the pen position plus the shaper's offset and `y`
/// the vertical offset below the baseline, both in the units of `size`.
#[derive(Debug, Copy, Clone)]
pub struct PlacedGlyph {
    pub font: FontId,
    pub gid: u16,
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Default, Clone)]
pub struct Shaped {
    pub glyphs: Vec<PlacedGlyph>,
    pub width: f32,
    pub clusters: Vec<ShapedCluster>,
}

/// The text bytes one shaper cluster covers and the horizontal extent `x0..x1` of its glyphs (`x0 <= x1`), in the
/// units of `size`; `rtl` is its bidi run's direction.
#[derive(Debug, Clone)]
pub struct ShapedCluster {
    pub bytes: Range<usize>,
    pub x0: f32,
    pub x1: f32,
    pub rtl: bool,
    /// The indices into [`Shaped::glyphs`] of the glyphs this cluster shaped to.
    pub glyphs: Range<usize>,
}

/// The face for `c` in `script`: the mono face when asked and it covers `c`, bundled Arabic, the bundled face when its cmap covers the
/// char, a host face for the script, else the bundled face (`.notdef`) with the script noted.
pub(super) fn char_font(c: char, script: Script, bold: bool, family: Family) -> FontId {
    let family = match family {
        Family::Face(id) => {
            if let Some(url) = url_font(id, bold)
                .filter(|f| font_for(*f).is_some_and(|f| f.glyph_index(c).is_some()))
            {
                return url;
            }
            orbital_canvas_base::faces::face_file(id).map_or(Family::Sans, |f| f.fallback)
        }
        other => other,
    };
    if family == Family::Mono {
        let mono = if bold { FontId::MonoBold } else { FontId::Mono };
        if font_for(mono).is_some_and(|f| f.glyph_index(c).is_some()) {
            return mono;
        }
    }
    if family == Family::Serif {
        let serif = if bold {
            FontId::SerifBold
        } else {
            FontId::Serif
        };
        if font_for(serif).is_some_and(|f| f.glyph_index(c).is_some()) {
            return serif;
        }
    }
    let named = match (family, bold) {
        (Family::Display, false) => Some(FontId::Display),
        (Family::Display, true) => Some(FontId::DisplayBold),
        (Family::Body, false) => Some(FontId::Body),
        (Family::Body, true) => Some(FontId::BodyBold),
        _ => None,
    };
    if let Some(named) =
        named.filter(|id| font_for(*id).is_some_and(|f| f.glyph_index(c).is_some()))
    {
        return named;
    }
    if script == Script::Arabic {
        return if bold {
            FontId::ArabicBold
        } else {
            FontId::ArabicRegular
        };
    }
    if face().is_some_and(|f| f.glyph_index(c).is_some()) {
        return FontId::Latin;
    }
    if let Some(i) = host_fonts()
        .read()
        .ok()
        .and_then(|h| h.iter().position(|(s, _)| *s == script))
    {
        return FontId::Host(i as u16);
    }
    if !matches!(script, Script::Common | Script::Inherited | Script::Unknown) {
        note_missing(script, c);
    }
    FontId::Latin
}

/// Face per char; Common/Inherited chars take the preceding script (the
/// following one when the text opens with them).
pub(super) fn char_fonts(text: &str, bold: bool, family: Family) -> Vec<FontId> {
    let scripts: Vec<Script> = text.chars().map(|c| c.script()).collect();
    let neutral = |s: Script| matches!(s, Script::Common | Script::Inherited);
    let first = scripts.iter().copied().find(|s| !neutral(*s));
    let mut current = first.unwrap_or(Script::Latin);
    text.chars()
        .zip(&scripts)
        .map(|(c, s)| {
            if !neutral(*s) {
                current = *s;
            }
            char_font(c, current, bold, family)
        })
        .collect()
}

/// Shape `text` at `size` into glyphs laid out left to right in visual order.
pub fn shape(text: &str, size: f32, bold: bool, family: Family) -> Shaped {
    shape_in(text, size, bold, family)
}

/// The most shaped texts the cache holds before it starts over.
const SHAPE_CACHE_ENTRIES: usize = 16_384;

pub(crate) type ShapeKey = (String, u32, bool, Family);

/// Shaped texts by their inputs, valid for one (font generation, asset generation): a face that loads, or a theme
/// face's file that arrives, can change how the same text shapes.
#[derive(Default)]
pub(crate) struct ShapeCache {
    generations: (u64, u64),
    entries: std::collections::HashMap<ShapeKey, Shaped>,
}

impl ShapeCache {
    pub(crate) fn get(&mut self, generations: (u64, u64), key: &ShapeKey) -> Option<Shaped> {
        if self.generations != generations {
            self.entries.clear();
            self.generations = generations;
        }
        self.entries.get(key).cloned()
    }

    pub(crate) fn put(&mut self, generations: (u64, u64), key: ShapeKey, shaped: &Shaped) {
        if self.generations != generations {
            return;
        }
        if self.entries.len() >= SHAPE_CACHE_ENTRIES {
            self.entries.clear();
        }
        self.entries.insert(key, shaped.clone());
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

fn shape_cache() -> &'static Mutex<ShapeCache> {
    static CACHE: OnceLock<Mutex<ShapeCache>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// [`shape`] in a generic family, kept by its inputs until a font or an asset (a theme face's file) arrives.
pub fn shape_in(text: &str, size: f32, bold: bool, family: Family) -> Shaped {
    let generations = (font_generation(), crate::asset::generation());
    let key = (text.to_owned(), size.to_bits(), bold, family);
    let cached = shape_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(generations, &key);
    if let Some(shaped) = cached {
        return shaped;
    }
    let shaped = shape_uncached(text, size, bold, family);
    shape_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .put(generations, key, &shaped);
    shaped
}

pub(crate) fn shape_uncached(text: &str, size: f32, bold: bool, family: Family) -> Shaped {
    let mut out = Shaped::default();
    if text.is_empty() {
        return out;
    }
    let fonts = char_fonts(text, bold, family);
    let font_at = |byte: usize| fonts[text[..byte].chars().count()];
    let info = BidiInfo::new(text, None);
    let mut pen = 0.0f32;
    for para in &info.paragraphs {
        let (levels, runs) = info.visual_runs(para, para.range.clone());
        for run in runs {
            let rtl = levels[run.start].is_rtl();
            let mut pieces: Vec<(FontId, Range<usize>)> = Vec::new();
            for (i, c) in text[run.clone()].char_indices() {
                let at = run.start + i;
                let font = font_at(at);
                match pieces.last_mut() {
                    Some((f, r)) if *f == font => r.end = at + c.len_utf8(),
                    _ => pieces.push((font, at..at + c.len_utf8())),
                }
            }
            if rtl {
                pieces.reverse();
            }
            for (font, range) in pieces {
                let Some(face) = font_for(font) else { continue };
                let scale = size / face.units_per_em() as f32;
                let mut buffer = UnicodeBuffer::new();
                buffer.push_str(&text[range.clone()]);
                buffer.set_direction(if rtl {
                    Direction::RightToLeft
                } else {
                    Direction::LeftToRight
                });
                let shaped = rustybuzz::shape(face, features_for(font), buffer);
                let piece_len = range.len();
                let piece_start = range.start;
                let mut groups: Vec<(u32, f32, f32, usize, usize)> = Vec::new();
                for (info, pos) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
                    let advance = pos.x_advance as f32 * scale;
                    match groups.last_mut() {
                        Some((c, _, x1, _, end)) if *c == info.cluster => {
                            *x1 = pen + advance;
                            *end = out.glyphs.len() + 1;
                        }
                        _ => groups.push((
                            info.cluster,
                            pen,
                            pen + advance,
                            out.glyphs.len(),
                            out.glyphs.len() + 1,
                        )),
                    }
                    out.glyphs.push(PlacedGlyph {
                        font,
                        gid: info.glyph_id as u16,
                        x: pen + pos.x_offset as f32 * scale,
                        y: -(pos.y_offset as f32) * scale,
                    });
                    pen += advance;
                }
                let mut starts: Vec<u32> = groups.iter().map(|g| g.0).collect();
                starts.sort_unstable();
                for (cluster, x0, x1, first, last) in groups {
                    let end = starts
                        .iter()
                        .copied()
                        .find(|s| *s > cluster)
                        .map_or(piece_len, |s| s as usize);
                    out.clusters.push(ShapedCluster {
                        bytes: piece_start + cluster as usize..piece_start + end,
                        x0: x0.min(x1),
                        x1: x0.max(x1),
                        rtl,
                        glyphs: first..last,
                    });
                }
            }
        }
    }
    out.width = pen;
    out
}

/// Advance width of `text` at `size`, in the same units as `size`.
pub fn text_width(text: &str, size: f32) -> f32 {
    shape(text, size, false, Family::Sans).width
}

/// [`text_width`] in the monospace face.
pub fn mono_text_width(text: &str, size: f32) -> f32 {
    shape(text, size, false, Family::Mono).width
}

/// The Latin face's ascent and descent in packet units. The vector text path
/// uses this same face for its anchor, so drawable baselines can map to it.
pub fn text_font_metrics(size: f32) -> Option<(f32, f32)> {
    let base = face()?;
    let scale = size / base.units_per_em() as f32;
    Some((
        base.ascender() as f32 * scale,
        base.descender() as f32 * scale,
    ))
}

/// The highest ascent and the lowest descent among the faces `text` is shaped with, in packet units. A face taller
/// than the Latin one (the Arabic face, whose dots hang under its letters) reaches past [`text_font_metrics`].
pub fn text_run_extent(text: &str, size: f32, bold: bool, family: Family) -> Option<(f32, f32)> {
    let (mut top, mut bottom) = text_font_metrics(size)?;
    for font in char_fonts(text, bold, family) {
        if let Some(face) = font_for(font) {
            let scale = size / face.units_per_em() as f32;
            top = top.max(face.ascender() as f32 * scale);
            bottom = bottom.min(face.descender() as f32 * scale);
        }
    }
    Some((top, bottom))
}
