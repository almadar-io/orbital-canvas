//! Path morphing: an outline flattened into contours, each resampled to a fixed number of points by arc length, so
//! any two outlines with the same number of contours blend point by point.

use crate::packet::{DrawCommand, MorphContour, PathSegment, Vec2};

/// Points per contour after resampling.
pub const MORPH_POINTS: usize = 96;

const CUBIC_STEPS: usize = 24;
const ARC_STEP: f32 = std::f32::consts::PI / 48.0;

/// The outline's contours, flattened the way the vector painter draws it (an arc joins the current point with a line).
pub fn contours(segments: &[PathSegment]) -> Vec<MorphContour> {
    let mut out = Vec::new();
    let mut points: Vec<Vec2> = Vec::new();
    let finish = |points: &mut Vec<Vec2>, closed: bool, out: &mut Vec<MorphContour>| {
        if points.len() > 1 {
            out.push(MorphContour {
                points: std::mem::take(points),
                closed,
            });
        } else {
            points.clear();
        }
    };
    for segment in segments {
        match *segment {
            PathSegment::MoveTo { x, y } => {
                finish(&mut points, false, &mut out);
                points.push(Vec2 { x, y });
            }
            PathSegment::LineTo { x, y } => points.push(Vec2 { x, y }),
            PathSegment::Arc {
                cx,
                cy,
                rx,
                ry,
                start,
                end,
            } => {
                let mut sweep = end - start;
                if sweep.abs() >= std::f32::consts::TAU - 1e-6 {
                    sweep = std::f32::consts::TAU.copysign(sweep);
                }
                let steps = (sweep.abs() / ARC_STEP).ceil().max(1.0) as usize;
                for k in 0..=steps {
                    let a = start + sweep * k as f32 / steps as f32;
                    points.push(Vec2 {
                        x: cx + rx * a.cos(),
                        y: cy + ry * a.sin(),
                    });
                }
            }
            PathSegment::CubicTo {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => {
                let p0 = *points.last().unwrap_or(&Vec2 { x: x1, y: y1 });
                if points.is_empty() {
                    points.push(p0);
                }
                for k in 1..=CUBIC_STEPS {
                    let t = k as f32 / CUBIC_STEPS as f32;
                    let u = 1.0 - t;
                    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                    points.push(Vec2 {
                        x: a * p0.x + b * x1 + c * x2 + d * x,
                        y: a * p0.y + b * y1 + c * y2 + d * y,
                    });
                }
            }
            PathSegment::Close => finish(&mut points, true, &mut out),
        }
    }
    finish(&mut points, false, &mut out);
    out.into_iter()
        .map(|c| resample(&c, MORPH_POINTS))
        .collect()
}

/// The contour as `n` points evenly spaced along its length (the closing edge counts on a closed contour).
pub fn resample(contour: &MorphContour, n: usize) -> MorphContour {
    let mut ring = contour.points.clone();
    if contour.closed {
        ring.push(contour.points[0]);
    }
    let lengths: Vec<f32> = ring
        .windows(2)
        .map(|w| (w[1].x - w[0].x).hypot(w[1].y - w[0].y))
        .collect();
    let total: f32 = lengths.iter().sum();
    let count = if contour.closed { n } else { n.max(2) };
    let span = if contour.closed { count } else { count - 1 };
    let mut points = Vec::with_capacity(count);
    let (mut edge, mut walked) = (0usize, 0.0f32);
    for k in 0..count {
        let target = if total > 0.0 {
            total * k as f32 / span as f32
        } else {
            0.0
        };
        while edge + 1 < lengths.len() && walked + lengths[edge] < target {
            walked += lengths[edge];
            edge += 1;
        }
        let len = lengths.get(edge).copied().unwrap_or(0.0);
        let t = if len > 0.0 {
            ((target - walked) / len).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (a, b) = (
            ring[edge.min(ring.len() - 1)],
            ring[(edge + 1).min(ring.len() - 1)],
        );
        points.push(Vec2 {
            x: a.x + (b.x - a.x) * t,
            y: a.y + (b.y - a.y) * t,
        });
    }
    MorphContour {
        points,
        closed: contour.closed,
    }
}

/// Turns each contour of `next` to start, and run, where it best matches the same contour of `reference`: a closed
/// contour tries every starting point in both directions, an open one both directions; the match with the least summed
/// squared distance wins. Blending aligned outlines moves each point the shortest way instead of folding the shape.
pub fn align_to(reference: &[MorphContour], next: &mut [MorphContour]) {
    for (r, c) in reference.iter().zip(next.iter_mut()) {
        let n = c.points.len();
        if n == 0 || r.points.len() != n {
            continue;
        }
        let forward = c.points.clone();
        let mut backward = c.points.clone();
        backward.reverse();
        let cost = |cand: &[Vec2], shift: usize| -> f32 {
            r.points
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let q = cand[(i + shift) % n];
                    (p.x - q.x).powi(2) + (p.y - q.y).powi(2)
                })
                .sum()
        };
        let shifts = if c.closed { n } else { 1 };
        let mut best = (f32::INFINITY, false, 0usize);
        for (reversed, cand) in [(false, &forward), (true, &backward)] {
            for shift in 0..shifts {
                let d = cost(cand, shift);
                if d < best.0 {
                    best = (d, reversed, shift);
                }
            }
        }
        let source = if best.1 { &backward } else { &forward };
        c.points = (0..n).map(|i| source[(i + best.2) % n]).collect();
    }
}

/// The outline `t` of the way from `a` to `b`; both hold the same number of resampled contours.
pub fn blend(a: &[MorphContour], b: &[MorphContour], t: f32) -> Vec<MorphContour> {
    a.iter()
        .zip(b)
        .map(|(ca, cb)| MorphContour {
            points: ca
                .points
                .iter()
                .zip(&cb.points)
                .map(|(p, q)| Vec2 {
                    x: p.x + (q.x - p.x) * t,
                    y: p.y + (q.y - p.y) * t,
                })
                .collect(),
            closed: if t < 0.5 { ca.closed } else { cb.closed },
        })
        .collect()
}

/// Contours as path segments.
pub fn segments(contours: &[MorphContour]) -> Vec<PathSegment> {
    let mut out = Vec::new();
    for contour in contours {
        let mut points = contour.points.iter();
        if let Some(first) = points.next() {
            out.push(PathSegment::MoveTo {
                x: first.x,
                y: first.y,
            });
            out.extend(points.map(|p| PathSegment::LineTo { x: p.x, y: p.y }));
            if contour.closed {
                out.push(PathSegment::Close);
            }
        }
    }
    out
}

/// The command drawing `outline` in place of its own: a path's segments are replaced, a blur keeps wrapping it; any
/// other command is unchanged.
pub fn with_outline(command: &DrawCommand, outline: &[PathSegment]) -> DrawCommand {
    match command {
        DrawCommand::Path { fill, stroke, .. } => DrawCommand::Path {
            segments: outline.to_vec(),
            fill: *fill,
            stroke: stroke.clone(),
        },
        DrawCommand::GradientPath { .. } | DrawCommand::PatternPath { .. } => {
            let mut out = command.clone();
            if let DrawCommand::GradientPath { segments, .. }
            | DrawCommand::PatternPath { segments, .. } = &mut out
            {
                *segments = outline.to_vec();
            }
            out
        }
        DrawCommand::Blurred { command, radius } => DrawCommand::Blurred {
            command: Box::new(with_outline(command, outline)),
            radius: *radius,
        },
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(at: f32, side: f32) -> Vec<PathSegment> {
        vec![
            PathSegment::MoveTo { x: at, y: at },
            PathSegment::LineTo {
                x: at + side,
                y: at,
            },
            PathSegment::LineTo {
                x: at + side,
                y: at + side,
            },
            PathSegment::LineTo {
                x: at,
                y: at + side,
            },
            PathSegment::Close,
        ]
    }

    #[test]
    fn a_closed_outline_resamples_to_evenly_spaced_points_on_its_edge() {
        let c = contours(&square(0.0, 96.0));
        assert_eq!(c.len(), 1);
        assert!(c[0].closed);
        assert_eq!(c[0].points.len(), MORPH_POINTS);
        let step = |i: usize| {
            let (a, b) = (c[0].points[i], c[0].points[(i + 1) % MORPH_POINTS]);
            (b.x - a.x).hypot(b.y - a.y)
        };
        assert!(
            (step(0) - 4.0).abs() < 1e-3 && (step(50) - 4.0).abs() < 1e-3,
            "perimeter 384 over 96 points"
        );
    }

    #[test]
    fn a_circle_flattens_onto_its_radius() {
        let c = contours(&[PathSegment::Arc {
            cx: 50.0,
            cy: 50.0,
            rx: 20.0,
            ry: 20.0,
            start: 0.0,
            end: std::f32::consts::TAU,
        }]);
        assert!(c[0]
            .points
            .iter()
            .all(|p| ((p.x - 50.0).hypot(p.y - 50.0) - 20.0).abs() < 0.1));
    }

    #[test]
    fn halfway_between_two_outlines_is_their_midpoint_and_the_ends_are_the_outlines() {
        let (a, b) = (contours(&square(0.0, 10.0)), contours(&square(100.0, 10.0)));
        let mid = blend(&a, &b, 0.5);
        assert_eq!(mid[0].points[0], Vec2 { x: 50.0, y: 50.0 });
        assert_eq!(
            blend(&a, &b, 0.0)[0].points,
            a[0].points,
            "control: the start is the first outline"
        );
        assert_eq!(blend(&a, &b, 1.0)[0].points, b[0].points);
    }

    fn square_from_corner(corner: usize) -> Vec<PathSegment> {
        let pts = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let mut out = Vec::new();
        for k in 0..4 {
            let (x, y) = pts[(corner + k) % 4];
            out.push(if k == 0 {
                PathSegment::MoveTo { x, y }
            } else {
                PathSegment::LineTo { x, y }
            });
        }
        out.push(PathSegment::Close);
        out
    }

    fn spread(c: &MorphContour) -> f32 {
        let (cx, cy) = (5.0, 5.0);
        c.points
            .iter()
            .map(|p| (p.x - cx).hypot(p.y - cy))
            .sum::<f32>()
            / c.points.len() as f32
    }

    #[test]
    fn an_aligned_outline_blends_without_folding_through_its_centre() {
        let a = contours(&square_from_corner(0));
        let mut b = contours(&square_from_corner(2));
        let folded = blend(&a, &b, 0.5);
        assert!(
            spread(&folded[0]) < 2.0,
            "control: unaligned, the halfway points fold to the centre"
        );
        align_to(&a, &mut b);
        let mid = blend(&a, &b, 0.5);
        assert!(
            (spread(&mid[0]) - spread(&a[0])).abs() < 0.1,
            "aligned, halfway is the same square"
        );
    }

    #[test]
    fn alignment_also_matches_a_contour_drawn_the_other_way_round() {
        let a = contours(&square_from_corner(0));
        let mut reversed = square_from_corner(0);
        reversed[1..4].reverse();
        let mut b = contours(&reversed);
        align_to(&a, &mut b);
        let mid = blend(&a, &b, 0.5);
        assert!((spread(&mid[0]) - spread(&a[0])).abs() < 0.1);
    }

    #[test]
    fn each_subpath_is_its_own_contour() {
        let mut two = square(0.0, 10.0);
        two.extend(square(20.0, 10.0));
        assert_eq!(contours(&two).len(), 2);
        assert_eq!(contours(&square(0.0, 10.0)).len(), 1, "control");
    }

    #[test]
    fn an_outline_replaces_a_paths_segments_and_keeps_its_paint() {
        let path = DrawCommand::Blurred {
            command: Box::new(DrawCommand::Path {
                segments: square(0.0, 10.0),
                fill: Some([1.0, 0.0, 0.0, 1.0]),
                stroke: None,
            }),
            radius: 2.0,
        };
        let outline = segments(&contours(&square(5.0, 10.0)));
        match with_outline(&path, &outline) {
            DrawCommand::Blurred { command, radius } => {
                assert_eq!(radius, 2.0);
                match *command {
                    DrawCommand::Path { segments, fill, .. } => {
                        assert_eq!(segments, outline);
                        assert_eq!(fill, Some([1.0, 0.0, 0.0, 1.0]));
                    }
                    _ => panic!("path"),
                }
            }
            _ => panic!("blurred"),
        }
    }
}
