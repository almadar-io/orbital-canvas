//! `chemistry-canvas` — port of `ChemistryCanvas.tsx` 2D mode: containers, bonds,
//! reaction arrows, atoms (element, charge, lone pairs) and the equation derived
//! into `LearningShape`s in canvas pixels, drawn by [`canvas`]. `mode: "3d"` is a
//! three.js scene and is refused.

use super::canvas::{self, CanvasSpec};
use super::shape::{parse_shapes, Align, Dash, Shape};
use super::theme::DiagramTheme;
use crate::mappers::game_canvas_2d::{get_f32, CanvasSceneError};
use crate::mappers::scene::MappedScene;
use serde_json::Value;
use std::collections::HashMap;
use std::f32::consts::PI;

const LONE_PAIR_ANGLES: [f32; 4] = [-90.0, 0.0, 90.0, 180.0];

fn f(v: &Value, key: &str) -> Option<f32> {
    v.get(key).and_then(Value::as_f64).map(|n| n as f32)
}

fn s<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

fn list<'a>(props: &'a Value, key: &str) -> &'a [Value] {
    props
        .get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn bond_state_color(state: &str) -> &'static str {
    match state {
        "forming" => "series-3",
        "breaking" => "series-2",
        "highlight" => "highlight",
        _ => "ink",
    }
}

/// `elementTones`: a series tone per distinct element, in order of first appearance.
fn element_tones(atoms: &[Value]) -> HashMap<&str, String> {
    let mut tones = HashMap::new();
    for a in atoms {
        if let Some(el) = s(a, "element") {
            let n = tones.len();
            tones
                .entry(el)
                .or_insert_with(|| DiagramTheme::series_tone(n));
        }
    }
    tones
}

fn container_shapes(c: &Value, out: &mut Vec<Shape>) {
    let (x, y) = (f(c, "x").unwrap_or(0.0), f(c, "y").unwrap_or(0.0));
    let (w, h) = (f(c, "width").unwrap_or(0.0), f(c, "height").unwrap_or(0.0));
    let color = s(c, "color").unwrap_or("ink");
    if let Some(lv) = f(c, "level") {
        let level_color = s(c, "levelColor").unwrap_or("series-1");
        out.push(
            Shape::rect(x + 1.0, y + h * (1.0 - lv), w - 2.0, h * lv - 1.0)
                .color(level_color)
                .fill(level_color)
                .fill_style("solid")
                .stroke_weight("thin")
                .opacity(0.5),
        );
    }
    let mut outline = Shape::rect(x, y, w, h).color(color).stroke_weight("normal");
    outline.fill = s(c, "fill").map(str::to_string);
    outline.line_width = f(c, "lineWidth");
    out.push(outline);

    let dash = match s(c, "divider").unwrap_or("none") {
        "dashed" => Some(Some(Dash::Dashed)),
        "dotted" => Some(Some(Dash::Dotted)),
        "none" => None,
        _ => Some(None),
    };
    if let Some(dash) = dash {
        out.push(
            Shape::line(x + w / 2.0, y, x + w / 2.0, y + h)
                .color(s(c, "dividerColor").unwrap_or(color))
                .stroke_weight("thin")
                .dash(dash),
        );
    }
    let label = |tx: f32, ty: f32, text: &str, tone: &str| {
        Shape::text(tx, ty, text)
            .tone(tone)
            .text_size("sm")
            .align(Align::Center)
    };
    if let Some(t) = s(c, "leftLabel").filter(|t| !t.is_empty()) {
        out.push(label(x + w * 0.25, y + 12.0, t, "label"));
    }
    if let Some(t) = s(c, "rightLabel").filter(|t| !t.is_empty()) {
        out.push(label(x + w * 0.75, y + 12.0, t, "label"));
    }
    if let Some(t) = s(c, "label").filter(|t| !t.is_empty()) {
        out.push(label(x + w / 2.0, y + h + 12.0, t, "ink"));
    }
}

fn bond_shapes(b: &Value, ends: ((f32, f32), (f32, f32)), parallel: bool, out: &mut Vec<Shape>) {
    let (a, c) = ends;
    let state = s(b, "state").unwrap_or("default");
    let color = s(b, "color").unwrap_or_else(|| bond_state_color(state));
    let dash = matches!(state, "forming" | "breaking").then_some(Dash::Dashed);
    let order = s(b, "type");
    if parallel {
        let (dx, dy) = (c.0 - a.0, c.1 - a.1);
        let dist = dx.hypot(dy).max(1e-6);
        let (px, py) = (-dy / dist, dx / dist);
        let offsets: &[f32] = match order {
            Some("double") => &[-3.0, 3.0],
            Some("triple") => &[-4.0, 0.0, 4.0],
            _ => &[0.0],
        };
        for off in offsets {
            out.push(
                Shape::line(
                    a.0 + px * off,
                    a.1 + py * off,
                    c.0 + px * off,
                    c.1 + py * off,
                )
                .color(color)
                .stroke_weight("normal")
                .dash(dash),
            );
        }
    } else {
        let width = match order {
            Some("double") => 4.0,
            Some("triple") => 6.0,
            _ => 2.0,
        };
        let mut line = Shape::line(a.0, a.1, c.0, c.1).color(color).dash(dash);
        line.line_width = Some(width);
        out.push(line);
    }
}

fn arrow_shapes(a: &Value, out: &mut Vec<Shape>) {
    let (x, y) = (f(a, "x").unwrap_or(0.0), f(a, "y").unwrap_or(0.0));
    let angle = f(a, "angle").unwrap_or(0.0) * (PI / 180.0);
    let len = f(a, "length").unwrap_or(60.0);
    let (x2, y2) = (x + angle.cos() * len, y + angle.sin() * len);
    out.push(
        Shape::arrow(x, y, x2, y2)
            .color(s(a, "color").unwrap_or("ink"))
            .stroke_weight("bold"),
    );
    if let Some(label) = s(a, "label").filter(|t| !t.is_empty()) {
        out.push(
            Shape::text((x + x2) / 2.0, (y + y2) / 2.0 - 10.0, label)
                .verbatim()
                .tone("ink")
                .text_size("sm")
                .align(Align::Center),
        );
    }
}

fn atom_shapes(a: &Value, tones: &HashMap<&str, String>, out: &mut Vec<Shape>) {
    let (Some(x), Some(y)) = (f(a, "x"), f(a, "y")) else {
        return;
    };
    let r = f(a, "radius").unwrap_or(14.0);
    let tone = s(a, "element")
        .and_then(|el| tones.get(el))
        .map_or("series-1", String::as_str);
    let color = s(a, "color").unwrap_or(tone);
    let mut circle = Shape::circle(x, y, r).color(color).fill(color);
    circle.id = s(a, "id").map(str::to_string);
    out.push(circle);
    if let Some(el) = s(a, "element").filter(|t| !t.is_empty()) {
        out.push(
            Shape::text(x, y, el)
                .verbatim()
                .tone("ink")
                .text_size("sm")
                .align(Align::Center),
        );
    }
    if let Some(charge) = s(a, "charge").filter(|t| !t.is_empty()) {
        out.push(
            Shape::text(x + r * 0.85, y - r * 0.85, charge)
                .verbatim()
                .tone("ink")
                .text_size("xs")
                .align(Align::Left),
        );
    }
    let pairs = f(a, "lonePairs").unwrap_or(0.0).clamp(0.0, 4.0);
    for angle in LONE_PAIR_ANGLES.iter().take(pairs.ceil() as usize) {
        let rad = angle * PI / 180.0;
        let (cx, cy) = (x + (r + 6.0) * rad.cos(), y + (r + 6.0) * rad.sin());
        let (perp_x, perp_y) = (-rad.sin(), rad.cos());
        for sign in [1.0, -1.0] {
            out.push(
                Shape::circle(cx + perp_x * 2.5 * sign, cy + perp_y * 2.5 * sign, 1.5)
                    .color("ink")
                    .fill("ink")
                    .fill_style("solid"),
            );
        }
    }
}

/// The shapes `ChemistryCanvas` derives in 2D mode, in its paint order.
pub fn derive_shapes(props: &Value, width: f32) -> Vec<Shape> {
    let mut out = Vec::new();
    let atoms = list(props, "atoms");
    let tones = element_tones(atoms);
    let mut by_id: HashMap<&str, (f32, f32)> = HashMap::new();
    for a in atoms {
        if let (Some(id), Some(x), Some(y)) =
            (s(a, "id").filter(|i| !i.is_empty()), f(a, "x"), f(a, "y"))
        {
            by_id.insert(id, (x, y));
        }
    }

    for c in list(props, "containers") {
        container_shapes(c, &mut out);
    }
    let parallel = s(props, "bondStyle") == Some("parallel");
    for b in list(props, "bonds") {
        let ends = s(b, "from")
            .zip(s(b, "to"))
            .and_then(|(from, to)| Some((*by_id.get(from)?, *by_id.get(to)?)));
        if let Some(ends) = ends {
            bond_shapes(b, ends, parallel, &mut out);
        }
    }
    for a in list(props, "arrows") {
        arrow_shapes(a, &mut out);
    }
    for a in atoms {
        atom_shapes(a, &tones, &mut out);
    }
    if let Some(eq) = s(props, "equation").filter(|t| !t.is_empty()) {
        let mut text = Shape::text(width / 2.0, 14.0, eq)
            .verbatim()
            .text_size("sm")
            .align(Align::Center);
        text.color = Some(s(props, "equationColor").unwrap_or("ink").to_string());
        out.push(text);
    }
    out.extend(parse_shapes(props.get("shapes")));
    out
}

pub fn map(props: &Value, viewport: Option<(f32, f32)>) -> Result<MappedScene, CanvasSceneError> {
    if let Some(mode) = s(props, "mode").filter(|m| *m == "3d") {
        return Err(CanvasSceneError::UnsupportedMode(mode.into()));
    }
    let width = get_f32(props, "width", 600.0);
    let spec = CanvasSpec::from_props(
        "chemistry-canvas",
        props,
        600.0,
        400.0,
        derive_shapes(props, width),
    );
    Ok(canvas::render(&spec, &canvas::theme_for(props), viewport))
}

#[cfg(test)]
mod tests {
    use super::super::shape::ShapeType;
    use super::*;
    use crate::packet::{DrawCommand, PathSegment};
    use serde_json::json;

    fn water_atoms(w: f64, h: f64) -> Value {
        json!([
            {"id": "o", "x": w * 0.5, "y": h * 0.42, "element": "O", "radius": 18, "color": "series-4", "lonePairs": 2, "charge": "δ-"},
            {"id": "h1", "x": w * 0.38, "y": h * 0.62, "element": "H", "radius": 12, "color": "series-1", "lonePairs": 0, "charge": "δ+"},
            {"id": "h2", "x": w * 0.62, "y": h * 0.62, "element": "H", "radius": 12, "color": "series-1", "lonePairs": 0, "charge": "δ+"}
        ])
    }

    /// `std-chem-molecule` at its config defaults (Water preset, 600x400):
    /// the resolved `chemistry-canvas` props `ChemMoleculeSim` renders.
    fn water() -> Value {
        json!({
            "type": "chemistry-canvas", "width": 600, "height": 400, "backgroundColor": "", "series": [],
            "bondStyle": "parallel", "equation": "H2O", "atoms": water_atoms(600.0, 400.0),
            "bonds": [{"from": "o", "to": "h1", "type": "single"}, {"from": "o", "to": "h2", "type": "single"}]
        })
    }

    /// `std-chem-molecule` Carbon Dioxide preset (two double bonds).
    fn co2(style: &str) -> Value {
        json!({
            "type": "chemistry-canvas", "width": 600, "height": 400, "bondStyle": style, "equation": "CO2",
            "atoms": [
                {"id": "c", "x": 300, "y": 200, "element": "C", "radius": 18, "color": "series-2", "lonePairs": 0, "charge": "δ+"},
                {"id": "o1", "x": 144, "y": 200, "element": "O", "radius": 16, "color": "series-4", "lonePairs": 2, "charge": "δ-"},
                {"id": "o2", "x": 456, "y": 200, "element": "O", "radius": 16, "color": "series-4", "lonePairs": 2, "charge": "δ-"}
            ],
            "bonds": [{"from": "c", "to": "o1", "type": "double"}, {"from": "c", "to": "o2", "type": "double"}]
        })
    }

    /// `std-chem-titration` at defaults, dose 0 (acidVolume 25, capacity 40,
    /// 600x400, indicator `series-1`, titrant `series-3`).
    fn titration() -> Value {
        let level_beaker = 25.0 / 65.0;
        json!({
            "type": "chemistry-canvas", "width": 600, "height": 400,
            "containers": [
                {"x": 72, "y": 120, "width": 168, "height": 240, "divider": "none", "level": level_beaker, "levelColor": "series-1", "label": "Beaker"},
                {"x": 348, "y": 20, "width": 36, "height": 220, "divider": "none", "level": 1, "levelColor": "series-3", "label": "Burette"}
            ],
            "readouts": [{"label": "pH", "value": "1"}, {"label": "mL added", "value": "0"}]
        })
    }

    /// `std-chem-osmosis` container: a full-canvas dashed membrane with the
    /// water/solute count labels in each half.
    fn osmosis() -> Value {
        json!({
            "type": "chemistry-canvas", "width": 600, "height": 400,
            "containers": [{"x": 0, "y": 0, "width": 600, "height": 400, "divider": "dashed", "leftLabel": "W 4", "rightLabel": "W 4 S 6"}]
        })
    }

    fn texts(scene: &MappedScene) -> Vec<(String, (f32, f32), f32, [f32; 4])> {
        scene
            .packet
            .draw_commands
            .iter()
            .filter_map(|c| match c {
                DrawCommand::Text {
                    text,
                    pos,
                    size,
                    color,
                    ..
                } => Some((text.clone(), (pos.x, pos.y), *size, *color)),
                _ => None,
            })
            .collect()
    }

    fn seg_lines(scene: &MappedScene) -> Vec<((f32, f32), (f32, f32), crate::packet::Stroke)> {
        scene
            .packet
            .draw_commands
            .iter()
            .filter_map(|c| match c {
                DrawCommand::Path { segments, fill: None, stroke: Some(s), .. } => match segments.as_slice() {
                    [PathSegment::MoveTo { x: x1, y: y1 }, PathSegment::LineTo { x: x2, y: y2 }] => Some(((*x1, *y1), (*x2, *y2), s.clone())),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    fn count(shapes: &[Shape], ty: ShapeType) -> usize {
        shapes.iter().filter(|s| s.ty == ty).count()
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn water_atoms_become_series_circles_with_element_charge_and_lone_pair_dots() {
        let shapes = derive_shapes(&water(), 600.0);
        let circles: Vec<_> = shapes
            .iter()
            .filter(|s| s.ty == ShapeType::Circle)
            .collect();
        assert_eq!(
            circles.len(),
            3 + 4,
            "three atoms plus two lone pairs of two dots"
        );
        let o = circles[0];
        assert_eq!((o.x, o.y, o.radius), (Some(300.0), Some(168.0), Some(18.0)));
        assert_eq!(
            (o.color.as_deref(), o.fill.as_deref(), o.id.as_deref()),
            (Some("series-4"), Some("series-4"), Some("o"))
        );
        let texts: Vec<_> = shapes.iter().filter(|s| s.ty == ShapeType::Text).collect();
        let strings: Vec<_> = texts.iter().filter_map(|t| t.text.as_deref()).collect();
        assert_eq!(strings, vec!["O", "δ-", "H", "δ+", "H", "δ+", "H2O"]);
        let charge = texts[1];
        assert!(
            near(charge.x.unwrap(), 300.0 + 18.0 * 0.85)
                && near(charge.y.unwrap(), 168.0 - 18.0 * 0.85)
        );
        assert_eq!(
            (charge.align, charge.text_size.as_deref()),
            (Some(Align::Left), Some("xs"))
        );
        let eq = texts[6];
        assert_eq!(
            (eq.x, eq.y, eq.align, eq.color.as_deref()),
            (Some(300.0), Some(14.0), Some(Align::Center), Some("ink"))
        );
    }

    #[test]
    fn lone_pair_dots_sit_in_compass_pairs_around_the_atom() {
        let shapes = derive_shapes(&water(), 600.0);
        let dots: Vec<_> = shapes
            .iter()
            .filter(|s| s.ty == ShapeType::Circle && s.radius == Some(1.5))
            .collect();
        assert_eq!(dots.len(), 4);
        assert!(
            dots.iter()
                .all(|d| d.color.as_deref() == Some("ink")
                    && d.fill_style.as_deref() == Some("solid"))
        );
        // first pair at -90 deg: centre (300, 168-24), split +-2.5 along x
        assert!(near(dots[0].x.unwrap(), 302.5) && near(dots[0].y.unwrap(), 144.0));
        assert!(near(dots[1].x.unwrap(), 297.5) && near(dots[1].y.unwrap(), 144.0));
        // second pair at 0 deg: centre (324, 168), split along y
        assert!(near(dots[2].x.unwrap(), 324.0) && near(dots[2].y.unwrap(), 170.5));
        assert!(near(dots[3].y.unwrap(), 165.5));
    }

    #[test]
    fn lone_pairs_clamp_to_zero_through_four() {
        let dots = |lp: i32| {
            let mut p = water();
            p["atoms"][0]["lonePairs"] = json!(lp);
            derive_shapes(&p, 600.0)
                .iter()
                .filter(|s| s.radius == Some(1.5))
                .count()
        };
        assert_eq!(dots(7), 8);
        assert_eq!(dots(-3), 0);
        assert_eq!(dots(0), 0);
        assert_eq!(dots(2), 4);
    }

    #[test]
    fn parallel_bond_style_offsets_strokes_by_order_and_thick_widens_one_stroke() {
        let parallel = derive_shapes(&co2("parallel"), 600.0);
        let lines: Vec<_> = parallel
            .iter()
            .filter(|s| s.ty == ShapeType::Line)
            .collect();
        assert_eq!(lines.len(), 4, "two double bonds, two strokes each");
        assert!(lines
            .iter()
            .all(|l| l.stroke.as_deref() == Some("normal") && l.line_width.is_none()));
        assert!(near(lines[0].y1.unwrap(), 200.0 - 3.0) || near(lines[0].y1.unwrap(), 200.0 + 3.0));
        assert!(near(
            (lines[0].y1.unwrap() - lines[1].y1.unwrap()).abs(),
            6.0
        ));

        let thick = derive_shapes(&co2("thick"), 600.0);
        let lines: Vec<_> = thick.iter().filter(|s| s.ty == ShapeType::Line).collect();
        assert_eq!(lines.len(), 2);
        assert!(lines
            .iter()
            .all(|l| l.line_width == Some(4.0) && l.stroke.is_none()));

        let mut single = co2("thick");
        single["bonds"][0]["type"] = json!("single");
        single["bonds"][1]["type"] = json!("triple");
        let lines: Vec<_> = derive_shapes(&single, 600.0)
            .into_iter()
            .filter(|s| s.ty == ShapeType::Line)
            .collect();
        assert_eq!(
            (lines[0].line_width, lines[1].line_width),
            (Some(2.0), Some(6.0))
        );
    }

    #[test]
    fn triple_parallel_bond_has_offsets_minus4_0_plus4() {
        let mut p = co2("parallel");
        p["bonds"] = json!([{"from": "c", "to": "o2", "type": "triple"}]);
        let ys: Vec<f32> = derive_shapes(&p, 600.0)
            .iter()
            .filter(|s| s.ty == ShapeType::Line)
            .map(|l| l.y1.unwrap())
            .collect();
        assert_eq!(ys.len(), 3);
        // c->o2 runs along +x, so the perpendicular is (0, 1)
        assert!(near(ys[0], 196.0) && near(ys[1], 200.0) && near(ys[2], 204.0));
    }

    #[test]
    fn bond_state_picks_color_and_dash_and_an_explicit_color_wins() {
        let line = |state: Option<&str>, color: Option<&str>| {
            let mut p = water();
            p["bonds"] = json!([{"from": "o", "to": "h1", "type": "single"}]);
            if let Some(s) = state {
                p["bonds"][0]["state"] = json!(s);
            }
            if let Some(c) = color {
                p["bonds"][0]["color"] = json!(c);
            }
            derive_shapes(&p, 600.0)
                .into_iter()
                .find(|s| s.ty == ShapeType::Line)
                .unwrap()
        };
        let d = line(None, None);
        assert_eq!((d.color.as_deref(), d.dash), (Some("ink"), None));
        let f = line(Some("forming"), None);
        assert_eq!(
            (f.color.as_deref(), f.dash),
            (Some("series-3"), Some(Dash::Dashed))
        );
        let b = line(Some("breaking"), None);
        assert_eq!(
            (b.color.as_deref(), b.dash),
            (Some("series-2"), Some(Dash::Dashed))
        );
        let h = line(Some("highlight"), None);
        assert_eq!((h.color.as_deref(), h.dash), (Some("highlight"), None));
        let o = line(Some("forming"), Some("#ff0000"));
        assert_eq!(
            (o.color.as_deref(), o.dash),
            (Some("#ff0000"), Some(Dash::Dashed))
        );
    }

    #[test]
    fn bonds_to_unknown_atoms_are_skipped() {
        let mut p = water();
        p["bonds"] =
            json!([{"from": "o", "to": "nope", "type": "single"}, {"from": "ghost", "to": "h1"}]);
        assert_eq!(count(&derive_shapes(&p, 600.0), ShapeType::Line), 0);
    }

    #[test]
    fn atoms_without_a_color_take_a_series_tone_per_distinct_element_in_order() {
        let p = json!({"atoms": [
            {"id": "a", "x": 1, "y": 1, "element": "C"},
            {"id": "b", "x": 2, "y": 2, "element": "H"},
            {"id": "c", "x": 3, "y": 3, "element": "C"},
            {"id": "d", "x": 4, "y": 4},
            {"id": "e", "x": 5, "y": 5, "element": "O", "color": "#00ff00"}
        ]});
        let colors: Vec<_> = derive_shapes(&p, 600.0)
            .iter()
            .filter(|s| s.ty == ShapeType::Circle)
            .map(|s| (s.color.clone().unwrap(), s.radius.unwrap()))
            .collect();
        let tones: Vec<_> = colors.iter().map(|(c, _)| c.as_str()).collect();
        assert_eq!(
            tones,
            vec!["series-1", "series-2", "series-1", "series-1", "#00ff00"]
        );
        assert!(colors.iter().all(|(_, r)| *r == 14.0), "default radius 14");
        let p = json!({"atoms": [{"x": 1, "y": 1, "element": "A"}, {"x": 1, "y": 1, "element": "B"}, {"x": 1, "y": 1, "element": "C"}]});
        let c: Vec<_> = derive_shapes(&p, 600.0)
            .iter()
            .filter(|s| s.ty == ShapeType::Circle)
            .map(|s| s.color.clone().unwrap())
            .collect();
        assert_eq!(c, vec!["series-1", "series-2", "series-3"]);
    }

    #[test]
    fn element_and_charge_text_only_for_non_empty_values() {
        let p = json!({"atoms": [{"id": "a", "x": 1, "y": 1, "element": "", "charge": ""}]});
        assert_eq!(count(&derive_shapes(&p, 600.0), ShapeType::Text), 0);
        let p = json!({"atoms": [{"id": "a", "x": 1, "y": 1, "element": "N", "charge": "+"}]});
        assert_eq!(count(&derive_shapes(&p, 600.0), ShapeType::Text), 2);
    }

    #[test]
    fn titration_containers_stack_liquid_then_outline_then_label() {
        let shapes = derive_shapes(&titration(), 600.0);
        let rects: Vec<_> = shapes.iter().filter(|s| s.ty == ShapeType::Rect).collect();
        assert_eq!(rects.len(), 4, "liquid + outline per container");
        let liquid = rects[0];
        let lv = 25.0_f32 / 65.0;
        assert!(near(liquid.x.unwrap(), 73.0) && near(liquid.width.unwrap(), 166.0));
        assert!(near(liquid.y.unwrap(), 120.0 + 240.0 * (1.0 - lv)));
        assert!(near(liquid.height.unwrap(), 240.0 * lv - 1.0));
        assert_eq!(
            (
                liquid.color.as_deref(),
                liquid.fill.as_deref(),
                liquid.fill_style.as_deref()
            ),
            (Some("series-1"), Some("series-1"), Some("solid"))
        );
        assert_eq!(
            (liquid.stroke.as_deref(), liquid.opacity),
            (Some("thin"), Some(0.5))
        );
        let outline = rects[1];
        assert_eq!(
            (outline.x, outline.y, outline.width, outline.height),
            (Some(72.0), Some(120.0), Some(168.0), Some(240.0))
        );
        assert_eq!(
            (
                outline.color.as_deref(),
                outline.fill.clone(),
                outline.stroke.as_deref()
            ),
            (Some("ink"), None, Some("normal"))
        );
        let label = shapes
            .iter()
            .find(|s| s.text.as_deref() == Some("Beaker"))
            .unwrap();
        assert!(near(label.x.unwrap(), 156.0) && near(label.y.unwrap(), 372.0));
        assert_eq!(
            (
                label.tone.as_deref(),
                label.text_size.as_deref(),
                label.align
            ),
            (Some("ink"), Some("sm"), Some(Align::Center))
        );
        assert_eq!(
            count(&shapes, ShapeType::Line),
            0,
            "divider none draws no line"
        );
    }

    #[test]
    fn a_container_without_a_level_draws_no_liquid() {
        let mut p = titration();
        p["containers"][0].as_object_mut().unwrap().remove("level");
        let shapes = derive_shapes(&p, 600.0);
        assert_eq!(count(&shapes, ShapeType::Rect), 3);
    }

    #[test]
    fn osmosis_divider_is_a_dashed_midline_with_half_labels() {
        let shapes = derive_shapes(&osmosis(), 600.0);
        let div = shapes.iter().find(|s| s.ty == ShapeType::Line).unwrap();
        assert_eq!(
            (div.x1, div.y1, div.x2, div.y2),
            (Some(300.0), Some(0.0), Some(300.0), Some(400.0))
        );
        assert_eq!(
            (div.dash, div.stroke.as_deref(), div.color.as_deref()),
            (Some(Dash::Dashed), Some("thin"), Some("ink"))
        );
        let l = shapes
            .iter()
            .find(|s| s.text.as_deref() == Some("W 4"))
            .unwrap();
        let r = shapes
            .iter()
            .find(|s| s.text.as_deref() == Some("W 4 S 6"))
            .unwrap();
        assert_eq!(
            (l.x, l.y, l.tone.as_deref()),
            (Some(150.0), Some(12.0), Some("label"))
        );
        assert_eq!((r.x, r.y), (Some(450.0), Some(12.0)));
    }

    #[test]
    fn divider_style_color_and_none_each_change_the_line() {
        let div = |d: &str, dc: Option<&str>| {
            let mut p = osmosis();
            p["containers"][0]["divider"] = json!(d);
            p["containers"][0]["color"] = json!("series-2");
            if let Some(c) = dc {
                p["containers"][0]["dividerColor"] = json!(c);
            }
            derive_shapes(&p, 600.0)
                .into_iter()
                .find(|s| s.ty == ShapeType::Line)
        };
        assert_eq!(div("dotted", None).unwrap().dash, Some(Dash::Dotted));
        assert_eq!(div("solid", None).unwrap().dash, None);
        assert_eq!(
            div("solid", None).unwrap().color.as_deref(),
            Some("series-2"),
            "defaults to the container color"
        );
        assert_eq!(
            div("solid", Some("series-5")).unwrap().color.as_deref(),
            Some("series-5")
        );
        assert!(div("none", None).is_none());
    }

    #[test]
    fn container_line_width_and_fill_reach_the_outline_rect() {
        let mut p = osmosis();
        p["containers"][0]["lineWidth"] = json!(4);
        p["containers"][0]["fill"] = json!("#112233");
        let shapes = derive_shapes(&p, 600.0);
        let rect = shapes.iter().find(|s| s.ty == ShapeType::Rect).unwrap();
        assert_eq!(
            (rect.line_width, rect.fill.as_deref()),
            (Some(4.0), Some("#112233"))
        );
    }

    #[test]
    fn arrows_run_along_their_angle_with_a_label_above_the_midpoint() {
        let p = json!({"arrows": [{"x": 100, "y": 50, "angle": 90, "length": 40, "color": "series-3", "label": "heat"}]});
        let shapes = derive_shapes(&p, 600.0);
        let a = &shapes[0];
        assert_eq!(a.ty, ShapeType::Arrow);
        assert!(near(a.x2.unwrap(), 100.0) && near(a.y2.unwrap(), 90.0));
        assert_eq!(
            (a.color.as_deref(), a.stroke.as_deref()),
            (Some("series-3"), Some("bold"))
        );
        let t = &shapes[1];
        assert!(near(t.x.unwrap(), 100.0) && near(t.y.unwrap(), 70.0 - 10.0));
        assert_eq!(
            (t.text.as_deref(), t.tone.as_deref(), t.text_case.as_deref()),
            (Some("heat"), Some("ink"), Some("verbatim"))
        );
        let d = derive_shapes(&json!({"arrows": [{"x": 10, "y": 10}]}), 600.0);
        assert_eq!(d.len(), 1, "no label, no text");
        assert!(
            near(d[0].x2.unwrap(), 70.0) && near(d[0].y2.unwrap(), 10.0),
            "default angle 0, length 60"
        );
        assert_eq!(d[0].color.as_deref(), Some("ink"));
    }

    #[test]
    fn paint_order_is_containers_bonds_arrows_atoms_equation_then_authored_shapes() {
        let mut p = water();
        p["containers"] = json!([{"x": 0, "y": 0, "width": 10, "height": 10}]);
        p["arrows"] = json!([{"x": 0, "y": 0}]);
        p["shapes"] = json!([{"type": "line", "x1": 0, "y1": 0, "x2": 1, "y2": 1, "id": "mine"}]);
        let shapes = derive_shapes(&p, 600.0);
        let first_of = |ty: ShapeType| shapes.iter().position(|s| s.ty == ty).unwrap();
        assert_eq!(shapes[0].ty, ShapeType::Rect);
        assert!(first_of(ShapeType::Rect) < first_of(ShapeType::Line));
        assert!(first_of(ShapeType::Arrow) < first_of(ShapeType::Circle));
        assert_eq!(shapes.last().unwrap().id.as_deref(), Some("mine"));
        assert_eq!(
            shapes[shapes.len() - 2].text.as_deref(),
            Some("H2O"),
            "equation just before the authored shapes"
        );
    }

    #[test]
    fn equation_is_centered_on_the_canvas_width_and_takes_its_color() {
        let mut p = json!({"width": 400, "equation": "A + B", "equationColor": "series-2"});
        let eq = derive_shapes(&p, 400.0).pop().unwrap();
        assert_eq!(
            (eq.x, eq.y, eq.color.as_deref(), eq.text_case.as_deref()),
            (Some(200.0), Some(14.0), Some("series-2"), Some("verbatim"))
        );
        p["equation"] = json!("");
        assert!(
            derive_shapes(&p, 400.0).is_empty(),
            "an empty equation draws nothing"
        );
    }

    #[test]
    fn painted_parallel_bonds_are_normal_ink_strokes_and_thick_ones_are_wider() {
        let scene = map(&water(), None).unwrap();
        let theme = canvas::theme_for(&water());
        let lines = seg_lines(&scene);
        assert_eq!(lines.len(), 2);
        assert!(lines
            .iter()
            .all(|(_, _, s)| s.color == theme.tone("ink").unwrap()
                && s.width == 1.5
                && s.dash.is_empty()));
        let mut thick = water();
        thick["bondStyle"] = json!("thick");
        let lines = seg_lines(&map(&thick, None).unwrap());
        assert!(
            lines.iter().all(|(_, _, s)| s.width == 2.0),
            "single bond thick width"
        );
        let mut forming = water();
        forming["bonds"][0]["state"] = json!("forming");
        let lines = seg_lines(&map(&forming, None).unwrap());
        assert_eq!(lines[0].2.dash, vec![6.0, 4.0]);
        assert_eq!(lines[0].2.color, theme.tone("series-3").unwrap());
        assert!(
            lines[1].2.dash.is_empty(),
            "control: the other bond stays solid"
        );
    }

    #[test]
    fn painted_equation_and_element_labels_appear_and_vanish_with_their_props() {
        let scene = map(&water(), None).unwrap();
        let t = texts(&scene);
        assert!(t
            .iter()
            .any(|(s, pos, _, _)| s == "H2O" && *pos == (300.0, 14.0)));
        assert!(t.iter().any(|(s, _, _, _)| s == "O"));
        let mut bare = water();
        bare["equation"] = json!("");
        assert!(!texts(&map(&bare, None).unwrap())
            .iter()
            .any(|(s, ..)| s == "H2O"));
    }

    #[test]
    fn titration_readouts_are_drawn_by_the_canvas_atom() {
        let with = texts(&map(&titration(), None).unwrap());
        assert!(with.iter().any(|(s, ..)| s.contains("pH")), "{with:?}");
        let mut p = titration();
        p.as_object_mut().unwrap().remove("readouts");
        let without = texts(&map(&p, None).unwrap());
        assert!(without.len() < with.len());
    }

    #[test]
    fn three_d_mode_is_refused_and_two_d_is_not() {
        let mut p = water();
        p["mode"] = json!("3d");
        match map(&p, None) {
            Err(CanvasSceneError::UnsupportedMode(m)) => assert_eq!(m, "3d"),
            other => panic!("{other:?}"),
        }
        p["mode"] = json!("2d");
        assert!(map(&p, None).is_ok());
    }

    #[test]
    fn interactive_scenes_hit_test_atoms_with_the_authored_event_and_id() {
        let mut p = water();
        p["interactive"] = json!(true);
        p["onShapeClick"] = json!("PICK_ATOM");
        let scene = map(&p, None).unwrap();
        assert!(!scene.hits.is_empty());
        let atom_hit = scene
            .hits
            .iter()
            .find(|h| h.emits[0].payload.get("id") == Some(&json!("o")))
            .expect("oxygen hit");
        assert_eq!(atom_hit.emits[0].event, "PICK_ATOM");
        assert_eq!(atom_hit.emits[0].payload["type"], "circle");
        let mut off = water();
        off["onShapeClick"] = json!("PICK_ATOM");
        assert!(
            map(&off, None).unwrap().hits.is_empty(),
            "control: not interactive"
        );
    }

    #[test]
    fn series_override_recolors_atoms_that_take_a_series_tone() {
        let mut p = water();
        p["atoms"][0].as_object_mut().unwrap().remove("color");
        let plain = map(&p, None).unwrap();
        p["series"] = json!(["#ff0000"]);
        let recolored = map(&p, None).unwrap();
        assert_ne!(
            format!("{:?}", plain.packet.draw_commands),
            format!("{:?}", recolored.packet.draw_commands)
        );
    }
}
