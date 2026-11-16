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
            ..
        } => match text_path(text, *pos, *size, *align, *weight >= 600, *family) {
            Some(paths) => {
                let b = paths.all.bounds();
                let reach = halo.as_ref().map(|h| h.width).unwrap_or(0.0) + size * 0.05 + 1.0;
                RunBounds::Box {
                    x0: b.left(),
                    y0: b.top(),
                    x1: b.right(),
                    y1: b.bottom(),
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
        _ => RunBounds::Anywhere,
    }
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
