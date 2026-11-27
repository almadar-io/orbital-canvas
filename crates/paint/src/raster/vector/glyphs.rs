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
    pub(super) all: Option<tiny_skia::Path>,
    pub(super) faux_bold: Option<tiny_skia::Path>,
    /// Glyphs painted from colour layers (COLR) rather than filled in the text colour.
    pub(super) color: Vec<ColorGlyph>,
}

/// A colour glyph placed in a text: its face, glyph id, origin (pen and baseline) and scale from font units.
#[derive(Debug, Clone, Copy)]
pub(super) struct ColorGlyph {
    pub(super) font: FontId,
    pub(super) gid: u16,
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) scale: f32,
}

/// Whether glyph `gid` of `font` is painted from colour layers, so it cannot be drawn as one-colour coverage.
pub fn is_color_glyph(font: FontId, gid: u16) -> bool {
    font_for(font).is_some_and(|f| f.is_color_glyph(ttf_parser::GlyphId(gid)))
}

impl TextPaths {
    /// The box the text paints into: its outlines and its colour glyphs' layers.
    pub(super) fn bounds(&self) -> Option<tiny_skia::Rect> {
        let mut boxes: Vec<tiny_skia::Rect> = self.all.iter().map(|p| p.bounds()).collect();
        boxes.extend(self.color.iter().filter_map(color_glyph_bounds));
        let first = *boxes.first()?;
        boxes.iter().skip(1).try_fold(first, |acc, b| {
            tiny_skia::Rect::from_ltrb(
                acc.left().min(b.left()),
                acc.top().min(b.top()),
                acc.right().max(b.right()),
                acc.bottom().max(b.bottom()),
            )
        })
    }
}

/// Paints a COLR glyph's layers: a solid paint fills the layer outline just read (COLRv0), else the innermost clip
/// (COLRv1). Gradient paints paint nothing; the bundled emoji face is COLRv0, solid layers only.
struct LayerPainter<'p> {
    pixmap: &'p mut Pixmap,
    ts: Vec<Transform>,
    outline: Option<tiny_skia::Path>,
    clips: Vec<tiny_skia::Path>,
    alpha: f32,
    bounds: Option<tiny_skia::Rect>,
}

impl LayerPainter<'_> {
    fn current(&self) -> Transform {
        self.ts.last().copied().unwrap_or_default()
    }
}

impl<'a> ttf_parser::colr::Painter<'a> for LayerPainter<'_> {
    fn outline_glyph(&mut self, glyph_id: ttf_parser::GlyphId) {
        self.outline = None;
        let Some(face) = font_for(FontId::Emoji) else {
            return;
        };
        let mut builder = PathBuilder::new();
        if face
            .outline_glyph(
                glyph_id,
                &mut GlyphOutline::new(&mut builder, 0.0, 0.0, 1.0),
            )
            .is_some()
        {
            self.outline = builder.finish();
        }
    }
    fn paint(&mut self, paint_with: ttf_parser::colr::Paint<'a>) {
        let ttf_parser::colr::Paint::Solid(c) = paint_with else {
            return;
        };
        let Some(clip) = self.outline.as_ref().or(self.clips.last()) else {
            return;
        };
        let ts = self.current();
        if let Some(b) = clip.clone().transform(ts).map(|p| p.bounds()) {
            self.bounds = Some(match self.bounds {
                None => b,
                Some(acc) => tiny_skia::Rect::from_ltrb(
                    acc.left().min(b.left()),
                    acc.top().min(b.top()),
                    acc.right().max(b.right()),
                    acc.bottom().max(b.bottom()),
                )
                .unwrap_or(acc),
            });
        }
        let color = [
            c.red as f32 / 255.0,
            c.green as f32 / 255.0,
            c.blue as f32 / 255.0,
            c.alpha as f32 / 255.0 * self.alpha,
        ];
        let clip = clip.clone();
        self.pixmap
            .fill_path(&clip, &paint(color), FillRule::Winding, ts, None);
    }
    fn push_clip(&mut self) {
        if let Some(outline) = self.outline.take() {
            self.clips.push(outline);
        }
    }
    fn push_clip_box(&mut self, _clipbox: ttf_parser::colr::ClipBox) {}
    fn pop_clip(&mut self) {
        self.clips.pop();
    }
    fn push_layer(&mut self, _mode: ttf_parser::colr::CompositeMode) {}
    fn pop_layer(&mut self) {}
    fn push_transform(&mut self, t: ttf_parser::Transform) {
        // Outlines are read with y flipped (GlyphOutline), so a font-space transform is conjugated by that flip.
        let flipped = Transform::from_row(t.a, -t.b, -t.c, t.d, t.e, -t.f);
        let next = self.current().pre_concat(flipped);
        self.ts.push(next);
    }
    fn pop_transform(&mut self) {
        if self.ts.len() > 1 {
            self.ts.pop();
        }
    }
}

fn layer_painter<'p>(
    pixmap: &'p mut Pixmap,
    glyph: &ColorGlyph,
    ts: Transform,
    alpha: f32,
) -> LayerPainter<'p> {
    let place = ts
        .pre_translate(glyph.x, glyph.y)
        .pre_scale(glyph.scale, glyph.scale);
    LayerPainter {
        pixmap,
        ts: vec![place],
        outline: None,
        clips: Vec::new(),
        alpha,
        bounds: None,
    }
}

/// Paints one colour glyph's layers onto `pixmap` under `ts`: a layer in the foreground colour takes the text's
/// `color`, and every layer's alpha is scaled by the text's.
pub(super) fn paint_color_glyph(
    pixmap: &mut Pixmap,
    glyph: &ColorGlyph,
    ts: Transform,
    color: [f32; 4],
) {
    let Some(face) = font_for(glyph.font) else {
        return;
    };
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let foreground =
        ttf_parser::RgbaColor::new(byte(color[0]), byte(color[1]), byte(color[2]), 255);
    let id = ttf_parser::GlyphId(glyph.gid);
    if color[3] >= 1.0 {
        let mut painter = layer_painter(pixmap, glyph, ts, 1.0);
        face.paint_color_glyph(id, 0, foreground, &mut painter);
        return;
    }
    // Overlapping layers would compound a partial alpha: the glyph is painted whole, then composited at it.
    let Some(device) = color_glyph_bounds(glyph).and_then(|b| b.transform(ts)) else {
        return;
    };
    let (left, top) = (device.left().floor(), device.top().floor());
    let (w, h) = (
        (device.right().ceil() - left) as u32,
        (device.bottom().ceil() - top) as u32,
    );
    let Some(mut layer) = Pixmap::new(w.max(1), h.max(1)) else {
        return;
    };
    let mut painter = layer_painter(&mut layer, glyph, ts.post_translate(-left, -top), 1.0);
    face.paint_color_glyph(id, 0, foreground, &mut painter);
    let opacity = tiny_skia::PixmapPaint {
        opacity: color[3].clamp(0.0, 1.0),
        ..Default::default()
    };
    pixmap.draw_pixmap(
        left as i32,
        top as i32,
        layer.as_ref(),
        &opacity,
        Transform::identity(),
        None,
    );
}

fn color_glyph_bounds(glyph: &ColorGlyph) -> Option<tiny_skia::Rect> {
    let face = font_for(glyph.font)?;
    let mut scratch = Pixmap::new(1, 1)?;
    let mut painter = layer_painter(&mut scratch, glyph, Transform::identity(), 1.0);
    let black = ttf_parser::RgbaColor::new(0, 0, 0, 255);
    face.paint_color_glyph(ttf_parser::GlyphId(glyph.gid), 0, black, &mut painter)?;
    painter.bounds
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
    let mut color = Vec::new();
    for g in &shaped.glyphs {
        let Some(face) = font_for(g.font) else {
            continue;
        };
        let scale = size / face.units_per_em() as f32;
        let id = ttf_parser::GlyphId(g.gid);
        if face.is_color_glyph(id) {
            color.push(ColorGlyph {
                font: g.font,
                gid: g.gid,
                x: x0 + g.x,
                y: baseline + g.y,
                scale,
            });
            continue;
        }
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
    let all = all.finish();
    if all.is_none() && color.is_empty() {
        return None;
    }
    Some(TextPaths {
        all,
        faux_bold: faux.finish(),
        color,
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
    let mut groups: Vec<([f32; 4], PathBuilder, PathBuilder, Vec<ColorGlyph>)> = Vec::new();
    for (g, color) in shaped.glyphs.iter().zip(colors) {
        let Some(face) = font_for(g.font) else {
            continue;
        };
        let at = match groups.iter().position(|(c, _, _, _)| *c == color) {
            Some(at) => at,
            None => {
                groups.push((color, PathBuilder::new(), PathBuilder::new(), Vec::new()));
                groups.len() - 1
            }
        };
        let scale = size / face.units_per_em() as f32;
        let id = ttf_parser::GlyphId(g.gid);
        if face.is_color_glyph(id) {
            groups[at].3.push(ColorGlyph {
                font: g.font,
                gid: g.gid,
                x: x0 + g.x,
                y: baseline + g.y,
                scale,
            });
            continue;
        }
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
            .filter_map(|(color, all, faux, glyphs)| {
                let all = all.finish();
                if all.is_none() && glyphs.is_empty() {
                    return None;
                }
                Some((
                    color,
                    TextPaths {
                        all,
                        faux_bold: faux.finish(),
                        color: glyphs,
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
    if let Some(all) = &paths.all {
        pixmap.fill_path(all, &paint(color), FillRule::Winding, ts, None);
    }
    for glyph in &paths.color {
        paint_color_glyph(pixmap, glyph, ts, color);
    }
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
