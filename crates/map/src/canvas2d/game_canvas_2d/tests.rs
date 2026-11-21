use super::*;
use orbital_canvas_base::packet::TextFamily;
use serde_json::json;

fn packet_for(props: Value) -> GameRenderPacket {
    from_props(&props).unwrap()
}

#[test]
fn shape_gradients_use_shared_vector_commands_and_world_coordinates() {
    let gradient = json!({
        "kind": "linear", "from": {"x": 0, "y": 0}, "to": {"x": 256, "y": 0},
        "stops": [{"offset": 0, "color": "#ff0000"}, {"offset": 1, "color": "#0000ff"}]
    });
    let p = packet_for(json!({
        "projection": "free", "scale": 1.0,
        "drawables": [
            {"type": "draw-shape", "shape": "rect", "position": {"x": 1, "y": 1}, "width": 256, "height": 256, "gradient": gradient},
            {"type": "draw-shape", "shape": "ellipse", "position": {"x": 2, "y": 1}, "radiusX": 128, "radiusY": 128, "gradient": gradient},
            {"type": "draw-shape", "shape": "poly", "position": {"x": 3, "y": 1}, "points": [{"x": 0, "y": 0}, {"x": 256, "y": 0}, {"x": 0, "y": 256}], "gradient": gradient},
            {"type": "draw-shape", "shape": "path", "position": {"x": 4, "y": 1}, "d": "M0 0 L256 0 L256 256 Z", "gradient": gradient}
        ]
    }));
    let gradients: Vec<_> = p
        .draw_commands
        .iter()
        .filter_map(|cmd| match cmd {
            DrawCommand::GradientPath {
                fill_gradient: Some(Gradient::Linear { from, to, .. }),
                ..
            } => Some((*from, *to)),
            _ => None,
        })
        .collect();
    assert_eq!(gradients.len(), 4);
    assert!((gradients[0].1.x - gradients[0].0.x - TILE_WIDTH).abs() < 0.01);
}

#[test]
fn shape_fill_pattern_paints_between_fill_and_stroke() {
    let p = packet_for(json!({
        "projection": "free", "scale": 1.0,
        "drawables": [{"type": "draw-shape", "shape": "rect",
            "position": {"x": 0, "y": 0}, "width": 1, "height": 1,
            "fill": "#ffffff", "stroke": "#ff0000",
            "fillPattern": {"kind": "noise", "alpha": 0.5, "color": "#000000"}}]
    }));
    let styles: Vec<_> = p
        .draw_commands
        .iter()
        .filter_map(|command| match command {
            DrawCommand::GradientPath { fill, .. } if fill.is_some() => Some("fill"),
            DrawCommand::PatternPath { .. } => Some("pattern"),
            DrawCommand::GradientPath { fill, stroke, .. }
                if fill.is_none() && stroke.is_some() =>
            {
                Some("stroke")
            }
            _ => None,
        })
        .collect();
    assert_eq!(styles, ["fill", "pattern", "stroke"]);
}

#[test]
fn draw_sprite_sheet_uses_host_clock_and_authored_clock_override() {
    let data_url = |mime: &str, data: &str| {
        format!(
            "data:{mime},{}",
            data.bytes()
                .map(|byte| format!("%{byte:02X}"))
                .collect::<String>()
        )
    };
    let sheet = data_url(
        "image/svg+xml",
        "<svg xmlns='http://www.w3.org/2000/svg' width='32' height='36'/>",
    );
    let manifest = data_url(
        "application/json",
        r#"{
        "frameWidth": 8, "frameHeight": 12, "columns": 4,
        "animations": {"walk": {"row": 2, "frames": 3, "frameRate": 10, "loop": true}}
    }"#,
    );
    let mut scene = json!({
        "projection": "free", "scale": 1.0,
        "drawables": [{"type": "draw-sprite", "position": {"x": 0, "y": 0},
            "asset": {"url": sheet, "atlas": manifest}, "animation": "walk"}]
    });
    assert!(has_active_fx(&scene));
    let frame = |scene: &Value, time| {
        from_props_at(scene, time)
            .unwrap()
            .draw_commands
            .into_iter()
            .find_map(|command| match command {
                DrawCommand::Sprite { src, .. } => Some(src),
                _ => None,
            })
            .expect("sprite frame")
    };
    assert_eq!(frame(&scene, 150.0).x, 8.0);
    scene["drawables"][0]["clockMs"] = json!(350.0);
    scene["drawables"][0]["loop"] = json!(false);
    let stopped = frame(&scene, 150.0);
    assert_eq!(
        (stopped.x, stopped.y, stopped.w, stopped.h),
        (16.0, 24.0, 8.0, 12.0)
    );
    scene["drawables"][0]["loop"] = json!(true);
    assert_eq!(frame(&scene, 150.0).x, 0.0);
}

fn shadowed_disc(blur: f64) -> GameRenderPacket {
    packet_for(json!({
        "projection": "free", "scale": 1.0,
        "drawables": [
            {"type": "draw-shape", "shape": "ellipse", "position": {"x": 1, "y": 1},
                "anchor": "center", "radiusX": 128, "fill": "#ffffff",
                "shadow": {"color": "#00ff00", "blur": blur}}
        ]
    }))
}

#[test]
fn a_shape_shadow_draws_its_coverage_blurred_under_the_shape() {
    let packet = shadowed_disc(64.0);
    let [DrawCommand::Clear { .. }, DrawCommand::Shadow {
        commands,
        color,
        sigma,
    }, shape] = packet.draw_commands.as_slice()
    else {
        panic!("expected clear, shadow, shape: {:?}", packet.draw_commands);
    };
    assert_eq!(*color, [0.0, 1.0, 0.0, 1.0]);
    // canvas shadowBlur is twice the Gaussian sigma; blur is in world units
    assert_eq!(
        *sigma,
        0.25 * orbital_canvas_base::projector::TILE_WIDTH / 2.0
    );
    assert_eq!(format!("{commands:?}"), format!("{:?}", [shape]));
}

#[test]
fn control_a_shadow_without_blur_draws_only_the_shape() {
    let packet = shadowed_disc(0.0);
    assert_eq!(packet.draw_commands.len(), 2);
    assert!(!matches!(
        packet.draw_commands[1],
        DrawCommand::Shadow { .. }
    ));
}

#[test]
fn blur_is_scoped_and_unsupported_sprite_shadow_and_blend_do_not_paint_base() {
    let scene = json!({
        "projection": "free", "scale": 1.0,
        "drawables": [
            {"type": "draw-shape", "shape": "rect", "position": {"x": 0, "y": 0},
                "width": 1, "height": 1, "fill": "#ffffff", "blur": 4},
            {"type": "draw-shape", "shape": "rect", "position": {"x": 1, "y": 0},
                "width": 1, "height": 1, "fill": "#ffffff", "blendMode": "multiply"},
            {"type": "draw-sprite", "position": {"x": 3, "y": 0},
                "asset": {"url": "data:image/svg+xml,%3Csvg%20xmlns%3D%27http%3A%2F%2Fwww.w3.org%2F2000%2Fsvg%27%20width%3D%271%27%20height%3D%271%27%2F%3E"},
                "shadow": {"color": "#000000", "blur": 1}}
        ]
    });
    let packet = packet_for(scene);
    assert_eq!(packet.draw_commands.len(), 2);
    assert!(matches!(packet.draw_commands[0], DrawCommand::Clear { .. }));
    assert!(matches!(
        packet.draw_commands[1],
        DrawCommand::Blurred { .. }
    ));
}

#[test]
fn text_atom_and_layer_use_the_shared_vector_font_pipeline() {
    let p = packet_for(json!({
        "projection": "free",
        "scale": 1.0,
        "drawables": [
            { "type": "draw-text", "text": "one", "position": {"x": 1, "y": 2},
              "color": "#ffffff", "font": "bold 14px monospace", "align": "center",
              "baseline": "middle", "opacity": 0.5 },
            { "type": "draw-text-layer", "items": [
                { "type": "draw-text", "text": "two", "position": {"x": 2, "y": 3},
                  "color": "#000000" }
            ] }
        ]
    }));
    let texts: Vec<_> = p
        .draw_commands
        .iter()
        .filter_map(|cmd| match cmd {
            DrawCommand::Text {
                text,
                size,
                color,
                align,
                weight,
                family,
                ..
            } => Some((text.as_str(), *size, color[3], *align, *weight, *family)),
            _ => None,
        })
        .collect();
    assert_eq!(texts.len(), 2);
    assert_eq!(
        texts[0],
        ("one", 14.0, 0.5, TextAlign::Center, 700, TextFamily::Mono)
    );
    assert_eq!(
        texts[1],
        ("two", 10.0, 1.0, TextAlign::Left, 400, TextFamily::Sans)
    );
}

#[test]
fn a_serif_font_yields_a_serif_text_command_and_no_font_the_default_family() {
    let family_of = |font: Option<&str>| {
        let mut atom = json!({ "type": "draw-text", "text": "x", "position": {"x": 1, "y": 2},
            "color": "#ffffff" });
        if let Some(f) = font {
            atom["font"] = json!(f);
        }
        let p = packet_for(json!({
            "projection": "free", "scale": 1.0, "drawables": [atom]
        }));
        p.draw_commands
            .iter()
            .find_map(|c| match c {
                DrawCommand::Text { family, .. } => Some(*family),
                _ => None,
            })
            .expect("a text command")
    };
    assert_eq!(family_of(Some("400 16px serif")), TextFamily::Serif);
    assert_eq!(family_of(Some("400 16px sans-serif")), TextFamily::Sans);
    assert_eq!(family_of(None), TextFamily::Sans);
    assert_eq!(
        parse_text_font(Some("700 20px serif")),
        Some((20.0, 700, TextFamily::Serif))
    );
}

fn editable(extra: Value) -> Value {
    let mut text = json!({ "type": "draw-text", "id": "t", "text": "ab", "position": {"x": 0, "y": 0}, "anchor": "top-left",
        "color": "#ffffff", "font": "20px sans-serif", "align": "left", "baseline": "middle" });
    for (k, v) in extra.as_object().unwrap() {
        text[k] = v.clone();
    }
    json!({ "projection": "free", "scale": 1.0, "drawables": [text] })
}

fn quads(p: &GameRenderPacket) -> Vec<Rect> {
    p.draw_commands
        .iter()
        .filter_map(|c| match c {
            DrawCommand::Quad { rect, .. } => Some(*rect),
            _ => None,
        })
        .collect()
}

#[test]
fn a_caret_is_a_bar_at_the_shaped_width_of_the_text_before_it() {
    let p = packet_for(editable(json!({ "caret": 1 })));
    let q = quads(&p);
    assert_eq!(q.len(), 1, "one caret bar: {:?}", p.draw_commands);
    let before = orbital_canvas_paint::vector::shape("a", 20.0, false, TextFamily::Sans).width;
    assert!(
        (q[0].x + q[0].w / 2.0 - before).abs() < 0.01,
        "the bar stands after 'a': {:?} vs {before}",
        q[0]
    );
    assert!(
        quads(&packet_for(editable(json!({ "caret": -1 })))).is_empty(),
        "control: no caret, no bar"
    );
    assert!(
        quads(&packet_for(editable(json!({})))).is_empty(),
        "control: no decorations at all"
    );
}

#[test]
fn a_selection_band_is_painted_behind_its_characters() {
    let p = packet_for(editable(
        json!({ "selectionStart": 0, "selectionEnd": 2, "selectionColor": "#3366ff" }),
    ));
    let band = p
        .draw_commands
        .iter()
        .position(|c| matches!(c, DrawCommand::Quad { .. }))
        .expect("a band");
    let text = p
        .draw_commands
        .iter()
        .position(|c| matches!(c, DrawCommand::Text { .. }))
        .expect("the text");
    assert!(
        band < text,
        "the band is drawn first, under the text: {:?}",
        p.draw_commands
    );
    let w = orbital_canvas_paint::vector::shape("ab", 20.0, false, TextFamily::Sans).width;
    assert!((quads(&p)[0].w - w).abs() < 0.01);
    assert!(
        quads(&packet_for(editable(
            json!({ "selectionStart": 1, "selectionEnd": 1 })
        )))
        .is_empty(),
        "control: an empty selection paints no band"
    );
}

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

fn text_commands(p: &GameRenderPacket) -> Vec<(String, [f32; 4], Vec<ColorRun>)> {
    p.draw_commands
        .iter()
        .filter_map(|c| match c {
            DrawCommand::Text {
                text,
                color,
                color_runs,
                ..
            } => Some((text.clone(), *color, color_runs.clone())),
            _ => None,
        })
        .collect()
}

fn colour_run_error(extra: Value) -> String {
    match from_props(&editable(extra)).unwrap_err() {
        CanvasSceneError::InvalidProp { field, detail } => {
            assert_eq!(field, "colorRuns", "{detail}");
            detail
        }
        other => panic!("not an invalid prop: {other:?}"),
    }
}

#[test]
fn a_colour_run_colours_exactly_its_characters_and_the_rest_keeps_the_base_colour() {
    let p = packet_for(editable(
        json!({ "text": "abcd", "colorRuns": [{ "from": 1, "to": 3, "color": "#ff0000" }] }),
    ));
    let cmds = text_commands(&p);
    assert_eq!(cmds.len(), 1);
    assert_eq!(cmds[0].1, WHITE, "the node's own colour stays the base");
    assert_eq!(
        cmds[0].2,
        vec![ColorRun {
            from: 1,
            to: 3,
            color: RED
        }]
    );
    let shaped = orbital_canvas_paint::vector::shape("abcd", 20.0, false, TextFamily::Sans);
    assert_eq!(
        orbital_canvas_paint::vector::glyph_colors("abcd", &shaped, WHITE, &cmds[0].2),
        vec![WHITE, RED, RED, WHITE]
    );
}

#[test]
fn control_a_text_without_colour_runs_is_the_packet_it_was_and_serializes_no_runs() {
    let plain = packet_for(editable(json!({ "text": "abcd" })));
    let empty = packet_for(editable(json!({ "text": "abcd", "colorRuns": [] })));
    let null = packet_for(editable(json!({ "text": "abcd", "colorRuns": null })));
    assert_eq!(
        format!("{:?}", plain.draw_commands),
        format!("{:?}", empty.draw_commands)
    );
    assert_eq!(
        format!("{:?}", plain.draw_commands),
        format!("{:?}", null.draw_commands)
    );
    assert!(text_commands(&plain)[0].2.is_empty());
    let wire = serde_json::to_value(&plain.draw_commands).unwrap();
    assert!(!wire.to_string().contains("color_runs"), "{wire}");
}

#[test]
fn adjacent_colour_runs_each_take_their_own_colour() {
    let p = packet_for(editable(json!({ "text": "abcd", "colorRuns": [
        { "from": 0, "to": 2, "color": "#ff0000" }, { "from": 2, "to": 3, "color": "#0000ff" }] })));
    let runs = &text_commands(&p)[0].2;
    assert_eq!(
        runs,
        &vec![
            ColorRun {
                from: 0,
                to: 2,
                color: RED
            },
            ColorRun {
                from: 2,
                to: 3,
                color: BLUE
            }
        ]
    );
    let shaped = orbital_canvas_paint::vector::shape("abcd", 20.0, false, TextFamily::Sans);
    assert_eq!(
        orbital_canvas_paint::vector::glyph_colors("abcd", &shaped, WHITE, runs),
        vec![RED, RED, BLUE, WHITE]
    );
}

#[test]
fn a_colour_run_across_a_wrapped_line_break_splits_into_each_lines_own_indices() {
    let wrapped = |extra: Value| {
        let mut props = json!({ "text": "aaa bbb ccc", "width": 50, "colorRuns": [{ "from": 2, "to": 9, "color": "#ff0000" }] });
        for (k, v) in extra.as_object().unwrap() {
            props[k] = v.clone();
        }
        text_commands(&packet_for(editable(props)))
    };
    let cmds = wrapped(json!({}));
    let lines: Vec<&str> = cmds.iter().map(|c| c.0.as_str()).collect();
    assert_eq!(lines, vec!["aaa", "bbb", "ccc"]);
    assert_eq!(
        cmds[0].2,
        vec![ColorRun {
            from: 2,
            to: 3,
            color: RED
        }]
    );
    assert_eq!(
        cmds[1].2,
        vec![ColorRun {
            from: 0,
            to: 3,
            color: RED
        }]
    );
    assert_eq!(
        cmds[2].2,
        vec![ColorRun {
            from: 0,
            to: 1,
            color: RED
        }],
        "the run ends one character into the last line"
    );
    let none = wrapped(json!({ "colorRuns": [] }));
    assert!(
        none.iter().all(|c| c.2.is_empty()),
        "control: no runs, no run on any line"
    );
}

#[test]
fn a_colour_run_after_a_hard_newline_counts_the_newline_as_a_character() {
    let p = packet_for(editable(
        json!({ "text": "ab\ncd", "width": 200, "colorRuns": [{ "from": 3, "to": 4, "color": "#ff0000" }] }),
    ));
    let cmds = text_commands(&p);
    assert_eq!(cmds[0].0, "ab");
    assert!(cmds[0].2.is_empty());
    assert_eq!(cmds[1].0, "cd");
    assert_eq!(
        cmds[1].2,
        vec![ColorRun {
            from: 0,
            to: 1,
            color: RED
        }]
    );
}

#[test]
fn opacity_applies_to_colour_runs_as_to_the_base_colour() {
    let p = packet_for(editable(
        json!({ "text": "abcd", "opacity": 0.5, "colorRuns": [{ "from": 0, "to": 1, "color": "#ff0000" }] }),
    ));
    let cmd = &text_commands(&p)[0];
    assert_eq!(cmd.1[3], 0.5);
    assert_eq!(cmd.2[0].color, [1.0, 0.0, 0.0, 0.5]);
    let opaque = packet_for(editable(
        json!({ "text": "abcd", "colorRuns": [{ "from": 0, "to": 1, "color": "#ff0000" }] }),
    ));
    assert_eq!(
        text_commands(&opaque)[0].2[0].color[3],
        1.0,
        "control: full opacity keeps the run's own alpha"
    );
}

#[test]
fn a_colour_run_stays_on_its_characters_when_a_preedit_is_shown_before_them() {
    let p = packet_for(editable(
        json!({ "caret": 1, "preedit": "X", "colorRuns": [{ "from": 1, "to": 2, "color": "#ff0000" }] }),
    ));
    let cmd = &text_commands(&p)[0];
    assert_eq!(cmd.0, "aXb");
    assert_eq!(
        cmd.2,
        vec![ColorRun {
            from: 2,
            to: 3,
            color: RED
        }],
        "the run covers b, not the preedit"
    );
}

#[test]
fn invalid_colour_runs_are_scene_errors_naming_colour_runs() {
    let run = |from: i64, to: i64, color: &str| json!({ "from": from, "to": to, "color": color });
    assert!(
        colour_run_error(json!({ "colorRuns": [run(2, 2, "#f00")] })).contains("below"),
        "from == to"
    );
    assert!(
        colour_run_error(json!({ "colorRuns": [run(3, 1, "#f00")] })).contains("below"),
        "inverted"
    );
    assert!(
        colour_run_error(json!({ "colorRuns": [run(0, 3, "#f00")] })).contains("past"),
        "out of range"
    );
    assert!(
        colour_run_error(json!({ "colorRuns": [run(0, 2, "#f00"), run(1, 2, "#00f")] }))
            .contains("overlap"),
        "overlapping"
    );
    assert!(
        colour_run_error(json!({ "colorRuns": [run(1, 2, "#f00"), run(0, 1, "#00f")] }))
            .contains("sorted"),
        "unsorted"
    );
    assert!(
        colour_run_error(json!({ "colorRuns": [run(0, 1, "not-a-colour")] })).contains("color"),
        "bad colour"
    );
    assert!(
        colour_run_error(json!({ "colorRuns": [{ "from": 0, "to": 1 }] })).contains("color"),
        "missing colour"
    );
    assert!(
        colour_run_error(json!({ "colorRuns": [{ "from": -1, "to": 1, "color": "#f00" }] }))
            .contains("from"),
        "negative index"
    );
    assert!(
        colour_run_error(json!({ "colorRuns": "red" })).contains("list"),
        "not a list"
    );
    assert!(
        from_props(&editable(
            json!({ "colorRuns": [run(0, 1, "#f00"), run(1, 2, "#00f")] })
        ))
        .is_ok(),
        "control: touching runs are valid"
    );
}

#[test]
fn a_preedit_is_shown_at_the_caret_underlined_and_the_caret_follows_it() {
    let p = packet_for(editable(json!({ "caret": 1, "preedit": "X" })));
    let text = p.draw_commands.iter().find_map(|c| match c {
        DrawCommand::Text { text, .. } => Some(text.clone()),
        _ => None,
    });
    assert_eq!(text.as_deref(), Some("aXb"));
    let q = quads(&p);
    assert_eq!(
        q.len(),
        2,
        "an underline and the caret: {:?}",
        p.draw_commands
    );
    let after = orbital_canvas_paint::vector::shape("aX", 20.0, false, TextFamily::Sans).width;
    assert!(
        (q[1].x + q[1].w / 2.0 - after).abs() < 0.01,
        "the caret stands after the preedit"
    );
}

#[test]
fn the_caret_blinks_on_the_scene_clock_and_a_caret_keeps_the_scene_drawing() {
    let scene = editable(json!({ "caret": 1 }));
    let at = |ms: f64| quads(&from_props_at(&scene, ms).unwrap()).len();
    assert_eq!(at(100.0), 1, "on in the first 530 ms");
    assert_eq!(at(600.0), 0, "off in the next 530 ms");
    assert_eq!(at(1100.0), 1, "on again");
    assert!(has_active_fx(&scene), "a caret keeps frames coming");
    assert!(
        !has_active_fx(&editable(json!({}))),
        "control: plain text does not"
    );
}

fn edit_hit(extra: Value) -> (Vec<f32>, crate::scene::TextHit) {
    let mut props = extra;
    props["hit"] = json!(true);
    let (_, gathered) = from_props_mapped(&editable(props), 0.0).unwrap();
    let hit = gathered.text_hits.into_iter().next().expect("a text hit");
    (hit.lines[0].edges.clone(), hit)
}

fn sans_width(text: &str) -> f32 {
    orbital_canvas_paint::vector::shape(text, 20.0, false, TextFamily::Sans).width
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.01
}

#[test]
fn pure_arabic_caret_positions_run_right_to_left() {
    let (edges, _) = edit_hit(json!({ "text": "سلم" }));
    let w = sans_width("سلم");
    assert_eq!(edges.len(), 4);
    assert!(
        near(edges[0], w) && near(edges[3], 0.0),
        "the first index is at the right edge, the end at the left: {edges:?}"
    );
    assert!(
        edges.windows(2).all(|p| p[0] > p[1]),
        "strictly leftward: {edges:?}"
    );
    let p = packet_for(editable(json!({ "text": "سلم", "caret": 1 })));
    let q = quads(&p);
    assert_eq!(q.len(), 1, "one caret bar");
    assert!(
        near(q[0].x + q[0].w / 2.0, edges[1]),
        "the bar stands at the index-1 edge: {:?} vs {}",
        q[0],
        edges[1]
    );
}

#[test]
fn a_ligature_shares_its_advance_between_its_characters() {
    let (edges, _) = edit_hit(json!({ "text": "سلام" }));
    assert_eq!(edges.len(), 5);
    assert!(
        near(edges[0], sans_width("سلام")) && near(edges[4], 0.0),
        "{edges:?}"
    );
    assert!(
        edges.windows(2).all(|p| p[0] > p[1]),
        "strictly leftward through the lam-alef ligature: {edges:?}"
    );
}

#[test]
fn a_mixed_line_has_its_carets_in_visual_order_of_the_bidi_runs() {
    let (edges, _) = edit_hit(json!({ "text": "aسلb" }));
    let (wa, run, total) = (sans_width("a"), sans_width("سل"), sans_width("aسلb"));
    assert_eq!(edges.len(), 5);
    assert!(near(edges[0], 0.0) && near(edges[4], total), "{edges:?}");
    assert!(
        near(edges[1], wa),
        "the boundary after 'a' is at the lower-level character's edge: {edges:?}"
    );
    assert!(
        near(edges[3], wa + run),
        "the boundary before 'b' is at b's leading edge: {edges:?}"
    );
    assert!(
        edges[1] < edges[2] && edges[2] < edges[3],
        "inside the run the index sits between: {edges:?}"
    );
}

#[test]
fn a_selection_across_a_direction_boundary_paints_two_rects() {
    let (edges, _) = edit_hit(json!({ "text": "aسلمb" }));
    let wa = sans_width("a");
    let split = quads(&packet_for(editable(
        json!({ "text": "aسلمb", "selectionStart": 2, "selectionEnd": 5 }),
    )));
    assert_eq!(
        split.len(),
        2,
        "the run's left part and 'b' are not visually adjacent: {split:?}"
    );
    let mut rects = split.clone();
    rects.sort_by(|a, b| a.x.total_cmp(&b.x));
    assert!(
        near(rects[0].x, wa) && near(rects[0].x + rects[0].w, edges[2]),
        "{rects:?} {edges:?}"
    );
    assert!(
        near(rects[1].x, edges[4]) && near(rects[1].x + rects[1].w, edges[5]),
        "{rects:?} {edges:?}"
    );
    let whole = quads(&packet_for(editable(
        json!({ "text": "aسلمb", "selectionStart": 0, "selectionEnd": 4 }),
    )));
    assert_eq!(
        whole.len(),
        1,
        "control: a range that stays visually contiguous paints one rect: {whole:?}"
    );
}

#[test]
fn a_pointer_x_maps_back_to_the_logical_index_of_its_caret_edge() {
    for text in ["سلم", "سلام", "aسلb", "aسلمb", "ab"] {
        let (edges, hit) = edit_hit(json!({ "text": text }));
        let y = hit.lines[0].top + 1.0;
        for (i, x) in edges.iter().enumerate() {
            assert_eq!(
                hit.index_at(*x, y),
                Some(i),
                "{text}: index {i} at x {x} in {edges:?}"
            );
        }
    }
}

#[test]
fn left_to_right_text_keeps_its_prefix_width_edges_and_right_alignment_ends_at_the_anchor() {
    let (edges, _) = edit_hit(json!({ "text": "abc" }));
    let expected: Vec<f32> = ["", "a", "ab", "abc"]
        .iter()
        .map(|p| sans_width(p))
        .collect();
    assert!(
        edges.iter().zip(&expected).all(|(a, b)| near(*a, *b)),
        "{edges:?} vs {expected:?}"
    );
    let (right, _) =
        edit_hit(json!({ "text": "abc", "align": "right", "position": {"x": 200, "y": 0} }));
    let w = sans_width("abc");
    assert!(
        near(right[0], 200.0 - w) && near(right[3], 200.0),
        "{right:?}"
    );
    let (rtl, _) =
        edit_hit(json!({ "text": "سلم", "align": "right", "position": {"x": 200, "y": 0} }));
    let w = sans_width("سلم");
    assert!(
        near(rtl[0], 200.0) && near(rtl[3], 200.0 - w),
        "an RTL line ends at the anchor too: {rtl:?}"
    );
}

#[test]
fn a_css_numeric_weight_is_read_like_bold_and_an_out_of_range_one_is_refused() {
    assert_eq!(
        parse_text_font(Some("600 26px sans-serif")),
        Some((26.0, 600, TextFamily::Sans))
    );
    assert_eq!(
        parse_text_font(Some("400 15px monospace")),
        Some((15.0, 400, TextFamily::Mono))
    );
    assert_eq!(
        parse_text_font(Some("bold 12px sans-serif")),
        Some((12.0, 700, TextFamily::Sans))
    );
    assert_eq!(parse_text_font(Some("1200 12px sans-serif")), None);
    assert_eq!(parse_text_font(Some("600 12px Custom")), None);
}

#[test]
fn a_css_family_list_takes_the_first_family_the_pack_carries() {
    let family = |font: &str| parse_text_font(Some(font)).map(|f| (f.1, f.2));
    assert_eq!(
        family("700 26px 'Reem Kufi', serif"),
        Some((700, TextFamily::Display))
    );
    assert_eq!(
        family("16px \"Cairo\", sans-serif"),
        Some((400, TextFamily::Body))
    );
    assert_eq!(family("16px cairo"), Some((400, TextFamily::Body)));
    assert_eq!(family("16px REEM KUFI"), Some((400, TextFamily::Display)));
    assert_eq!(
        family("16px Foo, sans-serif"),
        Some((400, TextFamily::Sans))
    );
    assert_eq!(
        family("16px Foo, Bar, monospace"),
        Some((400, TextFamily::Mono))
    );
    assert_eq!(
        family("16px system-ui, sans-serif"),
        Some((400, TextFamily::Sans))
    );
    assert_eq!(
        family("16px Foo"),
        None,
        "control: no family the pack carries"
    );
    assert_eq!(family("16px Foo, Bar"), None);
}

#[test]
fn unsupported_text_font_and_baseline_do_not_silently_change_contract() {
    let p = packet_for(json!({
        "projection": "free",
        "scale": 1.0,
        "drawables": [
            { "type": "draw-text", "text": "custom", "position": {"x": 1, "y": 2},
              "color": "#ffffff", "font": "12px Custom" },
            { "type": "draw-text", "text": "hanging", "position": {"x": 1, "y": 2},
              "color": "#ffffff", "baseline": "hanging" }
        ]
    }));
    assert!(!p
        .draw_commands
        .iter()
        .any(|cmd| matches!(cmd, DrawCommand::Text { .. })));
}

#[test]
fn stroke_only_shapes_and_svg_paths_reuse_vector_primitives() {
    let p = packet_for(json!({
        "projection": "free",
        "scale": 1.0,
        "drawables": [
            { "type": "draw-shape", "shape": "rect", "position": {"x": 1, "y": 2},
              "width": 1, "height": 1, "stroke": "#ff0000", "strokeWidth": 3,
              "strokeDash": [4, 2], "strokeDashOffset": 1 },
            { "type": "draw-shape", "shape": "path", "position": {"x": 2, "y": 3},
              "d": "M0 0 L1 0 L1 1 Z", "fill": "#00ff00", "stroke": "#0000ff" }
        ]
    }));
    let paths: Vec<_> = p
        .draw_commands
        .iter()
        .filter_map(|cmd| match cmd {
            DrawCommand::Path {
                segments,
                fill,
                stroke,
            } => Some((segments, fill, stroke)),
            _ => None,
        })
        .collect();
    assert_eq!(paths.len(), 2);
    assert!(paths[0].1.is_none());
    let rect_stroke = paths[0].2.as_ref().expect("rect stroke");
    assert_eq!(rect_stroke.width, 3.0);
    assert_eq!(rect_stroke.dash, vec![4.0, 2.0]);
    assert_eq!(rect_stroke.dash_offset, 1.0);
    assert!(matches!(paths[0].0.last(), Some(PathSegment::Close)));
    assert_eq!(paths[1].1.expect("path fill")[1], 1.0);
    assert!(paths[1].2.is_some());
    assert!(paths[1].0.len() >= 4);
}

#[test]
fn draw_shape_rotation_uses_its_world_pivot() {
    let p = packet_for(json!({
        "projection": "free", "scale": 1.0,
        "drawables": [{"type": "draw-shape", "shape": "rect",
            "position": {"x": 0, "y": 0}, "width": 256, "height": 256,
            "fill": "#ff0000", "rotate": std::f32::consts::FRAC_PI_2,
            "pivot": {"x": 128, "y": 128}}]
    }));
    let points = p
        .draw_commands
        .iter()
        .find_map(|cmd| match cmd {
            DrawCommand::Poly { points, .. } => Some(points),
            _ => None,
        })
        .expect("rotated rectangle polygon");
    assert!((points[0].x - 256.0).abs() < 1e-3);
    assert!(points[0].y.abs() < 1e-3);
}

#[test]
fn draw_poly_offsets_are_pixels_in_a_free_scene_like_path_offsets() {
    let p = packet_for(json!({
        "projection": "free", "scale": 1.0,
        "drawables": [{"type": "draw-shape", "shape": "poly",
            "position": {"x": 0, "y": 0},
            "points": [{"x": 0, "y": 0}, {"x": 256, "y": 0}, {"x": 0, "y": 256}],
            "offsetX": 64, "offsetY": -128, "fill": "#ff0000"}]
    }));
    let points = p
        .draw_commands
        .iter()
        .find_map(|command| match command {
            DrawCommand::Poly { points, .. } => Some(points),
            _ => None,
        })
        .expect("filled polygon");
    assert_eq!(points[0].x, 0.25 * TILE_WIDTH);
    assert_eq!(points[0].y, -0.5 * TILE_WIDTH);
}

#[test]
fn draw_shape_animation_repaints_authored_geometry_and_color() {
    let scene = json!({
        "projection": "free", "scale": 1.0,
        "drawables": [{"type": "draw-shape", "shape": "rect",
            "position": {"x": 0, "y": 0}, "height": 256,
            "width": 0, "fill": "#ff0000",
            "animation": {"durationMs": 1000, "loop": false,
                "keyframes": [
                    {"at": 0, "width": 0, "fill": "#ff0000"},
                    {"at": 1, "width": 512, "fill": "#0000ff"}
                ]}}]
    });
    assert!(has_active_fx(&scene));
    let halfway = from_props_at(&scene, 500.0).unwrap();
    let finished = from_props_at(&scene, 1500.0).unwrap();
    let quad = |packet: &GameRenderPacket| {
        packet
            .draw_commands
            .iter()
            .find_map(|command| match command {
                DrawCommand::Quad { rect, color } => Some((*rect, *color)),
                _ => None,
            })
            .expect("animated rectangle")
    };
    let (half_rect, half_color) = quad(&halfway);
    let (final_rect, final_color) = quad(&finished);
    assert_eq!(half_rect.w, TILE_WIDTH);
    assert_eq!(final_rect.w, 2.0 * TILE_WIDTH);
    assert_eq!(half_color[0], 1.0);
    assert_eq!(final_color[2], 1.0);
}

#[test]
fn unresolved_sprite_is_a_typed_error() {
    let scene = json!({
        "projection": "free", "scale": 1.0,
        "drawables": [{"type": "draw-sprite", "position": {"x": 0, "y": 0},
            "width": 1, "height": 0.5, "opacity": 0.5}]
    });
    assert!(matches!(
        from_props(&scene),
        Err(CanvasSceneError::UnresolvedSprite(_))
    ));
    let resolved = json!({
        "projection": "free", "scale": 1.0,
        "drawables": [{"type": "draw-sprite", "position": {"x": 0, "y": 0},
            "asset": {"url": "data:image/svg+xml;utf8,%3Csvg%20xmlns%3D%22http%3A%2F%2Fwww.w3.org%2F2000%2Fsvg%22%20width%3D%2230%22%20height%3D%2210%22%2F%3E"}}]
    });
    assert!(from_props(&resolved).is_ok());
}

#[test]
fn an_image_that_fails_to_load_paints_nothing_and_the_rest_of_the_scene_paints() {
    let bad = "data:image/png;base64,@@@";
    orbital_canvas_paint::asset::ensure_asset(bad);
    let scene = json!({
        "projection": "free", "scale": 1.0,
        "drawables": [
            {"type": "draw-sprite", "id": "photo", "position": {"x": 0, "y": 0}, "anchor": "top-left",
                "width": 40, "height": 40, "asset": {"url": bad}, "semantic": {"role": "img", "name": "A portrait"}},
            {"type": "draw-shape", "shape": "rect", "id": "card", "anchor": "top-left",
                "position": {"x": 0, "y": 50}, "width": 40, "height": 20, "fill": "#ff0000"}]
    });
    let packet = from_props(&scene).expect("one failed image does not refuse the scene");
    assert!(
        !packet
            .draw_commands
            .iter()
            .any(|c| matches!(c, DrawCommand::Sprite { .. })),
        "the failed image paints nothing"
    );
    assert!(
        packet
            .draw_commands
            .iter()
            .any(|c| matches!(c, DrawCommand::Quad { .. })),
        "the rest of the scene paints: {:?}",
        packet.draw_commands
    );
}

fn text_size(scene: &Value) -> f32 {
    let packet = from_props(scene).expect("scene maps");
    packet
        .draw_commands
        .iter()
        .find_map(|c| match c {
            DrawCommand::Text { size, .. } => Some(*size),
            _ => None,
        })
        .expect("a text command")
}

fn label(extra: Value) -> Value {
    let mut node = json!({"type": "draw-text", "id": "t", "text": "The Bootloader of Knowledge",
        "position": {"x": 100, "y": 20}, "anchor": "top-left", "align": "center", "baseline": "middle",
        "color": "#ffffff", "font": "600 20px sans-serif"});
    for (k, v) in extra.as_object().unwrap() {
        node[k] = v.clone();
    }
    json!({"projection": "free", "scale": 1.0, "drawables": [node]})
}

#[test]
fn a_single_line_text_wider_than_its_max_width_is_drawn_smaller_to_fit() {
    let natural = orbital_canvas_paint::vector::shape(
        "The Bootloader of Knowledge",
        20.0,
        true,
        orbital_canvas_base::packet::TextFamily::Sans,
    )
    .width;
    assert!(
        natural > 120.0,
        "the label is wider than the box: {natural}"
    );
    let size = text_size(&label(json!({"maxWidth": 120})));
    assert!(size < 20.0, "drawn smaller: {size}");
    let fitted = orbital_canvas_paint::vector::shape(
        "The Bootloader of Knowledge",
        size,
        true,
        orbital_canvas_base::packet::TextFamily::Sans,
    )
    .width;
    assert!(fitted <= 120.5, "fits its max width: {fitted}");
}

#[test]
fn control_a_text_narrower_than_its_max_width_keeps_its_size() {
    assert_eq!(text_size(&label(json!({"maxWidth": 900}))), 20.0);
    assert_eq!(text_size(&label(json!({}))), 20.0, "no max width");
}

#[test]
fn an_invalid_max_width_is_a_scene_error_naming_it() {
    for bad in [json!(0), json!(-4), json!("wide")] {
        assert!(
            matches!(
                from_props(&label(json!({"maxWidth": bad}))),
                Err(CanvasSceneError::InvalidProp { ref field, .. }) if field == "maxWidth"
            ),
            "{bad}"
        );
    }
    assert!(
        matches!(
            from_props(&label(json!({"maxWidth": 100, "width": 200}))),
            Err(CanvasSceneError::InvalidProp { ref field, .. }) if field == "maxWidth"
        ),
        "a paragraph box has its own width"
    );
}

#[test]
fn skinned_mesh_errors_reach_the_scene_error() {
    let bad = json!({
        "projection": "free", "scale": 1.0,
        "drawables": [{"type": "draw-skinned-mesh", "position": {"x": 0, "y": 0},
            "asset": {"url": "unused"}, "weightsOverlay": true,
            "mesh": {"vertices": [{"x":0,"y":0,"u":0,"v":0,"weights":[{"bone":3,"w":1}]}],
                "triangles": [[0, 0, 0]]},
            "bones": [{"name":"root","pivot":[0,0],"tip":[1,0]}]}]
    });
    assert!(matches!(
        from_props(&bad),
        Err(CanvasSceneError::Skin(
            orbital_canvas_base::skinning_2d::SkinError::BoneIndex { .. }
        ))
    ));
    let no_mesh = json!({
        "projection": "free", "scale": 1.0,
        "drawables": [{"type": "draw-skinned-mesh", "position": {"x": 0, "y": 0},
            "asset": {"url": "unused"}, "weightsOverlay": true,
            "bones": [{"name":"root","pivot":[0,0],"tip":[1,0]}]}]
    });
    assert!(matches!(
        from_props(&no_mesh),
        Err(CanvasSceneError::SkinnedMesh(_))
    ));
}

#[test]
fn fx_layer_errors_reach_the_scene_error() {
    let props = json!({
        "projection": "free", "scale": 1.0,
        "drawables": [{ "type": "draw-fx-layer", "items": [] }]
    });
    assert!(matches!(
        from_props(&props),
        Err(CanvasSceneError::Fx(
            orbital_canvas_base::fx_projection::FxError::Layer {
                field: "tickMs",
                ..
            }
        ))
    ));
}

#[test]
fn background_color_remains_under_camera_tiled_whole_image() {
    let background = "data:image/svg+xml;utf8,%3Csvg%20xmlns%3D%22http%3A%2F%2Fwww.w3.org%2F2000%2Fsvg%22%20width%3D%2230%22%20height%3D%2210%22%2F%3E";
    let packet = packet_for(json!({
        "width": 60, "height": 20, "bgColor": "#112233",
        "backgroundImage": background, "drawables": []
    }));
    assert!(
        matches!(packet.draw_commands.first(), Some(DrawCommand::Clear { color }) if color[0] == 17.0 / 255.0)
    );
    let tiles: Vec<_> = packet
        .draw_commands
        .iter()
        .filter_map(|command| match command {
            DrawCommand::Sprite { dst, .. } => Some(dst),
            _ => None,
        })
        .collect();
    assert_eq!(tiles.len(), 9);
    assert!(tiles.iter().all(|tile| tile.w == 30.0 && tile.h == 10.0));
    assert_eq!(tiles[0].x, -30.0);
    assert_eq!(tiles[0].y, -10.0);
}

#[test]
fn draw_group_translates_nested_2d_items_and_multiplies_alpha() {
    let plain = packet_for(json!({
        "projection": "free", "scale": 1.0,
        "drawables": [{"type": "draw-shape", "shape": "rect",
            "position": {"x": 0, "y": 0}, "width": 1, "height": 1,
            "fill": "#ff0000"}]
    }));
    let grouped = packet_for(json!({
        "projection": "free", "scale": 1.0,
        "drawables": [{"type": "draw-group", "position": {"x": 2, "y": 3},
            "opacity": 0.5, "items": [{"type": "draw-shape", "shape": "rect",
                "position": {"x": 0, "y": 0}, "width": 1, "height": 1,
                "fill": "#ff0000"}]}]
    }));
    let rect = |packet: &GameRenderPacket| {
        packet
            .draw_commands
            .iter()
            .find_map(|cmd| match cmd {
                DrawCommand::Quad { rect, color } => Some((*rect, *color)),
                _ => None,
            })
            .expect("filled rect")
    };
    let (plain_rect, _) = rect(&plain);
    let (group_rect, group_color) = rect(&grouped);
    assert!(group_rect.x > plain_rect.x);
    assert!(group_rect.y > plain_rect.y);
    assert_eq!(group_color[3], 0.5);
}

#[test]
fn draw_group_animation_moves_children_on_the_host_clock() {
    let scene = json!({
        "projection": "free", "scale": 1.0,
        "drawables": [{"type": "draw-group", "position": {"x": 0, "y": 0},
            "animation": {"durationMs": 1000, "loop": false,
                "keyframes": [
                    {"at": 0, "offsetX": 0, "opacity": 1},
                    {"at": 1, "offsetX": 256, "opacity": 0.5}
                ]},
            "items": [{"type": "draw-shape", "shape": "rect",
                "position": {"x": 0, "y": 0}, "width": 256, "height": 256,
                "fill": "#ff0000"}]}]
    });
    assert!(has_active_fx(&scene));
    let initial = from_props_at(&scene, 0.0).unwrap();
    let final_frame = from_props_at(&scene, 1000.0).unwrap();
    let quad = |packet: &GameRenderPacket| {
        packet
            .draw_commands
            .iter()
            .find_map(|command| match command {
                DrawCommand::Quad { rect, color } => Some((*rect, *color)),
                _ => None,
            })
            .expect("grouped rectangle")
    };
    let (initial_rect, _) = quad(&initial);
    let (final_rect, final_color) = quad(&final_frame);
    assert!((final_rect.x - initial_rect.x - TILE_WIDTH).abs() < 0.1);
    assert_eq!(final_color[3], 0.5);
}

#[test]
fn draw_group_clip_scopes_mixed_child_commands() {
    let p = packet_for(json!({
        "projection": "free", "scale": 1.0,
        "drawables": [{"type": "draw-group", "position": {"x": 2, "y": 3},
            "clip": "M0 0 L1 0 L1 1 Z", "items": [{"type": "draw-shape",
                "shape": "rect", "position": {"x": 0, "y": 0},
                "width": 1, "height": 1, "fill": "#ff0000"}]}]
    }));
    let group = p
        .draw_commands
        .iter()
        .find_map(|command| match command {
            DrawCommand::ClipGroup { clip, commands } => Some((clip, commands)),
            _ => None,
        })
        .expect("scoped clip group");
    assert!(group.0.len() >= 4);
    assert!(group
        .1
        .iter()
        .any(|command| matches!(command, DrawCommand::Quad { .. })));
    assert!(!p
        .draw_commands
        .iter()
        .any(|command| matches!(command, DrawCommand::Quad { .. })));
}

#[test]
fn draw_group_uses_shared_affine_transform_for_rotated_shape() {
    let p = packet_for(json!({
        "projection": "free", "scale": 1.0,
        "drawables": [{"type": "draw-group", "position": {"x": 2, "y": 3},
            "scale": 2, "rotate": std::f64::consts::FRAC_PI_2,
            "items": [{"type": "draw-shape", "shape": "rect",
                "position": {"x": 0, "y": 0}, "width": 1, "height": 1,
                "fill": "#ff0000"}]}]
    }));
    assert!(p
        .draw_commands
        .iter()
        .any(|cmd| matches!(cmd, DrawCommand::Poly { points, .. } if points.len() == 4)));
    assert!(!p
        .draw_commands
        .iter()
        .any(|cmd| matches!(cmd, DrawCommand::Quad { .. })));
}

#[test]
fn fx_layer_uses_core_projection_and_epoch_paint_clock() {
    let props = json!({
        "projection": "free", "scale": 1.0,
        "drawables": [{ "type": "draw-fx-layer", "tickMs": 500,
            "fontFamily": "system-ui, sans-serif", "textColor": "#ffe066",
            "items": [{ "id": "fx-1", "type": "hit", "position": { "x": 2, "y": 3 },
                "ttl": 2, "maxTtl": 4, "bornAt": 1000, "message": "-2" }],
            "presets": [{ "type": "hit", "shape": "ring", "color": "#ff0000", "size": 0.5,
                "vx": 0, "vz": 0, "gravity": 0 }]
        }]
    });
    let p = from_props_at(&props, 1500.0).unwrap();
    let stroke_alpha = p
        .draw_commands
        .iter()
        .find_map(|cmd| match cmd {
            DrawCommand::Path {
                stroke: Some(stroke),
                ..
            } => Some(stroke.color[3]),
            DrawCommand::AdditiveGroup { commands } => {
                commands.iter().find_map(|inner| match inner {
                    DrawCommand::Path {
                        stroke: Some(stroke),
                        ..
                    }
                    | DrawCommand::GradientPath {
                        stroke: Some(stroke),
                        ..
                    } => Some(stroke.color[3]),
                    _ => None,
                })
            }
            _ => None,
        })
        .expect("ring stroke");
    assert_eq!(stroke_alpha, 0.75);
    assert!(p
        .draw_commands
        .iter()
        .any(|cmd| matches!(cmd, DrawCommand::Text { text, .. } if text == "-2")));
    let expired = from_props_at(&props, 3000.0).unwrap();
    assert!(!expired.draw_commands.iter().any(|cmd| matches!(
        cmd,
        DrawCommand::Text { .. } | DrawCommand::Path { .. } | DrawCommand::AdditiveGroup { .. }
    )));
}

#[test]
fn clear_uses_bgcolor_then_fallback() {
    let p = packet_for(json!({}));
    match &p.draw_commands[0] {
        DrawCommand::Clear { color } => assert_eq!(*color, BACKGROUND_FALLBACK),
        other => panic!("expected Clear, got {other:?}"),
    }
    let p = packet_for(json!({ "bgColor": "#000000" }));
    match &p.draw_commands[0] {
        DrawCommand::Clear { color } => assert_eq!(*color, [0.0, 0.0, 0.0, 1.0]),
        other => panic!("expected Clear, got {other:?}"),
    }
}

#[test]
fn cell_shape_emits_diamond_poly_in_screen_space() {
    // Single cell at (0,0), iso, scale 1, viewport 800x600, no cameraPos.
    // Camera: zoom=1, cam=(0,0) → screen = (world - 400, world - 300) + (400,300) = world.
    let p = packet_for(json!({
        "scale": 1.0,
        "drawables": [
            { "type": "draw-shape", "shape": "cell",
              "position": { "x": 0, "y": 0 }, "fill": "#4ade80" }
        ]
    }));
    let poly = p
        .draw_commands
        .iter()
        .find_map(|c| match c {
            DrawCommand::Poly { points, color } => Some((points, color)),
            _ => None,
        })
        .expect("a Poly for the cell");
    assert_eq!(poly.0.len(), 4);
    // Diamond top vertex at (tw/2, diamondTopY) = (128, 374).
    assert!((poly.0[0].x - 128.0).abs() < 1e-3);
    assert!((poly.0[0].y - 374.0).abs() < 1e-3);
    assert!((poly.1[0] - 74.0 / 255.0).abs() < 1e-3); // #4ade80 → 74,222,128
    assert!((poly.1[1] - 222.0 / 255.0).abs() < 1e-3);
}

#[test]
fn camera_pos_centers_viewport() {
    // cameraPos (0,0) iso scale 1: anchor 'center' = (128, 374+64) = (128, 438);
    // cam = (128-400, 438-300) = (-272, 138).
    // screen(p) = (p - (400,300) - cam) + (400,300) = p - cam.
    let p = packet_for(json!({
        "scale": 1.0,
        "cameraPos": { "x": 0, "y": 0 },
        "drawables": [
            { "type": "draw-shape", "shape": "rect",
              "position": { "x": 0, "y": 0 }, "width": 1, "height": 1,
              "fill": "#ffffff" }
        ]
    }));
    let rect = p
        .draw_commands
        .iter()
        .find_map(|c| match c {
            DrawCommand::Quad { rect, .. } => Some(rect),
            _ => None,
        })
        .expect("a Quad for the rect");
    // world (0,0) → screen (0 - (-272), 0 - 138) = (272, -138).
    assert!((rect.x - 272.0).abs() < 1e-3);
    assert!((rect.y + 138.0).abs() < 1e-3);
    assert!((rect.w - 256.0).abs() < 1e-3); // width 1 * tw, zoom 1
}

#[test]
fn layer_items_paint_with_layer_atom() {
    let p = packet_for(json!({
        "scale": 1.0,
        "drawables": [
            { "type": "draw-shape-layer", "items": [
                { "shape": "rect", "position": { "x": 0, "y": 0 },
                  "width": 0.5, "height": 0.5, "fill": "#ff0000" },
                { "shape": "rect", "position": { "x": 1, "y": 0 },
                  "width": 0.5, "height": 0.5, "fill": "#00ff00" }
            ]}
        ]
    }));
    let quads = p
        .draw_commands
        .iter()
        .filter(|c| matches!(c, DrawCommand::Quad { .. }))
        .count();
    assert_eq!(quads, 2);
}

#[test]
fn base_offset_applies_for_iso_grids() {
    // Two rows (max y = 1) → baseOffsetX = 1 * 256/2 = 128 at scale 1.
    let p = packet_for(json!({
        "scale": 1.0,
        "drawables": [
            { "type": "draw-shape", "shape": "cell",
              "position": { "x": 0, "y": 0 }, "fill": "#000000" },
            { "type": "draw-shape", "shape": "cell",
              "position": { "x": 0, "y": 1 }, "fill": "#000000" }
        ]
    }));
    let first = p
        .draw_commands
        .iter()
        .find_map(|c| match c {
            DrawCommand::Poly { points, .. } => Some(points),
            _ => None,
        })
        .expect("first cell poly");
    // Cell (0,0) project: iso (0*128 + 128, 0) → top vertex (128+128, 374).
    assert!((first[0].x - 256.0).abs() < 1e-3);
}

fn evade_like_node() -> Value {
    json!({
        "type": "canvas",
        "mode": "2d",
        "projection": "flat",
        "camera": { "zoom": 0.5, "pos": { "x": 2.0, "y": 1.0 } },
        "backgroundColor": "#0f172a",
        "children": [
            { "type": "draw-shape", "shape": "rect", "position": { "x": 0, "y": 0 },
              "width": 1, "height": 1, "fill": "#ff0000" },
            { "type": "draw-shape-layer", "items": [
                { "shape": "rect", "position": { "x": 1, "y": 0 },
                  "width": 1, "height": 1, "fill": "#00ff00" } ] },
            "@trait.Ignored"
        ]
    })
}

#[test]
fn canvas_node_children_route_to_drawables() {
    let routed = from_canvas_node(&evade_like_node()).unwrap();
    let direct = from_props(&json!({
        "projection": "flat", "scale": 0.5, "cameraPos": { "x": 2.0, "y": 1.0 },
        "bgColor": "#0f172a",
        "drawables": [
            { "type": "draw-shape", "shape": "rect", "position": { "x": 0, "y": 0 },
              "width": 1, "height": 1, "fill": "#ff0000" },
            { "type": "draw-shape-layer", "items": [
                { "shape": "rect", "position": { "x": 1, "y": 0 },
                  "width": 1, "height": 1, "fill": "#00ff00" } ] }
        ]
    }))
    .unwrap();
    assert_eq!(
        serde_json::to_value(&routed).unwrap(),
        serde_json::to_value(&direct).unwrap()
    );
    let quads = routed
        .draw_commands
        .iter()
        .filter(|c| matches!(c, DrawCommand::Quad { .. }))
        .count();
    assert_eq!(quads, 2, "string child dropped, both shapes painted");
}

#[test]
fn canvas_node_control_without_children_paints_background_only() {
    let p = from_canvas_node(&json!({ "type": "canvas", "backgroundColor": "#000000" })).unwrap();
    assert_eq!(p.draw_commands.len(), 1);
    match &p.draw_commands[0] {
        DrawCommand::Clear { color } => assert_eq!(*color, [0.0, 0.0, 0.0, 1.0]),
        other => panic!("expected Clear, got {other:?}"),
    }
}

#[test]
fn canvas_node_camera_zoom_scales_the_scene() {
    let at = |zoom: f64| {
        let p = from_canvas_node(&json!({
            "type": "canvas", "projection": "flat", "camera": { "zoom": zoom },
            "children": [ { "type": "draw-shape", "shape": "rect", "position": { "x": 0, "y": 0 },
                            "width": 1, "height": 1, "fill": "#ffffff" } ]
        }))
        .unwrap();
        match p.draw_commands.iter().find_map(|c| match c {
            DrawCommand::Quad { rect, .. } => Some(*rect),
            _ => None,
        }) {
            Some(r) => r.w,
            None => panic!("no quad"),
        }
    };
    assert!((at(1.0) - 256.0).abs() < 1e-3);
    assert!(
        at(0.5) < at(1.0),
        "camera.zoom must reach the mapper's scale"
    );
    assert!(at(0.25) < at(0.5));
}

#[test]
fn canvas_2d_string_camera_is_not_a_camera_object() {
    let p = from_canvas_node(&json!({
        "type": "canvas-2d", "camera": "fixed", "scale": 1.0, "projection": "flat",
        "drawables": [ { "type": "draw-shape", "shape": "rect", "position": { "x": 0, "y": 0 },
                         "width": 1, "height": 1, "fill": "#ffffff" } ]
    }))
    .unwrap();
    assert!(p
        .draw_commands
        .iter()
        .any(|c| matches!(c, DrawCommand::Quad { rect, .. } if (rect.w - 256.0).abs() < 1e-3)));
}

#[test]
fn canvas_node_explicit_drawables_win_over_children() {
    let p = from_canvas_node(&json!({
        "type": "canvas", "projection": "flat", "scale": 1.0,
        "drawables": [ { "type": "draw-shape", "shape": "rect", "position": { "x": 0, "y": 0 },
                         "width": 1, "height": 1, "fill": "#ffffff" } ],
        "children": [ { "type": "draw-shape", "shape": "rect", "position": { "x": 1, "y": 0 },
                        "width": 1, "height": 1, "fill": "#ffffff" },
                      { "type": "draw-shape", "shape": "rect", "position": { "x": 2, "y": 0 },
                        "width": 1, "height": 1, "fill": "#ffffff" } ]
    }))
    .unwrap();
    let quads = p
        .draw_commands
        .iter()
        .filter(|c| matches!(c, DrawCommand::Quad { .. }))
        .count();
    assert_eq!(quads, 1);
}

#[test]
fn canvas_node_nested_props_shape_matches_flat() {
    let flat = json!({ "type": "canvas", "projection": "flat", "scale": 1.0,
        "children": [ { "type": "draw-shape", "shape": "rect", "position": { "x": 0, "y": 0 },
                        "width": 1, "height": 1, "fill": "#ffffff" } ] });
    let nested = json!({ "type": "canvas", "props": { "projection": "flat", "scale": 1.0,
        "children": [ { "type": "draw-shape", "props": { "shape": "rect", "position": { "x": 0, "y": 0 },
                        "width": 1, "height": 1, "fill": "#ffffff" } } ] } });
    assert_eq!(
        serde_json::to_value(from_canvas_node(&flat).unwrap()).unwrap(),
        serde_json::to_value(from_canvas_node(&nested).unwrap()).unwrap()
    );
}

#[test]
fn canvas_node_refuses_3d_and_non_objects() {
    assert_eq!(
        from_canvas_node(&json!({ "type": "canvas", "mode": "3d" })).unwrap_err(),
        CanvasSceneError::UnsupportedMode("3d".into())
    );
    assert_eq!(
        from_canvas_node(&json!([1, 2])).unwrap_err(),
        CanvasSceneError::NotAnObject
    );
    assert!(from_canvas_node(&json!({ "type": "canvas", "mode": "2d" })).is_ok());
}

#[test]
fn viewport_overrides_size_and_keeps_scene_undistorted() {
    let node = evade_like_node();
    let tall = from_canvas_node_in_viewport(&node, 400.0, 800.0).unwrap();
    assert_eq!((tall.width, tall.height), (400.0, 800.0));
    let quad_dims = |p: &GameRenderPacket| -> Vec<(f32, f32)> {
        p.draw_commands
            .iter()
            .filter_map(|c| match c {
                DrawCommand::Quad { rect, .. } => Some((rect.w, rect.h)),
                _ => None,
            })
            .collect()
    };
    let wide = from_canvas_node_in_viewport(&node, 800.0, 400.0).unwrap();
    assert_eq!(
        quad_dims(&tall),
        quad_dims(&wide),
        "cell size is viewport-independent"
    );
    assert!(!quad_dims(&tall).is_empty());
}

#[test]
fn viewport_equal_to_default_matches_plain_node_mapping() {
    let node = evade_like_node();
    let a = from_canvas_node(&node).unwrap();
    let b = from_canvas_node_in_viewport(&node, 800.0, 600.0).unwrap();
    assert_eq!(
        serde_json::to_value(&a).unwrap(),
        serde_json::to_value(&b).unwrap()
    );
}

#[test]
fn viewport_still_refuses_3d() {
    assert_eq!(
        from_canvas_node_in_viewport(&json!({ "type": "canvas", "mode": "3d" }), 10.0, 10.0)
            .unwrap_err(),
        CanvasSceneError::UnsupportedMode("3d".into())
    );
}

fn first_sprite_dst(p: &GameRenderPacket) -> Rect {
    p.draw_commands
        .iter()
        .find_map(|c| match c {
            DrawCommand::Sprite { dst, .. } => Some(*dst),
            _ => None,
        })
        .expect("a sprite command")
}

fn sprite_scene(projection: &str, extra: Value) -> Value {
    let mut sprite = json!({
        "type": "draw-sprite", "position": { "x": 0, "y": 0 },
        "asset": { "url": "data:image/svg+xml,%3Csvg%20xmlns%3D%27http%3A%2F%2Fwww.w3.org%2F2000%2Fsvg%27%20width%3D%2764%27%20height%3D%2764%27%2F%3E" },
        "frame": { "x": 0, "y": 0, "w": 64, "h": 64 }
    });
    if let (Some(dst), Some(src)) = (sprite.as_object_mut(), extra.as_object()) {
        dst.extend(src.clone());
    }
    json!({ "projection": projection, "scale": 0.5, "drawables": [sprite] })
}

/// `spriteRect`: no width/height = one cell on a tile grid, whatever the art's px size.
#[test]
fn omitted_sprite_size_is_one_cell_on_tile_grids_not_the_source_px() {
    let dst = first_sprite_dst(&packet_for(sprite_scene("isometric", json!({}))));
    let cell = TILE_WIDTH * 0.5 * 0.5;
    assert!(
        (dst.w - cell).abs() < 1e-3 && (dst.h - cell).abs() < 1e-3,
        "{dst:?}"
    );
}

#[test]
fn explicit_sprite_size_is_in_cell_fractions() {
    let dst = first_sprite_dst(&packet_for(sprite_scene(
        "isometric",
        json!({ "width": 2 }),
    )));
    let cell = TILE_WIDTH * 0.5 * 0.5;
    assert!(
        (dst.w - 2.0 * cell).abs() < 1e-3 && (dst.h - cell).abs() < 1e-3,
        "{dst:?}"
    );
}

/// Control: a world-pixel `free` board keeps the source's native px size.
#[test]
fn omitted_sprite_size_is_native_px_on_free_boards() {
    let dst = first_sprite_dst(&packet_for(sprite_scene("free", json!({}))));
    assert!(
        (dst.w - 64.0 * 0.5).abs() < 1e-3 && (dst.h - 64.0 * 0.5).abs() < 1e-3,
        "{dst:?}"
    );
}

fn free_props(drawables: Value) -> Value {
    json!({"width": 200, "height": 100, "scale": 1.0, "projection": "free",
        "bgColor": "#00000000", "drawables": drawables})
}

fn rect_shape(extra: Value) -> Value {
    let mut shape = json!({"type": "draw-shape", "shape": "rect",
        "position": {"x": 10, "y": 20}, "width": 64, "height": 32, "fill": "#ff0000"});
    for (key, value) in extra.as_object().unwrap() {
        shape[key] = value.clone();
    }
    shape
}

fn sdf_commands(packet: &GameRenderPacket) -> Vec<&DrawCommand> {
    packet
        .draw_commands
        .iter()
        .filter(|c| matches!(c, DrawCommand::Sdf { .. }))
        .collect()
}

#[test]
fn corners_a_material_and_the_new_kinds_map_to_sdf_and_a_plain_rect_keeps_its_quad() {
    let plain = packet_for(free_props(json!([rect_shape(json!({}))])));
    assert!(plain
        .draw_commands
        .iter()
        .any(|c| matches!(c, DrawCommand::Quad { .. })));
    assert!(
        sdf_commands(&plain).is_empty(),
        "control: no new field, no Sdf"
    );

    let cornered = packet_for(free_props(json!([rect_shape(
        json!({"corners": {"radius": [4, 4, 4, 4], "kind": "bevel"}})
    )])));
    assert!(!cornered
        .draw_commands
        .iter()
        .any(|c| matches!(c, DrawCommand::Quad { .. })));
    match sdf_commands(&cornered).as_slice() {
        [DrawCommand::Sdf {
            kind,
            rect,
            corners,
            corner_kind,
            fill,
            ..
        }] => {
            assert_eq!(*kind, SdfKind::Rect);
            assert_eq!(
                *rect,
                Rect {
                    x: 10.0,
                    y: 20.0,
                    w: 64.0,
                    h: 32.0
                }
            );
            assert_eq!((*corners, *corner_kind), ([4.0; 4], 1));
            assert_eq!(*fill, [1.0, 0.0, 0.0, 1.0]);
        }
        other => panic!("one Sdf: {other:?}"),
    }
    for (shape, extra, expected) in [
        ("capsule", json!({}), SdfKind::Capsule),
        ("arc", json!({"sweep": 3.0, "thickness": 4}), SdfKind::Arc),
        (
            "polygon",
            json!({"sides": 8, "starRatio": 0.7}),
            SdfKind::Polygon,
        ),
    ] {
        let mut node = rect_shape(extra);
        node["shape"] = json!(shape);
        let packet = packet_for(free_props(json!([node])));
        assert!(
            matches!(sdf_commands(&packet).as_slice(), [DrawCommand::Sdf { kind, .. }] if *kind == expected),
            "{shape}"
        );
    }
}

#[test]
fn invalid_sdf_fields_are_scene_errors_and_a_bad_wgsl_names_material_wgsl() {
    let bad = from_props(&free_props(json!([rect_shape(
        json!({"material": {"wgsl": "fn nope() {}", "params": [], "live": false}})
    )])))
    .unwrap_err();
    assert!(
        matches!(&bad, CanvasSceneError::InvalidProp { field, .. } if field == "material.wgsl"),
        "{bad:?}"
    );
    let good = packet_for(free_props(json!([rect_shape(json!({"material": {
        "wgsl": "fn shade(px: Pixel) -> vec4<f32> { return px.color; }",
        "params": [1, 2], "live": true}}))])));
    assert!(matches!(
        sdf_commands(&good).as_slice(),
        [DrawCommand::Sdf { material: Some(m), .. }] if m.params == vec![1.0, 2.0] && m.live
    ));
    let kind = from_props(&free_props(json!([rect_shape(
        json!({"corners": {"radius": [1, 1, 1, 1], "kind": "wavy"}})
    )])))
    .unwrap_err();
    assert!(
        matches!(&kind, CanvasSceneError::InvalidProp { field, .. } if field == "corners.kind"),
        "{kind:?}"
    );
}

#[test]
fn anchor_since_reaches_an_sdf_material_in_seconds_and_rests_without_an_anchor_or_epoch() {
    const WGSL: &str =
        "fn shade(px: Pixel) -> vec4<f32> { return vec4<f32>(px.since, 0.0, 0.0, 1.0); }";
    let since_at = |material: Value, epoch: f64| {
        let packet = from_props_at(
            &free_props(json!([rect_shape(json!({"material": material}))])),
            epoch,
        )
        .unwrap();
        match sdf_commands(&packet).as_slice() {
            [DrawCommand::Sdf {
                material: Some(m), ..
            }] => m.since,
            other => panic!("one material shape, got {other:?}"),
        }
    };
    let anchored = json!({"wgsl": WGSL, "anchor": 1_700_000_000_000.0});
    assert_eq!(since_at(anchored.clone(), 1_700_000_002_500.0), 2.5);
    assert_eq!(since_at(anchored, 0.0), 0.0);
    assert_eq!(since_at(json!({"wgsl": WGSL}), 1_700_000_002_500.0), 0.0);
    let bad = from_props(&free_props(json!([rect_shape(
        json!({"material": {"wgsl": WGSL, "anchor": "soon"}})
    )])))
    .unwrap_err();
    assert!(
        matches!(&bad, CanvasSceneError::InvalidProp { field, .. } if field == "material.anchor"),
        "{bad:?}"
    );
}

#[test]
fn a_semantic_node_in_a_scaled_group_sits_where_the_group_draws_it() {
    let button = json!({"type": "draw-shape", "shape": "rect", "id": "go", "hit": true, "anchor": "top-left",
        "position": {"x": 20, "y": 40}, "width": 40, "height": 20, "fill": "#000000",
        "semantic": {"role": "button", "name": "Go"}});
    let group = json!({"type": "draw-group", "id": "g", "position": {"x": 100, "y": 50}, "scale": 0.5, "items": [button.clone()]});
    let (_, gathered) = from_props_mapped(&free_props(json!([group])), 0.0).unwrap();
    let r = gathered.semantic[0].rect;
    assert!(
        (r.x - 110.0).abs() < 1e-3
            && (r.y - 70.0).abs() < 1e-3
            && (r.w - 20.0).abs() < 1e-3
            && (r.h - 10.0).abs() < 1e-3,
        "{r:?}"
    );
    assert_eq!(
        gathered.semantic[0].rect, gathered.regions[0].rect,
        "the overlay node and the hit region agree"
    );
    let (_, plain) = from_props_mapped(&free_props(json!([button])), 0.0).unwrap();
    assert_eq!(
        plain.semantic[0].rect,
        Rect {
            x: 20.0,
            y: 40.0,
            w: 40.0,
            h: 20.0
        },
        "control: an ungrouped node keeps its rect"
    );
}

#[test]
fn a_semantic_value_from_a_text_takes_that_text_as_its_value() {
    let combo = |sem: Value| {
        json!({"type": "draw-shape", "shape": "rect", "id": "season", "hit": true, "anchor": "top-left",
            "position": {"x": 0, "y": 0}, "width": 40, "height": 20, "fill": "#000000", "semantic": sem})
    };
    let shown = json!({"type": "draw-text", "id": "season:value", "text": "الربيع", "anchor": "top-left",
        "position": {"x": 0, "y": 0}, "color": "#ffffff"});
    let (_, gathered) = from_props_mapped(
        &free_props(json!([
            combo(json!({"role": "combobox", "name": "Season", "valueFrom": "season:value"})),
            shown.clone()
        ])),
        0.0,
    )
    .unwrap();
    assert_eq!(gathered.semantic[0].value.as_deref(), Some("الربيع"));
    let (_, plain) = from_props_mapped(
        &free_props(json!([
            combo(json!({"role": "combobox", "name": "Season", "value": "spring"})),
            shown.clone()
        ])),
        0.0,
    )
    .unwrap();
    assert_eq!(
        plain.semantic[0].value.as_deref(),
        Some("spring"),
        "control: a literal value is kept"
    );
    let both = from_props_mapped(
        &free_props(json!([combo(json!({"role": "combobox", "name": "Season", "value": "spring", "valueFrom": "season:value"})), shown])),
        0.0,
    )
    .unwrap_err();
    assert!(
        matches!(&both, CanvasSceneError::InvalidProp { field, .. } if field == "semantic.valueFrom"),
        "{both:?}"
    );
}

#[test]
fn a_semantic_node_labelled_by_a_text_takes_that_text_as_its_name() {
    let button = |sem: Value| {
        json!({"type": "draw-shape", "shape": "rect", "id": "begin", "hit": true, "anchor": "top-left",
            "position": {"x": 0, "y": 0}, "width": 40, "height": 20, "fill": "#000000", "semantic": sem})
    };
    let label = json!({"type": "draw-text", "id": "begin:label", "text": "ابدأ", "anchor": "top-left",
        "position": {"x": 0, "y": 0}, "color": "#ffffff"});
    let (_, gathered) = from_props_mapped(
        &free_props(json!([
            button(json!({"role": "button", "labelledBy": "begin:label"})),
            label.clone()
        ])),
        0.0,
    )
    .unwrap();
    assert_eq!(
        gathered.semantic[0].name, "ابدأ",
        "the text painted after the node still names it"
    );
    let (_, plain) = from_props_mapped(
        &free_props(json!([
            button(json!({"role": "button", "name": "Begin"})),
            label.clone()
        ])),
        0.0,
    )
    .unwrap();
    assert_eq!(
        plain.semantic[0].name, "Begin",
        "control: a literal name is kept"
    );
    let missing = from_props_mapped(
        &free_props(json!([
            button(json!({"role": "button", "labelledBy": "nowhere"})),
            label.clone()
        ])),
        0.0,
    )
    .unwrap_err();
    assert!(
        matches!(&missing, CanvasSceneError::InvalidProp { field, .. } if field == "semantic.labelledBy"),
        "{missing:?}"
    );
    let both = from_props_mapped(
        &free_props(json!([
            button(json!({"role": "button", "name": "Begin", "labelledBy": "begin:label"})),
            label
        ])),
        0.0,
    )
    .unwrap_err();
    assert!(
        matches!(&both, CanvasSceneError::InvalidProp { field, .. } if field == "semantic.labelledBy"),
        "{both:?}"
    );
}

#[test]
fn semantic_drawables_form_the_tree_in_reading_order_at_their_regions() {
    let node = |id: &str, hit: bool, x: i32, sem: Value| {
        json!({"type": "draw-shape", "shape": "rect", "id": id, "hit": hit, "anchor": "top-left",
            "position": {"x": x, "y": 0}, "width": 40, "height": 20, "fill": "#000000", "semantic": sem})
    };
    let (_, gathered) = from_props_mapped(
        &free_props(json!([
            node("second", true, 0, json!({"role": "button", "name": "Two", "order": 2})),
            node("first", false, 50, json!({"role": "textbox", "name": "One", "value": "hi", "caret": 1, "focused": true, "order": 1})),
            json!({"type": "draw-shape", "shape": "rect", "anchor": "top-left", "position": {"x": 0, "y": 0},
                "width": 10, "height": 10, "fill": "#000000", "semantic": {"role": "button", "name": "no id"}}),
        ])),
        0.0,
    )
    .unwrap();
    let mut nodes = gathered.semantic.clone();
    nodes.sort_by(|a, b| a.order.total_cmp(&b.order));
    let ids: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["first", "second"],
        "control: a semantic drawable with no id is no node"
    );
    assert_eq!(nodes[0].caret, Some(1));
    assert!(nodes[0].focused && nodes[0].value.as_deref() == Some("hi"));
    assert_eq!(
        (nodes[0].rect.x, nodes[0].rect.w),
        (50.0, 40.0),
        "painted bounds without a hit"
    );
    assert_eq!(
        (nodes[1].rect.x, nodes[1].rect.w),
        (0.0, 40.0),
        "the pointer region with a hit"
    );
}

#[test]
fn a_semantic_node_without_a_role_is_a_scene_error() {
    let result = from_props_mapped(
        &free_props(
            json!([{"type": "draw-shape", "shape": "rect", "id": "x", "anchor": "top-left",
            "position": {"x": 0, "y": 0}, "width": 10, "height": 10, "fill": "#000000", "semantic": {"name": "nameless role"}}]),
        ),
        0.0,
    );
    assert!(result.is_err());
}

#[test]
fn an_unpainted_hit_rect_is_still_a_region_and_an_unpainted_plain_rect_is_not() {
    let hit_area = |id: &str, hit: bool| {
        json!({"type": "draw-shape", "shape": "rect", "id": id, "hit": hit, "anchor": "top-left",
            "position": {"x": 10, "y": 20}, "width": 64, "height": 32})
    };
    let (_, gathered) = from_props_mapped(
        &free_props(json!([hit_area("area", true), hit_area("plain", false)])),
        0.0,
    )
    .unwrap();
    let ids: Vec<&str> = gathered.regions.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, vec!["area"]);
    assert_eq!(gathered.regions[0].rect.w, 64.0);
    assert_eq!(gathered.regions[0].rect.h, 32.0);
}

#[test]
fn a_region_per_hit_drawable_with_the_right_rect_and_topmost_last() {
    let (packet, gathered) = from_props_mapped(
        &free_props(json!([
            rect_shape(json!({"id": "a", "hit": true, "corners": {"radius": [2, 2, 2, 2]}})),
            rect_shape(json!({"id": "b", "hit": true, "position": {"x": 30, "y": 30}})),
            rect_shape(json!({"id": "c", "hit": false})),
            rect_shape(json!({"hit": true})),
        ])),
        0.0,
    )
    .unwrap();
    assert_eq!(packet.draw_commands.len(), 5, "clear and four shapes");
    let ids: Vec<&str> = gathered.regions.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(
        ids,
        ["a", "b"],
        "no region without hit and id; later is topmost"
    );
    assert_eq!(
        gathered.regions[0].rect,
        Rect {
            x: 10.0,
            y: 20.0,
            w: 64.0,
            h: 32.0
        }
    );
    assert_eq!(
        gathered.regions[1].rect,
        Rect {
            x: 30.0,
            y: 30.0,
            w: 64.0,
            h: 32.0
        }
    );
}

#[test]
fn a_measured_text_reports_its_size_and_control_without_measure_reports_none() {
    let text = |measure: bool| {
        json!({"type": "draw-text", "id": "t", "text": "hello", "color": "#000000",
            "font": "12px sans-serif", "position": {"x": 5, "y": 5}, "measure": measure})
    };
    let (_, gathered) = from_props_mapped(&free_props(json!([text(true)])), 0.0).unwrap();
    match gathered.measured.as_slice() {
        [m] => {
            assert_eq!(m.id, "t");
            assert_eq!(m.lines, 1);
            assert!(
                (m.width - orbital_canvas_paint::vector::text_width("hello", 12.0)).abs() < 1e-3
            );
            assert!((m.height - 12.0 * 1.3).abs() < 1e-3);
        }
        other => panic!("one entry: {other:?}"),
    }
    let (_, none) = from_props_mapped(&free_props(json!([text(false)])), 0.0).unwrap();
    assert!(none.measured.is_empty());
}

#[test]
fn a_paragraph_wraps_into_lines_within_its_width_and_control_without_width_is_one_line() {
    let width = orbital_canvas_paint::vector::text_width("alpha beta", 12.0) + 0.5;
    let node = |extra: Value| {
        let mut n = json!({"type": "draw-text", "id": "p", "text": "alpha beta go on",
            "color": "#000000", "font": "12px sans-serif", "position": {"x": 5, "y": 5},
            "measure": true});
        for (key, value) in extra.as_object().unwrap() {
            n[key] = value.clone();
        }
        n
    };
    let (packet, gathered) =
        from_props_mapped(&free_props(json!([node(json!({"width": width}))])), 0.0).unwrap();
    let lines: Vec<&str> = packet
        .draw_commands
        .iter()
        .filter_map(|c| match c {
            DrawCommand::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(lines, ["alpha beta", "go on"]);
    assert_eq!(gathered.measured[0].lines, 2);
    assert!(gathered.measured[0].width <= width);
    let (single, _) = from_props_mapped(&free_props(json!([node(json!({}))])), 0.0).unwrap();
    assert_eq!(
        single
            .draw_commands
            .iter()
            .filter(|c| matches!(c, DrawCommand::Text { .. }))
            .count(),
        1
    );
}

#[test]
fn layout_wraps_within_width_truncates_with_an_ellipsis_and_overflows_without_truncate() {
    let text = "alpha beta go on";
    let width = orbital_canvas_paint::vector::text_width("alpha beta", 12.0) + 0.5;
    let wrapped = layout_paragraph(
        text,
        12.0,
        false,
        orbital_canvas_base::packet::TextFamily::Sans,
        ParagraphBox {
            width,
            wrap: true,
            max_lines: None,
            truncate: false,
        },
    );
    assert_eq!(wrapped.lines, ["alpha beta", "go on"]);
    assert!(wrapped.widths.iter().all(|w| *w <= width));
    let flat = layout_paragraph(
        text,
        12.0,
        false,
        orbital_canvas_base::packet::TextFamily::Sans,
        ParagraphBox {
            width,
            wrap: false,
            max_lines: None,
            truncate: false,
        },
    );
    assert_eq!(flat.lines, [text], "control: wrap off keeps one line");

    let cut = layout_paragraph(
        text,
        12.0,
        false,
        orbital_canvas_base::packet::TextFamily::Sans,
        ParagraphBox {
            width,
            wrap: true,
            max_lines: Some(1),
            truncate: true,
        },
    );
    assert_eq!(cut.lines.len(), 1);
    assert!(cut.lines[0].ends_with('\u{2026}'), "{:?}", cut.lines);
    assert!(cut.widths[0] <= width);
    let capped = layout_paragraph(
        text,
        12.0,
        false,
        orbital_canvas_base::packet::TextFamily::Sans,
        ParagraphBox {
            width,
            wrap: true,
            max_lines: Some(1),
            truncate: false,
        },
    );
    assert_eq!(
        capped.lines,
        ["alpha beta"],
        "control: no truncate, no ellipsis"
    );

    let narrow = orbital_canvas_paint::vector::text_width("alpha", 12.0) * 0.6;
    let one_line = layout_paragraph(
        "alphabet",
        12.0,
        false,
        orbital_canvas_base::packet::TextFamily::Sans,
        ParagraphBox {
            width: narrow,
            wrap: true,
            max_lines: None,
            truncate: true,
        },
    );
    assert!(one_line.lines[0].ends_with('\u{2026}') && one_line.widths[0] <= narrow);
    let overflow = layout_paragraph(
        "alphabet",
        12.0,
        false,
        orbital_canvas_base::packet::TextFamily::Sans,
        ParagraphBox {
            width: narrow,
            wrap: true,
            max_lines: None,
            truncate: false,
        },
    );
    assert!(
        overflow.lines.len() >= 2 && overflow.lines.concat() == "alphabet",
        "{:?}",
        overflow.lines
    );

    let hard = layout_paragraph(
        "a\nb",
        12.0,
        false,
        orbital_canvas_base::packet::TextFamily::Sans,
        ParagraphBox {
            width: 1000.0,
            wrap: true,
            max_lines: None,
            truncate: false,
        },
    );
    assert_eq!(hard.lines, ["a", "b"]);
}

fn animated_text_props(animation: Value) -> Value {
    free_props(
        json!([{"type": "draw-text", "text": "hi", "color": "#000000",
        "font": "12px sans-serif", "position": {"x": 5, "y": 5}, "animation": animation}]),
    )
}

fn text_alpha_and_x(packet: &GameRenderPacket) -> (f32, f32) {
    packet
        .draw_commands
        .iter()
        .find_map(|c| match c {
            DrawCommand::Text { color, pos, .. } => Some((color[3], pos.x)),
            _ => None,
        })
        .expect("a text")
}

#[test]
fn a_text_opacity_animation_paints_at_half_opacity_halfway() {
    let props = animated_text_props(json!({"durationMs": 1000, "keyframes": [
        {"at": 0, "opacity": 0, "offsetX": 0}, {"at": 1, "opacity": 1, "offsetX": 10}]}));
    let (alpha, x) = text_alpha_and_x(&from_props_at(&props, 500.0).unwrap());
    let (rest_alpha, rest_x) = text_alpha_and_x(&from_props_at(&props, 0.0).unwrap());
    assert!((alpha - 0.5).abs() < 1e-3, "{alpha}");
    assert!((x - rest_x - 5.0).abs() < 1e-3, "offsetX is halfway");
    assert_eq!(rest_alpha, 1.0, "control: no clock, no animation");
    assert!(has_active_fx(&props), "the canvas asks for frames");
}

#[test]
fn a_non_looping_text_animation_holds_its_final_values_and_a_plain_text_is_unchanged() {
    let props = animated_text_props(json!({"durationMs": 1000, "loop": false, "keyframes": [
        {"at": 0, "opacity": 0}, {"at": 1, "opacity": 0.25}]}));
    let (held, _) = text_alpha_and_x(&from_props_at(&props, 5000.0).unwrap());
    assert!((held - 0.25).abs() < 1e-3, "{held}");
    let looping = animated_text_props(json!({"durationMs": 1000, "keyframes": [
        {"at": 0, "opacity": 0}, {"at": 1, "opacity": 0.25}]}));
    let (wrapped, _) = text_alpha_and_x(&from_props_at(&looping, 5500.0).unwrap());
    assert!(
        (wrapped - 0.125).abs() < 1e-3,
        "control: a looping one wraps: {wrapped}"
    );
    let plain = free_props(
        json!([{"type": "draw-text", "text": "hi", "color": "#000000",
        "font": "12px sans-serif", "position": {"x": 5, "y": 5}}]),
    );
    let (alpha, _) = text_alpha_and_x(&from_props_at(&plain, 500.0).unwrap());
    assert_eq!(alpha, 1.0);
    assert!(
        !has_active_fx(&plain),
        "control: a plain text needs no frames"
    );
}

fn reveal_text(extra: Value, reveal: Value) -> Value {
    let mut node = json!({"type": "draw-text", "text": "abcdef", "color": "#000000",
        "font": "12px sans-serif", "anchor": "top-left", "position": {"x": 5, "y": 5}});
    for (key, value) in extra.as_object().unwrap() {
        node[key] = value.clone();
    }
    if !reveal.is_null() {
        node["reveal"] = reveal;
    }
    free_props(json!([node]))
}

fn six_reveal() -> Value {
    json!({"startMs": 1000, "perGlyphMs": 100, "durationMs": 100})
}

fn revealed_texts(props: &Value, at: f64) -> Vec<(String, f32, f32, f32)> {
    from_props_at(props, at)
        .unwrap()
        .draw_commands
        .iter()
        .filter_map(|c| match c {
            DrawCommand::Text {
                text, color, pos, ..
            } => Some((text.clone(), color[3], pos.x, pos.y)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_reveal_draws_no_glyph_before_its_start_and_the_leading_glyphs_midway() {
    let props = reveal_text(json!({}), six_reveal());
    assert!(
        revealed_texts(&props, 500.0).is_empty(),
        "before the start nothing shows"
    );
    let mid = revealed_texts(&props, 1250.0);
    let shown: Vec<&str> = mid.iter().map(|t| t.0.as_str()).collect();
    assert_eq!(shown, ["a", "b", "c"], "the fourth glyph starts at 1300");
    assert_eq!(mid[0].1, 1.0);
    assert_eq!(mid[1].1, 1.0);
    let eased = mid[2].1;
    assert!(
        eased > 0.5 && eased < 1.0,
        "ease-out is past the linear half at t=0.5: {eased}"
    );
    let rest = revealed_texts(&reveal_text(json!({}), Value::Null), 1250.0);
    assert!(
        mid[2].3 > rest[0].3,
        "an unfinished glyph sits lower than its place"
    );
    assert_eq!(mid[0].3, rest[0].3, "a finished glyph is at its place");
}

#[test]
fn a_finished_reveal_equals_the_text_without_reveal() {
    let control = reveal_text(json!({}), Value::Null);
    let done = reveal_text(json!({}), six_reveal());
    assert_eq!(
        revealed_texts(&done, 1600.0),
        revealed_texts(&control, 1600.0)
    );
    assert_eq!(
        revealed_texts(&done, 0.0),
        revealed_texts(&control, 0.0),
        "no clock is the end state"
    );
    assert_ne!(
        revealed_texts(&done, 1250.0),
        revealed_texts(&control, 1250.0)
    );
}

#[test]
fn a_reveal_offset_y_moves_unfinished_glyphs_and_defaults_to_six() {
    let lift = |reveal: Value| {
        let props = reveal_text(json!({}), reveal);
        let rest = revealed_texts(&reveal_text(json!({}), Value::Null), 1000.0);
        let first = revealed_texts(&props, 1000.0 + 1e-3);
        first[0].3 - rest[0].3
    };
    assert!((lift(six_reveal()) - 6.0).abs() < 0.01);
    assert!(
        (lift(json!({"startMs": 1000, "perGlyphMs": 100, "durationMs": 100, "offsetY": 20}))
            - 20.0)
            .abs()
            < 0.05
    );
}

#[test]
fn an_invalid_reveal_is_a_scene_error_naming_reveal() {
    for reveal in [
        json!({"perGlyphMs": 100, "durationMs": 100}),
        json!({"startMs": 1000, "durationMs": 100}),
        json!({"startMs": 1000, "perGlyphMs": 100}),
        json!({"startMs": -1, "perGlyphMs": 100, "durationMs": 100}),
        json!({"startMs": 1000, "perGlyphMs": -5, "durationMs": 100}),
        json!({"startMs": 1000, "perGlyphMs": 100, "durationMs": -1}),
        json!({"startMs": 1000, "perGlyphMs": 100, "durationMs": 100, "offsetY": "up"}),
        json!(7),
    ] {
        let bad = from_props(&reveal_text(json!({}), reveal.clone())).unwrap_err();
        assert!(
            matches!(&bad, CanvasSceneError::InvalidProp { field, .. } if field == "reveal"),
            "{reveal}: {bad:?}"
        );
    }
    assert!(
        from_props(&reveal_text(json!({}), six_reveal())).is_ok(),
        "control: a valid one maps"
    );
}

#[test]
fn a_wrapped_paragraph_counts_glyphs_across_lines() {
    let width = orbital_canvas_paint::vector::text_width("alpha beta", 12.0) + 0.5;
    let extra = json!({"text": "alpha beta go on", "width": width});
    let control = revealed_texts(&reveal_text(extra.clone(), Value::Null), 1000.0);
    assert_eq!(control.len(), 2);
    let first_line = control[0].0.chars().count();
    let reveal = json!({"startMs": 1000, "perGlyphMs": 100, "durationMs": 100});
    let props = reveal_text(extra, reveal);
    let at = 1000.0 + first_line as f64 * 100.0 + 50.0;
    let mid = revealed_texts(&props, at);
    let second: Vec<&(String, f32, f32, f32)> =
        mid.iter().filter(|t| t.3 > control[1].3 - 1.0).collect();
    assert_eq!(
        second.len(),
        1,
        "only the first glyph of line two has started: {mid:?}"
    );
    assert_eq!(
        second[0].0,
        control[1].0.chars().next().unwrap().to_string()
    );
    let inked = control[0].0.chars().filter(|c| !c.is_whitespace()).count();
    assert_eq!(
        mid.len(),
        inked + 1,
        "every inked glyph of line one shows: {mid:?}"
    );
    assert_eq!(revealed_texts(&props, 60_000.0), control);
}

#[test]
fn a_reveal_keeps_the_text_opacity_and_animation_and_starts_the_rightmost_unit_first_for_rtl() {
    let dim = reveal_text(
        json!({"opacity": 0.5, "animation": {"durationMs": 1000, "keyframes": [
        {"at": 0, "offsetX": 10}, {"at": 1, "offsetX": 10}]}}),
        six_reveal(),
    );
    let plain = reveal_text(
        json!({"opacity": 0.5, "animation": {"durationMs": 1000, "keyframes": [
        {"at": 0, "offsetX": 10}, {"at": 1, "offsetX": 10}]}}),
        Value::Null,
    );
    let shown = revealed_texts(&dim, 1150.0);
    let rest = revealed_texts(&plain, 1150.0);
    assert_eq!(
        shown[0].1, 0.5,
        "the glyph is at the text's own opacity once done"
    );
    assert!(
        (shown[0].2 - rest[0].2).abs() < 1e-3,
        "offsetX of the animation applies"
    );
    let arabic = reveal_text(json!({"text": "الربيع"}), six_reveal());
    let control = revealed_texts(&reveal_text(json!({"text": "الربيع"}), Value::Null), 1000.0);
    let widest =
        |texts: &[(String, f32, f32, f32)]| texts.iter().map(|t| t.2).fold(f32::MIN, f32::max);
    let first = revealed_texts(&arabic, 1050.0);
    assert_eq!(first.len(), 1, "one unit has started: {first:?}");
    assert!("الربيع".contains(first[0].0.as_str()));
    let later = revealed_texts(&arabic, 1250.0);
    assert!(later.len() >= 2, "more than one unit by then: {later:?}");
    assert_eq!(
        widest(&later),
        first[0].2,
        "the first unit is the rightmost; later ones come in to its left"
    );
    assert!(
        first[0].2 > control[0].2,
        "reading starts right of the line's left edge"
    );
    assert_eq!(revealed_texts(&arabic, 60_000.0), control);
    let latin = revealed_texts(&dim, 1250.0);
    assert!(
        latin[0].2 < latin[1].2 && latin[1].2 < latin[2].2,
        "control: Latin starts leftmost and moves right"
    );
}

fn started(extra: Value) -> Value {
    let mut animation = json!({"durationMs": 1000, "loop": false, "keyframes": [
        {"at": 0, "offsetX": 0, "opacity": 1}, {"at": 1, "offsetX": 20, "opacity": 0.25}]});
    for (key, value) in extra.as_object().unwrap() {
        animation[key] = value.clone();
    }
    animation
}

fn started_text(animation: Value) -> Value {
    animated_text_props(animation)
}

/// What a scene draws at epoch `t`: mapped at `t`, with the packet's own motion played there.
fn drawn_at(props: &Value, t: f64) -> GameRenderPacket {
    from_props_at(props, t)
        .unwrap()
        .resolved_at(0.0, t, true)
        .into_owned()
}

#[test]
fn a_start_ms_text_animation_runs_from_its_start_and_without_it_runs_from_the_scene_clock() {
    let props = started_text(started(json!({"startMs": 5000})));
    let at = |t: f64| text_alpha_and_x(&drawn_at(&props, t));
    let (a0, x0) = at(4000.0);
    assert_eq!(a0, 1.0, "before the start the first keyframe shows");
    let (a1, x1) = at(5500.0);
    assert!(
        (a1 - 0.625).abs() < 1e-3 && (x1 - x0 - 10.0).abs() < 1e-3,
        "{a1} {x1}"
    );
    assert!(
        (at(9000.0).0 - 0.25).abs() < 1e-3,
        "loop false clamps at the end"
    );
    assert!((at(0.0).0 - 0.25).abs() < 1e-3, "no clock is the end state");
    let plain = started_text(started(json!({})));
    let (control, _) = text_alpha_and_x(&from_props_at(&plain, 5500.0).unwrap());
    assert!(
        (control - 0.25).abs() < 1e-3,
        "control: without startMs it has long ended"
    );
    let (rest, _) = text_alpha_and_x(&from_props_at(&plain, 0.0).unwrap());
    assert_eq!(
        rest, 1.0,
        "control: without startMs no clock draws the rest frame"
    );
}

#[test]
fn a_looping_start_ms_animation_cycles_from_its_start() {
    let props = started_text(started(json!({"startMs": 5000, "loop": true})));
    let (alpha, _) = text_alpha_and_x(&drawn_at(&props, 6250.0));
    assert!(
        (alpha - 0.8125).abs() < 1e-3,
        "a quarter into the second cycle: {alpha}"
    );
    let (before, _) = text_alpha_and_x(&drawn_at(&props, 4500.0));
    assert_eq!(before, 1.0);
}

fn debug_of(props: &Value, t: f64) -> String {
    format!("{:?}", drawn_at(props, t).draw_commands)
}

#[test]
fn a_start_ms_shape_and_group_animation_start_at_their_instant() {
    let shape = |extra: Value, animation: Option<Value>| {
        let mut node = rect_shape(extra);
        if let Some(a) = animation {
            node["animation"] = a;
        }
        free_props(json!([node]))
    };
    let slide =
        json!({"startMs": 5000, "keyframes": [{"at": 0, "offsetX": 0}, {"at": 1, "offsetX": 20}]});
    let animated = shape(json!({}), Some(started(slide.clone())));
    for (t, offset) in [(4000.0, 0), (5500.0, 10), (9000.0, 20)] {
        assert_eq!(
            debug_of(&animated, t),
            debug_of(&shape(json!({"offsetX": offset}), None), 0.0),
            "shape at {t}"
        );
    }
    assert_eq!(
        debug_of(&animated, 0.0),
        debug_of(&shape(json!({"offsetX": 20}), None), 0.0),
        "no clock is the end state"
    );
    let grouped = free_props(json!([{"type": "draw-group", "position": {"x": 0, "y": 0},
        "items": [rect_shape(json!({}))], "animation": started(slide)}]));
    let quad_x = |t: f64| match drawn_at(&grouped, t).draw_commands.as_slice() {
        [_, DrawCommand::Quad { rect, .. }] => rect.x,
        other => panic!("one quad: {other:?}"),
    };
    assert_eq!(
        quad_x(4000.0),
        10.0,
        "before the start the group rests at the first keyframe"
    );
    assert_eq!(quad_x(5500.0), 20.0);
    assert_eq!(quad_x(9000.0), 30.0);
    assert_eq!(quad_x(0.0), 30.0, "no clock is the end state");
}

#[test]
fn an_invalid_animation_start_ms_is_a_scene_error_naming_animation() {
    for start in [json!(-1), json!("soon"), json!(true)] {
        let animation = started(json!({"startMs": start}));
        let mut shape = rect_shape(json!({}));
        shape["animation"] = animation.clone();
        let mut group = json!({"type": "draw-group", "position": {"x": 0, "y": 0}, "items": []});
        group["animation"] = animation.clone();
        for props in [
            free_props(json!([shape])),
            free_props(json!([group])),
            started_text(animation.clone()),
        ] {
            let bad = from_props(&props).unwrap_err();
            assert!(
                matches!(&bad, CanvasSceneError::InvalidProp { field, .. } if field == "animation"),
                "{start}: {bad:?}"
            );
        }
    }
    assert!(
        from_props(&started_text(started(json!({"startMs": 0})))).is_ok(),
        "control"
    );
}

#[test]
fn a_path_scale_multiplies_its_coordinates_and_an_invalid_one_is_a_scene_error() {
    let path = |extra: Value| {
        let mut node = json!({"type": "draw-shape", "shape": "path", "d": "M0 0 L24 0",
            "position": {"x": 10, "y": 10}, "stroke": "#000000", "strokeWidth": 1});
        for (key, value) in extra.as_object().unwrap() {
            node[key] = value.clone();
        }
        free_props(json!([node]))
    };
    let length = |props: &Value| {
        let packet = from_props(props).unwrap();
        let points: Vec<Vec2> = packet
            .draw_commands
            .iter()
            .find_map(|c| match c {
                DrawCommand::Path { segments, .. } | DrawCommand::GradientPath { segments, .. } => {
                    Some(
                        segments
                            .iter()
                            .filter_map(|s| match s {
                                PathSegment::MoveTo { x, y } | PathSegment::LineTo { x, y } => {
                                    Some(Vec2 { x: *x, y: *y })
                                }
                                _ => None,
                            })
                            .collect(),
                    )
                }
                _ => None,
            })
            .expect("a path");
        points[1].x - points[0].x
    };
    assert!(
        (length(&path(json!({}))) - 24.0).abs() < 1e-3,
        "control: scale 1"
    );
    assert!((length(&path(json!({"scale": 1}))) - 24.0).abs() < 1e-3);
    assert!((length(&path(json!({"scale": 0.5}))) - 12.0).abs() < 1e-3);
    for bad in [json!(0), json!(-1), json!("big")] {
        let err = from_props(&path(json!({"scale": bad.clone()}))).unwrap_err();
        assert!(
            matches!(&err, CanvasSceneError::InvalidProp { field, .. } if field == "scale"),
            "{bad}: {err:?}"
        );
    }
}

fn clipped_group(clip: Value) -> Value {
    let mut group = json!({"type": "draw-group", "position": {"x": 2, "y": 3}, "items": [
        {"type": "draw-shape", "shape": "rect", "id": "inside", "hit": true, "anchor": "top-left",
         "position": {"x": 0, "y": 0}, "width": 30, "height": 10, "fill": "#ff0000", "corners": {"radius": [2, 2, 2, 2]}},
        {"type": "draw-shape", "shape": "rect", "id": "edge", "hit": true, "anchor": "top-left",
         "position": {"x": 40, "y": 0}, "width": 30, "height": 10, "fill": "#ff0000", "corners": {"radius": [2, 2, 2, 2]}},
        {"type": "draw-shape", "shape": "rect", "id": "outside", "hit": true, "anchor": "top-left",
         "position": {"x": 0, "y": 40}, "width": 30, "height": 10, "fill": "#ff0000", "corners": {"radius": [2, 2, 2, 2]}},
        {"type": "draw-text", "text": "hello", "position": {"x": 0, "y": 5}, "color": "#ffffff",
         "font": "12px sans-serif", "align": "left", "baseline": "middle"}]});
    if !clip.is_null() {
        group["clip"] = clip;
    }
    free_props(json!([group]))
}

#[test]
fn a_rect_clip_scopes_sdf_shapes_and_text_and_cuts_the_regions() {
    let (packet, gathered) = from_props_mapped(
        &clipped_group(json!({"x": 0, "y": 0, "width": 50, "height": 20})),
        0.0,
    )
    .unwrap();
    let Some((clip, commands)) = packet.draw_commands.iter().find_map(|c| match c {
        DrawCommand::ClipGroup { clip, commands } => Some((clip, commands)),
        _ => None,
    }) else {
        panic!("a scoped clip group: {:?}", packet.draw_commands)
    };
    assert!(
        commands
            .iter()
            .any(|c| matches!(c, DrawCommand::Text { .. })),
        "text rides the clip"
    );
    let sdf = commands.iter().find_map(|c| match c {
        DrawCommand::Sdf { rect, .. } => Some(*rect),
        _ => None,
    });
    let sdf = sdf.expect("an SDF shape rides the clip, not refused by the group");
    let bounds = path_bounds(clip).expect("clip box");
    assert_eq!(
        (bounds.x, bounds.y),
        (sdf.x, sdf.y),
        "the clip is anchored at the group origin"
    );
    assert!(
        (bounds.w - sdf.w * 50.0 / 30.0).abs() < 1e-3 && (bounds.h - sdf.h * 2.0).abs() < 1e-3,
        "{bounds:?} {sdf:?}"
    );
    let ids: Vec<&str> = gathered.regions.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["inside", "edge"],
        "a region wholly outside the clip is dropped"
    );
    let edge = gathered
        .regions
        .iter()
        .find(|r| r.id == "edge")
        .unwrap()
        .rect;
    assert!(
        (edge.x + edge.w - (bounds.x + bounds.w)).abs() < 1e-3 && edge.w < sdf.w,
        "the straddling region is cut at the edge: {edge:?}"
    );
    let inside = gathered
        .regions
        .iter()
        .find(|r| r.id == "inside")
        .unwrap()
        .rect;
    assert_eq!(
        inside, sdf,
        "control: a region inside the clip is untouched"
    );
}

#[test]
fn control_a_group_without_a_clip_keeps_every_region_and_no_clip_group() {
    let (packet, gathered) = from_props_mapped(&clipped_group(Value::Null), 0.0).unwrap();
    assert!(!packet
        .draw_commands
        .iter()
        .any(|c| matches!(c, DrawCommand::ClipGroup { .. })));
    assert_eq!(gathered.regions.len(), 3);
    assert!(
        packet
            .draw_commands
            .iter()
            .filter(|c| matches!(c, DrawCommand::Sdf { .. }))
            .count()
            == 3
    );
}

#[test]
fn a_rounded_rect_clip_has_curved_corners_and_a_square_one_has_none() {
    let curves = |clip: Value| {
        let packet = packet_for(clipped_group(clip));
        packet.draw_commands.iter().find_map(|c| match c {
            DrawCommand::ClipGroup { clip, .. } => Some(
                clip.iter()
                    .filter(|s| matches!(s, PathSegment::CubicTo { .. }))
                    .count(),
            ),
            _ => None,
        })
    };
    assert_eq!(
        curves(json!({"x": 0, "y": 0, "width": 50, "height": 20, "radius": 4})),
        Some(4)
    );
    assert_eq!(
        curves(json!({"x": 0, "y": 0, "width": 50, "height": 20, "radius": [4, 0, 0, 4]})),
        Some(2)
    );
    assert_eq!(
        curves(json!({"x": 0, "y": 0, "width": 50, "height": 20})),
        Some(0),
        "control: no radius, no curve"
    );
}

#[test]
fn a_malformed_rect_clip_is_a_scene_error_naming_clip() {
    for clip in [
        json!({"x": 0, "y": 0, "height": 20}),
        json!({"x": 0, "y": 0, "width": 0, "height": 20}),
        json!({"x": 0, "y": 0, "width": 50, "height": 20, "radius": [1, 2, 3]}),
        json!({"x": 0, "y": 0, "width": 50, "height": 20, "radius": -2}),
    ] {
        let error = from_props(&clipped_group(clip.clone())).unwrap_err();
        assert!(
            matches!(&error, CanvasSceneError::InvalidProp { field, .. } if field == "clip"),
            "{clip}: {error:?}"
        );
    }
}

fn union_group(extra: Value, second: Value) -> Value {
    let mut group = json!({"type": "draw-group", "position": {"x": 0, "y": 0}, "smoothUnion": 8, "items": [
        {"type": "draw-shape", "shape": "capsule", "id": "a", "hit": true, "position": {"x": 10, "y": 20},
         "width": 24, "height": 24, "fill": "#ff0000"},
        {"type": "draw-shape", "shape": "capsule", "id": "b", "hit": true, "position": {"x": 40, "y": 20},
         "width": 24, "height": 24, "fill": "#0000ff"}]});
    for (key, value) in extra.as_object().unwrap() {
        group[key] = value.clone();
    }
    for (key, value) in second.as_object().unwrap() {
        group["items"][1][key] = value.clone();
    }
    free_props(json!([group]))
}

#[test]
fn a_smooth_union_group_paints_its_two_shapes_as_one_blend_and_keeps_both_regions() {
    let (packet, gathered) = from_props_mapped(&union_group(json!({}), json!({})), 0.0).unwrap();
    let blends: Vec<&DrawCommand> = packet
        .draw_commands
        .iter()
        .filter(|c| matches!(c, DrawCommand::SdfBlend { .. }))
        .collect();
    let [DrawCommand::SdfBlend {
        first,
        second,
        radius,
    }] = blends.as_slice()
    else {
        panic!("one blend: {:?}", packet.draw_commands)
    };
    assert_eq!(*radius, 8.0);
    assert!(matches!(first.as_ref(), DrawCommand::Sdf { fill, .. } if fill[0] == 1.0));
    assert!(matches!(second.as_ref(), DrawCommand::Sdf { fill, .. } if fill[2] == 1.0));
    assert!(
        sdf_commands(&packet).is_empty(),
        "the two shapes are not also painted alone"
    );
    let ids: Vec<&str> = gathered.regions.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["a", "b"],
        "each shape keeps its own pointer region"
    );
}

#[test]
fn control_a_group_without_smooth_union_paints_two_separate_shapes() {
    let mut props = union_group(json!({}), json!({}));
    props["drawables"][0]
        .as_object_mut()
        .unwrap()
        .remove("smoothUnion");
    let packet = packet_for(props);
    assert!(!packet
        .draw_commands
        .iter()
        .any(|c| matches!(c, DrawCommand::SdfBlend { .. })));
    assert_eq!(sdf_commands(&packet).len(), 2);
}

#[test]
fn a_smooth_union_that_cannot_blend_is_a_scene_error_naming_smooth_union() {
    let bad = [
        union_group(json!({"smoothUnion": 0}), json!({})),
        union_group(json!({"smoothUnion": "wide"}), json!({})),
        union_group(json!({}), json!({"stroke": "#ffffff"})),
        union_group(
            json!({}),
            json!({"shadow": {"color": "#000000", "blur": 4}}),
        ),
        union_group(json!({}), json!({"opacity": 0.5})),
        union_group(
            json!({}),
            json!({"material": {"wgsl": "fn shade(px: Pixel) -> vec4<f32> { return px.color; }", "params": [], "live": false}}),
        ),
    ];
    for props in bad {
        let error = from_props(&props).unwrap_err();
        assert!(
            matches!(&error, CanvasSceneError::InvalidProp { field, .. } if field == "smoothUnion"),
            "{error:?}"
        );
    }
    let mut three = union_group(json!({}), json!({}));
    let extra = three["drawables"][0]["items"][0].clone();
    three["drawables"][0]["items"]
        .as_array_mut()
        .unwrap()
        .push(extra);
    assert!(
        matches!(from_props(&three).unwrap_err(), CanvasSceneError::InvalidProp { field, .. } if field == "smoothUnion"),
        "three children"
    );
    let mut dashed = union_group(json!({}), json!({}));
    dashed["drawables"][0]["items"][0]["stroke"] = json!("#ffffff");
    dashed["drawables"][0]["items"][0]["strokeDash"] = json!([4, 2]);
    assert!(
        matches!(from_props(&dashed).unwrap_err(), CanvasSceneError::InvalidProp { field, .. } if field == "smoothUnion"),
        "dashed stroke"
    );
    let mut solid = union_group(json!({}), json!({}));
    solid["drawables"][0]["items"][0]["stroke"] = json!("#ffffff");
    assert!(
        from_props(&solid).is_ok(),
        "control: a solid stroke on the first shape blends"
    );
}

fn sprite_src(
    fit: Option<&str>,
    url: &str,
    w: f64,
    h: f64,
) -> Option<orbital_canvas_base::packet::Rect> {
    let mut sprite = json!({ "type": "draw-sprite", "id": "photo", "asset": { "url": url }, "position": { "x": 0, "y": 0 }, "width": w, "height": h, "anchor": "top-left" });
    if let Some(fit) = fit {
        sprite["fit"] = json!(fit);
    }
    packet_for(json!({ "width": 800, "height": 600, "drawables": [sprite] }))
        .draw_commands
        .into_iter()
        .find_map(|c| match c {
            DrawCommand::Sprite { src, .. } => Some(src),
            _ => None,
        })
}

#[test]
fn a_cover_sprite_crops_the_whole_image_centred_to_its_aspect_and_a_plain_sprite_takes_it_all() {
    let url = "test://images/cover-400x200.png";
    let mut bytes = Vec::new();
    image::RgbaImage::new(400, 200)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    orbital_canvas_paint::asset::register_asset(url, bytes.into(), Some("image/png"));
    let square = sprite_src(Some("cover"), url, 100.0, 100.0).expect("drawn");
    assert_eq!(
        (square.x, square.y, square.w, square.h),
        (100.0, 0.0, 200.0, 200.0)
    );
    let tall = sprite_src(Some("cover"), url, 50.0, 100.0).expect("drawn");
    assert_eq!((tall.x, tall.y, tall.w, tall.h), (150.0, 0.0, 100.0, 200.0));
    let wide = sprite_src(Some("cover"), url, 400.0, 50.0).expect("drawn");
    assert_eq!((wide.x, wide.y, wide.w, wide.h), (0.0, 75.0, 400.0, 50.0));
    let plain = sprite_src(None, url, 100.0, 100.0).expect("drawn");
    assert_eq!(
        (plain.x, plain.y, plain.w, plain.h),
        (0.0, 0.0, 400.0, 200.0),
        "control: no fit draws the whole image"
    );
}

fn sdf_shadow(shadow: Value) -> Option<orbital_canvas_base::packet::SdfShadow> {
    let packet = packet_for(free_props(json!([rect_shape(
        json!({"corners": {"radius": [4, 4, 4, 4], "kind": "round"}, "shadow": shadow})
    )])));
    match sdf_commands(&packet).as_slice() {
        [DrawCommand::Sdf { shadow, .. }] => *shadow,
        other => panic!("one sdf expected: {other:?}"),
    }
}

#[test]
fn an_sdf_shadow_takes_its_offset_and_a_hard_offset_shadow_draws_with_no_blur() {
    let soft = sdf_shadow(json!({"color": "#000000", "blur": 8, "offsetX": 4, "offsetY": 6}))
        .expect("soft");
    assert_eq!((soft.offset.x, soft.offset.y), (4.0, 6.0));
    let hard = sdf_shadow(json!({"color": "#000000", "blur": 0, "offsetX": 6, "offsetY": 6}))
        .expect("hard offset shadow draws");
    assert_eq!((hard.offset.x, hard.offset.y), (6.0, 6.0));
    assert!(
        hard.sigma > 0.0 && hard.sigma <= 0.5,
        "a crisp edge, never a zero sigma: {}",
        hard.sigma
    );
    assert!(
        sdf_shadow(json!({"color": "#000000", "blur": 0})).is_none(),
        "control: no blur and no offset draws no shadow"
    );
    let plain = sdf_shadow(json!({"color": "#000000", "blur": 8})).expect("plain");
    assert_eq!(
        (plain.offset.x, plain.offset.y),
        (0.0, 0.0),
        "control: no offset stays put"
    );
}

fn text_group(group: Value) -> Value {
    let mut base = json!({"type": "draw-group", "position": {"x": 100, "y": 50}, "items": [
        {"type": "draw-text", "id": "t", "hit": true, "text": "Tilted", "anchor": "top-left",
         "position": {"x": 0, "y": 0}, "color": "#ffffff", "font": "bold 20px sans-serif"},
        rect_shape(json!({"position": {"x": 0, "y": 30}, "width": 10, "height": 10}))]});
    for (key, value) in group.as_object().unwrap() {
        base[key] = value.clone();
    }
    free_props(json!([base]))
}

fn first_text(packet: &GameRenderPacket) -> (Vec2, f32, f32) {
    packet
        .draw_commands
        .iter()
        .find_map(|c| match c {
            DrawCommand::Text {
                pos,
                size,
                rotation,
                ..
            } => Some((*pos, *size, *rotation)),
            _ => None,
        })
        .expect("the group painted its text")
}

#[test]
fn text_in_a_rotated_group_paints_rotated_about_the_group_position_with_its_siblings() {
    let (packet, gathered) = from_props_mapped(&text_group(json!({"rotate": 0.5})), 0.0).unwrap();
    let (pos, size, rotation) = first_text(&packet);
    assert!((rotation - 0.5).abs() < 1e-5, "{rotation}");
    assert!((size - 20.0).abs() < 1e-3);
    let (flat_pos, _, _) = first_text(&packet_for(text_group(json!({}))));
    let (dx, dy) = (flat_pos.x - 100.0, flat_pos.y - 50.0);
    let (sin, cos) = 0.5_f32.sin_cos();
    assert!(
        (pos.x - (100.0 + dx * cos - dy * sin)).abs() < 1e-3
            && (pos.y - (50.0 + dx * sin + dy * cos)).abs() < 1e-3,
        "the text anchor turns about the group position: {pos:?}"
    );
    assert!(
        packet
            .draw_commands
            .iter()
            .any(|c| matches!(c, DrawCommand::Poly { .. })),
        "the sibling shape paints, rotated"
    );
    let hit = &gathered.text_hits[0].lines[0];
    let width = hit.edges.last().unwrap() - hit.edges.first().unwrap();
    assert!(hit.bottom > hit.top && width > 0.0);
}

#[test]
fn a_rotated_text_hit_covers_the_rotated_bounding_box() {
    let (_, flat) = from_props_mapped(&text_group(json!({})), 0.0).unwrap();
    let (_, turned) = from_props_mapped(
        &text_group(json!({"rotate": std::f64::consts::FRAC_PI_2})),
        0.0,
    )
    .unwrap();
    let line = |g: &Gathered| {
        let l = &g.text_hits[0].lines[0];
        (
            l.edges.last().unwrap() - l.edges.first().unwrap(),
            l.bottom - l.top,
        )
    };
    let (flat_w, flat_h) = line(&flat);
    let (turned_w, turned_h) = line(&turned);
    assert!((turned_w - flat_h).abs() < 1e-2, "{turned_w} vs {flat_h}");
    assert!((turned_h - flat_w).abs() < 1e-2, "{turned_h} vs {flat_w}");
}

#[test]
fn control_text_in_an_unrotated_group_keeps_rotation_zero_and_its_place() {
    let (flat_pos, _, _) = first_text(&packet_for(text_group(json!({}))));
    assert!(
        (flat_pos.x - 100.0).abs() < 1e-3,
        "an unrotated group only moves its text"
    );
    for group in [json!({}), json!({"rotate": 0.0})] {
        let packet = packet_for(text_group(group));
        let (pos, _, rotation) = first_text(&packet);
        assert_eq!(rotation, 0.0);
        assert_eq!(pos, flat_pos);
    }
}

#[test]
fn text_in_a_rotated_scaled_group_carries_both() {
    let packet = packet_for(text_group(json!({"rotate": 0.3, "scale": 2})));
    let (_, size, rotation) = first_text(&packet);
    assert!((rotation - 0.3).abs() < 1e-5);
    assert!((size - 40.0).abs() < 1e-3);
}

fn sdf_group(group: Value) -> Value {
    let mut base = json!({"type": "draw-group", "position": {"x": 10, "y": 10}, "items": [
        {"type": "draw-shape", "shape": "rect", "position": {"x": 0, "y": 0}, "anchor": "top-left",
         "width": 40, "height": 20, "fill": "#ff0000", "corners": {"radius": [8, 8, 8, 8]}}]});
    for (key, value) in group.as_object().unwrap() {
        base[key] = value.clone();
    }
    free_props(json!([base]))
}

#[test]
fn an_sdf_shape_in_a_moved_scaled_rotated_group_follows_the_transform() {
    let moved = packet_for(sdf_group(json!({})));
    let scaled = packet_for(sdf_group(json!({"scale": 2, "rotate": 0.4})));
    let moved_sdf = sdf_commands(&moved);
    let [DrawCommand::Sdf {
        rect: r0,
        corners: c0,
        rotation: a0,
        ..
    }] = moved_sdf.as_slice()
    else {
        panic!("one sdf in the moved group: {:?}", moved.draw_commands)
    };
    let scaled_sdf = sdf_commands(&scaled);
    let [DrawCommand::Sdf {
        rect: r1,
        corners: c1,
        rotation: a1,
        ..
    }] = scaled_sdf.as_slice()
    else {
        panic!("one sdf in the scaled group: {:?}", scaled.draw_commands)
    };
    assert!(
        (r0.x - 10.0).abs() < 1e-3 && (r0.y - 10.0).abs() < 1e-3,
        "{r0:?}"
    );
    assert!((r1.w - 2.0 * r0.w).abs() < 1e-3 && (r1.h - 2.0 * r0.h).abs() < 1e-3);
    assert!((c1[0] - 2.0 * c0[0]).abs() < 1e-3);
    assert!((a1 - a0 - 0.4).abs() < 1e-5);
}

fn offset_group(extra: Value) -> Value {
    let mut group = json!({"type": "draw-group", "position": {"x": 0, "y": 0}, "items": [
        {"type": "draw-shape", "shape": "rect", "position": {"x": 0, "y": 0}, "width": 256, "height": 256,
         "fill": "#ff0000"}]});
    for (key, value) in extra.as_object().unwrap() {
        group[key] = value.clone();
    }
    json!({"projection": "free", "scale": 1.0, "drawables": [group]})
}

fn quad_origin(packet: &GameRenderPacket) -> (f32, f32) {
    packet
        .draw_commands
        .iter()
        .find_map(|c| match c {
            DrawCommand::Quad { rect, .. } => Some((rect.x, rect.y)),
            _ => None,
        })
        .expect("grouped rectangle")
}

#[test]
fn a_groups_static_offset_moves_it_and_adds_to_the_animated_one() {
    let (x0, y0) = quad_origin(&packet_for(offset_group(json!({}))));
    let (x1, y1) = quad_origin(&packet_for(offset_group(
        json!({"offsetX": 256, "offsetY": 128}),
    )));
    assert!((x1 - x0 - TILE_WIDTH).abs() < 0.1, "{x1} {x0}");
    assert!((y1 - y0 - TILE_WIDTH / 2.0).abs() < 0.1, "{y1} {y0}");
    let animation = json!({"durationMs": 1000, "loop": false,
        "keyframes": [{"at": 0, "offsetX": 0}, {"at": 1, "offsetX": 256}]});
    let both = offset_group(json!({"offsetX": 256, "animation": animation.clone()}));
    let (bx, _) = quad_origin(&from_props_at(&both, 1000.0).unwrap());
    assert!((bx - x0 - 2.0 * TILE_WIDTH).abs() < 0.1, "{bx}");
    let only_animated = offset_group(json!({"animation": animation}));
    let (ax, _) = quad_origin(&from_props_at(&only_animated, 1000.0).unwrap());
    assert!(
        (ax - x0 - TILE_WIDTH).abs() < 0.1,
        "control: animation alone is unchanged"
    );
}

fn fit_node(text: &str, extra: Value) -> Value {
    let mut node = json!({"type": "draw-text", "id": "f", "text": text, "color": "#000000",
        "font": "bold 40px sans-serif", "position": {"x": 5, "y": 5}, "anchor": "top-left",
        "measure": true, "width": 80});
    for (key, value) in extra.as_object().unwrap() {
        node[key] = value.clone();
    }
    node
}

fn fitted(node: Value) -> (f32, Vec<(String, f32)>, Measured) {
    let (packet, gathered) = from_props_mapped(&free_props(json!([node])), 0.0).unwrap();
    let texts: Vec<(String, f32)> = packet
        .draw_commands
        .iter()
        .filter_map(|c| match c {
            DrawCommand::Text { text, size, .. } => Some((text.clone(), *size)),
            _ => None,
        })
        .collect();
    let size = texts.first().expect("painted").1;
    (size, texts, gathered.measured[0].clone())
}

const LONG: &str = "Poster type in a fixed cell spills over what is below";

#[test]
fn fit_shrink_steps_a_long_string_down_until_its_measured_height_is_within_the_box() {
    let (size, _, m) = fitted(fit_node(
        LONG,
        json!({"height": 60, "fit": "shrink", "minSize": 8}),
    ));
    assert!((8.0..40.0).contains(&size), "{size}");
    assert!(m.height <= 60.0 + 1e-3, "{}", m.height);
    assert!(m.width <= 80.0 + 1e-3, "{}", m.width);
    assert!((m.height - m.lines as f32 * size * 1.3).abs() < 1e-3);
    let (again, _, _) = fitted(fit_node(
        LONG,
        json!({"height": 60, "fit": "shrink", "minSize": 8}),
    ));
    assert_eq!(size, again);
}

#[test]
fn control_fit_none_keeps_the_declared_size_and_overflows_as_before() {
    for extra in [
        json!({"height": 60}),
        json!({"height": 60, "fit": "none"}),
        json!({}),
    ] {
        let (size, _, m) = fitted(fit_node(LONG, extra));
        assert_eq!(size, 40.0);
        assert!(m.height > 60.0);
    }
}

#[test]
fn control_text_that_already_fits_keeps_its_declared_size() {
    let (size, _, m) = fitted(fit_node(
        "Hi",
        json!({"height": 60, "fit": "shrink", "width": 200}),
    ));
    assert_eq!(size, 40.0);
    assert_eq!(m.lines, 1);
}

#[test]
fn fit_shrink_steps_down_for_one_unbreakable_word_wider_than_the_box() {
    let (size, texts, m) = fitted(fit_node(
        "Extraordinarily",
        json!({"height": 200, "fit": "shrink", "minSize": 8}),
    ));
    assert!(size < 40.0, "{size}");
    assert_eq!(texts.len(), 1);
    assert!(m.width <= 80.0 + 1e-3, "{}", m.width);
}

#[test]
fn fit_shrink_stops_at_min_size_and_paints_there_overflowing() {
    let (size, _, m) = fitted(fit_node(
        LONG,
        json!({"height": 20, "fit": "shrink", "minSize": 18}),
    ));
    assert_eq!(size, 18.0);
    assert!(m.height > 20.0);
}

#[test]
fn fit_shrink_respects_max_lines() {
    let (_, texts, m) = fitted(fit_node(
        LONG,
        json!({"height": 60, "fit": "shrink", "minSize": 8, "maxLines": 2, "width": 200}),
    ));
    assert!(texts.len() <= 2);
    assert!(m.height <= 60.0 + 1e-3);
}

#[test]
fn fit_shrink_works_for_right_to_left_text() {
    let (size, _, m) = fitted(fit_node(
        "عنوان طويل جدا يتجاوز الصندوق المحدد له في الصفحة",
        json!({"height": 60, "fit": "shrink", "minSize": 8}),
    ));
    assert!(size < 40.0);
    assert!(m.height <= 60.0 + 1e-3);
}

#[test]
fn fit_shrink_inside_a_rotated_group_fits_the_same_and_keeps_the_rotation() {
    let group = |rotate: f64| {
        free_props(
            json!([{"type": "draw-group", "position": {"x": 100, "y": 50}, "rotate": rotate,
            "items": [fit_node(LONG, json!({"height": 60, "fit": "shrink", "minSize": 8}))]}]),
        )
    };
    let flat_size = flat_size_of(&group(0.0));
    let (_, tilted_size, rotation) = first_text(&packet_for(group(0.5)));
    assert!((rotation - 0.5).abs() < 1e-5);
    assert!((flat_size - tilted_size).abs() < 1e-3);
    assert!(flat_size < 40.0);
}

fn flat_size_of(props: &Value) -> f32 {
    first_text(&packet_for(props.clone())).1
}

#[test]
fn fit_shrink_needs_a_width_and_a_height_and_names_the_prop_otherwise() {
    for node in [
        fit_node(LONG, json!({"fit": "shrink"})),
        json!({"type": "draw-text", "text": "x", "color": "#000", "position": {"x": 1, "y": 1},
            "fit": "shrink", "height": 20}),
        fit_node(LONG, json!({"fit": "grow", "height": 20})),
        fit_node(LONG, json!({"fit": "shrink", "height": 20, "minSize": 0})),
        fit_node(LONG, json!({"height": -1})),
    ] {
        assert!(from_props_mapped(&free_props(json!([node])), 0.0).is_err());
    }
}

fn headed_text(sem: Value) -> Value {
    json!({"type": "draw-text", "id": "title", "hit": true, "text": "Overview", "anchor": "top-left",
        "position": {"x": 0, "y": 0}, "color": "#111111", "semantic": sem})
}

#[test]
fn a_heading_semantic_carries_its_level_and_no_level_is_none() {
    let (_, gathered) = from_props_mapped(
        &free_props(json!([headed_text(
            json!({"role": "heading", "level": 2, "name": "Overview"})
        )])),
        0.0,
    )
    .unwrap();
    assert_eq!(gathered.semantic[0].level, Some(2));
    let (_, plain) = from_props_mapped(
        &free_props(json!([headed_text(
            json!({"role": "heading", "name": "Overview"})
        )])),
        0.0,
    )
    .unwrap();
    assert_eq!(plain.semantic[0].level, None, "control: no level is None");
    let (_, null_level) = from_props_mapped(
        &free_props(json!([headed_text(
            json!({"role": "text", "level": null, "name": "Overview"})
        )])),
        0.0,
    )
    .unwrap();
    assert_eq!(
        null_level.semantic[0].level, None,
        "edge: a null level is no level, as plain text sends it"
    );
    for edge in [1, 6] {
        let (_, edge_level) = from_props_mapped(
            &free_props(json!([headed_text(
                json!({"role": "heading", "level": edge, "name": "Overview"})
            )])),
            0.0,
        )
        .unwrap();
        assert_eq!(edge_level.semantic[0].level, Some(edge as u8));
    }
}

#[test]
fn a_semantic_level_outside_one_to_six_or_not_an_integer_is_refused() {
    for level in [json!(0), json!(7), json!(-1), json!(2.5), json!("2")] {
        let refused = from_props_mapped(
            &free_props(json!([headed_text(
                json!({"role": "heading", "level": level.clone(), "name": "Overview"})
            )])),
            0.0,
        )
        .unwrap_err();
        assert!(
            matches!(&refused, CanvasSceneError::InvalidProp { field, .. } if field == "semantic.level"),
            "{level}: {refused:?}"
        );
    }
}

#[test]
fn a_link_semantic_carries_its_href_and_no_href_is_none() {
    let (_, gathered) = from_props_mapped(
        &free_props(json!([headed_text(
            json!({"role": "link", "name": "About", "href": "/about"})
        )])),
        0.0,
    )
    .unwrap();
    assert_eq!(gathered.semantic[0].href.as_deref(), Some("/about"));
    let (_, plain) = from_props_mapped(
        &free_props(json!([headed_text(
            json!({"role": "link", "name": "About"})
        )])),
        0.0,
    )
    .unwrap();
    assert_eq!(plain.semantic[0].href, None, "control: no href is None");
    for absent in [json!(null), json!("")] {
        let (_, none) = from_props_mapped(
            &free_props(json!([headed_text(
                json!({"role": "link", "name": "About", "href": absent})
            )])),
            0.0,
        )
        .unwrap();
        assert_eq!(none.semantic[0].href, None, "edge: null and empty are no href");
    }
}

#[test]
fn a_semantic_href_that_is_not_a_string_is_refused() {
    for href in [json!(7), json!(true), json!(["/about"])] {
        let refused = from_props_mapped(
            &free_props(json!([headed_text(
                json!({"role": "link", "name": "About", "href": href.clone()})
            )])),
            0.0,
        )
        .unwrap_err();
        assert!(
            matches!(&refused, CanvasSceneError::InvalidProp { field, .. } if field == "semantic.href"),
            "{href}: {refused:?}"
        );
    }
}

#[test]
fn a_combobox_semantic_carries_whether_it_is_expanded_and_none_when_unsaid() {
    for open in [true, false] {
        let (_, gathered) = from_props_mapped(
            &free_props(json!([headed_text(
                json!({"role": "combobox", "name": "Language", "expanded": open})
            )])),
            0.0,
        )
        .unwrap();
        assert_eq!(gathered.semantic[0].expanded, Some(open));
    }
    for absent in [json!({"role": "combobox", "name": "Language"}), json!({"role": "combobox", "name": "Language", "expanded": null})] {
        let (_, plain) =
            from_props_mapped(&free_props(json!([headed_text(absent)])), 0.0).unwrap();
        assert_eq!(plain.semantic[0].expanded, None, "control: unsaid is None");
    }
    let refused = from_props_mapped(
        &free_props(json!([headed_text(
            json!({"role": "combobox", "name": "Language", "expanded": "yes"})
        )])),
        0.0,
    )
    .unwrap_err();
    assert!(
        matches!(&refused, CanvasSceneError::InvalidProp { field, .. } if field == "semantic.expanded"),
        "{refused:?}"
    );
}

#[test]
fn an_option_semantic_names_the_node_that_owns_it() {
    let (_, gathered) = from_props_mapped(
        &free_props(json!([headed_text(
            json!({"role": "option", "name": "English", "owner": "langPick"})
        )])),
        0.0,
    )
    .unwrap();
    assert_eq!(gathered.semantic[0].owner.as_deref(), Some("langPick"));
    for absent in [json!({"role": "option", "name": "English"}), json!({"role": "option", "name": "English", "owner": ""}), json!({"role": "option", "name": "English", "owner": null})] {
        let (_, plain) =
            from_props_mapped(&free_props(json!([headed_text(absent)])), 0.0).unwrap();
        assert_eq!(plain.semantic[0].owner, None, "control: no owner is None");
    }
    let refused = from_props_mapped(
        &free_props(json!([headed_text(
            json!({"role": "option", "name": "English", "owner": 3})
        )])),
        0.0,
    )
    .unwrap_err();
    assert!(
        matches!(&refused, CanvasSceneError::InvalidProp { field, .. } if field == "semantic.owner"),
        "{refused:?}"
    );
}

#[test]
fn a_group_semantic_with_a_name_is_a_node() {
    let (_, gathered) = from_props_mapped(
        &free_props(json!([headed_text(
            json!({"role": "group", "name": "Filters"})
        )])),
        0.0,
    )
    .unwrap();
    assert_eq!(gathered.semantic[0].role, "group");
    assert_eq!(gathered.semantic[0].name, "Filters");
}

#[test]
fn a_start_ms_fade_and_slide_is_mapped_once_and_played_by_the_packet() {
    let props = started_text(started(json!({"startMs": 5000})));
    let packet = from_props_at(&props, 5500.0).unwrap();
    assert!(
        matches!(
            packet.animations.as_slice(),
            [orbital_canvas_base::packet::Animation {
                clock: orbital_canvas_base::packet::AnimationClock::Epoch { start_ms },
                ..
            }] if *start_ms == 5000.0
        ),
        "{:?}",
        packet.animations
    );
    assert_eq!(
        text_alpha_and_x(&from_props_at(&props, 5500.0).unwrap()),
        text_alpha_and_x(&from_props_at(&props, 9000.0).unwrap()),
        "the mapping is the same at every clock: the packet moves it"
    );
    assert!(
        !has_started_animation(&json!({"type": "canvas", "drawables": props["drawables"].clone()})),
        "nothing left for a re-map"
    );
}

fn texts_in(commands: &[DrawCommand], out: &mut Vec<(f32, f32, f32)>) {
    for command in commands {
        match command {
            DrawCommand::Text { color, pos, .. } => out.push((color[3], pos.x, pos.y)),
            DrawCommand::ClipGroup { commands, .. }
            | DrawCommand::AdditiveGroup { commands }
            | DrawCommand::Target { commands, .. } => texts_in(commands, out),
            _ => {}
        }
    }
}

fn all_texts(packet: &GameRenderPacket) -> Vec<(f32, f32, f32)> {
    let mut found = Vec::new();
    texts_in(&packet.draw_commands, &mut found);
    found
}

fn in_clip(items: Value) -> Value {
    json!({"type": "draw-group", "position": {"x": 0, "y": 0},
        "clip": {"x": 0, "y": 0, "width": 100, "height": 100}, "items": items})
}

fn plain_text(x: f64, animation: Option<Value>) -> Value {
    let mut text = json!({"type": "draw-text", "text": "hi", "color": "#000000",
        "font": "12px sans-serif", "position": {"x": x, "y": 5}});
    if let Some(a) = animation {
        text["animation"] = a;
    }
    text
}

#[test]
fn an_epoch_fade_in_a_clip_is_a_packet_animation_with_a_path_and_matches_the_unfolded_one() {
    let fade = started(json!({"startMs": 5000}));
    let folded = free_props(json!([in_clip(json!([plain_text(
        5.0,
        Some(fade.clone())
    )]))]));
    let packet = from_props_at(&folded, 5500.0).unwrap();
    let [animation] = packet.animations.as_slice() else {
        panic!("one packet animation: {:?}", packet.animations)
    };
    assert!(!animation.path.is_empty(), "{animation:?}");
    assert!(
        !has_started_animation(
            &json!({"type": "canvas", "drawables": folded["drawables"].clone()})
        ),
        "nothing left for a re-map"
    );
    let plain = free_props(json!([plain_text(5.0, Some(fade))]));
    for t in [4000.0, 5500.0, 9000.0] {
        assert_eq!(
            all_texts(&drawn_at(&folded, t)),
            all_texts(&drawn_at(&plain, t)),
            "the clip's text fades as the unfolded one at {t}"
        );
    }
    let alpha = all_texts(&drawn_at(&folded, 5500.0))[0].0;
    assert!((alpha - 0.625).abs() < 1e-3, "{alpha}");
}

#[test]
fn control_an_epoch_fade_outside_any_fold_has_an_empty_path() {
    let props = free_props(json!([plain_text(
        5.0,
        Some(started(json!({"startMs": 5000})))
    )]));
    let packet = from_props_at(&props, 5500.0).unwrap();
    assert!(
        matches!(packet.animations.as_slice(), [a] if a.path.is_empty()),
        "{:?}",
        packet.animations
    );
}

#[test]
fn nested_clips_compose_the_animation_path() {
    let fade = started(json!({"startMs": 5000}));
    let props = free_props(json!([in_clip(json!([
        plain_text(1.0, None),
        in_clip(json!([plain_text(2.0, None), plain_text(3.0, Some(fade))]))
    ]))]));
    let packet = from_props_at(&props, 5500.0).unwrap();
    let [animation] = packet.animations.as_slice() else {
        panic!("one packet animation: {:?}", packet.animations)
    };
    assert_eq!(animation.path.len(), 2, "{animation:?}");
    let alphas: Vec<f32> = all_texts(&drawn_at(&props, 5500.0))
        .iter()
        .map(|t| t.0)
        .collect();
    assert_eq!(alphas.len(), 3);
    assert_eq!(alphas[0], 1.0);
    assert_eq!(alphas[1], 1.0, "the inner sibling rests");
    assert!((alphas[2] - 0.625).abs() < 1e-3, "{alphas:?}");
}

#[test]
fn a_scroll_animation_under_a_clip_moves_only_its_own_commands() {
    let scroll = json!({"clock": "scroll", "from": 0, "to": 100,
        "keyframes": [{"at": 0, "offsetY": 0}, {"at": 1, "offsetY": 3}]});
    let props = free_props(json!([in_clip(json!([
        plain_text(1.0, None),
        plain_text(2.0, Some(scroll))
    ]))]));
    let packet = from_props(&props).unwrap();
    let rest = all_texts(&packet);
    let mut scrolled = packet.clone();
    scrolled.scroll = 100.0;
    let moved = all_texts(&scrolled.resolved_at(0.0, 0.0, true));
    assert_eq!(moved[0].2, rest[0].2, "the sibling does not move");
    assert!(
        (moved[1].2 - rest[1].2).abs() > 0.5,
        "the animated text moves: {rest:?} {moved:?}"
    );
}

#[test]
fn an_untracked_opacity_stays_on_a_text_whose_slide_the_packet_plays() {
    let props = free_props(
        json!([{"type": "draw-text", "text": "hi", "color": "#000000",
        "font": "12px sans-serif", "position": {"x": 5, "y": 5}, "opacity": 0.5,
        "animation": {"startMs": 5000, "durationMs": 1000, "loop": false,
            "keyframes": [{"at": 0, "offsetX": 0}, {"at": 1, "offsetX": 20}]}}]),
    );
    let (alpha, _) = text_alpha_and_x(&drawn_at(&props, 5500.0));
    assert!((alpha - 0.5).abs() < 1e-4, "{alpha}");
}

#[test]
fn a_heading_text_without_hit_still_reaches_the_semantic_tree_with_its_level() {
    let props = free_props(json!([{"type": "draw-text", "id": "h", "text": "Title",
        "color": "#000000", "font": "32px sans-serif", "position": {"x": 5, "y": 5},
        "semantic": {"role": "heading", "level": 2}}]));
    let (_, gathered) = from_props_mapped(&props, 0.0).unwrap();
    let node = gathered
        .semantic
        .iter()
        .find(|n| n.id == "h")
        .expect("a heading node");
    assert_eq!((node.role.as_str(), node.level), ("heading", Some(2)));
    assert!(
        node.rect.w > 0.0 && node.rect.h > 0.0,
        "it covers its glyphs"
    );
}

fn halo_text(halo: Value) -> Value {
    free_props(
        json!([{"type": "draw-text", "text": "hi", "color": "#000000",
        "font": "16px sans-serif", "position": {"x": 5, "y": 5}, "halo": halo}]),
    )
}

fn text_halo(packet: &GameRenderPacket) -> Option<orbital_canvas_base::packet::Halo> {
    packet.draw_commands.iter().find_map(|c| match c {
        DrawCommand::Text { halo, .. } => Some(*halo),
        _ => None,
    })?
}

#[test]
fn a_draw_text_halo_reaches_the_text_command() {
    let halo = text_halo(&from_props(&halo_text(json!({"color": "#ffffff", "width": 4}))).unwrap())
        .expect("a halo");
    assert_eq!(halo.color, [1.0, 1.0, 1.0, 1.0]);
    assert_eq!(halo.width, 4.0);
    assert!(
        text_halo(&from_props(&animated_text_props(json!(null))).unwrap()).is_none(),
        "control: no halo declared, none drawn"
    );
    for bad in [
        json!({"color": "#fff", "width": 0}),
        json!({"color": "nope", "width": 3}),
        json!({"width": 3}),
    ] {
        let err = from_props(&halo_text(bad.clone())).unwrap_err();
        assert!(
            matches!(&err, CanvasSceneError::InvalidProp { field, .. } if field == "halo"),
            "{bad}: {err:?}"
        );
    }
}

fn inset_look(gap: f64) -> Value {
    json!({"stroke": "#00ff00", "strokeWidth": 1, "gap": gap})
}

#[test]
fn an_inset_paints_a_second_outline_of_the_same_rect_gap_px_inside_with_the_radii_reduced() {
    let cornered = |extra: Value| {
        let mut base = json!({"stroke": "#0000ff", "strokeWidth": 3,
            "corners": {"radius": [8, 8, 8, 8], "kind": "round"}});
        for (key, value) in extra.as_object().unwrap() {
            base[key] = value.clone();
        }
        packet_for(free_props(json!([rect_shape(base)])))
    };
    let control = cornered(json!({}));
    assert_eq!(
        sdf_commands(&control).len(),
        1,
        "control: no inset, one outline"
    );

    let inset = cornered(json!({"inset": inset_look(4.0)}));
    match sdf_commands(&inset).as_slice() {
        [outer, DrawCommand::Sdf {
            rect,
            corners,
            corner_kind,
            fill,
            stroke: Some(stroke),
            shadow: None,
            material: None,
            ..
        }] => {
            assert_eq!(
                format!("{outer:?}"),
                format!("{:?}", sdf_commands(&control)[0]),
                "the outer outline is unchanged"
            );
            assert_eq!(
                *rect,
                Rect {
                    x: 14.0,
                    y: 24.0,
                    w: 56.0,
                    h: 24.0
                }
            );
            assert_eq!((*corners, *corner_kind), ([4.0; 4], 0));
            assert_eq!(*fill, [0.0; 4], "the inset never fills");
            assert_eq!(stroke.width, 1.0);
            assert_eq!(stroke.color, [0.0, 1.0, 0.0, 1.0]);
        }
        other => panic!("an outer and an inner outline, got {other:?}"),
    }
}

#[test]
fn a_notched_rect_inset_follows_the_notch_and_a_gap_wider_than_the_rect_paints_no_inner_line() {
    let notched = packet_for(free_props(json!([rect_shape(json!({
        "stroke": "#0000ff", "strokeWidth": 2,
        "corners": {"radius": [6, 6, 6, 6], "kind": "notch"},
        "inset": inset_look(2.0)}))])));
    match sdf_commands(&notched).as_slice() {
        [_, DrawCommand::Sdf {
            corners,
            corner_kind,
            ..
        }] => assert_eq!((*corners, *corner_kind), ([4.0; 4], 2)),
        other => panic!("notched outer and inner, got {other:?}"),
    }
    let too_wide = packet_for(free_props(json!([rect_shape(json!({
        "stroke": "#0000ff", "corners": {"radius": [6, 6, 6, 6], "kind": "round"},
        "inset": inset_look(16.0)}))])));
    assert_eq!(
        sdf_commands(&too_wide).len(),
        1,
        "16 px gap leaves no inner rect in a 32 px tall rect"
    );
}

#[test]
fn an_inset_on_a_plain_rect_and_an_ellipse_keeps_the_outer_commands_and_adds_inner_ones() {
    let plain = |extra: Value| packet_for(free_props(json!([rect_shape(extra)])));
    let without = plain(json!({"stroke": "#0000ff", "strokeWidth": 2}));
    let with = plain(json!({"stroke": "#0000ff", "strokeWidth": 2, "inset": inset_look(3.0)}));
    assert!(with.draw_commands.len() > without.draw_commands.len());
    assert_eq!(
        format!("{:?}", &with.draw_commands[..without.draw_commands.len()]),
        format!("{:?}", without.draw_commands),
        "the outer outline is the control's commands"
    );
    let ellipse = |extra: Value| {
        let mut node = json!({"type": "draw-shape", "shape": "ellipse", "position": {"x": 50, "y": 50},
            "radiusX": 20, "radiusY": 10, "fill": "#ff0000", "stroke": "#0000ff"});
        for (key, value) in extra.as_object().unwrap() {
            node[key] = value.clone();
        }
        packet_for(free_props(json!([node])))
    };
    assert!(
        ellipse(json!({"inset": inset_look(3.0)}))
            .draw_commands
            .len()
            > ellipse(json!({})).draw_commands.len()
    );
}

#[test]
fn an_inset_needs_a_stroke_a_width_and_a_gap_above_zero() {
    for bad in [
        json!({"strokeWidth": 1, "gap": 2}),
        json!({"stroke": "#000000", "gap": 2}),
        json!({"stroke": "#000000", "strokeWidth": 1, "gap": 0}),
    ] {
        let err = from_props(&free_props(json!([rect_shape(json!({"inset": bad}))]))).unwrap_err();
        assert!(
            matches!(&err, CanvasSceneError::InvalidProp { field, .. } if field.starts_with("inset")),
            "{err:?}"
        );
    }
}

const ICON: &str = "data:image/svg+xml;utf8,%3Csvg%20xmlns%3D%22http%3A%2F%2Fwww.w3.org%2F2000%2Fsvg%22%20width%3D%2230%22%20height%3D%2210%22%2F%3E";

fn sprite_tints(node: &Value) -> Vec<Option<[f32; 4]>> {
    from_canvas_node(node)
        .unwrap()
        .draw_commands
        .iter()
        .filter_map(|c| match c {
            DrawCommand::Sprite { tint, .. } => Some(*tint),
            _ => None,
        })
        .collect()
}

fn icon_scene(extra: Value) -> Value {
    let mut sprite = json!({"type": "draw-sprite", "position": {"x": 0, "y": 0},
        "asset": {"url": ICON}});
    for (key, value) in extra.as_object().unwrap() {
        sprite[key] = value.clone();
    }
    json!({"type": "canvas", "projection": "free", "scale": 1.0, "drawables": [sprite]})
}

#[test]
fn a_sprite_tint_reaches_the_command_and_a_sprite_without_one_stays_untinted() {
    assert_eq!(
        sprite_tints(&icon_scene(json!({"tint": "#ff0000"}))),
        vec![Some([1.0, 0.0, 0.0, 1.0])]
    );
    assert_eq!(
        sprite_tints(&icon_scene(json!({}))),
        vec![None],
        "control: no tint, no recolour"
    );
    assert_eq!(
        sprite_tints(&icon_scene(json!({"tint": null}))),
        vec![None],
        "a null tint is none"
    );
}

#[test]
fn a_palette_name_tint_takes_the_theme_colour_like_any_colour_slot() {
    let mut scene = icon_scene(json!({"tint": "accent"}));
    scene["canvasTheme"] = json!({"palette": {"accent": "#0000ff"}});
    assert_eq!(sprite_tints(&scene), vec![Some([0.0, 0.0, 1.0, 1.0])]);
}

#[test]
fn a_tint_that_is_no_colour_and_a_tint_on_a_target_sprite_are_refused_by_name() {
    let field = |scene: &Value| match from_canvas_node(scene).unwrap_err() {
        CanvasSceneError::InvalidProp { field, .. } => field,
        other => panic!("not an invalid prop: {other:?}"),
    };
    assert_eq!(
        field(&icon_scene(json!({"tint": "no-such-colour"}))),
        "tint"
    );
    assert_eq!(field(&icon_scene(json!({"tint": 3}))), "tint");
    let target = icon_scene(json!({"texture": "target:t", "tint": "#ff0000",
        "width": 4, "height": 4}));
    assert_eq!(field(&target), "tint");
}

fn orb(extra: Value) -> Value {
    let mut node = json!({"type": "draw-scene3d", "id": "orb",
        "position": {"x": 20, "y": 10}, "width": 64, "height": 48,
        "scene": {"drawables": [{"type": "draw-mesh", "shape": "sphere",
            "position": {"x": 0, "y": 0},
            "material": {"kind": "standard", "color": "#ff0000"}}]}});
    for (key, value) in extra.as_object().unwrap() {
        node[key] = value.clone();
    }
    node
}

fn scene3d_canvas(drawable: Value) -> Value {
    json!({"type": "canvas", "projection": "free", "scale": 1.0, "width": 200, "height": 120,
        "backgroundColor": "#00000000", "drawables": [drawable]})
}

fn targets_of(packet: &GameRenderPacket) -> Vec<&DrawCommand> {
    packet
        .draw_commands
        .iter()
        .filter(|c| matches!(c, DrawCommand::Target { .. }))
        .collect()
}

fn scene_sprite(packet: &GameRenderPacket) -> (Rect, f32, f32) {
    packet
        .draw_commands
        .iter()
        .find_map(|c| match c {
            DrawCommand::Sprite {
                texture,
                dst,
                rotation,
                opacity,
                ..
            } if texture.starts_with("target:scene3d:") => Some((*dst, *rotation, *opacity)),
            _ => None,
        })
        .expect("the scene's sprite")
}

fn held_scene(command: &DrawCommand) -> &orbital_canvas_base::scene3d::Scene3D {
    match command {
        DrawCommand::Target { commands, .. } => match commands.as_slice() {
            [DrawCommand::Scene3D { scene }] => scene,
            other => panic!("a target holds the one scene: {other:?}"),
        },
        other => panic!("not a target: {other:?}"),
    }
}

#[test]
fn a_scene3d_drawable_maps_to_one_target_holding_the_scene_and_a_sprite_over_its_rect() {
    let packet = from_canvas_node(&scene3d_canvas(orb(json!({})))).unwrap();
    let targets = targets_of(&packet);
    let [target @ DrawCommand::Target {
        id,
        width,
        height,
        live,
        ..
    }] = targets.as_slice()
    else {
        panic!("one target: {:?}", packet.draw_commands)
    };
    assert_eq!(
        (id.as_str(), *width, *height, *live),
        ("scene3d:orb", 128, 96, false),
        "the texture is the rect times the default pixel ratio 2"
    );
    assert_eq!(held_scene(target).meshes.len(), 1);
    assert!(
        held_scene(target).grid.is_none(),
        "no ground grid by default"
    );
    let (dst, rotation, opacity) = scene_sprite(&packet);
    assert_eq!(
        (dst.x, dst.y, dst.w, dst.h, rotation, opacity),
        (20.0, 10.0, 64.0, 48.0, 0.0, 1.0)
    );
    let fine = from_canvas_node(&scene3d_canvas(orb(json!({"pixelRatio": 3})))).unwrap();
    assert!(matches!(
        targets_of(&fine)[..],
        [DrawCommand::Target {
            width: 192,
            height: 144,
            ..
        }]
    ));
}

#[test]
fn a_scene_without_a_scene3d_drawable_has_no_target_and_no_scene() {
    let plain = scene3d_canvas(json!({"type": "draw-shape", "shape": "rect",
        "position": {"x": 0, "y": 0}, "width": 4, "height": 4, "fill": "#ff0000"}));
    let packet = from_canvas_node(&plain).unwrap();
    assert!(targets_of(&packet).is_empty());
    assert!(!packet
        .draw_commands
        .iter()
        .any(|c| matches!(c, DrawCommand::Scene3D { .. } | DrawCommand::Sprite { .. })));
}

#[test]
fn drawables_without_an_id_get_distinct_targets_in_order() {
    let mut canvas = scene3d_canvas(orb(json!({})));
    let mut first = orb(json!({}));
    first.as_object_mut().unwrap().remove("id");
    let second = first.clone();
    canvas["drawables"] = json!([first, second, orb(json!({}))]);
    let packet = from_canvas_node(&canvas).unwrap();
    let ids: Vec<&str> = targets_of(&packet)
        .iter()
        .filter_map(|c| match c {
            DrawCommand::Target { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(ids, ["scene3d:#0", "scene3d:#1", "scene3d:orb"]);
}

#[test]
fn a_palette_name_in_a_mesh_colour_takes_the_theme_colour_and_a_literal_stays() {
    let colour = |drawable: Value| {
        let mut canvas = scene3d_canvas(drawable);
        canvas["canvasTheme"] = json!({"palette": {"brand": "#00ff00"}});
        let packet = from_canvas_node(&canvas).unwrap();
        held_scene(targets_of(&packet)[0]).meshes[0].material.color
    };
    let named = orb(json!({}));
    let mut named = named;
    named["scene"]["drawables"][0]["material"]["color"] = json!("brand");
    assert_eq!(colour(named), [0.0, 1.0, 0.0]);
    assert_eq!(
        colour(orb(json!({}))),
        [1.0, 0.0, 0.0],
        "control: a literal stays"
    );
}

#[test]
fn the_page_scroll_moves_the_scene_sprite_and_leaves_its_target() {
    let packet = from_canvas_node(&scene3d_canvas(orb(json!({})))).unwrap();
    let before = serde_json::to_value(targets_of(&packet)[0]).unwrap();
    let scrolled = packet.clone().scrolled(15.0);
    let (dst, ..) = scene_sprite(&scrolled);
    assert_eq!((dst.x, dst.y), (20.0, -5.0));
    assert_eq!(
        serde_json::to_value(targets_of(&scrolled)[0]).unwrap(),
        before
    );
    let (still, ..) = scene_sprite(&packet.scrolled(0.0));
    assert_eq!(still.y, 10.0, "control: no scroll, no move");
}

fn spinning() -> Value {
    json!({"durationMs": 1000, "keyframes": [
        {"at": 0, "rotateY": 0}, {"at": 1, "rotateY": 6.0}]})
}

#[test]
fn the_scenes_own_animation_plays_on_the_clock_and_rests_without_one() {
    let mut node = orb(json!({}));
    node["scene"]["drawables"][0]["animation"] = spinning();
    let canvas = scene3d_canvas(node.clone());
    let at = |ms: f64| {
        let packet = from_canvas_node_at(&canvas, ms).unwrap();
        serde_json::to_value(&held_scene(targets_of(&packet)[0]).meshes).unwrap()
    };
    assert_ne!(at(1000.0), at(1250.0), "two instants, two poses");
    assert_eq!(at(1250.0), at(2250.0), "a looping animation repeats");
    let mut still = node;
    still["scene"]["drawables"][0]
        .as_object_mut()
        .unwrap()
        .remove("animation");
    let unanimated = from_canvas_node_at(&scene3d_canvas(still), 1250.0).unwrap();
    assert_eq!(
        at(0.0),
        serde_json::to_value(&held_scene(targets_of(&unanimated)[0]).meshes).unwrap(),
        "no clock is the rest pose, which is the unanimated scene"
    );
}

#[test]
fn the_drawables_own_animation_moves_and_rotates_the_whole_object() {
    let animation = json!({"durationMs": 1000, "keyframes": [
        {"at": 0, "rotateZ": 0, "scale": 1, "offsetX": 0, "opacity": 1},
        {"at": 1, "rotateZ": 1.0, "scale": 2, "offsetX": 10, "opacity": 0.5}]});
    let canvas = scene3d_canvas(orb(json!({"animation": animation})));
    let at = |ms: f64| scene_sprite(&from_canvas_node_at(&canvas, ms).unwrap());
    let (rest, rest_rotation, rest_opacity) = at(0.0);
    assert_eq!(
        (rest.x, rest.w, rest_rotation, rest_opacity),
        (20.0, 64.0, 0.0, 1.0)
    );
    let (mid, rotation, opacity) = at(500.0);
    assert!((rotation - 0.5).abs() < 1e-4 && (opacity - 0.75).abs() < 1e-4);
    assert!(
        (mid.w - 96.0).abs() < 1e-3,
        "scale about the centre: {mid:?}"
    );
    assert!(
        (mid.x + mid.w / 2.0 - (20.0 + 32.0 + 5.0)).abs() < 1e-3,
        "{mid:?}"
    );
    let target = from_canvas_node_at(&canvas, 500.0).unwrap();
    assert!(matches!(
        targets_of(&target)[..],
        [DrawCommand::Target { width: 128, .. }]
    ));
}

#[test]
fn a_scroll_clocked_animation_on_the_drawable_is_played_by_the_packet() {
    let animation = json!({"clock": "scroll", "from": 0, "to": 100,
        "keyframes": [{"at": 0, "rotate": 0}, {"at": 1, "rotate": 90}]});
    let packet = from_canvas_node(&scene3d_canvas(orb(json!({"animation": animation})))).unwrap();
    assert_eq!(packet.animations.len(), 1);
    let sprite_at = packet
        .draw_commands
        .iter()
        .position(|c| matches!(c, DrawCommand::Sprite { .. }))
        .unwrap();
    assert!(packet.animations[0].first <= sprite_at && sprite_at < packet.animations[0].end);
    let mut scrolled = packet.clone();
    scrolled.scroll = 50.0;
    let half = scrolled.resolved_at(0.0, 0.0, true).into_owned();
    let (_, rotation, _) = scene_sprite(&half);
    assert!(
        (rotation - std::f32::consts::FRAC_PI_4).abs() < 1e-3,
        "half the scroll turns it a half of 90 degrees: {rotation}"
    );
    let (_, rest, _) = scene_sprite(&packet);
    assert_eq!(rest, 0.0, "control: the mapper draws the rest frame");
}

#[test]
fn a_scene3d_with_a_semantic_is_one_node_over_its_rect_and_a_hit_is_a_region() {
    let node = orb(json!({"semantic": {"role": "img", "name": "A red orb"}, "hit": true}));
    let (_, gathered) = from_canvas_node_mapped(&scene3d_canvas(node), 0.0).unwrap();
    let [semantic] = gathered.semantic.as_slice() else {
        panic!("one semantic node: {:?}", gathered.semantic)
    };
    assert_eq!(
        (semantic.role.as_str(), semantic.name.as_str()),
        ("img", "A red orb")
    );
    assert_eq!(
        (
            semantic.rect.x,
            semantic.rect.y,
            semantic.rect.w,
            semantic.rect.h
        ),
        (20.0, 10.0, 64.0, 48.0)
    );
    assert_eq!(gathered.regions.len(), 1);
    let (_, plain) = from_canvas_node_mapped(&scene3d_canvas(orb(json!({}))), 0.0).unwrap();
    assert!(
        plain.semantic.is_empty() && plain.regions.is_empty(),
        "control: neither declared"
    );
}

#[test]
fn a_scene3d_missing_what_it_needs_is_refused_by_field() {
    let field = |extra: Value, remove: &str| {
        let mut node = orb(extra);
        node.as_object_mut().unwrap().remove(remove);
        match from_canvas_node(&scene3d_canvas(node)).unwrap_err() {
            CanvasSceneError::InvalidProp { field, .. } => field,
            other => panic!("not an invalid prop: {other:?}"),
        }
    };
    assert_eq!(field(json!({}), "scene"), "scene");
    assert_eq!(field(json!({}), "width"), "width");
    assert_eq!(field(json!({}), "height"), "height");
    assert_eq!(field(json!({"pixelRatio": 0}), "id"), "pixelRatio");
    assert_eq!(field(json!({"width": -4}), "id"), "width");
}

#[test]
fn only_a_scene3d_that_moves_wants_frames() {
    let still = scene3d_canvas(orb(json!({})));
    assert!(
        !has_active_fx(&still),
        "control: a still scene needs no frames"
    );
    let live = |node: Value| {
        let (_, gathered) = from_canvas_node_mapped(&scene3d_canvas(node), 0.0).unwrap();
        gathered.live_scenes3d
    };
    assert!(live(orb(json!({}))).is_empty(), "control: nothing moves");
    let mut moving = orb(json!({}));
    moving["scene"]["drawables"][0]["animation"] = spinning();
    assert!(
        !has_active_fx(&scene3d_canvas(moving.clone())),
        "the page is not remapped for it"
    );
    let found = live(moving);
    assert_eq!(found.len(), 1, "the moving scene is recorded");
    let (at_rest, at_later) = (
        found[0].scene_at(0.0).unwrap(),
        found[0].scene_at(250.0).unwrap(),
    );
    assert_ne!(
        serde_json::to_value(&at_rest.meshes).unwrap(),
        serde_json::to_value(&at_later.meshes).unwrap(),
        "the recorded scene plays at the clock"
    );
    let shader = orb(json!({"scene": {"drawables": [{"type": "draw-mesh",
        "position": {"x": 0, "y": 0},
        "material": {"kind": "shader", "wgsl": "x"}}]}}));
    assert!(scene_moves(&shader["scene"]), "a shader material plays too");
    let timed = orb(json!({"animation": {"durationMs": 500,
        "keyframes": [{"at": 0, "rotateZ": 0}, {"at": 1, "rotateZ": 1}]}}));
    assert!(
        has_active_fx(&scene3d_canvas(timed)),
        "the drawable's own time animation"
    );
}

#[test]
fn an_anchored_material_is_no_mapper_effect_the_host_reads_its_time_at_each_frame() {
    let plate = |material: Value| {
        json!({"type": "canvas", "drawables": [{"type": "draw-shape", "shape": "rect", "position": {"x": 0, "y": 0},
            "width": 10, "height": 10, "fill": "#000000", "material": material}]})
    };
    let shade = "fn shade(px: Pixel) -> vec4<f32> { return px.color; }";
    let anchored = plate(json!({"wgsl": shade, "anchor": 1000.0}));
    assert!(
        !has_active_fx(&anchored),
        "an anchor alone asks for no new mapping"
    );
    assert!(!active_fx_live(&anchored, 2000.0));
    let frames = json!([{"at": 0, "opacity": 1}, {"at": 1, "opacity": 1}]);
    let looping = json!({"type": "canvas", "drawables": [{"type": "draw-text", "text": "07", "color": "#000000",
        "font": "16px sans-serif", "position": {"x": 5, "y": 5}, "animation": {"durationMs": 2000, "keyframes": frames}}]});
    assert!(
        active_fx_live(&looping, 2000.0),
        "control: a looping animation is still mapped at the clock"
    );
}

#[test]
fn a_finished_paint_clock_animation_is_not_live_and_a_looping_one_is() {
    let text = |animation: Value| {
        json!({"type": "canvas", "drawables": [{"type": "draw-text", "text": "07", "color": "#000000",
            "font": "16px sans-serif", "position": {"x": 5, "y": 5}, "animation": animation}]})
    };
    let frames = json!([{"at": 0, "opacity": 1}, {"at": 1, "opacity": 1}]);
    let epoch = 1.8e12;
    let once = text(json!({"durationMs": 1, "loop": false, "keyframes": frames.clone()}));
    assert!(
        !active_fx_live(&once, epoch),
        "a non-looping paint-clock animation has finished on the epoch clock: no frames"
    );
    assert!(
        has_active_fx(&once),
        "it is still mapped at the clock once, for its end state"
    );
    assert!(
        active_fx_live(
            &text(json!({"durationMs": 2000, "keyframes": frames.clone()})),
            epoch
        ),
        "control: a looping one keeps moving"
    );
    assert!(
        active_fx_live(
            &text(json!({"durationMs": 1000, "loop": false, "keyframes": frames})),
            500.0
        ),
        "control: inside its duration on a scene clock it still plays"
    );
}

fn broken_words_box(width: f32, wrap: bool, truncate: bool) -> ParagraphBox {
    ParagraphBox {
        width,
        wrap,
        max_lines: None,
        truncate,
    }
}

fn sans_at(text: &str, size: f32) -> f32 {
    orbital_canvas_paint::vector::shape(text, size, false, TextFamily::Sans).width
}

#[test]
fn a_paragraph_laid_out_again_comes_from_the_cache_unchanged() {
    let text = "cache round trip alpha beta gamma delta epsilon";
    let frame = broken_words_box(sans_at("cache round trip alpha", 12.0), true, false);
    let first = layout_paragraph(text, 12.0, false, TextFamily::Sans, frame);
    let again = layout_paragraph(text, 12.0, false, TextFamily::Sans, frame);
    assert!(first.lines.len() >= 2, "{:?}", first.lines);
    assert_eq!(first, again);
    let fresh =
        paragraph::layout_paragraph_uncached(text, 12.0, false, TextFamily::Sans, frame);
    assert_eq!(first, fresh);
    assert!(paragraph::paragraph_cache_len() >= 1);
}

#[test]
fn control_a_different_width_wrap_or_line_cap_is_its_own_layout() {
    let text = "keyed control one two three four five\nsix seven eight nine ten";
    let wide_width = sans_at(text, 12.0) * 2.0;
    let wide = layout_paragraph(
        text,
        12.0,
        false,
        TextFamily::Sans,
        broken_words_box(wide_width, true, false),
    );
    let narrow = layout_paragraph(
        text,
        12.0,
        false,
        TextFamily::Sans,
        broken_words_box(sans_at("keyed control one", 12.0), true, false),
    );
    assert!(narrow.lines.len() > wide.lines.len(), "{:?}", narrow.lines);
    let flat = layout_paragraph(
        text,
        12.0,
        false,
        TextFamily::Sans,
        broken_words_box(sans_at("keyed control one", 12.0), false, false),
    );
    assert_eq!(flat.lines.len(), 2);
    assert_ne!(flat, narrow);
    let capped = layout_paragraph(
        text,
        12.0,
        false,
        TextFamily::Sans,
        ParagraphBox {
            max_lines: Some(1),
            ..broken_words_box(sans_at("keyed control one", 12.0), true, false)
        },
    );
    assert!(capped.lines.len() <= 1, "{:?}", capped.lines);
    assert_ne!(capped, narrow);
    let wide_again = layout_paragraph(
        text,
        12.0,
        false,
        TextFamily::Sans,
        broken_words_box(wide_width, true, false),
    );
    assert_eq!(wide, wide_again);
}

#[test]
fn edge_an_empty_paragraph_is_cached_like_any_other() {
    let frame = broken_words_box(100.0, true, false);
    let first = layout_paragraph("", 12.0, false, TextFamily::Sans, frame);
    let again = layout_paragraph("", 12.0, false, TextFamily::Sans, frame);
    assert_eq!(first, again);
}

#[test]
fn a_word_wider_than_its_box_breaks_at_the_last_grapheme_that_fits() {
    let width = sans_at("Temperature", 12.0) * 0.77;
    let laid = layout_paragraph(
        "Temperature",
        12.0,
        false,
        TextFamily::Sans,
        broken_words_box(width, true, false),
    );
    assert!(laid.lines.len() >= 2, "{:?}", laid.lines);
    assert!(
        laid.widths.iter().all(|w| *w <= width + 1e-3),
        "{:?}",
        laid.widths
    );
    assert_eq!(laid.lines.concat(), "Temperature");
    assert_eq!(laid.starts[1], laid.lines[0].chars().count());
    assert!(laid.broken);

    let control = layout_paragraph(
        "Air temperature",
        12.0,
        false,
        TextFamily::Sans,
        broken_words_box(sans_at("Air temperature", 12.0) + 1.0, true, false),
    );
    assert_eq!(control.lines, ["Air temperature"]);
    assert!(!control.broken);
}

#[test]
fn words_that_fit_are_never_broken_and_the_wide_one_starts_its_own_line() {
    let width = sans_at("Temperature", 12.0) * 0.77;
    let laid = layout_paragraph(
        "go Temperature ok",
        12.0,
        false,
        TextFamily::Sans,
        broken_words_box(width, true, false),
    );
    assert_eq!(laid.lines[0], "go");
    assert!(laid.widths.iter().all(|w| *w <= width + 1e-3));
    assert_eq!(laid.lines.join("").replace(' ', ""), "goTemperatureok");
}

#[test]
fn a_url_without_spaces_breaks_and_wrap_off_is_unchanged() {
    let url = "api.openair.city/v1/forecast";
    let width = sans_at(url, 12.0) * 0.5;
    let laid = layout_paragraph(
        url,
        12.0,
        false,
        TextFamily::Sans,
        broken_words_box(width, true, false),
    );
    assert!(laid.lines.len() >= 2);
    assert!(laid.widths.iter().all(|w| *w <= width + 1e-3));
    assert_eq!(laid.lines.concat(), url);
    let flat = layout_paragraph(
        url,
        12.0,
        false,
        TextFamily::Sans,
        broken_words_box(width, false, false),
    );
    assert_eq!(flat.lines, [url]);
    assert!(!flat.broken);
}

#[test]
fn a_grapheme_cluster_is_never_split() {
    let text = "e\u{0301}e\u{0301}e\u{0301}e\u{0301}e\u{0301}e\u{0301}";
    let width = sans_at("e\u{0301}e\u{0301}", 12.0) + 0.1;
    let laid = layout_paragraph(
        text,
        12.0,
        false,
        TextFamily::Sans,
        broken_words_box(width, true, false),
    );
    assert!(laid.lines.len() >= 2);
    assert!(
        laid.lines.iter().all(|l| !l.starts_with('\u{0301}')),
        "{:?}",
        laid.lines
    );
    assert_eq!(laid.lines.concat(), text);
}

#[test]
fn a_right_to_left_word_breaks_in_logical_order() {
    let text = "الربيعالربيعالربيع";
    let width = sans_at(text, 12.0) * 0.5;
    let laid = layout_paragraph(
        text,
        12.0,
        false,
        TextFamily::Sans,
        broken_words_box(width, true, false),
    );
    assert!(laid.lines.len() >= 2, "{:?}", laid.lines);
    assert_eq!(laid.lines.concat(), text);
}

#[test]
fn truncate_still_ellipsizes_a_wide_word_instead_of_breaking_it() {
    let width = sans_at("alpha", 12.0) * 0.6;
    let laid = layout_paragraph(
        "alphabet",
        12.0,
        false,
        TextFamily::Sans,
        broken_words_box(width, true, true),
    );
    assert_eq!(laid.lines.len(), 1);
    assert!(laid.lines[0].ends_with('\u{2026}'));
    assert!(!laid.broken);
}

#[test]
fn shrink_prefers_a_smaller_size_over_breaking_and_breaks_only_at_the_floor() {
    let width = sans_at("Temperature", 24.0) * 0.7;
    let fit = |min_size: f32| {
        let node = json!({"width": width, "height": 40.0, "fit": "shrink", "minSize": min_size});
        read_fit(&node, 1.0).unwrap().unwrap()
    };
    let frame = broken_words_box(width, true, false);
    let (size, shrunk) = fit_paragraph(
        "Temperature",
        24.0,
        false,
        TextFamily::Sans,
        frame,
        1.3,
        fit(4.0),
    );
    assert!((4.0..24.0).contains(&size));
    assert_eq!(shrunk.lines, ["Temperature"]);
    assert!(!shrunk.broken);

    let (size, floor) = fit_paragraph(
        "Temperature",
        24.0,
        false,
        TextFamily::Sans,
        frame,
        1.3,
        fit(20.0),
    );
    assert!((size - 20.0).abs() < 1e-3);
    assert!(floor.lines.len() >= 2 && floor.broken);
    assert!(floor.widths.iter().all(|w| *w <= width + 1e-3));
}

#[test]
fn a_broken_word_paints_and_measures_two_lines() {
    let width = sans_at("Temperature", 20.0) * 0.77;
    let node = json!({"type": "draw-text", "text": "Temperature", "color": "#000000",
        "font": "20px sans-serif", "position": {"x": 0, "y": 0}, "width": width,
        "id": "t", "measure": true});
    let (packet, gathered) = from_props_mapped(&free_props(json!([node])), 0.0).unwrap();
    let texts = packet
        .draw_commands
        .iter()
        .filter(|c| matches!(c, DrawCommand::Text { .. }))
        .count();
    assert!(texts >= 2, "{texts}");
    assert_eq!(gathered.measured[0].lines as usize, texts);
    assert!(gathered.measured[0].width <= width + 1e-3);
}

fn dashed_route(scale: f64) -> GameRenderPacket {
    packet_for(json!({
        "projection": "free", "scale": 1.0,
        "drawables": [{"type": "draw-shape", "id": "route", "shape": "path", "d": "M0 0 H100",
            "position": {"x": 0, "y": 0}, "scale": scale, "stroke": "#000000", "strokeWidth": 2,
            "strokeDash": [100, 100],
            "animation": {"clock": "scroll", "from": 0, "to": 100,
                "keyframes": [{"at": 0, "dashOffset": 100}, {"at": 1, "dashOffset": 0}]}}]
    }))
}

fn route_stroke(packet: &GameRenderPacket) -> orbital_canvas_base::packet::Stroke {
    packet
        .draw_commands
        .iter()
        .find_map(|c| match c {
            DrawCommand::Path { stroke: Some(s), .. } => Some(s.clone()),
            _ => None,
        })
        .expect("the route paints a stroked path")
}

#[test]
fn a_scroll_keyframe_dash_offset_moves_a_dashed_path_in_its_own_units() {
    let packet = dashed_route(2.0);
    let rest = route_stroke(&packet);
    let unit = route_stroke(&dashed_route(1.0)).dash[0] / 100.0;
    assert!(unit > 0.0);
    assert_eq!(rest.dash, vec![200.0 * unit, 200.0 * unit], "a path's dash follows its scale, like its outline");
    let offsets: Vec<_> = packet.animations[0].keyframes.iter().map(|k| k.dash_offset).collect();
    assert_eq!(offsets, vec![Some(200.0 * unit), Some(0.0)], "the keyframe offset is in the same units as the dash");
    let mut scrolled = packet.clone();
    scrolled.scroll = 50.0;
    let half = scrolled.resolved_at(0.0, 0.0, true).into_owned();
    assert!((route_stroke(&half).dash_offset - 100.0 * unit).abs() < 1e-3, "half the scroll draws half the route in");
}

#[test]
fn control_an_unscaled_path_keeps_its_authored_dash() {
    let packet = packet_for(json!({
        "projection": "free", "scale": 1.0,
        "drawables": [{"type": "draw-shape", "id": "box", "shape": "path", "d": "M0 0 H100",
            "position": {"x": 0, "y": 0}, "stroke": "#000000", "strokeDash": [4, 2]}]
    }));
    let unit = route_stroke(&dashed_route(1.0)).dash[0] / 100.0;
    assert_eq!(route_stroke(&packet).dash, vec![4.0 * unit, 2.0 * unit], "an unscaled path is unchanged");
}

#[test]
fn a_canvas_nodes_theme_layer_is_laid_over_its_host_theme_before_references_resolve() {
    let node = |layer: bool| {
        let mut n = json!({ "type": "canvas", "mode": "2d",
            "canvasTheme": { "palette": { "card": "#ffffff", "background": "#eeeeee" } },
            "drawables": [{ "type": "draw-shape", "fill": "card", "stroke": "background" }] });
        if layer {
            n["themeLayer"] = json!({ "palette": { "card": "#123456" } });
        }
        prepare_canvas_node(&n).expect("props")
    };
    let laid = node(true);
    assert_eq!(laid["drawables"][0]["fill"], "#123456");
    assert_eq!(laid["drawables"][0]["stroke"], "#eeeeee");
    assert!(laid.get("themeLayer").is_none(), "the layer is consumed, not drawn");
    assert_eq!(node(false)["drawables"][0]["fill"], "#ffffff");
}
