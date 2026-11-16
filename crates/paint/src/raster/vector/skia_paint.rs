//! Packet paints, gradients, fill patterns, strokes and paths as tiny-skia draws them.

use super::*;

pub(super) fn color(c: [f32; 4]) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba(
        c[0].clamp(0.0, 1.0),
        c[1].clamp(0.0, 1.0),
        c[2].clamp(0.0, 1.0),
        c[3].clamp(0.0, 1.0),
    )
    .unwrap_or(tiny_skia::Color::TRANSPARENT)
}

pub(super) fn paint(c: [f32; 4]) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(color(c));
    p.anti_alias = true;
    p
}

pub(super) fn sk_gradient_stops(stops: &[GradientStop]) -> Vec<SkGradientStop> {
    stops
        .iter()
        .map(|stop| SkGradientStop::new(stop.offset.clamp(0.0, 1.0), color(stop.color)))
        .collect()
}

pub(super) fn conic_color(stops: &[GradientStop], progress: f32) -> [f32; 4] {
    let Some(first) = stops.first() else {
        return [0.0; 4];
    };
    let mut previous = first;
    for next in stops.iter().skip(1) {
        if progress <= next.offset {
            let span = (next.offset - previous.offset).max(f32::EPSILON);
            let t = ((progress - previous.offset) / span).clamp(0.0, 1.0);
            return std::array::from_fn(|i| {
                previous.color[i] + (next.color[i] - previous.color[i]) * t
            });
        }
        previous = next;
    }
    previous.color
}

pub(super) fn conic_pixmap(
    width: u32,
    height: u32,
    center: Vec2,
    angle: f32,
    stops: &[GradientStop],
    ts: Transform,
) -> Option<Pixmap> {
    let mut pixmap = Pixmap::new(width, height)?;
    let mut ordered = stops.to_vec();
    ordered.sort_by(|a, b| a.offset.total_cmp(&b.offset));
    let center = Vec2 {
        x: center.x * ts.sx + ts.tx,
        y: center.y * ts.sy + ts.ty,
    };
    for (index, pixel) in pixmap.pixels_mut().iter_mut().enumerate() {
        let x = (index % width as usize) as f32 + 0.5;
        let y = (index / width as usize) as f32 + 0.5;
        let turn = ((x - center.x).atan2(center.y - y) - angle).rem_euclid(std::f32::consts::TAU)
            / std::f32::consts::TAU;
        *pixel = color(conic_color(&ordered, turn))
            .premultiply()
            .to_color_u8();
    }
    Some(pixmap)
}

pub(super) fn paint_gradient_path(
    pixmap: &mut Pixmap,
    path: &tiny_skia::Path,
    gradient: &Gradient,
    ts: Transform,
    stroke: Option<&tiny_skia::Stroke>,
) {
    let mut p = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    match gradient {
        Gradient::Linear { from, to, stops } => {
            let Some(shader) = LinearGradient::new(
                tiny_skia::Point::from_xy(from.x, from.y),
                tiny_skia::Point::from_xy(to.x, to.y),
                sk_gradient_stops(stops),
                SpreadMode::Pad,
                Transform::identity(),
            ) else {
                return;
            };
            p.shader = shader;
        }
        Gradient::Radial {
            center,
            radius,
            stops,
        } => {
            let point = tiny_skia::Point::from_xy(center.x, center.y);
            let Some(shader) = RadialGradient::new(
                point,
                point,
                *radius,
                sk_gradient_stops(stops),
                SpreadMode::Pad,
                Transform::identity(),
            ) else {
                return;
            };
            p.shader = shader;
        }
        Gradient::Conic {
            center,
            angle,
            stops,
        } => {
            let Some(texture) =
                conic_pixmap(pixmap.width(), pixmap.height(), *center, *angle, stops, ts)
            else {
                return;
            };
            p.shader = Pattern::new(
                texture.as_ref(),
                SpreadMode::Pad,
                tiny_skia::FilterQuality::Nearest,
                1.0,
                Transform::identity(),
            );
            if let Some(stroke) = stroke {
                pixmap.stroke_path(path, &p, stroke, ts, None);
            } else {
                pixmap.fill_path(path, &p, FillRule::Winding, ts, None);
            }
            return;
        }
    }
    if let Some(stroke) = stroke {
        pixmap.stroke_path(path, &p, stroke, ts, None);
    } else {
        pixmap.fill_path(path, &p, FillRule::Winding, ts, None);
    }
}

pub(super) fn pattern_tile(pattern: &FillPattern) -> Option<Pixmap> {
    match pattern {
        FillPattern::Noise {
            alpha, color: tint, ..
        } => {
            let mut pixmap = Pixmap::new(32, 32)?;
            for y in 0..32 {
                for x in 0..32 {
                    let s = ((x as f64 * 12.9898 + y as f64 * 78.233).sin() * 43758.5453).fract();
                    let noise = if s < 0.0 { s + 1.0 } else { s };
                    let mut color = *tint;
                    color[3] *= (noise as f32 * alpha).clamp(0.0, 1.0);
                    pixmap.pixels_mut()[(y * 32 + x) as usize] = premultiplied_color(color);
                }
            }
            Some(pixmap)
        }
        FillPattern::Image { url, .. } => {
            let bytes = crate::asset::asset_bytes(url)?;
            let image = match crate::asset::asset_kind(url)? {
                crate::asset::AssetKind::Raster => {
                    image::load_from_memory(&bytes).ok()?.into_rgba8()
                }
                crate::asset::AssetKind::Svg => {
                    let (width, height) = crate::svg::intrinsic_size(url, &bytes)?;
                    let raster = crate::svg::rasterize(url, &bytes, width, height)?;
                    image::RgbaImage::from_raw(width, height, raster.rgba)?
                }
            };
            let (width, height) = image.dimensions();
            let mut pixmap = Pixmap::new(width, height)?;
            for (target, source) in pixmap.pixels_mut().iter_mut().zip(image.pixels()) {
                *target = tiny_skia::ColorU8::from_rgba(source[0], source[1], source[2], source[3])
                    .premultiply();
            }
            Some(pixmap)
        }
    }
}

pub(super) fn premultiplied_color(color: [f32; 4]) -> tiny_skia::PremultipliedColorU8 {
    tiny_skia::ColorU8::from_rgba(
        (color[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        (color[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (color[2].clamp(0.0, 1.0) * 255.0).round() as u8,
        (color[3].clamp(0.0, 1.0) * 255.0).round() as u8,
    )
    .premultiply()
}

pub(super) fn sk_stroke(stroke: &Stroke) -> tiny_skia::Stroke {
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
pub(super) fn arc_to(
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

pub(super) fn build_path(segments: &[PathSegment]) -> Option<tiny_skia::Path> {
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
