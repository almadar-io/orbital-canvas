//! The learning canvases' 3D modes beyond the scene: the 2D families that read as a layer over the scene (readouts,
//! traces, shapes, physics meters, the chemistry equation) drawn by the 2D learning mapper over the
//! [`DrawCommand::Scene3D`], and the click bridge that turns a picked mesh id into the molecule's
//! `onShapeClick({ id, index })` payload (`get3DClickPayload`).

use super::game_canvas_2d::CanvasSceneError;
use super::learning;
use crate::packet::DrawCommand;
use serde_json::{json, Map, Value};

type Fields = Map<String, Value>;

/// The families every learning canvas paints over its 3D scene, and each molecule's own.
const COMMON_OVERLAY: &[&str] = &["shapes", "readouts", "traces"];
const PHYSICS_OVERLAY: &[&str] = &["meters"];
const CHEMISTRY_OVERLAY: &[&str] = &["equation", "equationColor"];
/// What the 2D mapper lays the overlay out and themes it with.
const LAYOUT: &[&str] = &["width", "height", "series"];

fn fields(node: &Value) -> Result<(&str, &Fields), CanvasSceneError> {
    let object = node.as_object().ok_or(CanvasSceneError::NotAnObject)?;
    let f = object
        .get("props")
        .and_then(Value::as_object)
        .unwrap_or(object);
    let pattern = object
        .get("type")
        .or_else(|| f.get("type"))
        .and_then(Value::as_str)
        .ok_or_else(|| bad("type", "is required"))?;
    Ok((pattern, f))
}

/// The 2D commands to paint after the scene in a `width` x `height` viewport: the overlay families exactly as the
/// 2D canvas lays them out (contain-fit), without its ground. Nothing when the node declares none.
pub fn learning_overlay_2d(
    node: &Value,
    viewport: (f32, f32),
) -> Result<Vec<DrawCommand>, CanvasSceneError> {
    let (pattern, f) = fields(node)?;
    let own: &[&str] = match pattern {
        "physics-canvas" => PHYSICS_OVERLAY,
        "chemistry-canvas" => CHEMISTRY_OVERLAY,
        "biology-canvas" => &[],
        other => return Err(bad("type", format!("`{other}` has no learning 3D mode"))),
    };
    let declared = |key: &&str| f.get(*key).is_some_and(|v| !v.is_null());
    if !COMMON_OVERLAY.iter().chain(own).any(declared) {
        return Ok(Vec::new());
    }
    let mut props = Fields::new();
    for key in LAYOUT.iter().chain(COMMON_OVERLAY).chain(own) {
        if let Some(value) = f.get(*key).filter(|v| !v.is_null()) {
            props.insert((*key).to_string(), value.clone());
        }
    }
    let scene = learning::map(pattern, &Value::Object(props), Some(viewport))?;
    let mut commands = scene.packet.draw_commands.into_iter();
    match commands.next() {
        Some(DrawCommand::Clear { .. }) => Ok(commands.collect()),
        other => Err(bad(
            pattern,
            format!("the 2D learning canvas starts with its ground, not {other:?}"),
        )),
    }
}

/// `get3DClickPayload`: `{ id, index }` where `index` is the item's place in its vocabulary list (`-1` when no
/// item carries the id). Physics maps `vectors3d` then `bodies`, biology `helix3d.rungs` then `nodes` (a body or
/// node wins an id it shares), chemistry `atoms`.
pub fn learning_click_payload(node: &Value, id: &str) -> Result<Value, CanvasSceneError> {
    let (pattern, f) = fields(node)?;
    let lists: Vec<(String, &Value)> = match pattern {
        "physics-canvas" => vec![
            (
                "vectors3d".into(),
                f.get("vectors3d").unwrap_or(&Value::Null),
            ),
            ("bodies".into(), f.get("bodies").unwrap_or(&Value::Null)),
        ],
        "biology-canvas" => vec![
            (
                "helix3d.rungs".into(),
                f.get("helix3d")
                    .and_then(|h| h.get("rungs"))
                    .unwrap_or(&Value::Null),
            ),
            ("nodes".into(), f.get("nodes").unwrap_or(&Value::Null)),
        ],
        "chemistry-canvas" => vec![("atoms".into(), f.get("atoms").unwrap_or(&Value::Null))],
        other => return Err(bad("type", format!("`{other}` has no learning 3D mode"))),
    };
    let mut index: i64 = -1;
    for (name, list) in lists {
        let items = match list {
            Value::Null => continue,
            Value::Array(items) => items,
            _ => return Err(bad(&format!("{pattern}.{name}"), "must be an array")),
        };
        for (i, item) in items.iter().enumerate() {
            if item.get("id").and_then(Value::as_str) == Some(id) && !id.is_empty() {
                index = i as i64;
            }
        }
    }
    Ok(json!({ "id": id, "index": index }))
}

fn bad(field: &str, detail: impl std::fmt::Display) -> CanvasSceneError {
    CanvasSceneError::InvalidProp {
        field: field.to_string(),
        detail: detail.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readouts_and_meters_paint_over_the_scene_without_the_ground_and_control_none_paints_nothing()
    {
        let node = json!({"type": "physics-canvas", "mode": "3d", "width": 600, "height": 400,
            "bodies": [{"id": "b", "x": 1, "y": 2, "z": 0}],
            "readouts": [{"label": "v", "value": 3}],
            "meters": [{"label": "KE", "value": 2.5, "max": 10}]});
        let cmds = learning_overlay_2d(&node, (600.0, 400.0)).unwrap();
        assert!(!cmds.is_empty());
        assert!(!cmds.iter().any(|c| matches!(c, DrawCommand::Clear { .. })));
        let texts: Vec<&str> = cmds
            .iter()
            .filter_map(|c| match c {
                DrawCommand::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(texts.iter().any(|t| t.contains("KE")), "{texts:?}");
        let bare = json!({"type": "physics-canvas", "mode": "3d", "bodies": [{"id": "b", "x": 1, "y": 2}]});
        assert!(learning_overlay_2d(&bare, (600.0, 400.0))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn the_equation_overlays_chemistry_and_control_biology_has_no_equation_family() {
        let chem =
            json!({"type": "chemistry-canvas", "mode": "3d", "equation": "H2O", "atoms": []});
        let cmds = learning_overlay_2d(&chem, (600.0, 400.0)).unwrap();
        assert!(cmds
            .iter()
            .any(|c| matches!(c, DrawCommand::Text { text, .. } if text == "H2O")));
        let bio = json!({"type": "biology-canvas", "mode": "3d", "equation": "H2O"});
        assert!(learning_overlay_2d(&bio, (600.0, 400.0))
            .unwrap()
            .is_empty());
        assert!(learning_overlay_2d(&json!({"type": "canvas"}), (1.0, 1.0)).is_err());
    }

    #[test]
    fn a_body_wins_an_id_it_shares_with_a_vector_and_control_an_unknown_id_is_minus_one() {
        let node = json!({"type": "physics-canvas", "mode": "3d",
            "vectors3d": [{"id": "x"}, {"id": "shared"}],
            "bodies": [{"id": "a"}, {"id": "b"}, {"id": "shared"}]});
        assert_eq!(
            learning_click_payload(&node, "shared").unwrap(),
            json!({"id": "shared", "index": 2})
        );
        assert_eq!(
            learning_click_payload(&node, "x").unwrap(),
            json!({"id": "x", "index": 0})
        );
        assert_eq!(
            learning_click_payload(&node, "hx-a-3").unwrap(),
            json!({"id": "hx-a-3", "index": -1})
        );
    }

    #[test]
    fn biology_maps_rungs_then_nodes_and_chemistry_atoms_and_control_a_bad_list_names_its_field() {
        let bio = json!({"type": "biology-canvas", "helix3d": {"rungs": [{"id": "r0"}, {"id": "n"}]},
            "nodes": [{"id": "n"}]});
        assert_eq!(learning_click_payload(&bio, "r0").unwrap()["index"], 0);
        assert_eq!(learning_click_payload(&bio, "n").unwrap()["index"], 0);
        let chem = json!({"type": "chemistry-canvas", "atoms": [{"id": "o"}, {"id": "h1"}]});
        assert_eq!(learning_click_payload(&chem, "h1").unwrap()["index"], 1);
        let broken = json!({"type": "chemistry-canvas", "atoms": {"id": "o"}});
        assert!(matches!(
            learning_click_payload(&broken, "o"),
            Err(CanvasSceneError::InvalidProp { ref field, .. }) if field == "chemistry-canvas.atoms"
        ));
    }
}
