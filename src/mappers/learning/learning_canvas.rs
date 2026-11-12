//! `learning-canvas` — the declarative atom itself: the authored `shapes`
//! drawn as-is, plus its `readouts` and `traces`.

use super::canvas::{self, CanvasSpec};
use super::shape::parse_shapes;
use super::theme::css_of;
use crate::mappers::game_canvas_2d::{paint_drawables, CanvasSceneError};
use crate::mappers::scene::MappedScene;
use crate::projector::{Projection, Projector};
use serde_json::Value;

pub fn map(props: &Value, viewport: Option<(f32, f32)>) -> Result<MappedScene, CanvasSceneError> {
    let mut spec = CanvasSpec::from_props(
        "learning-canvas",
        props,
        600.0,
        400.0,
        parse_shapes(props.get("shapes")),
    );
    spec.particles = canvas::parse_particles(props.get("particles"))?;
    let theme = canvas::theme_for(props);
    let mut scene = canvas::render(&spec, &theme, viewport);
    let drawables = props.get("drawables").and_then(Value::as_array);
    if let (Some(drawables), Some(projector)) = (
        drawables.filter(|d| !d.is_empty()),
        props.get("projector").and_then(projector_of),
    ) {
        let (xf, _, _) = canvas::fit(spec.width, spec.height, viewport);
        let resolve = |c: &str| css_of(theme.mark_color(Some(c), "ink"));
        scene.packet.draw_commands.extend(paint_drawables(
            drawables,
            &projector,
            &resolve,
            &|x, y| xf.p(x, y),
            xf.s,
        )?);
    }
    Ok(scene)
}

fn projector_of(p: &Value) -> Option<Projector> {
    let tile_width = p.get("tileWidth")?.as_f64()? as f32;
    let floor_height = p.get("floorHeight")?.as_f64()? as f32;
    let diamond_top_y = p.get("diamondTopY")?.as_f64()? as f32;
    let square_grid = p.get("squareGrid")?.as_bool()?;
    let layout = if p.get("worldPixelDirect")?.as_bool()? {
        Projection::Free
    } else if square_grid {
        Projection::Flat
    } else {
        Projection::Isometric
    };
    Some(Projector {
        tile_width,
        floor_height,
        diamond_top_y,
        scale: 1.0,
        base_offset_x: 0.0,
        layout,
        square_grid,
        plot: None,
    })
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
        let scene = map(&hotspot_scene(), None).unwrap();
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
        let scene = map(&props, None).unwrap();
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
        let scene = map(&hotspot_scene(), None).unwrap();
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
        let scene = map(&hotspot_scene(), Some((300.0, 190.0))).unwrap();
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
        let base = map(&props, None).unwrap().packet.draw_commands.len();
        props["readouts"] = json!([{"label": "score", "value": 3}]);
        props["traces"] = json!([{"series": [{"samples": [{"x": 0, "y": 0}, {"x": 1, "y": 1}]}]}]);
        assert!(map(&props, None).unwrap().packet.draw_commands.len() > base + 4);
    }

    fn flat_projector() -> Value {
        json!({"tileWidth": 40, "floorHeight": 20, "diamondTopY": 0, "squareGrid": true, "worldPixelDirect": false})
    }

    fn cell_drawable() -> Value {
        json!({"type": "draw-shape", "shape": "cell", "position": {"x": 2, "y": 1}, "fill": "series-1"})
    }

    #[test]
    fn drawables_paint_through_the_projector_at_their_projected_cell() {
        let mut props = hotspot_scene();
        props["drawables"] = json!([cell_drawable()]);
        props["projector"] = flat_projector();
        let scene = map(&props, None).unwrap();
        let Some(DrawCommand::Poly { points, .. }) = scene.packet.draw_commands.last() else {
            panic!(
                "the drawable paints last: {:?}",
                scene.packet.draw_commands.last()
            )
        };
        assert_eq!((points[0].x, points[0].y), (80.0, 40.0));
        assert_eq!(points[1].x - points[0].x, 40.0);
        let fitted = map(&props, Some((300.0, 190.0))).unwrap();
        let Some(DrawCommand::Poly { points, .. }) = fitted.packet.draw_commands.last() else {
            panic!()
        };
        assert_eq!(
            (points[0].x, points[0].y),
            (40.0, 20.0),
            "half-scale viewport"
        );
    }

    #[test]
    fn control_drawables_without_a_projector_or_without_drawables_add_nothing() {
        let base = map(&hotspot_scene(), None)
            .unwrap()
            .packet
            .draw_commands
            .len();
        let mut no_projector = hotspot_scene();
        no_projector["drawables"] = json!([cell_drawable()]);
        assert_eq!(
            map(&no_projector, None).unwrap().packet.draw_commands.len(),
            base
        );
        let mut empty = hotspot_scene();
        empty["drawables"] = json!([]);
        empty["projector"] = flat_projector();
        assert_eq!(map(&empty, None).unwrap().packet.draw_commands.len(), base);
    }

    #[test]
    fn hovering_a_shape_emits_on_shape_hover_with_its_id_and_empty_space_emits_nothing() {
        let mut props = hotspot_scene();
        props.as_object_mut().unwrap().remove("onShapeClick");
        props["onShapeHover"] = json!("HOVER_SHAPE");
        let scene = map(&props, None).unwrap();
        let hit = scene.emits_at(300.0, 170.0);
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].event, "HOVER_SHAPE");
        assert_eq!(hit[0].arg, "payload");
        assert_eq!(
            hit[0].payload,
            json!({"id": "nucleus", "type": "circle", "index": 0})
        );
        assert!(scene.emits_at(20.0, 20.0).is_empty());
    }

    #[test]
    fn control_without_on_shape_hover_a_shape_emits_only_its_click() {
        let scene = map(&hotspot_scene(), None).unwrap();
        let hit = scene.emits_at(300.0, 170.0);
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].event, "TAP_SHAPE");
    }
}
