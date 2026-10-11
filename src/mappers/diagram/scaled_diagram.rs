//! `scaled-diagram` (`ScaledDiagram.tsx`): wraps a fixed-size diagram and scales it down to the
//! container width (never up), top-left anchored, a box as tall as the scaled diagram. Here the
//! wrapped diagram is the first child the Rust renderer can draw (another scene pattern); the
//! child is laid out at the scale directly, so its hit regions and text scale with it.

use super::draw::frame;
use crate::mappers::game_canvas_2d::CanvasSceneError;
use crate::mappers::learning::gap_once;
use crate::mappers::scene::{is_scene_pattern, map_scene, scene_size, MappedScene};
use serde_json::Value;

/// A diagram narrower than this is not a "real diagram worth scaling" (`MIN_DIAGRAM_WIDTH`).
const MIN_DIAGRAM_WIDTH: f32 = 200.0;

fn type_of(node: &Value) -> Option<&str> {
    let obj = node.as_object()?;
    obj.get("type")
        .or_else(|| obj.get("props").and_then(|p| p.get("type")))
        .and_then(Value::as_str)
}

fn children_of(node: &Value) -> Vec<&Value> {
    let obj = match node.as_object() {
        Some(o) => o,
        None => return Vec::new(),
    };
    let list = obj
        .get("children")
        .or_else(|| obj.get("props").and_then(|p| p.get("children")));
    match list {
        Some(Value::Array(items)) => items.iter().collect(),
        Some(single @ Value::Object(_)) => vec![single],
        _ => Vec::new(),
    }
}

/// The first descendant that is a scene pattern, looking through wrapper nodes (`boxedChildren`).
fn first_diagram(node: &Value) -> Option<&Value> {
    for child in children_of(node) {
        if type_of(child).is_some_and(is_scene_pattern) {
            return Some(child);
        }
        if let Some(found) = first_diagram(child) {
            return Some(found);
        }
    }
    None
}

pub fn map(props: &Value, viewport: Option<(f32, f32)>) -> Result<MappedScene, CanvasSceneError> {
    let Some(child) = first_diagram(props) else {
        gap_once(
            "scaled-diagram-child",
            "scaled-diagram holds no child the Rust renderer can draw",
        );
        return Err(CanvasSceneError::NothingToDraw("scaled-diagram".into()));
    };
    let (cw, ch) = scene_size(child)?;
    let scale = match viewport {
        Some((vw, _)) if cw >= MIN_DIAGRAM_WIDTH && ch > 0.0 => (vw / cw).min(1.0),
        _ => 1.0,
    };
    let mut scene = map_scene(child, Some((cw * scale, ch * scale)))?;
    if let Some((vw, vh)) = viewport {
        let (_, pw, ph) = frame(vw, vh, Some((vw, vh)));
        scene.packet.width = pw.max(scene.packet.width);
        scene.packet.height = ph.max(scene.packet.height);
    }
    scene.packet.pattern = "scaled-diagram".into();
    Ok(scene)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn machine() -> Value {
        serde_json::from_str(include_str!(
            "fixtures/state-machine-view-similarity-run.json"
        ))
        .unwrap()
    }

    /// `std-behavior-tour` style: the diagram sits inside a wrapper node.
    fn wrapped() -> Value {
        json!({"type": "scaled-diagram", "children": [{"type": "box", "children": [machine()]}]})
    }

    fn right_edge(s: &MappedScene) -> f32 {
        s.hits
            .iter()
            .map(|h| h.rect.x + h.rect.w)
            .fold(0.0, f32::max)
    }

    #[test]
    fn a_wide_diagram_scales_down_to_the_container_width_and_never_up() {
        let natural = map(&wrapped(), None).unwrap();
        let nw = natural.packet.width;
        assert!(nw > 400.0, "the machine is wider than a phone: {nw}");
        let narrow = map(&wrapped(), Some((nw / 2.0, 400.0))).unwrap();
        assert!(
            right_edge(&narrow) <= nw / 2.0 + 1.0,
            "every region fits the half-width container"
        );
        assert!(
            right_edge(&narrow) < right_edge(&natural) * 0.75,
            "and the diagram shrank to get there"
        );
        let wide = map(&wrapped(), Some((nw * 2.0, 1000.0))).unwrap();
        assert!(
            (right_edge(&wide) - right_edge(&natural)).abs() < 0.5,
            "control: a wider container leaves the diagram at its natural size"
        );
    }

    #[test]
    fn child_taps_still_reach_the_childs_events_after_scaling() {
        let narrow = map(&wrapped(), Some((300.0, 300.0))).unwrap();
        let hit = narrow
            .hits
            .iter()
            .find(|h| h.emits[0].payload["stateId"] == "embedding")
            .expect("state region");
        let (x, y) = (hit.rect.x + hit.rect.w / 2.0, hit.rect.y + hit.rect.h / 2.0);
        assert_eq!(narrow.emits_at(x, y)[0].event, "STATE_CLICK");
    }

    #[test]
    fn no_drawable_child_is_nothing_to_draw_and_a_direct_child_works() {
        let tour = json!({"type": "scaled-diagram", "children": [{"type": "box", "className": "w-[1280px]", "children": ["@config.demo"]}]});
        assert!(
            matches!(map(&tour, None), Err(CanvasSceneError::NothingToDraw(_))),
            "a box of arbitrary widgets is not a Rust scene"
        );
        assert!(matches!(
            map(&json!({"type": "scaled-diagram"}), None),
            Err(CanvasSceneError::NothingToDraw(_))
        ));
        let direct = json!({"type": "scaled-diagram", "children": [machine()]});
        assert_eq!(map(&direct, None).unwrap().packet.pattern, "scaled-diagram");
    }
}
