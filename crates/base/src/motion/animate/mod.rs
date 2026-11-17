//! Playing a packet's [`Animation`]s: the CSS keyframe model (delay, iterations, per-interval
//! timing functions, properties that fall back to the rest frame) evaluated at a host clock time.
//! `draw_commands` carries each animated range's rest frame, so reduced motion, a delay that has
//! not elapsed and a finished animation all draw it unchanged.

use crate::packet::{
    Animation, AnimationClock, ColorRun, DrawCommand, Easing, FillPattern, GameRenderPacket,
    Gradient, Halo, PathSegment, Stroke, Vec2,
};
use serde_json::Value;
use std::borrow::Cow;
use std::f32::consts::TAU;

/// One authored drawable animation track at the host paint clock. Shape,
/// group, and mesh descriptors use the same keyframe contract.
/// The epoch-ms instant a drawable animation starts at, when it declares one.
pub fn start_ms(animation: &Value) -> Result<Option<f64>, String> {
    match animation.get("startMs").filter(|v| !v.is_null()) {
        None => Ok(None),
        Some(v) => v
            .as_f64()
            .filter(|n| n.is_finite() && *n >= 0.0)
            .map(Some)
            .ok_or_else(|| "startMs must be a non-negative finite number".to_owned()),
    }
}

/// Whether a drawable `animation` plays on the page scroll (`clock: "scroll"`): the packet plays it per frame, so the
/// paint clock draws its rest frame.
pub fn is_scroll_clocked(animation: &Value) -> bool {
    animation.get("clock").and_then(Value::as_str) == Some("scroll")
}

/// Whether the packet plays a drawable `animation` per frame: on the scroll clock, or morphing a path (a keyframe
/// carries `d`). The mapper then draws its rest frame.
pub fn plays_on_packet(animation: &Value) -> bool {
    is_scroll_clocked(animation)
        || animation
            .get("keyframes")
            .and_then(Value::as_array)
            .is_some_and(|frames| frames.iter().any(|f| f.get("d").is_some()))
}

/// The keys an epoch-clock packet animation carries: what a packet [`crate::packet::Keyframe`] moves the same way a
/// drawable track does.
const EPOCH_PACKET_KEYS: [&str; 3] = ["opacity", "offsetX", "offsetY"];

/// Whether a drawable `startMs` animation can play on the packet's epoch clock instead of the mapper's: a duration,
/// keyframes naming only opacity and offsets, and every key it names given at both `at: 0` and `at: 1`, so the
/// packet's interpolation and holds match the drawable track's exactly.
pub fn plays_on_epoch_packet(animation: &Value) -> bool {
    if !matches!(start_ms(animation), Ok(Some(_))) || plays_on_packet(animation) {
        return false;
    }
    let Some(frames) = animation
        .get("keyframes")
        .and_then(Value::as_array)
        .filter(|f| !f.is_empty())
    else {
        return false;
    };
    if !animation
        .get("durationMs")
        .and_then(Value::as_f64)
        .is_some_and(|d| d.is_finite() && d > 0.0)
    {
        return false;
    }
    let at = |frame: &Value| frame.get("at").and_then(Value::as_f64);
    let only_known_keys = frames.iter().all(|frame| {
        frame.as_object().is_some_and(|fields| {
            fields
                .keys()
                .all(|k| k == "at" || EPOCH_PACKET_KEYS.contains(&k.as_str()))
        }) && at(frame).is_some_and(|a| (0.0..=1.0).contains(&a))
    });
    let end = |value: f64| frames.iter().find(|f| at(f) == Some(value));
    let (Some(first), Some(last)) = (end(0.0), end(1.0)) else {
        return false;
    };
    only_known_keys
        && EPOCH_PACKET_KEYS.iter().all(|key| {
            let named = frames.iter().any(|f| f.get(*key).is_some());
            !named || (first.get(*key).is_some() && last.get(*key).is_some())
        })
}

/// Whether the animation plays at the paint clock: any running clock, or a `startMs` animation at no clock, which
/// shows its end state. One the packet plays never does.
pub fn plays_at(animation: &Value, epoch_now_ms: f64) -> bool {
    !plays_on_packet(animation)
        && (epoch_now_ms > 0.0 || matches!(start_ms(animation), Ok(Some(_))))
}

pub fn drawable_track(animation: &Value, time_ms: f64, key: &str) -> Option<Value> {
    let duration = animation.get("durationMs")?.as_f64()?;
    let frames = animation.get("keyframes")?.as_array()?;
    if !duration.is_finite() || duration <= 0.0 || frames.is_empty() {
        return None;
    }
    let looping = animation.get("loop").and_then(Value::as_bool) != Some(false);
    let local = match start_ms(animation).ok().flatten() {
        None => Some(time_ms),
        Some(_) if time_ms <= 0.0 => None,
        Some(start) => Some((time_ms - start).max(0.0)),
    };
    let progress = match local {
        None => 1.0,
        Some(local) => {
            let cycle = local / duration;
            if looping {
                cycle - cycle.floor()
            } else {
                cycle.min(1.0)
            }
        }
    };
    let mut defined: Vec<_> = frames
        .iter()
        .filter_map(|frame| {
            let value = key
                .split('.')
                .try_fold(frame, |value, part| value.get(part))?;
            Some((frame.get("at")?.as_f64()?, value))
        })
        .collect();
    defined.sort_by(|a, b| a.0.total_cmp(&b.0));
    let first = defined.first()?;
    let previous = defined.iter().rev().find(|(at, _)| *at <= progress);
    let Some((at, value)) = previous else {
        return Some(first.1.clone());
    };
    let Some((next_at, next_value)) = defined.iter().find(|(next_at, _)| *next_at > progress)
    else {
        return Some((*value).clone());
    };
    match (value.as_f64(), next_value.as_f64()) {
        (Some(a), Some(b)) => {
            let span = next_at - at;
            let fraction = if span > 0.0 {
                (progress - at) / span
            } else {
                1.0
            };
            serde_json::Number::from_f64(a + (b - a) * fraction).map(Value::Number)
        }
        _ => Some((*value).clone()),
    }
}

#[cfg(test)]
mod drawable_track_tests;

/// The properties of one animation sampled at one time.
#[derive(Debug, Copy, Clone, PartialEq)]
pub(crate) struct Sample {
    pub scale: f32,
    /// Degrees added to the rest frame.
    pub rotate: f32,
    pub opacity: f32,
    pub dash_offset: f32,
    pub offset_x: f32,
    pub offset_y: f32,
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
    #[cfg(test)]
    pub(crate) fn sample(&self, t_ms: f64) -> Option<Sample> {
        self.time_progress(t_ms).map(|p| self.sample_progress(p))
    }

    /// Where an epoch animation is at `epoch_ms`: 0 before it starts, its last frame once its iterations end, and its
    /// end with no epoch clock.
    fn epoch_progress(&self, start_ms: f64, epoch_ms: f64) -> f32 {
        if epoch_ms <= 0.0 || self.duration_ms <= 0.0 {
            return 1.0;
        }
        let cycle = ((epoch_ms - start_ms).max(0.0) / self.duration_ms as f64) as f32;
        match self.iterations {
            Some(n) if cycle >= n as f32 => 1.0,
            _ => cycle - cycle.floor(),
        }
    }

    /// Whether an epoch animation still moves at `epoch_ms`: not yet started, playing, or looping.
    fn epoch_moving(&self, epoch_ms: f64) -> bool {
        let AnimationClock::Epoch { start_ms } = self.clock else {
            return false;
        };
        epoch_ms > 0.0
            && match self.iterations {
                None => true,
                Some(n) => epoch_ms < start_ms + (self.duration_ms * n as f32) as f64,
            }
    }

    fn time_progress(&self, t_ms: f64) -> Option<f32> {
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
        Some(local / self.duration_ms - cycle)
    }

    /// Where the animation is on its own clock, 0 to 1; `None` while it shows the rest frame.
    pub(crate) fn progress_at(&self, t_ms: f64, epoch_ms: f64, scroll: f32) -> Option<f32> {
        match self.clock {
            AnimationClock::Time => self.time_progress(t_ms),
            AnimationClock::Epoch { start_ms } => Some(self.epoch_progress(start_ms, epoch_ms)),
            AnimationClock::Scroll { from, to } => Some(if to > from {
                ((scroll - from) / (to - from)).clamp(0.0, 1.0)
            } else if scroll < from {
                0.0
            } else {
                1.0
            }),
        }
    }

    /// The morph outline at `progress`: the two frames around it blended with the animation's easing; `None`
    /// without morph frames.
    pub(crate) fn morph_at(&self, progress: f32) -> Option<Vec<crate::packet::MorphContour>> {
        let mut frames: Vec<&crate::packet::MorphFrame> = self.morph.iter().collect();
        frames.sort_by(|a, b| a.offset.total_cmp(&b.offset));
        let first = frames.first()?;
        let before = frames
            .iter()
            .rev()
            .find(|f| f.offset <= progress)
            .unwrap_or(first);
        let Some(after) = frames.iter().find(|f| f.offset > progress) else {
            return Some(before.contours.clone());
        };
        if before.offset > progress {
            return Some(before.contours.clone());
        }
        let span = (after.offset - before.offset).max(f32::EPSILON);
        let k = self.easing.apply((progress - before.offset) / span);
        Some(crate::morph::blend(&before.contours, &after.contours, k))
    }

    fn sample_progress(&self, progress: f32) -> Sample {
        let rotation = track(self, progress, self.base_rotation, |k| k.rotate);
        Sample {
            scale: track(self, progress, 1.0, |k| k.scale),
            rotate: rotation - self.base_rotation,
            opacity: track(self, progress, 1.0, |k| k.opacity),
            dash_offset: track(self, progress, 0.0, |k| k.dash_offset),
            offset_x: track(self, progress, 0.0, |k| k.offset_x),
            offset_y: track(self, progress, 0.0, |k| k.offset_y),
        }
    }

    /// The animation on its own clock: the host time `t_ms`, or the page `scroll` offset.
    #[cfg(test)]
    pub(crate) fn sample_at(&self, t_ms: f64, scroll: f32) -> Option<Sample> {
        self.progress_at(t_ms, 0.0, scroll)
            .map(|p| self.sample_progress(p))
    }
}

#[derive(Clone, Copy)]
struct Xform {
    fade_layers: bool,
    origin: Vec2,
    translation: Vec2,
    scale: f32,
    sin: f32,
    cos: f32,
    rotated: bool,
    opacity: f32,
    dash_offset: f32,
    scale_strokes: bool,
}

impl Xform {
    fn new(a: &Animation, s: Sample) -> Self {
        let (sin, cos) = s.rotate.to_radians().sin_cos();
        Self {
            fade_layers: true,
            origin: a.origin,
            translation: Vec2 {
                x: s.offset_x,
                y: s.offset_y,
            },
            scale: s.scale,
            sin,
            cos,
            rotated: s.rotate != 0.0,
            opacity: s.opacity,
            dash_offset: s.dash_offset,
            scale_strokes: true,
        }
    }

    fn group(origin: Vec2, translation: Vec2, scale: f32, radians: f32, opacity: f32) -> Self {
        let (sin, cos) = radians.sin_cos();
        Self {
            fade_layers: true,
            origin,
            translation,
            scale,
            sin,
            cos,
            rotated: radians != 0.0,
            opacity,
            dash_offset: 0.0,
            scale_strokes: false,
        }
    }

    fn point(&self, p: Vec2) -> Vec2 {
        let (dx, dy) = (
            (p.x - self.origin.x) * self.scale,
            (p.y - self.origin.y) * self.scale,
        );
        Vec2 {
            x: self.origin.x + dx * self.cos - dy * self.sin + self.translation.x,
            y: self.origin.y + dx * self.sin + dy * self.cos + self.translation.y,
        }
    }

    fn color(&self, mut c: [f32; 4]) -> [f32; 4] {
        c[3] = (c[3] * self.opacity).clamp(0.0, 1.0);
        c
    }

    fn stroke(&self, s: &Stroke) -> Stroke {
        Stroke {
            color: self.color(s.color),
            width: s.width * if self.scale_strokes { self.scale } else { 1.0 },
            dash: s
                .dash
                .iter()
                .map(|d| d * if self.scale_strokes { self.scale } else { 1.0 })
                .collect(),
            dash_offset: s.dash_offset + self.dash_offset,
            cap: s.cap,
            join: s.join,
        }
    }

    fn gradient(&self, gradient: &Gradient) -> Gradient {
        let stops = |stops: &[crate::packet::GradientStop]| {
            stops
                .iter()
                .map(|stop| crate::packet::GradientStop {
                    offset: stop.offset,
                    color: self.color(stop.color),
                })
                .collect()
        };
        match gradient {
            Gradient::Linear {
                from,
                to,
                stops: colors,
            } => Gradient::Linear {
                from: self.point(*from),
                to: self.point(*to),
                stops: stops(colors),
            },
            Gradient::Radial {
                center,
                radius,
                stops: colors,
            } => Gradient::Radial {
                center: self.point(*center),
                radius: radius * self.scale,
                stops: stops(colors),
            },
            Gradient::Conic {
                center,
                angle,
                stops: colors,
            } => Gradient::Conic {
                center: self.point(*center),
                angle: angle + self.sin.atan2(self.cos),
                stops: stops(colors),
            },
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
        if let DrawCommand::Faded { opacity, commands } = cmd {
            let inner = Self {
                opacity: 1.0,
                ..*self
            };
            return DrawCommand::Faded {
                opacity: (*opacity * self.opacity).clamp(0.0, 1.0),
                commands: commands.iter().map(|c| inner.command(c)).collect(),
            };
        }
        if self.fade_layers && self.opacity != 1.0 && is_layer(cmd) {
            let inner = Self {
                opacity: 1.0,
                ..*self
            };
            return DrawCommand::Faded {
                opacity: self.opacity.clamp(0.0, 1.0),
                commands: vec![inner.command(cmd)],
            };
        }
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
            DrawCommand::GradientPath {
                segments,
                fill,
                stroke,
                fill_gradient,
                stroke_gradient,
            } => DrawCommand::GradientPath {
                segments: self.segments(segments),
                fill: fill.map(|color| self.color(color)),
                stroke: stroke.as_ref().map(|stroke| self.stroke(stroke)),
                fill_gradient: fill_gradient
                    .as_ref()
                    .map(|gradient| self.gradient(gradient)),
                stroke_gradient: stroke_gradient
                    .as_ref()
                    .map(|gradient| self.gradient(gradient)),
            },
            DrawCommand::PatternPath {
                segments,
                pattern,
                opacity,
            } => {
                let scale = match pattern {
                    FillPattern::Noise {
                        scale,
                        alpha,
                        color,
                    } => FillPattern::Noise {
                        scale: *scale / self.scale.abs().max(f32::EPSILON),
                        alpha: *alpha,
                        color: *color,
                    },
                    FillPattern::Image { url, scale } => FillPattern::Image {
                        url: url.clone(),
                        scale: *scale / self.scale.abs().max(f32::EPSILON),
                    },
                };
                DrawCommand::PatternPath {
                    segments: self.segments(segments),
                    pattern: scale,
                    opacity: *opacity * self.opacity,
                }
            }
            DrawCommand::Blurred { command, radius } => DrawCommand::Blurred {
                command: Box::new(self.command(command)),
                radius: *radius,
            },
            DrawCommand::Shadow {
                commands,
                color,
                sigma,
            } => DrawCommand::Shadow {
                commands: commands
                    .iter()
                    .map(|command| self.command(command))
                    .collect(),
                color: *color,
                sigma: *sigma * self.scale.abs(),
            },
            DrawCommand::AdditiveGroup { commands } => DrawCommand::AdditiveGroup {
                commands: commands
                    .iter()
                    .map(|command| {
                        Self {
                            fade_layers: false,
                            ..*self
                        }
                        .command(command)
                    })
                    .collect(),
            },
            DrawCommand::ClipGroup { clip, commands } => DrawCommand::ClipGroup {
                clip: self.segments(clip),
                commands: commands
                    .iter()
                    .map(|command| self.command(command))
                    .collect(),
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
                family,
                color_runs,
                rotation,
            } => DrawCommand::Text {
                text: text.clone(),
                pos: self.point(*pos),
                size: size * self.scale,
                color: self.color(*color),
                align: *align,
                weight: *weight,
                rotation: *rotation + self.sin.atan2(self.cos),
                family: *family,
                color_runs: color_runs
                    .iter()
                    .map(|run| ColorRun {
                        color: self.color(run.color),
                        ..*run
                    })
                    .collect(),
                halo: halo.map(|h| Halo {
                    color: self.color(h.color),
                    width: h.width * self.scale,
                }),
            },
            DrawCommand::Quad { rect, color } if self.rotated => DrawCommand::Poly {
                points: [
                    Vec2 {
                        x: rect.x,
                        y: rect.y,
                    },
                    Vec2 {
                        x: rect.x + rect.w,
                        y: rect.y,
                    },
                    Vec2 {
                        x: rect.x + rect.w,
                        y: rect.y + rect.h,
                    },
                    Vec2 {
                        x: rect.x,
                        y: rect.y + rect.h,
                    },
                ]
                .map(|p| self.point(p))
                .to_vec(),
                color: self.color(*color),
            },
            DrawCommand::Quad { rect, color } => {
                let p = self.point(Vec2 {
                    x: rect.x,
                    y: rect.y,
                });
                DrawCommand::Quad {
                    rect: crate::packet::Rect {
                        x: p.x,
                        y: p.y,
                        w: rect.w * self.scale,
                        h: rect.h * self.scale,
                    },
                    color: self.color(*color),
                }
            }
            DrawCommand::Poly { points, color } => DrawCommand::Poly {
                points: points.iter().map(|p| self.point(*p)).collect(),
                color: self.color(*color),
            },
            DrawCommand::TexturedTriangle {
                texture,
                points,
                uv,
                uv_in_pixels,
                opacity,
            } => DrawCommand::TexturedTriangle {
                texture: texture.clone(),
                points: points.map(|point| self.point(point)),
                uv: *uv,
                uv_in_pixels: *uv_in_pixels,
                opacity: opacity * self.opacity,
            },
            DrawCommand::Sprite {
                texture,
                src,
                dst,
                rotation,
                opacity,
                flip_x,
                flip_y,
                tint,
            } => {
                let center = self.point(Vec2 {
                    x: dst.x + dst.w / 2.0,
                    y: dst.y + dst.h / 2.0,
                });
                let w = dst.w * self.scale.abs();
                let h = dst.h * self.scale.abs();
                DrawCommand::Sprite {
                    texture: texture.clone(),
                    src: *src,
                    dst: crate::packet::Rect {
                        x: center.x - w / 2.0,
                        y: center.y - h / 2.0,
                        w,
                        h,
                    },
                    rotation: *rotation
                        + self.sin.atan2(self.cos)
                        + if self.scale < 0.0 {
                            std::f32::consts::PI
                        } else {
                            0.0
                        },
                    opacity: *opacity * self.opacity,
                    flip_x: *flip_x,
                    flip_y: *flip_y,
                    tint: *tint,
                }
            }
            DrawCommand::SdfBackdrop { shape, backdrop } => DrawCommand::SdfBackdrop {
                shape: Box::new(self.command(shape)),
                backdrop: crate::packet::SdfBackdrop {
                    blur: backdrop.blur * self.scale.abs(),
                    ..*backdrop
                },
            },
            DrawCommand::SdfBlend {
                first,
                second,
                radius,
            } => DrawCommand::SdfBlend {
                first: Box::new(self.command(first)),
                second: Box::new(self.command(second)),
                radius: radius * self.scale.abs(),
            },
            DrawCommand::Sdf {
                kind,
                rect,
                corners,
                corner_kind,
                arc,
                polygon,
                fill,
                stroke,
                shadow,
                rotation,
                opacity,
                material,
            } => {
                let center = self.point(Vec2 {
                    x: rect.x + rect.w / 2.0,
                    y: rect.y + rect.h / 2.0,
                });
                let size = self.scale.abs();
                let (w, h) = (rect.w * size, rect.h * size);
                let stroke_scale = if self.scale_strokes { self.scale } else { 1.0 };
                DrawCommand::Sdf {
                    kind: *kind,
                    rect: crate::packet::Rect {
                        x: center.x - w / 2.0,
                        y: center.y - h / 2.0,
                        w,
                        h,
                    },
                    corners: corners.map(|radius| radius * size),
                    corner_kind: *corner_kind,
                    arc: [arc[0], arc[1], arc[2] * size],
                    polygon: *polygon,
                    fill: self.color(*fill),
                    stroke: stroke.map(|s| crate::packet::SdfStroke {
                        color: self.color(s.color),
                        width: s.width * stroke_scale,
                        dash: s.dash.map(|d| d * stroke_scale),
                        dash_offset: s.dash_offset + self.dash_offset,
                    }),
                    shadow: shadow.map(|s| crate::packet::SdfShadow {
                        color: self.color(s.color),
                        sigma: s.sigma * size,
                        offset: Vec2 {
                            x: (s.offset.x * self.cos - s.offset.y * self.sin) * self.scale,
                            y: (s.offset.x * self.sin + s.offset.y * self.cos) * self.scale,
                        },
                    }),
                    rotation: *rotation
                        + self.sin.atan2(self.cos)
                        + if self.scale < 0.0 {
                            std::f32::consts::PI
                        } else {
                            0.0
                        },
                    opacity: *opacity * self.opacity,
                    material: material.clone(),
                }
            }
            other => other.clone(),
        }
    }
}

/// Commands drawn through the CPU vector layer, whose cache keys on content: a fade must stay outside them.
fn is_layer(cmd: &DrawCommand) -> bool {
    match cmd {
        DrawCommand::Path { .. }
        | DrawCommand::GradientPath { .. }
        | DrawCommand::PatternPath { .. }
        | DrawCommand::Blurred { .. }
        | DrawCommand::Shadow { .. }
        | DrawCommand::ClippedPath { .. }
        | DrawCommand::Line { .. } => true,
        DrawCommand::Text { halo, .. } => halo.is_some(),
        _ => false,
    }
}

/// Apply one 2D group transform to the range a drawable group just emitted.
/// The same command geometry path serves packet animations and group nesting.
pub fn transform_group_commands(
    commands: &mut [DrawCommand],
    origin: Vec2,
    translation: Vec2,
    scale: f32,
    radians: f32,
    opacity: f32,
) {
    let transform = Xform::group(origin, translation, scale, radians, opacity);
    for command in commands {
        *command = transform.command(command);
    }
}

/// Replace each command of `first..end` in the list `path` leads to (a folded command's children at each step; an
/// `SdfBlend`'s shapes are children 0 and 1) by `f` of it.
fn play_range(
    commands: &mut [DrawCommand],
    path: &[usize],
    first: usize,
    end: usize,
    f: &mut dyn FnMut(&DrawCommand) -> DrawCommand,
) {
    let Some((&at, rest)) = path.split_first() else {
        let end = end.min(commands.len());
        for command in &mut commands[first.min(end)..end] {
            *command = f(command);
        }
        return;
    };
    match commands.get_mut(at) {
        Some(
            DrawCommand::ClipGroup { commands, .. }
            | DrawCommand::AdditiveGroup { commands }
            | DrawCommand::Target { commands, .. },
        ) => play_range(commands, rest, first, end, f),
        Some(DrawCommand::SdfBlend {
            first: a,
            second: b,
            ..
        }) if rest.is_empty() => {
            for (i, shape) in [a, b].into_iter().enumerate() {
                if i >= first && i < end {
                    **shape = f(shape);
                }
            }
        }
        _ => {}
    }
}

impl GameRenderPacket {
    /// Whether the packet carries motion on the host clock, the motion that needs frames of its own; scroll-clocked
    /// motion moves only when the scroll does.
    pub fn is_animated(&self) -> bool {
        self.animations
            .iter()
            .any(|a| a.clock == AnimationClock::Time)
    }

    /// Whether the packet carries any motion, on either clock.
    pub fn has_animations(&self) -> bool {
        !self.animations.is_empty()
    }

    /// The packet with the page scrolled `offset` packet units down: every command outside the pinned ranges moves
    /// up by it, and materials read it as `Pixel.scroll`.
    pub fn scrolled(mut self, offset: f32) -> GameRenderPacket {
        self.scroll = offset;
        if offset == 0.0 {
            return self;
        }
        let shift = Xform::group(
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 0.0, y: -offset },
            1.0,
            0.0,
            1.0,
        );
        for (i, command) in self.draw_commands.iter_mut().enumerate() {
            if !self
                .pinned
                .iter()
                .any(|&(first, end)| i >= first && i < end)
            {
                *command = shift.command(command);
            }
        }
        self
    }

    /// Whether an epoch-clock animation still moves at `epoch_ms` (the page's wall clock): it needs frames until then.
    pub fn epoch_live(&self, epoch_ms: f64) -> bool {
        self.animations.iter().any(|a| a.epoch_moving(epoch_ms))
    }

    /// The packet as it draws at `t_ms` on the host clock. With `motion` off, or nothing animated,
    /// that is the rest frame: the packet itself.
    pub fn resolved(&self, t_ms: f64, motion: bool) -> Cow<'_, GameRenderPacket> {
        self.resolved_at(t_ms, 0.0, motion)
    }

    /// [`Self::resolved`] with the page's wall clock `epoch_ms` for epoch-clock animations; with `motion` off those
    /// show their end, as a drawable `startMs` animation does under reduced motion.
    pub fn resolved_at(&self, t_ms: f64, epoch_ms: f64, motion: bool) -> Cow<'_, GameRenderPacket> {
        let epoch = |a: &Animation| matches!(a.clock, AnimationClock::Epoch { .. });
        if self.animations.is_empty() || (!motion && !self.animations.iter().any(epoch)) {
            return Cow::Borrowed(self);
        }
        let mut out = self.clone();
        for a in &self.animations {
            let progress = if motion {
                a.progress_at(t_ms, epoch_ms, self.scroll)
            } else {
                epoch(a).then_some(1.0)
            };
            let Some(progress) = progress else {
                continue;
            };
            let x = Xform::new(a, a.sample_progress(progress));
            let outline = a.morph_at(progress).map(|c| crate::morph::segments(&c));
            play_range(
                &mut out.draw_commands,
                &a.path,
                a.first,
                a.end,
                &mut |command| match &outline {
                    Some(outline) => x.command(&crate::morph::with_outline(command, outline)),
                    None => x.command(command),
                },
            );
        }
        Cow::Owned(out)
    }
}

#[cfg(test)]
mod tests;
