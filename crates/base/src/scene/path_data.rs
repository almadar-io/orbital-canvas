//! SVG path data, read as the web's `Path2D` reads it, into packet path segments.

use crate::packet::{PathSegment, Rect};
use crate::transform::Xf;
use std::f32::consts::{FRAC_PI_2, TAU};

/// SVG path data (every command, absolute and relative), as the web's `Path2D` reads it.
/// `Q`/`T` are elevated and `A` converted to cubics. Malformed data rejects the whole path.
pub fn parse_path_data(d: &str, xf: Xf) -> Option<Vec<PathSegment>> {
    let mut lx = PathLexer {
        s: d.as_bytes(),
        i: 0,
    };
    let mut out = Vec::new();
    let (mut cx, mut cy) = (0.0f32, 0.0f32);
    let mut start = (0.0f32, 0.0f32);
    // The previous segment's second control point, for `S` (cubic) and `T` (quadratic).
    let mut last_cubic: Option<(f32, f32)> = None;
    let mut last_quad: Option<(f32, f32)> = None;
    let mut cmd: Option<u8> = None;
    let p = |x: f32, y: f32| xf.p(x, y);
    let cubic_seg = |c1: (f32, f32), c2: (f32, f32), e: (f32, f32)| {
        let (x1, y1) = p(c1.0, c1.1);
        let (x2, y2) = p(c2.0, c2.1);
        let (x, y) = p(e.0, e.1);
        PathSegment::CubicTo {
            x1,
            y1,
            x2,
            y2,
            x,
            y,
        }
    };
    loop {
        lx.skip_separators();
        if lx.done() {
            break;
        }
        if let Some(c) = lx.command() {
            cmd = Some(c);
        } else if cmd.is_none() {
            return None;
        }
        let c = cmd?;
        let rel = c.is_ascii_lowercase();
        let (ox, oy) = if rel { (cx, cy) } else { (0.0, 0.0) };
        let mut next_cubic = None;
        let mut next_quad = None;
        match c.to_ascii_uppercase() {
            b'Z' => {
                out.push(PathSegment::Close);
                (cx, cy) = start;
                cmd = None;
            }
            b'M' | b'L' => {
                let (x, y) = (lx.number()? + ox, lx.number()? + oy);
                let (px, py) = p(x, y);
                if c.eq_ignore_ascii_case(&b'M') {
                    out.push(PathSegment::MoveTo { x: px, y: py });
                    start = (x, y);
                    cmd = Some(if rel { b'l' } else { b'L' });
                } else {
                    out.push(PathSegment::LineTo { x: px, y: py });
                }
                (cx, cy) = (x, y);
            }
            b'H' => {
                cx = lx.number()? + ox;
                let (px, py) = p(cx, cy);
                out.push(PathSegment::LineTo { x: px, y: py });
            }
            b'V' => {
                cy = lx.number()? + oy;
                let (px, py) = p(cx, cy);
                out.push(PathSegment::LineTo { x: px, y: py });
            }
            b'C' | b'S' => {
                let c1 = if c.eq_ignore_ascii_case(&b'C') {
                    (lx.number()? + ox, lx.number()? + oy)
                } else {
                    last_cubic.map_or((cx, cy), |(qx, qy)| (2.0 * cx - qx, 2.0 * cy - qy))
                };
                let c2 = (lx.number()? + ox, lx.number()? + oy);
                let e = (lx.number()? + ox, lx.number()? + oy);
                out.push(cubic_seg(c1, c2, e));
                next_cubic = Some(c2);
                (cx, cy) = e;
            }
            b'Q' | b'T' => {
                let q = if c.eq_ignore_ascii_case(&b'Q') {
                    (lx.number()? + ox, lx.number()? + oy)
                } else {
                    last_quad.map_or((cx, cy), |(qx, qy)| (2.0 * cx - qx, 2.0 * cy - qy))
                };
                let e = (lx.number()? + ox, lx.number()? + oy);
                let c1 = (cx + 2.0 / 3.0 * (q.0 - cx), cy + 2.0 / 3.0 * (q.1 - cy));
                let c2 = (e.0 + 2.0 / 3.0 * (q.0 - e.0), e.1 + 2.0 / 3.0 * (q.1 - e.1));
                out.push(cubic_seg(c1, c2, e));
                next_quad = Some(q);
                (cx, cy) = e;
            }
            b'A' => {
                let (rx, ry, rot) = (lx.number()?, lx.number()?, lx.number()?);
                let (large, sweep) = (lx.flag()?, lx.flag()?);
                let e = (lx.number()? + ox, lx.number()? + oy);
                if e != (cx, cy) {
                    if rx == 0.0 || ry == 0.0 {
                        let (px, py) = p(e.0, e.1);
                        out.push(PathSegment::LineTo { x: px, y: py });
                    } else {
                        for (c1, c2, end) in
                            arc_to_cubics((cx, cy), e, rx.abs(), ry.abs(), rot, large, sweep)
                        {
                            out.push(cubic_seg(c1, c2, end));
                        }
                    }
                }
                (cx, cy) = e;
            }
            _ => return None,
        }
        last_cubic = next_cubic;
        last_quad = next_quad;
    }
    Some(out)
}

struct PathLexer<'a> {
    s: &'a [u8],
    i: usize,
}

impl PathLexer<'_> {
    fn done(&self) -> bool {
        self.i >= self.s.len()
    }

    fn skip_separators(&mut self) {
        while self.i < self.s.len()
            && (self.s[self.i].is_ascii_whitespace() || self.s[self.i] == b',')
        {
            self.i += 1;
        }
    }

    fn command(&mut self) -> Option<u8> {
        let c = *self.s.get(self.i)?;
        // `e`/`E` only ever appear inside a number's exponent.
        if c.is_ascii_alphabetic() && c != b'e' && c != b'E' {
            self.i += 1;
            Some(c)
        } else {
            None
        }
    }

    fn number(&mut self) -> Option<f32> {
        self.skip_separators();
        let begin = self.i;
        let at = |i: usize| self.s.get(i).copied();
        let mut i = self.i;
        if matches!(at(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let mut digits = 0;
        while at(i).is_some_and(|c| c.is_ascii_digit()) {
            i += 1;
            digits += 1;
        }
        if at(i) == Some(b'.') {
            i += 1;
            while at(i).is_some_and(|c| c.is_ascii_digit()) {
                i += 1;
                digits += 1;
            }
        }
        if digits == 0 {
            return None;
        }
        if matches!(at(i), Some(b'e' | b'E')) {
            let mut j = i + 1;
            if matches!(at(j), Some(b'+' | b'-')) {
                j += 1;
            }
            if at(j).is_some_and(|c| c.is_ascii_digit()) {
                while at(j).is_some_and(|c| c.is_ascii_digit()) {
                    j += 1;
                }
                i = j;
            }
        }
        self.i = i;
        std::str::from_utf8(&self.s[begin..i]).ok()?.parse().ok()
    }

    /// An arc flag: exactly one `0` or `1`, which may abut the next value.
    fn flag(&mut self) -> Option<bool> {
        self.skip_separators();
        let c = *self.s.get(self.i)?;
        self.i += 1;
        match c {
            b'0' => Some(false),
            b'1' => Some(true),
            _ => None,
        }
    }
}

type Cubic = ((f32, f32), (f32, f32), (f32, f32));

/// SVG arc (endpoint parameterization, spec F.6.5) as cubic Beziers of at most 90 degrees each.
fn arc_to_cubics(
    p0: (f32, f32),
    p1: (f32, f32),
    mut rx: f32,
    mut ry: f32,
    rotation_deg: f32,
    large: bool,
    sweep: bool,
) -> Vec<Cubic> {
    let (sin, cos) = rotation_deg.to_radians().sin_cos();
    let (dx, dy) = ((p0.0 - p1.0) / 2.0, (p0.1 - p1.1) / 2.0);
    let (x1p, y1p) = (cos * dx + sin * dy, -sin * dx + cos * dy);
    let lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    if lambda > 1.0 {
        let k = lambda.sqrt();
        rx *= k;
        ry *= k;
    }
    let num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
    let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
    let mut coef = (num / den).max(0.0).sqrt();
    if large == sweep {
        coef = -coef;
    }
    let (cxp, cyp) = (coef * rx * y1p / ry, -coef * ry * x1p / rx);
    let (cx, cy) = (
        cos * cxp - sin * cyp + (p0.0 + p1.0) / 2.0,
        sin * cxp + cos * cyp + (p0.1 + p1.1) / 2.0,
    );
    let angle = |ux: f32, uy: f32, vx: f32, vy: f32| (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
    let theta1 = angle(1.0, 0.0, (x1p - cxp) / rx, (y1p - cyp) / ry);
    let mut delta = angle(
        (x1p - cxp) / rx,
        (y1p - cyp) / ry,
        (-x1p - cxp) / rx,
        (-y1p - cyp) / ry,
    );
    if !sweep && delta > 0.0 {
        delta -= TAU;
    } else if sweep && delta < 0.0 {
        delta += TAU;
    }
    let pieces = (delta.abs() / FRAC_PI_2).ceil().max(1.0) as usize;
    let step = delta / pieces as f32;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let on = |t: f32| {
        let (st, ct) = t.sin_cos();
        let (ex, ey) = (rx * ct, ry * st);
        (cx + cos * ex - sin * ey, cy + sin * ex + cos * ey)
    };
    let tangent = |t: f32| {
        let (st, ct) = t.sin_cos();
        let (ex, ey) = (-rx * st, ry * ct);
        (cos * ex - sin * ey, sin * ex + cos * ey)
    };
    let mut out = Vec::with_capacity(pieces);
    for i in 0..pieces {
        let (t0, t1) = (theta1 + step * i as f32, theta1 + step * (i + 1) as f32);
        let (a, b) = (on(t0), if i + 1 == pieces { p1 } else { on(t1) });
        let (da, db) = (tangent(t0), tangent(t1));
        out.push((
            (a.0 + k * da.0, a.1 + k * da.1),
            (b.0 - k * db.0, b.1 - k * db.1),
            b,
        ));
    }
    out
}

/// The box that holds every point of a path (curve control points included).
pub fn path_bounds(segments: &[PathSegment]) -> Option<Rect> {
    let mut bounds: Option<(f32, f32, f32, f32)> = None;
    let mut add = |x: f32, y: f32| {
        bounds = Some(match bounds {
            None => (x, y, x, y),
            Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
        });
    };
    for segment in segments {
        match *segment {
            PathSegment::MoveTo { x, y } | PathSegment::LineTo { x, y } => add(x, y),
            PathSegment::CubicTo {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => {
                add(x1, y1);
                add(x2, y2);
                add(x, y);
            }
            PathSegment::Arc { cx, cy, rx, ry, .. } => {
                add(cx - rx, cy - ry);
                add(cx + rx, cy + ry);
            }
            PathSegment::Close => {}
        }
    }
    bounds.map(|(x0, y0, x1, y1)| Rect {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    })
}

/// The rectangle a path outlines when it is one closed axis-aligned rectangle with area (four corners joined by
/// straight horizontal and vertical edges), else `None`.
pub fn axis_rect(segments: &[PathSegment]) -> Option<Rect> {
    let mut corners: Vec<(f32, f32)> = Vec::with_capacity(5);
    for (i, segment) in segments.iter().enumerate() {
        match *segment {
            PathSegment::MoveTo { x, y } if i == 0 => corners.push((x, y)),
            PathSegment::LineTo { x, y } if i > 0 => corners.push((x, y)),
            PathSegment::Close if i + 1 == segments.len() => {}
            _ => return None,
        }
    }
    if corners.len() == 5 && corners[4] == corners[0] {
        corners.pop();
    }
    if corners.len() != 4 {
        return None;
    }
    let straight = (0..4).all(|i| {
        let (a, b) = (corners[i], corners[(i + 1) % 4]);
        (a.0 == b.0) != (a.1 == b.1)
    });
    let bounds = path_bounds(segments)?;
    (straight && bounds.w > 0.0 && bounds.h > 0.0).then_some(bounds)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x: f32, y: f32, w: f32, h: f32) -> Vec<PathSegment> {
        vec![
            PathSegment::MoveTo { x, y },
            PathSegment::LineTo { x: x + w, y },
            PathSegment::LineTo { x: x + w, y: y + h },
            PathSegment::LineTo { x, y: y + h },
            PathSegment::Close,
        ]
    }

    #[test]
    fn path_bounds_hold_every_point_and_control_point() {
        let mut curve = square(2.0, 3.0, 4.0, 5.0);
        curve.push(PathSegment::CubicTo {
            x1: -1.0,
            y1: 3.0,
            x2: 6.0,
            y2: 12.0,
            x: 2.0,
            y: 3.0,
        });
        let b = path_bounds(&curve).unwrap();
        assert_eq!((b.x, b.y, b.w, b.h), (-1.0, 3.0, 7.0, 9.0));
        assert!(
            path_bounds(&[PathSegment::Close]).is_none(),
            "control: no point, no box"
        );
    }

    #[test]
    fn an_axis_rect_is_a_closed_four_corner_outline_with_straight_edges() {
        let r = axis_rect(&square(2.0, 3.0, 4.0, 5.0)).unwrap();
        assert_eq!((r.x, r.y, r.w, r.h), (2.0, 3.0, 4.0, 5.0));
        let mut repeated = square(2.0, 3.0, 4.0, 5.0);
        repeated.insert(4, PathSegment::LineTo { x: 2.0, y: 3.0 });
        assert!(
            axis_rect(&repeated).is_some(),
            "a line back to the start closes it too"
        );
        let skew = vec![
            PathSegment::MoveTo { x: 0.0, y: 0.0 },
            PathSegment::LineTo { x: 4.0, y: 1.0 },
            PathSegment::LineTo { x: 4.0, y: 5.0 },
            PathSegment::LineTo { x: 0.0, y: 4.0 },
            PathSegment::Close,
        ];
        assert!(
            axis_rect(&skew).is_none(),
            "control: a rotated quad is no axis rect"
        );
        let mut rounded = square(2.0, 3.0, 4.0, 5.0);
        rounded.insert(
            2,
            PathSegment::CubicTo {
                x1: 6.0,
                y1: 3.0,
                x2: 6.0,
                y2: 3.0,
                x: 6.0,
                y: 3.0,
            },
        );
        assert!(
            axis_rect(&rounded).is_none(),
            "control: a curve is no axis rect"
        );
        let triangle = vec![
            PathSegment::MoveTo { x: 0.0, y: 0.0 },
            PathSegment::LineTo { x: 4.0, y: 0.0 },
            PathSegment::LineTo { x: 4.0, y: 4.0 },
            PathSegment::Close,
        ];
        assert!(axis_rect(&triangle).is_none(), "control: three corners");
        assert!(
            axis_rect(&square(2.0, 3.0, 0.0, 5.0)).is_none(),
            "control: no area"
        );
    }

    #[test]
    fn path_data_parses_move_line_h_v_z_absolute_and_relative() {
        let segs = parse_path_data("M 1 2 L 3 4 L 5,6", Xf::identity()).unwrap();
        assert_eq!(segs.len(), 3);
        assert!(matches!(segs[2], PathSegment::LineTo { x, y } if x == 5.0 && y == 6.0));
        let rel = parse_path_data("M 1 1 l 2 2 h 3 v -1 z", Xf::identity()).unwrap();
        assert!(matches!(rel[1], PathSegment::LineTo { x, y } if x == 3.0 && y == 3.0));
        assert!(matches!(rel[2], PathSegment::LineTo { x, y } if x == 6.0 && y == 3.0));
        assert!(matches!(rel[3], PathSegment::LineTo { x, y } if x == 6.0 && y == 2.0));
        assert!(matches!(rel[4], PathSegment::Close));
    }

    fn cubic(seg: &PathSegment) -> [f32; 6] {
        match *seg {
            PathSegment::CubicTo {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => [x1, y1, x2, y2, x, y],
            ref other => panic!("not a cubic: {other:?}"),
        }
    }

    fn close(a: [f32; 6], b: [f32; 6]) -> bool {
        a.iter().zip(b).all(|(p, q)| (p - q).abs() < 1e-3)
    }

    #[test]
    fn cubic_and_smooth_cubic_reflect_the_previous_control_point() {
        let segs = parse_path_data("M0 0 C 1 2 3 2 4 0 S 7 -2 8 0", Xf::identity()).unwrap();
        assert!(close(cubic(&segs[1]), [1.0, 2.0, 3.0, 2.0, 4.0, 0.0]));
        assert!(close(cubic(&segs[2]), [5.0, -2.0, 7.0, -2.0, 8.0, 0.0]));
        let rel = parse_path_data("m10 10 c 1 2 3 2 4 0 s 3 -2 4 0", Xf::identity()).unwrap();
        assert!(close(cubic(&rel[2]), [15.0, 8.0, 17.0, 8.0, 18.0, 10.0]));
    }

    #[test]
    fn control_smooth_cubic_without_a_previous_cubic_starts_at_the_current_point() {
        let segs = parse_path_data("M0 0 L 2 0 S 3 1 4 0", Xf::identity()).unwrap();
        assert!(close(cubic(&segs[2]), [2.0, 0.0, 3.0, 1.0, 4.0, 0.0]));
    }

    #[test]
    fn quadratic_and_smooth_quadratic_elevate_to_cubics() {
        let segs = parse_path_data("M0 0 Q 3 3 6 0 T 12 0", Xf::identity()).unwrap();
        assert!(close(cubic(&segs[1]), [2.0, 2.0, 4.0, 2.0, 6.0, 0.0]));
        assert!(close(cubic(&segs[2]), [8.0, -2.0, 10.0, -2.0, 12.0, 0.0]));
    }

    #[test]
    fn an_arc_becomes_cubics_ending_exactly_at_its_endpoint_on_the_circle() {
        let segs = parse_path_data("M 0 10 A 10 10 0 0 1 20 10", Xf::identity()).unwrap();
        let last = cubic(segs.last().unwrap());
        assert!((last[4] - 20.0).abs() < 1e-3 && (last[5] - 10.0).abs() < 1e-3);
        // A sweep-1 (clockwise on screen) half circle from (0,10) to (20,10) bulges up to y = 0.
        let top = segs[1..]
            .iter()
            .map(cubic)
            .flat_map(|c| [c[1], c[3], c[5]])
            .fold(f32::MAX, f32::min);
        assert!(top < 2.0, "{segs:?}");
        let other = parse_path_data("M 0 10 A 10 10 0 0 0 20 10", Xf::identity()).unwrap();
        let bottom = other[1..]
            .iter()
            .map(cubic)
            .flat_map(|c| [c[1], c[3], c[5]])
            .fold(f32::MIN, f32::max);
        assert!(bottom > 18.0, "sweep 0 bulges down: {other:?}");
    }

    #[test]
    fn arc_edge_cases_follow_the_svg_spec() {
        // zero radius: a straight line
        let zero = parse_path_data("M 0 0 A 0 5 0 0 1 10 0", Xf::identity()).unwrap();
        assert!(matches!(zero[1], PathSegment::LineTo { x, y } if x == 10.0 && y == 0.0));
        // same endpoint: nothing drawn
        let same = parse_path_data("M 3 3 A 5 5 0 0 1 3 3", Xf::identity()).unwrap();
        assert_eq!(same.len(), 1);
        // radii too small are scaled up: the arc still reaches its endpoint
        let small = parse_path_data("M 0 0 A 1 1 0 0 1 10 0", Xf::identity()).unwrap();
        let end = cubic(small.last().unwrap());
        assert!((end[4] - 10.0).abs() < 1e-3);
        // compact flags and numbers: "a5,5 0 011 1" and "1-2" / ".5.5"
        let compact = parse_path_data("M0 0a5,5 0 0110 0L1-2 .5.5", Xf::identity()).unwrap();
        assert!(
            matches!(compact.last().unwrap(), PathSegment::LineTo { x, y } if *x == 0.5 && *y == 0.5)
        );
    }

    #[test]
    fn control_malformed_path_data_rejects_the_whole_path() {
        assert!(parse_path_data("M 0 0 C 1 1 2", Xf::identity()).is_none());
        assert!(parse_path_data("M 0 0 X 1 1", Xf::identity()).is_none());
        assert!(
            parse_path_data("M 0 0 A 5 5 0 2 1 3 3", Xf::identity()).is_none(),
            "flag must be 0 or 1"
        );
    }

    #[test]
    fn path_data_is_mapped_through_the_transform() {
        let xf = Xf {
            s: 2.0,
            ox: 10.0,
            oy: 0.0,
        };
        let segs = parse_path_data("M0 0 C 1 1 2 1 3 0", xf).unwrap();
        assert!(close(cubic(&segs[1]), [12.0, 2.0, 14.0, 2.0, 16.0, 0.0]));
    }
}
