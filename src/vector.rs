//! CPU vector layer: `Path`, `Line` and `Text` commands rasterized anti-aliased
//! (tiny-skia) into a straight-alpha RGBA buffer that the GPU renderer
//! composites as one texture, so vector marks keep their place in the packet's
//! z-order between quads and sprites.
//!
//! Text is drawn from glyph outlines of the embedded fonts (`assets/fonts`).
//! Each label is split into bidi runs (unicode-bidi), each run into script
//! runs (unicode-script) that pick a face, and every run is shaped with
//! rustybuzz, so Arabic joins, ligates and runs right to left. Latin and
//! Greek use a DejaVu Sans subset, Arabic a Noto Sans Arabic subset (real
//! Regular and Bold). Scripts with no bundled face (CJK, Hebrew, Indic) draw
//! the DejaVu `.notdef` box.

use crate::packet::{DrawCommand, LineCap, LineJoin, PathSegment, Stroke, TextAlign, Vec2};
use rustybuzz::{Direction, UnicodeBuffer};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};
use tiny_skia::{
    FillRule, LineCap as SkCap, LineJoin as SkJoin, Mask, Paint, PathBuilder, Pixmap, StrokeDash,
    Transform,
};
use unicode_bidi::BidiInfo;
use unicode_script::{Script, UnicodeScript};

static LATIN_BYTES: &[u8] = include_bytes!("../assets/fonts/DejaVuSans-subset.ttf");
static ARABIC_REGULAR_BYTES: &[u8] =
    include_bytes!("../assets/fonts/NotoSansArabic-Regular-subset.ttf");
static ARABIC_BOLD_BYTES: &[u8] = include_bytes!("../assets/fonts/NotoSansArabic-Bold-subset.ttf");
static MONO_BYTES: &[u8] = include_bytes!("../assets/fonts/DejaVuSansMono-subset.ttf");

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum FontId {
    Latin,
    ArabicRegular,
    ArabicBold,
    /// DejaVu Sans Mono: text a theme draws in its `mono` face.
    Mono,
    /// A face the host supplied for a script no bundled face covers (`add_font_face`).
    Host(u16),
}

impl FontId {
    /// Only the bundled Arabic has a real bold; every other face is emboldened by stroking.
    fn faux_bold(self) -> bool {
        !matches!(self, FontId::ArabicRegular | FontId::ArabicBold)
    }
}

fn font_for(id: FontId) -> Option<&'static rustybuzz::Face<'static>> {
    static FACES: OnceLock<[Option<rustybuzz::Face<'static>>; 4]> = OnceLock::new();
    let faces = FACES.get_or_init(|| {
        [
            rustybuzz::Face::from_slice(LATIN_BYTES, 0),
            rustybuzz::Face::from_slice(ARABIC_REGULAR_BYTES, 0),
            rustybuzz::Face::from_slice(ARABIC_BOLD_BYTES, 0),
            rustybuzz::Face::from_slice(MONO_BYTES, 0),
        ]
    });
    match id {
        FontId::Latin => faces[0].as_ref(),
        FontId::ArabicRegular => faces[1].as_ref(),
        FontId::ArabicBold => faces[2].as_ref(),
        FontId::Mono => faces[3].as_ref(),
        FontId::Host(i) => host_fonts().read().ok()?.get(i as usize).map(|(_, f)| *f),
    }
}

type HostFaces = RwLock<Vec<(Script, &'static rustybuzz::Face<'static>)>>;

fn host_fonts() -> &'static HostFaces {
    static HOST: OnceLock<HostFaces> = OnceLock::new();
    HOST.get_or_init(|| RwLock::new(Vec::new()))
}

/// Scripts text needed that no face covers: `pending` until the host drains them, `samples`
/// (one char per script ever reported) so each is asked for once and a supplied face can be
/// checked against it.
#[derive(Default)]
struct MissingScripts {
    pending: Vec<MissingScript>,
    samples: std::collections::HashMap<Script, char>,
}

/// One script painted text needed, with a char of it the host can ask its font system for.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct MissingScript {
    /// ISO 15924 short name (`Hebr`).
    pub script: String,
    pub sample: String,
}

fn missing() -> &'static Mutex<MissingScripts> {
    static MISSING: OnceLock<Mutex<MissingScripts>> = OnceLock::new();
    MISSING.get_or_init(|| Mutex::new(MissingScripts::default()))
}

static FONT_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Tests that add a face (moving [`font_generation`]) and tests that depend on it holding still
/// take turns.
#[cfg(test)]
pub(crate) static FONT_GENERATION_TEST_LOCK: Mutex<()> = Mutex::new(());

/// Register the host platform's face for `script` (ISO 15924 short name, `Hebr`), face `index`
/// of a collection (0 for a single font). Text in that
/// script shapes with it from the next paint; [`font_generation`] moves so hosts repaint. A face
/// with no glyph for the char the script was reported with is refused, so the host can try its
/// next candidate.
pub fn add_font_face(script: &str, bytes: Vec<u8>, index: u32) -> Result<(), String> {
    let script =
        Script::from_short_name(script).ok_or_else(|| format!("unknown script `{script}`"))?;
    let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
    let face = rustybuzz::Face::from_slice(bytes, index).ok_or("the bytes are not a font face")?;
    let sample = missing()
        .lock()
        .ok()
        .and_then(|m| m.samples.get(&script).copied());
    if let Some(c) = sample.filter(|c| face.glyph_index(*c).is_none()) {
        return Err(format!(
            "the face has no glyph for `{c}` ({})",
            script.short_name()
        ));
    }
    let face: &'static rustybuzz::Face<'static> = Box::leak(Box::new(face));
    let mut hosts = host_fonts().write().map_err(|_| "font registry poisoned")?;
    if hosts.iter().all(|(s, _)| *s != script) {
        hosts.push((script, face));
    }
    FONT_GENERATION.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

/// Scripts that text needed and no bundled or host face covers, each returned once. Such text
/// draws the bundled face's `.notdef` until the host supplies a face.
pub fn take_missing_scripts() -> Vec<MissingScript> {
    missing()
        .lock()
        .map(|mut m| std::mem::take(&mut m.pending))
        .unwrap_or_default()
}

/// Moves whenever a host face is added, so a host knows painted text may change.
pub fn font_generation() -> u64 {
    FONT_GENERATION.load(Ordering::SeqCst)
}

fn note_missing(script: Script, sample: char) {
    if let Ok(mut m) = missing().lock() {
        if let std::collections::hash_map::Entry::Vacant(slot) = m.samples.entry(script) {
            slot.insert(sample);
            m.pending.push(MissingScript {
                script: script.short_name().to_string(),
                sample: sample.to_string(),
            });
        }
    }
}

fn face() -> Option<&'static rustybuzz::Face<'static>> {
    font_for(FontId::Latin)
}

/// One shaped glyph: `x` is the pen position plus the shaper's offset and `y`
/// the vertical offset below the baseline, both in the units of `size`.
#[derive(Debug, Copy, Clone)]
pub(crate) struct PlacedGlyph {
    pub font: FontId,
    pub gid: u16,
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Default)]
pub(crate) struct Shaped {
    pub glyphs: Vec<PlacedGlyph>,
    pub width: f32,
}

/// The face for `c` in `script`: the mono face when asked and it covers `c`, bundled Arabic, the bundled face when its cmap covers the
/// char, a host face for the script, else the bundled face (`.notdef`) with the script noted.
fn char_font(c: char, script: Script, bold: bool, mono: bool) -> FontId {
    if mono && font_for(FontId::Mono).is_some_and(|f| f.glyph_index(c).is_some()) {
        return FontId::Mono;
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
fn char_fonts(text: &str, bold: bool, mono: bool) -> Vec<FontId> {
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
            char_font(c, current, bold, mono)
        })
        .collect()
}

/// Shape `text` at `size` into glyphs laid out left to right in visual order.
pub(crate) fn shape(text: &str, size: f32, bold: bool, mono: bool) -> Shaped {
    let mut out = Shaped::default();
    if text.is_empty() {
        return out;
    }
    let fonts = char_fonts(text, bold, mono);
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
                buffer.push_str(&text[range]);
                buffer.set_direction(if rtl {
                    Direction::RightToLeft
                } else {
                    Direction::LeftToRight
                });
                let shaped = rustybuzz::shape(face, &[], buffer);
                for (info, pos) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
                    out.glyphs.push(PlacedGlyph {
                        font,
                        gid: info.glyph_id as u16,
                        x: pen + pos.x_offset as f32 * scale,
                        y: -(pos.y_offset as f32) * scale,
                    });
                    pen += pos.x_advance as f32 * scale;
                }
            }
        }
    }
    out.width = pen;
    out
}

/// Whether `command` is drawn by the vector layer.
pub fn is_vector(command: &DrawCommand) -> bool {
    matches!(
        command,
        DrawCommand::Path { .. }
            | DrawCommand::ClippedPath { .. }
            | DrawCommand::Line { .. }
            | DrawCommand::Text { .. }
    )
}

/// Advance width of `text` at `size`, in the same units as `size`.
pub fn text_width(text: &str, size: f32) -> f32 {
    shape(text, size, false, false).width
}

/// [`text_width`] in the monospace face.
pub fn mono_text_width(text: &str, size: f32) -> f32 {
    shape(text, size, false, true).width
}

fn color(c: [f32; 4]) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba(
        c[0].clamp(0.0, 1.0),
        c[1].clamp(0.0, 1.0),
        c[2].clamp(0.0, 1.0),
        c[3].clamp(0.0, 1.0),
    )
    .unwrap_or(tiny_skia::Color::TRANSPARENT)
}

fn paint(c: [f32; 4]) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(color(c));
    p.anti_alias = true;
    p
}

fn sk_stroke(stroke: &Stroke) -> tiny_skia::Stroke {
    let mut s = tiny_skia::Stroke {
        width: stroke.width.max(0.0),
        line_cap: match stroke.cap {
            LineCap::Butt => SkCap::Butt,
            LineCap::Round => SkCap::Round,
            LineCap::Square => SkCap::Square,
        },
        line_join: match stroke.join {
            LineJoin::Miter => SkJoin::Miter,
            LineJoin::Round => SkJoin::Round,
            LineJoin::Bevel => SkJoin::Bevel,
        },
        ..tiny_skia::Stroke::default()
    };
    if !stroke.dash.is_empty() {
        let mut intervals = stroke.dash.clone();
        if intervals.len() % 2 == 1 {
            intervals.extend_from_within(..);
        }
        s.dash = StrokeDash::new(intervals, stroke.dash_offset);
    }
    s
}

/// Append an ellipse arc as cubic segments of at most a quarter turn. Joined to
/// the current point by a line, or starts the subpath when there is none.
fn arc_to(
    b: &mut PathBuilder,
    has_point: bool,
    (cx, cy, rx, ry): (f32, f32, f32, f32),
    start: f32,
    end: f32,
) {
    let mut sweep = end - start;
    let full = std::f32::consts::TAU;
    if sweep.abs() >= full - 1e-6 {
        sweep = full.copysign(sweep);
    }
    let (sx, sy) = (cx + rx * start.cos(), cy + ry * start.sin());
    if has_point {
        b.line_to(sx, sy);
    } else {
        b.move_to(sx, sy);
    }
    let pieces = (sweep.abs() / std::f32::consts::FRAC_PI_2).ceil().max(1.0) as usize;
    let da = sweep / pieces as f32;
    let k = 4.0 / 3.0 * (da / 4.0).tan();
    let mut a = start;
    for _ in 0..pieces {
        let b_ang = a + da;
        let (sa, ca) = a.sin_cos();
        let (sb, cb) = b_ang.sin_cos();
        b.cubic_to(
            cx + rx * (ca - k * sa),
            cy + ry * (sa + k * ca),
            cx + rx * (cb + k * sb),
            cy + ry * (sb - k * cb),
            cx + rx * cb,
            cy + ry * sb,
        );
        a = b_ang;
    }
}

fn build_path(segments: &[PathSegment]) -> Option<tiny_skia::Path> {
    let mut b = PathBuilder::new();
    let mut has_point = false;
    for seg in segments {
        match *seg {
            PathSegment::MoveTo { x, y } => {
                b.move_to(x, y);
                has_point = true;
            }
            PathSegment::LineTo { x, y } => {
                if has_point {
                    b.line_to(x, y);
                } else {
                    b.move_to(x, y);
                    has_point = true;
                }
            }
            PathSegment::Arc {
                cx,
                cy,
                rx,
                ry,
                start,
                end,
            } => {
                arc_to(&mut b, has_point, (cx, cy, rx, ry), start, end);
                has_point = true;
            }
            PathSegment::CubicTo {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => {
                if !has_point {
                    b.move_to(x1, y1);
                    has_point = true;
                }
                b.cubic_to(x1, y1, x2, y2, x, y);
            }
            PathSegment::Close => {
                b.close();
            }
        }
    }
    b.finish()
}

struct GlyphOutline<'a>(&'a mut PathBuilder, f32, f32, f32);

impl GlyphOutline<'_> {
    fn p(&self, x: f32, y: f32) -> (f32, f32) {
        (self.1 + x * self.3, self.2 - y * self.3)
    }
}

impl ttf_parser::OutlineBuilder for GlyphOutline<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.p(x, y);
        self.0.move_to(x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.p(x, y);
        self.0.line_to(x, y);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (x1, y1) = self.p(x1, y1);
        let (x, y) = self.p(x, y);
        self.0.quad_to(x1, y1, x, y);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (x1, y1) = self.p(x1, y1);
        let (x2, y2) = self.p(x2, y2);
        let (x, y) = self.p(x, y);
        self.0.cubic_to(x1, y1, x2, y2, x, y);
    }
    fn close(&mut self) {
        self.0.close();
    }
}

struct TextPaths {
    all: tiny_skia::Path,
    faux_bold: Option<tiny_skia::Path>,
}

/// Outline of `text` with its anchor placed per `align`, `pos.y` at the em box
/// middle of the Latin face. Bold glyphs from faces without a real bold are
/// returned apart so the caller can thicken them.
fn text_path(
    text: &str,
    pos: Vec2,
    size: f32,
    align: TextAlign,
    bold: bool,
    mono: bool,
) -> Option<TextPaths> {
    let base = face()?;
    let shaped = shape(text, size, bold, mono);
    let x0 = match align {
        TextAlign::Left => pos.x,
        TextAlign::Center => pos.x - shaped.width / 2.0,
        TextAlign::Right => pos.x - shaped.width,
    };
    let base_scale = size / base.units_per_em() as f32;
    let baseline = pos.y + (base.ascender() as f32 + base.descender() as f32) / 2.0 * base_scale;
    let mut all = PathBuilder::new();
    let mut faux = PathBuilder::new();
    for g in &shaped.glyphs {
        let Some(face) = font_for(g.font) else {
            continue;
        };
        let scale = size / face.units_per_em() as f32;
        let id = ttf_parser::GlyphId(g.gid);
        face.outline_glyph(
            id,
            &mut GlyphOutline(&mut all, x0 + g.x, baseline + g.y, scale),
        );
        if bold && g.font.faux_bold() {
            face.outline_glyph(
                id,
                &mut GlyphOutline(&mut faux, x0 + g.x, baseline + g.y, scale),
            );
        }
    }
    Some(TextPaths {
        all: all.finish()?,
        faux_bold: faux.finish(),
    })
}

fn draw_command(pixmap: &mut Pixmap, command: &DrawCommand, ts: Transform) {
    match command {
        DrawCommand::Path {
            segments,
            fill,
            stroke,
        } => {
            let Some(path) = build_path(segments) else {
                return;
            };
            if let Some(f) = fill {
                pixmap.fill_path(&path, &paint(*f), FillRule::Winding, ts, None);
            }
            if let Some(s) = stroke {
                pixmap.stroke_path(&path, &paint(s.color), &sk_stroke(s), ts, None);
            }
        }
        DrawCommand::ClippedPath {
            segments,
            fill,
            stroke,
            clip,
            cut,
        } => {
            let Some(path) = build_path(segments) else {
                return;
            };
            let Some(mask) = region_mask(pixmap.width(), pixmap.height(), clip, cut, ts) else {
                return;
            };
            if let Some(f) = fill {
                pixmap.fill_path(&path, &paint(*f), FillRule::Winding, ts, Some(&mask));
            }
            if let Some(s) = stroke {
                pixmap.stroke_path(&path, &paint(s.color), &sk_stroke(s), ts, Some(&mask));
            }
        }
        DrawCommand::Line {
            from,
            to,
            color: c,
            width,
        } => {
            let mut b = PathBuilder::new();
            b.move_to(from.x, from.y);
            b.line_to(to.x, to.y);
            if let Some(path) = b.finish() {
                let stroke = Stroke {
                    color: *c,
                    width: *width,
                    dash: Vec::new(),
                    dash_offset: 0.0,
                    cap: LineCap::Butt,
                    join: LineJoin::Miter,
                };
                pixmap.stroke_path(&path, &paint(*c), &sk_stroke(&stroke), ts, None);
            }
        }
        DrawCommand::Text {
            text,
            pos,
            size,
            color: c,
            align,
            weight,
            halo,
            mono,
        } => {
            let bold = *weight >= 600;
            let Some(paths) = text_path(text, *pos, *size, *align, bold, *mono) else {
                return;
            };
            let path = &paths.all;
            if let Some(h) = halo {
                let stroke = Stroke {
                    color: h.color,
                    width: h.width,
                    dash: Vec::new(),
                    dash_offset: 0.0,
                    cap: LineCap::Round,
                    join: LineJoin::Round,
                };
                pixmap.stroke_path(path, &paint(h.color), &sk_stroke(&stroke), ts, None);
            }
            pixmap.fill_path(path, &paint(*c), FillRule::Winding, ts, None);
            if let Some(faux) = &paths.faux_bold {
                let stroke = Stroke {
                    color: *c,
                    width: size * 0.045,
                    dash: Vec::new(),
                    dash_offset: 0.0,
                    cap: LineCap::Round,
                    join: LineJoin::Round,
                };
                pixmap.stroke_path(faux, &paint(*c), &sk_stroke(&stroke), ts, None);
            }
        }
        _ => {}
    }
}

/// Coverage where every `clip` path covers (all pixels when there is none) and no `cut` path does.
fn region_mask(
    w: u32,
    h: u32,
    clip: &[Vec<PathSegment>],
    cut: &[Vec<PathSegment>],
    ts: Transform,
) -> Option<Mask> {
    let mut mask = Mask::new(w, h)?;
    mask.data_mut().fill(255);
    for segments in clip {
        let path = build_path(segments)?;
        mask.intersect_path(&path, FillRule::Winding, true, ts);
    }
    for segments in cut {
        let Some(path) = build_path(segments) else {
            continue;
        };
        let mut hole = Mask::new(w, h)?;
        hole.fill_path(&path, FillRule::Winding, true, ts);
        for (m, c) in mask.data_mut().iter_mut().zip(hole.data()) {
            *m = ((*m as u32 * (255 - *c as u32) + 127) / 255) as u8;
        }
    }
    Some(mask)
}

/// Rasterize `commands` (vector commands only; others are ignored) onto a
/// transparent `px_w` x `px_h` surface, scaling packet units by `sx`/`sy`.
/// Returns straight-alpha RGBA8, or `None` when the surface cannot be allocated.
pub fn rasterize(
    commands: &[&DrawCommand],
    px_w: u32,
    px_h: u32,
    sx: f32,
    sy: f32,
) -> Option<Vec<u8>> {
    let mut pixmap = Pixmap::new(px_w.max(1), px_h.max(1))?;
    let ts = Transform::from_scale(sx, sy);
    for command in commands {
        draw_command(&mut pixmap, command, ts);
    }
    let mut data = pixmap.take();
    for px in data.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    Some(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

    fn at(data: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * w + x) * 4) as usize;
        [data[i], data[i + 1], data[i + 2], data[i + 3]]
    }

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Vec<PathSegment> {
        vec![
            PathSegment::MoveTo { x, y },
            PathSegment::LineTo { x: x + w, y },
            PathSegment::LineTo { x: x + w, y: y + h },
            PathSegment::LineTo { x, y: y + h },
            PathSegment::Close,
        ]
    }

    #[test]
    fn filled_path_paints_inside_and_not_outside() {
        let cmd = DrawCommand::Path {
            segments: rect(10.0, 10.0, 20.0, 20.0),
            fill: Some(RED),
            stroke: None,
        };
        let data = rasterize(&[&cmd], 40, 40, 1.0, 1.0).unwrap();
        assert_eq!(at(&data, 40, 20, 20), [255, 0, 0, 255]);
        assert_eq!(at(&data, 40, 5, 5)[3], 0);
    }

    #[test]
    fn density_scale_moves_the_mark_with_the_target() {
        let cmd = DrawCommand::Path {
            segments: rect(10.0, 10.0, 20.0, 20.0),
            fill: Some(RED),
            stroke: None,
        };
        let data = rasterize(&[&cmd], 80, 80, 2.0, 2.0).unwrap();
        assert_eq!(at(&data, 80, 40, 40), [255, 0, 0, 255]);
        assert_eq!(at(&data, 80, 15, 15)[3], 0);
    }

    #[test]
    fn dashed_stroke_leaves_gaps_a_solid_stroke_does_not() {
        let line = |dash: Vec<f32>| DrawCommand::Path {
            segments: vec![
                PathSegment::MoveTo { x: 0.0, y: 10.0 },
                PathSegment::LineTo { x: 60.0, y: 10.0 },
            ],
            fill: None,
            stroke: Some(Stroke {
                color: RED,
                width: 4.0,
                dash,
                dash_offset: 0.0,
                cap: LineCap::Butt,
                join: LineJoin::Miter,
            }),
        };
        let solid = rasterize(&[&line(vec![])], 60, 20, 1.0, 1.0).unwrap();
        let dashed = rasterize(&[&line(vec![6.0, 4.0])], 60, 20, 1.0, 1.0).unwrap();
        let covered = |d: &[u8]| (0..60).filter(|x| at(d, 60, *x, 10)[3] > 200).count();
        assert_eq!(covered(&solid), 60);
        let n = covered(&dashed);
        assert!((30..=42).contains(&n), "dashed coverage {n}");
        assert_eq!(at(&dashed, 60, 8, 10)[3], 0, "gap after the first 6px dash");
    }

    #[test]
    fn dash_offset_slides_the_pattern_along_the_stroke() {
        let line = |offset: f32| DrawCommand::Path {
            segments: vec![
                PathSegment::MoveTo { x: 0.0, y: 10.0 },
                PathSegment::LineTo { x: 60.0, y: 10.0 },
            ],
            fill: None,
            stroke: Some(Stroke {
                color: RED,
                width: 4.0,
                dash: vec![6.0, 4.0],
                dash_offset: offset,
                cap: LineCap::Butt,
                join: LineJoin::Miter,
            }),
        };
        let rest = rasterize(&[&line(0.0)], 60, 20, 1.0, 1.0).unwrap();
        let slid = rasterize(&[&line(5.0)], 60, 20, 1.0, 1.0).unwrap();
        assert!(
            at(&rest, 60, 3, 10)[3] > 200,
            "x=3 is inside the first dash"
        );
        assert_eq!(
            at(&slid, 60, 3, 10)[3],
            0,
            "five units into the pattern x=3 is a gap"
        );
        let negative = rasterize(&[&line(-14.0)], 60, 20, 1.0, 1.0).unwrap();
        let wrapped = rasterize(&[&line(6.0)], 60, 20, 1.0, 1.0).unwrap();
        assert_eq!(
            negative, wrapped,
            "a negative phase wraps by the 10-unit pattern, as SVG does"
        );
    }

    #[test]
    fn full_arc_is_a_round_disc_and_a_half_arc_is_one_side() {
        let disc = DrawCommand::Path {
            segments: vec![PathSegment::Arc {
                cx: 20.0,
                cy: 20.0,
                rx: 10.0,
                ry: 10.0,
                start: 0.0,
                end: std::f32::consts::TAU,
            }],
            fill: Some(RED),
            stroke: None,
        };
        let data = rasterize(&[&disc], 40, 40, 1.0, 1.0).unwrap();
        assert_eq!(at(&data, 40, 20, 20)[3], 255);
        assert_eq!(at(&data, 40, 29, 20)[3], 255);
        assert_eq!(at(&data, 40, 29, 29)[3], 0, "corner outside the circle");

        // 0..PI sweeps clockwise on screen = the lower half.
        let half = DrawCommand::Path {
            segments: vec![PathSegment::Arc {
                cx: 20.0,
                cy: 20.0,
                rx: 10.0,
                ry: 10.0,
                start: 0.0,
                end: std::f32::consts::PI,
            }],
            fill: Some(RED),
            stroke: None,
        };
        let data = rasterize(&[&half], 40, 40, 1.0, 1.0).unwrap();
        assert_eq!(at(&data, 40, 20, 26)[3], 255);
        assert_eq!(at(&data, 40, 20, 14)[3], 0);
    }

    fn ink(data: &[u8], w: u32, h: u32) -> Option<(u32, u32, u32, u32)> {
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        let mut any = false;
        for y in 0..h {
            for x in 0..w {
                if at(data, w, x, y)[3] > 0 {
                    any = true;
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        any.then_some((x0, y0, x1, y1))
    }

    fn text(s: &str, x: f32, align: TextAlign) -> DrawCommand {
        DrawCommand::Text {
            text: s.into(),
            pos: Vec2 { x, y: 20.0 },
            size: 14.0,
            color: RED,
            align,
            weight: 400,
            mono: false,
            halo: None,
        }
    }

    #[test]
    fn text_draws_glyph_ink_and_a_longer_string_is_wider() {
        let short = rasterize(&[&text("x", 5.0, TextAlign::Left)], 120, 40, 1.0, 1.0).unwrap();
        let long = rasterize(&[&text("xxxxxx", 5.0, TextAlign::Left)], 120, 40, 1.0, 1.0).unwrap();
        let (sx0, _, sx1, _) = ink(&short, 120, 40).expect("ink for x");
        let (lx0, _, lx1, _) = ink(&long, 120, 40).expect("ink for xxxxxx");
        assert!(
            lx1 - lx0 > (sx1 - sx0) * 4,
            "six glyphs span far more than one"
        );
        assert!(lx0 >= 5);
    }

    #[test]
    fn text_alignment_moves_ink_across_the_anchor() {
        let l = rasterize(&[&text("abc", 60.0, TextAlign::Left)], 120, 40, 1.0, 1.0).unwrap();
        let c = rasterize(&[&text("abc", 60.0, TextAlign::Center)], 120, 40, 1.0, 1.0).unwrap();
        let r = rasterize(&[&text("abc", 60.0, TextAlign::Right)], 120, 40, 1.0, 1.0).unwrap();
        let (l0, _, _, _) = ink(&l, 120, 40).unwrap();
        let (c0, _, c1, _) = ink(&c, 120, 40).unwrap();
        let (_, _, r1, _) = ink(&r, 120, 40).unwrap();
        assert!(l0 >= 59, "left text starts at the anchor, got {l0}");
        assert!(c0 < 60 && c1 > 60, "centered text straddles the anchor");
        assert!(r1 <= 61, "right text ends at the anchor, got {r1}");
    }

    #[test]
    fn text_is_vertically_centered_on_the_anchor() {
        let d = rasterize(&[&text("x", 5.0, TextAlign::Left)], 60, 40, 1.0, 1.0).unwrap();
        let (_, y0, _, y1) = ink(&d, 60, 40).unwrap();
        let mid = (y0 + y1) as f32 / 2.0;
        assert!(
            (mid - 20.0).abs() < 4.0,
            "x-height centered near y=20, got {mid}"
        );
    }

    #[test]
    fn halo_adds_ink_around_the_glyph() {
        let plain = text("o", 10.0, TextAlign::Left);
        let haloed = DrawCommand::Text {
            text: "o".into(),
            pos: Vec2 { x: 10.0, y: 20.0 },
            size: 14.0,
            color: RED,
            align: TextAlign::Left,
            weight: 400,
            mono: false,
            halo: Some(crate::packet::Halo {
                color: [1.0, 1.0, 1.0, 1.0],
                width: 4.0,
            }),
        };
        let a = rasterize(&[&plain], 60, 40, 1.0, 1.0).unwrap();
        let b = rasterize(&[&haloed], 60, 40, 1.0, 1.0).unwrap();
        let count = |d: &[u8]| d.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert!(count(&b) > count(&a));
    }

    fn circle(cx: f32, cy: f32, r: f32) -> Vec<PathSegment> {
        vec![PathSegment::Arc {
            cx,
            cy,
            rx: r,
            ry: r,
            start: 0.0,
            end: std::f32::consts::TAU,
        }]
    }

    #[test]
    fn a_clipped_path_paints_only_inside_every_clip_and_outside_every_cut() {
        // Two overlapping discs; the lens is their intersection, minus a small cut disc.
        let cmd = DrawCommand::ClippedPath {
            segments: rect(0.0, 0.0, 60.0, 40.0),
            fill: Some(RED),
            stroke: None,
            clip: vec![circle(20.0, 20.0, 15.0), circle(40.0, 20.0, 15.0)],
            cut: vec![circle(30.0, 12.0, 3.0)],
        };
        let data = rasterize(&[&cmd], 60, 40, 1.0, 1.0).unwrap();
        assert_eq!(at(&data, 60, 30, 26), [255, 0, 0, 255], "inside the lens");
        assert_eq!(at(&data, 60, 10, 20)[3], 0, "only in the left disc");
        assert_eq!(at(&data, 60, 50, 20)[3], 0, "only in the right disc");
        assert_eq!(at(&data, 60, 30, 12)[3], 0, "punched out by the cut");
    }

    #[test]
    fn control_a_clipped_path_with_no_regions_paints_like_a_path() {
        let clipped = DrawCommand::ClippedPath {
            segments: rect(10.0, 10.0, 20.0, 20.0),
            fill: Some(RED),
            stroke: None,
            clip: Vec::new(),
            cut: Vec::new(),
        };
        let plain = DrawCommand::Path {
            segments: rect(10.0, 10.0, 20.0, 20.0),
            fill: Some(RED),
            stroke: None,
        };
        assert_eq!(
            rasterize(&[&clipped], 40, 40, 1.0, 1.0).unwrap(),
            rasterize(&[&plain], 40, 40, 1.0, 1.0).unwrap()
        );
    }

    #[test]
    fn the_mono_face_gives_every_char_one_advance_and_the_body_face_does_not() {
        let (i, w) = (mono_text_width("i", 12.0), mono_text_width("W", 12.0));
        assert!((i - w).abs() < 0.01 && (i - 7.22).abs() < 0.1, "{i} {w}");
        assert!(
            text_width("i", 12.0) < text_width("W", 12.0),
            "control: the body face is proportional"
        );
        assert!(shape("ab", 12.0, false, true)
            .glyphs
            .iter()
            .all(|g| g.font == FontId::Mono));
    }

    #[test]
    fn mono_text_falls_to_the_script_face_where_mono_has_no_glyph() {
        let s = shape("a\u{645}", 12.0, false, true);
        assert_eq!(s.glyphs.first().map(|g| g.font), Some(FontId::Mono));
        assert!(s.glyphs.iter().any(|g| g.font == FontId::ArabicRegular));
    }

    /// The missing-script list is process-wide; tests that drain it take turns.
    static MISSING_LIST: Mutex<()> = Mutex::new(());

    #[test]
    fn an_uncovered_script_is_reported_missing_once() {
        let _turn = MISSING_LIST.lock().unwrap_or_else(|e| e.into_inner());
        let s = shape("\u{05e9}\u{05dc}\u{05d5}\u{05dd}", 16.0, false, false);
        assert!(!s.glyphs.is_empty(), "still lays out (as .notdef)");
        let first = take_missing_scripts();
        assert!(
            first.contains(&MissingScript {
                script: "Hebr".into(),
                sample: "\u{05e9}".into()
            }),
            "{first:?}"
        );
        shape("\u{05e9}\u{05dc}\u{05d5}\u{05dd}", 16.0, false, false);
        assert!(
            !take_missing_scripts().iter().any(|m| m.script == "Hebr"),
            "reported once"
        );
    }

    #[test]
    fn control_a_covered_script_is_never_reported() {
        let _turn = MISSING_LIST.lock().unwrap_or_else(|e| e.into_inner());
        shape("Hello \u{03b8}", 16.0, false, false);
        let missing = take_missing_scripts();
        assert!(
            !missing
                .iter()
                .any(|m| m.script == "Latn" || m.script == "Grek"),
            "{missing:?}"
        );
    }

    #[test]
    fn a_host_face_routes_its_script_and_bumps_the_font_generation() {
        let _font = FONT_GENERATION_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let before = font_generation();
        add_font_face("Deva", ARABIC_REGULAR_BYTES.to_vec(), 0).unwrap();
        assert!(font_generation() > before);
        let s = shape("\u{0928}\u{092e}", 16.0, false, false);
        assert!(
            s.glyphs.iter().all(|g| matches!(g.font, FontId::Host(_))),
            "{:?}",
            fonts_of(&s)
        );
        let latin = shape("ab", 16.0, false, false);
        assert!(
            latin.glyphs.iter().all(|g| g.font == FontId::Latin),
            "control: Latin keeps the bundled face"
        );
    }

    #[test]
    fn a_face_without_the_reported_sample_glyph_is_refused() {
        let _font = FONT_GENERATION_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        shape("\u{0e01}", 16.0, false, false);
        let err = add_font_face("Thai", LATIN_BYTES.to_vec(), 0).unwrap_err();
        assert!(err.contains("no glyph"), "{err}");
        assert!(
            shape("\u{0e01}", 16.0, false, false)
                .glyphs
                .iter()
                .all(|g| g.font == FontId::Latin),
            "the refused face is not registered"
        );
    }

    #[test]
    fn add_font_face_rejects_an_unknown_script_and_bytes_that_are_not_a_font() {
        let _font = FONT_GENERATION_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        assert!(add_font_face("Zzzq", ARABIC_REGULAR_BYTES.to_vec(), 0).is_err());
        assert!(add_font_face("Khmr", vec![0, 1, 2, 3], 0).is_err());
    }

    #[test]
    fn semi_transparent_fill_is_straight_alpha() {
        let cmd = DrawCommand::Path {
            segments: rect(0.0, 0.0, 10.0, 10.0),
            fill: Some([1.0, 0.0, 0.0, 0.5]),
            stroke: None,
        };
        let data = rasterize(&[&cmd], 10, 10, 1.0, 1.0).unwrap();
        let p = at(&data, 10, 5, 5);
        assert!(p[0] >= 250 && (p[3] as i32 - 128).abs() <= 2, "{p:?}");
    }

    fn nominal_gids(font: FontId, s: &str) -> Vec<u16> {
        let f = font_for(font).unwrap();
        s.chars()
            .map(|c| f.glyph_index(c).map_or(0, |g| g.0))
            .collect()
    }

    fn fonts_of(s: &Shaped) -> Vec<FontId> {
        s.glyphs.iter().map(|g| g.font).collect()
    }

    const KITAB: &str = "\u{643}\u{62A}\u{627}\u{628}";
    const MARHABA: &str = "\u{645}\u{631}\u{62D}\u{628}\u{627}";

    #[test]
    fn arabic_glyph_count_differs_from_char_count_and_latin_does_not() {
        for word in [KITAB, MARHABA] {
            let ar = shape(word, 14.0, false, false);
            assert_ne!(ar.glyphs.len(), word.chars().count(), "{word}");
        }
        let en = shape("salm", 14.0, false, false);
        assert_eq!(en.glyphs.len(), "salm".chars().count());
    }

    #[test]
    fn arabic_uses_cursive_forms_not_the_nominal_glyphs() {
        let s = shape(MARHABA, 14.0, false, false);
        assert!(s.glyphs.iter().all(|g| g.font == FontId::ArabicRegular));
        let nominal = nominal_gids(FontId::ArabicRegular, MARHABA);
        assert!(nominal.iter().all(|g| *g != 0), "subset covers the letters");
        let first_logical = s.glyphs.last().map(|g| g.gid);
        assert_ne!(
            first_logical,
            Some(nominal[0]),
            "initial meem is not the isolated meem"
        );
        let lone = shape("\u{645}", 14.0, false, false);
        assert_eq!(
            lone.glyphs.last().map(|g| g.gid),
            Some(nominal[0]),
            "control: isolated meem is nominal"
        );
    }

    #[test]
    fn rtl_run_is_reversed_and_an_ltr_run_is_not() {
        let alef_dal = "\u{627}\u{62F}";
        let nominal = nominal_gids(FontId::ArabicRegular, alef_dal);
        let rtl = shape(alef_dal, 14.0, false, false);
        let got: Vec<u16> = rtl.glyphs.iter().map(|g| g.gid).collect();
        assert_eq!(got, vec![nominal[1], nominal[0]], "dal draws left of alef");
        assert!(rtl.glyphs[0].x < rtl.glyphs[1].x);

        let ltr = shape("ab", 14.0, false, false);
        let got: Vec<u16> = ltr.glyphs.iter().map(|g| g.gid).collect();
        assert_eq!(got, nominal_gids(FontId::Latin, "ab"));
    }

    #[test]
    fn mixed_latin_and_arabic_pick_their_own_fonts_in_bidi_order() {
        let ltr_base = format!("abc {MARHABA} def");
        let s = shape(&ltr_base, 14.0, false, false);
        let fonts = fonts_of(&s);
        assert_eq!(fonts.first(), Some(&FontId::Latin));
        assert_eq!(fonts.last(), Some(&FontId::Latin));
        let arabic = fonts
            .iter()
            .filter(|f| **f == FontId::ArabicRegular)
            .count();
        assert!(arabic >= 5, "every letter shaped with the Arabic face");
        let parts = text_width(MARHABA, 14.0) + text_width("abc", 14.0) + text_width("def", 14.0);
        assert!(s.width > parts, "spaces add to the three runs");
        assert!(s.glyphs.last().unwrap().x > s.glyphs[0].x);

        let rtl_base = format!("{MARHABA} abc");
        let r = shape(&rtl_base, 14.0, false, false);
        let fonts = fonts_of(&r);
        assert_eq!(
            fonts.first(),
            Some(&FontId::Latin),
            "Latin run sits left of an RTL paragraph"
        );
        assert_eq!(fonts.last(), Some(&FontId::ArabicRegular));
    }

    #[test]
    fn latin_label_is_unchanged_by_shaping() {
        let label = "Hello, World 123";
        let s = shape(label, 14.0, false, false);
        assert_eq!(
            s.glyphs.iter().map(|g| g.gid).collect::<Vec<_>>(),
            nominal_gids(FontId::Latin, label)
        );
        let f = font_for(FontId::Latin).unwrap();
        let scale = 14.0 / f.units_per_em() as f32;
        let expect: f32 = label
            .chars()
            .map(|c| f.glyph_hor_advance(f.glyph_index(c).unwrap()).unwrap() as f32 * scale)
            .sum();
        assert!((s.width - expect).abs() < 0.01);
        assert!((text_width(label, 14.0) - expect).abs() < 0.01);
        assert!(s
            .glyphs
            .iter()
            .all(|g| g.font == FontId::Latin && g.y == 0.0));
    }

    #[test]
    fn bold_arabic_uses_the_real_bold_face_and_bold_latin_stays_faux() {
        let b = shape(MARHABA, 14.0, true, false);
        assert!(b.glyphs.iter().all(|g| g.font == FontId::ArabicBold));
        let l = shape("abc", 14.0, true, false);
        assert!(l.glyphs.iter().all(|g| g.font == FontId::Latin));
    }

    #[test]
    fn arabic_text_paints_ink_and_right_align_ends_at_the_anchor() {
        let l = rasterize(&[&text(MARHABA, 60.0, TextAlign::Left)], 140, 40, 1.0, 1.0).unwrap();
        let c = rasterize(
            &[&text(MARHABA, 60.0, TextAlign::Center)],
            140,
            40,
            1.0,
            1.0,
        )
        .unwrap();
        let r = rasterize(&[&text(MARHABA, 60.0, TextAlign::Right)], 140, 40, 1.0, 1.0).unwrap();
        let (l0, _, l1, _) = ink(&l, 140, 40).expect("arabic ink");
        let (c0, _, c1, _) = ink(&c, 140, 40).unwrap();
        let (_, _, r1, _) = ink(&r, 140, 40).unwrap();
        assert!(l0 >= 58 && l1 > l0 + 10);
        assert!(c0 < 60 && c1 > 60);
        assert!(
            r1 <= 62,
            "right-aligned Arabic ends at the anchor, got {r1}"
        );
    }

    #[test]
    fn missing_glyph_in_every_face_still_measures_and_draws_nothing_wild() {
        let s = shape("", 14.0, false, false);
        assert!(s.glyphs.is_empty() && s.width == 0.0);
        let cjk = shape("\u{4E2D}", 14.0, false, false);
        assert_eq!(cjk.glyphs.len(), 1);
    }
}
