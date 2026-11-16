//! Glyph outlines as paths: a text's layout, its paths by colour run, and one glyph's coverage bitmap for the atlas.

use super::*;

/// A glyph's contours into a path builder, scaled and placed. A contour whose points all coincide encloses nothing, so
/// filling ignores it, but a stroke (the halo, faux bold) would draw it as a dot: it is dropped (DejaVu's "u" carries
/// one, which stroked as an umlaut).
pub(super) struct GlyphOutline<'a> {
    pub(super) out: &'a mut PathBuilder,
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) scale: f32,
    pub(super) contour: Vec<Segment>,
}

#[derive(Clone, Copy)]
pub(super) enum Segment {
    Move(f32, f32),
    Line(f32, f32),
    Quad(f32, f32, f32, f32),
    Cubic(f32, f32, f32, f32, f32, f32),
}

impl<'a> GlyphOutline<'a> {
    pub(super) fn new(out: &'a mut PathBuilder, x: f32, y: f32, scale: f32) -> Self {
        Self {
            out,
            x,
            y,
            scale,
            contour: Vec::new(),
        }
    }

    pub(super) fn p(&self, x: f32, y: f32) -> (f32, f32) {
        (self.x + x * self.scale, self.y - y * self.scale)
    }

    /// Emit the pending contour unless it is a single point; `close` closes it.
    pub(super) fn flush(&mut self, close: bool) {
        let contour = std::mem::take(&mut self.contour);
        let mut points = contour.iter().flat_map(|s| match *s {
            Segment::Move(x, y) | Segment::Line(x, y) => vec![(x, y)],
            Segment::Quad(x1, y1, x, y) => vec![(x1, y1), (x, y)],
            Segment::Cubic(x1, y1, x2, y2, x, y) => vec![(x1, y1), (x2, y2), (x, y)],
        });
        let Some(first) = points.next() else { return };
        if points.all(|p| p == first) {
            return;
        }
        for s in contour {
            match s {
                Segment::Move(x, y) => self.out.move_to(x, y),
                Segment::Line(x, y) => self.out.line_to(x, y),
                Segment::Quad(x1, y1, x, y) => self.out.quad_to(x1, y1, x, y),
                Segment::Cubic(x1, y1, x2, y2, x, y) => self.out.cubic_to(x1, y1, x2, y2, x, y),
            }
        }
        if close {
            self.out.close();
        }
    }
}

impl Drop for GlyphOutline<'_> {
    fn drop(&mut self) {
        self.flush(false);
    }
}

impl ttf_parser::OutlineBuilder for GlyphOutline<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.flush(false);
        let (x, y) = self.p(x, y);
        self.contour.push(Segment::Move(x, y));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.p(x, y);
        self.contour.push(Segment::Line(x, y));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (x1, y1) = self.p(x1, y1);
        let (x, y) = self.p(x, y);
        self.contour.push(Segment::Quad(x1, y1, x, y));
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (x1, y1) = self.p(x1, y1);
        let (x2, y2) = self.p(x2, y2);
        let (x, y) = self.p(x, y);
        self.contour.push(Segment::Cubic(x1, y1, x2, y2, x, y));
    }
    fn close(&mut self) {
        self.flush(true);
    }
}

pub(super) struct TextPaths {
    pub(super) all: tiny_skia::Path,
    pub(super) faux_bold: Option<tiny_skia::Path>,
}

/// Shaped `text` with its anchor placed per `align`: the left pen `x0` and the baseline, `pos.y` at the em box
/// middle of the Latin face. Glyph `x`/`y` are offsets from them.
pub struct TextLayout {
    pub shaped: Shaped,
    pub x0: f32,
    pub baseline: f32,
}

pub fn text_layout(
    text: &str,
    pos: Vec2,
    size: f32,
    align: TextAlign,
    bold: bool,
    family: Family,
) -> Option<TextLayout> {
    let base = face()?;
    let shaped = shape(text, size, bold, family);
    let x0 = match align {
        TextAlign::Left => pos.x,
        TextAlign::Center => pos.x - shaped.width / 2.0,
        TextAlign::Right => pos.x - shaped.width,
    };
    let base_scale = size / base.units_per_em() as f32;
    let baseline = pos.y + (base.ascender() as f32 + base.descender() as f32) / 2.0 * base_scale;
    Some(TextLayout {
        shaped,
        x0,
        baseline,
    })
}

/// Outline of `text` placed by [`text_layout`]. Bold glyphs from faces without a real bold are
/// returned apart so the caller can thicken them.
pub(super) fn text_path(
    text: &str,
    pos: Vec2,
    size: f32,
    align: TextAlign,
    bold: bool,
    family: Family,
) -> Option<TextPaths> {
    let TextLayout {
        shaped,
        x0,
        baseline,
    } = text_layout(text, pos, size, align, bold, family)?;
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
            &mut GlyphOutline::new(&mut all, x0 + g.x, baseline + g.y, scale),
        );
        if bold && g.font.faux_bold() {
            face.outline_glyph(
                id,
                &mut GlyphOutline::new(&mut faux, x0 + g.x, baseline + g.y, scale),
            );
        }
    }
    Some(TextPaths {
        all: all.finish()?,
        faux_bold: faux.finish(),
    })
}

/// The colour of each glyph of `shaped` (shaped from `text`): its cluster's first character's run, else `base`.
pub fn glyph_colors(
    text: &str,
    shaped: &Shaped,
    base: [f32; 4],
    runs: &[ColorRun],
) -> Vec<[f32; 4]> {
    let mut colors = vec![base; shaped.glyphs.len()];
    if runs.is_empty() {
        return colors;
    }
    let offsets: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    for cluster in &shaped.clusters {
        let index = offsets.partition_point(|o| *o < cluster.bytes.start);
        if let Some(run) = runs.iter().find(|r| r.from <= index && index < r.to) {
            for slot in &mut colors[cluster.glyphs.clone()] {
                *slot = run.color;
            }
        }
    }
    colors
}

/// [`text_path`] split by glyph colour: one entry per distinct colour, in first-use order.
#[allow(clippy::too_many_arguments)]
pub(super) fn text_paths_by_color(
    text: &str,
    pos: Vec2,
    size: f32,
    align: TextAlign,
    bold: bool,
    family: Family,
    base: [f32; 4],
    runs: &[ColorRun],
) -> Option<Vec<([f32; 4], TextPaths)>> {
    let TextLayout {
        shaped,
        x0,
        baseline,
    } = text_layout(text, pos, size, align, bold, family)?;
    let colors = glyph_colors(text, &shaped, base, runs);
    let mut groups: Vec<([f32; 4], PathBuilder, PathBuilder)> = Vec::new();
    for (g, color) in shaped.glyphs.iter().zip(colors) {
        let Some(face) = font_for(g.font) else {
            continue;
        };
        let at = match groups.iter().position(|(c, _, _)| *c == color) {
            Some(at) => at,
            None => {
                groups.push((color, PathBuilder::new(), PathBuilder::new()));
                groups.len() - 1
            }
        };
        let scale = size / face.units_per_em() as f32;
        let id = ttf_parser::GlyphId(g.gid);
        face.outline_glyph(
            id,
            &mut GlyphOutline::new(&mut groups[at].1, x0 + g.x, baseline + g.y, scale),
        );
        if bold && g.font.faux_bold() {
            face.outline_glyph(
                id,
                &mut GlyphOutline::new(&mut groups[at].2, x0 + g.x, baseline + g.y, scale),
            );
        }
    }
    Some(
        groups
            .into_iter()
            .filter_map(|(color, all, faux)| {
                Some((
                    color,
                    TextPaths {
                        all: all.finish()?,
                        faux_bold: faux.finish(),
                    },
                ))
            })
            .collect(),
    )
}

/// One glyph's anti-aliased coverage, `left`/`top` being the bitmap's offset from the glyph origin in pixels.
pub struct GlyphBitmap {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub coverage: Vec<u8>,
}

/// The coverage of glyph `gid` of `font` at `size` px with its origin `frac` px past a pixel corner, filled
/// (and, for a bold run in a face without a real bold, stroked) exactly as [`draw_command`] draws text.
/// `None` for a glyph with no outline.
pub fn rasterize_glyph(
    font: FontId,
    gid: u16,
    size: f32,
    faux_bold: bool,
    frac: (f32, f32),
) -> Option<GlyphBitmap> {
    let face = font_for(font)?;
    let scale = size / face.units_per_em() as f32;
    let id = ttf_parser::GlyphId(gid);
    let mut builder = PathBuilder::new();
    face.outline_glyph(
        id,
        &mut GlyphOutline::new(&mut builder, frac.0, frac.1, scale),
    )?;
    let path = builder.finish()?;
    let stroke_width = size * 0.045;
    let pad = 1.0 + if faux_bold { stroke_width / 2.0 } else { 0.0 };
    let bounds = path.bounds();
    let left = (bounds.left() - pad).floor();
    let top = (bounds.top() - pad).floor();
    let right = (bounds.right() + pad).ceil();
    let bottom = (bounds.bottom() + pad).ceil();
    let (width, height) = ((right - left) as u32, (bottom - top) as u32);
    let mut pixmap = Pixmap::new(width, height)?;
    let ts = Transform::from_translate(-left, -top);
    let white = paint([1.0; 4]);
    pixmap.fill_path(&path, &white, FillRule::Winding, ts, None);
    if faux_bold {
        let stroke = Stroke {
            color: [1.0; 4],
            width: stroke_width,
            dash: Vec::new(),
            dash_offset: 0.0,
            cap: LineCap::Round,
            join: LineJoin::Round,
        };
        pixmap.stroke_path(&path, &white, &sk_stroke(&stroke), ts, None);
    }
    Some(GlyphBitmap {
        left: left as i32,
        top: top as i32,
        width,
        height,
        coverage: pixmap.data().chunks_exact(4).map(|px| px[3]).collect(),
    })
}

pub(super) fn fill_text(
    pixmap: &mut Pixmap,
    paths: &TextPaths,
    color: [f32; 4],
    size: f32,
    ts: Transform,
) {
    pixmap.fill_path(&paths.all, &paint(color), FillRule::Winding, ts, None);
    if let Some(faux) = &paths.faux_bold {
        let stroke = Stroke {
            color,
            width: size * 0.045,
            dash: Vec::new(),
            dash_offset: 0.0,
            cap: LineCap::Round,
            join: LineJoin::Round,
        };
        pixmap.stroke_path(faux, &paint(color), &sk_stroke(&stroke), ts, None);
    }
}
