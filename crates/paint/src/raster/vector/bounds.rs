//! What a command or a run of commands covers, and which commands the vector layer draws.

use super::*;

/// Where a run of vector commands can paint, in packet units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RunBounds {
    /// Nothing in the run paints.
    Empty,
    /// Everything the run paints lies inside `x0..x1` by `y0..y1`.
    Box { x0: f32, y0: f32, x1: f32, y1: f32 },
    /// A command whose reach is not known: the run may paint anywhere.
    Anywhere,
}

impl RunBounds {
    pub(super) fn union(self, other: RunBounds) -> RunBounds {
        match (self, other) {
            (RunBounds::Anywhere, _) | (_, RunBounds::Anywhere) => RunBounds::Anywhere,
            (RunBounds::Empty, b) | (b, RunBounds::Empty) => b,
            (
                RunBounds::Box { x0, y0, x1, y1 },
                RunBounds::Box {
                    x0: a0,
                    y0: b0,
                    x1: a1,
                    y1: b1,
                },
            ) => RunBounds::Box {
                x0: x0.min(a0),
                y0: y0.min(b0),
                x1: x1.max(a1),
                y1: y1.max(b1),
            },
        }
    }

    pub(super) fn grow(self, by: f32) -> RunBounds {
        match self {
            RunBounds::Box { x0, y0, x1, y1 } => RunBounds::Box {
                x0: x0 - by,
                y0: y0 - by,
                x1: x1 + by,
                y1: y1 + by,
            },
            other => other,
        }
    }
}

pub(super) fn path_bounds(segments: &[PathSegment], stroke: Option<&Stroke>) -> RunBounds {
    let Some(path) = build_path(segments) else {
        return RunBounds::Empty;
    };
    let b = path.bounds();
    let reach = stroke.map(|s| s.width * 2.0).unwrap_or(0.0);
    RunBounds::Box {
        x0: b.left(),
        y0: b.top(),
        x1: b.right(),
        y1: b.bottom(),
    }
    .grow(reach)
}

/// The most text ink bounds the cache holds before it starts over.
const TEXT_BOUNDS_CACHE_ENTRIES: usize = 16_384;

type TextBoundsKey = (String, u32, u32, u32, u8, bool, Family);

/// Ink bounds of placed texts by their inputs, valid for one (font generation, asset generation).
#[derive(Default)]
struct TextBoundsCache {
    generations: (u64, u64),
    entries: std::collections::HashMap<TextBoundsKey, Option<[f32; 4]>>,
}

impl TextBoundsCache {
    fn get(&mut self, generations: (u64, u64), key: &TextBoundsKey) -> Option<Option<[f32; 4]>> {
        if self.generations != generations {
            self.entries.clear();
            self.generations = generations;
        }
        self.entries.get(key).copied()
    }

    fn put(&mut self, generations: (u64, u64), key: TextBoundsKey, bounds: Option<[f32; 4]>) {
        if self.generations != generations {
            return;
        }
        if self.entries.len() >= TEXT_BOUNDS_CACHE_ENTRIES {
            self.entries.clear();
        }
        self.entries.insert(key, bounds);
    }
}

fn text_bounds_cache() -> &'static Mutex<TextBoundsCache> {
    static CACHE: OnceLock<Mutex<TextBoundsCache>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

#[cfg(test)]
pub(crate) fn text_bounds_cache_len() -> usize {
    text_bounds_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entries
        .len()
}

/// `[left, top, right, bottom]` of the outline [`text_path`] places, kept by its inputs until a font or an asset arrives.
pub(super) fn text_ink_bounds(
    text: &str,
    pos: Vec2,
    size: f32,
    align: TextAlign,
    bold: bool,
    family: Family,
) -> Option<[f32; 4]> {
    let generations = (font_generation(), crate::asset::generation());
    let align_code = match align {
        TextAlign::Left => 0,
        TextAlign::Center => 1,
        TextAlign::Right => 2,
    };
    let key = (
        text.to_owned(),
        pos.x.to_bits(),
        pos.y.to_bits(),
        size.to_bits(),
        align_code,
        bold,
        family,
    );
    let cached = text_bounds_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(generations, &key);
    if let Some(bounds) = cached {
        return bounds;
    }
    let bounds = text_path(text, pos, size, align, bold, family)
        .and_then(|paths| paths.bounds())
        .map(|b| [b.left(), b.top(), b.right(), b.bottom()]);
    text_bounds_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .put(generations, key, bounds);
    bounds
}

/// The box a vector command paints into, from the same path and text outlines [`draw_command`] fills: a stroke
/// reaches twice its width past the outline (a mitre), a blur and a shadow three times their spread.
pub fn command_bounds(command: &DrawCommand) -> RunBounds {
    match command {
        DrawCommand::Path {
            segments, stroke, ..
        }
        | DrawCommand::GradientPath {
            segments, stroke, ..
        }
        | DrawCommand::ClippedPath {
            segments, stroke, ..
        } => path_bounds(segments, stroke.as_ref()),
        DrawCommand::PatternPath { segments, .. } => path_bounds(segments, None),
        DrawCommand::Line {
            from, to, width, ..
        } => RunBounds::Box {
            x0: from.x.min(to.x),
            y0: from.y.min(to.y),
            x1: from.x.max(to.x),
            y1: from.y.max(to.y),
        }
        .grow(*width * 2.0),
        DrawCommand::Text {
            text,
            pos,
            size,
            align,
            weight,
            halo,
            family,
            rotation,
            ..
        } => match text_ink_bounds(text, *pos, *size, *align, *weight >= 600, *family) {
            Some([left, top, right, bottom]) => {
                let reach = halo.as_ref().map(|h| h.width).unwrap_or(0.0) + size * 0.05 + 1.0;
                let (sin, cos) = rotation.sin_cos();
                let corners =
                    [(left, top), (right, top), (right, bottom), (left, bottom)].map(|(x, y)| {
                        let (dx, dy) = (x - pos.x, y - pos.y);
                        (pos.x + dx * cos - dy * sin, pos.y + dx * sin + dy * cos)
                    });
                let xs = corners.map(|c| c.0);
                let ys = corners.map(|c| c.1);
                RunBounds::Box {
                    x0: xs.into_iter().fold(f32::INFINITY, f32::min),
                    y0: ys.into_iter().fold(f32::INFINITY, f32::min),
                    x1: xs.into_iter().fold(f32::NEG_INFINITY, f32::max),
                    y1: ys.into_iter().fold(f32::NEG_INFINITY, f32::max),
                }
                .grow(reach)
            }
            None => RunBounds::Empty,
        },
        DrawCommand::Blurred { command, radius } => command_bounds(command).grow(radius * 3.0),
        DrawCommand::Shadow {
            commands, sigma, ..
        } => commands
            .iter()
            .fold(RunBounds::Empty, |acc, c| acc.union(command_bounds(c)))
            .grow(sigma * 3.0),
        DrawCommand::Quad { rect, .. } => rect_bounds(rect, 0.0),
        DrawCommand::Sprite { dst, rotation, .. } => rect_bounds(dst, *rotation),
        DrawCommand::Poly { points, .. } => points_bounds(points),
        DrawCommand::TexturedTriangle { points, .. } => points_bounds(points),
        DrawCommand::Sdf {
            rect,
            rotation,
            stroke,
            shadow,
            ..
        } => {
            let body = rect_bounds(rect, *rotation).grow(stroke.as_ref().map_or(0.0, |s| s.width));
            match shadow {
                Some(s) => body.union(
                    rect_bounds(
                        &Rect {
                            x: rect.x + s.offset.x,
                            y: rect.y + s.offset.y,
                            ..*rect
                        },
                        *rotation,
                    )
                    .grow(s.sigma * 3.0),
                ),
                None => body,
            }
        }
        DrawCommand::SdfBlend {
            first,
            second,
            radius,
        } => command_bounds(first)
            .union(command_bounds(second))
            .grow(*radius),
        DrawCommand::SdfBackdrop { shape, .. } => command_bounds(shape),
        DrawCommand::AdditiveGroup { commands } => commands
            .iter()
            .fold(RunBounds::Empty, |acc, c| acc.union(command_bounds(c))),
        DrawCommand::Faded {
            scale,
            origin,
            offset,
            commands,
            ..
        } => match commands
            .iter()
            .fold(RunBounds::Empty, |acc, c| acc.union(command_bounds(c)))
        {
            RunBounds::Box { x0, y0, x1, y1 } => {
                let at = |v: f32, o: f32, t: f32| o + (v - o) * scale + t;
                let (ax, bx) = (at(x0, origin.x, offset.x), at(x1, origin.x, offset.x));
                let (ay, by) = (at(y0, origin.y, offset.y), at(y1, origin.y, offset.y));
                RunBounds::Box {
                    x0: ax.min(bx),
                    y0: ay.min(by),
                    x1: ax.max(bx),
                    y1: ay.max(by),
                }
            }
            other => other,
        },
        DrawCommand::ClipGroup { clip, .. } => path_bounds(clip, None),
        DrawCommand::Clear { .. }
        | DrawCommand::Particles { .. }
        | DrawCommand::Scene3D { .. }
        | DrawCommand::Target { .. }
        | DrawCommand::TileGrid { .. }
        | DrawCommand::Scissor { .. }
        | DrawCommand::ResetScissor => RunBounds::Anywhere,
    }
}

/// A rect's box; turned about its centre, the circle through its corners holds it at any angle.
fn rect_bounds(rect: &Rect, rotation: f32) -> RunBounds {
    if rotation == 0.0 {
        return RunBounds::Box {
            x0: rect.x.min(rect.x + rect.w),
            y0: rect.y.min(rect.y + rect.h),
            x1: rect.x.max(rect.x + rect.w),
            y1: rect.y.max(rect.y + rect.h),
        };
    }
    let (cx, cy) = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
    let r = rect.w.hypot(rect.h) / 2.0;
    RunBounds::Box {
        x0: cx - r,
        y0: cy - r,
        x1: cx + r,
        y1: cy + r,
    }
}

fn points_bounds(points: &[Vec2]) -> RunBounds {
    points.iter().fold(RunBounds::Empty, |acc, p| {
        acc.union(RunBounds::Box {
            x0: p.x,
            y0: p.y,
            x1: p.x,
            y1: p.y,
        })
    })
}

/// The union of [`command_bounds`] over a run.
pub fn run_bounds(commands: &[&DrawCommand]) -> RunBounds {
    commands
        .iter()
        .fold(RunBounds::Empty, |acc, c| acc.union(command_bounds(c)))
}

/// Whether `command` is drawn by the vector layer.
pub fn is_vector(command: &DrawCommand) -> bool {
    matches!(
        command,
        DrawCommand::Path { .. }
            | DrawCommand::GradientPath { .. }
            | DrawCommand::PatternPath { .. }
            | DrawCommand::Blurred { .. }
            | DrawCommand::Shadow { .. }
            | DrawCommand::ClippedPath { .. }
            | DrawCommand::Line { .. }
            | DrawCommand::Text { .. }
    )
}

/// Whether a vector run must be invalidated when an image asset finishes loading.
pub fn uses_image_pattern(command: &DrawCommand) -> bool {
    match command {
        DrawCommand::PatternPath {
            pattern: FillPattern::Image { .. },
            ..
        } => true,
        DrawCommand::Blurred { command, .. } => uses_image_pattern(command),
        DrawCommand::Shadow { commands, .. } => commands.iter().any(uses_image_pattern),
        DrawCommand::AdditiveGroup { commands } => commands.iter().any(uses_image_pattern),
        _ => false,
    }
}

pub fn uses_text(command: &DrawCommand) -> bool {
    match command {
        DrawCommand::Text { .. } => true,
        DrawCommand::Blurred { command, .. } => uses_text(command),
        DrawCommand::Shadow { commands, .. } => commands.iter().any(uses_text),
        DrawCommand::AdditiveGroup { commands } => commands.iter().any(uses_text),
        _ => false,
    }
}
