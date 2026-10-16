//! The three patterns that draw the AVL state machine: `state-machine-view` (a trait's
//! `TraitLevelData`), `jazari-state-machine` (an orbital schema or one trait, gear nodes) and
//! `state-graph` (a player-built machine from state names and transitions).

use super::draw::{frame, Ink, Pen};
use super::state_machine::{
    prepare, Diagram, NodeShape, Opts, StateInfo, TraitData, TransitionInfo,
};
use super::{props_f32, props_str};
use crate::mappers::game_canvas_2d::CanvasSceneError;
use crate::mappers::learning::canvas::theme_for;
use crate::mappers::learning::theme::DiagramTheme;
use crate::mappers::learning::Xf;
use crate::mappers::scene::MappedScene;
use crate::packet::TextAlign;
use serde_json::Value;

const MESSAGE_W: f32 = 320.0;
const MESSAGE_H: f32 = 96.0;
const MAX_SCALE: f32 = 1.25;

fn message_scene(
    pattern: &str,
    props: &Value,
    text: &str,
    tone: &str,
    viewport: Option<(f32, f32)>,
) -> MappedScene {
    let theme = theme_for(props);
    let (xf, pw, ph) = frame(MESSAGE_W, MESSAGE_H, viewport);
    let mut pen = Pen::new(&theme, xf);
    pen.clear(theme.ground);
    let color = pen.tone(tone);
    pen.text_middle(
        MESSAGE_W / 2.0,
        MESSAGE_H / 2.0,
        text,
        14.0,
        color,
        TextAlign::Center,
        400,
    );
    pen.finish(pattern, pw, ph)
}

fn error_text(props: &Value) -> Option<String> {
    match props.get("error")? {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        Value::Object(o) => Some(
            o.get("message")
                .and_then(Value::as_str)
                .unwrap_or("[object Object]")
                .to_string(),
        ),
        other => Some(other.to_string()),
    }
}

fn status_scene(pattern: &str, props: &Value, viewport: Option<(f32, f32)>) -> Option<MappedScene> {
    if props
        .get("isLoading")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Some(message_scene(
            pattern,
            props,
            "Loading state machine\u{2026}",
            "label",
            viewport,
        ));
    }
    error_text(props).map(|e| message_scene(pattern, props, &e, "error", viewport))
}

fn paint_diagram(
    pattern: &str,
    theme: &DiagramTheme,
    d: &Diagram,
    viewport: Option<(f32, f32)>,
) -> MappedScene {
    let (xf, pw, ph) = frame(d.width, d.height, viewport);
    let mut pen = Pen::new(theme, xf);
    pen.clear(theme.ground);
    d.paint(&mut pen);
    pen.finish(pattern, pw, ph)
}

pub fn state_machine_view(
    props: &Value,
    viewport: Option<(f32, f32)>,
) -> Result<MappedScene, CanvasSceneError> {
    if let Some(s) = status_scene("state-machine-view", props, viewport) {
        return Ok(s);
    }
    let Some(trait_data) = props.get("trait").and_then(TraitData::from_level) else {
        return Ok(message_scene(
            "state-machine-view",
            props,
            "No state machine to visualize",
            "label",
            viewport,
        ));
    };
    let theme = theme_for(props);
    let opts = Opts::from_props(props);
    let d = prepare(&trait_data, &opts, 0.0);
    Ok(paint_diagram("state-machine-view", &theme, &d, viewport))
}

fn first_entity(schema: &Value) -> Option<(String, Vec<String>)> {
    let entity = schema.get("orbitals")?.as_array()?.first()?.get("entity")?;
    let obj = entity.as_object()?;
    let name = obj.get("name")?.as_str()?.to_string();
    let fields = obj
        .get("fields")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|f| {
                    f.get("name")
                        .and_then(Value::as_str)
                        .filter(|n| !n.is_empty())
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default();
    Some((name, fields))
}

fn extract_trait<'a>(
    props: &'a Value,
    schema: Option<&'a Value>,
    index: usize,
) -> Option<&'a Value> {
    if let Some(t) = props.get("trait").filter(|t| t.is_object()) {
        return Some(t);
    }
    for orbital in schema?.get("orbitals")?.as_array()? {
        let traits = orbital
            .get("traits")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        if index < traits.len() {
            let r = &traits[index];
            return (r.is_object() && r.get("name").is_some() && r.get("ref").is_none())
                .then_some(r);
        }
    }
    None
}

pub fn jazari(
    props: &Value,
    viewport: Option<(f32, f32)>,
) -> Result<MappedScene, CanvasSceneError> {
    if let Some(s) = status_scene("jazari-state-machine", props, viewport) {
        return Ok(s);
    }
    let schema = props.get("schema").filter(|s| s.is_object());
    let entity = schema.and_then(first_entity);
    let index = props_f32(props, "traitIndex", 0.0).max(0.0) as usize;
    let trait_data = extract_trait(props, schema, index)
        .and_then(|t| TraitData::from_trait(t, entity.as_ref().map_or("", |(n, _)| n.as_str())))
        .filter(|t| !t.states.is_empty());
    let Some(trait_data) = trait_data else {
        return Ok(message_scene(
            "jazari-state-machine",
            props,
            "No state machine to visualize",
            "label",
            viewport,
        ));
    };
    let theme = theme_for(props);
    let mut opts = Opts::from_props(props);
    opts.node_shape = NodeShape::Gear;
    opts.show_header = true;
    if props
        .get("entityFields")
        .and_then(Value::as_array)
        .is_none()
    {
        opts.entity_fields = entity.map(|(_, f)| f).unwrap_or_default();
    }
    let d = prepare(&trait_data, &opts, 0.0);
    Ok(paint_diagram("jazari-state-machine", &theme, &d, viewport))
}

fn graph_trait(props: &Value) -> TraitData {
    let initial = props_str(props, "initialState");
    let states = props
        .get("states")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(|n| StateInfo {
                    name: n.to_string(),
                    is_initial: Some(n) == initial,
                    is_terminal: false,
                })
                .collect()
        })
        .unwrap_or_default();
    let transitions = props
        .get("transitions")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .enumerate()
                .map(|(i, t)| TransitionInfo {
                    from: props_str(t, "from").unwrap_or_default().to_string(),
                    to: props_str(t, "to").unwrap_or_default().to_string(),
                    event: props_str(t, "event").unwrap_or_default().to_string(),
                    guard: props_str(t, "guardHint").map(|g| {
                        orbital_core_base::schema::format_sexpr(
                            &orbital_core_base::schema::SExpression::String(g.to_string()),
                        )
                    }),
                    effects: Vec::new(),
                    index: i,
                })
                .collect()
        })
        .unwrap_or_default();
    TraitData {
        name: "StateGraph".into(),
        states,
        transitions,
        ..TraitData::default()
    }
}

pub fn state_graph(
    props: &Value,
    viewport: Option<(f32, f32)>,
) -> Result<MappedScene, CanvasSceneError> {
    let theme = theme_for(props);
    let (w, h) = (
        props_f32(props, "width", 500.0).max(1.0),
        props_f32(props, "height", 400.0).max(1.0),
    );
    let (outer, pw, ph) = frame(w, h, viewport);
    let mut pen = Pen::new(&theme, outer);
    pen.clear(theme.ground);
    let (ground, border) = (pen.ground(), pen.tone("grid"));
    pen.rrect(
        0.5,
        0.5,
        w - 1.0,
        h - 1.0,
        theme.corner,
        Some(ground),
        Some(&Ink::new(border, 1.0)),
    );

    let trait_data = graph_trait(props);
    let mut opts = Opts::from_props(props);
    opts.show_header = false;
    opts.active_state = props_str(props, "currentState")
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    opts.pending_source = props_str(props, "addingFrom")
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    opts.state_click = props_str(props, "nodeClickEvent")
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let pad = 1.0 + 8.0;
    let d = prepare(&trait_data, &opts, 0.0);
    if !trait_data.states.is_empty() {
        let (aw, ah) = (w - 2.0 * pad, h - 2.0 * pad);
        let scale = (aw / d.width).min(ah / d.height).min(MAX_SCALE);
        let (ox, oy) = (pad, pad);
        pen.xf = Xf {
            s: outer.s * scale,
            ox: outer.ox + outer.s * ox,
            oy: outer.oy + outer.s * oy,
        };
        d.paint(&mut pen);
    }
    Ok(pen.finish("state-graph", pw, ph))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::{DrawCommand, Easing, PathSegment};
    use serde_json::json;

    /// `std-ml-similarity` trait `SimilarityRun` (3 states, 8 transitions, 2 guards) as
    /// `traitLevelFromTrait` yields it, with the machine sitting in `embedding`.
    fn machine() -> Value {
        serde_json::from_str(include_str!(
            "fixtures/state-machine-view-similarity-run.json"
        ))
        .unwrap()
    }

    /// The same trait as the raw `Trait` the jazari pattern takes, inside its orbital schema.
    fn jazari_props() -> Value {
        let f: Value =
            serde_json::from_str(include_str!("fixtures/jazari-similarity-run.json")).unwrap();
        json!({
            "type": "jazari-state-machine",
            "schema": {"orbitals": [{"entity": {"name": "SimilarityComparison", "fields": [{"name": "query"}, {"name": "status"}]}, "traits": [f["trait"].clone()]}]}
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
    fn the_machine_draws_every_state_name_every_event_and_the_guard_text() {
        let scene = state_machine_view(&machine(), None).unwrap();
        let t = texts(&scene);
        for name in [
            "idle",
            "embedding",
            "resolved",
            "COMPARE",
            "EMBEDDED",
            "SimilarityRun",
        ] {
            assert!(t.iter().any(|x| x == name), "{name} missing from {t:?}");
        }
        assert!(
            t.iter()
                .any(|x| x == "(> (array/len @payload.candidates) 0)"),
            "a guard prints as .lolo text"
        );
        assert_eq!(
            t.iter().filter(|x| *x == "now").count(),
            1,
            "one active state, one badge"
        );
    }

    #[test]
    fn taps_report_the_state_and_the_transition_with_the_web_payloads() {
        let scene = state_machine_view(&machine(), None).unwrap();
        let state_hits: Vec<_> = scene
            .hits
            .iter()
            .filter(|h| h.emits[0].event == "STATE_CLICK")
            .collect();
        assert_eq!(state_hits.len(), 3);
        let embedding = state_hits
            .iter()
            .find(|h| h.emits[0].payload["stateId"] == "embedding")
            .unwrap();
        let (x, y) = (
            embedding.rect.x + embedding.rect.w / 2.0,
            embedding.rect.y + embedding.rect.h / 2.0,
        );
        assert_eq!(
            scene.emits_at(x, y)[0].delivered(),
            json!({"stateId": "embedding"}),
            "an event-ref payload is not wrapped"
        );
        let transitions = scene
            .hits
            .iter()
            .filter(|h| h.emits[0].event == "TRANSITION_CLICK")
            .count();
        assert_eq!(transitions, 8);
        let mut plain = machine();
        plain.as_object_mut().unwrap().remove("stateClickEvent");
        plain
            .as_object_mut()
            .unwrap()
            .remove("transitionClickEvent");
        assert!(
            state_machine_view(&plain, None).unwrap().hits.is_empty(),
            "control: no authored event, no regions"
        );
        let mut callback = machine();
        callback["onTransitionClick"] = json!("PICK_TRANSITION");
        let both = state_machine_view(&callback, None).unwrap();
        let hit = both.hits.iter().find(|h| h.emits.len() == 2).unwrap();
        assert_eq!(
            hit.emits[0].arg, "transition",
            "the callback form wraps under its argument name"
        );
    }

    #[test]
    fn the_fired_wire_marches_and_the_active_ring_breathes_on_the_web_timings() {
        let scene = state_machine_view(&machine(), None).unwrap();
        let anims = &scene.packet.animations;
        assert_eq!(anims.len(), 2, "one fired wire, one active ring");
        let march = anims
            .iter()
            .find(|a| a.duration_ms == 800.0)
            .expect("dash march");
        assert_eq!(march.keyframes[1].dash_offset, Some(-14.0));
        let breathe = anims
            .iter()
            .find(|a| a.duration_ms == 1600.0)
            .expect("ring pulse");
        let swing: Vec<f32> = breathe
            .keyframes
            .iter()
            .filter_map(|k| k.opacity)
            .map(|o| o * 0.22)
            .collect();
        assert!(
            (swing[0] - 0.28).abs() < 1e-4
                && (swing[1] - 0.06).abs() < 1e-4
                && (swing[2] - 0.28).abs() < 1e-4
        );
        assert!(anims
            .iter()
            .all(|a| a.easing == Easing::Linear && a.iterations.is_none()));
        let dash_at = |t: f64| -> f32 {
            let p = scene.packet.resolved(t, true).into_owned();
            p.draw_commands[march.first..march.end]
                .iter()
                .find_map(|c| match c {
                    DrawCommand::Path {
                        stroke: Some(s), ..
                    } => Some(s.dash_offset),
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(dash_at(0.0), 0.0);
        assert!(dash_at(400.0) < dash_at(0.0), "the dashes advance");
        let still = scene.packet.resolved(400.0, false).into_owned();
        assert!(
            still.draw_commands[march.first..march.end].iter().all(
                |c| !matches!(c, DrawCommand::Path { stroke: Some(s), .. } if s.dash_offset != 0.0)
            ),
            "reduced motion: no march"
        );
    }

    #[test]
    fn a_quiet_machine_and_a_motion_free_theme_declare_no_motion() {
        let mut quiet = machine();
        quiet.as_object_mut().unwrap().remove("activeTransition");
        quiet.as_object_mut().unwrap().remove("activeState");
        assert!(state_machine_view(&quiet, None)
            .unwrap()
            .packet
            .animations
            .is_empty());
        let mut off = machine();
        off["canvasTheme"] = json!({"motion": {"enabled": false}});
        assert!(state_machine_view(&off, None)
            .unwrap()
            .packet
            .animations
            .is_empty());
        let mut only_state = machine();
        only_state
            .as_object_mut()
            .unwrap()
            .remove("activeTransition");
        assert_eq!(
            state_machine_view(&only_state, None)
                .unwrap()
                .packet
                .animations
                .len(),
            1
        );
    }

    #[test]
    fn the_fired_transition_and_the_active_state_change_the_paint() {
        let paint = |m: &Value| -> usize {
            state_machine_view(m, None)
                .unwrap()
                .packet
                .draw_commands
                .iter()
                .filter(
                    |c| matches!(c, DrawCommand::Path { stroke: Some(s), .. } if s.width >= 2.5),
                )
                .count()
        };
        let fired = machine();
        let mut quiet = machine();
        quiet.as_object_mut().unwrap().remove("activeTransition");
        assert!(
            paint(&fired) > paint(&quiet),
            "the fired wire is drawn at active weight"
        );
        let mut idle = machine();
        idle.as_object_mut().unwrap().remove("activeState");
        let t = texts(&state_machine_view(&idle, None).unwrap());
        assert!(
            !t.iter().any(|x| x == "now"),
            "control: no active state, no badge"
        );
    }

    #[test]
    fn jazari_draws_gear_nodes_where_the_view_draws_pills() {
        let gear = jazari(&jazari_props(), None).unwrap();
        let toothed = |s: &MappedScene| {
            s.packet.draw_commands.iter().any(|c| matches!(c, DrawCommand::Path { segments, fill: Some(_), .. } if segments.len() == 49))
        };
        assert!(toothed(&gear), "12 teeth x 4 points, plus close");
        assert!(!toothed(&state_machine_view(&machine(), None).unwrap()));
        let t = texts(&gear);
        assert!(
            t.iter().any(|x| x.starts_with("on SimilarityComparison")),
            "the schema supplies the entity name"
        );
        assert!(
            t.iter().any(|x| x.contains("query, status")),
            "and its field list"
        );
    }

    #[test]
    fn jazari_without_a_trait_or_with_an_unresolvable_one_says_so() {
        let none = jazari(&json!({"type": "jazari-state-machine"}), None).unwrap();
        assert_eq!(texts(&none), ["No state machine to visualize"]);
        let mut props = jazari_props();
        props["traitIndex"] = json!(5);
        assert_eq!(
            texts(&jazari(&props, None).unwrap()),
            ["No state machine to visualize"],
            "an index past the traits resolves nothing"
        );
        props["traitIndex"] = json!(0);
        assert_ne!(
            texts(&jazari(&props, None).unwrap()),
            ["No state machine to visualize"],
            "control: index 0 resolves"
        );
    }

    #[test]
    fn status_scenes_replace_the_diagram() {
        let mut m = machine();
        m["isLoading"] = json!(true);
        assert_eq!(
            texts(&state_machine_view(&m, None).unwrap()),
            ["Loading state machine\u{2026}"]
        );
        let mut e = machine();
        e["error"] = json!({"message": "boom"});
        assert_eq!(texts(&state_machine_view(&e, None).unwrap()), ["boom"]);
        assert_eq!(
            texts(&state_machine_view(&json!({"type": "state-machine-view"}), None).unwrap()),
            ["No state machine to visualize"]
        );
    }

    #[test]
    fn state_graph_marks_initial_current_and_pending_states_and_reports_node_taps() {
        let props = json!({
            "type": "state-graph", "width": 500, "height": 400, "nodeClickEvent": "PICK_STATE",
            "states": ["idle", "running", "done"], "initialState": "idle", "currentState": "running", "addingFrom": "done",
            "transitions": [{"from": "idle", "to": "running", "event": "START"}, {"from": "running", "to": "done", "event": "FINISH", "guardHint": "ready"}]
        });
        let scene = state_graph(&props, None).unwrap();
        let t = texts(&scene);
        assert!(
            t.iter().any(|x| x == "\"ready\""),
            "a guard hint prints as the quoted string the web formats"
        );
        assert_eq!(t.iter().filter(|x| *x == "now").count(), 1);
        assert_eq!(scene.hits.len(), 3);
        let pending_box = |s: &MappedScene| {
            s.packet.draw_commands.iter().any(|c| matches!(c, DrawCommand::Path { stroke: Some(st), fill: Some(_), segments } if !st.dash.is_empty() && st.width > 1.5 && matches!(segments[0], PathSegment::MoveTo { .. })))
        };
        assert!(
            pending_box(&scene),
            "the pending source gets a dashed emphasised border"
        );
        let mut calm = props.clone();
        calm.as_object_mut().unwrap().remove("addingFrom");
        assert!(
            !pending_box(&state_graph(&calm, None).unwrap()),
            "control: no pending source, no dashed box"
        );
        let mut no_event = props.clone();
        no_event.as_object_mut().unwrap().remove("nodeClickEvent");
        assert!(
            state_graph(&no_event, None).unwrap().hits.is_empty(),
            "control"
        );
        assert_eq!(state_graph(&props, None).unwrap().packet.width, 500.0);
    }

    #[test]
    fn a_state_graph_wider_than_its_box_is_scaled_to_fit_where_the_web_would_scroll() {
        let props = |w: f32| {
            json!({
                "type": "state-graph", "width": w, "height": 400, "nodeClickEvent": "PICK_STATE",
                "states": ["idle", "running", "done"], "initialState": "idle",
                "transitions": [{"from": "idle", "to": "running", "event": "START"}, {"from": "running", "to": "done", "event": "FINISH"}]
            })
        };
        let right_edge = |s: &MappedScene| {
            s.hits
                .iter()
                .map(|h| h.rect.x + h.rect.w)
                .fold(0.0, f32::max)
        };
        let narrow = state_graph(&props(300.0), None).unwrap();
        assert!(
            right_edge(&narrow) <= 300.0,
            "every state fits the 300px box: {}",
            right_edge(&narrow)
        );
        let roomy = state_graph(&props(900.0), None).unwrap();
        assert!(
            right_edge(&roomy) <= 900.0 && right_edge(&roomy) > right_edge(&narrow),
            "control: a roomy box keeps the diagram larger"
        );
    }

    #[test]
    fn right_to_left_puts_the_initial_state_on_the_right() {
        let x_of = |m: &Value| {
            let scene = state_machine_view(m, None).unwrap();
            scene
                .hits
                .iter()
                .find(|h| h.emits[0].payload["stateId"] == "idle")
                .map(|h| h.rect.x)
                .unwrap()
        };
        let mut rtl = machine();
        rtl["direction"] = json!("rtl");
        assert!(x_of(&rtl) > x_of(&machine()));
    }
}
