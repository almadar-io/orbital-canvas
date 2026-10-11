//! `learning-canvas` — the declarative atom itself: the authored `shapes`
//! drawn as-is, plus its `readouts` and `traces`.

use super::canvas::{self, CanvasSpec};
use super::shape::parse_shapes;
use crate::mappers::scene::MappedScene;
use serde_json::Value;

pub fn map(props: &Value, viewport: Option<(f32, f32)>) -> MappedScene {
    let spec = CanvasSpec::from_props(
        "learning-canvas",
        props,
        600.0,
        400.0,
        parse_shapes(props.get("shapes")),
    );
    canvas::render(&spec, &canvas::theme_for(props), viewport)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::{DrawCommand, PathSegment};
    use serde_json::json;

    /// `std-learn-hotspot` at its config defaults: the cell parts become solid
    /// circles in their series tone plus a centered `xs` label under each, drawn
    /// on a 600x380 canvas that reports taps through `TAP_SHAPE`.
    fn hotspot_scene() -> Value {
        let parts = [
            ("nucleus", "Nucleus", 300, 170, 34, "series-1"),
            ("membrane", "Membrane", 130, 120, 26, "series-2"),
            ("mitochondrion", "Mitochondrion", 460, 120, 24, "series-3"),
            ("ribosome", "Ribosome", 200, 280, 18, "series-4"),
            ("vacuole", "Vacuole", 420, 280, 28, "series-5"),
        ];
        let mut shapes: Vec<Value> = parts
            .iter()
            .map(|(id, _, x, y, r, c)| {
                json!({"type": "circle", "id": id, "x": x, "y": y, "radius": r, "tone": c, "fillTone": c, "fillStyle": "solid"})
            })
            .collect();
        shapes.extend(parts.iter().map(|(id, label, x, y, r, _)| {
            json!({"type": "text", "id": format!("label-{id}"), "x": x, "y": y + r + 14, "text": label, "textSize": "xs", "align": "center", "tone": "label"})
        }));
        json!({
            "type": "learning-canvas", "width": 600, "height": 380, "backgroundColor": "",
            "shapes": shapes, "interactive": true, "animate": false, "onShapeClick": "TAP_SHAPE"
        })
    }

    #[test]
    fn hotspot_scene_draws_five_solid_series_circles_and_five_centered_labels() {
        let scene = map(&hotspot_scene(), None);
        let cmds = &scene.packet.draw_commands;
        let circles: Vec<_> = cmds
            .iter()
            .filter_map(|c| match c {
                DrawCommand::Path {
                    segments,
                    fill: Some(f),
                    ..
                } if matches!(segments[0], PathSegment::Arc { .. }) => Some(*f),
                _ => None,
            })
            .collect();
        assert_eq!(circles.len(), 5);
        assert!(
            circles.iter().all(|f| f[3] == 1.0),
            "fillStyle solid: opaque"
        );
        let labels: Vec<_> = cmds
            .iter()
            .filter(|c| matches!(c, DrawCommand::Text { .. }))
            .collect();
        assert_eq!(labels.len(), 5);
        let DrawCommand::Clear { color } = cmds[0] else {
            panic!()
        };
        assert_eq!(
            color,
            canvas::theme_for(&hotspot_scene()).ground,
            "an empty backgroundColor leaves the theme ground"
        );
    }

    #[test]
    fn a_fillstyle_other_than_solid_changes_the_circle_alpha() {
        let mut props = hotspot_scene();
        props["shapes"][0]["fillStyle"] = json!("tint");
        let scene = map(&props, None);
        let alphas: Vec<f32> = scene
            .packet
            .draw_commands
            .iter()
            .filter_map(|c| match c {
                DrawCommand::Path { fill: Some(f), .. } => Some(f[3]),
                _ => None,
            })
            .collect();
        assert!(alphas[0] < 0.5 && alphas[1] == 1.0, "{alphas:?}");
    }

    #[test]
    fn tapping_a_part_emits_tap_shape_with_its_id_and_a_miss_emits_nothing() {
        let scene = map(&hotspot_scene(), None);
        let hit = scene.emits_at(300.0, 170.0);
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].event, "TAP_SHAPE");
        assert_eq!(
            hit[0].payload,
            json!({"id": "nucleus", "type": "circle", "index": 0})
        );
        let ribosome = scene.emits_at(200.0, 280.0);
        assert_eq!(ribosome[0].payload["id"], "ribosome");
        assert!(scene.emits_at(20.0, 20.0).is_empty());
    }

    #[test]
    fn the_viewport_fit_keeps_taps_on_their_shape() {
        let scene = map(&hotspot_scene(), Some((300.0, 190.0)));
        let hit = scene.emits_at(150.0, 85.0);
        assert_eq!(
            hit[0].payload["id"], "nucleus",
            "scene (300,170) lands at half scale"
        );
        assert!(
            scene.emits_at(300.0, 170.0).is_empty(),
            "control: unscaled point misses"
        );
    }

    #[test]
    fn readouts_and_traces_ride_on_the_authored_shapes() {
        let mut props = hotspot_scene();
        let base = map(&props, None).packet.draw_commands.len();
        props["readouts"] = json!([{"label": "score", "value": 3}]);
        props["traces"] = json!([{"series": [{"samples": [{"x": 0, "y": 0}, {"x": 1, "y": 1}]}]}]);
        assert!(map(&props, None).packet.draw_commands.len() > base + 4);
    }
}
