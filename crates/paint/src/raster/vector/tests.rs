use super::*;

const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

fn at(data: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [data[i], data[i + 1], data[i + 2], data[i + 3]]
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> Vec<PathSegment> {
    vec![
        PathSegment::MoveTo { x, y },
        PathSegment::LineTo { x: x + w, y },
        PathSegment::LineTo { x: x + w, y: y + h },
        PathSegment::LineTo { x, y: y + h },
        PathSegment::Close,
    ]
}

#[test]
fn a_stroked_path_reaches_past_its_outline_by_its_stroke() {
    let path = DrawCommand::Path {
        segments: rect(10.0, 20.0, 30.0, 5.0),
        fill: None,
        stroke: Some(Stroke {
            color: RED,
            width: 4.0,
            dash: Vec::new(),
            dash_offset: 0.0,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
        }),
    };
    assert_eq!(
        command_bounds(&path),
        RunBounds::Box {
            x0: 2.0,
            y0: 12.0,
            x1: 48.0,
            y1: 33.0
        }
    );
}

#[test]
fn control_a_filled_path_is_bounded_by_its_outline() {
    let path = DrawCommand::Path {
        segments: rect(10.0, 20.0, 30.0, 5.0),
        fill: Some(RED),
        stroke: None,
    };
    assert_eq!(
        command_bounds(&path),
        RunBounds::Box {
            x0: 10.0,
            y0: 20.0,
            x1: 40.0,
            y1: 25.0
        }
    );
}

#[test]
fn a_shadow_reaches_three_spreads_past_its_shapes_and_a_run_is_their_union() {
    let a = DrawCommand::Path {
        segments: rect(0.0, 0.0, 10.0, 10.0),
        fill: Some(RED),
        stroke: None,
    };
    let b = DrawCommand::Path {
        segments: rect(50.0, 60.0, 10.0, 10.0),
        fill: Some(RED),
        stroke: None,
    };
    let shadow = DrawCommand::Shadow {
        commands: vec![a.clone()],
        color: RED,
        sigma: 2.0,
    };
    assert_eq!(
        command_bounds(&shadow),
        RunBounds::Box {
            x0: -6.0,
            y0: -6.0,
            x1: 16.0,
            y1: 16.0
        }
    );
    assert_eq!(
        run_bounds(&[&a, &b]),
        RunBounds::Box {
            x0: 0.0,
            y0: 0.0,
            x1: 60.0,
            y1: 70.0
        }
    );
}

#[test]
fn text_is_bounded_by_its_glyph_outlines_and_a_command_of_unknown_reach_spans_anywhere() {
    let word = DrawCommand::Text {
        color_runs: Vec::new(),
        rotation: 0.0,
        text: "Orb".into(),
        pos: Vec2 { x: 100.0, y: 50.0 },
        size: 20.0,
        color: RED,
        align: TextAlign::Left,
        weight: 400,
        halo: None,
        family: orbital_canvas_base::packet::TextFamily::Sans,
    };
    let RunBounds::Box { x0, y0, x1, y1 } = command_bounds(&word) else {
        panic!("text has a box");
    };
    assert!(
        (96.0..=102.0).contains(&x0) && x1 > 120.0 && x1 < 160.0,
        "{x0}..{x1}"
    );
    assert!(
        y0 > 30.0 && y0 < 50.0 && y1 > 50.0 && y1 < 70.0,
        "{y0}..{y1}"
    );
    let quad = DrawCommand::Quad {
        rect: orbital_canvas_base::packet::Rect {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        },
        color: RED,
    };
    assert_eq!(run_bounds(&[&word, &quad]), RunBounds::Anywhere);
}

#[test]
fn a_box_raster_draws_the_same_pixels_as_the_whole_surface_at_its_offset() {
    let path = DrawCommand::Path {
        segments: rect(20.0, 30.0, 6.0, 4.0),
        fill: Some(RED),
        stroke: None,
    };
    let whole = rasterize(&[&path], 40, 40, 1.0, 1.0).unwrap();
    let part = rasterize_box(&[&path], 10, 10, 1.0, 1.0, 18.0, 28.0).unwrap();
    for y in 0..10 {
        for x in 0..10 {
            assert_eq!(
                at(&part, 10, x, y),
                at(&whole, 40, x + 18, y + 28),
                "pixel {x},{y}"
            );
        }
    }
}

#[test]
fn a_shadow_paints_the_coverage_in_its_color_blurred_past_the_edge() {
    let square = DrawCommand::Path {
        segments: rect(15.0, 15.0, 10.0, 10.0),
        fill: Some([1.0, 1.0, 1.0, 1.0]),
        stroke: None,
    };
    let shadow = DrawCommand::Shadow {
        commands: vec![square.clone()],
        color: [0.0, 1.0, 0.0, 1.0],
        sigma: 3.0,
    };
    let glow = rasterize(&[&shadow], 40, 40, 1.0, 1.0).unwrap();
    let [r, g, b, a] = at(&glow, 40, 12, 20);
    assert!(
        a > 0 && g > 0,
        "past the edge the blur reaches: {:?}",
        [r, g, b, a]
    );
    assert_eq!(
        (r, b),
        (0, 0),
        "painted in the shadow color, not the shape's"
    );
    assert_eq!(
        at(&glow, 40, 2, 2)[3],
        0,
        "far from the shape nothing paints"
    );
    let plain = rasterize(&[&square], 40, 40, 1.0, 1.0).unwrap();
    assert_eq!(
        at(&plain, 40, 12, 20)[3],
        0,
        "control: the shape alone stops at its edge"
    );
}

fn ink_top(text: &str, weight: u16) -> Option<u32> {
    let cmd = DrawCommand::Text {
        color_runs: Vec::new(),
        rotation: 0.0,
        text: text.into(),
        pos: Vec2 { x: 40.0, y: 40.0 },
        size: 48.0,
        color: [0.0, 0.0, 0.0, 1.0],
        align: TextAlign::Center,
        weight,
        halo: None,
        family: orbital_canvas_base::packet::TextFamily::Sans,
    };
    let rgba = rasterize(&[&cmd], 80, 80, 1.0, 1.0).unwrap();
    (0..80).find(|&y| (0..80).any(|x| at(&rgba, 80, x, y)[3] > 0))
}

#[test]
fn a_bold_u_draws_no_umlaut_from_the_point_contour_in_its_glyph() {
    let (u, v) = (ink_top("u", 700).unwrap(), ink_top("v", 700).unwrap());
    assert!(
        u + 1 >= v,
        "bold u starts at row {u}, bold v at {v}: a dot above the u"
    );
    let (u, v) = (ink_top("u", 400).unwrap(), ink_top("v", 400).unwrap());
    assert!(
        u + 1 >= v,
        "control: regular u ({u}) and v ({v}) share the x-height"
    );
}

#[test]
fn filled_path_paints_inside_and_not_outside() {
    let cmd = DrawCommand::Path {
        segments: rect(10.0, 10.0, 20.0, 20.0),
        fill: Some(RED),
        stroke: None,
    };
    let data = rasterize(&[&cmd], 40, 40, 1.0, 1.0).unwrap();
    assert_eq!(at(&data, 40, 20, 20), [255, 0, 0, 255]);
    assert_eq!(at(&data, 40, 5, 5)[3], 0);
}

#[test]
fn gradient_path_samples_linear_radial_and_conic_fills() {
    let red = GradientStop {
        offset: 0.0,
        color: [1.0, 0.0, 0.0, 1.0],
    };
    let blue = GradientStop {
        offset: 1.0,
        color: [0.0, 0.0, 1.0, 1.0],
    };
    let command = |gradient| DrawCommand::GradientPath {
        segments: rect(0.0, 0.0, 40.0, 40.0),
        fill: None,
        stroke: None,
        fill_gradient: Some(gradient),
        stroke_gradient: None,
    };
    let linear = rasterize(
        &[&command(Gradient::Linear {
            from: Vec2 { x: 0.0, y: 0.0 },
            to: Vec2 { x: 40.0, y: 0.0 },
            stops: vec![red, blue],
        })],
        40,
        40,
        1.0,
        1.0,
    )
    .unwrap();
    assert!(at(&linear, 40, 5, 20)[0] > at(&linear, 40, 35, 20)[0]);
    assert!(at(&linear, 40, 35, 20)[2] > at(&linear, 40, 5, 20)[2]);

    let radial = rasterize(
        &[&command(Gradient::Radial {
            center: Vec2 { x: 20.0, y: 20.0 },
            radius: 20.0,
            stops: vec![red, blue],
        })],
        40,
        40,
        1.0,
        1.0,
    )
    .unwrap();
    assert!(at(&radial, 40, 20, 20)[0] > at(&radial, 40, 35, 20)[0]);
    assert!(at(&radial, 40, 35, 20)[2] > at(&radial, 40, 20, 20)[2]);

    let conic = rasterize(
        &[&command(Gradient::Conic {
            center: Vec2 { x: 20.0, y: 20.0 },
            angle: 0.0,
            stops: vec![red, blue],
        })],
        40,
        40,
        1.0,
        1.0,
    )
    .unwrap();
    assert!(at(&conic, 40, 20, 5)[0] > at(&conic, 40, 5, 20)[0]);
    assert!(at(&conic, 40, 5, 20)[2] > at(&conic, 40, 20, 5)[2]);
}

#[test]
fn noise_pattern_path_overlays_only_inside_its_shape() {
    let command = DrawCommand::PatternPath {
        segments: rect(5.0, 5.0, 20.0, 20.0),
        pattern: FillPattern::Noise {
            scale: 2.0,
            alpha: 0.8,
            color: [1.0, 0.0, 0.0, 1.0],
        },
        opacity: 1.0,
    };
    let data = rasterize(&[&command], 30, 30, 1.0, 1.0).unwrap();
    assert_eq!(at(&data, 30, 2, 2)[3], 0);
    assert!((6..24).any(|y| (6..24).any(|x| at(&data, 30, x, y)[3] > 0)));
}

#[test]
fn density_scale_moves_the_mark_with_the_target() {
    let cmd = DrawCommand::Path {
        segments: rect(10.0, 10.0, 20.0, 20.0),
        fill: Some(RED),
        stroke: None,
    };
    let data = rasterize(&[&cmd], 80, 80, 2.0, 2.0).unwrap();
    assert_eq!(at(&data, 80, 40, 40), [255, 0, 0, 255]);
    assert_eq!(at(&data, 80, 15, 15)[3], 0);
}

#[test]
fn dashed_stroke_leaves_gaps_a_solid_stroke_does_not() {
    let line = |dash: Vec<f32>| DrawCommand::Path {
        segments: vec![
            PathSegment::MoveTo { x: 0.0, y: 10.0 },
            PathSegment::LineTo { x: 60.0, y: 10.0 },
        ],
        fill: None,
        stroke: Some(Stroke {
            color: RED,
            width: 4.0,
            dash,
            dash_offset: 0.0,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
        }),
    };
    let solid = rasterize(&[&line(vec![])], 60, 20, 1.0, 1.0).unwrap();
    let dashed = rasterize(&[&line(vec![6.0, 4.0])], 60, 20, 1.0, 1.0).unwrap();
    let covered = |d: &[u8]| (0..60).filter(|x| at(d, 60, *x, 10)[3] > 200).count();
    assert_eq!(covered(&solid), 60);
    let n = covered(&dashed);
    assert!((30..=42).contains(&n), "dashed coverage {n}");
    assert_eq!(at(&dashed, 60, 8, 10)[3], 0, "gap after the first 6px dash");
}

#[test]
fn dash_offset_slides_the_pattern_along_the_stroke() {
    let line = |offset: f32| DrawCommand::Path {
        segments: vec![
            PathSegment::MoveTo { x: 0.0, y: 10.0 },
            PathSegment::LineTo { x: 60.0, y: 10.0 },
        ],
        fill: None,
        stroke: Some(Stroke {
            color: RED,
            width: 4.0,
            dash: vec![6.0, 4.0],
            dash_offset: offset,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
        }),
    };
    let rest = rasterize(&[&line(0.0)], 60, 20, 1.0, 1.0).unwrap();
    let slid = rasterize(&[&line(5.0)], 60, 20, 1.0, 1.0).unwrap();
    assert!(
        at(&rest, 60, 3, 10)[3] > 200,
        "x=3 is inside the first dash"
    );
    assert_eq!(
        at(&slid, 60, 3, 10)[3],
        0,
        "five units into the pattern x=3 is a gap"
    );
    let negative = rasterize(&[&line(-14.0)], 60, 20, 1.0, 1.0).unwrap();
    let wrapped = rasterize(&[&line(6.0)], 60, 20, 1.0, 1.0).unwrap();
    assert_eq!(
        negative, wrapped,
        "a negative phase wraps by the 10-unit pattern, as SVG does"
    );
}

#[test]
fn full_arc_is_a_round_disc_and_a_half_arc_is_one_side() {
    let disc = DrawCommand::Path {
        segments: vec![PathSegment::Arc {
            cx: 20.0,
            cy: 20.0,
            rx: 10.0,
            ry: 10.0,
            start: 0.0,
            end: std::f32::consts::TAU,
        }],
        fill: Some(RED),
        stroke: None,
    };
    let data = rasterize(&[&disc], 40, 40, 1.0, 1.0).unwrap();
    assert_eq!(at(&data, 40, 20, 20)[3], 255);
    assert_eq!(at(&data, 40, 29, 20)[3], 255);
    assert_eq!(at(&data, 40, 29, 29)[3], 0, "corner outside the circle");

    // 0..PI sweeps clockwise on screen = the lower half.
    let half = DrawCommand::Path {
        segments: vec![PathSegment::Arc {
            cx: 20.0,
            cy: 20.0,
            rx: 10.0,
            ry: 10.0,
            start: 0.0,
            end: std::f32::consts::PI,
        }],
        fill: Some(RED),
        stroke: None,
    };
    let data = rasterize(&[&half], 40, 40, 1.0, 1.0).unwrap();
    assert_eq!(at(&data, 40, 20, 26)[3], 255);
    assert_eq!(at(&data, 40, 20, 14)[3], 0);
}

fn ink(data: &[u8], w: u32, h: u32) -> Option<(u32, u32, u32, u32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    let mut any = false;
    for y in 0..h {
        for x in 0..w {
            if at(data, w, x, y)[3] > 0 {
                any = true;
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    any.then_some((x0, y0, x1, y1))
}

fn text(s: &str, x: f32, align: TextAlign) -> DrawCommand {
    DrawCommand::Text {
        color_runs: Vec::new(),
        rotation: 0.0,
        text: s.into(),
        pos: Vec2 { x, y: 20.0 },
        size: 14.0,
        color: RED,
        align,
        weight: 400,
        family: orbital_canvas_base::packet::TextFamily::Sans,
        halo: None,
    }
}

#[test]
fn text_draws_glyph_ink_and_a_longer_string_is_wider() {
    let short = rasterize(&[&text("x", 5.0, TextAlign::Left)], 120, 40, 1.0, 1.0).unwrap();
    let long = rasterize(&[&text("xxxxxx", 5.0, TextAlign::Left)], 120, 40, 1.0, 1.0).unwrap();
    let (sx0, _, sx1, _) = ink(&short, 120, 40).expect("ink for x");
    let (lx0, _, lx1, _) = ink(&long, 120, 40).expect("ink for xxxxxx");
    assert!(
        lx1 - lx0 > (sx1 - sx0) * 4,
        "six glyphs span far more than one"
    );
    assert!(lx0 >= 5);
}

#[test]
fn text_alignment_moves_ink_across_the_anchor() {
    let l = rasterize(&[&text("abc", 60.0, TextAlign::Left)], 120, 40, 1.0, 1.0).unwrap();
    let c = rasterize(&[&text("abc", 60.0, TextAlign::Center)], 120, 40, 1.0, 1.0).unwrap();
    let r = rasterize(&[&text("abc", 60.0, TextAlign::Right)], 120, 40, 1.0, 1.0).unwrap();
    let (l0, _, _, _) = ink(&l, 120, 40).unwrap();
    let (c0, _, c1, _) = ink(&c, 120, 40).unwrap();
    let (_, _, r1, _) = ink(&r, 120, 40).unwrap();
    assert!(l0 >= 59, "left text starts at the anchor, got {l0}");
    assert!(c0 < 60 && c1 > 60, "centered text straddles the anchor");
    assert!(r1 <= 61, "right text ends at the anchor, got {r1}");
}

#[test]
fn text_is_vertically_centered_on_the_anchor() {
    let d = rasterize(&[&text("x", 5.0, TextAlign::Left)], 60, 40, 1.0, 1.0).unwrap();
    let (_, y0, _, y1) = ink(&d, 60, 40).unwrap();
    let mid = (y0 + y1) as f32 / 2.0;
    assert!(
        (mid - 20.0).abs() < 4.0,
        "x-height centered near y=20, got {mid}"
    );
}

#[test]
fn halo_adds_ink_around_the_glyph() {
    let plain = text("o", 10.0, TextAlign::Left);
    let haloed = DrawCommand::Text {
        color_runs: Vec::new(),
        rotation: 0.0,
        text: "o".into(),
        pos: Vec2 { x: 10.0, y: 20.0 },
        size: 14.0,
        color: RED,
        align: TextAlign::Left,
        weight: 400,
        family: orbital_canvas_base::packet::TextFamily::Sans,
        halo: Some(orbital_canvas_base::packet::Halo {
            color: [1.0, 1.0, 1.0, 1.0],
            width: 4.0,
        }),
    };
    let a = rasterize(&[&plain], 60, 40, 1.0, 1.0).unwrap();
    let b = rasterize(&[&haloed], 60, 40, 1.0, 1.0).unwrap();
    let count = |d: &[u8]| d.chunks_exact(4).filter(|p| p[3] > 0).count();
    assert!(count(&b) > count(&a));
}

fn circle(cx: f32, cy: f32, r: f32) -> Vec<PathSegment> {
    vec![PathSegment::Arc {
        cx,
        cy,
        rx: r,
        ry: r,
        start: 0.0,
        end: std::f32::consts::TAU,
    }]
}

#[test]
fn a_clipped_path_paints_only_inside_every_clip_and_outside_every_cut() {
    // Two overlapping discs; the lens is their intersection, minus a small cut disc.
    let cmd = DrawCommand::ClippedPath {
        segments: rect(0.0, 0.0, 60.0, 40.0),
        fill: Some(RED),
        stroke: None,
        clip: vec![circle(20.0, 20.0, 15.0), circle(40.0, 20.0, 15.0)],
        cut: vec![circle(30.0, 12.0, 3.0)],
    };
    let data = rasterize(&[&cmd], 60, 40, 1.0, 1.0).unwrap();
    assert_eq!(at(&data, 60, 30, 26), [255, 0, 0, 255], "inside the lens");
    assert_eq!(at(&data, 60, 10, 20)[3], 0, "only in the left disc");
    assert_eq!(at(&data, 60, 50, 20)[3], 0, "only in the right disc");
    assert_eq!(at(&data, 60, 30, 12)[3], 0, "punched out by the cut");
}

#[test]
fn control_a_clipped_path_with_no_regions_paints_like_a_path() {
    let clipped = DrawCommand::ClippedPath {
        segments: rect(10.0, 10.0, 20.0, 20.0),
        fill: Some(RED),
        stroke: None,
        clip: Vec::new(),
        cut: Vec::new(),
    };
    let plain = DrawCommand::Path {
        segments: rect(10.0, 10.0, 20.0, 20.0),
        fill: Some(RED),
        stroke: None,
    };
    assert_eq!(
        rasterize(&[&clipped], 40, 40, 1.0, 1.0).unwrap(),
        rasterize(&[&plain], 40, 40, 1.0, 1.0).unwrap()
    );
}

#[test]
fn the_mono_face_gives_every_char_one_advance_and_the_body_face_does_not() {
    let (i, w) = (mono_text_width("i", 12.0), mono_text_width("W", 12.0));
    assert!((i - w).abs() < 0.01 && (i - 7.22).abs() < 0.1, "{i} {w}");
    assert!(
        text_width("i", 12.0) < text_width("W", 12.0),
        "control: the body face is proportional"
    );
    assert!(shape("ab", 12.0, false, Family::Mono)
        .glyphs
        .iter()
        .all(|g| g.font == FontId::Mono));
}

#[test]
fn mono_code_keeps_one_glyph_per_typed_char_and_bold_takes_the_bold_file() {
    for text in ["->", "!=", ">=", "=>", "::"] {
        let s = shape(text, 14.0, false, Family::Mono);
        let nominal: Vec<u16> = text
            .chars()
            .filter_map(|c| font_for(FontId::Mono)?.glyph_index(c).map(|g| g.0))
            .collect();
        let gids: Vec<u16> = s.glyphs.iter().map(|g| g.gid).collect();
        assert_eq!(
            gids, nominal,
            "{text} draws the typed characters, not a ligature"
        );
    }
    let bold = shape("ab", 14.0, true, Family::Mono);
    assert!(bold.glyphs.iter().all(|g| g.font == FontId::MonoBold));
    assert!(!FontId::Mono.faux_bold() && !FontId::MonoBold.faux_bold());
    let outline = |id: FontId| {
        let face = font_for(id).unwrap();
        let bbox = face
            .glyph_bounding_box(face.glyph_index('H').unwrap())
            .unwrap();
        (bbox.x_min, bbox.x_max, bbox.y_max)
    };
    assert_ne!(outline(FontId::Mono), outline(FontId::MonoBold));
    assert!(
        (shape("iW", 14.0, true, Family::Mono).width
            - shape("iW", 14.0, false, Family::Mono).width)
            .abs()
            < 0.01,
        "bold keeps the cell width"
    );
    assert!(
        shape("ab", 14.0, true, Family::Sans)
            .glyphs
            .iter()
            .all(|g| g.font == FontId::Latin),
        "control: sans bold stays in the sans face"
    );
}

#[test]
fn mono_text_falls_to_the_script_face_where_mono_has_no_glyph() {
    let s = shape("a\u{645}", 12.0, false, Family::Mono);
    assert_eq!(s.glyphs.first().map(|g| g.font), Some(FontId::Mono));
    assert!(s.glyphs.iter().any(|g| g.font == FontId::ArabicRegular));
}

/// The missing-script list is process-wide; tests that drain it take turns.
static MISSING_LIST: Mutex<()> = Mutex::new(());

#[test]
fn an_uncovered_script_is_reported_missing_once() {
    let _turn = MISSING_LIST.lock().unwrap_or_else(|e| e.into_inner());
    let s = shape(
        "\u{05e9}\u{05dc}\u{05d5}\u{05dd}",
        16.0,
        false,
        Family::Sans,
    );
    assert!(!s.glyphs.is_empty(), "still lays out (as .notdef)");
    let first = take_missing_scripts();
    assert!(
        first.contains(&MissingScript {
            script: "Hebr".into(),
            sample: "\u{05e9}".into()
        }),
        "{first:?}"
    );
    shape(
        "\u{05e9}\u{05dc}\u{05d5}\u{05dd}",
        16.0,
        false,
        Family::Sans,
    );
    assert!(
        !take_missing_scripts().iter().any(|m| m.script == "Hebr"),
        "reported once"
    );
}

#[test]
fn control_a_covered_script_is_never_reported() {
    let _turn = MISSING_LIST.lock().unwrap_or_else(|e| e.into_inner());
    shape("Hello \u{03b8}", 16.0, false, Family::Sans);
    let missing = take_missing_scripts();
    assert!(
        !missing
            .iter()
            .any(|m| m.script == "Latn" || m.script == "Grek"),
        "{missing:?}"
    );
}

#[test]
fn a_host_face_routes_its_script_and_bumps_the_font_generation() {
    let _font = FONT_GENERATION_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let before = font_generation();
    add_font_face("Deva", ARABIC_REGULAR_BYTES.to_vec(), 0).unwrap();
    assert!(font_generation() > before);
    let s = shape("\u{0928}\u{092e}", 16.0, false, Family::Sans);
    assert!(
        s.glyphs.iter().all(|g| matches!(g.font, FontId::Host(_))),
        "{:?}",
        fonts_of(&s)
    );
    let latin = shape("ab", 16.0, false, Family::Sans);
    assert!(
        latin.glyphs.iter().all(|g| g.font == FontId::Latin),
        "control: Latin keeps the bundled face"
    );
}

#[test]
fn a_face_without_the_reported_sample_glyph_is_refused() {
    let _font = FONT_GENERATION_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    shape("\u{0e01}", 16.0, false, Family::Sans);
    let err = add_font_face("Thai", LATIN_BYTES.to_vec(), 0).unwrap_err();
    assert!(err.contains("no glyph"), "{err}");
    assert!(
        shape("\u{0e01}", 16.0, false, Family::Sans)
            .glyphs
            .iter()
            .all(|g| g.font == FontId::Latin),
        "the refused face is not registered"
    );
}

#[test]
fn add_font_face_rejects_an_unknown_script_and_bytes_that_are_not_a_font() {
    let _font = FONT_GENERATION_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    assert!(add_font_face("Zzzq", ARABIC_REGULAR_BYTES.to_vec(), 0).is_err());
    assert!(add_font_face("Khmr", vec![0, 1, 2, 3], 0).is_err());
}

#[test]
fn semi_transparent_fill_is_straight_alpha() {
    let cmd = DrawCommand::Path {
        segments: rect(0.0, 0.0, 10.0, 10.0),
        fill: Some([1.0, 0.0, 0.0, 0.5]),
        stroke: None,
    };
    let data = rasterize(&[&cmd], 10, 10, 1.0, 1.0).unwrap();
    let p = at(&data, 10, 5, 5);
    assert!(p[0] >= 250 && (p[3] as i32 - 128).abs() <= 2, "{p:?}");
}

fn nominal_gids(font: FontId, s: &str) -> Vec<u16> {
    let f = font_for(font).unwrap();
    s.chars()
        .map(|c| f.glyph_index(c).map_or(0, |g| g.0))
        .collect()
}

fn fonts_of(s: &Shaped) -> Vec<FontId> {
    s.glyphs.iter().map(|g| g.font).collect()
}

const KITAB: &str = "\u{643}\u{62A}\u{627}\u{628}";
const MARHABA: &str = "\u{645}\u{631}\u{62D}\u{628}\u{627}";

#[test]
fn arabic_glyph_count_differs_from_char_count_and_latin_does_not() {
    for word in [KITAB, MARHABA] {
        let ar = shape(word, 14.0, false, Family::Sans);
        assert_ne!(ar.glyphs.len(), word.chars().count(), "{word}");
    }
    let en = shape("salm", 14.0, false, Family::Sans);
    assert_eq!(en.glyphs.len(), "salm".chars().count());
}

#[test]
fn arabic_uses_cursive_forms_not_the_nominal_glyphs() {
    let s = shape(MARHABA, 14.0, false, Family::Sans);
    assert!(s.glyphs.iter().all(|g| g.font == FontId::ArabicRegular));
    let nominal = nominal_gids(FontId::ArabicRegular, MARHABA);
    assert!(nominal.iter().all(|g| *g != 0), "subset covers the letters");
    let first_logical = s.glyphs.last().map(|g| g.gid);
    assert_ne!(
        first_logical,
        Some(nominal[0]),
        "initial meem is not the isolated meem"
    );
    let lone = shape("\u{645}", 14.0, false, Family::Sans);
    assert_eq!(
        lone.glyphs.last().map(|g| g.gid),
        Some(nominal[0]),
        "control: isolated meem is nominal"
    );
}

#[test]
fn rtl_run_is_reversed_and_an_ltr_run_is_not() {
    let alef_dal = "\u{627}\u{62F}";
    let nominal = nominal_gids(FontId::ArabicRegular, alef_dal);
    let rtl = shape(alef_dal, 14.0, false, Family::Sans);
    let got: Vec<u16> = rtl.glyphs.iter().map(|g| g.gid).collect();
    assert_eq!(got, vec![nominal[1], nominal[0]], "dal draws left of alef");
    assert!(rtl.glyphs[0].x < rtl.glyphs[1].x);

    let ltr = shape("ab", 14.0, false, Family::Sans);
    let got: Vec<u16> = ltr.glyphs.iter().map(|g| g.gid).collect();
    assert_eq!(got, nominal_gids(FontId::Latin, "ab"));
}

#[test]
fn mixed_latin_and_arabic_pick_their_own_fonts_in_bidi_order() {
    let ltr_base = format!("abc {MARHABA} def");
    let s = shape(&ltr_base, 14.0, false, Family::Sans);
    let fonts = fonts_of(&s);
    assert_eq!(fonts.first(), Some(&FontId::Latin));
    assert_eq!(fonts.last(), Some(&FontId::Latin));
    let arabic = fonts
        .iter()
        .filter(|f| **f == FontId::ArabicRegular)
        .count();
    assert!(arabic >= 5, "every letter shaped with the Arabic face");
    let parts = text_width(MARHABA, 14.0) + text_width("abc", 14.0) + text_width("def", 14.0);
    assert!(s.width > parts, "spaces add to the three runs");
    assert!(s.glyphs.last().unwrap().x > s.glyphs[0].x);

    let rtl_base = format!("{MARHABA} abc");
    let r = shape(&rtl_base, 14.0, false, Family::Sans);
    let fonts = fonts_of(&r);
    assert_eq!(
        fonts.first(),
        Some(&FontId::Latin),
        "Latin run sits left of an RTL paragraph"
    );
    assert_eq!(fonts.last(), Some(&FontId::ArabicRegular));
}

#[test]
fn latin_label_is_unchanged_by_shaping() {
    let label = "Hello, World 123";
    let s = shape(label, 14.0, false, Family::Sans);
    assert_eq!(
        s.glyphs.iter().map(|g| g.gid).collect::<Vec<_>>(),
        nominal_gids(FontId::Latin, label)
    );
    let f = font_for(FontId::Latin).unwrap();
    let scale = 14.0 / f.units_per_em() as f32;
    let expect: f32 = label
        .chars()
        .map(|c| f.glyph_hor_advance(f.glyph_index(c).unwrap()).unwrap() as f32 * scale)
        .sum();
    assert!((s.width - expect).abs() < 0.01);
    assert!((text_width(label, 14.0) - expect).abs() < 0.01);
    assert!(s
        .glyphs
        .iter()
        .all(|g| g.font == FontId::Latin && g.y == 0.0));
}

#[test]
fn bold_arabic_uses_the_real_bold_face_and_bold_latin_stays_faux() {
    let b = shape(MARHABA, 14.0, true, Family::Sans);
    assert!(b.glyphs.iter().all(|g| g.font == FontId::ArabicBold));
    let l = shape("abc", 14.0, true, Family::Sans);
    assert!(l.glyphs.iter().all(|g| g.font == FontId::Latin));
}

#[test]
fn arabic_text_paints_ink_and_right_align_ends_at_the_anchor() {
    let l = rasterize(&[&text(MARHABA, 60.0, TextAlign::Left)], 140, 40, 1.0, 1.0).unwrap();
    let c = rasterize(
        &[&text(MARHABA, 60.0, TextAlign::Center)],
        140,
        40,
        1.0,
        1.0,
    )
    .unwrap();
    let r = rasterize(&[&text(MARHABA, 60.0, TextAlign::Right)], 140, 40, 1.0, 1.0).unwrap();
    let (l0, _, l1, _) = ink(&l, 140, 40).expect("arabic ink");
    let (c0, _, c1, _) = ink(&c, 140, 40).unwrap();
    let (_, _, r1, _) = ink(&r, 140, 40).unwrap();
    assert!(l0 >= 58 && l1 > l0 + 10);
    assert!(c0 < 60 && c1 > 60);
    assert!(
        r1 <= 62,
        "right-aligned Arabic ends at the anchor, got {r1}"
    );
}

#[test]
fn missing_glyph_in_every_face_still_measures_and_draws_nothing_wild() {
    let s = shape("", 14.0, false, Family::Sans);
    assert!(s.glyphs.is_empty() && s.width == 0.0);
    let cjk = shape("\u{4E2D}", 14.0, false, Family::Sans);
    assert_eq!(cjk.glyphs.len(), 1);
}

fn bitmap(font: FontId, c: char) -> Option<GlyphBitmap> {
    let face = font_for(font)?;
    let gid = face.glyph_index(c)?.0;
    rasterize_glyph(font, gid, 40.0, false, (0.0, 0.0))
}

#[test]
fn the_serif_family_draws_its_own_face_and_sans_does_not() {
    let serif = char_fonts("Hg", false, Family::Serif);
    assert_eq!(serif, vec![FontId::Serif, FontId::Serif]);
    let bold = char_fonts("Hg", true, Family::Serif);
    assert_eq!(bold, vec![FontId::SerifBold, FontId::SerifBold]);
    assert_eq!(
        char_fonts("Hg", false, Family::Sans),
        vec![FontId::Latin, FontId::Latin],
        "control: sans stays in the sans face"
    );
    let (a, b) = (bitmap(FontId::Serif, 'H'), bitmap(FontId::Latin, 'H'));
    let (a, b) = (a.expect("serif H"), b.expect("sans H"));
    assert!(
        (a.width, a.height, &a.coverage) != (b.width, b.height, &b.coverage),
        "serif and sans H paint different shapes"
    );
    let again = bitmap(FontId::Serif, 'H').expect("serif H");
    assert_eq!(
        (a.width, a.coverage),
        (again.width, again.coverage),
        "control: one face twice is the same"
    );
}

#[test]
fn a_text_command_with_the_serif_family_lays_out_in_the_serif_face() {
    let at = Vec2 { x: 0.0, y: 0.0 };
    let layout = |family| text_layout("Hg", at, 24.0, TextAlign::Left, false, family).unwrap();
    assert!(layout(Family::Serif)
        .shaped
        .glyphs
        .iter()
        .all(|g| g.font == FontId::Serif));
    assert!(layout(Family::Sans)
        .shaped
        .glyphs
        .iter()
        .all(|g| g.font == FontId::Latin));
}

#[test]
fn serif_arabic_joins_in_the_serif_face() {
    let shaped = shape_in("بيت", 30.0, false, Family::Serif);
    assert!(shaped.glyphs.iter().all(|g| g.font == FontId::Serif));
    let nominal: Vec<u16> = "بيت"
        .chars()
        .filter_map(|c| font_for(FontId::Serif)?.glyph_index(c).map(|g| g.0))
        .collect();
    let gids: Vec<u16> = shaped.glyphs.iter().rev().map(|g| g.gid).collect();
    assert_ne!(gids, nominal, "cursive forms, not the isolated letters");
}

#[test]
fn the_display_and_body_families_shape_in_their_own_faces_and_bold_takes_the_bold_file() {
    for (family, regular, bold) in [
        (Family::Display, FontId::Display, FontId::DisplayBold),
        (Family::Body, FontId::Body, FontId::BodyBold),
    ] {
        for text in ["Hello", "مرحبا بك"] {
            let ids = char_fonts(text, false, family);
            assert!(
                ids.iter().all(|f| *f == regular),
                "{family:?} {text}: {ids:?}"
            );
            let ids = char_fonts(text, true, family);
            assert!(
                ids.iter().all(|f| *f == bold),
                "{family:?} bold {text}: {ids:?}"
            );
            let sans = shape(text, 20.0, false, Family::Sans).width;
            let own = shape(text, 20.0, false, family).width;
            assert!(
                (sans - own).abs() > 0.5,
                "{family:?} {text} is a different width from sans: {sans} {own}"
            );
        }
        let outline = |id: FontId| {
            let face = font_for(id).unwrap();
            let bbox = face
                .glyph_bounding_box(face.glyph_index('H').unwrap())
                .unwrap();
            (bbox.x_min, bbox.x_max, bbox.y_max)
        };
        assert_ne!(
            outline(regular),
            outline(bold),
            "the bold file draws its own shapes"
        );
        assert!(
            !regular.faux_bold() && !bold.faux_bold(),
            "a real bold file is not emboldened by stroking"
        );
    }
    assert_eq!(
        char_fonts("Hello", false, Family::Sans),
        vec![FontId::Latin; 5],
        "control: sans stays in the sans face"
    );
}

#[test]
fn display_and_body_cover_latin_and_the_arabic_letters() {
    for id in [
        FontId::Display,
        FontId::DisplayBold,
        FontId::Body,
        FontId::BodyBold,
    ] {
        let face = font_for(id).unwrap();
        let covered = |range: std::ops::RangeInclusive<u32>| {
            range
                .filter(|c| char::from_u32(*c).is_some_and(|c| face.glyph_index(c).is_some()))
                .count()
        };
        assert!(
            "AZaz09 .,".chars().all(|c| face.glyph_index(c).is_some()),
            "{id:?} Latin"
        );
        let letters = covered(0x621..=0x64A);
        println!("{id:?}: arabic letters {letters}/37, marks 064B-0652 {}/8, digits 0660-0669 {}/10, lam-alef U+FEFB {}", covered(0x64B..=0x652), covered(0x660..=0x669), covered(0xFEFB..=0xFEFB));
        assert_eq!(
            letters, 37,
            "{id:?} carries every assigned Arabic letter (U+063B-063F are unassigned)"
        );
    }
}

const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

fn colored(text: &str, runs: Vec<ColorRun>) -> DrawCommand {
    DrawCommand::Text {
        text: text.into(),
        pos: Vec2 { x: 4.0, y: 30.0 },
        size: 24.0,
        color: RED,
        align: TextAlign::Left,
        weight: 400,
        halo: None,
        family: orbital_canvas_base::packet::TextFamily::Sans,
        color_runs: runs,
        rotation: 0.0,
    }
}

#[test]
fn a_run_inside_right_to_left_text_colours_the_glyphs_of_its_characters() {
    let text = "aسلمb";
    let shaped = shape(text, 24.0, false, Family::Sans);
    let colors = glyph_colors(
        text,
        &shaped,
        RED,
        &[ColorRun {
            from: 1,
            to: 4,
            color: BLUE,
        }],
    );
    let first = shaped.clusters.iter().find(|c| c.bytes.start == 0).unwrap();
    let last = shaped
        .clusters
        .iter()
        .find(|c| c.bytes.end == text.len())
        .unwrap();
    let (a, b) = (
        shaped.glyphs[first.glyphs.start].x,
        shaped.glyphs[last.glyphs.start].x,
    );
    let blue: Vec<usize> = (0..colors.len()).filter(|i| colors[*i] == BLUE).collect();
    assert_eq!(
        blue.len(),
        colors.len() - 2,
        "every glyph but a and b: {colors:?}"
    );
    assert!(
        blue.iter()
            .all(|i| shaped.glyphs[*i].x > a && shaped.glyphs[*i].x < b),
        "the Arabic glyphs sit between a and b"
    );
    assert_eq!(colors[first.glyphs.start], RED);
    assert_eq!(colors[last.glyphs.start], RED);
    let plain = glyph_colors(text, &shaped, RED, &[]);
    assert!(
        plain.iter().all(|c| *c == RED),
        "control: no runs, every glyph in the base colour"
    );
    let tail = glyph_colors(
        text,
        &shaped,
        RED,
        &[ColorRun {
            from: 4,
            to: 5,
            color: BLUE,
        }],
    );
    assert_eq!(
        tail.iter().filter(|c| **c == BLUE).count(),
        1,
        "control: a run on b alone colours one glyph"
    );
}

#[test]
fn the_cpu_text_layer_fills_each_glyph_in_its_runs_colour() {
    let cell = shape("W", 24.0, false, Family::Sans).width;
    let drawn =
        |runs: Vec<ColorRun>| rasterize(&[&colored("WWW", runs)], 100, 50, 1.0, 1.0).unwrap();
    let inner = |data: &[u8], k: u32| -> Vec<[u8; 4]> {
        let x0 = (4.0 + cell * (k as f32 + 0.25)) as u32;
        let x1 = (4.0 + cell * (k as f32 + 0.75)) as u32;
        (0..50)
            .flat_map(|y| (x0..x1).map(move |x| (x, y)))
            .map(|(x, y)| at(data, 100, x, y))
            .filter(|p| p[3] == 255)
            .collect()
    };
    let striped = drawn(vec![ColorRun {
        from: 1,
        to: 2,
        color: BLUE,
    }]);
    for (k, want) in [
        (0, [255, 0, 0, 255]),
        (1, [0, 0, 255, 255]),
        (2, [255, 0, 0, 255]),
    ] {
        let ink = inner(&striped, k);
        assert!(!ink.is_empty(), "glyph {k} inks");
        assert!(ink.iter().all(|p| *p == want), "glyph {k} is {want:?}");
    }
    let plain = drawn(Vec::new());
    assert!(
        inner(&plain, 1).iter().all(|p| *p == [255, 0, 0, 255]),
        "control: without runs the middle glyph is the base colour"
    );
}

#[test]
fn a_theme_face_draws_from_its_file_once_loaded_and_from_its_generic_face_before() {
    let _font = FONT_GENERATION_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let url = "test://fonts/theme-face-mono.ttf";
    let family =
        orbital_canvas_base::faces::register_face("Theme Face Test", url, Family::Serif).unwrap();
    crate::asset::register_asset("test://fonts/theme-face-other.ttf", MONO_BYTES.into(), None);
    let before = shape("Ab", 16.0, false, family);
    assert!(
        before.glyphs.iter().all(|g| g.font == FontId::Serif),
        "not loaded yet: the generic serif draws, {:?}",
        fonts_of(&before)
    );
    let generation = font_generation();
    crate::asset::register_asset(url, MONO_BYTES.into(), None);
    let after = shape("Ab", 16.0, false, family);
    assert!(
        after
            .glyphs
            .iter()
            .all(|g| matches!(g.font, FontId::Url(_))),
        "{:?}",
        fonts_of(&after)
    );
    assert!(
        font_generation() > generation,
        "a loaded face repaints text"
    );
    let bold = shape("Ab", 16.0, true, family);
    let (regular_id, bold_id) = (after.glyphs[0].font, bold.glyphs[0].font);
    assert!(
        matches!(bold_id, FontId::Url(_)) && bold_id != regular_id,
        "bold is its own face entry: {regular_id:?} {bold_id:?}"
    );
    assert!(
        bold_id.faux_bold(),
        "control: a font with no weight axis is emboldened by stroking"
    );
    let plain = shape("Ab", 16.0, false, Family::Sans);
    assert!(
        plain.glyphs.iter().all(|g| g.font == FontId::Latin),
        "control: other text keeps the bundled face"
    );
}

fn word_turned(rotation: f32) -> DrawCommand {
    DrawCommand::Text {
        color_runs: Vec::new(),
        rotation,
        text: "Orb".into(),
        pos: Vec2 { x: 100.0, y: 50.0 },
        size: 20.0,
        color: RED,
        align: TextAlign::Left,
        weight: 400,
        halo: None,
        family: orbital_canvas_base::packet::TextFamily::Sans,
    }
}

#[test]
fn rotated_text_is_bounded_by_its_rotated_outlines_and_an_unrotated_one_is_unchanged() {
    let RunBounds::Box { x0, y0, x1, y1 } = command_bounds(&word_turned(0.0)) else {
        panic!("text has a box");
    };
    let RunBounds::Box {
        x0: tx0,
        y0: ty0,
        x1: tx1,
        y1: ty1,
    } = command_bounds(&word_turned(std::f32::consts::FRAC_PI_2))
    else {
        panic!("turned text has a box");
    };
    assert!(
        ((x1 - x0) - (ty1 - ty0)).abs() < 1e-2,
        "width becomes height"
    );
    assert!(
        ((y1 - y0) - (tx1 - tx0)).abs() < 1e-2,
        "height becomes width"
    );
}

#[test]
fn rotated_text_paints_ink_where_the_turned_glyphs_fall_and_not_where_they_were() {
    let paint_at = |rotation: f32| {
        let mut pixmap = Pixmap::new(200, 120).unwrap();
        draw_command(&mut pixmap, &word_turned(rotation), Transform::identity());
        pixmap
    };
    let ink = |pixmap: &Pixmap, x0: u32, y0: u32, x1: u32, y1: u32| {
        (y0..y1).any(|y| (x0..x1).any(|x| pixmap.data()[((y * 200 + x) * 4 + 3) as usize] > 0))
    };
    let flat = paint_at(0.0);
    let turned = paint_at(std::f32::consts::FRAC_PI_2);
    assert!(
        ink(&flat, 100, 35, 130, 55),
        "control: flat text inks to the right of its anchor"
    );
    assert!(
        !ink(&flat, 80, 80, 120, 110),
        "control: flat text leaves the area below blank"
    );
    assert!(
        ink(&turned, 80, 80, 120, 110),
        "turned text inks below its anchor"
    );
    assert!(
        !ink(&turned, 125, 35, 140, 55),
        "turned text leaves the old line blank"
    );
}

#[test]
fn shaping_is_kept_by_its_inputs_until_a_font_or_asset_arrives() {
    let mut cache = shaping::ShapeCache::default();
    let key =
        |size: f32, bold: bool| ("cache probe".to_owned(), size.to_bits(), bold, Family::Sans);
    let shaped = shape_uncached("cache probe", 23.5, false, Family::Sans);
    assert!(
        cache.get((1, 1), &key(23.5, false)).is_none(),
        "never shaped"
    );
    cache.put((1, 1), key(23.5, false), &shaped);
    let kept = cache.get((1, 1), &key(23.5, false)).expect("kept");
    assert_eq!(kept.width, shaped.width);
    assert_eq!(kept.glyphs.len(), shaped.glyphs.len());
    assert!(
        cache.get((1, 1), &key(23.5, true)).is_none(),
        "control: bold is its own entry"
    );
    assert!(
        cache.get((1, 1), &key(24.0, false)).is_none(),
        "control: another size is its own entry"
    );
    assert!(
        cache.get((1, 2), &key(23.5, false)).is_none(),
        "an arrived asset clears what was shaped before it"
    );
    assert_eq!(cache.len(), 0);
    cache.put((1, 1), key(23.5, false), &shaped);
    assert_eq!(
        cache.len(),
        0,
        "control: a result shaped under old fonts is not kept"
    );
}

#[test]
fn cached_shaping_returns_the_glyphs_fresh_shaping_does() {
    let text = "cache probe ٱلسلام 37";
    let fresh = shape_uncached(text, 23.5, false, Family::Sans);
    for _ in 0..2 {
        let shaped = shape_in(text, 23.5, false, Family::Sans);
        assert_eq!(shaped.width, fresh.width);
        assert!(shaped
            .glyphs
            .iter()
            .zip(&fresh.glyphs)
            .all(|(a, b)| a.gid == b.gid && a.x == b.x && a.y == b.y && a.font == b.font));
        assert_eq!(shaped.clusters.len(), fresh.clusters.len());
    }
}
