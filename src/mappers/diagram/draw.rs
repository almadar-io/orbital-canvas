//! The pen every diagram mapper draws with: scene-unit geometry in, packet draw commands out.
//! Every color resolves through the scene's [`DiagramTheme`]; a CSS-variable color prop
//! (`var(--color-primary)`) maps to the tone that variable feeds in `canvas_theme`.

use crate::mappers::learning::canvas::fit;
use crate::mappers::learning::shape::parse_path_data;
use crate::mappers::learning::theme::{DiagramTheme, Rgba};
use crate::mappers::learning::{gap_once, Xf};
use crate::mappers::scene::{Drag, Emit, Hit, MappedScene};
use crate::packet::{
    Animation, DrawCommand, Easing, GameRenderPacket, Keyframe, LineCap, LineJoin, PathSegment,
    Rect, Stroke, TextAlign, Vec2,
};
use std::f32::consts::TAU;

/// Pixels from an SVG text baseline up to the middle of its em box.
const BASELINE_TO_MIDDLE: f32 = 0.35;
const CURVE_STEPS: usize = 18;

#[derive(Debug, Clone)]
pub struct Ink {
    pub color: Rgba,
    pub width: f32,
    pub dash: Vec<f32>,
    pub cap: LineCap,
    pub join: LineJoin,
}

impl Ink {
    pub fn new(color: Rgba, width: f32) -> Self {
        Self {
            color,
            width,
            dash: Vec::new(),
            cap: LineCap::Round,
            join: LineJoin::Round,
        }
    }

    pub fn dashed(mut self, dash: &[f32]) -> Self {
        self.dash = dash.to_vec();
        self
    }

    pub fn butt(mut self) -> Self {
        self.cap = LineCap::Butt;
        self
    }
}

pub fn with_alpha(mut c: Rgba, a: f32) -> Rgba {
    c[3] *= a;
    c
}

/// A tone mixed over transparent: CSS `color-mix(in srgb, <c> <percent>%, transparent)`.
pub fn tint(c: Rgba, percent: f32) -> Rgba {
    with_alpha(c, percent / 100.0)
}

/// Linear mix in sRGB: CSS `color-mix(in srgb, <a> <percent>%, <b>)`.
pub fn mix(a: Rgba, percent: f32, b: Rgba) -> Rgba {
    let t = percent / 100.0;
    [
        a[0] * t + b[0] * (1.0 - t),
        a[1] * t + b[1] * (1.0 - t),
        a[2] * t + b[2] * (1.0 - t),
        a[3] * t + b[3] * (1.0 - t),
    ]
}

pub struct Pen<'a> {
    pub theme: &'a DiagramTheme,
    pub xf: Xf,
    pub cmds: Vec<DrawCommand>,
    pub animations: Vec<Animation>,
    pub hits: Vec<Hit>,
    pub drags: Vec<Drag>,
}

impl<'a> Pen<'a> {
    pub fn new(theme: &'a DiagramTheme, xf: Xf) -> Self {
        Self {
            theme,
            xf,
            cmds: Vec::new(),
            animations: Vec::new(),
            hits: Vec::new(),
            drags: Vec::new(),
        }
    }

    pub fn tone(&self, name: &str) -> Rgba {
        self.theme.tone(name).unwrap_or(self.theme.ground)
    }

    pub fn ground(&self) -> Rgba {
        self.theme.ground
    }

    /// `--color-card`: card surfaces drawn inside a diagram (AVL `surface`).
    pub fn card(&self) -> Rgba {
        self.theme.surface_card
    }

    /// `--color-muted`: the neutral wash (AVL `wash`).
    pub fn muted(&self) -> Rgba {
        self.theme.surface_muted
    }

    /// A color prop: a tone name, a CSS literal, or a `var(--color-*)` token; absent falls to `fallback`.
    pub fn paint(&self, value: Option<&str>, fallback: &str) -> Rgba {
        let Some(v) = value.filter(|v| !v.is_empty()) else {
            return self.tone(fallback);
        };
        if let Some(name) = v
            .strip_prefix("var(--color-")
            .and_then(|r| r.strip_suffix(')'))
        {
            return self.token(name, fallback);
        }
        self.theme.mark_color(Some(v), fallback)
    }

    /// The tone a theme token feeds (inverse of `canvas_theme`'s tone sources).
    pub fn token(&self, name: &str, fallback: &str) -> Rgba {
        match name {
            "foreground" => self.tone("ink"),
            "muted-foreground" => self.tone("label"),
            "border" => self.tone("grid"),
            "muted" => self.tone("fill"),
            "background" | "card" | "primary-foreground" => self.ground(),
            other => match self.theme.tone(other) {
                Some(c) => c,
                None => {
                    gap_once(
                        "color-token",
                        &format!("theme token `{other}` has no tone — fell back to {fallback}"),
                    );
                    self.tone(fallback)
                }
            },
        }
    }

    fn p(&self, x: f32, y: f32) -> (f32, f32) {
        self.xf.p(x, y)
    }

    fn stroke(&self, ink: &Ink) -> Stroke {
        Stroke {
            color: ink.color,
            width: ink.width * self.xf.s,
            dash: ink.dash.iter().map(|d| d * self.xf.s).collect(),
            dash_offset: 0.0,
            cap: ink.cap,
            join: ink.join,
        }
    }

    pub fn clear(&mut self, color: Rgba) {
        self.cmds.push(DrawCommand::Clear { color });
    }

    fn path(&mut self, segments: Vec<PathSegment>, fill: Option<Rgba>, ink: Option<&Ink>) {
        if fill.is_none() && ink.is_none() {
            return;
        }
        self.cmds.push(DrawCommand::Path {
            segments,
            fill,
            stroke: ink.map(|i| self.stroke(i)),
        });
    }

    pub fn polyline(
        &mut self,
        points: &[(f32, f32)],
        closed: bool,
        fill: Option<Rgba>,
        ink: Option<&Ink>,
    ) {
        if points.len() < 2 {
            return;
        }
        let mut segs = Vec::with_capacity(points.len() + 1);
        for (i, (x, y)) in points.iter().enumerate() {
            let (px, py) = self.p(*x, *y);
            segs.push(if i == 0 {
                PathSegment::MoveTo { x: px, y: py }
            } else {
                PathSegment::LineTo { x: px, y: py }
            });
        }
        if closed {
            segs.push(PathSegment::Close);
        }
        self.path(segs, fill, ink);
    }

    /// SVG path data in scene units (every command; see [`parse_path_data`]).
    pub fn svg_path(&mut self, d: &str, fill: Option<Rgba>, ink: Option<&Ink>) {
        match parse_path_data(d, self.xf) {
            Some(segments) => self.path(segments, fill, ink),
            None => gap_once("svg-path-data", "malformed path data — skipped"),
        }
    }

    pub fn line(&mut self, a: (f32, f32), b: (f32, f32), ink: &Ink) {
        self.polyline(&[a, b], false, None, Some(ink));
    }

    pub fn circle(&mut self, cx: f32, cy: f32, r: f32, fill: Option<Rgba>, ink: Option<&Ink>) {
        let (px, py) = self.p(cx, cy);
        let rr = r * self.xf.s;
        self.path(
            vec![PathSegment::Arc {
                cx: px,
                cy: py,
                rx: rr,
                ry: rr,
                start: 0.0,
                end: TAU,
            }],
            fill,
            ink,
        );
    }

    /// An ellipse rotated by `rotation` degrees about (`cx`, `cy`) — flattened, since arcs carry no rotation.
    pub fn ellipse(
        &mut self,
        cx: f32,
        cy: f32,
        rx: f32,
        ry: f32,
        rotation: f32,
        fill: Option<Rgba>,
        ink: Option<&Ink>,
    ) {
        let (sin, cos) = rotation.to_radians().sin_cos();
        let pts: Vec<(f32, f32)> = (0..64)
            .map(|i| {
                let t = i as f32 / 64.0 * TAU;
                let (ex, ey) = (rx * t.cos(), ry * t.sin());
                (cx + ex * cos - ey * sin, cy + ex * sin + ey * cos)
            })
            .collect();
        self.polyline(&pts, true, fill, ink);
    }

    pub fn rrect(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        r: f32,
        fill: Option<Rgba>,
        ink: Option<&Ink>,
    ) {
        let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
        if r <= 0.0 {
            self.polyline(
                &[(x, y), (x + w, y), (x + w, y + h), (x, y + h)],
                true,
                fill,
                ink,
            );
            return;
        }
        let s = self.xf.s;
        let arc = |pen: &Self, cx: f32, cy: f32, start: f32| {
            let (px, py) = pen.p(cx, cy);
            PathSegment::Arc {
                cx: px,
                cy: py,
                rx: r * s,
                ry: r * s,
                start,
                end: start + TAU / 4.0,
            }
        };
        let lt = |pen: &Self, px: f32, py: f32| {
            let (x, y) = pen.p(px, py);
            PathSegment::LineTo { x, y }
        };
        let (sx, sy) = self.p(x + r, y);
        let segs = vec![
            PathSegment::MoveTo { x: sx, y: sy },
            lt(self, x + w - r, y),
            arc(self, x + w - r, y + r, -TAU / 4.0),
            lt(self, x + w, y + h - r),
            arc(self, x + w - r, y + h - r, 0.0),
            lt(self, x + r, y + h),
            arc(self, x + r, y + h - r, TAU / 4.0),
            lt(self, x, y + r),
            arc(self, x + r, y + r, TAU / 2.0),
            PathSegment::Close,
        ];
        self.path(segs, fill, ink);
    }

    /// Cubic Bezier points from (`p0`) through controls to (`p3`), flattened.
    pub fn cubic_points(
        p0: (f32, f32),
        p1: (f32, f32),
        p2: (f32, f32),
        p3: (f32, f32),
    ) -> Vec<(f32, f32)> {
        (0..=CURVE_STEPS)
            .map(|i| {
                let t = i as f32 / CURVE_STEPS as f32;
                let u = 1.0 - t;
                let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                (
                    a * p0.0 + b * p1.0 + c * p2.0 + d * p3.0,
                    a * p0.1 + b * p1.1 + c * p2.1 + d * p3.1,
                )
            })
            .collect()
    }

    /// Quadratic Bezier points, flattened.
    pub fn quad_points(p0: (f32, f32), p1: (f32, f32), p2: (f32, f32)) -> Vec<(f32, f32)> {
        (0..=CURVE_STEPS)
            .map(|i| {
                let t = i as f32 / CURVE_STEPS as f32;
                let u = 1.0 - t;
                (
                    u * u * p0.0 + 2.0 * u * t * p1.0 + t * t * p2.0,
                    u * u * p0.1 + 2.0 * u * t * p1.1 + t * t * p2.1,
                )
            })
            .collect()
    }

    /// Text at an SVG baseline: `x` per `align`, `baseline_y` the glyph baseline.
    pub fn text(
        &mut self,
        x: f32,
        baseline_y: f32,
        text: &str,
        size: f32,
        color: Rgba,
        align: TextAlign,
        weight: u16,
    ) {
        self.text_middle(
            x,
            baseline_y - size * BASELINE_TO_MIDDLE,
            text,
            size,
            color,
            align,
            weight,
        );
    }

    /// Text centered vertically on `middle_y`.
    pub fn text_middle(
        &mut self,
        x: f32,
        middle_y: f32,
        text: &str,
        size: f32,
        color: Rgba,
        align: TextAlign,
        weight: u16,
    ) {
        self.text_face(x, middle_y, text, size, color, align, weight, false);
    }

    /// [`Self::text_middle`] in the monospace face (`font-family: mono` on the web).
    pub fn mono_text_middle(
        &mut self,
        x: f32,
        middle_y: f32,
        text: &str,
        size: f32,
        color: Rgba,
        align: TextAlign,
        weight: u16,
    ) {
        self.text_face(x, middle_y, text, size, color, align, weight, true);
    }

    fn text_face(
        &mut self,
        x: f32,
        middle_y: f32,
        text: &str,
        size: f32,
        color: Rgba,
        align: TextAlign,
        weight: u16,
        mono: bool,
    ) {
        if text.is_empty() {
            return;
        }
        let (px, py) = self.p(x, middle_y);
        self.cmds.push(DrawCommand::Text {
            text: text.to_string(),
            pos: Vec2 { x: px, y: py },
            size: size * self.xf.s,
            color,
            align,
            weight,
            halo: None,
            mono,
        });
    }

    /// Draw with `draw` and animate what it drew about `origin` (scene units). The commands are the
    /// rest frame; with the theme's motion off nothing is declared and only that frame draws.
    pub fn animated(&mut self, origin: (f32, f32), motion: Motion, draw: impl FnOnce(&mut Self)) {
        let first = self.cmds.len();
        draw(self);
        let end = self.cmds.len();
        if !self.theme.motion || end == first {
            return;
        }
        let (x, y) = self.p(origin.0, origin.1);
        let s = self.xf.s;
        self.animations.push(Animation {
            first,
            end,
            duration_ms: motion.duration_ms,
            delay_ms: motion.delay_ms,
            iterations: motion.iterations,
            easing: motion.easing,
            origin: Vec2 { x, y },
            base_rotation: motion.base_rotation,
            keyframes: motion
                .keyframes
                .into_iter()
                .map(|k| Keyframe {
                    dash_offset: k.dash_offset.map(|d| d * s),
                    ..k
                })
                .collect(),
        });
    }

    pub fn push(&mut self, cmd: DrawCommand) {
        self.cmds.push(cmd);
    }

    /// A tappable region given in scene units.
    pub fn hit(&mut self, x: f32, y: f32, w: f32, h: f32, emits: Vec<Emit>) {
        if emits.is_empty() {
            return;
        }
        let (px, py) = self.p(x, y);
        self.hits.push(Hit {
            rect: Rect {
                x: px,
                y: py,
                w: w * self.xf.s,
                h: h * self.xf.s,
            },
            emits,
        });
    }

    pub fn finish(self, pattern: &str, width: f32, height: f32) -> MappedScene {
        MappedScene {
            packet: GameRenderPacket {
                pattern: pattern.into(),
                width,
                height,
                draw_commands: self.cmds,
                animations: self.animations,
                ..Default::default()
            },
            hits: self.hits,
            drags: self.drags,
            ..MappedScene::default()
        }
    }
}

/// A CSS keyframe animation in scene units: what the web component declares, mirrored.
#[derive(Debug, Clone)]
pub struct Motion {
    pub duration_ms: f32,
    pub delay_ms: f32,
    /// Absent is `infinite`.
    pub iterations: Option<u32>,
    pub easing: Easing,
    /// The rotation (degrees) the rest frame is drawn at, which a keyframe `rotate` replaces.
    pub base_rotation: f32,
    pub keyframes: Vec<Keyframe>,
}

impl Motion {
    pub fn new(duration_ms: f32, easing: Easing, keyframes: Vec<Keyframe>) -> Self {
        Self {
            duration_ms,
            delay_ms: 0.0,
            iterations: None,
            easing,
            base_rotation: 0.0,
            keyframes,
        }
    }

    pub fn delayed(mut self, ms: f32) -> Self {
        self.delay_ms = ms;
        self
    }

    pub fn from_rotation(mut self, degrees: f32) -> Self {
        self.base_rotation = degrees;
        self
    }
}

/// Lay a `width` x `height` scene out in the viewport and hand back the pen's transform plus
/// the packet size.
pub fn frame(width: f32, height: f32, viewport: Option<(f32, f32)>) -> (Xf, f32, f32) {
    fit(width.max(1.0), height.max(1.0), viewport)
}

impl Pen<'_> {
    /// A radial fade: concentric ellipses whose accumulated alpha follows `stops` (`(t, alpha)` with
    /// `t` 0 at the centre and 1 at the rim, alpha linear between stops). The vector layer has no
    /// gradients, so a CSS `radial-gradient` is drawn as `STEPS` stacked discs. `focus` shifts the
    /// inner discs toward a focal point, as a fraction of the radii (CSS `circle at 30% 30%`).
    pub fn radial_fade(
        &mut self,
        cx: f32,
        cy: f32,
        rx: f32,
        ry: f32,
        rotation: f32,
        color: Rgba,
        stops: &[(f32, f32)],
        focus: (f32, f32),
    ) {
        const STEPS: usize = 14;
        let target = |t: f32| -> f32 {
            let mut prev = stops[0];
            if t <= prev.0 {
                return prev.1;
            }
            for &s in &stops[1..] {
                if t <= s.0 {
                    let k = (t - prev.0) / (s.0 - prev.0).max(f32::EPSILON);
                    return prev.1 + (s.1 - prev.1) * k;
                }
                prev = s;
            }
            prev.1
        };
        let (sin, cos) = rotation.to_radians().sin_cos();
        let mut cumulative = 0.0f32;
        for i in 0..STEPS {
            let t = 1.0 - i as f32 / STEPS as f32;
            let want = target(t - 0.5 / STEPS as f32).clamp(0.0, 0.999);
            let a = (1.0 - (1.0 - want) / (1.0 - cumulative)).max(0.0);
            if a <= 0.0 {
                continue;
            }
            cumulative = 1.0 - (1.0 - cumulative) * (1.0 - a);
            let (fx, fy) = (focus.0 * rx * (1.0 - t), focus.1 * ry * (1.0 - t));
            let (ox, oy) = (fx * cos - fy * sin, fx * sin + fy * cos);
            self.ellipse(
                cx + ox,
                cy + oy,
                rx * t,
                ry * t,
                rotation,
                Some(with_alpha(color, a * color[3])),
                None,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mappers::learning::Xf;

    fn spin() -> Motion {
        Motion::new(
            1000.0,
            Easing::Linear,
            vec![
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
        )
    }

    #[test]
    fn animated_records_the_range_it_drew_in_packet_units() {
        let theme = DiagramTheme::heritage(false);
        let xf = Xf {
            s: 2.0,
            ox: 10.0,
            oy: 20.0,
        };
        let mut pen = Pen::new(&theme, xf);
        pen.circle(1.0, 1.0, 1.0, None, Some(&Ink::new([1.0; 4], 1.0)));
        pen.animated((5.0, 5.0), spin(), |p| {
            p.circle(5.0, 5.0, 3.0, None, Some(&Ink::new([1.0; 4], 1.0)));
            p.circle(6.0, 6.0, 3.0, None, Some(&Ink::new([1.0; 4], 1.0)));
        });
        let scene = pen.finish("t", 100.0, 100.0);
        let a = &scene.packet.animations[0];
        assert_eq!(
            (a.first, a.end),
            (1, 3),
            "after the one circle drawn before it"
        );
        assert_eq!(
            (a.origin.x, a.origin.y),
            (20.0, 30.0),
            "(5,5) through the 2x transform plus offset"
        );
        assert_eq!(
            a.keyframes[1].dash_offset,
            Some(-28.0),
            "dash phases scale with the scene"
        );
    }

    #[test]
    fn nothing_drawn_or_motion_off_declares_nothing_but_still_draws() {
        let theme = DiagramTheme::heritage(false);
        let mut pen = Pen::new(&theme, Xf::identity());
        pen.animated((0.0, 0.0), spin(), |_| {});
        assert!(
            pen.animations.is_empty(),
            "an empty range has nothing to animate"
        );

        let mut still = DiagramTheme::heritage(false);
        still.motion = false;
        let mut pen = Pen::new(&still, Xf::identity());
        pen.animated((0.0, 0.0), spin(), |p| {
            p.circle(1.0, 1.0, 1.0, None, Some(&Ink::new([1.0; 4], 1.0)))
        });
        assert!(pen.animations.is_empty());
        assert_eq!(pen.cmds.len(), 1, "the rest frame is drawn");
    }
}
