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
        clock: AnimationClock::Time,
        morph: Vec::new(),
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

fn centre(p: &GameRenderPacket, i: usize) -> (f32, f32) {
    match &p.draw_commands[i] {
        DrawCommand::Path { segments, .. } => match segments[0] {
            PathSegment::Arc { cx, cy, .. } => (cx, cy),
            _ => panic!("not an arc"),
        },
        _ => panic!("not a path"),
    }
}

fn scrolled(from: f32, to: f32) -> Animation {
    Animation {
        delay_ms: 0.0,
        easing: Easing::Linear,
        clock: AnimationClock::Scroll { from, to },
        keyframes: vec![
            Keyframe {
                offset: 0.0,
                opacity: Some(0.0),
                offset_y: Some(40.0),
                ..Keyframe::default()
            },
            Keyframe {
                offset: 1.0,
                opacity: Some(1.0),
                offset_y: Some(0.0),
                ..Keyframe::default()
            },
        ],
        ..pulse()
    }
}

#[test]
fn a_scroll_clocked_animation_follows_the_scroll_offset_and_holds_outside_its_range() {
    let a = scrolled(100.0, 300.0);
    let before = a
        .sample_at(99_999.0, 0.0)
        .expect("held at the start before the range");
    assert_eq!((before.opacity, before.offset_y), (0.0, 40.0));
    let mid = a.sample_at(0.0, 200.0).unwrap();
    assert!((mid.opacity - 0.5).abs() < 1e-4 && (mid.offset_y - 20.0).abs() < 1e-4);
    let after = a.sample_at(0.0, 900.0).unwrap();
    assert_eq!((after.opacity, after.offset_y), (1.0, 0.0));
    let back = a.sample_at(0.0, 150.0).unwrap();
    assert!(
        (back.opacity - 0.25).abs() < 1e-4,
        "scrolling back plays it backwards"
    );
}

#[test]
fn a_time_clocked_animation_ignores_the_scroll_offset() {
    let a = Animation {
        delay_ms: 0.0,
        ..pulse()
    };
    assert_eq!(
        a.sample_at(500.0, 0.0),
        a.sample_at(500.0, 4000.0),
        "control"
    );
    assert_eq!(a.sample_at(500.0, 0.0), a.sample(500.0));
}

#[test]
fn a_degenerate_scroll_range_is_a_step_at_its_offset() {
    let a = scrolled(200.0, 200.0);
    assert_eq!(a.sample_at(0.0, 199.0).unwrap().opacity, 0.0);
    assert_eq!(a.sample_at(0.0, 200.0).unwrap().opacity, 1.0);
}

#[test]
fn keyframe_offsets_translate_the_range_and_resolving_plays_at_the_packet_scroll() {
    let mut p = packet(vec![circle(40.0), circle(10.0)], vec![scrolled(0.0, 100.0)]);
    p.scroll = 50.0;
    let at = p.resolved(0.0, true);
    assert_eq!(centre(&at, 0), (70.0, 90.0), "half of 40 below the rest");
    assert!((alpha(&at, 0) - 0.5).abs() < 1e-4);
    assert_eq!(centre(&at, 1), (70.0, 70.0), "control: outside the range");
    p.scroll = 100.0;
    assert_eq!(centre(&p.resolved(0.0, true), 0), (70.0, 70.0));
}

#[test]
fn only_time_clocked_motion_asks_for_frames() {
    let by_scroll = packet(vec![circle(40.0)], vec![scrolled(0.0, 100.0)]);
    assert!(!by_scroll.is_animated(), "a still page needs no frames");
    assert!(by_scroll.has_animations());
    let by_time = packet(vec![circle(40.0)], vec![pulse()]);
    assert!(by_time.is_animated(), "control");
    assert!(
        matches!(by_scroll.resolved(0.0, false), Cow::Borrowed(_)),
        "reduced motion: rest frame"
    );
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
            color_runs: Vec::new(),
            text: "x".into(),
            pos: Vec2 { x: 70.0, y: 40.0 },
            size: 10.0,
            color: RED,
            align: Default::default(),
            weight: 400,
            family: crate::packet::TextFamily::Sans,
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

#[test]
fn affine_group_transform_carries_quad_sprite_and_constant_width_stroke() {
    let mut commands = vec![
        DrawCommand::Quad {
            rect: crate::packet::Rect {
                x: 1.0,
                y: 0.0,
                w: 2.0,
                h: 1.0,
            },
            color: RED,
        },
        DrawCommand::Sprite {
            texture: "icon.png".into(),
            src: crate::packet::Rect {
                x: 0.0,
                y: 0.0,
                w: 2.0,
                h: 2.0,
            },
            dst: crate::packet::Rect {
                x: 0.0,
                y: 0.0,
                w: 2.0,
                h: 2.0,
            },
            rotation: 0.0,
            opacity: 1.0,
            flip_x: false,
            flip_y: false,
        },
        DrawCommand::Path {
            segments: vec![
                PathSegment::MoveTo { x: 0.0, y: 0.0 },
                PathSegment::LineTo { x: 1.0, y: 0.0 },
            ],
            fill: None,
            stroke: Some(Stroke {
                color: RED,
                width: 3.0,
                dash: vec![2.0, 1.0],
                dash_offset: 0.0,
                cap: LineCap::Butt,
                join: LineJoin::Miter,
            }),
        },
    ];
    transform_group_commands(
        &mut commands,
        Vec2 { x: 0.0, y: 0.0 },
        Vec2 { x: 5.0, y: 7.0 },
        2.0,
        std::f32::consts::FRAC_PI_2,
        0.5,
    );
    let DrawCommand::Poly { points, color } = &commands[0] else {
        panic!("rotated quad")
    };
    assert!((points[0].x - 5.0).abs() < 1e-4 && (points[0].y - 9.0).abs() < 1e-4);
    assert_eq!(color[3], 0.5);
    let DrawCommand::Sprite {
        dst,
        rotation,
        opacity,
        ..
    } = &commands[1]
    else {
        panic!("sprite")
    };
    assert!((dst.x - 1.0).abs() < 1e-4 && (dst.y - 7.0).abs() < 1e-4);
    assert_eq!((dst.w, dst.h), (4.0, 4.0));
    assert!((*rotation - std::f32::consts::FRAC_PI_2).abs() < 1e-4);
    assert_eq!(*opacity, 0.5);
    let DrawCommand::Path {
        stroke: Some(stroke),
        ..
    } = &commands[2]
    else {
        panic!("path stroke")
    };
    assert_eq!(stroke.width, 3.0);
    assert_eq!(stroke.dash, vec![2.0, 1.0]);
    assert_eq!(stroke.color[3], 0.5);
}

#[test]
fn an_sdf_shape_rides_the_group_transform_about_its_centre() {
    let sdf = DrawCommand::Sdf {
        kind: crate::packet::SdfKind::Rect,
        rect: crate::packet::Rect {
            x: 10.0,
            y: 10.0,
            w: 20.0,
            h: 10.0,
        },
        corners: [2.0; 4],
        corner_kind: 0,
        arc: [0.0; 3],
        polygon: [0.0; 2],
        fill: RED,
        stroke: Some(crate::packet::SdfStroke {
            color: RED,
            width: 3.0,
            dash: [0.0; 2],
            dash_offset: 0.0,
        }),
        shadow: Some(crate::packet::SdfShadow {
            color: RED,
            sigma: 4.0,
            offset: Vec2::default(),
        }),
        rotation: 0.0,
        opacity: 1.0,
        material: None,
    };
    let mut moved = vec![sdf.clone()];
    transform_group_commands(
        &mut moved,
        Vec2::default(),
        Vec2 { x: 5.0, y: 0.0 },
        2.0,
        0.0,
        0.5,
    );
    let DrawCommand::Sdf {
        rect,
        corners,
        fill,
        stroke,
        shadow,
        rotation,
        opacity,
        ..
    } = &moved[0]
    else {
        panic!("sdf")
    };
    assert!(
        (rect.x - 25.0).abs() < 1e-4 && (rect.y - 20.0).abs() < 1e-4,
        "{rect:?}"
    );
    assert_eq!((rect.w, rect.h), (40.0, 20.0));
    assert_eq!(*corners, [4.0; 4]);
    assert_eq!(fill[3], 0.5);
    assert_eq!(stroke.unwrap().width, 3.0, "group strokes keep their width");
    assert_eq!(shadow.unwrap().sigma, 8.0);
    assert_eq!((*rotation, *opacity), (0.0, 0.5));
    let mut turned = vec![sdf.clone()];
    transform_group_commands(
        &mut turned,
        Vec2::default(),
        Vec2::default(),
        1.0,
        std::f32::consts::FRAC_PI_2,
        1.0,
    );
    let DrawCommand::Sdf { rect, rotation, .. } = &turned[0] else {
        panic!("sdf")
    };
    assert!((rotation - std::f32::consts::FRAC_PI_2).abs() < 1e-4);
    assert!(
        (rect.x + rect.w / 2.0 + 15.0).abs() < 1e-3 && (rect.y + rect.h / 2.0 - 20.0).abs() < 1e-3,
        "centre (20,15) turns to (-15,20): {rect:?}"
    );
    let mut still = vec![sdf.clone()];
    transform_group_commands(&mut still, Vec2::default(), Vec2::default(), 1.0, 0.0, 1.0);
    assert_eq!(
        format!("{:?}", still[0]),
        format!("{sdf:?}"),
        "control: the identity transform changes nothing"
    );
}

#[test]
fn a_blend_rides_the_group_transform_with_its_radius_scaled() {
    let member = |x: f32| DrawCommand::Sdf {
        kind: crate::packet::SdfKind::Ellipse,
        rect: crate::packet::Rect {
            x,
            y: 0.0,
            w: 10.0,
            h: 10.0,
        },
        corners: [0.0; 4],
        corner_kind: 0,
        arc: [0.0; 3],
        polygon: [0.0; 2],
        fill: RED,
        stroke: None,
        shadow: None,
        rotation: 0.0,
        opacity: 1.0,
        material: None,
    };
    let mut commands = vec![DrawCommand::SdfBlend {
        first: Box::new(member(0.0)),
        second: Box::new(member(20.0)),
        radius: 3.0,
    }];
    transform_group_commands(
        &mut commands,
        Vec2::default(),
        Vec2 { x: 1.0, y: 0.0 },
        2.0,
        0.0,
        1.0,
    );
    let DrawCommand::SdfBlend {
        first,
        second,
        radius,
    } = &commands[0]
    else {
        panic!("blend")
    };
    assert_eq!(*radius, 6.0);
    let (DrawCommand::Sdf { rect: a, .. }, DrawCommand::Sdf { rect: b, .. }) =
        (first.as_ref(), second.as_ref())
    else {
        panic!("members")
    };
    assert_eq!((a.x, a.w), (1.0, 20.0));
    assert_eq!((b.x, b.w), (41.0, 20.0));
}
