//! The three patterns that draw the AVL state machine: `state-machine-view` (a trait's
//! `TraitLevelData`), `jazari-state-machine` (an orbital schema or one trait, gear nodes) and
//! `state-graph` (a player-built machine from state names and transitions).

use super::draw::{frame, Ink, Pen};
use super::state_machine::{
    prepare, prepare_orbital, Diagram, Mounted, NodeShape, Opts, OrbitalOpts, StateInfo, TraitData,
    TransitionInfo,
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
    if let Some(mounted) = props.get("orbital").and_then(Mounted::from_value) {
        let theme = theme_for(props);
        let opts = OrbitalOpts::from_props(props);
        let d = prepare_orbital(&mounted, &opts);
        let (xf, pw, ph) = frame(d.width, d.height, viewport);
        let mut pen = Pen::new(&theme, xf);
        pen.clear(theme.ground);
        d.paint(&mut pen);
        return Ok(pen.finish("state-machine-view", pw, ph));
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
                            &orbital_core_base::schema::SExpression::String(g.to_string().into()),
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
    fn jazari_machine_paints_each_source_of_a_multi_source_transition() {
        let mut props = jazari_props();
        let baseline = texts(&jazari(&props, None).unwrap())
            .iter()
            .filter(|text| *text == "COMPARE")
            .count();
        props["schema"]["orbitals"][0]["traits"][0]["stateMachine"]["transitions"][1]["from"] =
            json!(["idle", "embedding"]);
        let trait_data = TraitData::from_trait(
            &props["schema"]["orbitals"][0]["traits"][0],
            "SimilarityComparison",
        )
        .unwrap();
        let sources: Vec<_> = trait_data
            .transitions
            .iter()
            .filter(|transition| transition.index == 1)
            .map(|transition| transition.from.as_str())
            .collect();
        assert_eq!(sources, ["idle", "embedding"]);
        let painted = texts(&jazari(&props, None).unwrap());
        assert_eq!(
            painted.iter().filter(|text| *text == "COMPARE").count(),
            baseline + 1
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

    fn mounted_trait(name: &str, patch: Value) -> Value {
        let mut t = json!({
            "trait": name, "orbital": "O", "parent": null, "depth": 0, "embeddedIn": [],
            "renders": true, "linked": false, "observer": false, "stepsIn": "browser",
            "linkedEntity": "Thing",
            "states": [
                {"name": "idle", "isInitial": true, "isTerminal": false},
                {"name": "busy", "isInitial": false, "isTerminal": false}
            ],
            "transitions": [
                {"index": 0, "from": ["idle"], "to": "busy", "event": "GO", "guarded": false, "emits": ["STARTED"]},
                {"index": 1, "from": ["busy"], "to": "idle", "event": "STOP", "guarded": false, "emits": []}
            ],
            "emits": ["STARTED"], "listens": [], "instances": []
        });
        for (k, v) in patch.as_object().unwrap() {
            t[k] = v.clone();
        }
        t
    }

    fn orbital_props(traits: Vec<Value>, extra: Value) -> Value {
        let mut p = json!({
            "type": "state-machine-view",
            "orbital": {"page": "/p", "traits": traits, "wires": []}
        });
        for (k, v) in extra.as_object().unwrap() {
            if k == "wires" {
                p["orbital"]["wires"] = v.clone();
            } else {
                p[k] = v.clone();
            }
        }
        p
    }

    fn nested() -> Vec<Value> {
        vec![
            mounted_trait("Parent", json!({})),
            mounted_trait(
                "Child",
                json!({"parent": "Parent", "depth": 1, "embeddedIn": ["idle"]}),
            ),
            mounted_trait("Other", json!({})),
        ]
    }

    fn laid_out(props: &Value) -> (Vec<super::super::state_machine::PlacedBox>, f32) {
        let m = Mounted::from_value(&props["orbital"]).unwrap();
        let o = OrbitalOpts::from_props(props);
        let d = prepare_orbital(&m, &o);
        (d.boxes.clone(), d.width)
    }

    fn contains(outer: super::super::layered::Rect, inner: super::super::layered::Rect) -> bool {
        inner.x >= outer.x
            && inner.y >= outer.y
            && inner.x + inner.w <= outer.x + outer.w
            && inner.y + inner.h <= outer.y + outer.h
    }

    fn count(t: &[String], what: &str) -> usize {
        t.iter().filter(|x| *x == what).count()
    }

    fn heavy_strokes(s: &MappedScene) -> usize {
        s.packet
            .draw_commands
            .iter()
            .filter(|c| matches!(c, DrawCommand::Path { stroke: Some(k), .. } if k.width >= 2.0))
            .count()
    }

    #[test]
    fn an_orbital_lays_the_embedded_trait_inside_its_embedders_box() {
        let props = orbital_props(nested(), json!({}));
        let m = Mounted::from_value(&props["orbital"]).unwrap();
        let o = OrbitalOpts::from_props(&props);
        let d = prepare_orbital(&m, &o);
        let by = |n: &str| d.boxes.iter().find(|b| b.trait_name == n).unwrap();
        let (parent, child, other) = (by("Parent"), by("Child"), by("Other"));
        assert_eq!(child.parent.as_deref(), Some("Parent"));
        assert!(parent.container && !child.container);
        assert!(
            contains(parent.rect, child.rect),
            "{parent:?} holds {child:?}"
        );
        assert!(
            !contains(parent.rect, other.rect),
            "a sibling root sits beside it"
        );
        assert_eq!(other.parent, None);
        for s in d.states.iter().filter(|s| s.trait_name == "Child") {
            assert!(
                contains(child.rect, s.rect),
                "child state {s:?} is in the child box"
            );
        }
        for s in d.states.iter().filter(|s| s.trait_name == "Parent") {
            assert!(contains(parent.rect, s.rect));
            assert!(
                !contains(child.rect, s.rect),
                "the parent's own states are not in the child"
            );
        }
        assert_eq!(
            d.embeds.len(),
            1,
            "one dotted tie from the embedding state to the child"
        );
        assert_eq!(d.embeds[0].trait_name, "Child");
        assert_eq!(d.embeds[0].state, "idle");
        let t = texts(&state_machine_view(&props, None).unwrap());
        for name in ["Parent", "Child", "Other", "idle", "busy", "GO", "STOP"] {
            assert!(t.iter().any(|x| x == name), "{name} missing from {t:?}");
        }
        assert_eq!(count(&t, "GO"), 3, "every opened box draws its own machine");
    }

    #[test]
    fn expand_depth_zero_collapses_every_box_but_keeps_the_nesting() {
        let deep = orbital_props(nested(), json!({"expandDepth": 0}));
        let (boxes, _) = laid_out(&deep);
        use super::super::state_machine::BoxKind;
        assert!(boxes.iter().all(|b| b.kind == BoxKind::Collapsed));
        let by = |n: &str| boxes.iter().find(|b| b.trait_name == n).unwrap();
        assert!(contains(by("Parent").rect, by("Child").rect));
        let t = texts(&state_machine_view(&deep, None).unwrap());
        assert!(
            t.iter().any(|x| x == "Child"),
            "the compact box still names the trait"
        );
        for hidden in ["idle", "busy", "GO", "STOP"] {
            assert!(
                !t.iter().any(|x| x == hidden),
                "{hidden} must stay folded: {t:?}"
            );
        }
        let open = orbital_props(nested(), json!({"expandDepth": 2}));
        let (boxes, _) = laid_out(&open);
        assert!(
            boxes.iter().all(|b| b.kind == BoxKind::Machine),
            "control: depth below 2 opens"
        );
        let one = orbital_props(nested(), json!({"expandDepth": 1}));
        let (boxes, _) = laid_out(&one);
        let kind = |n: &str| boxes.iter().find(|b| b.trait_name == n).unwrap().kind;
        assert_eq!(kind("Parent"), BoxKind::Machine);
        assert_eq!(kind("Child"), BoxKind::Collapsed, "depth 1 is not below 1");
    }

    #[test]
    fn an_active_state_is_highlighted_and_a_collapsed_box_names_it() {
        let quiet = state_machine_view(&orbital_props(nested(), json!({})), None).unwrap();
        let live = state_machine_view(
            &orbital_props(
                nested(),
                json!({"activeStates": [{"trait": "Parent", "state": "busy"}]}),
            ),
            None,
        )
        .unwrap();
        assert!(
            heavy_strokes(&live) > heavy_strokes(&quiet),
            "the active state's border is drawn at active weight"
        );
        let folded = state_machine_view(
            &orbital_props(
                nested(),
                json!({"expandDepth": 0, "activeStates": [{"trait": "Child", "state": "busy"}]}),
            ),
            None,
        )
        .unwrap();
        assert!(
            texts(&folded).iter().any(|x| x == "busy"),
            "the compact box carries the state badge"
        );
        let none =
            state_machine_view(&orbital_props(nested(), json!({"expandDepth": 0})), None).unwrap();
        assert!(
            !texts(&none).iter().any(|x| x == "busy"),
            "control: no active state, no badge"
        );
    }

    #[test]
    fn a_running_instance_marks_its_state_when_no_active_state_is_given() {
        let mut t = nested();
        t[0]["instances"] = json!([{"state": "busy"}]);
        let marked = state_machine_view(&orbital_props(t, json!({})), None).unwrap();
        let plain = state_machine_view(&orbital_props(nested(), json!({})), None).unwrap();
        assert!(heavy_strokes(&marked) > heavy_strokes(&plain));
    }

    #[test]
    fn the_fired_transition_is_emphasized_with_its_step_number() {
        let fired = state_machine_view(
            &orbital_props(
                nested(),
                json!({"firedTransition": {"trait": "Parent", "index": 0}}),
            ),
            None,
        )
        .unwrap();
        let quiet = state_machine_view(&orbital_props(nested(), json!({})), None).unwrap();
        assert!(count(&texts(&fired), "1") == 1, "one numbered step");
        assert!(
            count(&texts(&quiet), "1") == 0,
            "control: nothing fired, no step number"
        );
        assert!(
            heavy_strokes(&fired) > heavy_strokes(&quiet),
            "the fired wire is drawn at active weight"
        );
        let routed = state_machine_view(
            &orbital_props(
                nested(),
                json!({"route": [{"trait": "Parent", "index": 0}, {"trait": "Child", "index": 1}]}),
            ),
            None,
        )
        .unwrap();
        let t = texts(&routed);
        assert!(
            t.iter().any(|x| x == "1") && t.iter().any(|x| x == "2"),
            "a route numbers its steps in order"
        );
    }

    #[test]
    fn traits_that_never_render_sit_in_a_background_lane_below_the_page() {
        let props = orbital_props(
            vec![
                mounted_trait("Ui", json!({})),
                mounted_trait("Store", json!({"renders": false})),
            ],
            json!({}),
        );
        let m = Mounted::from_value(&props["orbital"]).unwrap();
        let o = OrbitalOpts::from_props(&props);
        let d = prepare_orbital(&m, &o);
        let by = |n: &str| d.boxes.iter().find(|b| b.trait_name == n).unwrap();
        assert!(by("Store").lane && !by("Ui").lane);
        assert!(
            by("Store").rect.y >= by("Ui").rect.y + by("Ui").rect.h,
            "the lane starts below the tree"
        );
        assert!(d.lane_top.is_some());
        assert!(texts(&state_machine_view(&props, None).unwrap())
            .iter()
            .any(|x| x == "Runs in the background"));
        let alone = orbital_props(vec![mounted_trait("Ui", json!({}))], json!({}));
        let m = Mounted::from_value(&alone["orbital"]).unwrap();
        let o = OrbitalOpts::from_props(&alone);
        assert!(
            prepare_orbital(&m, &o).lane_top.is_none(),
            "control: no lane without background traits"
        );
        assert!(!texts(&state_machine_view(&alone, None).unwrap())
            .iter()
            .any(|x| x == "Runs in the background"));
    }

    #[test]
    fn listen_wires_draw_as_labelled_arrows_only_for_the_chosen_trait_or_the_route() {
        let wires = json!({"wires": [
            {"from": "A", "to": "B", "event": "PING", "triggers": "GO"},
            {"from": "A", "to": "B", "event": "PING", "triggers": "STOP"}
        ]});
        let traits = || vec![mounted_trait("A", json!({})), mounted_trait("B", json!({}))];
        let m_props = orbital_props(traits(), wires.clone());
        let m = Mounted::from_value(&m_props["orbital"]).unwrap();
        let o = OrbitalOpts::from_props(&m_props);
        let d = prepare_orbital(&m, &o);
        assert_eq!(
            d.wires.len(),
            1,
            "two listens between the same pair are one wire"
        );
        assert_eq!(d.wires[0].events, vec!["PING".to_string()]);
        let by = |n: &str| d.boxes.iter().find(|b| b.trait_name == n).unwrap().rect;
        assert!(
            d.wires[0].points[0].0 >= by("A").x && d.wires[0].points[1].0 <= by("B").x + by("B").w,
            "the arrow runs from A to B"
        );
        let chosen = state_machine_view(
            &orbital_props(
                traits(),
                json!({"wires": wires["wires"].clone(), "selectedTrait": "A"}),
            ),
            None,
        )
        .unwrap();
        assert_eq!(count(&texts(&chosen), "PING"), 1);
        let plain = state_machine_view(&m_props, None).unwrap();
        assert_eq!(
            count(&texts(&plain), "PING"),
            0,
            "control: an unchosen wire stays out of sight"
        );
        let routed = state_machine_view(
            &orbital_props(
                traits(),
                json!({"wires": wires["wires"].clone(), "route": [{"trait": "A", "index": 0}, {"trait": "B", "index": 0}]}),
            ),
            None,
        )
        .unwrap();
        assert_eq!(
            count(&texts(&routed), "PING"),
            1,
            "the last dispatch's wire draws"
        );
    }

    #[test]
    fn an_empty_or_null_orbital_paints_the_single_trait_view_exactly_as_before() {
        let base = machine();
        let want = serde_json::to_string(&state_machine_view(&base, None).unwrap().packet).unwrap();
        for orbital in [
            Value::Null,
            json!({"page": "/p", "traits": [], "wires": []}),
        ] {
            let mut p = base.clone();
            p["orbital"] = orbital;
            let got = serde_json::to_string(&state_machine_view(&p, None).unwrap().packet).unwrap();
            assert_eq!(got, want);
        }
        let mut with = base.clone();
        with["orbital"] = orbital_props(nested(), json!({}))["orbital"].clone();
        let got = serde_json::to_string(&state_machine_view(&with, None).unwrap().packet).unwrap();
        assert_ne!(got, want, "a real orbital wins over the trait prop");
    }
}
