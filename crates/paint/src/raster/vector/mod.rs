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

use orbital_canvas_base::packet::{
    ColorRun, DrawCommand, FillPattern, Gradient, GradientStop, LineCap, LineJoin, PathSegment,
    Stroke, TextAlign, Vec2,
};
use rustybuzz::{Direction, UnicodeBuffer};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};
use tiny_skia::{
    FillRule, GradientStop as SkGradientStop, LineCap as SkCap, LineJoin as SkJoin, LinearGradient,
    Mask, Paint, PathBuilder, Pattern, Pixmap, RadialGradient, SpreadMode, StrokeDash, Transform,
};
use unicode_bidi::BidiInfo;
use unicode_script::{Script, UnicodeScript};

pub use orbital_canvas_base::packet::TextFamily as Family;

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
        DrawCommand::GradientPath {
            segments,
            fill,
            stroke,
            fill_gradient,
            stroke_gradient,
        } => {
            let Some(path) = build_path(segments) else {
                return;
            };
            if let Some(gradient) = fill_gradient {
                paint_gradient_path(pixmap, &path, gradient, ts, None);
            } else if let Some(color) = fill {
                pixmap.fill_path(&path, &paint(*color), FillRule::Winding, ts, None);
            }
            if let Some(stroke) = stroke {
                let style = sk_stroke(stroke);
                if let Some(gradient) = stroke_gradient {
                    paint_gradient_path(pixmap, &path, gradient, ts, Some(&style));
                } else {
                    pixmap.stroke_path(&path, &paint(stroke.color), &style, ts, None);
                }
            }
        }
        DrawCommand::PatternPath {
            segments,
            pattern,
            opacity,
        } => {
            let Some(path) = build_path(segments) else {
                return;
            };
            let Some(tile) = pattern_tile(pattern) else {
                return;
            };
            let scale = match pattern {
                FillPattern::Noise { scale, .. } | FillPattern::Image { scale, .. } => *scale,
            };
            if !scale.is_finite() || scale <= 0.0 {
                return;
            }
            let paint = Paint {
                anti_alias: true,
                shader: Pattern::new(
                    tile.as_ref(),
                    SpreadMode::Repeat,
                    tiny_skia::FilterQuality::Nearest,
                    opacity.clamp(0.0, 1.0),
                    Transform::from_scale(scale, scale),
                ),
                ..Paint::default()
            };
            pixmap.fill_path(&path, &paint, FillRule::Winding, ts, None);
        }
        DrawCommand::Blurred { command, radius } => {
            let (width, height) = (pixmap.width(), pixmap.height());
            let Some(mut layer) = Pixmap::new(width, height) else {
                return;
            };
            draw_command(&mut layer, command, ts);
            let Some(image) = image::RgbaImage::from_raw(width, height, layer.take()) else {
                return;
            };
            let sigma = radius.max(0.0) * (ts.sx.abs() + ts.sy.abs()) / 2.0;
            let blurred = image::imageops::blur(&image, sigma);
            let Some(size) = tiny_skia::IntSize::from_wh(width, height) else {
                return;
            };
            let Some(result) = Pixmap::from_vec(blurred.into_raw(), size) else {
                return;
            };
            pixmap.draw_pixmap(
                0,
                0,
                result.as_ref(),
                &tiny_skia::PixmapPaint::default(),
                Transform::identity(),
                None,
            );
        }
        DrawCommand::Shadow {
            commands,
            color,
            sigma,
        } => {
            let (width, height) = (pixmap.width(), pixmap.height());
            let Some(mut layer) = Pixmap::new(width, height) else {
                return;
            };
            for command in commands {
                draw_command(&mut layer, command, ts);
            }
            let tint = color.map(|c| c.clamp(0.0, 1.0));
            let mut coverage = layer.take();
            for px in coverage.chunks_exact_mut(4) {
                let a = f32::from(px[3]) / 255.0 * tint[3];
                for (channel, c) in px.iter_mut().zip(tint) {
                    *channel = (c * a * 255.0).round() as u8;
                }
                px[3] = (a * 255.0).round() as u8;
            }
            let Some(image) = image::RgbaImage::from_raw(width, height, coverage) else {
                return;
            };
            let sigma = sigma.max(0.0) * (ts.sx.abs() + ts.sy.abs()) / 2.0;
            let blurred = image::imageops::blur(&image, sigma);
            let Some(size) = tiny_skia::IntSize::from_wh(width, height) else {
                return;
            };
            let Some(result) = Pixmap::from_vec(blurred.into_raw(), size) else {
                return;
            };
            pixmap.draw_pixmap(
                0,
                0,
                result.as_ref(),
                &tiny_skia::PixmapPaint::default(),
                Transform::identity(),
                None,
            );
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
            family,
            color_runs,
        } => {
            let bold = *weight >= 600;
            let Some(paths) = text_path(text, *pos, *size, *align, bold, *family) else {
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
            if color_runs.is_empty() {
                fill_text(pixmap, &paths, *c, *size, ts);
            } else if let Some(groups) =
                text_paths_by_color(text, *pos, *size, *align, bold, *family, *c, color_runs)
            {
                for (color, group) in &groups {
                    fill_text(pixmap, group, *color, *size, ts);
                }
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

/// Rasterize one authored clip path to an RGBA coverage texture for GPU masking.
pub fn rasterize_clip_mask(
    segments: &[PathSegment],
    width: u32,
    height: u32,
    scale_x: f32,
    scale_y: f32,
) -> Option<Vec<u8>> {
    let mask = region_mask(
        width,
        height,
        &[segments.to_vec()],
        &[],
        Transform::from_scale(scale_x, scale_y),
    )?;
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for alpha in mask.data() {
        rgba.extend_from_slice(&[255, 255, 255, *alpha]);
    }
    Some(rgba)
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
    rasterize_box(commands, px_w, px_h, sx, sy, 0.0, 0.0)
}

/// [`rasterize`] of the part of packet space whose top-left corner is (`ox`, `oy`): the surface's pixel (0, 0)
/// is that corner, so a run is drawn into its own box instead of the whole canvas.
pub fn rasterize_box(
    commands: &[&DrawCommand],
    px_w: u32,
    px_h: u32,
    sx: f32,
    sy: f32,
    ox: f32,
    oy: f32,
) -> Option<Vec<u8>> {
    let mut pixmap = Pixmap::new(px_w.max(1), px_h.max(1))?;
    let ts = Transform::from_scale(sx, sy).pre_translate(-ox, -oy);
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

mod bounds;
mod fonts;
mod glyphs;
mod shaping;
mod skia_paint;
pub use self::bounds::*;
pub use self::fonts::*;
pub use self::glyphs::*;
pub use self::shaping::*;
use self::skia_paint::*;

#[cfg(test)]
mod tests;
