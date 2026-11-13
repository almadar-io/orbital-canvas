//! Prop parsing for backdrops (rasm floor contract 14.6) and render targets (14.7): `draw-shape.backdrop`,
//! `draw-group.target`, `draw-sprite.texture: "target:<id>"` and `material.targets`.

use super::game_canvas_2d::{parse_css_color, CanvasSceneError};
use crate::packet::{DrawCommand, SdfBackdrop, Vec2};
use serde_json::Value;

fn invalid(field: &str, detail: impl Into<String>) -> CanvasSceneError {
    CanvasSceneError::InvalidProp {
        field: field.into(),
        detail: detail.into(),
    }
}

/// Whether a draw-shape declares a backdrop.
pub(crate) fn has_backdrop(node: &Value) -> bool {
    node.get("backdrop").is_some_and(|b| !b.is_null())
}

/// Wrap the SDF shape the mapper just pushed in its backdrop, `blur` scaled to canvas pixels by `zoom`.
pub(crate) fn wrap_backdrop(
    node: &Value,
    zoom: f32,
    out: &mut Vec<DrawCommand>,
) -> Result<(), CanvasSceneError> {
    let Some(spec) = node.get("backdrop").filter(|b| !b.is_null()) else {
        return Ok(());
    };
    if !spec.is_object() {
        return Err(invalid(
            "backdrop",
            "needs `blur` and optionally `tint` and `saturate`",
        ));
    }
    let blur = spec
        .get("blur")
        .and_then(Value::as_f64)
        .filter(|b| b.is_finite() && *b >= 0.0)
        .ok_or_else(|| invalid("backdrop.blur", "a finite number of px, zero or more"))?;
    let tint = match spec.get("tint").filter(|t| !t.is_null()) {
        None => None,
        Some(tint) => Some(
            tint.as_str()
                .and_then(parse_css_color)
                .ok_or_else(|| invalid("backdrop.tint", "a css colour"))?,
        ),
    };
    let saturate = match spec.get("saturate").filter(|s| !s.is_null()) {
        None => 1.0,
        Some(s) => s
            .as_f64()
            .filter(|s| s.is_finite() && *s >= 0.0)
            .ok_or_else(|| invalid("backdrop.saturate", "a finite number, zero or more"))?
            as f32,
    };
    let Some(shape @ DrawCommand::Sdf { .. }) = out.pop() else {
        return Err(invalid(
            "backdrop",
            "a backdrop shape paints through the SDF pass",
        ));
    };
    out.push(DrawCommand::SdfBackdrop {
        shape: Box::new(shape),
        backdrop: SdfBackdrop {
            blur: blur as f32 * zoom,
            tint,
            saturate,
        },
    });
    Ok(())
}

/// The ids in a material's `targets` list.
pub(crate) fn material_targets(spec: &Value) -> Result<Vec<String>, CanvasSceneError> {
    let Some(list) = spec.get("targets").filter(|t| !t.is_null()) else {
        return Ok(Vec::new());
    };
    let list = list
        .as_array()
        .ok_or_else(|| invalid("material.targets", "a list of render target ids"))?;
    list.iter()
        .map(|id| {
            id.as_str()
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| invalid("material.targets", "entries are non-empty target ids"))
        })
        .collect()
}

/// Turn what a `draw-group` with `target` painted (`out[start..]`, canvas pixels) into one [`DrawCommand::Target`]
/// whose children sit relative to the group's local origin `zero`.
pub(crate) fn collect_target(
    spec: &Value,
    zero: (f32, f32),
    out: &mut Vec<DrawCommand>,
    start: usize,
) -> Result<(), CanvasSceneError> {
    let id = spec
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| invalid("target", "needs a non-empty `id`"))?;
    let size = |key: &str| -> Result<u32, CanvasSceneError> {
        spec.get(key)
            .and_then(Value::as_f64)
            .filter(|v| v.fract() == 0.0 && *v >= 1.0 && *v <= f64::from(u32::MAX))
            .map(|v| v as u32)
            .ok_or_else(|| {
                invalid(
                    "target",
                    format!("`{key}` is a whole number of px above zero"),
                )
            })
    };
    let (width, height) = (size("width")?, size("height")?);
    let live = spec.get("live").and_then(Value::as_bool).unwrap_or(false);
    let mut commands: Vec<DrawCommand> = out.drain(start..).collect();
    crate::animate::transform_group_commands(
        &mut commands,
        Vec2::default(),
        Vec2 {
            x: -zero.0,
            y: -zero.1,
        },
        1.0,
        0.0,
        1.0,
    );
    out.push(DrawCommand::Target {
        id: id.to_owned(),
        width,
        height,
        live,
        commands,
    });
    Ok(())
}

/// A `draw-sprite` `texture` as the packet's texture string; it must be `target:<id>` with an id.
pub(crate) fn target_texture(texture: &Value) -> Result<String, CanvasSceneError> {
    let text = texture
        .as_str()
        .ok_or_else(|| invalid("texture", "`target:<id>`"))?;
    match text.strip_prefix(crate::render_target::TARGET_PREFIX) {
        Some(id) if !id.is_empty() => Ok(text.to_owned()),
        _ => Err(invalid("texture", "`target:<id>` with a render target id")),
    }
}

/// A target sprite has no source size to default to, so `width` and `height` are required.
pub(crate) fn require_sprite_size(node: &Value) -> Result<(), CanvasSceneError> {
    for key in ["width", "height"] {
        if node.get(key).and_then(Value::as_f64).is_none() {
            return Err(invalid(
                key,
                "a target sprite needs its own `width` and `height`",
            ));
        }
    }
    Ok(())
}

/// Refuse a scene whose targets read each other in a cycle.
pub(crate) fn check_target_cycles(commands: &[DrawCommand]) -> Result<(), CanvasSceneError> {
    match crate::render_target::find_cycle(commands) {
        Some(id) => Err(invalid(
            "target",
            format!("`{id}` is read by its own children, directly or through other targets"),
        )),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::super::game_canvas_2d::{from_props, from_props_mapped};
    use super::*;
    use crate::packet::GameRenderPacket;
    use serde_json::json;

    const WGSL: &str = "fn shade(px: Pixel) -> vec4<f32> { return px.color; }";

    fn scene(drawables: Value) -> Value {
        json!({"width": 200, "height": 100, "scale": 1.0, "projection": "free",
            "bgColor": "#00000000", "drawables": drawables})
    }

    fn shape(extra: Value) -> Value {
        let mut node = json!({"type": "draw-shape", "shape": "rect",
            "position": {"x": 10, "y": 20}, "width": 64, "height": 32, "fill": "#ff000080"});
        for (key, value) in extra.as_object().unwrap() {
            node[key] = value.clone();
        }
        node
    }

    fn map(drawables: Value) -> Result<GameRenderPacket, CanvasSceneError> {
        from_props(&scene(drawables))
    }

    fn field(error: CanvasSceneError) -> String {
        match error {
            CanvasSceneError::InvalidProp { field, .. } => field,
            other => panic!("not an invalid prop: {other:?}"),
        }
    }

    #[test]
    fn a_backdrop_shape_wraps_its_sdf_with_blur_tint_and_saturate_and_a_plain_one_does_not() {
        let packet = map(json!([shape(
            json!({"backdrop": {"blur": 12, "tint": "#ffffff40", "saturate": 1.5}})
        )]))
        .unwrap();
        let wrapped: Vec<_> = packet
            .draw_commands
            .iter()
            .filter_map(|c| match c {
                DrawCommand::SdfBackdrop { shape, backdrop } => Some((shape, backdrop)),
                _ => None,
            })
            .collect();
        let [(inner, backdrop)] = wrapped.as_slice() else {
            panic!("one backdrop shape: {:?}", packet.draw_commands)
        };
        assert!(matches!(inner.as_ref(), DrawCommand::Sdf { .. }));
        assert_eq!(backdrop.blur, 12.0);
        assert_eq!(backdrop.saturate, 1.5);
        assert!(backdrop.tint.is_some_and(|t| t[3] > 0.2 && t[3] < 0.3));
        let plain = map(json!([shape(json!({"corners": {"radius": [4, 4, 4, 4]}}))])).unwrap();
        assert!(
            !plain
                .draw_commands
                .iter()
                .any(|c| matches!(c, DrawCommand::SdfBackdrop { .. })),
            "control: no backdrop, no wrapper"
        );
        let bare = map(json!([shape(json!({"backdrop": {"blur": 0}}))])).unwrap();
        let backdrop = bare.draw_commands.iter().find_map(|c| match c {
            DrawCommand::SdfBackdrop { backdrop, .. } => Some(*backdrop),
            _ => None,
        });
        assert_eq!(
            backdrop,
            Some(SdfBackdrop {
                blur: 0.0,
                tint: None,
                saturate: 1.0
            }),
            "defaults: no tint, saturate 1"
        );
    }

    #[test]
    fn a_backdrop_alone_with_no_fill_still_paints_and_the_region_survives() {
        let node =
            shape(json!({"id": "glass", "hit": true, "fill": null, "backdrop": {"blur": 6}}));
        let (packet, gathered) = from_props_mapped(&scene(json!([node])), 0.0).unwrap();
        assert!(packet
            .draw_commands
            .iter()
            .any(|c| matches!(c, DrawCommand::SdfBackdrop { .. })));
        assert_eq!(gathered.regions.len(), 1);
        let none = shape(json!({"fill": null}));
        let quiet = map(json!([none])).unwrap();
        assert!(
            !quiet
                .draw_commands
                .iter()
                .any(|c| matches!(c, DrawCommand::SdfBackdrop { .. } | DrawCommand::Sdf { .. })),
            "control: no fill, stroke, material or backdrop paints nothing"
        );
    }

    #[test]
    fn a_malformed_backdrop_is_a_scene_error_naming_the_field() {
        for (backdrop, expected) in [
            (json!({}), "backdrop.blur"),
            (json!({"blur": -1}), "backdrop.blur"),
            (json!({"blur": "wide"}), "backdrop.blur"),
            (json!({"blur": 4, "saturate": -1}), "backdrop.saturate"),
            (json!({"blur": 4, "tint": "nope"}), "backdrop.tint"),
            (json!(5), "backdrop"),
        ] {
            let error = map(json!([shape(json!({"backdrop": backdrop}))])).unwrap_err();
            assert_eq!(field(error), expected, "{backdrop}");
        }
        assert!(
            map(json!([shape(
                json!({"backdrop": {"blur": 4, "tint": "#fff", "saturate": 0}})
            )]))
            .is_ok(),
            "control: a good one"
        );
    }

    #[test]
    fn a_backdrop_shape_cannot_join_a_smooth_union_and_zoom_scales_its_blur() {
        let group = json!({"type": "draw-group", "position": {"x": 0, "y": 0}, "smoothUnion": 4, "items": [
            shape(json!({"backdrop": {"blur": 4}})), shape(json!({}))]});
        assert_eq!(field(map(json!([group])).unwrap_err()), "smoothUnion");
        let mut zoomed = scene(json!([shape(json!({"backdrop": {"blur": 10}}))]));
        zoomed["scale"] = json!(2.0);
        let packet = from_props(&zoomed).unwrap();
        let blur = packet.draw_commands.iter().find_map(|c| match c {
            DrawCommand::SdfBackdrop { backdrop, .. } => Some(backdrop.blur),
            _ => None,
        });
        assert_eq!(blur, Some(20.0));
    }

    #[test]
    fn a_group_transform_moves_a_backdrop_shape_and_scales_its_blur() {
        let group = json!({"type": "draw-group", "position": {"x": 5, "y": 7}, "scale": 2.0, "items": [
            shape(json!({"backdrop": {"blur": 4}}))]});
        let packet = map(json!([group])).unwrap();
        let moved = packet.draw_commands.iter().find_map(|c| match c {
            DrawCommand::SdfBackdrop { shape, backdrop } => match shape.as_ref() {
                DrawCommand::Sdf { rect, .. } => Some((rect.w, backdrop.blur)),
                _ => None,
            },
            _ => None,
        });
        assert_eq!(
            moved,
            Some((128.0, 8.0)),
            "size and blur ride the group scale"
        );
    }

    fn material(extra: Value) -> Value {
        let mut material = json!({"wgsl": WGSL, "params": [], "live": false});
        for (key, value) in extra.as_object().unwrap() {
            material[key] = value.clone();
        }
        shape(json!({"material": material}))
    }

    #[test]
    fn material_targets_reach_the_command_and_a_bad_list_names_the_field() {
        let packet = map(json!([material(json!({"targets": ["a", "b"]}))])).unwrap();
        let targets = packet.draw_commands.iter().find_map(|c| match c {
            DrawCommand::Sdf {
                material: Some(m), ..
            } => Some(m.targets.clone()),
            _ => None,
        });
        assert_eq!(targets, Some(vec!["a".to_owned(), "b".to_owned()]));
        let none = map(json!([material(json!({}))])).unwrap();
        let targets = none.draw_commands.iter().find_map(|c| match c {
            DrawCommand::Sdf {
                material: Some(m), ..
            } => Some(m.targets.clone()),
            _ => None,
        });
        assert_eq!(targets, Some(Vec::new()), "control: no list, no targets");
        for bad in [json!("a"), json!([1]), json!([""])] {
            let error = map(json!([material(json!({"targets": bad}))])).unwrap_err();
            assert_eq!(field(error), "material.targets", "{bad}");
        }
    }

    fn target_group(id: &str, extra: Value) -> Value {
        let mut group = json!({"type": "draw-group", "position": {"x": 30, "y": 40},
            "target": {"id": id, "width": 32, "height": 16}, "items": [
            {"type": "draw-shape", "shape": "rect", "position": {"x": 2, "y": 3}, "width": 10, "height": 6,
             "fill": "#00ff00", "id": "inner", "hit": true}]});
        for (key, value) in extra.as_object().unwrap() {
            group[key] = value.clone();
        }
        group
    }

    #[test]
    fn a_target_group_paints_offscreen_children_from_its_local_origin_and_nothing_in_the_frame() {
        let (packet, gathered) =
            from_props_mapped(&scene(json!([target_group("t", json!({}))])), 0.0).unwrap();
        let targets: Vec<_> = packet
            .draw_commands
            .iter()
            .filter_map(|c| match c {
                DrawCommand::Target {
                    id,
                    width,
                    height,
                    live,
                    commands,
                } => Some((id.clone(), *width, *height, *live, commands)),
                _ => None,
            })
            .collect();
        let [(id, w, h, live, commands)] = targets.as_slice() else {
            panic!("one target")
        };
        assert_eq!((id.as_str(), *w, *h, *live), ("t", 32, 16, false));
        let rect = commands.iter().find_map(|c| match c {
            DrawCommand::Sdf { rect, .. } | DrawCommand::Quad { rect, .. } => Some(*rect),
            _ => None,
        });
        let rect = rect.expect("the child is a rect mark");
        assert_eq!(
            (rect.x, rect.y, rect.w, rect.h),
            (2.0, 3.0, 10.0, 6.0),
            "group-local, not at the group's position"
        );
        assert!(
            !packet.draw_commands.iter().any(|c| matches!(
                c,
                DrawCommand::Quad { .. } | DrawCommand::Sdf { .. } | DrawCommand::Path { .. }
            )),
            "nothing paints in the frame"
        );
        assert!(
            gathered.regions.is_empty(),
            "an offscreen child is no pointer region"
        );
        let plain = map(json!([target_group("t", json!({"target": null}))])).unwrap();
        assert!(
            !plain
                .draw_commands
                .iter()
                .any(|c| matches!(c, DrawCommand::Target { .. })),
            "control: no target, no Target command"
        );
        let (shown, shown_regions) = from_props_mapped(
            &scene(json!([target_group("t", json!({"target": null}))])),
            0.0,
        )
        .unwrap();
        assert!(
            shown.draw_commands.len() > 1 && shown_regions.regions.len() == 1,
            "control: the same group without a target paints and hits"
        );
    }

    #[test]
    fn a_malformed_target_is_a_scene_error_naming_target() {
        for target in [
            json!({"width": 8, "height": 8}),
            json!({"id": "", "width": 8, "height": 8}),
            json!({"id": "a", "width": 0, "height": 8}),
            json!({"id": "a", "width": 8, "height": -2}),
            json!({"id": "a", "width": 8.5, "height": 8}),
            json!({"id": "a", "width": 8}),
        ] {
            let error = map(json!([target_group("a", json!({"target": target}))])).unwrap_err();
            assert_eq!(field(error), "target", "{target}");
        }
        let live = map(json!([target_group(
            "a",
            json!({"target": {"id": "a", "width": 4, "height": 4, "live": true}})
        )]))
        .unwrap();
        assert!(live
            .draw_commands
            .iter()
            .any(|c| matches!(c, DrawCommand::Target { live: true, .. })));
    }

    fn target_sprite(texture: &str, extra: Value) -> Value {
        let mut sprite = json!({"type": "draw-sprite", "position": {"x": 4, "y": 6}, "texture": texture,
            "width": 32, "height": 16});
        for (key, value) in extra.as_object().unwrap() {
            sprite[key] = value.clone();
        }
        sprite
    }

    #[test]
    fn a_target_sprite_maps_to_a_sprite_of_that_texture_and_a_bad_one_is_an_error() {
        let packet = map(json!([target_sprite(
            "target:t",
            json!({"opacity": 0.5, "frame": {"x": 1, "y": 2, "w": 3, "h": 4}})
        )]))
        .unwrap();
        let sprite = packet.draw_commands.iter().find_map(|c| match c {
            DrawCommand::Sprite {
                texture,
                src,
                dst,
                opacity,
                ..
            } => Some((texture.clone(), *src, *dst, *opacity)),
            _ => None,
        });
        let (texture, src, dst, opacity) = sprite.expect("a sprite");
        assert_eq!(texture, "target:t");
        assert_eq!((src.x, src.w), (1.0, 3.0));
        assert_eq!((dst.x, dst.y, dst.w, dst.h), (4.0, 6.0, 32.0, 16.0));
        assert_eq!(opacity, 0.5);
        assert_eq!(
            field(map(json!([target_sprite("target:", json!({}))])).unwrap_err()),
            "texture"
        );
        assert_eq!(
            field(map(json!([target_sprite("hero.png", json!({}))])).unwrap_err()),
            "texture"
        );
        let mut no_size = target_sprite("target:t", json!({}));
        no_size.as_object_mut().unwrap().remove("height");
        assert_eq!(field(map(json!([no_size])).unwrap_err()), "height");
    }

    #[test]
    fn targets_that_read_each_other_in_a_cycle_are_a_scene_error_and_a_chain_is_not() {
        let reads = |id: &str, other: &str| {
            let mut group =
                target_group(id, json!({"target": {"id": id, "width": 8, "height": 8}}));
            group["items"] = json!([target_sprite(&format!("target:{other}"), json!({}))]);
            group
        };
        assert_eq!(
            field(map(json!([reads("a", "a")])).unwrap_err()),
            "target",
            "self"
        );
        assert_eq!(
            field(map(json!([reads("a", "b"), reads("b", "a")])).unwrap_err()),
            "target",
            "two"
        );
        let mut chain = target_group("b", json!({"target": {"id": "b", "width": 8, "height": 8}}));
        chain["items"] = json!([]);
        assert!(
            map(json!([reads("a", "b"), chain.clone()])).is_ok(),
            "control: reader before writer"
        );
        assert!(
            map(json!([chain, reads("a", "b")])).is_ok(),
            "control: writer before reader"
        );
    }

    #[test]
    fn a_live_target_keeps_the_scene_drawing_and_a_still_one_does_not() {
        let live = scene(json!([target_group(
            "a",
            json!({"target": {"id": "a", "width": 4, "height": 4, "live": true}})
        )]));
        assert!(crate::spring::has_live_material(&live));
        let still = scene(json!([target_group("a", json!({}))]));
        assert!(
            !crate::spring::has_live_material(&still),
            "control: a still target"
        );
    }
}
