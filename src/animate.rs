//! Playing a packet's [`Animation`]s: the CSS keyframe model (delay, iterations, per-interval
//! timing functions, properties that fall back to the rest frame) evaluated at a host clock time.
//! `draw_commands` carries each animated range's rest frame, so reduced motion, a delay that has
//! not elapsed and a finished animation all draw it unchanged.

use crate::packet::{
    Animation, DrawCommand, Easing, GameRenderPacket, Halo, PathSegment, Stroke, Vec2,
};
use std::borrow::Cow;
use std::f32::consts::TAU;

/// The properties of one animation sampled at one time.
#[derive(Debug, Copy, Clone, PartialEq)]
pub(crate) struct Sample {
    pub scale: f32,
    /// Degrees added to the rest frame.
    pub rotate: f32,
    pub opacity: f32,
    pub dash_offset: f32,
}

fn bezier(p1: (f32, f32), p2: (f32, f32), x: f32) -> f32 {
    let at = |t: f32, a: f32, b: f32| {
        let u = 1.0 - t;
        3.0 * u * u * t * a + 3.0 * u * t * t * b + t * t * t
    };
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..32 {
        let mid = (lo + hi) / 2.0;
        if at(mid, p1.0, p2.0) < x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    at((lo + hi) / 2.0, p1.1, p2.1)
}

impl Easing {
    /// Eased progress for linear progress `x` in 0..=1.
    pub fn apply(self, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        match self {
            Easing::Linear => x,
            Easing::Ease => bezier((0.25, 0.1), (0.25, 1.0), x),
            Easing::EaseIn => bezier((0.42, 0.0), (1.0, 1.0), x),
            Easing::EaseOut => bezier((0.0, 0.0), (0.58, 1.0), x),
            Easing::EaseInOut => bezier((0.42, 0.0), (0.58, 1.0), x),
        }
    }
}

/// One property's value at `progress`: keyframes naming it, with the rest value standing in at
/// 0 and 1 when they do not.
fn track(
    a: &Animation,
    progress: f32,
    rest: f32,
    of: impl Fn(&crate::packet::Keyframe) -> Option<f32>,
) -> f32 {
    let mut points: Vec<(f32, f32)> = a
        .keyframes
        .iter()
        .filter_map(|k| of(k).map(|v| (k.offset.clamp(0.0, 1.0), v)))
        .collect();
    points.sort_by(|x, y| x.0.total_cmp(&y.0));
    if points.is_empty() {
        return rest;
    }
    if points[0].0 > 0.0 {
        points.insert(0, (0.0, rest));
    }
    if points[points.len() - 1].0 < 1.0 {
        points.push((1.0, rest));
    }
    for w in points.windows(2) {
        let ((o0, v0), (o1, v1)) = (w[0], w[1]);
        if progress >= o0 && progress <= o1 {
            let span = (o1 - o0).max(f32::EPSILON);
            let k = a.easing.apply((progress - o0) / span);
            return v0 + (v1 - v0) * k;
        }
    }
    rest
}

impl Animation {
    /// The animation at `t_ms` on the host clock; `None` while it shows the rest frame.
    pub(crate) fn sample(&self, t_ms: f64) -> Option<Sample> {
        if self.duration_ms <= 0.0 {
            return None;
        }
        let local = t_ms as f32 - self.delay_ms;
        if local < 0.0 {
            return None;
        }
        let cycle = (local / self.duration_ms).floor();
        if self.iterations.is_some_and(|n| cycle >= n as f32) {
            return None;
        }
        let progress = local / self.duration_ms - cycle;
        let rotation = track(self, progress, self.base_rotation, |k| k.rotate);
        Some(Sample {
            scale: track(self, progress, 1.0, |k| k.scale),
            rotate: rotation - self.base_rotation,
            opacity: track(self, progress, 1.0, |k| k.opacity),
            dash_offset: track(self, progress, 0.0, |k| k.dash_offset),
        })
    }
}

struct Xform {
    origin: Vec2,
    scale: f32,
    sin: f32,
    cos: f32,
    rotated: bool,
    opacity: f32,
    dash_offset: f32,
}

impl Xform {
    fn new(a: &Animation, s: Sample) -> Self {
        let (sin, cos) = s.rotate.to_radians().sin_cos();
        Self {
            origin: a.origin,
            scale: s.scale,
            sin,
            cos,
            rotated: s.rotate != 0.0,
            opacity: s.opacity,
            dash_offset: s.dash_offset,
        }
    }

    fn point(&self, p: Vec2) -> Vec2 {
        let (dx, dy) = (
            (p.x - self.origin.x) * self.scale,
            (p.y - self.origin.y) * self.scale,
        );
        Vec2 {
            x: self.origin.x + dx * self.cos - dy * self.sin,
            y: self.origin.y + dx * self.sin + dy * self.cos,
        }
    }

    fn color(&self, mut c: [f32; 4]) -> [f32; 4] {
        c[3] = (c[3] * self.opacity).clamp(0.0, 1.0);
        c
    }

    fn stroke(&self, s: &Stroke) -> Stroke {
        Stroke {
            color: self.color(s.color),
            width: s.width * self.scale,
            dash: s.dash.iter().map(|d| d * self.scale).collect(),
            dash_offset: s.dash_offset + self.dash_offset,
            cap: s.cap,
            join: s.join,
        }
    }

    fn segments(&self, segments: &[PathSegment]) -> Vec<PathSegment> {
        let mut out = Vec::with_capacity(segments.len());
        for seg in segments {
            match *seg {
                PathSegment::MoveTo { x, y } => {
                    let p = self.point(Vec2 { x, y });
                    out.push(PathSegment::MoveTo { x: p.x, y: p.y });
                }
                PathSegment::LineTo { x, y } => {
                    let p = self.point(Vec2 { x, y });
                    out.push(PathSegment::LineTo { x: p.x, y: p.y });
                }
                PathSegment::CubicTo {
                    x1,
                    y1,
                    x2,
                    y2,
                    x,
                    y,
                } => {
                    let c1 = self.point(Vec2 { x: x1, y: y1 });
                    let c2 = self.point(Vec2 { x: x2, y: y2 });
                    let p = self.point(Vec2 { x, y });
                    out.push(PathSegment::CubicTo {
                        x1: c1.x,
                        y1: c1.y,
                        x2: c2.x,
                        y2: c2.y,
                        x: p.x,
                        y: p.y,
                    });
                }
                PathSegment::Arc {
                    cx,
                    cy,
                    rx,
                    ry,
                    start,
                    end,
                } => {
                    let c = self.point(Vec2 { x: cx, y: cy });
                    let spin = self.sin.atan2(self.cos);
                    if !self.rotated || (rx - ry).abs() <= f32::EPSILON {
                        out.push(PathSegment::Arc {
                            cx: c.x,
                            cy: c.y,
                            rx: rx * self.scale,
                            ry: ry * self.scale,
                            start: start + spin,
                            end: end + spin,
                        });
                    } else {
                        let sweep = (end - start).clamp(-TAU, TAU);
                        let steps = ((sweep.abs() / TAU) * 64.0).ceil().max(2.0) as usize;
                        for i in 0..=steps {
                            let a = start + sweep * i as f32 / steps as f32;
                            let p = self.point(Vec2 {
                                x: cx + rx * a.cos(),
                                y: cy + ry * a.sin(),
                            });
                            out.push(if i == 0 {
                                PathSegment::MoveTo { x: p.x, y: p.y }
                            } else {
                                PathSegment::LineTo { x: p.x, y: p.y }
                            });
                        }
                    }
                }
                PathSegment::Close => out.push(PathSegment::Close),
            }
        }
        out
    }

    fn command(&self, cmd: &DrawCommand) -> DrawCommand {
        match cmd {
            DrawCommand::Path {
                segments,
                fill,
                stroke,
            } => DrawCommand::Path {
                segments: self.segments(segments),
                fill: fill.map(|c| self.color(c)),
                stroke: stroke.as_ref().map(|s| self.stroke(s)),
            },
            DrawCommand::ClippedPath {
                segments,
                fill,
                stroke,
                clip,
                cut,
            } => DrawCommand::ClippedPath {
                segments: self.segments(segments),
                fill: fill.map(|c| self.color(c)),
                stroke: stroke.as_ref().map(|s| self.stroke(s)),
                clip: clip.iter().map(|r| self.segments(r)).collect(),
                cut: cut.iter().map(|r| self.segments(r)).collect(),
            },
            DrawCommand::Line {
                from,
                to,
                color,
                width,
            } => DrawCommand::Line {
                from: self.point(*from),
                to: self.point(*to),
                color: self.color(*color),
                width: width * self.scale,
            },
            DrawCommand::Text {
                text,
                pos,
                size,
                color,
                align,
                weight,
                halo,
                mono,
            } => DrawCommand::Text {
                text: text.clone(),
                pos: self.point(*pos),
                size: size * self.scale,
                color: self.color(*color),
                align: *align,
                weight: *weight,
                mono: *mono,
                halo: halo.map(|h| Halo {
                    color: self.color(h.color),
                    width: h.width * self.scale,
                }),
            },
            DrawCommand::Quad { rect, color } => DrawCommand::Quad {
                rect: *rect,
                color: self.color(*color),
            },
            DrawCommand::Poly { points, color } => DrawCommand::Poly {
                points: points.iter().map(|p| self.point(*p)).collect(),
                color: self.color(*color),
            },
            other => other.clone(),
        }
    }
}

impl GameRenderPacket {
    /// Whether the packet carries motion.
    pub fn is_animated(&self) -> bool {
        !self.animations.is_empty()
    }

    /// The packet as it draws at `t_ms` on the host clock. With `motion` off, or nothing animated,
    /// that is the rest frame: the packet itself.
    pub fn resolved(&self, t_ms: f64, motion: bool) -> Cow<'_, GameRenderPacket> {
        if !motion || self.animations.is_empty() {
            return Cow::Borrowed(self);
        }
        let mut out = self.clone();
        for a in &self.animations {
            let Some(sample) = a.sample(t_ms) else {
                continue;
            };
            let x = Xform::new(a, sample);
            let end = a.end.min(out.draw_commands.len());
            for i in a.first.min(end)..end {
                out.draw_commands[i] = x.command(&self.draw_commands[i]);
            }
        }
        Cow::Owned(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::{Keyframe, LineCap, LineJoin};

    const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

    fn pulse() -> Animation {
        Animation {
            first: 0,
            end: 1,
            duration_ms: 2000.0,
            delay_ms: 600.0,
            iterations: None,
            easing: Easing::EaseOut,
            origin: Vec2 { x: 70.0, y: 70.0 },
            base_rotation: 0.0,
            keyframes: vec![
                Keyframe {
                    offset: 0.0,
                    scale: Some(0.3),
                    opacity: Some(0.4),
                    ..Keyframe::default()
                },
                Keyframe {
                    offset: 1.0,
                    scale: Some(1.0),
                    opacity: Some(0.0),
                    ..Keyframe::default()
                },
            ],
        }
    }

    fn circle(r: f32) -> DrawCommand {
        DrawCommand::Path {
            segments: vec![PathSegment::Arc {
                cx: 70.0,
                cy: 70.0,
                rx: r,
                ry: r,
                start: 0.0,
                end: TAU,
            }],
            fill: None,
            stroke: Some(Stroke {
                color: RED,
                width: 2.0,
                dash: vec![],
                dash_offset: 0.0,
                cap: LineCap::Round,
                join: LineJoin::Round,
            }),
        }
    }

    fn packet(cmds: Vec<DrawCommand>, animations: Vec<Animation>) -> GameRenderPacket {
        GameRenderPacket {
            width: 140.0,
            height: 140.0,
            draw_commands: cmds,
            animations,
            ..Default::default()
        }
    }

    fn radius(p: &GameRenderPacket, i: usize) -> f32 {
        match &p.draw_commands[i] {
            DrawCommand::Path { segments, .. } => match segments[0] {
                PathSegment::Arc { rx, .. } => rx,
                _ => panic!("not an arc"),
            },
            _ => panic!("not a path"),
        }
    }

    fn alpha(p: &GameRenderPacket, i: usize) -> f32 {
        match &p.draw_commands[i] {
            DrawCommand::Path {
                stroke: Some(s), ..
            } => s.color[3],
            _ => panic!("no stroke"),
        }
    }

    #[test]
    fn easing_runs_from_zero_to_one_and_each_curve_keeps_its_shape() {
        for e in [
            Easing::Linear,
            Easing::Ease,
            Easing::EaseIn,
            Easing::EaseOut,
            Easing::EaseInOut,
        ] {
            assert!(
                e.apply(0.0).abs() < 1e-4 && (e.apply(1.0) - 1.0).abs() < 1e-4,
                "{e:?}"
            );
        }
        assert_eq!(Easing::Linear.apply(0.25), 0.25);
        assert!(Easing::EaseOut.apply(0.5) > 0.5, "ease-out decelerates");
        assert!(
            Easing::EaseIn.apply(0.5) < 0.5,
            "control: ease-in accelerates"
        );
        assert!(
            (Easing::EaseInOut.apply(0.5) - 0.5).abs() < 1e-3,
            "ease-in-out is symmetric"
        );
    }

    #[test]
    fn a_delayed_animation_shows_its_rest_frame_until_the_delay_elapses() {
        let a = pulse();
        assert_eq!(a.sample(0.0), None);
        assert_eq!(a.sample(599.0), None);
        let s = a.sample(600.0).expect("starts at the delay");
        assert!((s.scale - 0.3).abs() < 1e-3 && (s.opacity - 0.4).abs() < 1e-3);
    }

    #[test]
    fn an_infinite_animation_repeats_and_a_finite_one_returns_to_rest() {
        let a = Animation {
            delay_ms: 0.0,
            ..pulse()
        };
        let (first, again) = (a.sample(500.0).unwrap(), a.sample(2500.0).unwrap());
        assert!((first.scale - again.scale).abs() < 1e-4);
        let once = Animation {
            iterations: Some(1),
            ..a
        };
        assert!(once.sample(1999.0).is_some());
        assert_eq!(
            once.sample(2000.0),
            None,
            "control: the second cycle never plays"
        );
    }

    #[test]
    fn midway_through_an_ease_out_cycle_the_ring_is_past_the_linear_midpoint() {
        let a = Animation {
            delay_ms: 0.0,
            ..pulse()
        };
        let s = a.sample(1000.0).unwrap();
        assert!(
            s.scale > 0.65,
            "linear midpoint of 0.3..1 is 0.65, got {}",
            s.scale
        );
        assert!(s.opacity < 0.2);
    }

    #[test]
    fn a_property_no_keyframe_names_keeps_the_rest_value() {
        let a = Animation {
            delay_ms: 0.0,
            ..pulse()
        };
        let s = a.sample(500.0).unwrap();
        assert_eq!((s.rotate, s.dash_offset), (0.0, 0.0));
    }

    #[test]
    fn a_keyframe_rotation_replaces_the_rotation_the_rest_frame_carries() {
        let a = Animation {
            base_rotation: 60.0,
            delay_ms: 0.0,
            duration_ms: 1000.0,
            easing: Easing::Linear,
            keyframes: vec![
                Keyframe {
                    offset: 0.0,
                    rotate: Some(0.0),
                    ..Keyframe::default()
                },
                Keyframe {
                    offset: 1.0,
                    rotate: Some(360.0),
                    ..Keyframe::default()
                },
            ],
            ..pulse()
        };
        assert!(
            (a.sample(0.0).unwrap().rotate + 60.0).abs() < 1e-3,
            "starts at 0 degrees, 60 below the rest"
        );
        assert!((a.sample(500.0).unwrap().rotate - 120.0).abs() < 1e-3);
    }

    #[test]
    fn resolving_scales_a_circle_about_the_origin_and_fades_its_stroke() {
        let p = packet(
            vec![circle(40.0)],
            vec![Animation {
                delay_ms: 0.0,
                ..pulse()
            }],
        );
        let at0 = p.resolved(0.0, true);
        assert!((radius(&at0, 0) - 12.0).abs() < 1e-3, "40 x 0.3");
        assert!((alpha(&at0, 0) - 0.4).abs() < 1e-3);
        match &at0.draw_commands[0] {
            DrawCommand::Path {
                segments,
                stroke: Some(s),
                ..
            } => {
                assert!(
                    matches!(segments[0], PathSegment::Arc { cx, cy, .. } if cx == 70.0 && cy == 70.0)
                );
                assert!(
                    (s.width - 0.6).abs() < 1e-3,
                    "stroke scales with the geometry, like CSS scale"
                );
            }
            _ => panic!("path"),
        }
        let later = p.resolved(1000.0, true);
        assert_ne!(radius(&later, 0), radius(&at0, 0), "the ring has moved");
    }

    #[test]
    fn motion_off_a_static_packet_and_the_rest_frame_are_the_packet_itself() {
        let animated = packet(
            vec![circle(40.0)],
            vec![Animation {
                delay_ms: 0.0,
                ..pulse()
            }],
        );
        assert!(
            matches!(animated.resolved(500.0, false), Cow::Borrowed(_)),
            "reduced motion"
        );
        let still = packet(vec![circle(40.0)], vec![]);
        assert!(
            matches!(still.resolved(500.0, true), Cow::Borrowed(_)),
            "control: nothing declared"
        );
        assert!(!still.is_animated() && animated.is_animated());
        let waiting = animated.resolved(0.0, true);
        assert!(
            (radius(&waiting, 0) - 12.0).abs() < 1e-3,
            "control: motion on shows the animated frame at t=0"
        );
        let delayed = packet(vec![circle(40.0)], vec![pulse()]);
        assert_eq!(
            radius(&delayed.resolved(100.0, true), 0),
            40.0,
            "inside the delay: rest frame"
        );
    }

    #[test]
    fn only_the_animated_range_changes() {
        let p = packet(
            vec![circle(10.0), circle(40.0), circle(20.0)],
            vec![Animation {
                first: 1,
                end: 2,
                delay_ms: 0.0,
                ..pulse()
            }],
        );
        let r = p.resolved(0.0, true);
        assert_eq!((radius(&r, 0), radius(&r, 2)), (10.0, 20.0));
        assert!((radius(&r, 1) - 12.0).abs() < 1e-3);
    }

    #[test]
    fn a_range_past_the_command_list_is_clamped_not_a_panic() {
        let p = packet(
            vec![circle(40.0)],
            vec![Animation {
                first: 0,
                end: 9,
                delay_ms: 0.0,
                ..pulse()
            }],
        );
        assert_eq!(p.resolved(0.0, true).draw_commands.len(), 1);
        let backwards = packet(
            vec![circle(40.0)],
            vec![Animation {
                first: 5,
                end: 2,
                delay_ms: 0.0,
                ..pulse()
            }],
        );
        assert_eq!(radius(&backwards.resolved(0.0, true), 0), 40.0);
    }

    #[test]
    fn dash_offset_accumulates_on_the_stroke_and_scales_nothing_else() {
        let mut c = circle(40.0);
        if let DrawCommand::Path {
            stroke: Some(s), ..
        } = &mut c
        {
            s.dash = vec![8.0, 6.0];
        }
        let a = Animation {
            delay_ms: 0.0,
            duration_ms: 800.0,
            easing: Easing::Linear,
            keyframes: vec![
                Keyframe {
                    offset: 0.0,
                    dash_offset: Some(0.0),
                    ..Keyframe::default()
                },
                Keyframe {
                    offset: 1.0,
                    dash_offset: Some(-14.0),
                    ..Keyframe::default()
                },
            ],
            ..pulse()
        };
        let r = packet(vec![c], vec![a]).resolved(400.0, true).into_owned();
        match &r.draw_commands[0] {
            DrawCommand::Path {
                stroke: Some(s), ..
            } => {
                assert!((s.dash_offset + 7.0).abs() < 1e-3);
                assert_eq!(s.dash, vec![8.0, 6.0]);
                assert_eq!(s.width, 2.0);
            }
            _ => panic!("path"),
        }
    }

    #[test]
    fn rotating_a_circle_keeps_an_arc_and_rotating_an_ellipse_flattens_it() {
        let rot = Animation {
            delay_ms: 0.0,
            duration_ms: 1000.0,
            easing: Easing::Linear,
            keyframes: vec![
                Keyframe {
                    offset: 0.0,
                    rotate: Some(0.0),
                    ..Keyframe::default()
                },
                Keyframe {
                    offset: 1.0,
                    rotate: Some(360.0),
                    ..Keyframe::default()
                },
            ],
            origin: Vec2 { x: 0.0, y: 0.0 },
            ..pulse()
        };
        let off_centre = DrawCommand::Path {
            segments: vec![PathSegment::Arc {
                cx: 10.0,
                cy: 0.0,
                rx: 4.0,
                ry: 4.0,
                start: 0.0,
                end: TAU,
            }],
            fill: Some(RED),
            stroke: None,
        };
        let r = packet(vec![off_centre], vec![rot.clone()])
            .resolved(250.0, true)
            .into_owned();
        match &r.draw_commands[0] {
            DrawCommand::Path { segments, .. } => match segments[0] {
                PathSegment::Arc { cx, cy, rx, .. } => {
                    assert!(
                        cx.abs() < 1e-3 && (cy - 10.0).abs() < 1e-3,
                        "a quarter turn moves (10,0) to (0,10)"
                    );
                    assert_eq!(rx, 4.0);
                }
                _ => panic!("a circle stays an arc"),
            },
            _ => panic!("path"),
        }
        let ellipse = DrawCommand::Path {
            segments: vec![PathSegment::Arc {
                cx: 0.0,
                cy: 0.0,
                rx: 8.0,
                ry: 2.0,
                start: 0.0,
                end: TAU,
            }],
            fill: Some(RED),
            stroke: None,
        };
        let r = packet(vec![ellipse], vec![rot])
            .resolved(250.0, true)
            .into_owned();
        match &r.draw_commands[0] {
            DrawCommand::Path { segments, .. } => {
                assert!(
                    segments.len() > 8
                        && segments
                            .iter()
                            .all(|s| !matches!(s, PathSegment::Arc { .. }))
                );
                let ys = segments.iter().filter_map(|s| match s {
                    PathSegment::MoveTo { y, .. } | PathSegment::LineTo { y, .. } => Some(y.abs()),
                    _ => None,
                });
                assert!(
                    ys.fold(0.0f32, f32::max) > 7.5,
                    "the long axis now points down"
                );
            }
            _ => panic!("path"),
        }
    }

    #[test]
    fn text_and_lines_scale_and_fade_with_the_group() {
        let cmds = vec![
            DrawCommand::Text {
                text: "x".into(),
                pos: Vec2 { x: 70.0, y: 40.0 },
                size: 10.0,
                color: RED,
                align: Default::default(),
                weight: 400,
                mono: false,
                halo: None,
            },
            DrawCommand::Line {
                from: Vec2 { x: 70.0, y: 70.0 },
                to: Vec2 { x: 70.0, y: 40.0 },
                color: RED,
                width: 2.0,
            },
        ];
        let r = packet(
            cmds,
            vec![Animation {
                end: 2,
                delay_ms: 0.0,
                ..pulse()
            }],
        )
        .resolved(0.0, true)
        .into_owned();
        match (&r.draw_commands[0], &r.draw_commands[1]) {
            (
                DrawCommand::Text {
                    size, pos, color, ..
                },
                DrawCommand::Line { to, width, .. },
            ) => {
                assert!((size - 3.0).abs() < 1e-3 && (pos.y - 61.0).abs() < 1e-3);
                assert!((color[3] - 0.4).abs() < 1e-3);
                assert!((to.y - 61.0).abs() < 1e-3 && (width - 0.6).abs() < 1e-3);
            }
            _ => panic!("text and line"),
        }
    }
}
