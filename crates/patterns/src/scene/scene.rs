//! One render-ui scene mapped for native: which mapper a pattern goes to, and the mapped scene.

pub use orbital_canvas_map::scene::*;

use crate::{diagram, learning};
use orbital_canvas_base::packet::GameRenderPacket;
use orbital_canvas_map::error::CanvasSceneError;
use orbital_canvas_base::scene_doc::SceneDoc;
use orbital_canvas_map::game_canvas_2d;
use serde_json::Value;

/// Patterns the scene mapper draws itself: the learning canvases, the diagram patterns and the
/// game canvases.
pub fn is_scene_pattern(pattern: &str) -> bool {
    learning::handles(pattern)
        || diagram::handles(pattern)
        || matches!(pattern, "canvas" | "canvas-2d")
}

/// Every registry pattern a host draws as a Rust scene: the learning canvases, the diagrams, the
/// `svg-*` atoms and the game `canvas`. A [`diagram::SCENE_ONLY`] scene names no registry pattern.
pub fn scene_patterns() -> impl Iterator<Item = &'static str> {
    learning::PATTERNS
        .iter()
        .chain(diagram::PATTERNS)
        .chain(diagram::svg_atoms::ATOMS)
        .chain(&["canvas", "canvas-2d"])
        .copied()
}

/// The node a host hands a scene: the pattern's props under its `type`, plus the kernel's
/// `canvasTheme` when the host has one (absent, scenes paint with their built-in palette).
pub fn host_node(pattern: &str, props: Value, canvas_theme: Option<&Value>) -> Value {
    let mut fields = match props {
        Value::Object(fields) => fields,
        _ => serde_json::Map::new(),
    };
    fields.insert("type".to_string(), Value::String(pattern.to_string()));
    if let Some(theme) = canvas_theme {
        fields.insert("canvasTheme".to_string(), theme.clone());
    }
    orbital_canvas_base::exact_ints::exact_ints(Value::Object(fields))
}

/// Map a render-ui node to its scene. The node's `type` names the pattern: the
/// learning canvases go to [`learning::map`], game `canvas` uses its authored
/// 2D or 3D mode. `viewport` is the host surface in
/// logical points; absent, scenes use their own authored size.
pub fn map_scene(
    node: &Value,
    viewport: Option<(f32, f32)>,
) -> Result<MappedScene, CanvasSceneError> {
    map_scene_cached(node, viewport, &mut game_canvas_2d::DrawableCache::default())
}

/// [`map_scene`] replaying the unchanged drawables of a game canvas `cache` kept from the last mapping.
fn map_scene_cached(
    node: &Value,
    viewport: Option<(f32, f32)>,
    cache: &mut game_canvas_2d::DrawableCache,
) -> Result<MappedScene, CanvasSceneError> {
    let obj = node.as_object().ok_or(CanvasSceneError::NotAnObject)?;
    let fields = match obj.get("props") {
        Some(Value::Object(nested)) => nested,
        _ => obj,
    };
    let pattern = obj
        .get("type")
        .or_else(|| fields.get("type"))
        .and_then(Value::as_str);
    let three_d = fields.get("mode").and_then(Value::as_str) == Some("3d");
    if three_d
        && matches!(
            pattern,
            Some("canvas" | "physics-canvas" | "biology-canvas" | "chemistry-canvas")
        )
    {
        return scene3d_packet(
            pattern.unwrap_or("canvas"),
            &Value::Object(fields.clone()),
            viewport,
        );
    }
    if let Some(pattern) = pattern.filter(|p| learning::handles(p)) {
        return learning::map(pattern, &Value::Object(fields.clone()), viewport);
    }
    if let Some(pattern) = pattern.filter(|p| diagram::handles(p)) {
        return diagram::map(pattern, &Value::Object(fields.clone()), viewport);
    }
    let prepared = game_canvas_2d::PreparedDoc { props: game_canvas_2d::prepare_canvas_node(node)?, drawables: None };
    let (packet, gathered) = game_canvas_2d::from_prepared_doc_cached(&prepared, viewport, 0.0, cache)?;
    Ok(game_canvas_scene(fields, packet, gathered, prepared))
}

/// Whether `node` is drawn by the 2D drawable walk, whose drawables a host may hold apart ([`SceneDoc`]).
pub fn walks_drawables(node: &Value) -> bool {
    let Some(obj) = node.as_object() else {
        return false;
    };
    let fields = match obj.get("props") {
        Some(Value::Object(nested)) => nested,
        _ => obj,
    };
    let pattern = obj.get("type").or_else(|| fields.get("type")).and_then(Value::as_str);
    pattern_walks_drawables(pattern, fields.get("mode").and_then(Value::as_str))
}

/// [`walks_drawables`] of a node of `pattern` in `mode`.
pub fn pattern_walks_drawables(pattern: Option<&str>, mode: Option<&str>) -> bool {
    let three_d = mode == Some("3d");
    !(pattern == Some("graph-canvas")
        || (three_d && matches!(pattern, Some("canvas" | "physics-canvas" | "biology-canvas" | "chemistry-canvas")))
        || pattern.is_some_and(|p| learning::handles(p) || diagram::handles(p)))
}

/// [`map_scene_with`] of a scene whose drawables are held apart: only a drawable that is new since the last mapping
/// is themed and walked again. A scene kept whole maps as [`map_scene_with`] maps it.
pub fn map_scene_doc_with(
    doc: &SceneDoc,
    viewport: Option<(f32, f32)>,
    memory: &mut SceneMemory,
) -> Result<MappedScene, CanvasSceneError> {
    if doc.drawables.is_none() {
        return map_scene_with(&doc.frame, viewport, memory);
    }
    let fields = doc.root().as_object().ok_or(CanvasSceneError::NotAnObject)?;
    let pattern = doc.frame.get("type").or_else(|| fields.get("type")).and_then(Value::as_str);
    if memory.pattern.as_deref() != pattern {
        *memory = SceneMemory { pattern: pattern.map(str::to_string), ..SceneMemory::default() };
    }
    let prepared = game_canvas_2d::prepare_doc(doc, &mut memory.themed)?;
    let (packet, gathered) = game_canvas_2d::from_prepared_doc_cached(&prepared, viewport, 0.0, &mut memory.drawables)?;
    Ok(game_canvas_scene(fields, packet, gathered, prepared))
}

fn game_canvas_scene(
    fields: &serde_json::Map<String, Value>,
    packet: GameRenderPacket,
    gathered: game_canvas_2d::Gathered,
    prepared: game_canvas_2d::PreparedDoc,
) -> MappedScene {
    let event = |key: &str| fields.get(key).and_then(Value::as_str).map(str::to_string);
    let events = PointerEvents {
        down: event("pointerDownEvent"),
        up: event("pointerUpEvent"),
        enter: event("pointerEnterEvent"),
        leave: event("pointerLeaveEvent"),
        moved: event("pointerMoveEvent"),
    };
    MappedScene {
        pointer_regions: gathered
            .regions
            .into_iter()
            .map(|region| PointerRegion {
                id: region.id,
                rect: region.rect,
                rotation: region.rotation,
                events: events.clone(),
                wheel: region.wheel,
                fixed: region.fixed,
                drag: region.drag,
            })
            .collect(),
        measured: gathered.measured,
        text_hits: gathered.text_hits,
        semantic_nodes: {
            let mut nodes = gathered.semantic;
            nodes.sort_by(|a, b| a.order.total_cmp(&b.order));
            nodes
        },
        live_scenes3d: gathered.live_scenes3d,
        prepared: Some(prepared),
        ..MappedScene::drawn(packet)
    }
}

/// A 3D canvas as one [`DrawCommand::Scene3D`](orbital_canvas_base::packet::DrawCommand::Scene3D) across the viewport.
fn scene3d_packet(
    pattern: &str,
    fields: &Value,
    viewport: Option<(f32, f32)>,
) -> Result<MappedScene, CanvasSceneError> {
    let (width, height) = viewport.unwrap_or((800.0, 600.0));
    let mut node = fields.clone();
    // the host's canvas theme is the scene's theme, not one of the pattern's props
    let theme = match node
        .as_object_mut()
        .and_then(|map| map.remove("canvasTheme"))
    {
        Some(theme) => orbital_canvas_map::canvas_scene3d::SceneTheme::from_canvas_theme(&theme)?,
        None => orbital_canvas_map::canvas_scene3d::SceneTheme::default(),
    };
    let scene = if pattern == "canvas" {
        orbital_canvas_map::canvas_scene3d::scene_from_canvas_node_themed(
            &node, width, height, &theme,
        )?
    } else {
        if let Some(map) = node.as_object_mut() {
            map.insert("type".into(), Value::String(pattern.into()));
        }
        crate::learning_scene3d::learning_scene3d(&node, width, height, &theme)?
    };
    Ok(MappedScene::drawn(GameRenderPacket {
        pattern: pattern.to_string(),
        width,
        height,
        draw_commands: vec![orbital_canvas_base::packet::DrawCommand::Scene3D {
            scene: Box::new(scene),
        }],
        ..Default::default()
    }))
}

/// [`map_scene`] with the renderer's memory of this scene: a graph keeps its settled layout and
/// view across re-maps, and applies the pan and zoom gestures queued on `memory`.
pub fn map_scene_with(
    node: &Value,
    viewport: Option<(f32, f32)>,
    memory: &mut SceneMemory,
) -> Result<MappedScene, CanvasSceneError> {
    let obj = node.as_object().ok_or(CanvasSceneError::NotAnObject)?;
    let fields = match obj.get("props") {
        Some(Value::Object(nested)) => nested,
        _ => obj,
    };
    let pattern = obj
        .get("type")
        .or_else(|| fields.get("type"))
        .and_then(Value::as_str);
    if memory.pattern.as_deref() != pattern {
        *memory = SceneMemory {
            pattern: pattern.map(str::to_string),
            ..SceneMemory::default()
        };
    }
    if pattern == Some("graph-canvas") {
        return learning::graph_canvas::map_with(&Value::Object(fields.clone()), viewport, memory);
    }
    map_scene_cached(node, viewport, &mut memory.drawables)
}

/// The scene's own size in logical units (what the host sizes its surface to by aspect ratio):
/// the authored `width` x `height` of a learning canvas, the default board of a game canvas.
pub fn scene_size(node: &Value) -> Result<(f32, f32), CanvasSceneError> {
    let mapped = map_scene(node, None)?;
    Ok((mapped.packet.width, mapped.packet.height))
}

#[cfg(test)]
mod tests {
    use orbital_canvas_base::packet::Rect;

    fn scene3d(node: serde_json::Value) -> orbital_canvas_base::scene3d::Scene3D {
        let mapped = super::map_scene(&node, Some((600.0, 400.0))).expect("a 3D scene");
        match mapped.packet.draw_commands.as_slice() {
            [orbital_canvas_base::packet::DrawCommand::Scene3D { scene }] => (**scene).clone(),
            other => panic!("expected one 3D scene: {other:?}"),
        }
    }

    #[test]
    fn a_host_canvas_theme_themes_a_3d_scene_instead_of_being_refused_as_a_prop() {
        let node = |theme: Option<serde_json::Value>| {
            let mut n = json!({"type": "physics-canvas", "mode": "3d", "bodies": [{"id": "b", "x": 1, "y": 2}]});
            if let Some(t) = theme {
                n["canvasTheme"] = t;
            }
            n
        };
        let themed = scene3d(node(Some(
            json!({"ground": "transparent", "tones": {"ink": "currentColor"},
            "scene": {"ambient": 0.2}}),
        )));
        let plain = scene3d(node(None));
        assert_ne!(
            themed.lights.ambient, plain.lights.ambient,
            "the theme's ambient reaches the lights"
        );
        let unset = scene3d(node(Some(json!({"ground": "transparent", "tones": {}}))));
        assert_eq!(
            unset, plain,
            "control: a theme without a scene axis draws the resolver's fallbacks"
        );
    }

    #[test]
    fn a_canvas_theme_reads_ground_tones_and_the_scene_axis() {
        let t = orbital_canvas_map::canvas_scene3d::SceneTheme::from_canvas_theme(&json!({
            "ground": "#112233", "tones": {"ink": "#000000", "axis": "currentColor"},
            "scene": {"ambient": 0.3, "keyColor": "#ff0000", "flat": true}}))
        .unwrap();
        assert_eq!(t.ground.as_deref(), Some("#112233"));
        assert_eq!(t.tones.get("ink").map(String::as_str), Some("#000000"));
        assert!(
            !t.tones.contains_key("axis"),
            "a tone left at currentColor is unset"
        );
        assert_eq!(
            (t.ambient, t.key_color.as_str(), t.flat, t.key),
            (0.3, "#ff0000", true, 1.0)
        );
        assert!(
            orbital_canvas_map::canvas_scene3d::SceneTheme::from_canvas_theme(
                &json!({"scene": {"flat": 1}})
            )
            .is_err()
        );
    }

    #[test]
    fn scene_patterns_are_the_registry_names_every_host_draws_as_a_scene() {
        let names: Vec<&str> = super::scene_patterns().collect();
        for name in [
            "graph-canvas",
            "state-machine-view",
            "svg-flow",
            "canvas",
            "canvas-2d",
        ] {
            assert!(names.contains(&name), "{name}");
        }
        assert!(names.iter().all(|n| super::is_scene_pattern(n)));
        assert!(
            !names.contains(&"mermaid"),
            "control: a scene-only kind names no pattern"
        );
        assert!(
            !names.contains(&"button"),
            "control: a component is not a scene"
        );
    }

    use super::*;

    fn headed_canvas(level: serde_json::Value, max_lines: serde_json::Value) -> serde_json::Value {
        host_node(
            "canvas",
            serde_json::json!({"projection": "flat", "camera": {"zoom": 1}, "backgroundColor": "#000000",
                "children": [{"type": "draw-text", "id": "t", "text": "Overview", "anchor": "top-left",
                    "position": {"x": 0.5, "y": 0.0}, "color": "#111111", "width": 120.0, "maxLines": max_lines,
                    "semantic": {"role": "heading", "level": level, "name": "Overview"}}]}),
            None,
        )
    }

    #[test]
    fn a_host_node_reads_a_whole_float_as_the_int_it_is() {
        let view = Some((600.0, 400.0));
        let floated = map_scene(&headed_canvas(serde_json::json!(2.0), serde_json::json!(3.0)), view)
            .expect("a kernel Number(2.0) is the int 2 (EXEC-EVAL-11)");
        let int = map_scene(&headed_canvas(serde_json::json!(2), serde_json::json!(3)), view).unwrap();
        assert_eq!(floated.semantic_nodes[0].level, Some(2));
        assert_eq!(floated.semantic_nodes, int.semantic_nodes);
        assert_eq!(format!("{:?}", floated.packet), format!("{:?}", int.packet));
    }

    #[test]
    fn control_a_host_node_still_refuses_a_level_that_is_no_int() {
        let refused = map_scene(&headed_canvas(serde_json::json!(2.5), serde_json::json!(3)), Some((600.0, 400.0)));
        assert!(
            matches!(&refused, Err(CanvasSceneError::InvalidProp { field, .. }) if field == "semantic.level"),
            "{refused:?}"
        );
    }

    #[test]
    fn authored_3d_canvas_routes_to_one_scene3d_command() {
        let node = serde_json::json!({
            "type": "canvas",
            "props": {
                "mode": "3d",
                "backgroundColor": "#000000",
                "camera": {"mode": "perspective", "fov": 60},
                "children": [{"type": "draw-mesh", "props": {
                    "shape": "box", "position": {"x": 0, "y": 0, "z": 0},
                    "material": {"kind": "basic", "side": "double", "color": "#00ff00"}
                }}]
            }
        });
        let mapped = map_scene(&node, Some((320.0, 240.0))).unwrap();
        assert_eq!(mapped.packet.pattern, "canvas");
        assert!(matches!(
            mapped.packet.draw_commands.as_slice(),
            [orbital_canvas_base::packet::DrawCommand::Scene3D { scene }] if scene.meshes.len() == 1
        ));
    }

    #[test]
    fn a_3d_learning_canvas_routes_to_scene3d_and_control_its_2d_mode_does_not() {
        let three_d = serde_json::json!({"type": "physics-canvas", "mode": "3d", "bodies": []});
        let mapped = map_scene(&three_d, Some((320.0, 240.0))).unwrap();
        assert!(matches!(
            mapped.packet.draw_commands.as_slice(),
            [orbital_canvas_base::packet::DrawCommand::Scene3D { .. }]
        ));
        let flat = serde_json::json!({"type": "physics-canvas", "bodies": []});
        let mapped = map_scene(&flat, Some((320.0, 240.0))).unwrap();
        assert!(!mapped
            .packet
            .draw_commands
            .iter()
            .any(|c| matches!(c, orbital_canvas_base::packet::DrawCommand::Scene3D { .. })));
    }
    use serde_json::json;

    #[test]
    fn a_callback_payload_is_delivered_under_its_declared_argument_name() {
        let wrapped = Emit {
            event: "E".into(),
            arg: "payload".into(),
            payload: json!({"id": "a"}),
        };
        assert_eq!(wrapped.delivered(), json!({"payload": {"id": "a"}}));
        let flat = Emit {
            event: "E".into(),
            arg: String::new(),
            payload: json!({"id": "a"}),
        };
        assert_eq!(
            flat.delivered(),
            json!({"id": "a"}),
            "control: an event-ref payload is not wrapped"
        );
    }

    #[test]
    fn a_learning_scene_reports_its_authored_size_and_a_control_prop_changes_it() {
        let node = json!({"type": "learning-canvas", "width": 600, "height": 380});
        assert_eq!(scene_size(&node).unwrap(), (600.0, 380.0));
        let taller = json!({"type": "learning-canvas", "width": 600, "height": 500});
        assert_eq!(scene_size(&taller).unwrap(), (600.0, 500.0));
        let nested = json!({"type": "math-canvas", "props": {"width": 300, "height": 200}});
        assert_eq!(scene_size(&nested).unwrap(), (300.0, 200.0));
    }

    #[test]
    fn the_type_picks_the_mapper_and_an_unmapped_pattern_is_refused() {
        let learning = map_scene(&json!({"type": "learning-canvas"}), None).unwrap();
        assert_eq!(learning.packet.pattern, "learning-canvas");
        let game = map_scene(&json!({"type": "canvas-2d"}), None).unwrap();
        assert_eq!(game.packet.pattern, "canvas-2d");
        assert_eq!(
            map_scene(&json!(3), None).unwrap_err(),
            CanvasSceneError::NotAnObject
        );
    }

    #[test]
    fn an_orbital_painter_canvas_takes_its_declared_world_size_and_a_plain_one_does_not() {
        let orbital = json!({"type": "canvas", "mode": "2d", "projection": "free", "painter": "orbital",
            "worldWidth": 1200, "worldHeight": 760, "drawables": []});
        assert_eq!(scene_size(&orbital).unwrap(), (1200.0, 760.0));
        let plain = json!({"type": "canvas", "mode": "2d", "projection": "free",
            "worldWidth": 1200, "worldHeight": 760, "drawables": []});
        assert_eq!(
            scene_size(&plain).unwrap(),
            (800.0, 600.0),
            "control: today's games keep width/height"
        );
    }

    #[test]
    fn a_canvas_scene_carries_pointer_regions_with_the_canvas_events() {
        let node = json!({"type": "canvas", "projection": "free", "scale": 1.0,
        "pointerDownEvent": "DOWN", "pointerMoveEvent": "MOVE",
        "drawables": [
            {"type": "draw-shape", "shape": "rect", "id": "r", "hit": true,
             "position": {"x": 4, "y": 6}, "width": 64, "height": 32, "fill": "#ff0000"},
            {"type": "draw-shape", "shape": "rect", "id": "plain",
             "position": {"x": 4, "y": 6}, "width": 64, "height": 32, "fill": "#ff0000"}
        ]});
        let mapped = map_scene(&node, Some((200.0, 100.0))).unwrap();
        assert_eq!(mapped.pointer_regions.len(), 1, "only the hit drawable");
        let region = &mapped.pointer_regions[0];
        assert_eq!(region.id, "r");
        assert_eq!(
            region.rect,
            Rect {
                x: 4.0,
                y: 6.0,
                w: 64.0,
                h: 32.0
            }
        );
        assert_eq!(region.events.down.as_deref(), Some("DOWN"));
        assert_eq!(region.events.moved.as_deref(), Some("MOVE"));
        assert_eq!(
            region.events.enter, None,
            "control: an unset event stays unset"
        );
        let learning = map_scene(&json!({"type": "learning-canvas"}), None).unwrap();
        assert!(learning.pointer_regions.is_empty() && learning.measured.is_empty());
    }
}
