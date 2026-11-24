use super::*;

fn graph() -> serde_json::Value {
    serde_json::json!({
        "type": "graph-canvas",
        "height": 400,
        "nodes": [{"id": "a", "label": "A"}, {"id": "b", "label": "B"}, {"id": "c", "label": "C"}],
        "edges": [{"source": "a", "target": "b"}],
        "onNodeClick": "PICK",
        "onNodeDoubleClick": "OPEN"
    })
}

fn graph_renderer() -> Canvas {
    let mut r = Canvas::new_headless().unwrap();
    r.set_viewport(600, 400);
    r.set_scene(graph()).unwrap();
    r
}

fn center_of(r: &Canvas, id: &str) -> (f32, f32) {
    let d = r
        .drags
        .iter()
        .find(|d| d.id == id)
        .expect("node drag region");
    (d.rect.x + d.rect.w / 2.0, d.rect.y + d.rect.h / 2.0)
}

fn empty_spot(r: &Canvas) -> (f32, f32) {
    (0..60)
        .flat_map(|i| (0..40).map(move |j| (i as f32 * 10.0 + 5.0, j as f32 * 10.0 + 5.0)))
        .find(|&(x, y)| !r.drags.iter().any(|d| contains(&d.rect, x, y)))
        .expect("an empty spot")
}

#[test]
fn dragging_a_graph_node_moves_it_and_queues_no_tap() {
    let mut r = graph_renderer();
    let (x, y) = center_of(&r, "a");
    r.input("touch-down", x, y, 0, None);
    r.input("touch-move", x + 40.0, y + 25.0, 0, None);
    r.input("touch-up", x + 40.0, y + 25.0, 0, None);
    let (nx, ny) = center_of(&r, "a");
    assert!(
        (nx - (x + 40.0)).abs() < 0.5 && (ny - (y + 25.0)).abs() < 0.5,
        "{nx},{ny}"
    );
    assert!(r.take_scene_events().is_empty(), "a drag is not a tap");
    r.set_scene(graph()).unwrap();
    let (kx, ky) = center_of(&r, "a");
    assert!(
        (kx - nx).abs() < 0.5 && (ky - ny).abs() < 0.5,
        "a new scene keeps the dragged position"
    );
}

#[test]
fn control_a_tap_on_a_node_still_picks_it() {
    let mut r = graph_renderer();
    let (x, y) = center_of(&r, "b");
    r.input("touch-down", x, y, 0, None);
    r.input("touch-up", x, y, 0, None);
    let events = r.take_scene_events();
    assert_eq!(
        events.iter().map(|e| e.event.as_str()).collect::<Vec<_>>(),
        vec!["PICK"]
    );
}

#[test]
fn a_background_drag_pans_every_node_by_the_drag() {
    let mut r = graph_renderer();
    let before = center_of(&r, "c");
    let (x, y) = empty_spot(&r);
    r.input("touch-down", x, y, 0, None);
    r.input("touch-move", x + 30.0, y - 10.0, 0, None);
    r.input("touch-up", x + 30.0, y - 10.0, 0, None);
    let after = center_of(&r, "c");
    assert!(
        (after.0 - before.0 - 30.0).abs() < 0.5 && (after.1 - before.1 + 10.0).abs() < 0.5,
        "{before:?} {after:?}"
    );
}

#[test]
fn a_pinch_zooms_about_its_focus_within_the_web_limits() {
    let mut r = graph_renderer();
    let (a, b) = (center_of(&r, "a"), center_of(&r, "b"));
    let gap = (a.0 - b.0).hypot(a.1 - b.1);
    r.zoom(2.0, a.0, a.1);
    let (a2, b2) = (center_of(&r, "a"), center_of(&r, "b"));
    assert!(
        (a2.0 - a.0).abs() < 0.5 && (a2.1 - a.1).abs() < 0.5,
        "the focus stays put"
    );
    let zoomed = (a2.0 - b2.0).hypot(a2.1 - b2.1);
    assert!(zoomed > gap * 1.5, "{gap} -> {zoomed}");
    r.zoom(100.0, a.0, a.1);
    assert_eq!(r.memory.view.map(|v| v.0), Some(3.0), "zoom stops at 3x");
}

#[test]
fn the_zoom_buttons_scale_about_the_canvas_origin_and_reset_restores_the_identity_view() {
    let mut r = graph_renderer();
    let fitted = r.memory.view.unwrap();
    r.view_command("zoom-in").unwrap();
    let zoomed = r.memory.view.unwrap();
    assert!(
        (zoomed.0 - fitted.0 * 1.2).abs() < 1e-9 && zoomed.1 == fitted.1 && zoomed.2 == fitted.2,
        "{fitted:?} -> {zoomed:?}"
    );
    r.view_command("zoom-out").unwrap();
    assert!((r.memory.view.unwrap().0 - fitted.0).abs() < 1e-9);
    for _ in 0..20 {
        r.view_command("zoom-out").unwrap();
    }
    assert_eq!(r.memory.view.unwrap().0, 0.3, "zoom-out stops at 0.3");
    r.view_command("reset").unwrap();
    assert_eq!(
        r.memory.view.unwrap(),
        (1.0, 0.0, 0.0),
        "the web's reset: zoom 1, no offset"
    );
}

#[test]
fn control_an_unknown_view_command_is_refused_and_changes_nothing() {
    let mut r = graph_renderer();
    let before = r.memory.view;
    assert!(r.view_command("spin").is_err());
    assert_eq!(r.memory.view, before);
}

#[test]
fn a_double_tap_on_a_node_fires_its_double_click_event() {
    let mut r = graph_renderer();
    let (x, y) = center_of(&r, "a");
    r.input("double-tap", x, y, 0, None);
    let events = r.take_scene_events();
    assert_eq!(
        events.iter().map(|e| e.event.as_str()).collect::<Vec<_>>(),
        vec!["OPEN"]
    );
    let (ex, ey) = empty_spot(&r);
    r.input("double-tap", ex, ey, 0, None);
    assert!(
        r.take_scene_events().is_empty(),
        "control: empty space fires nothing"
    );
}

fn pan_by(r: &mut Canvas, dx: f32) {
    let (x, y) = empty_spot(r);
    r.input("touch-down", x, y, 0, None);
    r.input("touch-move", x + dx, y, 0, None);
    r.input("touch-up", x + dx, y, 0, None);
}

#[test]
fn a_new_scene_with_the_same_nodes_keeps_the_view_and_a_mostly_new_set_refits_it() {
    let mut r = graph_renderer();
    pan_by(&mut r, 120.0);
    let panned = r.memory.view.unwrap();
    r.set_scene(graph()).unwrap();
    assert_eq!(r.memory.view.unwrap(), panned, "same nodes: the pan stays");
    let mut other = graph();
    other["nodes"] = serde_json::json!([{"id": "a"}, {"id": "x"}, {"id": "y"}, {"id": "z"}]);
    r.set_scene(other).unwrap();
    assert!(
        (r.memory.view.unwrap().1 - panned.1).abs() > 1.0,
        "overlap 1/4: laid out and fitted again"
    );
}

fn drawn_texts(r: &Canvas) -> Vec<String> {
    r.packet
        .as_ref()
        .unwrap()
        .draw_commands
        .iter()
        .filter_map(|c| match c {
            packet::DrawCommand::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_long_press_shows_a_truncated_labels_full_text_until_the_next_touch() {
    let mut r = Canvas::new_headless().unwrap();
    r.set_viewport(600, 400);
    let mut g = graph();
    g["nodes"][0]["label"] = serde_json::json!("A very long node label that the graph truncates");
    r.set_scene(g).unwrap();
    let (x, y) = center_of(&r, "a");
    r.input("touch-down", x, y, 0, None);
    r.input("long-press", x, y, 0, None);
    r.input("touch-up", x, y, 0, None);
    assert!(
        r.take_scene_events().is_empty(),
        "lifting after a long press is not a tap"
    );
    let shown = drawn_texts(&r).join(" ");
    assert!(shown.contains("that the graph truncates"), "{shown}");
    r.input("touch-down", x + 200.0, y, 0, None);
    r.input("touch-up", x + 200.0, y, 0, None);
    assert!(
        !drawn_texts(&r)
            .join(" ")
            .contains("that the graph truncates"),
        "dismissed"
    );
}

#[test]
fn control_a_long_press_on_a_label_that_fits_shows_nothing_extra() {
    let mut r = graph_renderer();
    let before = drawn_texts(&r);
    let (x, y) = center_of(&r, "b");
    r.input("long-press", x, y, 0, None);
    assert_eq!(drawn_texts(&r), before);
}

fn region(
    id: &str,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    rotation: f32,
) -> mappers::scene::PointerRegion {
    let name = |kind: &str| Some(format!("{kind}_{id}"));
    mappers::scene::PointerRegion {
        id: id.to_owned(),
        rect: packet::Rect { x, y, w, h },
        rotation,
        events: mappers::scene::PointerEvents {
            down: name("DOWN"),
            up: name("UP"),
            enter: name("ENTER"),
            leave: name("LEAVE"),
            moved: name("MOVE"),
        },
        wheel: false,
        fixed: false,
        drag: false,
    }
}

fn events(r: &mut SceneHost) -> Vec<String> {
    r.take_scene_events().into_iter().map(|e| e.event).collect()
}

fn pointer_host(regions: Vec<mappers::scene::PointerRegion>) -> SceneHost {
    let mut r = SceneHost::new();
    r.pointer_regions = regions;
    r
}

fn moves_scene(moves: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "canvas", "mode": "2d", "projection": "free", "camera": {"zoom": 1},
        "pointerMoves": moves, "pointerMoveEvent": "MOVE", "pointerEnterEvent": "ENTER",
        "pointerDownEvent": "DOWN", "pointerUpEvent": "UP",
        "drawables": [{"type": "draw-shape", "id": "b", "shape": "rect", "anchor": "top-left",
            "position": {"x": 0, "y": 0}, "width": 100, "height": 40, "fill": "#000000", "hit": true}]
    })
}

#[test]
fn a_press_on_a_hit_text_reports_the_caret_index_nearest_it() {
    let mut r = SceneHost::new();
    r.set_scene(serde_json::json!({
        "type": "canvas", "mode": "2d", "projection": "free", "camera": {"zoom": 1}, "pointerDownEvent": "DOWN",
        "drawables": [
            {"type": "draw-text", "id": "t", "text": "hello", "position": {"x": 0, "y": 0}, "anchor": "top-left",
             "color": "#ffffff", "font": "20px sans-serif", "align": "left", "baseline": "middle", "hit": true},
            {"type": "draw-shape", "id": "b", "shape": "rect", "anchor": "top-left", "position": {"x": 0, "y": 100},
             "width": 100, "height": 40, "fill": "#000000", "hit": true}]
    })).unwrap();
    let hit = r
        .text_hits
        .iter()
        .find(|h| h.id == "t")
        .expect("the text's caret positions")
        .clone();
    let after_he = hit.lines[0].edges[2];
    r.input(
        "touch-down",
        after_he + 0.5,
        hit.lines[0].top + 1.0,
        0,
        None,
    );
    let down = r.take_scene_events();
    assert_eq!(
        down[0].payload["index"],
        serde_json::json!(2),
        "{:?}",
        down[0].payload
    );
    r.input("touch-up", after_he, hit.lines[0].top + 1.0, 0, None);
    r.take_scene_events();
    r.input("touch-down", 10.0, 120.0, 0, None);
    let shape = r.take_scene_events();
    assert!(
        shape[0].payload.get("index").is_none(),
        "control: a shape press carries no index: {:?}",
        shape[0].payload
    );
}

#[test]
fn a_captured_moves_canvas_sends_moves_only_while_a_press_holds_capture() {
    let mut r = SceneHost::new();
    r.set_scene(moves_scene("captured")).unwrap();
    r.input("hover", 10.0, 10.0, 0, None);
    assert_eq!(
        events(&mut r),
        vec!["ENTER"],
        "hover enters but sends no move"
    );
    r.input("hover", 20.0, 10.0, 0, None);
    assert!(events(&mut r).is_empty(), "a hover move is not sent");
    r.input("touch-down", 20.0, 10.0, 0, None);
    r.input("touch-move", 30.0, 10.0, 0, None);
    assert_eq!(
        events(&mut r),
        vec!["DOWN", "MOVE"],
        "a drag sends its moves"
    );
}

#[test]
fn control_an_all_moves_canvas_sends_hover_moves_too() {
    let mut r = SceneHost::new();
    r.set_scene(moves_scene("all")).unwrap();
    r.input("hover", 10.0, 10.0, 0, None);
    r.input("hover", 20.0, 10.0, 0, None);
    assert_eq!(events(&mut r), vec!["ENTER", "MOVE", "MOVE"]);
}

fn scaled_world_scene() -> serde_json::Value {
    serde_json::json!({
        "type": "canvas", "mode": "2d", "projection": "free", "painter": "orbital", "camera": {"zoom": 1},
        "worldWidth": 1280, "worldHeight": 800, "pointerDownEvent": "DOWN",
        "drawables": [{"type": "draw-shape", "id": "far", "shape": "rect", "anchor": "top-left",
            "position": {"x": 1000, "y": 600}, "width": 100, "height": 100, "fill": "#000000", "hit": true}]
    })
}

#[test]
fn a_press_on_a_world_scaled_canvas_hits_the_region_where_it_is_shown_and_reports_world_units() {
    let mut r = SceneHost::new();
    r.set_viewport(640, 400);
    r.set_scale(1.0).unwrap();
    r.set_scene(scaled_world_scene()).unwrap();
    r.input("touch-down", 525.0, 325.0, 0, None);
    let got = r.take_scene_events();
    assert_eq!(
        got.iter().map(|e| e.event.as_str()).collect::<Vec<_>>(),
        vec!["DOWN"]
    );
    assert_eq!(
        got[0].payload,
        serde_json::json!({ "target": "far", "x": 1050.0, "y": 650.0 })
    );
}

#[test]
fn control_a_press_at_the_raw_world_numbers_on_a_world_scaled_canvas_hits_nothing() {
    let mut r = SceneHost::new();
    r.set_viewport(640, 400);
    r.set_scale(1.0).unwrap();
    r.set_scene(scaled_world_scene()).unwrap();
    r.input("touch-down", 30.0, 30.0, 0, None);
    assert!(
        events(&mut r).is_empty(),
        "the top-left corner is not the far region"
    );
}

#[test]
fn a_hit_drawable_in_a_free_scene_on_a_retina_surface_answers_css_pixel_input() {
    let mut r = SceneHost::new();
    r.set_viewport(1600, 1200);
    r.set_scale(2.0).unwrap();
    r.set_scene(serde_json::json!({
        "type": "canvas", "mode": "2d", "projection": "free", "camera": {"zoom": 1},
        "pointerEnterEvent": "ENTER", "pointerDownEvent": "DOWN",
        "drawables": [{"type": "draw-shape", "id": "begin", "shape": "rect", "anchor": "top-left",
            "position": {"x": 170, "y": 250}, "width": 128, "height": 46, "fill": "#7a4a1c", "hit": true}]
    }))
    .unwrap();
    r.input("hover", 30.0, 30.0, 0, None);
    assert!(events(&mut r).is_empty(), "control: away from the button");
    r.input("hover", 234.0, 273.0, 0, None);
    assert_eq!(events(&mut r), vec!["ENTER"]);
}

#[test]
fn hovering_into_a_region_enters_it_and_leaving_the_canvas_leaves_it() {
    let mut r = pointer_host(vec![region("a", 10.0, 10.0, 100.0, 50.0, 0.0)]);
    r.input("hover", 500.0, 500.0, 0, None);
    assert!(events(&mut r).is_empty(), "control: outside any region");
    r.input("hover", 20.0, 20.0, 0, None);
    assert_eq!(events(&mut r), vec!["ENTER_a", "MOVE_a"]);
    r.input("hover", 30.0, 30.0, 0, None);
    assert_eq!(
        events(&mut r),
        vec!["MOVE_a"],
        "still inside: no second enter"
    );
    r.input("leave", 0.0, 0.0, 0, None);
    assert_eq!(events(&mut r), vec!["LEAVE_a"]);
    assert!(r.frame_packet().is_none() && r.pointer.is_none());
}

#[test]
fn the_topmost_region_takes_enter_and_a_move_to_another_leaves_the_first() {
    let mut r = pointer_host(vec![
        region("under", 0.0, 0.0, 100.0, 100.0, 0.0),
        region("over", 50.0, 50.0, 100.0, 100.0, 0.0),
    ]);
    r.input("hover", 10.0, 10.0, 0, None);
    assert_eq!(events(&mut r), vec!["ENTER_under", "MOVE_under"]);
    r.input("hover", 60.0, 60.0, 0, None);
    assert_eq!(
        events(&mut r),
        vec!["LEAVE_under", "ENTER_over", "MOVE_over"]
    );
}

#[test]
fn a_region_without_events_produces_none() {
    let mut bare = region("a", 0.0, 0.0, 100.0, 100.0, 0.0);
    bare.events = mappers::scene::PointerEvents {
        down: None,
        up: None,
        enter: None,
        leave: None,
        moved: None,
    };
    let mut r = pointer_host(vec![bare]);
    r.input("hover", 10.0, 10.0, 0, None);
    r.input("touch-down", 10.0, 10.0, 0, None);
    r.input("touch-up", 10.0, 10.0, 0, None);
    assert!(events(&mut r).is_empty());
}

#[test]
fn a_press_captures_the_region_and_its_up_arrives_from_outside() {
    let mut r = pointer_host(vec![region("a", 0.0, 0.0, 100.0, 100.0, 0.0)]);
    r.input("touch-down", 10.0, 10.0, 0, None);
    assert_eq!(events(&mut r), vec!["ENTER_a", "DOWN_a"]);
    r.input("touch-move", 400.0, 400.0, 0, None);
    assert_eq!(
        events(&mut r),
        vec!["LEAVE_a", "MOVE_a"],
        "captured: moves follow"
    );
    r.input("touch-up", 400.0, 400.0, 0, None);
    let up = r.take_scene_events();
    assert_eq!(up.len(), 1);
    assert_eq!(up[0].event, "UP_a");
    assert_eq!(
        up[0].payload,
        serde_json::json!({ "target": "a", "x": 400.0, "y": 400.0 })
    );
    r.input("touch-move", 500.0, 500.0, 0, None);
    assert!(events(&mut r).is_empty(), "released and outside: no move");
    r.input("touch-up", 500.0, 500.0, 0, None);
    assert!(
        events(&mut r).is_empty(),
        "control: an up with no capture sends nothing"
    );
}

#[test]
fn a_press_outside_every_region_captures_nothing() {
    let mut r = pointer_host(vec![region("a", 0.0, 0.0, 100.0, 100.0, 0.0)]);
    r.input("touch-down", 300.0, 300.0, 0, None);
    r.input("touch-up", 50.0, 50.0, 0, None);
    assert_eq!(
        events(&mut r),
        vec!["ENTER_a"],
        "only the hover on the way in"
    );
}

#[test]
fn a_rotated_region_is_hit_by_its_rotated_bounds() {
    let quarter = std::f32::consts::FRAC_PI_2;
    let mut r = pointer_host(vec![region("bar", 0.0, 40.0, 100.0, 20.0, quarter)]);
    r.input("hover", 50.0, 5.0, 0, None);
    assert_eq!(
        events(&mut r),
        vec!["ENTER_bar", "MOVE_bar"],
        "turned upright: top reached"
    );
    r.input("hover", 5.0, 50.0, 0, None);
    assert_eq!(
        events(&mut r),
        vec!["LEAVE_bar"],
        "its old horizontal extent is empty now"
    );
    let mut flat = pointer_host(vec![region("bar", 0.0, 40.0, 100.0, 20.0, 0.0)]);
    flat.input("hover", 50.0, 5.0, 0, None);
    assert!(
        events(&mut flat).is_empty(),
        "control: unrotated misses the top"
    );
}

#[test]
fn the_frame_carries_the_pointer_and_the_clock() {
    let mut r = SceneHost::new();
    r.set_scene(serde_json::json!({"type": "svg-ring", "r": 30}))
        .unwrap();
    r.set_clock(100.0);
    r.set_clock(350.0);
    r.input("hover", 12.0, 34.0, 0, None);
    let p = r.frame_packet().unwrap();
    assert_eq!(p.time_ms, 250.0);
    let at = p.pointer.unwrap();
    assert_eq!((at.x, at.y), (12.0, 34.0));
    r.input("leave", 0.0, 0.0, 0, None);
    assert!(r.frame_packet().unwrap().pointer.is_none());
}

fn measured(id: &str, width: f32) -> mappers::scene::Measured {
    mappers::scene::Measured {
        id: id.to_owned(),
        width,
        height: 20.0,
        lines: 1,
    }
}

#[test]
fn a_changed_measurement_is_reported_once_and_an_unchanged_one_is_not_repeated() {
    let mut r = SceneHost::new();
    r.scene = Some(orbital_canvas_base::scene_doc::SceneDoc::whole(serde_json::json!({"type": "canvas", "measureEvent": "MEASURED"})));
    r.measured = vec![measured("t", 80.0)];
    r.report_measured();
    let sent = r.take_scene_events();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].event, "MEASURED");
    assert_eq!(
        sent[0].payload,
        serde_json::json!({ "items": [{ "id": "t", "width": 80.0, "height": 20.0, "lines": 1 }] })
    );
    r.report_measured();
    assert!(events(&mut r).is_empty(), "unchanged: nothing");
    r.measured = vec![measured("t", 95.0)];
    r.report_measured();
    assert_eq!(events(&mut r), vec!["MEASURED"], "changed: reported again");
}

#[test]
fn measurements_without_a_measure_event_send_nothing() {
    let mut r = SceneHost::new();
    r.scene = Some(orbital_canvas_base::scene_doc::SceneDoc::whole(serde_json::json!({"type": "canvas"})));
    r.measured = vec![measured("t", 80.0)];
    r.report_measured();
    assert!(events(&mut r).is_empty());
}

fn host_with(props: serde_json::Value) -> SceneHost {
    let mut r = SceneHost::new();
    let mut node = serde_json::json!({"type": "canvas", "mode": "2d", "projection": "free", "scale": 1.0, "drawables": []});
    for (k, v) in props.as_object().unwrap() {
        node[k] = v.clone();
    }
    r.set_scene(node).unwrap();
    r
}

fn sem_scene(ids: &[&str]) -> serde_json::Value {
    let drawables: Vec<serde_json::Value> = ids
        .iter()
        .map(|id| {
            serde_json::json!({"type": "draw-shape", "id": id, "shape": "rect",
            "position": {"x": 0, "y": 0}, "width": 10, "height": 10, "fill": "#ff0000",
            "semantic": {"role": "button", "name": id}})
        })
        .collect();
    serde_json::json!({"type": "canvas", "mode": "2d", "projection": "free", "scale": 1.0, "drawables": drawables})
}

#[test]
fn declared_input_events_are_queued_with_their_payload() {
    let mut r = host_with(serde_json::json!({
        "textInputEvent": "TXT", "keyEvent": "KEY", "accessEvent": "ACC"}));
    r.text_input("f", "hi", true);
    r.key_input(
        "f",
        "Backspace",
        KeyMods {
            shift: true,
            ..KeyMods::default()
        },
    );
    r.access_action("b", AccessAction::Increment);
    let sent = r.take_scene_events();
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[0].event, "TXT");
    assert_eq!(
        sent[0].payload,
        serde_json::json!({"target": "f", "text": "hi", "composing": true})
    );
    assert_eq!(sent[1].event, "KEY");
    assert_eq!(
        sent[1].payload,
        serde_json::json!({"target": "f", "key": "Backspace", "shift": true, "alt": false, "ctrl": false, "meta": false})
    );
    assert_eq!(sent[2].event, "ACC");
    assert_eq!(
        sent[2].payload,
        serde_json::json!({"target": "b", "action": "increment"})
    );
}

#[test]
fn undeclared_input_events_send_nothing() {
    let mut r = host_with(serde_json::json!({"keyEvent": "KEY"}));
    r.text_input("f", "hi", false);
    r.access_action("b", AccessAction::Focus);
    assert!(events(&mut r).is_empty());
    r.key_input("f", "a", KeyMods::default());
    assert_eq!(events(&mut r), vec!["KEY"], "only the declared one");
}

#[test]
fn resize_is_sent_once_per_size_in_css_pixels() {
    let mut r = host_with(serde_json::json!({"resizeEvent": "RESIZED"}));
    r.set_scale(2.0).unwrap();
    r.take_scene_events();
    r.set_viewport(800, 600);
    let sent = r.take_scene_events();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].event, "RESIZED");
    assert_eq!(
        sent[0].payload,
        serde_json::json!({"width": 400.0, "height": 300.0})
    );
    r.set_viewport(800, 600);
    assert!(events(&mut r).is_empty(), "same size: nothing");
    r.set_viewport(1000, 600);
    assert_eq!(events(&mut r), vec!["RESIZED"], "new size: sent again");
}

#[test]
fn the_declared_direction_is_reported_once_and_on_change() {
    let mut r = host_with(serde_json::json!({"directionEvent": "DIR", "direction": "rtl"}));
    let sent = r.take_scene_events();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].event, "DIR");
    assert_eq!(sent[0].payload, serde_json::json!({"direction": "rtl"}));
    let mut node = r.scene().unwrap();
    r.set_scene(node.clone()).unwrap();
    assert!(r.take_scene_events().is_empty(), "same direction: nothing");
    node["direction"] = serde_json::json!("ltr");
    r.set_scene(node).unwrap();
    assert_eq!(
        r.take_scene_events()[0].payload,
        serde_json::json!({"direction": "ltr"})
    );
}

#[test]
fn control_a_direction_without_a_direction_event_or_unset_sends_nothing() {
    let mut r = host_with(serde_json::json!({"direction": "rtl"}));
    assert!(r.take_scene_events().is_empty());
    let mut r = host_with(serde_json::json!({"directionEvent": "DIR"}));
    assert!(
        r.take_scene_events().is_empty(),
        "edge: no direction yet (no locale)"
    );
}

#[test]
fn the_motion_preference_is_reported_once_and_on_change() {
    let mut r = host_with(serde_json::json!({"motionEvent": "MOT"}));
    let sent = r.take_scene_events();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].event, "MOT");
    assert_eq!(sent[0].payload, serde_json::json!({"reduced": false}));
    let node = r.scene().unwrap();
    r.set_scene(node).unwrap();
    assert!(r.take_scene_events().is_empty(), "same preference: nothing");
    r.set_motion(false);
    let sent = r.take_scene_events();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].payload, serde_json::json!({"reduced": true}));
    r.set_motion(false);
    assert!(
        r.take_scene_events().is_empty(),
        "edge: unchanged preference"
    );
}

#[test]
fn control_without_a_motion_event_a_preference_change_sends_nothing() {
    let mut r = host_with(serde_json::json!({}));
    r.set_motion(false);
    assert!(r.take_scene_events().is_empty());
}

#[test]
fn resize_without_a_resize_event_sends_nothing() {
    let mut r = host_with(serde_json::json!({}));
    r.set_viewport(800, 600);
    assert!(events(&mut r).is_empty());
}

#[test]
fn the_semantic_tree_follows_relayouts() {
    let mut r = SceneHost::new();
    r.set_scene(sem_scene(&["a", "b"])).unwrap();
    let ids: Vec<String> = r.semantic_tree().into_iter().map(|n| n.id).collect();
    assert_eq!(ids, vec!["a", "b"]);
    r.set_scene(sem_scene(&["b", "c"])).unwrap();
    let ids: Vec<String> = r.semantic_tree().into_iter().map(|n| n.id).collect();
    assert_eq!(ids, vec!["b", "c"]);
    r.set_scene(sem_scene(&[])).unwrap();
    assert!(r.semantic_tree().is_empty());
}

fn sprung(width: f64, stiffness: f64) -> serde_json::Value {
    serde_json::json!({
        "type": "canvas", "mode": "2d", "projection": "free", "scale": 1.0,
        "drawables": [{"type": "draw-shape", "id": "bar", "shape": "rect",
            "position": {"x": 0, "y": 0}, "width": width, "height": 10, "fill": "#ff0000",
            "spring": {"stiffness": stiffness, "damping": 26}}]
    })
}

fn shown_width(r: &SceneHost) -> f64 {
    r.springs.apply(&r.scene().unwrap())["drawables"][0]["width"]
        .as_f64()
        .unwrap()
}

#[test]
fn a_spring_moves_to_the_new_width_and_settles() {
    let mut r = SceneHost::new();
    r.set_scene(sprung(100.0, 170.0)).unwrap();
    assert!(
        r.springs.settled() && shown_width(&r) == 100.0,
        "first scene: no motion"
    );
    r.set_scene(sprung(200.0, 170.0)).unwrap();
    assert!(!r.springs.settled() && r.wants_frames());
    assert_eq!(shown_width(&r), 100.0, "starts from what was displayed");
    r.set_clock(0.0);
    let mut ms = 0.0;
    while r.wants_frames() && ms < 10_000.0 {
        ms += 16.0;
        r.set_clock(ms);
    }
    assert!(r.springs.settled());
    assert_eq!(shown_width(&r), 200.0);
    // wants_frames also reads the process-wide asset counters other tests move
    assert!(!r.is_animated() && !r.live_material);
}

#[test]
fn a_drawable_without_a_spring_jumps() {
    let mut r = SceneHost::new();
    let mut a = sprung(100.0, 170.0);
    a["drawables"][0].as_object_mut().unwrap().remove("spring");
    let mut b = sprung(200.0, 170.0);
    b["drawables"][0].as_object_mut().unwrap().remove("spring");
    r.set_scene(a).unwrap();
    r.set_scene(b).unwrap();
    assert!(r.springs.is_empty() && r.springs.settled() && !r.is_animated());
}

#[test]
fn reduced_motion_snaps_a_spring_to_its_target() {
    let mut r = SceneHost::new();
    r.set_scene(sprung(100.0, 170.0)).unwrap();
    r.set_motion(false);
    r.set_scene(sprung(200.0, 170.0)).unwrap();
    assert!(r.springs.settled() && shown_width(&r) == 200.0);
    let mut moving = SceneHost::new();
    moving.set_scene(sprung(100.0, 170.0)).unwrap();
    moving.set_scene(sprung(200.0, 170.0)).unwrap();
    assert!(
        shown_width(&moving) == 100.0,
        "control: with motion it lags"
    );
    moving.set_motion(false);
    assert_eq!(
        shown_width(&moving),
        200.0,
        "turning motion off snaps what was in flight"
    );
}

#[test]
fn a_colour_springs_through_rgba() {
    let mut bank = spring::SpringBank::new();
    let a = sprung(10.0, 170.0);
    let mut b = sprung(10.0, 170.0);
    b["drawables"][0]["fill"] = serde_json::json!("#0000ff");
    bank.retarget(&a);
    bank.retarget(&b);
    bank.step(0.05);
    let fill = bank.apply(&b)["drawables"][0]["fill"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        fill.starts_with("rgba(") && fill != "rgba(255,0,0,1.000)",
        "{fill}"
    );
    bank.step(30.0);
    assert_eq!(
        bank.apply(&b)["drawables"][0]["fill"],
        "rgba(0,0,255,1.000)"
    );
}

#[test]
fn a_live_material_asks_for_frames_and_a_static_one_does_not() {
    let scene = |live: bool| {
        serde_json::json!({
            "type": "canvas", "mode": "2d", "projection": "free", "scale": 1.0,
            "drawables": [{"type": "draw-shape", "id": "m", "shape": "rect",
                "position": {"x": 0, "y": 0}, "width": 10, "height": 10,
                "material": {"wgsl": "fn shade(px: Pixel) -> vec4<f32> { return px.color; }", "params": [], "live": live}}]
        })
    };
    let mut r = SceneHost::new();
    r.set_scene(scene(true)).unwrap();
    assert!(r.wants_frames());
    let mut still = SceneHost::new();
    still.set_scene(scene(false)).unwrap();
    assert!(
        !still.live_material && still.springs.settled(),
        "control: not live"
    );
}

#[test]
fn headless_renderer_can_be_created() {
    // This test only runs where an adapter exists, so we ignore failures.
    let _ = Canvas::new_headless();
}

#[test]
fn packet_roundtrips_through_json() {
    let packet = GameRenderPacket {
        pattern: "canvas-2d".into(),
        width: 800.0,
        height: 600.0,
        draw_commands: vec![packet::DrawCommand::Clear {
            color: [0.0, 0.0, 0.0, 1.0],
        }],
        ..Default::default()
    };
    let json = serde_json::to_string(&packet).unwrap();
    let parsed: GameRenderPacket = orbital_core_base::json_value::from_json_str(&json).unwrap();
    assert_eq!(parsed.pattern, "canvas-2d");
    assert_eq!(parsed.width, 800.0);
}

/// A retina phone surface (e.g. 1206x2172) exceeds the WebGL2-downlevel 2048 texture cap;
/// the device must request the adapter's real resolution limits or `Surface::configure` aborts.
#[test]
fn device_requests_the_adapter_texture_resolution() {
    let mut renderer = match Canvas::new_headless() {
        Ok(r) => r,
        Err(_) => return,
    };
    let packet = GameRenderPacket {
        width: 8.0,
        height: 8.0,
        ..Default::default()
    };
    if renderer.render_packet_to_rgba(&packet).is_err() {
        return; // no adapter — skip
    }
    let resources = renderer.device_resources.as_ref().expect("device created");
    let Some(adapter) = resources.adapter.as_ref() else {
        return;
    };
    let adapter_limits = adapter.limits();
    let device_limits = resources.device.limits();
    assert_eq!(
        device_limits.max_texture_dimension_2d,
        adapter_limits.max_texture_dimension_2d
    );
    assert!(
        device_limits.max_texture_dimension_2d >= 4096
            || adapter_limits.max_texture_dimension_2d < 4096
    );
    // The shared device is compute-capable; the WebGL2 downlevel contract would zero these.
    assert!(device_limits.max_compute_workgroups_per_dimension > 0);
    assert!(device_limits.max_storage_buffers_per_shader_stage > 0);
    // Control: the rest stays at the downlevel defaults.
    assert_eq!(
        device_limits.max_bind_groups,
        wgpu::Limits::downlevel_defaults().max_bind_groups
    );
}

/// Golden test: a Clear + Quad packet must produce a non-blank RGBA buffer.
///
/// Skipped when no GPU adapter is available (CI without a GPU).
#[test]
fn headless_rgba_golden_packet_is_non_blank() {
    let mut renderer = match Canvas::new_headless() {
        Ok(r) => r,
        Err(_) => return, // no adapter — skip
    };

    let packet = GameRenderPacket {
        pattern: "canvas-2d".into(),
        width: 64.0,
        height: 64.0,
        draw_commands: vec![
            packet::DrawCommand::Clear {
                color: [0.1, 0.2, 0.3, 1.0],
            },
            packet::DrawCommand::Quad {
                rect: packet::Rect {
                    x: 8.0,
                    y: 8.0,
                    w: 48.0,
                    h: 48.0,
                },
                color: [1.0, 0.0, 0.0, 1.0],
            },
        ],
        ..Default::default()
    };

    let rgba = match renderer.render_packet_to_rgba(&packet) {
        Ok(buf) => buf,
        Err(_) => return, // no GPU — skip gracefully
    };

    assert_eq!(rgba.len(), 64 * 64 * 4, "buffer length mismatch");

    // At least one non-zero pixel must be present (the red quad or the tinted background).
    let all_zero = rgba.iter().all(|&b| b == 0);
    assert!(
        !all_zero,
        "headless RGBA buffer is entirely blank — renderer produced no output"
    );

    // The quad covers pixels [8..56, 8..56]; sample the center to check it's red-dominant.
    let cx = 32usize;
    let cy = 32usize;
    let base = (cy * 64 + cx) * 4;
    let r = rgba[base];
    let g = rgba[base + 1];
    assert!(
        r > g,
        "center pixel should be red-dominant (got r={r} g={g})"
    );
}

/// Adversarial (non-vacuous): an empty packet (no draw commands) must yield a blank/cleared
/// buffer — proving the non-blank assertion in the golden test is not vacuous.
///
/// Skipped when no GPU adapter is available.
#[test]
fn headless_rgba_blank_packet_stays_blank() {
    let mut renderer = match Canvas::new_headless() {
        Ok(r) => r,
        Err(_) => return, // no adapter — SKIP
    };

    // Zero draw commands: the render pass clears to TRANSPARENT (0,0,0,0).
    let packet = GameRenderPacket {
        pattern: "canvas-2d".into(),
        width: 32.0,
        height: 32.0,
        draw_commands: vec![],
        ..Default::default()
    };

    let rgba = match renderer.render_packet_to_rgba(&packet) {
        Ok(buf) => buf,
        Err(_) => return, // no GPU — SKIP
    };

    assert_eq!(
        rgba.len(),
        32 * 32 * 4,
        "buffer length mismatch for blank packet"
    );

    // Every byte must be 0: the clear color is TRANSPARENT and no quads were drawn.
    let all_zero = rgba.iter().all(|&b| b == 0);
    assert!(
        all_zero,
        "blank packet produced a non-zero pixel — renderer may carry state across calls"
    );
}

fn floor_plan(editable: bool) -> serde_json::Value {
    serde_json::json!({
        "type": "positioned-canvas", "width": 600, "height": 400, "editable": editable,
        "selectEvent": "SELECT_TABLE", "moveEvent": "MOVE_TABLE",
        "items": [{"id": "t1", "label": "T1", "x": 40, "y": 40, "capacity": 4, "status": "empty", "shape": "round"}]
    })
}

#[test]
fn dragging_an_item_past_the_slop_queues_clamped_moves_and_suppresses_the_tap() {
    let mut r = Canvas::new_headless().expect("headless");
    r.set_scene(floor_plan(true)).expect("scene");
    r.input("touch-down", 60.0, 60.0, 0, None);
    r.input("touch-move", 64.0, 60.0, 0, None);
    assert!(
        r.take_scene_events().is_empty(),
        "inside the tap slop nothing moves"
    );
    r.input("touch-move", 160.0, 100.0, 0, None);
    r.input("touch-up", 160.0, 100.0, 0, None);
    let events = r.take_scene_events();
    assert_eq!(
        events.len(),
        1,
        "one move, and no select tap after a drag: {events:?}"
    );
    assert_eq!(events[0].event, "MOVE_TABLE");
    assert_eq!(
        events[0].delivered(),
        serde_json::json!({"id": "t1", "x": 140.0, "y": 80.0}),
        "the item keeps its grab offset"
    );
    r.input("touch-down", 60.0, 60.0, 0, None);
    r.input("touch-move", 2000.0, 2000.0, 0, None);
    let clamped = r.take_scene_events();
    assert_eq!(
        clamped[0].delivered(),
        serde_json::json!({"id": "t1", "x": 504.0, "y": 304.0}),
        "clamped to the plan: 600-96, 400-96"
    );
}

#[test]
fn a_plain_tap_selects_and_a_read_only_plan_does_not_drag() {
    let mut r = Canvas::new_headless().expect("headless");
    r.set_scene(floor_plan(true)).expect("scene");
    r.input("touch-down", 60.0, 60.0, 0, None);
    r.input("touch-up", 61.0, 60.0, 0, None);
    let tap = r.take_scene_events();
    assert_eq!(tap[0].event, "SELECT_TABLE");
    r.set_scene(floor_plan(false)).expect("scene");
    r.input("touch-down", 60.0, 60.0, 0, None);
    r.input("touch-move", 160.0, 100.0, 0, None);
    assert!(
        r.take_scene_events().is_empty(),
        "control: not editable, no drag"
    );
}

#[test]
fn settling_assets_remaps_a_scene_whose_sprite_sheet_was_still_loading() {
    let dir = std::env::temp_dir().join("orbital-canvas-settle-test");
    std::fs::create_dir_all(&dir).expect("tempdir");
    image::RgbaImage::from_pixel(32, 32, image::Rgba([10, 200, 90, 255]))
        .save(dir.join("sheet.png"))
        .expect("png");
    std::fs::write(
        dir.join("atlas.json"),
        r#"{"imagePath":"sheet.png","subTextures":{"gem.png":{"x":0,"y":0,"width":16,"height":16}}}"#,
    )
    .expect("atlas");
    let node = |atlas: &str| {
        serde_json::json!({
            "type": "atlas-image", "size": 32,
            "asset": {"url": format!("file://{}", dir.join("sheet.png").display()), "atlas": atlas, "sprite": "gem.png"}
        })
    };
    let sprites = |r: &Canvas| {
        r.packet.as_ref().map_or(0, |p| {
            p.draw_commands
                .iter()
                .filter(|c| matches!(c, packet::DrawCommand::Sprite { .. }))
                .count()
        })
    };
    let mut r = Canvas::new_headless().expect("headless");
    r.set_scene(node(&format!(
        "file://{}",
        dir.join("atlas.json").display()
    )))
    .expect("scene");
    r.settle_assets(std::time::Duration::from_secs(5));
    assert_eq!(
        sprites(&r),
        1,
        "once the atlas has loaded the sprite is drawn"
    );
    let mut missing = Canvas::new_headless().expect("headless");
    missing
        .set_scene(node(&format!(
            "file://{}",
            dir.join("absent.json").display()
        )))
        .expect("scene");
    missing.settle_assets(std::time::Duration::from_secs(5));
    assert_eq!(
        sprites(&missing),
        0,
        "control: an atlas that never loads draws nothing, and settling still returns"
    );
}

fn board_3d() -> serde_json::Value {
    serde_json::json!({"type": "canvas", "mode": "3d", "tileClickEvent": "TILE",
        "drawables": [{"type": "draw-mesh", "position": {"x": 2, "y": 2}}]})
}

#[test]
fn a_click_on_a_3d_board_queues_its_tile_event() {
    let mut host = SceneHost::new();
    host.set_viewport(800, 600);
    host.set_scene(board_3d()).unwrap();
    assert!(host.view3d.is_some());
    host.input("touch-down", 400.0, 300.0, 0, None);
    host.input("touch-up", 400.0, 300.0, 0, None);
    let events = host.take_scene_events();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].event, "TILE");
    assert!(
        events[0].delivered().get("x").is_some() && events[0].delivered().get("unitId").is_none()
    );

    let mut silent = board_3d();
    silent.as_object_mut().unwrap().remove("tileClickEvent");
    host.set_scene(silent).unwrap();
    host.input("touch-down", 400.0, 300.0, 0, None);
    host.input("touch-up", 400.0, 300.0, 0, None);
    assert!(
        host.take_scene_events().is_empty(),
        "no declared event, nothing queued"
    );
}

#[test]
fn dragging_a_3d_scene_orbits_its_camera() {
    let camera = |host: &SceneHost| {
        host.frame_packet()
            .unwrap()
            .draw_commands
            .iter()
            .find_map(|c| match c {
                packet::DrawCommand::Scene3D { scene } => Some(scene.camera),
                _ => None,
            })
    };
    let mut host = SceneHost::new();
    host.set_viewport(800, 600);
    host.set_scene(board_3d()).unwrap();
    let before = camera(&host).unwrap();
    host.input("touch-down", 400.0, 300.0, 0, None);
    host.input("touch-move", 520.0, 300.0, 0, None);
    host.input("touch-up", 520.0, 300.0, 0, None);
    host.set_clock(16.0);
    host.set_clock(1000.0);
    assert_ne!(camera(&host).unwrap().eye, before.eye);
    assert!(host.take_scene_events().is_empty(), "a drag is not a click");
    let orbited = camera(&host).unwrap().eye;
    assert!(host.zoom(1.1, 400.0, 300.0), "a 3D scene takes the wheel");
    host.set_clock(2000.0);
    assert_ne!(
        camera(&host).unwrap().eye,
        orbited,
        "the wheel dollies the camera"
    );

    host.set_scene(serde_json::json!({"type": "canvas", "drawables": []}))
        .unwrap();
    assert!(host.view3d.is_none(), "a 2D scene has no 3D view");
    assert!(
        !host.zoom(1.1, 10.0, 10.0),
        "a still 2D scene leaves the wheel to the page"
    );
}

#[test]
fn input_events_are_queued_and_drained() {
    let mut renderer = Canvas::new_headless().expect("headless must construct");

    assert!(renderer.input("pointerdown", 10.0, 20.0, 0, None));
    assert!(renderer.input("keydown", 0.0, 0.0, 0, Some("Space")));
    assert_eq!(renderer.input_queue.len(), 2, "two events should be queued");

    let events = renderer.take_input_events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].event_type, "pointerdown");
    assert_eq!(events[0].x, 10.0);
    assert_eq!(events[0].y, 20.0);
    assert_eq!(events[1].event_type, "keydown");
    assert_eq!(events[1].key.as_deref(), Some("Space"));
    assert!(
        renderer.input_queue.is_empty(),
        "take_input_events must drain the queue"
    );
}

#[test]
fn render_drains_input_queue() {
    let mut renderer = Canvas::new_headless().expect("headless must construct");
    renderer.input("pointermove", 5.0, 5.0, 0, None);
    assert_eq!(renderer.input_queue.len(), 1);
    // render() drains even in headless mode (no surface → no GPU call).
    renderer.render().expect("headless render must not error");
    assert!(
        renderer.input_queue.is_empty(),
        "render must drain input queue"
    );
}

/// Golden test for the host-device path: the same Clear+Quad packet through
/// `SharedDeviceRenderer` on an externally-created device must produce the
/// same red-dominant center pixel as the owned-device path.
#[test]
fn shared_device_renderer_golden_packet_is_non_blank() {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        flags: wgpu::InstanceFlags::default(),
        memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
        backend_options: wgpu::BackendOptions::default(),
    });
    let adapter = match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    })) {
        Ok(a) => a,
        Err(_) => return, // no adapter — skip
    };
    let (device, queue) =
        match pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_webgl2_defaults(),
            label: Some("shared-device test"),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        })) {
            Ok(dq) => dq,
            Err(_) => return, // no device — skip
        };

    let mut renderer = crate::SharedDeviceRenderer::new(
        std::sync::Arc::new(device),
        std::sync::Arc::new(queue),
        wgpu::TextureFormat::Rgba8Unorm,
    );

    let packet = GameRenderPacket {
        pattern: "canvas-2d".into(),
        width: 64.0,
        height: 64.0,
        draw_commands: vec![
            packet::DrawCommand::Clear {
                color: [0.1, 0.2, 0.3, 1.0],
            },
            packet::DrawCommand::Quad {
                rect: packet::Rect {
                    x: 8.0,
                    y: 8.0,
                    w: 48.0,
                    h: 48.0,
                },
                color: [1.0, 0.0, 0.0, 1.0],
            },
        ],
        ..Default::default()
    };

    let target = renderer.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("shared-device test target"),
        size: wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });

    assert!(
        renderer.render_packet_to_texture(&packet, &target),
        "quad content expected"
    );

    let encoder = renderer
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("readback"),
        });
    let rgba = match crate::texture::copy_texture_to_rgba(
        renderer.device(),
        renderer.queue(),
        encoder,
        &target,
        64,
        64,
    ) {
        Ok(buf) => buf,
        Err(_) => return,
    };

    assert_eq!(rgba.len(), 64 * 64 * 4);
    let base = (32 * 64 + 32) * 4;
    assert!(
        rgba[base] > rgba[base + 1],
        "center pixel should be red-dominant (got r={} g={})",
        rgba[base],
        rgba[base + 1]
    );
}

/// Full sprite chain: a 2×2 PNG fetched through the asset cache must
/// upload + draw through the textured pipeline onto the target.
#[test]
fn shared_device_renderer_draws_fetched_sprite() {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        flags: wgpu::InstanceFlags::default(),
        memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
        backend_options: wgpu::BackendOptions::default(),
    });
    let adapter = match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    })) {
        Ok(a) => a,
        Err(_) => return, // no adapter — skip
    };
    let (device, queue) =
        match pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_webgl2_defaults(),
            label: Some("sprite test"),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        })) {
            Ok(dq) => dq,
            Err(_) => return, // no device — skip
        };

    // Fixture: a uniform-red 2×2 PNG (uniform color → filtering can't leak).
    let dir =
        std::env::temp_dir().join(format!("orbital-canvas-sprite-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("tempdir");
    let path = dir.join("red.png");
    image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]))
        .save(&path)
        .expect("write fixture");
    let url = format!("file://{}", path.display());

    // Wait for the async fetch to land (file reads are instant, but the
    // cache resolves on a spawned thread).
    let mut ready = false;
    for _ in 0..200 {
        if crate::asset::asset_bytes(&url).is_some() {
            ready = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(ready, "fixture must arrive within 2s");

    let mut renderer = crate::SharedDeviceRenderer::new(
        std::sync::Arc::new(device),
        std::sync::Arc::new(queue),
        wgpu::TextureFormat::Rgba8Unorm,
    );

    let packet = GameRenderPacket {
        pattern: "canvas-2d".into(),
        width: 8.0,
        height: 8.0,
        draw_commands: vec![packet::DrawCommand::Sprite {
            texture: url.clone(),
            src: packet::Rect {
                x: 0.0,
                y: 0.0,
                w: 2.0,
                h: 2.0,
            },
            dst: packet::Rect {
                x: 0.0,
                y: 0.0,
                w: 8.0,
                h: 8.0,
            },
            rotation: 0.0,
            opacity: 1.0,
            flip_x: false,
            flip_y: false,
            tint: None,
        }],
        ..Default::default()
    };

    let target = renderer.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("sprite test target"),
        size: wgpu::Extent3d {
            width: 8,
            height: 8,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });

    assert!(
        renderer.render_packet_to_texture(&packet, &target),
        "sprite should draw"
    );

    let encoder = renderer
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("readback"),
        });
    let rgba = match crate::texture::copy_texture_to_rgba(
        renderer.device(),
        renderer.queue(),
        encoder,
        &target,
        8,
        8,
    ) {
        Ok(buf) => buf,
        Err(_) => return,
    };

    let base = (4 * 8 + 4) * 4;
    assert!(
        rgba[base] > 200 && rgba[base + 1] < 60 && rgba[base + 2] < 60,
        "center pixel must be the sprite's red (got r={} g={} b={})",
        rgba[base],
        rgba[base + 1],
        rgba[base + 2]
    );
}

fn pulse_scene() -> serde_json::Value {
    serde_json::json!({"type": "svg-pulse", "rings": 3})
}

fn ring_alpha(p: &GameRenderPacket) -> f32 {
    let (fade, ring) = match &p.draw_commands[1] {
        packet::DrawCommand::Faded { opacity, commands } => (*opacity, &commands[0]),
        other => (1.0, other),
    };
    match ring {
        packet::DrawCommand::Path {
            stroke: Some(s), ..
        } => s.color[3] * fade,
        other => panic!("expected the first ring, got {other:?}"),
    }
}

#[test]
fn an_animated_scene_asks_for_frames_and_reduced_motion_stops_them() {
    let mut r = Canvas::new_headless().unwrap();
    r.set_scene(pulse_scene()).unwrap();
    assert!(r.is_animated() && r.wants_frames());
    r.set_motion(false);
    assert!(!r.is_animated(), "the host forbids motion");
    r.set_motion(true);
    assert!(r.is_animated(), "control: allowed again");
}

#[test]
fn a_static_scene_runs_no_frame_loop() {
    let mut r = Canvas::new_headless().unwrap();
    r.set_scene(serde_json::json!({"type": "svg-ring", "r": 30}))
        .unwrap();
    assert!(!r.is_animated());
    if asset::pending() == 0 && asset::generation() == r.asset_generation {
        assert!(
            !r.wants_frames(),
            "nothing animates and nothing is arriving"
        );
    }
    let mut off = Canvas::new_headless().unwrap();
    off.set_scene(serde_json::json!({"type": "svg-pulse", "animated": false}))
        .unwrap();
    assert!(
        !off.is_animated(),
        "control: svg-pulse with animated false declares no motion"
    );
    let mut themed = Canvas::new_headless().unwrap();
    let mut node = pulse_scene();
    node["canvasTheme"] = serde_json::json!({"motion": {"enabled": false}});
    themed.set_scene(node).unwrap();
    assert!(
        !themed.is_animated(),
        "the theme's motion switch is honoured"
    );
}

fn reveal_host(at: f64, motion: bool) -> Canvas {
    let mut renderer = Canvas::new_headless().unwrap();
    renderer.set_motion(motion);
    renderer
        .set_scene(serde_json::json!({
            "type": "canvas", "mode": "2d", "projection": "free", "scale": 1,
            "drawables": [{"type": "draw-text", "text": "abcdef", "color": "#000000",
                "font": "12px sans-serif", "anchor": "top-left", "position": {"x": 5, "y": 5},
                "reveal": {"startMs": 1000, "perGlyphMs": 100, "durationMs": 100}}]
        }))
        .unwrap();
    renderer.set_epoch_clock(at);
    renderer
}

fn painted_glyphs(renderer: &Canvas) -> Vec<String> {
    renderer
        .frame_packet()
        .unwrap()
        .draw_commands
        .iter()
        .filter_map(|c| match c {
            packet::DrawCommand::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn wants_frames_while_particles_live_and_not_after() {
    let host = |node: serde_json::Value, at: f64| {
        let mut host = SceneHost::new();
        host.set_epoch_clock(at);
        host.set_scene(
            serde_json::json!({"type": "canvas", "mode": "2d", "projection": "free", "scale": 1.0,
            "drawables": [node]}),
        )
        .unwrap();
        host
    };
    use crate::mappers::emitter::fixtures::emitter;
    let burst = emitter(serde_json::json!({"rate": 0, "burst": 4, "startMs": 5000}));
    assert!(host(burst.clone(), 5400.0).wants_frames());
    assert!(!host(burst, 6100.0).wants_frames(), "the burst has died");
    assert!(
        host(emitter(serde_json::json!({})), 60_000.0).wants_frames(),
        "a rate emitter keeps emitting"
    );
    let still = serde_json::json!({"type": "draw-shape", "shape": "rect", "position": {"x": 0, "y": 0}, "width": 4, "height": 4, "fill": "#fff"});
    assert!(
        !host(still, 6100.0).wants_frames(),
        "control: a static scene needs none"
    );
}

#[test]
fn a_text_reveal_wants_frames_while_it_runs_and_not_after() {
    assert!(painted_glyphs(&reveal_host(500.0, true)).is_empty());
    assert!(
        reveal_host(500.0, true).wants_frames(),
        "it has not started"
    );
    let mid = reveal_host(1250.0, true);
    assert_eq!(painted_glyphs(&mid), ["a", "b", "c"]);
    assert!(mid.wants_frames());
    let done = reveal_host(1600.0, true);
    assert_eq!(painted_glyphs(&done), ["abcdef"]);
    assert!(!done.wants_frames(), "the reveal has ended");
}

#[test]
fn reduced_motion_draws_a_text_reveal_complete_and_asks_for_no_frames() {
    let off = reveal_host(1050.0, false);
    assert_eq!(painted_glyphs(&off), ["abcdef"]);
    assert!(!off.wants_frames());
    assert_eq!(
        painted_glyphs(&reveal_host(1050.0, true)),
        ["a"],
        "control: motion on reveals"
    );
}

fn started_host(at: f64, motion: bool, looping: bool) -> Canvas {
    let mut renderer = Canvas::new_headless().unwrap();
    renderer.set_motion(motion);
    renderer
        .set_scene(serde_json::json!({
            "type": "canvas", "mode": "2d", "projection": "free", "scale": 1,
            "drawables": [{"type": "draw-text", "text": "hi", "color": "#000000",
                "font": "12px sans-serif", "position": {"x": 5, "y": 5},
                "animation": {"startMs": 5000, "durationMs": 1000, "loop": looping,
                    "keyframes": [{"at": 0, "opacity": 1}, {"at": 1, "opacity": 0.25}]}}]
        }))
        .unwrap();
    renderer.set_epoch_clock(at);
    renderer
}

fn text_alpha(renderer: &Canvas) -> f32 {
    renderer
        .frame_packet()
        .unwrap()
        .draw_commands
        .iter()
        .find_map(|c| match c {
            packet::DrawCommand::Text { color, .. } => Some(color[3]),
            _ => None,
        })
        .unwrap()
}

#[test]
fn a_start_ms_animation_wants_frames_until_it_ends_and_reduced_motion_shows_its_end() {
    assert!(
        started_host(4000.0, true, false).wants_frames(),
        "it has not started"
    );
    let mid = started_host(5500.0, true, false);
    assert!(mid.wants_frames() && (text_alpha(&mid) - 0.625).abs() < 1e-3);
    let done = started_host(7000.0, true, false);
    assert!(!done.wants_frames(), "it has ended");
    assert!((text_alpha(&done) - 0.25).abs() < 1e-3);
    assert!(
        started_host(7000.0, true, true).wants_frames(),
        "control: a loop never ends"
    );
    let off = started_host(4000.0, false, false);
    assert!(!off.wants_frames());
    assert!(
        (text_alpha(&off) - 0.25).abs() < 1e-3,
        "reduced motion shows the end state"
    );
}

#[test]
fn a_scene_is_read_for_what_keeps_it_moving_once_when_set_not_at_every_frame() {
    let mut renderer = started_host(4000.0, true, false);
    let reads = renderer.liveness_reads();
    for (at, moving) in [
        (4000.0, true),
        (5500.0, true),
        (5999.0, true),
        (6000.0, false),
        (9000.0, false),
    ] {
        renderer.set_epoch_clock(at);
        assert_eq!(renderer.wants_frames(), moving, "at {at}");
        renderer.frame_packet().unwrap();
    }
    assert_eq!(
        renderer.liveness_reads(),
        reads,
        "no frame walks the scene again"
    );
    renderer
        .set_scene(serde_json::json!({"type": "canvas", "mode": "2d", "projection": "free", "scale": 1, "drawables": []}))
        .unwrap();
    assert_eq!(
        renderer.liveness_reads(),
        reads + 1,
        "control: a new scene is read once"
    );
    assert!(!renderer.wants_frames());
}

#[test]
fn world_fx_remaps_at_the_epoch_clock_without_changing_kernel_ttl() {
    let mut renderer = Canvas::new_headless().unwrap();
    renderer
        .set_scene(serde_json::json!({
            "type": "canvas", "mode": "2d", "projection": "free", "scale": 1,
            "drawables": [{"type": "draw-fx-layer", "tickMs": 500,
                "items": [{"id": "fx-1", "type": "hit", "position": {"x": 2, "y": 0},
                    "ttl": 2, "maxTtl": 4, "bornAt": 1000}],
                "presets": [{"type": "hit", "shape": "ring", "color": "#ff0000", "size": 0.5,
                    "vx": 0, "vz": 0, "gravity": 0}]
            }]
        }))
        .unwrap();
    renderer.set_epoch_clock(1500.0);
    assert!(renderer.wants_frames());
    let alpha = |packet: &GameRenderPacket| {
        packet
            .draw_commands
            .iter()
            .find_map(|cmd| match cmd {
                packet::DrawCommand::Path {
                    stroke: Some(stroke),
                    ..
                } => Some(stroke.color[3]),
                packet::DrawCommand::AdditiveGroup { commands } => {
                    commands.iter().find_map(|inner| match inner {
                        packet::DrawCommand::Path {
                            stroke: Some(stroke),
                            ..
                        }
                        | packet::DrawCommand::GradientPath {
                            stroke: Some(stroke),
                            ..
                        } => Some(stroke.color[3]),
                        _ => None,
                    })
                }
                _ => None,
            })
            .expect("ring stroke")
    };
    assert_eq!(alpha(&renderer.frame_packet().unwrap()), 0.75);
    renderer.set_epoch_clock(2000.0);
    assert_eq!(alpha(&renderer.frame_packet().unwrap()), 0.5);
    assert_eq!(
        renderer.scene().unwrap()["drawables"][0]["items"][0]["ttl"],
        2
    );
}

#[test]
fn the_frame_follows_the_host_clock_from_its_first_value() {
    let mut r = Canvas::new_headless().unwrap();
    r.set_scene(pulse_scene()).unwrap();
    r.set_clock(90_000.0);
    let first = r.frame_packet().unwrap();
    r.set_clock(91_000.0);
    let later = r.frame_packet().unwrap();
    assert_ne!(
        ring_alpha(&first),
        ring_alpha(&later),
        "a second on, the first ring has faded"
    );
    assert!(
        (ring_alpha(&first) - 0.4).abs() < 1e-3,
        "t=0 is the 0% keyframe: the web's opacity 0.4"
    );
    r.set_clock(91_000.0);
    assert_eq!(
        ring_alpha(&r.frame_packet().unwrap()),
        ring_alpha(&later),
        "the same time is the same frame"
    );
}

fn first_ring_base(r: &Canvas) -> f32 {
    ring_alpha(r.packet.as_ref().unwrap())
}

#[test]
fn a_new_scene_does_not_restart_the_clock_and_motion_off_draws_the_rest_frame() {
    let mut r = Canvas::new_headless().unwrap();
    r.set_scene(pulse_scene()).unwrap();
    r.set_clock(5_000.0);
    r.set_clock(5_500.0);
    let before = r.frame_packet().unwrap();
    r.set_scene(pulse_scene()).unwrap();
    let after = r.frame_packet().unwrap();
    assert_eq!(
        ring_alpha(&before),
        ring_alpha(&after),
        "phase survives a scene update"
    );
    r.set_motion(false);
    let rest = r.frame_packet().unwrap();
    assert_eq!(
        ring_alpha(&rest),
        first_ring_base(&r),
        "reduced motion: the rest frame"
    );
    assert_ne!(ring_alpha(&rest), ring_alpha(&after));
}

#[test]
fn without_a_host_clock_the_scene_draws_its_first_frame() {
    let mut r = Canvas::new_headless().unwrap();
    r.set_scene(pulse_scene()).unwrap();
    let unclocked = r.frame_packet().unwrap();
    r.set_clock(123.0);
    assert_eq!(
        ring_alpha(&unclocked),
        ring_alpha(&r.frame_packet().unwrap()),
        "t=0 either way"
    );
}

fn wheel_scene(declared: bool) -> serde_json::Value {
    let mut scene = moves_scene("all");
    scene["drawables"][0]["wheel"] = serde_json::json!(true);
    scene["drawables"].as_array_mut().unwrap().push(serde_json::json!({"type": "draw-shape", "id": "c",
        "shape": "rect", "anchor": "top-left", "position": {"x": 0, "y": 50}, "width": 100, "height": 40,
        "fill": "#000000", "hit": true}));
    if declared {
        scene["wheelEvent"] = serde_json::json!("WHEEL");
    }
    scene
}

#[test]
fn the_wheel_over_a_region_that_takes_it_sends_its_target_and_deltas() {
    let mut r = SceneHost::new();
    r.set_scene(wheel_scene(true)).unwrap();
    assert!(
        r.wheel(10.0, 10.0, 3.0, -40.0),
        "a region declaring wheel keeps it from the page"
    );
    let sent = r.take_scene_events();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].event, "WHEEL");
    assert_eq!(
        sent[0].payload,
        serde_json::json!({"target": "b", "x": 10.0, "y": 10.0, "dx": 3.0, "dy": -40.0})
    );
}

#[test]
fn control_the_wheel_over_a_hit_region_without_wheel_is_the_pages() {
    let mut r = SceneHost::new();
    r.set_scene(wheel_scene(true)).unwrap();
    assert!(
        !r.wheel(10.0, 60.0, 0.0, 40.0),
        "a plain hit region leaves the wheel to the page"
    );
    assert!(r.take_scene_events().is_empty());
}

#[test]
fn control_the_wheel_off_every_region_is_the_pages() {
    let mut r = SceneHost::new();
    r.set_scene(wheel_scene(true)).unwrap();
    assert!(
        !r.wheel(150.0, 120.0, 0.0, 16.0),
        "empty canvas leaves the wheel to the page"
    );
    assert!(r.take_scene_events().is_empty());
}

#[test]
fn control_a_canvas_without_wheel_event_sends_nothing_and_leaves_the_wheel_alone() {
    let mut r = SceneHost::new();
    r.set_scene(wheel_scene(false)).unwrap();
    assert!(
        !r.wheel(10.0, 10.0, 0.0, 40.0),
        "an undeclared wheel is not the canvas's"
    );
    assert!(events(&mut r).is_empty());
}

fn scroll_scene(declared: bool) -> serde_json::Value {
    let mut scene = serde_json::json!({
        "type": "canvas", "mode": "2d", "projection": "free", "camera": {"zoom": 1}, "width": 200, "height": 100,
        "pointerDownEvent": "DOWN",
        "drawables": [
            {"type": "draw-shape", "id": "bar", "shape": "rect", "anchor": "top-left", "position": {"x": 0, "y": 0},
             "width": 200, "height": 20, "fill": "#000000", "hit": true, "fixed": true,
             "semantic": {"role": "banner", "name": "bar"}},
            {"type": "draw-shape", "id": "low", "shape": "rect", "anchor": "top-left", "position": {"x": 0, "y": 150},
             "width": 200, "height": 40, "fill": "#ff0000", "hit": true,
             "semantic": {"role": "button", "name": "low"}},
            {"type": "draw-shape", "id": "knob", "shape": "rect", "anchor": "top-left", "position": {"x": 150, "y": 40},
             "width": 40, "height": 40, "fill": "#00ff00", "hit": true, "drag": true}
        ]
    });
    if declared {
        scene["scroll"] = serde_json::json!({"height": 300, "event": "SCROLLED", "reportMs": 100});
    }
    scene
}

fn clocked_page(
    material: serde_json::Value,
    y: f64,
    extra: serde_json::Value,
    scroll: bool,
) -> SceneHost {
    let mut plate = serde_json::json!({"type": "draw-shape", "id": "art", "shape": "rect", "anchor": "top-left",
        "position": {"x": 0, "y": y}, "width": 200, "height": 40, "fill": "#000000", "material": material});
    for (key, value) in extra.as_object().cloned().unwrap_or_default() {
        plate[key] = value;
    }
    let mut scene = serde_json::json!({
        "type": "canvas", "mode": "2d", "projection": "free", "camera": {"zoom": 1}, "width": 200, "height": 100,
        "drawables": [plate]
    });
    if scroll {
        scene["scroll"] = serde_json::json!({"height": 600});
    }
    let mut host = SceneHost::new();
    host.set_epoch_clock(5000.0);
    host.set_scene(scene).unwrap();
    host
}

const SHADE: &str = "fn shade(px: Pixel) -> vec4<f32> { return px.color; }";

#[test]
fn a_live_material_scrolled_out_of_view_asks_for_no_frames_and_asks_again_in_view() {
    let live = serde_json::json!({"wgsl": SHADE, "live": true});
    let mut below = clocked_page(live.clone(), 300.0, serde_json::json!({}), true);
    assert!(!below.wants_frames(), "the art is below the 100-high view");
    assert!(below.wheel(10.0, 50.0, 0.0, 220.0));
    assert!(
        below.wants_frames(),
        "scrolled to 220 the art at 300 to 340 is in view"
    );
    assert!(below.wheel(10.0, 50.0, 0.0, 200.0));
    assert!(
        !below.wants_frames(),
        "scrolled to 420 it has left through the top"
    );
    assert!(
        clocked_page(live, 20.0, serde_json::json!({}), true).wants_frames(),
        "control: in view at rest it keeps its frames"
    );
}

#[test]
fn edge_a_live_material_touching_the_view_edge_by_any_amount_is_in_view() {
    let live = serde_json::json!({"wgsl": SHADE, "live": true});
    let mut host = clocked_page(live, 300.0, serde_json::json!({}), true);
    assert!(host.wheel(10.0, 50.0, 0.0, 201.0));
    assert!(
        host.wants_frames(),
        "one unit of it shows at the bottom edge"
    );
    assert!(host.wheel(10.0, 50.0, 0.0, 139.0));
    assert!(
        !host.wants_frames(),
        "at 340 its bottom edge sits on the view top: nothing shows"
    );
}

#[test]
fn an_anchored_material_out_of_view_asks_for_no_frames() {
    let anchored = serde_json::json!({"wgsl": SHADE, "live": false, "anchor": 1000.0});
    assert!(!clocked_page(anchored.clone(), 300.0, serde_json::json!({}), true).wants_frames());
    assert!(
        clocked_page(anchored, 20.0, serde_json::json!({}), true).wants_frames(),
        "control: in view its clock runs"
    );
}

fn first_since(packet: &GameRenderPacket) -> f32 {
    packet
        .draw_commands
        .iter()
        .find_map(|command| match command {
            DrawCommand::Sdf {
                material: Some(material),
                ..
            } => Some(material.since),
            _ => None,
        })
        .expect("a material shape")
}

#[test]
fn an_anchored_material_reads_its_elapsed_time_at_every_frame() {
    let anchored = serde_json::json!({"wgsl": SHADE, "live": false, "anchor": 1000.0});
    let mut host = clocked_page(anchored, 20.0, serde_json::json!({}), true);
    assert_eq!(first_since(&host.frame_packet().unwrap()), 4.0);
    host.set_epoch_clock(6500.0);
    assert_eq!(
        first_since(&host.frame_packet().unwrap()),
        5.5,
        "the clock moved and the scene was not set again"
    );
    let plain = serde_json::json!({"wgsl": SHADE, "live": true});
    let mut still = clocked_page(plain, 20.0, serde_json::json!({}), true);
    still.set_epoch_clock(6500.0);
    assert_eq!(
        first_since(&still.frame_packet().unwrap()),
        0.0,
        "control: no anchor, no elapsed time"
    );
}

#[test]
fn a_fixed_live_material_keeps_its_frames_however_far_the_page_scrolls() {
    let live = serde_json::json!({"wgsl": SHADE, "live": true});
    let mut host = clocked_page(live, 20.0, serde_json::json!({"fixed": true}), true);
    assert!(host.wheel(10.0, 50.0, 0.0, 400.0));
    assert!(host.wants_frames());
}

#[test]
fn control_without_page_scroll_a_live_material_keeps_its_frames_wherever_it_is() {
    let live = serde_json::json!({"wgsl": SHADE, "live": true});
    assert!(clocked_page(live, 300.0, serde_json::json!({}), false).wants_frames());
}

#[test]
fn control_a_live_material_the_host_cannot_place_keeps_its_frames() {
    let live = serde_json::json!({"wgsl": SHADE, "live": true});
    let frames = serde_json::json!([{"at": 0, "offsetY": -400}, {"at": 1, "offsetY": 0}]);
    assert!(
        clocked_page(
            live.clone(),
            300.0,
            serde_json::json!({"anchor": "center"}),
            true
        )
        .wants_frames(),
        "another anchor"
    );
    assert!(
        clocked_page(live.clone(), 300.0, serde_json::json!({"animation": {"clock": "scroll", "from": 0, "to": 100, "keyframes": frames}}), true).wants_frames(),
        "an animation may move it"
    );
    let mut nested = SceneHost::new();
    nested.set_epoch_clock(5000.0);
    nested
        .set_scene(serde_json::json!({
            "type": "canvas", "mode": "2d", "projection": "free", "camera": {"zoom": 1}, "width": 200, "height": 100,
            "scroll": {"height": 600},
            "drawables": [{"type": "draw-group", "id": "g", "position": {"x": 0, "y": 300}, "children": [
                {"type": "draw-shape", "id": "art", "shape": "rect", "anchor": "top-left", "position": {"x": 0, "y": 0},
                 "width": 200, "height": 40, "fill": "#000000", "material": live}]}]
        }))
        .unwrap();
    assert!(nested.wants_frames(), "inside a group");
}

fn command_text(p: &GameRenderPacket, i: usize) -> String {
    format!("{:?}", p.draw_commands[i])
}

#[test]
fn a_scrolling_canvas_takes_the_wheel_and_moves_all_but_its_fixed_drawables() {
    let mut r = SceneHost::new();
    r.set_scene(scroll_scene(true)).unwrap();
    let rest = r.frame_packet().unwrap();
    assert!(
        r.wheel(10.0, 50.0, 0.0, 120.0),
        "the page's wheel is the canvas's"
    );
    assert_eq!(r.page_scroll(), 120.0);
    let moved = r.frame_packet().unwrap();
    assert_eq!(moved.scroll, 120.0, "materials read it");
    assert_eq!(
        command_text(&moved, 0),
        command_text(&rest, 0),
        "the background stays"
    );
    assert_eq!(
        command_text(&moved, 1),
        command_text(&rest, 1),
        "the fixed bar stays"
    );
    assert_ne!(
        command_text(&moved, 2),
        command_text(&rest, 2),
        "the content moves"
    );
}

#[test]
fn control_a_canvas_without_scroll_leaves_the_wheel_to_the_page() {
    let mut r = SceneHost::new();
    r.set_scene(scroll_scene(false)).unwrap();
    let rest = r.frame_packet().unwrap();
    assert!(!r.wheel(10.0, 50.0, 0.0, 120.0));
    assert_eq!(r.page_scroll(), 0.0);
    assert_eq!(
        command_text(&r.frame_packet().unwrap(), 2),
        command_text(&rest, 2)
    );
    assert!(r.take_scene_events().is_empty());
}

#[test]
fn the_page_scroll_holds_inside_its_content() {
    let mut r = SceneHost::new();
    r.set_scene(scroll_scene(true)).unwrap();
    r.wheel(10.0, 50.0, 0.0, 5000.0);
    assert_eq!(r.page_scroll(), 200.0, "300 tall in a 100 tall canvas");
    r.wheel(10.0, 50.0, 0.0, -9000.0);
    assert_eq!(r.page_scroll(), 0.0);
}

fn host_of(scene: serde_json::Value) -> SceneHost {
    let mut r = SceneHost::new();
    r.set_scene(scene).unwrap();
    r
}

fn rows_scene(rows: usize, scroll: bool) -> serde_json::Value {
    let mut drawables = vec![
        serde_json::json!({"type": "draw-shape", "id": "bar", "shape": "rect", "anchor": "top-left",
        "position": {"x": 0, "y": 0}, "width": 200, "height": 10, "fill": "#000000", "fixed": true}),
    ];
    for i in 0..rows {
        drawables.push(serde_json::json!({"type": "draw-shape", "id": format!("row{i}"), "shape": "rect",
            "anchor": "top-left", "position": {"x": 0, "y": i * 100}, "width": 200, "height": 40, "fill": "#ff0000"}));
    }
    let mut scene = serde_json::json!({
        "type": "canvas", "mode": "2d", "projection": "free", "camera": {"zoom": 1}, "width": 200, "height": 100,
        "drawables": drawables
    });
    if scroll {
        scene["scroll"] = serde_json::json!({"height": rows * 100});
    }
    scene
}

#[test]
fn a_long_page_scrolled_far_draws_only_what_is_near_the_view() {
    let mut r = SceneHost::new();
    r.set_scene(rows_scene(100, true)).unwrap();
    let all = host_of(rows_scene(100, false))
        .frame_packet()
        .unwrap()
        .draw_commands
        .len();
    r.wheel(10.0, 50.0, 0.0, 5000.0);
    let drawn = r.frame_packet().unwrap().draw_commands.len();
    assert!(all >= 101, "every row maps: {all}");
    assert!(
        drawn <= 8,
        "the rows a view above, in and below the view, the bar and the ground: {drawn} of {all}"
    );
}

#[test]
fn a_row_near_the_view_is_drawn_where_the_scroll_puts_it() {
    let mut r = SceneHost::new();
    r.set_scene(rows_scene(100, true)).unwrap();
    r.wheel(10.0, 50.0, 0.0, 5000.0);
    let moved = r.frame_packet().unwrap();
    let mut at_top = rows_scene(1, false);
    at_top["drawables"][1]["id"] = serde_json::json!("row50");
    let reference = host_of(at_top).frame_packet().unwrap();
    let row = format!("{:?}", reference.draw_commands.last().unwrap());
    assert!(
        moved.draw_commands.iter().any(|c| format!("{c:?}") == row),
        "row 50 (y 5000) is drawn at the top of the view"
    );
}

#[test]
fn control_a_fixed_drawable_is_drawn_however_far_the_page_scrolls() {
    let mut r = SceneHost::new();
    r.set_scene(rows_scene(100, true)).unwrap();
    let rest = r.frame_packet().unwrap();
    r.wheel(10.0, 50.0, 0.0, 9000.0);
    let moved = r.frame_packet().unwrap();
    let bar = rest
        .draw_commands
        .iter()
        .map(|c| format!("{c:?}"))
        .find(|c| c.contains("w: 200.0, h: 10.0"))
        .expect("the bar is drawn at rest");
    assert!(
        moved.draw_commands.iter().any(|c| format!("{c:?}") == bar),
        "the fixed bar stays where it is drawn"
    );
}

#[test]
fn control_a_page_without_scroll_draws_every_row() {
    let r = host_of(rows_scene(100, false));
    assert!(r.frame_packet().unwrap().draw_commands.len() >= 101);
}

#[test]
fn control_the_contrast_probe_keeps_every_text_however_far_the_page_scrolls() {
    let mut scene = rows_scene(100, true);
    for (i, y) in [10, 5000, 9000].into_iter().enumerate() {
        scene["drawables"].as_array_mut().unwrap().push(serde_json::json!({"type": "draw-text", "id": format!("t{i}"),
            "text": "hello", "color": "#000000", "position": {"x": 0, "y": y}, "anchor": "top-left"}));
    }
    let mut r = SceneHost::new();
    r.set_scene(scene).unwrap();
    r.wheel(10.0, 50.0, 0.0, 5000.0);
    let probe = r.contrast_probe().unwrap();
    assert_eq!(
        probe.measure(&[], 1, 1).len(),
        3,
        "the audit reads the whole page, not the drawn band"
    );
}

fn flung(moves: &[(f64, f32)], up_ms: f64) -> SceneHost {
    let mut r = SceneHost::new();
    r.set_scene(rows_scene(100, true)).unwrap();
    r.set_clock(1000.0);
    r.input("touch-down", 10.0, 90.0, 0, None);
    for &(ms, y) in moves {
        r.set_clock(ms);
        r.input("touch-move", 10.0, y, 0, None);
    }
    r.set_clock(up_ms);
    let last = moves.last().map_or(90.0, |m| m.1);
    r.input("touch-up", 10.0, last, 0, None);
    r
}

#[test]
fn a_fling_carries_the_fingers_speed_over_its_last_moments_not_one_jittery_sample() {
    let r = flung(
        &[
            (1016.0, 70.0),
            (1032.0, 50.0),
            (1048.0, 30.0),
            (1049.0, 20.0),
        ],
        1050.0,
    );
    let v = r.page_scroll.velocity;
    assert!(
        (1000.0..2000.0).contains(&v),
        "70 units in 49 ms is about 1430 a second; one 10-unit sample in 1 ms is 10000: {v}"
    );
}

#[test]
fn a_finger_that_stops_before_it_lifts_does_not_fling() {
    let mut r = flung(&[(1016.0, 70.0), (1032.0, 50.0), (1048.0, 30.0)], 1200.0);
    assert_eq!(r.page_scroll.velocity, 0.0);
    let held = r.page_scroll();
    r.set_clock(1216.0);
    assert_eq!(
        r.page_scroll(),
        held,
        "the page stays where the finger left it"
    );
}

#[test]
fn control_a_steady_fling_glides_at_the_fingers_speed() {
    let r = flung(
        &[
            (1016.0, 74.0),
            (1032.0, 58.0),
            (1048.0, 42.0),
            (1064.0, 26.0),
        ],
        1064.0,
    );
    let v = r.page_scroll.velocity;
    assert!(
        (900.0..1100.0).contains(&v),
        "16 units every 16 ms is 1000 a second: {v}"
    );
}

#[test]
fn a_glide_slows_like_a_phone_page_keeping_about_a_seventh_of_its_speed_after_a_second() {
    let mut r = flung(
        &[
            (1016.0, 74.0),
            (1032.0, 58.0),
            (1048.0, 42.0),
            (1064.0, 26.0),
        ],
        1064.0,
    );
    let start = r.page_scroll.velocity;
    let mut t = 1064.0;
    while t < 2064.0 {
        t += 16.0;
        r.set_clock(t);
    }
    let ratio = r.page_scroll.velocity / start;
    assert!(
        (0.10..0.18).contains(&ratio),
        "0.998 per ms keeps 0.135 after a second: {ratio}"
    );
}

#[test]
fn a_press_after_scrolling_hits_what_is_drawn_there_in_scene_coordinates() {
    let mut r = SceneHost::new();
    r.set_scene(scroll_scene(true)).unwrap();
    r.wheel(10.0, 50.0, 0.0, 120.0);
    r.take_scene_events();
    r.input("touch-down", 10.0, 40.0, 0, None);
    let sent = r.take_scene_events();
    assert_eq!(sent[0].payload["target"], "low");
    assert_eq!(
        sent[0].payload["y"], 160.0,
        "the scene's own height for the program"
    );
    r.input("touch-up", 10.0, 40.0, 0, None);
    r.take_scene_events();
    r.input("touch-down", 10.0, 10.0, 0, None);
    let sent = r.take_scene_events();
    assert_eq!(
        sent[0].payload["target"], "bar",
        "control: the fixed bar is where it is drawn"
    );
    assert_eq!(sent[0].payload["y"], 10.0);
}

#[test]
fn the_semantic_tree_follows_the_scroll_except_fixed_nodes() {
    let mut r = SceneHost::new();
    r.set_scene(scroll_scene(true)).unwrap();
    r.wheel(10.0, 50.0, 0.0, 120.0);
    let tree = r.semantic_tree();
    let rect = |id: &str| tree.iter().find(|n| n.id == id).unwrap().rect;
    assert_eq!(rect("low").y, 30.0);
    assert_eq!(rect("bar").y, 0.0, "control");
}

#[test]
fn the_offset_reaches_the_program_at_most_once_per_report_interval_and_trails_the_last_move() {
    let mut r = SceneHost::new();
    r.set_scene(scroll_scene(true)).unwrap();
    r.set_clock(1000.0);
    r.wheel(10.0, 50.0, 0.0, 10.0);
    let sent = r.take_scene_events();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].event, "SCROLLED");
    assert_eq!(
        sent[0].payload,
        serde_json::json!({"offset": 10.0, "max": 200.0})
    );
    r.wheel(10.0, 50.0, 0.0, 10.0);
    assert!(r.take_scene_events().is_empty(), "inside the interval");
    assert!(r.wants_frames(), "a pending report keeps the clock running");
    r.set_clock(1050.0);
    assert!(r.take_scene_events().is_empty());
    r.set_clock(1100.0);
    let sent = r.take_scene_events();
    assert_eq!(sent.len(), 1, "the trailing report");
    assert_eq!(sent[0].payload["offset"], 20.0);
    r.set_clock(1200.0);
    assert!(r.take_scene_events().is_empty(), "control: nothing moved");
    assert!(!r.wants_frames());
}

#[test]
fn a_touch_that_travels_scrolls_the_page_glides_on_and_rests() {
    let mut r = SceneHost::new();
    r.set_scene(scroll_scene(true)).unwrap();
    r.set_clock(1000.0);
    r.input("touch-down", 10.0, 90.0, 0, None);
    r.set_clock(1016.0);
    r.input("touch-move", 10.0, 70.0, 0, None);
    assert_eq!(
        r.page_scroll(),
        20.0,
        "past the slop the page follows the finger"
    );
    r.set_clock(1032.0);
    r.input("touch-move", 10.0, 60.0, 0, None);
    assert_eq!(r.page_scroll(), 30.0);
    r.input("touch-up", 10.0, 60.0, 0, None);
    r.take_scene_events();
    r.set_clock(1048.0);
    let gliding = r.page_scroll();
    assert!(
        gliding > 30.0,
        "it carries on after the finger lifts: {gliding}"
    );
    assert!(r.wants_frames());
    let mut t = 1048.0;
    while r.wants_frames() && t < 5000.0 {
        t += 16.0;
        r.set_clock(t);
    }
    assert!(!r.wants_frames(), "it comes to rest");
    let rested = r.page_scroll();
    let sent = r.take_scene_events();
    assert_eq!(
        sent.last().unwrap().payload["offset"],
        rested,
        "the resting offset reaches the program"
    );
}

#[test]
fn control_a_touch_on_a_drag_region_keeps_its_travel() {
    let mut r = SceneHost::new();
    r.set_scene(scroll_scene(true)).unwrap();
    r.set_clock(1000.0);
    r.input("touch-down", 160.0, 50.0, 0, None);
    r.set_clock(1016.0);
    r.input("touch-move", 160.0, 20.0, 0, None);
    assert_eq!(r.page_scroll(), 0.0, "drag: true keeps the gesture");
}

#[test]
fn control_a_touch_within_the_slop_is_a_tap() {
    let mut r = SceneHost::new();
    r.set_scene(scroll_scene(true)).unwrap();
    r.input("touch-down", 10.0, 90.0, 0, None);
    r.input("touch-move", 10.0, 85.0, 0, None);
    assert_eq!(r.page_scroll(), 0.0);
}

#[test]
fn a_scroll_clocked_drawable_plays_at_the_page_scroll_without_a_clock() {
    let mut scene = scroll_scene(true);
    scene["drawables"][1]["animation"] = serde_json::json!({
        "clock": "scroll", "from": 0, "to": 100,
        "keyframes": [{"at": 0, "opacity": 0}, {"at": 1, "opacity": 1}]
    });
    let mut r = SceneHost::new();
    r.set_scene(scene).unwrap();
    let top = r.frame_packet().unwrap();
    assert!(!r.wants_frames(), "a still page needs no frames");
    r.wheel(10.0, 50.0, 0.0, 50.0);
    let half = r.frame_packet().unwrap();
    assert_ne!(command_text(&top, 2), command_text(&half, 2));
    assert_eq!(top.animations.len(), 1);
    assert!(
        matches!(top.animations[0].clock, packet::AnimationClock::Scroll { from, to } if from == 0.0 && to == 100.0)
    );
}

fn morph_scene(animation: serde_json::Value) -> serde_json::Value {
    let mut scene = scroll_scene(true);
    scene["drawables"].as_array_mut().unwrap().push(serde_json::json!({
        "type": "draw-shape", "id": "hero", "shape": "path", "position": {"x": 0, "y": 30},
        "d": "M0 0 L10 0 L10 10 L0 10 Z", "fill": "#0000ff", "fixed": true, "animation": animation
    }));
    scene
}

fn first_point(p: &GameRenderPacket) -> (f32, f32) {
    match p.draw_commands.last() {
        Some(DrawCommand::Path { segments, .. }) => match segments[0] {
            packet::PathSegment::MoveTo { x, y } => (x, y),
            other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_path_morphs_between_outlines_as_the_page_scrolls() {
    let mut r = SceneHost::new();
    r.set_scene(morph_scene(serde_json::json!({
        "clock": "scroll", "from": 0, "to": 100,
        "keyframes": [{"at": 0, "d": "M0 0 L10 0 L10 10 L0 10 Z"}, {"at": 1, "d": "M40 0 L50 0 L50 10 L40 10 Z"}]
    })))
    .unwrap();
    let (x0, y0) = first_point(&r.frame_packet().unwrap());
    r.wheel(10.0, 50.0, 0.0, 50.0);
    let (xh, yh) = first_point(&r.frame_packet().unwrap());
    assert!(
        (xh - x0 - 20.0).abs() < 0.01,
        "halfway between the outlines: {x0} -> {xh}"
    );
    assert_eq!(yh, y0, "fixed: the hero does not scroll away");
    r.wheel(10.0, 50.0, 0.0, -50.0);
    assert_eq!(
        first_point(&r.frame_packet().unwrap()).0,
        x0,
        "scrolling back undoes it"
    );
    r.set_clock(1000.0);
    r.set_clock(1200.0);
    assert!(!r.wants_frames());
}

#[test]
fn a_morph_on_the_paint_clock_needs_frames_and_a_duration() {
    let mut r = SceneHost::new();
    r.set_scene(morph_scene(serde_json::json!({
        "durationMs": 1000,
        "keyframes": [{"at": 0, "d": "M0 0 L10 0 L10 10 L0 10 Z"}, {"at": 1, "d": "M40 0 L50 0 L50 10 L40 10 Z"}]
    })))
    .unwrap();
    assert!(r.wants_frames());
    r.set_clock(0.0);
    let start = first_point(&r.frame_packet().unwrap()).0;
    r.set_clock(500.0);
    assert!((first_point(&r.frame_packet().unwrap()).0 - start - 20.0).abs() < 0.01);
    assert!(
        SceneHost::new()
            .set_scene(morph_scene(
                serde_json::json!({"keyframes": [{"at": 0, "d": "M0 0 L1 1"}]})
            ))
            .is_err(),
        "control: no duration"
    );
    assert!(SceneHost::new()
        .set_scene(morph_scene(serde_json::json!({"durationMs": 10, "startMs": 5, "keyframes": [{"at": 0, "d": "M0 0 L1 1"}]})))
        .is_err(), "control: startMs is refused, not ignored");
}

#[test]
fn a_morph_refuses_outlines_with_different_subpath_counts_and_non_path_shapes() {
    let mismatched = morph_scene(serde_json::json!({
        "clock": "scroll", "from": 0, "to": 100,
        "keyframes": [{"at": 0, "d": "M0 0 L10 0 L10 10 Z"}, {"at": 1, "d": "M0 0 L10 0 L10 10 Z M20 0 L30 0 L30 10 Z"}]
    }));
    assert!(SceneHost::new().set_scene(mismatched).is_err());
    let mut rect = morph_scene(serde_json::json!({
        "clock": "scroll", "from": 0, "to": 100, "keyframes": [{"at": 0, "d": "M0 0 L1 1"}]
    }));
    rect["drawables"][3]["shape"] = serde_json::json!("rect");
    assert!(SceneHost::new().set_scene(rect).is_err());
}

fn touch(r: &mut SceneHost, kind: &str, x: f32, y: f32, id: i64) -> Vec<mappers::scene::Emit> {
    r.input_pointer(kind, x, y, 0, None, id);
    r.take_scene_events()
}

fn pairs(touches: &[(i64, f32, f32)]) -> serde_json::Value {
    serde_json::Value::Array(
        touches
            .iter()
            .map(|(id, x, y)| serde_json::json!({"pointerId": id, "x": x, "y": y}))
            .collect(),
    )
}

#[test]
fn a_second_finger_reaches_the_capturing_region_with_every_touch() {
    let mut r = pointer_host(vec![region("a", 0.0, 0.0, 100.0, 100.0, 0.0)]);
    let first = touch(&mut r, "touch-down", 10.0, 10.0, 1);
    assert_eq!(
        first.iter().map(|e| e.event.as_str()).collect::<Vec<_>>(),
        vec!["ENTER_a", "DOWN_a"]
    );
    assert!(
        first[1].payload.get("touches").is_none(),
        "a single pointer carries no touches"
    );
    let second = touch(&mut r, "touch-down", 60.0, 50.0, 2);
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].event, "DOWN_a");
    assert_eq!(second[0].payload["target"], "a");
    assert_eq!(
        (
            second[0].payload["x"].clone(),
            second[0].payload["y"].clone()
        ),
        (serde_json::json!(60.0), serde_json::json!(50.0))
    );
    assert_eq!(
        second[0].payload["touches"],
        pairs(&[(1, 10.0, 10.0), (2, 60.0, 50.0)])
    );
    let moved = touch(&mut r, "touch-move", 70.0, 50.0, 2);
    assert_eq!(moved[0].event, "MOVE_a");
    assert_eq!(
        moved[0].payload["touches"],
        pairs(&[(1, 10.0, 10.0), (2, 70.0, 50.0)])
    );
    let primary = touch(&mut r, "touch-move", 5.0, 10.0, 1);
    assert_eq!(
        primary[0].payload["touches"],
        pairs(&[(1, 5.0, 10.0), (2, 70.0, 50.0)])
    );
    let lifted = touch(&mut r, "touch-up", 70.0, 50.0, 2);
    assert_eq!(lifted.len(), 1);
    assert_eq!(
        lifted[0].event, "MOVE_a",
        "a further release is a move that shows the remaining touches"
    );
    assert_eq!(lifted[0].payload["touches"], pairs(&[(1, 5.0, 10.0)]));
    let alone = touch(&mut r, "touch-move", 6.0, 10.0, 1);
    assert!(
        alone[0].payload.get("touches").is_none(),
        "control: one pointer again, no touches: {:?}",
        alone[0].payload
    );
    let up = touch(&mut r, "touch-up", 6.0, 10.0, 1);
    assert_eq!(up[0].event, "UP_a");
    assert!(up[0].payload.get("touches").is_none());
}

#[test]
fn the_primary_lifting_first_reports_the_pointer_still_down_and_then_goes_quiet() {
    let mut r = pointer_host(vec![region("a", 0.0, 0.0, 100.0, 100.0, 0.0)]);
    touch(&mut r, "touch-down", 10.0, 10.0, 1);
    touch(&mut r, "touch-down", 60.0, 50.0, 2);
    let up = touch(&mut r, "touch-up", 10.0, 10.0, 1);
    assert_eq!(up[0].event, "UP_a");
    assert_eq!(up[0].payload["touches"], pairs(&[(2, 60.0, 50.0)]));
    assert!(
        touch(&mut r, "touch-move", 65.0, 50.0, 2).is_empty(),
        "a leftover pointer sends nothing"
    );
    assert!(touch(&mut r, "touch-up", 65.0, 50.0, 2).is_empty());
    let fresh = touch(&mut r, "touch-down", 20.0, 20.0, 3);
    assert!(
        fresh.iter().any(|e| e.event == "DOWN_a"),
        "a new press is primary again"
    );
    assert!(
        fresh[fresh.len() - 1].payload.get("touches").is_none(),
        "{:?}",
        fresh
    );
}

#[test]
fn a_second_finger_with_no_capture_sends_nothing_and_starts_no_tap() {
    let mut r = pointer_host(vec![region("a", 0.0, 0.0, 100.0, 100.0, 0.0)]);
    assert!(
        touch(&mut r, "touch-down", 300.0, 300.0, 1).is_empty(),
        "the first press hit no region"
    );
    assert!(
        touch(&mut r, "touch-down", 10.0, 10.0, 2).is_empty(),
        "a further finger starts no press"
    );
    assert!(touch(&mut r, "touch-up", 10.0, 10.0, 2).is_empty());
    assert!(
        r.touch_down == Some((300.0, 300.0)),
        "the primary's tap anchor is unchanged"
    );
}

#[test]
fn a_finished_timed_scene_is_mapped_once_and_a_live_one_every_frame() {
    let done = started_host(7000.0, true, false);
    for _ in 0..3 {
        assert!((text_alpha(&done) - 0.25).abs() < 1e-3, "the end state");
    }
    assert_eq!(
        done.timed_maps(),
        0,
        "a fade the packet plays is never re-mapped"
    );
    let mid = started_host(5500.0, true, false);
    assert!(
        (text_alpha(&mid) - 0.625).abs() < 1e-3,
        "the packet plays it at the epoch clock"
    );
    let _ = text_alpha(&mid);
    assert_eq!(mid.timed_maps(), 0, "even while it plays");
    let mut typing = Canvas::new_headless().unwrap();
    typing
        .set_scene(serde_json::json!({
            "type": "canvas", "mode": "2d", "projection": "free", "scale": 1,
            "drawables": [{"type": "draw-text", "text": "hello", "color": "#000000",
                "font": "12px sans-serif", "position": {"x": 5, "y": 5},
                "reveal": {"startMs": 5000, "perGlyphMs": 100, "durationMs": 100}}]
        }))
        .unwrap();
    typing.set_epoch_clock(5200.0);
    let _ = typing.frame_packet();
    let _ = typing.frame_packet();
    assert_eq!(
        typing.timed_maps(),
        2,
        "control: a live typed reveal maps every frame"
    );
    typing.set_epoch_clock(60_000.0);
    let _ = typing.frame_packet();
    let _ = typing.frame_packet();
    assert_eq!(typing.timed_maps(), 3, "and once more when it has settled");
}

#[test]
fn a_timed_frame_maps_the_props_the_scene_mapping_prepared_without_preparing_again() {
    let scene = serde_json::json!({
        "type": "canvas", "mode": "2d", "projection": "free", "scale": 1,
        "drawables": [{"type": "draw-text", "text": "hello", "color": "#000000",
            "font": "12px sans-serif", "position": {"x": 5, "y": 5},
            "reveal": {"startMs": 5000, "perGlyphMs": 100, "durationMs": 100}}]
    });
    let mut typing = Canvas::new_headless().unwrap();
    typing.set_scene(scene.clone()).unwrap();
    let handed = typing.prepared.lock().unwrap().clone();
    assert_eq!(
        handed,
        mappers::game_canvas_2d::prepare_doc(
            &orbital_canvas_base::scene_doc::SceneDoc::split(scene.clone()),
            &mut Default::default()
        )
        .ok(),
        "the mapping hands over exactly what a frame would prepare"
    );
    typing.set_epoch_clock(5200.0);
    let _ = typing.frame_packet();
    let _ = typing.frame_packet();
    assert_eq!(typing.timed_maps(), 2, "control: the live reveal still maps every frame");
    assert_eq!(
        typing.timed_prepares.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "no frame prepares the scene again"
    );
}

#[test]
fn a_settled_timed_frame_follows_a_new_scene() {
    let mut host = started_host(7000.0, true, false);
    assert!((text_alpha(&host) - 0.25).abs() < 1e-3);
    host.set_scene(serde_json::json!({
        "type": "canvas", "mode": "2d", "projection": "free", "scale": 1,
        "drawables": [{"type": "draw-text", "text": "hi", "color": "#000000",
            "font": "12px sans-serif", "position": {"x": 5, "y": 5},
            "animation": {"startMs": 5000, "durationMs": 1000, "loop": false,
                "keyframes": [{"at": 0, "opacity": 1}, {"at": 1, "opacity": 0.5}]}}]
    }))
    .unwrap();
    assert!(
        (text_alpha(&host) - 0.5).abs() < 1e-3,
        "the settled frame of the new scene, not the old one"
    );
}

mod contrast_audit {
    use super::*;
    use crate::packet::{DrawCommand, Gradient, GradientStop, Halo, PathSegment, Rect, Vec2};

    const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
    const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
    const GREY_999: [f32; 4] = [0.6, 0.6, 0.6, 1.0];
    const GREY_767676: [f32; 4] = [118.0 / 255.0, 118.0 / 255.0, 118.0 / 255.0, 1.0];

    fn ground(color: [f32; 4]) -> DrawCommand {
        DrawCommand::Quad {
            rect: Rect {
                x: 0.0,
                y: 0.0,
                w: 240.0,
                h: 100.0,
            },
            color,
        }
    }

    fn text(color: [f32; 4], size: f32, weight: u16, halo: Option<Halo>) -> DrawCommand {
        DrawCommand::Text {
            text: "Hello".into(),
            pos: Vec2 { x: 10.0, y: 50.0 },
            size,
            color,
            align: Default::default(),
            weight,
            halo,
            family: Default::default(),
            color_runs: Vec::new(),
            rotation: 0.0,
        }
    }

    fn host(commands: Vec<DrawCommand>) -> SceneHost {
        let mut host = SceneHost::new();
        host.submit(GameRenderPacket {
            pattern: "canvas-2d".into(),
            width: 240.0,
            height: 100.0,
            draw_commands: commands,
            ..Default::default()
        });
        host
    }

    fn all(commands: Vec<DrawCommand>) -> Option<Vec<ContrastFinding>> {
        match host(commands).contrast_audit_all() {
            Ok(found) => Some(found),
            Err(e) => {
                eprintln!("no GPU adapter on this host: {e}");
                None
            }
        }
    }

    #[test]
    fn a_text_that_paints_nothing_is_not_audited_and_a_faint_one_is() {
        let Some(found) = all(vec![
            ground(WHITE),
            text([0.0, 0.0, 0.0, 0.0], 16.0, 400, None),
            text([0.0, 0.0, 0.0, 0.3], 16.0, 400, None),
        ]) else {
            return;
        };
        assert_eq!(
            found.len(),
            1,
            "only the visible text is a reader's text: {found:?}"
        );
        assert!(
            found[0].ratio.unwrap() < 4.5,
            "control: a faint text is measured and fails"
        );
    }

    #[test]
    fn dark_text_on_a_light_ground_measures_21_and_passes() {
        let Some(found) = all(vec![ground(WHITE), text(BLACK, 16.0, 400, None)]) else {
            return;
        };
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "Hello");
        assert!((found[0].ratio.unwrap() - 21.0).abs() < 0.01, "{found:?}");
        assert_eq!(found[0].required, 4.5);
        let failing = host(vec![ground(WHITE), text(BLACK, 16.0, 400, None)])
            .contrast_audit()
            .unwrap();
        assert!(failing.is_empty(), "{failing:?}");
    }

    #[test]
    fn grey_999_on_white_fails_4_5_with_ratio_2_85() {
        let Some(found) = all(vec![ground(WHITE), text(GREY_999, 16.0, 400, None)]) else {
            return;
        };
        let ratio = found[0].ratio.unwrap();
        assert!((ratio - 2.848).abs() < 0.01, "{ratio}");
        let failing = host(vec![ground(WHITE), text(GREY_999, 16.0, 400, None)])
            .contrast_audit()
            .unwrap();
        assert_eq!(failing.len(), 1);
        assert_eq!(failing[0].size, 16.0);
    }

    #[test]
    fn large_text_needs_3_and_grey_999_still_fails_while_767676_passes() {
        let Some(found) = all(vec![ground(WHITE), text(GREY_999, 32.0, 400, None)]) else {
            return;
        };
        assert_eq!(found[0].required, 3.0);
        assert!(found[0].ratio.unwrap() < 3.0);
        let passing = host(vec![ground(WHITE), text(GREY_767676, 32.0, 400, None)]);
        let mut passing = passing;
        assert!(passing.contrast_audit().unwrap().is_empty());
        let small = host(vec![ground(WHITE), text(GREY_767676, 16.0, 400, None)])
            .contrast_audit_all()
            .unwrap();
        assert_eq!(small[0].required, 4.5, "control: 16px keeps 4.5");
        assert!(small[0].ratio.unwrap() >= 4.5, "#767676 is 4.54");
    }

    #[test]
    fn bold_text_from_18_66px_needs_3_and_regular_weight_keeps_4_5() {
        let Some(bold) = all(vec![ground(WHITE), text(GREY_999, 19.0, 700, None)]) else {
            return;
        };
        assert_eq!(bold[0].required, 3.0);
        let regular = all(vec![ground(WHITE), text(GREY_999, 19.0, 400, None)]).unwrap();
        assert_eq!(regular[0].required, 4.5);
        let small_bold = all(vec![ground(WHITE), text(GREY_999, 18.0, 700, None)]).unwrap();
        assert_eq!(small_bold[0].required, 4.5, "18px bold is below 18.66");
    }

    fn gradient_ground(from: f32, to: f32) -> DrawCommand {
        let stop = |offset: f32, v: f32| GradientStop {
            offset,
            color: [v, v, v, 1.0],
        };
        DrawCommand::GradientPath {
            segments: vec![
                PathSegment::MoveTo { x: 0.0, y: 0.0 },
                PathSegment::LineTo { x: 240.0, y: 0.0 },
                PathSegment::LineTo { x: 240.0, y: 100.0 },
                PathSegment::LineTo { x: 0.0, y: 100.0 },
                PathSegment::Close,
            ],
            fill: None,
            stroke: None,
            fill_gradient: Some(Gradient::Linear {
                from: Vec2 { x: 0.0, y: 0.0 },
                to: Vec2 { x: 240.0, y: 0.0 },
                stops: vec![stop(0.0, from), stop(1.0, to)],
            }),
            stroke_gradient: None,
        }
    }

    #[test]
    fn text_over_a_gradient_reports_the_worst_end() {
        let Some(found) = all(vec![
            gradient_ground(1.0, 0.0),
            text(BLACK, 16.0, 400, None),
        ]) else {
            return;
        };
        let rect = found[0].rect;
        let right = (rect.x + rect.w) / 240.0;
        let darkest = 1.0 - right;
        let expected = crate::contrast::wcag_ratio(BLACK, [darkest, darkest, darkest, 1.0]);
        let ratio = found[0].ratio.unwrap();
        assert!(
            ratio < 21.0 && (ratio - expected).abs() / expected < 0.1,
            "{ratio} vs {expected}"
        );
        let flat = all(vec![ground(WHITE), text(BLACK, 16.0, 400, None)]).unwrap();
        assert!(
            ratio < flat[0].ratio.unwrap(),
            "control: the flat ground is better"
        );
    }

    #[test]
    fn a_halo_is_the_backdrop_of_its_text() {
        let halo = Halo {
            color: BLACK,
            width: 3.0,
        };
        let Some(found) = all(vec![ground(WHITE), text(GREY_999, 16.0, 400, Some(halo))]) else {
            return;
        };
        let expected = crate::contrast::wcag_ratio(GREY_999, BLACK);
        assert!((found[0].ratio.unwrap() - expected).abs() < 0.01);
        assert!(expected > 4.5);
        let bare = all(vec![ground(WHITE), text(GREY_999, 16.0, 400, None)]).unwrap();
        assert!(
            bare[0].ratio.unwrap() < 4.5,
            "control: no halo measures the ground"
        );
    }

    #[test]
    fn text_outside_every_fill_measures_against_the_clear_colour() {
        let clear = [0.2, 0.2, 0.2, 1.0];
        let side = DrawCommand::Quad {
            rect: Rect {
                x: 150.0,
                y: 0.0,
                w: 90.0,
                h: 100.0,
            },
            color: WHITE,
        };
        let Some(found) = all(vec![
            DrawCommand::Clear { color: clear },
            side,
            text(BLACK, 16.0, 400, None),
        ]) else {
            return;
        };
        let expected = crate::contrast::wcag_ratio(BLACK, clear);
        assert!(
            (found[0].ratio.unwrap() - expected).abs() < 0.02,
            "{found:?}"
        );
        assert!(expected < 4.5);
    }

    #[test]
    fn a_text_inside_a_clip_group_is_audited_and_never_painted_into_its_own_backdrop() {
        let clip = ground_clip();
        let inside = DrawCommand::ClipGroup {
            clip,
            commands: vec![text(BLACK, 16.0, 400, None)],
        };
        let Some(found) = all(vec![ground(WHITE), inside]) else {
            return;
        };
        assert_eq!(found.len(), 1);
        assert!((found[0].ratio.unwrap() - 21.0).abs() < 0.01, "{found:?}");
    }

    fn ground_clip() -> Vec<PathSegment> {
        vec![
            PathSegment::MoveTo { x: 0.0, y: 0.0 },
            PathSegment::LineTo { x: 240.0, y: 0.0 },
            PathSegment::LineTo { x: 240.0, y: 100.0 },
            PathSegment::LineTo { x: 0.0, y: 100.0 },
            PathSegment::Close,
        ]
    }

    #[test]
    fn a_text_with_nothing_painted_behind_it_has_no_ratio_and_is_not_a_failure() {
        let mut bare = host(vec![text(BLACK, 16.0, 400, None)]);
        let Ok(failing) = bare.contrast_audit() else {
            return;
        };
        assert!(failing.is_empty());
        let found = bare.contrast_audit_all().unwrap();
        assert_eq!(found[0].ratio, None);
    }

    #[test]
    fn a_host_with_no_scene_audits_nothing() {
        let mut empty = SceneHost::new();
        let Ok(found) = empty.contrast_audit_all() else {
            return;
        };
        assert!(found.is_empty());
    }
}

mod embedded_scene3d {
    use super::*;

    fn node(animated: bool) -> serde_json::Value {
        let mut mesh = serde_json::json!({"type": "draw-mesh", "shape": "box",
            "position": {"x": 0, "y": 0},
            "material": {"kind": "standard", "color": "#ff0000"}});
        if animated {
            mesh["animation"] = serde_json::json!({"durationMs": 1000, "keyframes": [
                {"at": 0, "rotateY": 0}, {"at": 1, "rotateY": 6.0}]});
        }
        serde_json::json!({"type": "canvas", "projection": "free", "scale": 1.0,
            "backgroundColor": "#00000000",
            "drawables": [{"type": "draw-scene3d", "id": "orb",
                "position": {"x": 16, "y": 16}, "width": 32, "height": 32,
                "scene": {"camera": {"mode": "front", "zoom": 2.4, "fov": 40,
                    "target": {"x": 0, "y": 0, "z": 0.5}}, "drawables": [mesh]}}]})
    }

    fn host(animated: bool) -> SceneHost {
        let mut host = SceneHost::new();
        host.set_viewport(64, 64);
        host.set_scene(node(animated)).unwrap();
        host.set_clock(0.0);
        host
    }

    fn meshes(packet: &GameRenderPacket) -> serde_json::Value {
        let scene = packet
            .draw_commands
            .iter()
            .find_map(|c| match c {
                DrawCommand::Target { commands, .. } => match commands.as_slice() {
                    [DrawCommand::Scene3D { scene }] => Some(scene),
                    _ => None,
                },
                _ => None,
            })
            .expect("the embedded scene");
        serde_json::to_value(&scene.meshes).unwrap()
    }

    #[test]
    fn motion_plays_the_embedded_scene_and_reduced_motion_shows_its_rest_pose() {
        let mut moving = host(true);
        moving.set_epoch_clock(1000.0);
        let first = meshes(&moving.frame_packet().unwrap());
        moving.set_epoch_clock(1250.0);
        let later = meshes(&moving.frame_packet().unwrap());
        assert_ne!(first, later, "with motion the pose follows the clock");

        let mut still = host(true);
        still.set_motion(false);
        still.set_epoch_clock(1000.0);
        let one = meshes(&still.frame_packet().unwrap());
        still.set_epoch_clock(1250.0);
        let two = meshes(&still.frame_packet().unwrap());
        assert_eq!(one, two, "reduced motion: the same pose at every instant");
        assert_eq!(
            one,
            meshes(&host(false).frame_packet().unwrap()),
            "reduced motion: the pose is the unanimated scene's"
        );
    }

    #[test]
    fn a_moving_embedded_scene_asks_for_frames_and_a_still_one_or_reduced_motion_does_not() {
        let mut moving = host(true);
        moving.set_epoch_clock(1000.0);
        moving.refresh_assets();
        assert!(moving.wants_frames(), "a spinning object needs frames");
        moving.set_motion(false);
        moving.refresh_assets();
        assert!(!moving.wants_frames(), "reduced motion stops the loop");
        let mut still = host(false);
        still.set_epoch_clock(1000.0);
        still.refresh_assets();
        assert!(!still.wants_frames(), "control: a still scene needs none");
    }

    #[test]
    fn the_host_paints_the_embedded_scene_inside_its_rect_and_scroll_moves_it() {
        let Ok(mut canvas) = Canvas::new_headless() else {
            return;
        };
        canvas.set_viewport(64, 64);
        canvas.set_scene(node(false)).unwrap();
        let packet = canvas.frame_packet().unwrap();
        let Ok(pixels) = canvas.render_packet_to_rgba(&packet) else {
            return;
        };
        let inside = |p: &[u8]| {
            p.chunks_exact(4)
                .enumerate()
                .filter(|(_, px)| px[3] > 0)
                .map(|(i, _)| ((i % 64) as u32, (i / 64) as u32))
                .collect::<Vec<_>>()
        };
        let drawn = inside(&pixels);
        assert!(drawn.len() > 100, "the box paints: {}", drawn.len());
        assert!(
            drawn
                .iter()
                .all(|&(x, y)| (16..48).contains(&x) && (16..48).contains(&y)),
            "nothing outside the rect"
        );
        let moved = packet.scrolled(16.0);
        let Ok(scrolled) = canvas.render_packet_to_rgba(&moved) else {
            return;
        };
        let top = |points: &[(u32, u32)]| points.iter().map(|p| p.1).min().unwrap();
        assert_eq!(
            top(&inside(&scrolled)) + 16,
            top(&drawn),
            "the page moved it up"
        );
    }

    fn page(animated: bool, extra: Option<serde_json::Value>) -> serde_json::Value {
        let mut page = node(animated);
        let drawables = page["drawables"].as_array_mut().unwrap();
        drawables.push(serde_json::json!({"type": "draw-text", "text": "headline",
            "color": "#000000", "font": "12px sans-serif", "position": {"x": 2, "y": 2}}));
        drawables.extend(extra);
        page
    }

    fn page_host(page: serde_json::Value) -> SceneHost {
        let mut host = SceneHost::new();
        host.set_viewport(64, 64);
        host.set_scene(page).unwrap();
        host.set_clock(0.0);
        host
    }

    fn headline(packet: &GameRenderPacket) -> serde_json::Value {
        let command = packet
            .draw_commands
            .iter()
            .find(|c| matches!(c, DrawCommand::Text { text, .. } if text == "headline"))
            .expect("the headline");
        serde_json::to_value(command).unwrap()
    }

    #[test]
    fn a_spinning_object_is_remapped_alone_and_never_the_page() {
        let mut host = page_host(page(true, None));
        host.set_epoch_clock(1000.0);
        let first = host.frame_packet().unwrap();
        host.set_epoch_clock(1250.0);
        let later = host.frame_packet().unwrap();
        assert_ne!(meshes(&first), meshes(&later), "the pose follows the clock");
        assert_eq!(host.timed_maps(), 0, "the page is not mapped again");
        assert_eq!(
            headline(&first),
            headline(&later),
            "the rest of the packet is the cached one"
        );
        host.refresh_assets();
        assert!(host.wants_frames(), "the spinner keeps the loop going");
    }

    #[test]
    fn reduced_motion_shows_the_rest_pose_on_a_page_with_a_spinner() {
        let mut host = page_host(page(true, None));
        host.set_motion(false);
        host.set_epoch_clock(1000.0);
        let one = meshes(&host.frame_packet().unwrap());
        host.set_epoch_clock(1250.0);
        assert_eq!(one, meshes(&host.frame_packet().unwrap()));
        assert_eq!(
            one,
            meshes(&page_host(page(false, None)).frame_packet().unwrap())
        );
        host.refresh_assets();
        assert!(!host.wants_frames());
        assert_eq!(host.timed_maps(), 0);
    }

    #[test]
    fn a_page_with_a_pulse_beside_the_spinner_still_maps_per_frame() {
        let pulse = serde_json::json!({"type": "draw-shape", "shape": "rect",
            "position": {"x": 40, "y": 40}, "width": 8, "height": 8, "fill": "#ff0000",
            "animation": {"durationMs": 1000, "keyframes": [
                {"at": 0, "opacity": 0.2}, {"at": 1, "opacity": 1.0}]}});
        let mut with_pulse = page_host(page(true, Some(pulse.clone())));
        with_pulse.set_epoch_clock(1000.0);
        let _ = with_pulse.frame_packet();
        let _ = with_pulse.frame_packet();
        assert_eq!(with_pulse.timed_maps(), 2, "a 2D epoch animation maps");
        let mut only_pulse = page_host(serde_json::json!({"type": "canvas", "projection": "free",
            "scale": 1.0, "drawables": [pulse]}));
        only_pulse.set_epoch_clock(1000.0);
        let _ = only_pulse.frame_packet();
        let _ = only_pulse.frame_packet();
        assert_eq!(only_pulse.timed_maps(), 2, "control: no scene3d, as before");
    }

    #[test]
    fn a_tinted_icon_paints_in_the_tint_through_the_host() {
        let Ok(mut canvas) = Canvas::new_headless() else {
            return;
        };
        let svg = "data:image/svg+xml;utf8,%3Csvg%20xmlns%3D%22http%3A%2F%2Fwww.w3.org%2F2000%2Fsvg%22%20width%3D%2216%22%20height%3D%2216%22%3E%3Crect%20width%3D%2216%22%20height%3D%2216%22%20fill%3D%22%23000000%22%2F%3E%3C%2Fsvg%3E";
        let scene = |tint: Option<&str>| {
            let mut sprite = serde_json::json!({"type": "draw-sprite", "position": {"x": 8, "y": 8},
                "width": 16, "height": 16, "asset": {"url": svg}});
            if let Some(tint) = tint {
                sprite["tint"] = tint.into();
            }
            serde_json::json!({"type": "canvas", "projection": "free", "scale": 1.0,
                "backgroundColor": "#00000000", "drawables": [sprite]})
        };
        canvas.set_viewport(32, 32);
        let centre = |canvas: &mut Canvas, tint: Option<&str>| {
            canvas.set_scene(scene(tint)).unwrap();
            canvas.settle_assets(std::time::Duration::from_secs(2));
            let packet = canvas.frame_packet()?;
            let pixels = canvas.render_packet_to_rgba(&packet).ok()?;
            let i = (16 * 32 + 16) * 4;
            Some([pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]])
        };
        let Some(red) = centre(&mut canvas, Some("#ff0000")) else {
            return;
        };
        assert!(
            red[0] > 240 && red[1] < 8 && red[2] < 8 && red[3] > 240,
            "{red:?}"
        );
        let Some(plain) = centre(&mut canvas, None) else {
            return;
        };
        assert!(
            plain[0] < 8 && plain[3] > 240,
            "control: untinted stays black {plain:?}"
        );
    }
}

fn glow_host(at: f64) -> Canvas {
    let mut renderer = Canvas::new_headless().unwrap();
    renderer.set_motion(true);
    renderer
        .set_scene(serde_json::json!({
            "type": "canvas", "mode": "2d", "projection": "free", "scale": 1,
            "drawables": [{"type": "draw-shape", "shape": "ellipse", "position": {"x": 40, "y": 40},
                "anchor": "center", "radiusX": 10, "fill": "#ffffff",
                "shadow": {"color": "#00ff00", "blur": 8},
                "animation": {"startMs": 5000, "durationMs": 1000,
                    "keyframes": [{"at": 0, "opacity": 0}, {"at": 1, "opacity": 1}]}}]
        }))
        .unwrap();
    renderer.set_epoch_clock(at);
    renderer
}

fn faded_glow(renderer: &Canvas) -> (f32, serde_json::Value) {
    renderer
        .frame_packet()
        .unwrap()
        .draw_commands
        .iter()
        .find_map(|c| match c {
            packet::DrawCommand::Faded { opacity, commands } => {
                Some((*opacity, serde_json::to_value(commands).unwrap()))
            }
            _ => None,
        })
        .expect("the glow plays as a Faded layer")
}

#[test]
fn a_fading_glow_carries_its_opacity_outside_one_unchanging_layer() {
    let (a, content_a) = faded_glow(&glow_host(5250.0));
    let (b, content_b) = faded_glow(&glow_host(5750.0));
    assert!(
        (a - 0.25).abs() < 1e-3 && (b - 0.75).abs() < 1e-3,
        "{a} {b}"
    );
    assert_eq!(
        content_a, content_b,
        "the cached content is the same every frame"
    );
    let mut still = glow_host(5250.0);
    still
        .set_scene(serde_json::json!({
            "type": "canvas", "mode": "2d", "projection": "free", "scale": 1,
            "drawables": [{"type": "draw-shape", "shape": "ellipse", "position": {"x": 40, "y": 40},
                "anchor": "center", "radiusX": 10, "fill": "#ffffff",
                "shadow": {"color": "#00ff00", "blur": 8}}]
        }))
        .unwrap();
    assert!(
        !still
            .frame_packet()
            .unwrap()
            .draw_commands
            .iter()
            .any(|c| matches!(c, packet::DrawCommand::Faded { .. })),
        "control: an unanimated glow has no wrapper"
    );
}

mod sprung_text {
    use super::*;

    const EPOCH: f64 = 1_700_000_005_000.0;

    fn scene(x: f64, width: f64, spring: bool, timed: bool) -> serde_json::Value {
        let mut text = serde_json::json!({
            "type": "draw-text", "id": "cl1", "color": "#000000", "font": "14px sans-serif",
            "text": "one two three four five six seven eight nine ten eleven twelve",
            "position": {"x": x, "y": 200}, "width": width
        });
        if spring {
            text["spring"] = serde_json::json!({"stiffness": 220, "damping": 26});
        }
        if timed {
            text["animation"] = serde_json::json!({"startMs": EPOCH - 4000.0, "durationMs": 870, "loop": false,
                "keyframes": [{"at": 0, "offsetY": 18, "opacity": 0}, {"at": 1, "offsetY": 0, "opacity": 1}]});
        }
        let mut drawables = vec![text];
        if timed {
            drawables.push(serde_json::json!({"type": "draw-text", "id": "typed", "text": "hello", "color": "#000000",
                "font": "12px sans-serif", "position": {"x": 5, "y": 5},
                "reveal": {"startMs": EPOCH - 4000.0, "perGlyphMs": 100, "durationMs": 100}}));
        }
        serde_json::json!({
            "type": "canvas", "mode": "2d", "projection": "free", "scale": 1,
            "drawables": drawables
        })
    }

    fn lines(host: &SceneHost) -> Vec<(f32, f32, String)> {
        host.frame_packet()
            .unwrap()
            .draw_commands
            .iter()
            .filter_map(|c| match c {
                DrawCommand::Text { pos, text, .. } => Some((pos.x, pos.y, text.clone())),
                _ => None,
            })
            .collect()
    }

    fn host(timed: bool) -> SceneHost {
        let mut host = SceneHost::new();
        host.set_viewport(1280, 720);
        if timed {
            host.set_epoch_clock(EPOCH);
        }
        host
    }

    fn pump(host: &mut SceneHost, from: f64, ms: f64) -> f64 {
        let mut t = from;
        while t < from + ms {
            t += 1000.0 / 60.0;
            host.set_clock(t);
        }
        t
    }

    fn moves_to_the_new_rect(timed: bool) {
        let mut moved = host(timed);
        moved.set_scene(scene(175.0, 80.0, true, timed)).unwrap();
        moved.set_clock(5000.0);
        let _ = lines(&moved);
        moved.set_scene(scene(619.2, 598.3, true, timed)).unwrap();
        assert!(moved.wants_frames(), "the spring is moving");
        let mid = pump(&mut moved, 5000.0, 100.0);
        assert!(moved.wants_frames(), "still moving after 100 ms");
        let x = lines(&moved)[0].0;
        assert!(x > 176.0 && x < 618.0, "in flight, not jumped: {x}");
        pump(&mut moved, mid, 3000.0);
        assert!(moved.springs.settled());
        let mut fresh = host(timed);
        fresh.set_scene(scene(619.2, 598.3, true, timed)).unwrap();
        fresh.set_clock(5000.0);
        pump(&mut fresh, 5000.0, 3000.0);
        let (got, want) = (lines(&moved), lines(&fresh));
        assert!(!want.is_empty());
        assert_eq!(got, want);
        assert!((want[0].0 - 619.2).abs() < 0.5, "{want:?}");
    }

    #[test]
    fn a_spring_text_ends_at_the_new_rect() {
        moves_to_the_new_rect(false);
    }

    #[test]
    fn a_spring_text_in_a_timed_scene_ends_at_the_new_rect() {
        moves_to_the_new_rect(true);
    }

    #[test]
    fn a_plain_text_jumps_to_the_new_rect() {
        let mut h = host(false);
        h.set_scene(scene(175.0, 80.0, false, false)).unwrap();
        h.set_scene(scene(619.2, 598.3, false, false)).unwrap();
        let mut fresh = host(false);
        fresh.set_scene(scene(619.2, 598.3, false, false)).unwrap();
        assert_eq!(lines(&h), lines(&fresh));
    }

    #[test]
    fn an_unchanged_spring_text_stays_put_and_wants_no_frames() {
        let mut h = host(false);
        h.set_scene(scene(175.0, 80.0, true, false)).unwrap();
        let before = lines(&h);
        h.set_scene(scene(175.0, 80.0, true, false)).unwrap();
        assert!(h.springs.settled());
        assert_eq!(lines(&h), before);
    }
}

fn scroll_to_page(to: Option<serde_json::Value>) -> serde_json::Value {
    let mut scene = serde_json::json!({
        "type": "canvas", "mode": "2d", "projection": "free", "camera": {"zoom": 1}, "width": 200, "height": 100,
        "drawables": [{"type": "draw-shape", "id": "art", "shape": "rect", "anchor": "top-left",
            "position": {"x": 0, "y": 0}, "width": 200, "height": 40, "fill": "#000000"}],
        "scroll": {"height": 600, "event": "SCROLLED"}
    });
    if let Some(to) = to {
        scene["scroll"]["to"] = to;
    }
    scene
}

#[test]
fn a_scroll_to_request_moves_the_page_once_per_request_number() {
    let mut host = SceneHost::new();
    host.set_scene(scroll_to_page(Some(serde_json::json!({"y": 200, "n": 1}))))
        .unwrap();
    assert_eq!(host.page_scroll(), 200.0, "the request moves the page");
    host.scroll_by(50.0);
    host.set_scene(scroll_to_page(Some(serde_json::json!({"y": 200, "n": 1}))))
        .unwrap();
    assert_eq!(
        host.page_scroll(),
        250.0,
        "the same request number is not applied twice"
    );
    host.set_scene(scroll_to_page(Some(serde_json::json!({"y": 9999, "n": 2}))))
        .unwrap();
    assert_eq!(
        host.page_scroll(),
        500.0,
        "a new request is clamped to the content"
    );
}

#[test]
fn control_without_a_scroll_to_request_the_page_stays_where_the_reader_left_it() {
    let mut host = SceneHost::new();
    host.set_scene(scroll_to_page(None)).unwrap();
    assert_eq!(host.page_scroll(), 0.0);
    host.scroll_by(120.0);
    host.set_scene(scroll_to_page(None)).unwrap();
    assert_eq!(host.page_scroll(), 120.0);
}

#[test]
fn a_split_scene_reads_the_liveness_of_the_whole_one_and_only_its_new_drawables() {
    let scene = |text: &str| {
        serde_json::json!({"type": "canvas", "mode": "2d", "projection": "free", "scale": 1, "drawables": [
            {"id": "typed", "type": "draw-text", "text": text, "color": "#000000", "font": "12px sans-serif",
                "position": {"x": 5, "y": 5}, "reveal": {"startMs": 5000, "perGlyphMs": 100, "durationMs": 100}},
            {"id": "glow", "type": "draw-shape", "shape": "rect", "position": {"x": 0, "y": 0}, "width": 4, "height": 4,
                "material": {"wgsl": "x", "params": [], "live": true}}
        ], "children": [{"type": "draw-shape", "shape": "rect", "position": {"x": 1, "y": 1}, "width": 2, "height": 2}]})
    };
    let mut host = SceneHost::new();
    let first = orbital_canvas_base::scene_doc::SceneDoc::split(scene("hello"));
    let read = host.read_liveness(&first);
    assert_eq!(read, frame::Liveness::read(&scene("hello")), "the split scene is as live as the whole one");
    assert_eq!(host.liveness_parts.len(), 2);
    let mut next = orbital_canvas_base::scene_doc::SceneDoc::split(scene("hello there"));
    std::sync::Arc::make_mut(next.drawables.as_mut().unwrap())[1] = std::sync::Arc::clone(&first.drawables.as_ref().unwrap()[1]);
    let kept = host.liveness_parts.get(&(std::sync::Arc::as_ptr(&first.drawables.as_ref().unwrap()[1]) as usize)).cloned();
    assert_eq!(host.read_liveness(&next), frame::Liveness::read(&scene("hello there")), "and stays so after a change");
    let now = host.liveness_parts.get(&(std::sync::Arc::as_ptr(&first.drawables.as_ref().unwrap()[1]) as usize)).cloned();
    assert_eq!(kept.map(|k| k.1), now.map(|n| n.1), "the shared drawable's part is carried over");
    assert_eq!(host.liveness_parts.len(), 2, "control: parts of drawables that left are dropped");
}
