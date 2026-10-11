//! The decorative `svg-*` atoms (`@almadar/ui` `components/core/atoms/svg/*`): each draws one
//! small shape set in its own `width` x `height` viewBox. The motion classes the web atoms name
//! (`themes/_base.css`: `almadar-svg-flow-dash`, `-pulse-ring`, `-pulse-expand`) are declared as
//! animations the host clock plays; each rest frame is the atom's static look, which is what the
//! web draws with motion off.

use super::draw::{frame, with_alpha, Ink, Motion, Pen};
use super::{props_f32, props_str};
use crate::mappers::game_canvas_2d::CanvasSceneError;
use crate::mappers::learning::canvas::theme_for;
use crate::mappers::scene::MappedScene;
use crate::packet::{Easing, Keyframe, TextAlign};
use serde_json::Value;
use std::f32::consts::{FRAC_PI_2, TAU};

pub const ATOMS: &[&str] = &[
    "svg-branch",
    "svg-connection",
    "svg-flow",
    "svg-grid",
    "svg-lobe",
    "svg-mesh",
    "svg-morph",
    "svg-node",
    "svg-pulse",
    "svg-ring",
    "svg-shield",
    "svg-stack",
];

const LABEL_PX: f32 = 11.0;

/// The viewBox an atom draws in.
fn extent(pattern: &str, props: &Value) -> (f32, f32) {
    let (dw, dh) = match pattern {
        "svg-mesh" => (120.0, 120.0),
        "svg-morph" => (130.0, 50.0),
        "svg-pulse" => (140.0, 140.0),
        "svg-stack" => (90.0, 80.0),
        _ => (100.0, 100.0),
    };
    if pattern == "svg-stack" {
        return (
            props_f32(props, "svgWidth", dw),
            props_f32(props, "svgHeight", dh),
        );
    }
    (
        props_f32(props, "width", dw),
        props_f32(props, "height", dh),
    )
}

pub fn map(
    pattern: &str,
    props: &Value,
    viewport: Option<(f32, f32)>,
) -> Result<MappedScene, CanvasSceneError> {
    let theme = theme_for(props);
    let (w, h) = extent(pattern, props);
    let (xf, pw, ph) = frame(w, h, viewport);
    let mut pen = Pen::new(&theme, xf);
    pen.clear(theme.ground);
    let color = pen.paint(props_str(props, "color"), "primary");
    let g = props_f32(props, "opacity", 1.0);
    let c = with_alpha(color, g);
    match pattern {
        "svg-branch" => branch(&mut pen, props, c),
        "svg-connection" => connection(&mut pen, props, c),
        "svg-flow" => flow(&mut pen, props, c),
        "svg-grid" => grid(&mut pen, props, c),
        "svg-lobe" => lobe(&mut pen, props, c),
        "svg-mesh" => mesh(&mut pen, props, c),
        "svg-morph" => morph(&mut pen, props, c),
        "svg-node" => node(&mut pen, props, c),
        "svg-pulse" => pulse(&mut pen, props, c),
        "svg-ring" => ring(&mut pen, props, c),
        "svg-shield" => shield(&mut pen, props, c),
        "svg-stack" => stack(&mut pen, props, c),
        other => unreachable!("svg_atoms::ATOMS admitted `{other}` with no drawer"),
    }
    Ok(pen.finish(pattern, pw, ph))
}

fn branch(pen: &mut Pen, p: &Value, c: [f32; 4]) {
    let x = props_f32(p, "x", 5.0);
    let y = props_f32(p, "y", 50.0);
    let size = props_f32(p, "size", 1.0);
    let branches = props_f32(p, "branches", 2.0).max(1.0) as usize;
    let variant = props_str(p, "variant").unwrap_or("fork");
    let ink = Ink::new(c, 2.0);
    if variant == "diamond" {
        let (w, h) = (20.0 * size, 14.0 * size);
        let pts = [
            (x + w, y),
            (x + w * 2.0, y + h),
            (x + w, y + h * 2.0),
            (x, y + h),
        ];
        pen.polyline(&pts, true, None, Some(&ink));
        return;
    }
    let in_len = 30.0 * size;
    let out_len = 30.0 * size;
    let spread = 20.0 * size;
    let jx = x + in_len;
    let frac = |i: usize| {
        if branches == 1 {
            0.0
        } else {
            (i as f32 / (branches - 1) as f32) * 2.0 - 1.0
        }
    };
    if variant == "fork" {
        pen.line((x, y), (jx, y), &ink);
        for i in 0..branches {
            let end_y = y + frac(i) * spread;
            let cp = jx + out_len * 0.5;
            let pts = Pen::cubic_points((jx, y), (cp, y), (cp, end_y), (jx + out_len, end_y));
            pen.polyline(&pts, false, None, Some(&ink));
        }
    } else {
        for i in 0..branches {
            let start_y = y + frac(i) * spread;
            let cp = x + in_len * 0.5;
            let pts = Pen::cubic_points((x, start_y), (cp, start_y), (cp, y), (jx, y));
            pen.polyline(&pts, false, None, Some(&ink));
        }
        pen.line((jx, y), (jx + out_len, y), &ink);
    }
    pen.circle(jx, y, 3.0, Some(c), None);
}

fn connection(pen: &mut Pen, p: &Value, c: [f32; 4]) {
    let variant = props_str(p, "variant").unwrap_or("solid");
    let mut ink = Ink::new(c, props_f32(p, "strokeWidth", 1.5));
    if variant != "solid" {
        ink = ink.dashed(&[8.0, 6.0]);
    }
    let (a, b) = (
        (props_f32(p, "x1", 10.0), props_f32(p, "y1", 50.0)),
        (props_f32(p, "x2", 90.0), props_f32(p, "y2", 50.0)),
    );
    if variant == "animated" {
        pen.animated(a, flow_dash(), |pen| pen.line(a, b, &ink));
    } else {
        pen.line(a, b, &ink);
    }
}

/// `almadar-svg-flow-dash`: one `8 6` dash period per second, linear.
fn flow_dash() -> Motion {
    Motion::new(
        1000.0,
        Easing::Linear,
        vec![
            Keyframe {
                offset: 0.0,
                dash_offset: Some(14.0),
                ..Keyframe::default()
            },
            Keyframe {
                offset: 1.0,
                dash_offset: Some(0.0),
                ..Keyframe::default()
            },
        ],
    )
}

fn flow_points(p: &Value) -> Vec<(f32, f32)> {
    match p.get("points").and_then(Value::as_array) {
        Some(items) => items
            .iter()
            .filter_map(|pt| {
                let pair = pt.as_array()?;
                Some((
                    pair.first()?.as_f64()? as f32,
                    pair.get(1)?.as_f64()? as f32,
                ))
            })
            .collect(),
        None => vec![(10.0, 50.0), (50.0, 20.0), (90.0, 50.0)],
    }
}

fn flow(pen: &mut Pen, p: &Value, c: [f32; 4]) {
    let pts = flow_points(p);
    if pts.len() < 2 {
        return;
    }
    let sw = props_f32(p, "strokeWidth", 1.5);
    if p.get("animated").and_then(Value::as_bool).unwrap_or(false) {
        let ink = Ink::new(c, sw).dashed(&[8.0, 6.0]);
        pen.animated(pts[0], flow_dash(), |pen| {
            pen.polyline(&pts, false, None, Some(&ink))
        });
    } else {
        pen.polyline(&pts, false, None, Some(&Ink::new(c, sw)));
    }
    let (a, b) = (pts[pts.len() - 2], pts[pts.len() - 1]);
    let theta = (b.1 - a.1).atan2(b.0 - a.0);
    let (sin, cos) = theta.sin_cos();
    let at = |mx: f32, my: f32| {
        let (dx, dy) = ((mx - 7.0) * sw, (my - 3.0) * sw);
        (b.0 + dx * cos - dy * sin, b.1 + dx * sin + dy * cos)
    };
    pen.polyline(
        &[at(0.0, 0.0), at(8.0, 3.0), at(0.0, 6.0)],
        true,
        Some(c),
        None,
    );
}

fn grid(pen: &mut Pen, p: &Value, c: [f32; 4]) {
    let (x, y) = (props_f32(p, "x", 10.0), props_f32(p, "y", 10.0));
    let cols = props_f32(p, "cols", 4.0).max(0.0) as usize;
    let rows = props_f32(p, "rows", 3.0).max(0.0) as usize;
    let spacing = props_f32(p, "spacing", 20.0);
    let radius = props_f32(p, "nodeRadius", 3.0);
    let highlights: Vec<usize> = p
        .get("highlights")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_u64().map(|n| n as usize))
                .collect()
        })
        .unwrap_or_default();
    for row in 0..rows {
        for col in 0..cols {
            let hot = highlights.contains(&(row * cols + col));
            let fill = if hot { c } else { with_alpha(c, 0.4) };
            let r = if hot { radius * 1.6 } else { radius };
            pen.circle(
                x + col as f32 * spacing,
                y + row as f32 * spacing,
                r,
                Some(fill),
                None,
            );
        }
    }
}

fn lobe(pen: &mut Pen, p: &Value, c: [f32; 4]) {
    let (cx, cy) = (props_f32(p, "cx", 50.0), props_f32(p, "cy", 50.0));
    let (rx, ry) = (props_f32(p, "rx", 14.0), props_f32(p, "ry", 20.0));
    let rotation = props_f32(p, "rotation", 0.0);
    let shells = props_f32(p, "shells", 2.0).clamp(1.0, 3.0) as usize;
    let (sin, cos) = rotation.to_radians().sin_cos();
    let rot = |x: f32, y: f32| {
        (
            (x - cx) * cos - (y - cy) * sin + cx,
            (x - cx) * sin + (y - cy) * cos + cy,
        )
    };
    for i in 0..shells {
        let scale = if i == 0 { 1.0 } else { 1.0 - i as f32 * 0.3 };
        let col = if i == 0 { c } else { with_alpha(c, 0.5) };
        let (srx, sry) = (rx * scale, ry * scale);
        let ink = Ink::new(col, 1.5);
        for dir in [-1.0f32, 1.0] {
            let (ex, ey) = rot(cx, cy + dir * sry);
            pen.ellipse(ex, ey, srx, sry, rotation, None, Some(&ink));
        }
    }
}

fn mesh(pen: &mut Pen, p: &Value, c: [f32; 4]) {
    let (cx, cy) = (props_f32(p, "cx", 60.0), props_f32(p, "cy", 60.0));
    let count = props_f32(p, "nodes", 6.0).max(0.0) as usize;
    let radius = props_f32(p, "radius", 50.0);
    let density = props_f32(p, "connectionDensity", 0.5).clamp(0.0, 1.0);
    let pos: Vec<(f32, f32)> = (0..count)
        .map(|i| {
            let a = i as f32 / count as f32 * TAU - FRAC_PI_2;
            (cx + a.cos() * radius, cy + a.sin() * radius)
        })
        .collect();
    let mut pairs = Vec::new();
    for i in 0..count {
        for j in i + 1..count {
            pairs.push((i, j));
        }
    }
    let take = (pairs.len() as f32 * density).round() as usize;
    let ink = Ink::new(with_alpha(c, 0.3), 1.0);
    for (a, b) in pairs.into_iter().take(take) {
        pen.line(pos[a], pos[b], &ink);
    }
    for (x, y) in pos {
        pen.circle(x, y, 4.0, Some(c), None);
    }
}

fn node(pen: &mut Pen, p: &Value, c: [f32; 4]) {
    let (x, y, r) = (
        props_f32(p, "x", 50.0),
        props_f32(p, "y", 50.0),
        props_f32(p, "r", 6.0),
    );
    let variant = props_str(p, "variant").unwrap_or("filled");
    if variant == "pulse" {
        // `almadar-svg-pulse-ring`: opacity 0.3 -> 0 (the ring's own 0.3 is the rest frame).
        let ring = Motion::new(
            2000.0,
            Easing::EaseOut,
            vec![
                Keyframe {
                    offset: 0.0,
                    scale: Some(0.6),
                    opacity: Some(1.0),
                    ..Keyframe::default()
                },
                Keyframe {
                    offset: 1.0,
                    scale: Some(1.0),
                    opacity: Some(0.0),
                    ..Keyframe::default()
                },
            ],
        );
        pen.animated((x, y), ring, |pen| {
            pen.circle(
                x,
                y,
                r * 2.0,
                None,
                Some(&Ink::new(with_alpha(c, 0.3), 1.0)),
            );
        });
    }
    if variant == "stroked" {
        pen.circle(x, y, r, None, Some(&Ink::new(c, 2.0)));
    } else {
        pen.circle(x, y, r, Some(c), None);
    }
    if let Some(label) = props_str(p, "label").filter(|l| !l.is_empty()) {
        pen.text(x, y + r + 14.0, label, LABEL_PX, c, TextAlign::Center, 400);
    }
}

fn pulse(pen: &mut Pen, p: &Value, c: [f32; 4]) {
    let (cx, cy) = (props_f32(p, "cx", 70.0), props_f32(p, "cy", 70.0));
    let rings = props_f32(p, "rings", 3.0).max(0.0) as usize;
    let max_r = props_f32(p, "maxRadius", 60.0);
    let animated = p.get("animated").and_then(Value::as_bool).unwrap_or(true);
    for i in 0..rings {
        let r = (i + 1) as f32 / rings as f32 * max_r;
        let ring_opacity = 1.0 - (i as f32 / rings as f32) * 0.6;
        let rest = with_alpha(c, ring_opacity * 0.4);
        if animated {
            // `almadar-svg-pulse-expand` sets opacity 0.4 -> 0 outright; the rest frame keeps the
            // static ring's `ringOpacity * 0.4`, so the multiplier starts at `1 / ringOpacity`.
            let expand = Motion::new(
                2000.0,
                Easing::EaseOut,
                vec![
                    Keyframe {
                        offset: 0.0,
                        scale: Some(0.3),
                        opacity: Some(1.0 / ring_opacity),
                        ..Keyframe::default()
                    },
                    Keyframe {
                        offset: 1.0,
                        scale: Some(1.0),
                        opacity: Some(0.0),
                        ..Keyframe::default()
                    },
                ],
            )
            .delayed(i as f32 * 600.0);
            pen.animated((cx, cy), expand, |pen| {
                pen.circle(cx, cy, r, None, Some(&Ink::new(rest, 1.5)));
            });
        } else {
            pen.circle(cx, cy, r, None, Some(&Ink::new(rest, 1.5)));
        }
    }
    pen.circle(cx, cy, 3.0, Some(c), None);
}

fn ring(pen: &mut Pen, p: &Value, c: [f32; 4]) {
    let (cx, cy, r) = (
        props_f32(p, "cx", 50.0),
        props_f32(p, "cy", 50.0),
        props_f32(p, "r", 40.0),
    );
    let variant = props_str(p, "variant").unwrap_or("solid");
    if variant == "glow" {
        const STEPS: usize = 12;
        for i in 0..STEPS {
            let t = (i as f32 + 0.5) / STEPS as f32;
            pen.circle(
                cx,
                cy,
                r * (1.0 - i as f32 / STEPS as f32),
                Some(with_alpha(c, 0.15 * (1.0 - t) / (STEPS as f32 * 0.5))),
                None,
            );
        }
    }
    let mut ink = Ink::new(c, props_f32(p, "strokeWidth", 1.5));
    if variant == "dashed" {
        ink = ink.dashed(&[6.0, 4.0]);
    }
    pen.circle(cx, cy, r, None, Some(&ink));
    if let Some(label) = props_str(p, "label").filter(|l| !l.is_empty()) {
        pen.text(cx, cy - r - 6.0, label, LABEL_PX, c, TextAlign::Center, 400);
    }
}

const SHIELD: [[(f32, f32); 3]; 6] = [
    [(15.0, 2.0), (5.0, 5.0), (2.0, 6.0)],
    [(2.0, 6.0), (2.0, 18.0), (5.0, 24.0)],
    [(8.0, 30.0), (15.0, 34.0), (15.0, 34.0)],
    [(15.0, 34.0), (22.0, 30.0), (25.0, 24.0)],
    [(28.0, 18.0), (28.0, 6.0), (28.0, 6.0)],
    [(25.0, 5.0), (15.0, 2.0), (15.0, 2.0)],
];

fn shield(pen: &mut Pen, p: &Value, c: [f32; 4]) {
    let (x, y, size) = (
        props_f32(p, "x", 50.0),
        props_f32(p, "y", 50.0),
        props_f32(p, "size", 1.0),
    );
    let variant = props_str(p, "variant").unwrap_or("outline");
    let map = |(px, py): (f32, f32)| (x - 15.0 * size + px * size, y - 18.0 * size + py * size);
    let mut pts: Vec<(f32, f32)> = Vec::new();
    let mut cur = (15.0, 2.0);
    for seg in SHIELD.iter() {
        let curve = Pen::cubic_points(map(cur), map(seg[0]), map(seg[1]), map(seg[2]));
        pts.extend(curve);
        cur = seg[2];
    }
    let filled = matches!(variant, "filled" | "check");
    pen.polyline(
        &pts,
        true,
        filled.then_some(c),
        Some(&Ink::new(c, 2.0 * size)),
    );
    if variant == "check" {
        let check: Vec<(f32, f32)> = [(10.0, 18.0), (14.0, 22.0), (21.0, 13.0)]
            .into_iter()
            .map(map)
            .collect();
        pen.polyline(
            &check,
            false,
            None,
            Some(&Ink::new(pen.ground(), 2.5 * size)),
        );
    }
}

fn stack(pen: &mut Pen, p: &Value, c: [f32; 4]) {
    let (x, y) = (props_f32(p, "x", 10.0), props_f32(p, "y", 40.0));
    let layers = props_f32(p, "layers", 3.0).clamp(2.0, 4.0) as usize;
    let (w, h) = (props_f32(p, "width", 60.0), props_f32(p, "height", 40.0));
    let labels: Vec<String> = p
        .get("labels")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let ground = pen.ground();
    for i in 0..layers {
        let li = layers - 1 - i;
        let lx = x + li as f32 * 4.0;
        let ly = y - li as f32 * 8.0;
        let lo = 0.3 + (li as f32 / (layers - 1) as f32) * 0.7;
        pen.rrect(
            lx,
            ly,
            w,
            h,
            6.0,
            Some(with_alpha(c, lo)),
            Some(&Ink::new(with_alpha(c, lo * lo * 0.6), 1.0)),
        );
        if let Some(label) = labels.get(li).filter(|l| !l.is_empty()) {
            pen.text(
                lx + w / 2.0,
                ly + h / 2.0 + 4.0,
                label,
                10.0,
                with_alpha(ground, lo),
                TextAlign::Center,
                400,
            );
        }
    }
}

fn arrow(pen: &mut Pen, x1: f32, y: f32, x2: f32, scale: f32, c: [f32; 4]) {
    let head = 4.0 * scale;
    let ink = Ink::new(with_alpha(c, 0.5), 1.5 * scale)
        .dashed(&[3.0 * scale, 3.0 * scale])
        .butt();
    pen.line((x1, y), (x2 - head, y), &ink);
    pen.polyline(
        &[
            (x2, y),
            (x2 - head, y - head / 2.0),
            (x2 - head, y + head / 2.0),
        ],
        true,
        Some(with_alpha(c, 0.6)),
        None,
    );
}

fn text_lines(pen: &mut Pen, x: f32, y: f32, s: f32, c: [f32; 4]) {
    for (i, w) in [28.0f32, 22.0, 26.0].into_iter().enumerate() {
        pen.rrect(
            x,
            y + i as f32 * 8.0 * s,
            w * s,
            3.0 * s,
            1.5 * s,
            Some(with_alpha(c, 0.7 - i as f32 * 0.1)),
            None,
        );
    }
}

fn code_brackets(pen: &mut Pen, x: f32, y: f32, s: f32, c: [f32; 4]) {
    let (h, w) = (20.0 * s, 8.0 * s);
    let ink = Ink::new(c, 2.0 * s);
    pen.polyline(
        &[(x + w, y), (x, y + h / 2.0), (x + w, y + h)],
        false,
        None,
        Some(&ink),
    );
    pen.polyline(
        &[
            (x + w * 2.0, y),
            (x + w * 3.0, y + h / 2.0),
            (x + w * 2.0, y + h),
        ],
        false,
        None,
        Some(&ink),
    );
}

fn app_rect(pen: &mut Pen, x: f32, y: f32, s: f32, c: [f32; 4]) {
    let (w, h) = (22.0 * s, 20.0 * s);
    pen.rrect(x, y, w, h, 4.0 * s, None, Some(&Ink::new(c, 2.0 * s)));
    pen.rrect(
        x + 4.0 * s,
        y + 4.0 * s,
        w - 8.0 * s,
        3.0 * s,
        1.0 * s,
        Some(with_alpha(c, 0.5)),
        None,
    );
    pen.rrect(
        x + 4.0 * s,
        y + 10.0 * s,
        (w - 8.0 * s) * 0.6,
        3.0 * s,
        1.0 * s,
        Some(with_alpha(c, 0.3)),
        None,
    );
}

fn morph(pen: &mut Pen, p: &Value, c: [f32; 4]) {
    let (x, y, s) = (
        props_f32(p, "x", 5.0),
        props_f32(p, "y", 10.0),
        props_f32(p, "size", 1.0),
    );
    let gap = 40.0 * s;
    let mid_y = y + 10.0 * s;
    match props_str(p, "variant").unwrap_or("generic") {
        "text-to-code" => {
            let left_end = x + 30.0 * s;
            text_lines(pen, x, y, s, c);
            arrow(pen, left_end, mid_y, left_end + gap, s, c);
            code_brackets(pen, left_end + gap, y, s, c);
        }
        "code-to-app" => {
            let left_end = x + 26.0 * s;
            code_brackets(pen, x, y, s, c);
            arrow(pen, left_end, mid_y, left_end + gap, s, c);
            app_rect(pen, left_end + gap, y, s, c);
        }
        _ => {
            let r = 10.0 * s;
            let square_start = x + r * 2.0 + gap;
            pen.circle(x + r, mid_y, r, None, Some(&Ink::new(c, 2.0 * s)));
            arrow(pen, x + r + r + 2.0 * s, mid_y, square_start, s, c);
            pen.rrect(
                square_start,
                mid_y - r,
                r * 2.0,
                r * 2.0,
                3.0 * s,
                None,
                Some(&Ink::new(c, 2.0 * s)),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::{DrawCommand, PathSegment};
    use serde_json::json;

    fn cmds(pattern: &str, props: Value) -> Vec<DrawCommand> {
        let mut p = props;
        p["type"] = json!(pattern);
        map(pattern, &p, None).unwrap().packet.draw_commands
    }

    type PathView<'a> = (
        &'a Vec<PathSegment>,
        Option<[f32; 4]>,
        Option<&'a crate::packet::Stroke>,
    );

    fn paths(c: &[DrawCommand]) -> Vec<PathView<'_>> {
        c.iter()
            .filter_map(|c| match c {
                DrawCommand::Path {
                    segments,
                    fill,
                    stroke,
                } => Some((segments, *fill, stroke.as_ref())),
                _ => None,
            })
            .collect()
    }

    fn primary() -> [f32; 4] {
        theme_for(&json!({})).tone("primary").unwrap()
    }

    #[test]
    fn the_color_prop_resolves_a_var_token_a_literal_or_defaults_to_primary() {
        let stroke_of = |props: Value| {
            paths(&cmds("svg-ring", props))[0]
                .2
                .map(|s| s.color)
                .unwrap()
        };
        assert_eq!(
            stroke_of(json!({})),
            primary(),
            "default is the primary tone"
        );
        assert_eq!(
            stroke_of(json!({"color": "var(--color-success)"})),
            theme_for(&json!({})).tone("success").unwrap()
        );
        assert_eq!(
            stroke_of(json!({"color": "#336699"})),
            crate::mappers::game_canvas_2d::parse_css_color("#336699").unwrap()
        );
        assert_eq!(
            stroke_of(json!({"opacity": 0.5}))[3],
            0.5,
            "the group opacity scales the alpha"
        );
    }

    #[test]
    fn ring_variants_solid_dashed_and_glow_differ_and_a_label_sits_above() {
        let solid = cmds("svg-ring", json!({"r": 30}));
        let dashed = cmds("svg-ring", json!({"r": 30, "variant": "dashed"}));
        let glow = cmds(
            "svg-ring",
            json!({"r": 30, "variant": "glow", "label": "core"}),
        );
        assert!(paths(&solid)[0].2.unwrap().dash.is_empty());
        assert_eq!(paths(&dashed)[0].2.unwrap().dash, vec![6.0, 4.0]);
        assert!(
            glow.len() > solid.len() + 5,
            "the glow is a stack of fading discs"
        );
        let label = glow
            .iter()
            .find_map(|c| match c {
                DrawCommand::Text { text, pos, .. } => Some((text.clone(), pos.y)),
                _ => None,
            })
            .unwrap();
        assert_eq!(label.0, "core");
        assert!(label.1 < 50.0 - 30.0, "above the ring top");
        assert!(
            !solid.iter().any(|c| matches!(c, DrawCommand::Text { .. })),
            "control: no label, no text"
        );
    }

    #[test]
    fn branch_fork_and_merge_draw_one_curve_per_branch_and_a_junction_and_diamond_is_a_closed_quad()
    {
        let fork = cmds("svg-branch", json!({"branches": 3}));
        let curves = paths(&fork)
            .iter()
            .filter(|p| p.2.is_some() && p.1.is_none() && p.0.len() > 4)
            .count();
        assert_eq!(curves, 3);
        assert_eq!(
            paths(&fork).iter().filter(|p| p.1.is_some()).count(),
            1,
            "one junction dot"
        );
        let one = cmds("svg-branch", json!({"branches": 1}));
        assert_eq!(
            paths(&one).iter().filter(|p| p.0.len() > 4).count(),
            1,
            "a single branch is one curve"
        );
        let diamond = cmds("svg-branch", json!({"variant": "diamond"}));
        let quad = paths(&diamond)
            .into_iter()
            .find(|p| p.0.len() == 5)
            .unwrap();
        assert!(matches!(quad.0[4], PathSegment::Close));
        assert!(quad.1.is_none(), "an outline");
        let merge = cmds("svg-branch", json!({"variant": "merge", "branches": 2}));
        let first = paths(&merge)[0].0[0];
        assert!(
            matches!(first, PathSegment::MoveTo { x, .. } if (x - 5.0).abs() < 0.01),
            "a merge starts at x"
        );
    }

    #[test]
    fn connection_and_flow_dash_when_dashed_or_animated_and_flow_ends_in_an_arrow_head() {
        assert!(paths(&cmds("svg-connection", json!({})))[0]
            .2
            .unwrap()
            .dash
            .is_empty());
        for v in ["dashed", "animated"] {
            assert_eq!(
                paths(&cmds("svg-connection", json!({"variant": v})))[0]
                    .2
                    .unwrap()
                    .dash,
                vec![8.0, 6.0],
                "{v}"
            );
        }
        let flow = cmds("svg-flow", json!({}));
        let p = paths(&flow);
        assert_eq!(p.len(), 2, "the polyline and its head");
        assert!(p[1].1.is_some() && p[1].2.is_none(), "a filled arrow head");
        assert!(
            paths(&cmds("svg-flow", json!({"animated": true})))[0]
                .2
                .unwrap()
                .dash
                == vec![8.0, 6.0]
        );
        assert!(
            paths(&cmds("svg-flow", json!({"points": [[1, 2]]}))).is_empty(),
            "fewer than two points draws nothing"
        );
    }

    #[test]
    fn grid_makes_rows_by_cols_dots_and_highlights_are_bigger_and_opaque() {
        let g = cmds("svg-grid", json!({"cols": 3, "rows": 2, "highlights": [4]}));
        let dots: Vec<_> = paths(&g)
            .into_iter()
            .map(|(s, f, _)| (s, f.unwrap()))
            .collect();
        assert_eq!(dots.len(), 6);
        let radius = |s: &Vec<PathSegment>| match s[0] {
            PathSegment::Arc { rx, .. } => rx,
            _ => 0.0,
        };
        assert_eq!(radius(dots[4].0), 3.0 * 1.6);
        assert_eq!(dots[4].1[3], 1.0);
        assert_eq!(radius(dots[0].0), 3.0);
        assert!(
            (dots[0].1[3] - 0.4).abs() < 1e-6,
            "control: the rest sit at 0.4"
        );
    }

    #[test]
    fn lobe_shells_clamp_to_one_to_three_and_mesh_connects_the_density_share_of_pairs() {
        let shells = |n: f64| paths(&cmds("svg-lobe", json!({"shells": n}))).len();
        assert_eq!(
            (shells(0.0), shells(2.0), shells(9.0)),
            (2, 4, 6),
            "two lobes per shell, clamped to 1..3"
        );
        let mesh = cmds("svg-mesh", json!({"nodes": 5, "connectionDensity": 0.5}));
        let p = paths(&mesh);
        let lines = p.iter().filter(|x| x.1.is_none()).count();
        assert_eq!(lines, 5, "round(10 pairs x 0.5)");
        assert_eq!(
            p.iter().filter(|x| x.1.is_some()).count(),
            5,
            "one dot per node"
        );
        let none = paths(&cmds(
            "svg-mesh",
            json!({"nodes": 5, "connectionDensity": 0}),
        ))
        .iter()
        .filter(|x| x.1.is_none())
        .count();
        assert_eq!(none, 0);
    }

    #[test]
    fn node_variants_fill_stroke_or_pulse_and_pulse_rings_follow_the_ring_count() {
        let n = |v: &str| paths(&cmds("svg-node", json!({"variant": v, "label": "A"}))).len();
        assert_eq!((n("filled"), n("stroked"), n("pulse")), (1, 1, 2));
        assert!(paths(&cmds("svg-node", json!({"variant": "stroked"})))[0]
            .1
            .is_none());
        assert!(paths(&cmds("svg-node", json!({})))[0].1.is_some());
        assert_eq!(
            paths(&cmds("svg-pulse", json!({"rings": 4}))).len(),
            5,
            "four rings and the core dot"
        );
        assert_eq!(paths(&cmds("svg-pulse", json!({"rings": 0}))).len(), 1);
    }

    fn scene(ty: &str, props: Value) -> MappedScene {
        let mut props = props;
        props["type"] = json!(ty);
        map(ty, &props, None).unwrap()
    }

    #[test]
    fn an_animated_flow_and_connection_run_the_flow_dash_over_the_line_only() {
        for (ty, props) in [
            ("svg-flow", json!({"animated": true})),
            ("svg-connection", json!({"variant": "animated"})),
        ] {
            let s = scene(ty, props);
            let [a] = s.packet.animations.as_slice() else {
                panic!("{ty}: one animation")
            };
            assert_eq!(
                (a.duration_ms, a.easing, a.end - a.first),
                (1000.0, Easing::Linear, 1),
                "{ty}"
            );
            assert_eq!(
                a.keyframes
                    .iter()
                    .map(|k| k.dash_offset)
                    .collect::<Vec<_>>(),
                vec![Some(14.0), Some(0.0)],
                "{ty}"
            );
        }
    }

    #[test]
    fn control_a_dashed_connection_and_a_plain_flow_do_not_move() {
        assert!(scene("svg-connection", json!({"variant": "dashed"}))
            .packet
            .animations
            .is_empty());
        assert!(scene("svg-flow", json!({})).packet.animations.is_empty());
    }

    #[test]
    fn a_pulse_node_rings_out_from_its_static_look_and_theme_motion_off_keeps_the_static_look() {
        let s = scene("svg-node", json!({"variant": "pulse"}));
        let [a] = s.packet.animations.as_slice() else {
            panic!("one ring animation")
        };
        assert_eq!((a.duration_ms, a.easing), (2000.0, Easing::EaseOut));
        let off = scene(
            "svg-node",
            json!({"variant": "pulse", "canvasTheme": {"motion": {"enabled": false}}}),
        );
        assert!(off.packet.animations.is_empty());
        assert_eq!(
            serde_json::to_string(&off.packet.draw_commands).unwrap(),
            serde_json::to_string(&s.packet.draw_commands).unwrap(),
            "the rest frame is the static ring"
        );
    }

    fn pulse_scene(props: Value) -> MappedScene {
        let mut props = props;
        props["type"] = json!("svg-pulse");
        map("svg-pulse", &props, None).unwrap()
    }

    fn ring_radius(cmds: &[DrawCommand], i: usize) -> f32 {
        match &cmds[i] {
            DrawCommand::Path { segments, .. } => match segments[0] {
                crate::packet::PathSegment::Arc { rx, .. } => rx,
                _ => panic!("arc"),
            },
            _ => panic!("path"),
        }
    }

    #[test]
    fn svg_pulse_declares_one_staggered_ease_out_animation_per_ring() {
        let s = pulse_scene(json!({"rings": 3}));
        assert_eq!(s.packet.animations.len(), 3);
        let delays: Vec<f32> = s.packet.animations.iter().map(|a| a.delay_ms).collect();
        assert_eq!(delays, vec![0.0, 600.0, 1200.0]);
        assert!(s.packet.animations.iter().all(|a| a.duration_ms == 2000.0
            && a.iterations.is_none()
            && a.easing == Easing::EaseOut
            && a.end == a.first + 1));
    }

    #[test]
    fn svg_pulse_frame_at_zero_differs_from_the_frame_at_half_a_period() {
        let s = pulse_scene(json!({"rings": 3}));
        let at0 = s.packet.resolved(0.0, true).into_owned();
        let half = s.packet.resolved(1000.0, true).into_owned();
        let first = s
            .packet
            .draw_commands
            .iter()
            .position(|c| matches!(c, DrawCommand::Path { .. }))
            .unwrap();
        assert_ne!(
            serde_json::to_string(&at0.draw_commands).unwrap(),
            serde_json::to_string(&half.draw_commands).unwrap()
        );
        assert!(ring_radius(&at0.draw_commands, first) < ring_radius(&half.draw_commands, first));
        assert_eq!(
            ring_radius(&at0.draw_commands, first + 1),
            ring_radius(&s.packet.draw_commands, first + 1),
            "control: the second ring is still inside its delay at t=0"
        );
    }

    fn pixels(s: &MappedScene, t: f64, motion: bool) -> Vec<u8> {
        let p = s.packet.resolved(t, motion);
        let vector: Vec<&DrawCommand> = p
            .draw_commands
            .iter()
            .filter(|c| crate::vector::is_vector(c))
            .collect();
        crate::vector::rasterize(&vector, 140, 140, 1.0, 1.0).unwrap()
    }

    #[test]
    fn rasterized_pulse_frames_differ_over_time_a_static_atom_does_not_and_reduced_motion_holds() {
        let pulse = pulse_scene(json!({"rings": 3}));
        assert_ne!(
            pixels(&pulse, 0.0, true),
            pixels(&pulse, 1000.0, true),
            "t=0 vs half a period"
        );
        assert_eq!(
            pixels(&pulse, 0.0, false),
            pixels(&pulse, 1000.0, false),
            "reduced motion: one frame"
        );
        let mut ring = json!({"r": 30, "type": "svg-ring"});
        ring["variant"] = json!("solid");
        let ring = map("svg-ring", &ring, None).unwrap();
        assert_eq!(
            pixels(&ring, 0.0, true),
            pixels(&ring, 1000.0, true),
            "control: a static atom is identical across t"
        );
    }

    #[test]
    fn svg_pulse_reduced_motion_and_unanimated_are_static() {
        let s = pulse_scene(json!({"rings": 3}));
        let rest = serde_json::to_string(&s.packet.draw_commands).unwrap();
        for t in [0.0, 700.0, 1900.0] {
            assert_eq!(
                serde_json::to_string(&s.packet.resolved(t, false).draw_commands).unwrap(),
                rest,
                "reduced motion at {t}"
            );
        }
        let still = pulse_scene(json!({"rings": 3, "animated": false}));
        assert!(
            still.packet.animations.is_empty(),
            "control: animated false declares nothing"
        );
        let off = pulse_scene(json!({"rings": 3, "canvasTheme": {"motion": {"enabled": false}}}));
        assert!(
            off.packet.animations.is_empty(),
            "the theme's motion switch removes it"
        );
    }

    #[test]
    fn shield_outline_filled_and_check_and_stack_layers_clamp() {
        let fills = |v: &str| {
            paths(&cmds("svg-shield", json!({"variant": v})))
                .iter()
                .filter(|p| p.1.is_some())
                .count()
        };
        assert_eq!(
            (fills("outline"), fills("filled"), fills("check")),
            (0, 1, 1)
        );
        assert_eq!(
            paths(&cmds("svg-shield", json!({"variant": "check"}))).len(),
            2,
            "the check mark is a second stroke"
        );
        let layers = |n: f64| paths(&cmds("svg-stack", json!({"layers": n}))).len();
        assert_eq!((layers(1.0), layers(3.0), layers(9.0)), (2, 3, 4));
        let labelled = cmds("svg-stack", json!({"layers": 2, "labels": ["a", "b"]}));
        assert_eq!(
            labelled
                .iter()
                .filter(|c| matches!(c, DrawCommand::Text { .. }))
                .count(),
            2
        );
    }

    #[test]
    fn morph_variants_draw_their_own_glyphs_and_each_atom_reports_its_own_viewbox() {
        let sig = |v: &str| paths(&cmds("svg-morph", json!({"variant": v}))).len();
        let (t2c, c2a, generic) = (sig("text-to-code"), sig("code-to-app"), sig("generic"));
        assert!(t2c != generic && c2a != generic);
        let size = |pattern: &str, props: Value| {
            let mut p = props;
            p["type"] = json!(pattern);
            let s = map(pattern, &p, None).unwrap();
            (s.packet.width, s.packet.height)
        };
        assert_eq!(size("svg-mesh", json!({})), (120.0, 120.0));
        assert_eq!(size("svg-morph", json!({})), (130.0, 50.0));
        assert_eq!(
            size("svg-stack", json!({"width": 60, "height": 40})),
            (90.0, 80.0),
            "a stack's width/height are its layers; svgWidth/svgHeight the viewBox"
        );
        assert_eq!(
            size("svg-ring", json!({"width": 64, "height": 32})),
            (64.0, 32.0)
        );
    }
}
