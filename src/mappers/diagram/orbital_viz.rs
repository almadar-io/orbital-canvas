//! `orbital-visualization` — the Rust twin of `OrbitalVisualization.tsx`: a schema's complexity
//! drawn as an atomic orbital (sphere, dumbbell, three dumbbells or a cloverleaf) with its name
//! and complexity score below. The web paints CSS radial gradients and glows; here they are
//! stacked translucent discs. The CSS pulse (sphere) and rotate (dumbbell, cloverleaf) animations
//! are declared on the packet and played by the host clock.

use super::draw::{frame, tint, Motion, Pen};
use super::props_str;
use crate::mappers::game_canvas_2d::CanvasSceneError;
use crate::mappers::learning::canvas::theme_for;
use crate::mappers::scene::{Emit, MappedScene};
use crate::packet::{Easing, Keyframe, TextAlign};
use serde_json::{json, Value};

struct Orbital {
    name: &'static str,
    series: usize,
    lobes: u8,
    has_node: bool,
    scale: f32,
}

const ORBITALS: [(&str, Orbital); 7] = [
    (
        "1s",
        Orbital {
            name: "1s Orbital",
            series: 1,
            lobes: 1,
            has_node: false,
            scale: 0.6,
        },
    ),
    (
        "2s",
        Orbital {
            name: "2s Orbital",
            series: 2,
            lobes: 1,
            has_node: false,
            scale: 0.8,
        },
    ),
    (
        "2p",
        Orbital {
            name: "2p Orbital",
            series: 3,
            lobes: 2,
            has_node: false,
            scale: 1.0,
        },
    ),
    (
        "3s",
        Orbital {
            name: "3s Orbital",
            series: 4,
            lobes: 1,
            has_node: true,
            scale: 1.0,
        },
    ),
    (
        "3p",
        Orbital {
            name: "3p Orbital",
            series: 5,
            lobes: 2,
            has_node: true,
            scale: 1.1,
        },
    ),
    (
        "3d",
        Orbital {
            name: "3d Orbital",
            series: 6,
            lobes: 4,
            has_node: true,
            scale: 1.2,
        },
    ),
    (
        "4f",
        Orbital {
            name: "4f Orbital",
            series: 7,
            lobes: 6,
            has_node: true,
            scale: 1.3,
        },
    ),
];

fn orbital_for(complexity: f32) -> &'static Orbital {
    let i = match complexity {
        c if c <= 3.0 => 0,
        c if c <= 8.0 => 1,
        c if c <= 15.0 => 2,
        c if c <= 25.0 => 3,
        c if c <= 40.0 => 4,
        c if c <= 60.0 => 5,
        _ => 6,
    };
    &ORBITALS[i].1
}

/// `calculateComplexity`: entities x3 + pages x2 + traits x2 + sections x1; no schema scores 1.
fn complexity(props: &Value) -> f32 {
    if let Some(c) = props.get("complexity").and_then(Value::as_f64) {
        return c as f32;
    }
    let Some(schema) = props.get("schema").filter(|s| s.is_object()) else {
        return 1.0;
    };
    let num = |v: Option<&Value>| v.and_then(Value::as_f64).unwrap_or(0.0) as f32;
    let pages = schema
        .get("ui")
        .and_then(|u| u.get("pages"))
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let sections: f32 = pages.iter().map(|p| num(p.get("sections"))).sum();
    num(schema.get("dataEntities")) * 3.0
        + pages.len() as f32 * 2.0
        + num(schema.get("traits")) * 2.0
        + sections
}

fn size_px(props: &Value) -> f32 {
    match props_str(props, "size").unwrap_or("md") {
        "sm" => 120.0,
        "lg" => 300.0,
        "xl" => 400.0,
        _ => 200.0,
    }
}

pub fn map(props: &Value, viewport: Option<(f32, f32)>) -> Result<MappedScene, CanvasSceneError> {
    let theme = theme_for(props);
    let size = size_px(props);
    let show_label = props
        .get("showLabel")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let (w, h) = (size, size + if show_label { 60.0 } else { 0.0 });
    let (xf, pw, ph) = frame(w, h, viewport);
    let mut pen = Pen::new(&theme, xf);
    pen.clear(theme.ground);

    let score = complexity(props);
    let o = orbital_for(score);
    let color = pen.tone(&format!("series-{}", o.series));
    let (cx, cy) = (size / 2.0, size / 2.0);

    pen.radial_fade(
        cx,
        cy,
        size * 0.4,
        size * 0.4,
        0.0,
        tint(color, 100.0),
        &[(0.0, 0.09), (0.7, 0.0)],
        (0.0, 0.0),
    );
    let animated = props
        .get("animated")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let centre = (cx, cy);
    match o.lobes {
        2 => {
            let one = |pen: &mut Pen, lobe_size: f32, rotation: f32| {
                let draw = |pen: &mut Pen| dumbbell(pen, o, size, lobe_size, rotation, color);
                if animated {
                    pen.animated(centre, spin(8000.0, rotation), draw);
                } else {
                    draw(pen);
                }
            };
            one(&mut pen, size, 0.0);
            if o.has_node {
                one(&mut pen, size * 0.7, 60.0);
                one(&mut pen, size * 0.7, 120.0);
            }
        }
        4 | 6 => {
            let draw = |pen: &mut Pen| cloverleaf(pen, o, size, color);
            if animated {
                pen.animated(centre, spin(12000.0, 0.0), draw);
            } else {
                draw(&mut pen);
            }
        }
        _ => {
            let draw = |pen: &mut Pen| sphere(pen, o, size, color);
            if animated {
                pen.animated(centre, pulse(), draw);
            } else {
                draw(&mut pen);
            }
        }
    }

    if show_label {
        let (ink, quiet) = (pen.tone("ink"), pen.tone("label"));
        pen.text_middle(
            w / 2.0,
            size + 16.0 + 14.0,
            o.name,
            18.0,
            ink,
            TextAlign::Center,
            600,
        );
        pen.text_middle(
            w / 2.0,
            size + 16.0 + 28.0 + 10.0,
            &format!("Complexity: {} units", format_units(score)),
            14.0,
            quiet,
            TextAlign::Center,
            400,
        );
    }
    if let Some(event) = props_str(props, "onClick").filter(|e| !e.is_empty()) {
        pen.hit(
            0.0,
            0.0,
            w,
            h,
            vec![Emit {
                event: event.to_string(),
                arg: String::new(),
                payload: json!({}),
            }],
        );
    }
    Ok(pen.finish("orbital-visualization", pw, ph))
}

/// `orbital-rotate`: a full turn, linear; it replaces the rotation the element is drawn at.
fn spin(duration_ms: f32, rest_rotation: f32) -> Motion {
    Motion::new(
        duration_ms,
        Easing::Linear,
        vec![
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
    )
    .from_rotation(rest_rotation)
}

/// `orbital-pulse`: 3s ease-in-out, swelling 5% and dimming to 0.9 at the midpoint.
fn pulse() -> Motion {
    let frame = |offset: f32, scale: f32, opacity: f32| Keyframe {
        offset,
        scale: Some(scale),
        opacity: Some(opacity),
        ..Keyframe::default()
    };
    Motion::new(
        3000.0,
        Easing::EaseInOut,
        vec![
            frame(0.0, 1.0, 1.0),
            frame(0.5, 1.05, 0.9),
            frame(1.0, 1.0, 1.0),
        ],
    )
}

fn format_units(c: f32) -> String {
    if c.fract() == 0.0 {
        format!("{}", c as i64)
    } else {
        c.to_string()
    }
}

fn sphere(pen: &mut Pen, o: &Orbital, size: f32, color: [f32; 4]) {
    let d = size * o.scale * 0.4;
    let (cx, cy, r) = (size / 2.0, size / 2.0, d / 2.0);
    pen.radial_fade(
        cx,
        cy,
        r + size * 0.3,
        r + size * 0.3,
        0.0,
        color,
        &[(0.0, 0.0), (r / (r + size * 0.3), 0.3), (1.0, 0.0)],
        (0.0, 0.0),
    );
    pen.radial_fade(
        cx,
        cy,
        r,
        r,
        0.0,
        color,
        &[(0.0, 0.87), (0.5, 0.53), (1.0, 0.27)],
        (-0.4, -0.4),
    );
}

fn dumbbell(pen: &mut Pen, o: &Orbital, container: f32, size: f32, rotation: f32, color: [f32; 4]) {
    let lobe = size * o.scale * 0.25;
    let (lw, lh) = (lobe, lobe * 1.4);
    let offset = size * 0.18;
    let (cx, cy) = (container / 2.0, container / 2.0);
    let (sin, cos) = rotation.to_radians().sin_cos();
    for dir in [-1.0f32, 1.0] {
        let (dx, dy) = (0.0, dir * (offset + lh / 2.0));
        let (lx, ly) = (cx + dx * cos - dy * sin, cy + dx * sin + dy * cos);
        pen.radial_fade(
            lx,
            ly,
            lw / 2.0 + size * 0.05,
            lh / 2.0 + size * 0.05,
            rotation,
            color,
            &[(0.0, 0.0), (0.8, 0.25), (1.0, 0.0)],
            (0.0, 0.0),
        );
        pen.radial_fade(
            lx,
            ly,
            lw / 2.0,
            lh / 2.0,
            rotation,
            color,
            &[(0.0, 0.87), (0.7, 0.4), (1.0, 0.0)],
            (0.0, -dir * 0.4),
        );
    }
    if o.has_node {
        let ink = pen.tone("ink");
        pen.circle(cx, cy, size * 0.03, Some(ink), None);
    }
}

fn cloverleaf(pen: &mut Pen, o: &Orbital, size: f32, color: [f32; 4]) {
    let lobes = o.lobes as usize;
    let step = 360.0 / lobes as f32;
    let (lw, lh) = (size * 0.18, size * 0.18 * 1.3);
    let dist = size * 0.22;
    let (cx, cy) = (size / 2.0, size / 2.0);
    for i in 0..lobes {
        let a = (i as f32 * step).to_radians();
        let (lx, ly) = (cx + a.cos() * dist, cy + a.sin() * dist);
        let rot = i as f32 * step + 90.0;
        pen.radial_fade(
            lx,
            ly,
            lw / 2.0 + size * 0.04,
            lh / 2.0 + size * 0.04,
            rot,
            color,
            &[(0.0, 0.0), (0.8, 0.25), (1.0, 0.0)],
            (0.0, 0.0),
        );
        pen.radial_fade(
            lx,
            ly,
            lw / 2.0,
            lh / 2.0,
            rot,
            color,
            &[(0.0, 0.87), (0.8, 0.33), (1.0, 0.0)],
            (0.0, -0.2),
        );
    }
    let ink = pen.tone("ink");
    pen.circle(cx, cy, size * 0.04, Some(ink), None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::DrawCommand;

    /// `ui-orbital-visualization` fed a KFlow schema summary: 2 entities, 3 pages (4 sections in all)
    /// and 2 traits score 2x3 + 3x2 + 2x2 + 4 = 20, a `3s` orbital.
    fn summary() -> Value {
        json!({
            "type": "orbital-visualization", "size": "md", "showLabel": true, "animated": true,
            "schema": {"dataEntities": 2, "traits": 2, "ui": {"pages": [{"sections": 1}, {"sections": 2}, {"sections": 1}]}}
        })
    }

    fn texts(s: &MappedScene) -> Vec<String> {
        s.packet
            .draw_commands
            .iter()
            .filter_map(|c| match c {
                DrawCommand::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_schema_summary_scores_and_names_its_orbital() {
        assert_eq!(
            texts(&map(&summary(), None).unwrap()),
            ["3s Orbital", "Complexity: 20 units"]
        );
        assert_eq!(complexity(&json!({})), 1.0, "no schema scores 1");
        let mut over = summary();
        over["complexity"] = json!(70);
        assert_eq!(
            texts(&map(&over, None).unwrap())[0],
            "4f Orbital",
            "an explicit complexity overrides the schema"
        );
    }

    #[test]
    fn each_complexity_band_picks_its_orbital_at_the_boundaries() {
        for (score, name) in [
            (3.0, "1s Orbital"),
            (4.0, "2s Orbital"),
            (8.0, "2s Orbital"),
            (9.0, "2p Orbital"),
            (25.0, "3s Orbital"),
            (26.0, "3p Orbital"),
            (60.0, "3d Orbital"),
            (61.0, "4f Orbital"),
        ] {
            assert_eq!(orbital_for(score).name, name, "score {score}");
        }
    }

    #[test]
    fn size_sets_the_scene_and_the_label_adds_sixty_units_of_height() {
        let md = map(&summary(), None).unwrap();
        assert_eq!((md.packet.width, md.packet.height), (200.0, 260.0));
        let mut xl = summary();
        xl["size"] = json!("xl");
        xl["showLabel"] = json!(false);
        let big = map(&xl, None).unwrap();
        assert_eq!((big.packet.width, big.packet.height), (400.0, 400.0));
        assert!(texts(&big).is_empty(), "control: no label text");
    }

    #[test]
    fn the_orbital_family_changes_what_is_drawn_and_colours_come_from_the_series_tones() {
        let count = |score: f32| {
            let mut p = summary();
            p["complexity"] = json!(score);
            map(&p, None).unwrap().packet.draw_commands.len()
        };
        assert!(
            count(2.0) < count(12.0),
            "a dumbbell has more lobes than a sphere"
        );
        assert!(
            count(12.0) < count(30.0),
            "3p adds two rotated dumbbells and a node"
        );
        assert_ne!(
            count(50.0),
            count(30.0),
            "a 3d cloverleaf is a different build from 3p"
        );
        let theme = theme_for(&summary());
        let series = theme.tone("series-4").unwrap();
        let scene = map(&summary(), None).unwrap();
        assert!(
            scene.packet.draw_commands.iter().any(
                |c| matches!(c, DrawCommand::Path { fill: Some(f), .. } if f[..3] == series[..3])
            ),
            "3s draws in series-4"
        );
    }

    fn scored(score: f32, animated: bool) -> MappedScene {
        let mut p = summary();
        p["complexity"] = json!(score);
        p["animated"] = json!(animated);
        map(&p, None).unwrap()
    }

    fn painted(s: &MappedScene, t: f64, motion: bool) -> String {
        serde_json::to_string(&s.packet.resolved(t, motion).draw_commands).unwrap()
    }

    #[test]
    fn a_sphere_pulses_on_the_three_second_ease_in_out_and_rest_is_not_the_pulse() {
        let s = scored(2.0, true);
        assert_eq!(s.packet.animations.len(), 1);
        let a = &s.packet.animations[0];
        assert_eq!(
            (a.duration_ms, a.easing, a.iterations),
            (3000.0, Easing::EaseInOut, None)
        );
        assert_eq!(
            (a.origin.x, a.origin.y),
            (100.0, 100.0),
            "about the container centre"
        );
        assert_ne!(
            painted(&s, 0.0, true),
            painted(&s, 1500.0, true),
            "t=0 vs the 1.05 midpoint"
        );
        assert_eq!(
            painted(&s, 0.0, true),
            painted(&s, 3000.0, true),
            "a whole period lands where it began"
        );
        assert_eq!(
            painted(&s, 1500.0, false),
            painted(&s, 0.0, false),
            "reduced motion is one static frame"
        );
    }

    #[test]
    fn dumbbells_and_the_cloverleaf_rotate_a_full_turn_per_period() {
        let two_p = scored(12.0, true);
        assert_eq!(two_p.packet.animations.len(), 1);
        assert_eq!(two_p.packet.animations[0].duration_ms, 8000.0);
        let three_p = scored(30.0, true);
        let rests: Vec<f32> = three_p
            .packet
            .animations
            .iter()
            .map(|a| a.base_rotation)
            .collect();
        assert_eq!(
            rests,
            vec![0.0, 60.0, 120.0],
            "the web's keyframe replaces each element's own rotation"
        );
        let clover = scored(50.0, true);
        assert_eq!(clover.packet.animations.len(), 1);
        assert_eq!(clover.packet.animations[0].duration_ms, 12000.0);
        assert_ne!(
            painted(&two_p, 0.0, true),
            painted(&two_p, 2000.0, true),
            "a quarter turn moves the lobes"
        );
        assert_eq!(
            painted(&two_p, 0.0, true),
            painted(&two_p, 8000.0, true),
            "a full turn is back"
        );
    }

    #[test]
    fn animated_false_and_a_motion_free_theme_declare_nothing() {
        for score in [2.0, 12.0, 30.0, 50.0] {
            assert!(
                scored(score, false).packet.animations.is_empty(),
                "score {score}"
            );
        }
        let mut p = summary();
        p["canvasTheme"] = json!({"motion": {"enabled": false}});
        assert!(map(&p, None).unwrap().packet.animations.is_empty());
        assert!(
            !scored(2.0, true).packet.animations.is_empty(),
            "control: the same scene animates by default"
        );
    }

    #[test]
    fn a_tap_emits_the_authored_click_event_and_no_event_means_no_region() {
        let mut p = summary();
        p["onClick"] = json!("OPEN_SCHEMA");
        let scene = map(&p, None).unwrap();
        assert_eq!(scene.emits_at(100.0, 100.0)[0].event, "OPEN_SCHEMA");
        assert!(
            scene.emits_at(250.0, 100.0).is_empty(),
            "control: outside the component"
        );
        assert!(map(&summary(), None).unwrap().hits.is_empty());
    }
}
