//! `draw-scene3d`: a lit 3D scene drawn into one rect of a 2D canvas. The scene is mapped by the 3D canvas's own
//! mapper, painted into a render target (a [`DrawCommand::Target`]) sized to the rect, and shown by a sprite over
//! the rect, so it keeps its place in the paint order and moves with the page like any unpinned command.

use super::*;
use crate::canvas_scene3d::{
    scene_from_canvas_node_at_epoch, scene_from_canvas_node_themed, SceneTheme,
};

/// Texture pixels per drawable pixel when the drawable declares no `pixelRatio`.
const DEFAULT_PIXEL_RATIO: f64 = 2.0;

/// The scene clock wraps so a shader material's `f32` seconds keep their precision on the epoch clock.
const SHADER_CLOCK_PERIOD_MS: f64 = 1_000_000.0;

pub(super) fn paint_scene3d_gathering(
    node: &Value,
    projector: &Projector,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
    zoom: f32,
    epoch_now_ms: f64,
    gathered: &mut Gathered,
    out: &mut Vec<DrawCommand>,
) -> Result<(), CanvasSceneError> {
    let start = out.len();
    let anonymous = gathered.scene3d_count;
    paint_scene3d(
        node,
        projector,
        to_screen,
        zoom,
        epoch_now_ms,
        anonymous,
        out,
    )?;
    if out.len() == start {
        return Ok(());
    }
    if node.get("id").and_then(Value::as_str).is_none() {
        gathered.scene3d_count += 1;
    }
    if let Some(id) = hit_id(node) {
        let placed = out[start..].iter().find_map(|command| match command {
            DrawCommand::Sprite { dst, rotation, .. } => Some((*dst, *rotation)),
            _ => None,
        });
        if let Some((rect, rotation)) = placed {
            gathered.regions.push(Region {
                id: id.to_string(),
                rect,
                rotation,
                wheel: takes_wheel(node),
                fixed: false,
                drag: takes_drag(node),
            });
        }
    }
    Ok(())
}

fn paint_scene3d(
    node: &Value,
    projector: &Projector,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
    zoom: f32,
    epoch_now_ms: f64,
    anonymous: usize,
    out: &mut Vec<DrawCommand>,
) -> Result<(), CanvasSceneError> {
    let Some(position) = node.get("position").and_then(parse_scene_pos) else {
        gap_once("scene3d-position", "draw-scene3d without position");
        return Ok(());
    };
    for key in ["width", "height"] {
        if !node
            .get(key)
            .and_then(Value::as_f64)
            .is_some_and(|v| v.is_finite() && v > 0.0)
        {
            return Err(invalid(key, "a 3D scene needs its own size above zero"));
        }
    }
    let Some(Value::Object(scene)) = node.get("scene") else {
        return Err(invalid(
            "scene",
            "a 3D scene: drawables, lighting, camera and backgroundColor, as a 3D canvas reads them",
        ));
    };
    let ratio = match node.get("pixelRatio").filter(|r| !r.is_null()) {
        None => DEFAULT_PIXEL_RATIO,
        Some(r) => r
            .as_f64()
            .filter(|r| r.is_finite() && *r > 0.0)
            .ok_or_else(|| invalid("pixelRatio", "a number above zero"))?,
    };

    let animation = node
        .get("animation")
        .filter(|a| orbital_canvas_base::animate::plays_at(a, epoch_now_ms));
    let track = |key: &str| {
        animation
            .and_then(|a| orbital_canvas_base::animate::drawable_track(a, epoch_now_ms, key))
            .and_then(|value| value.as_f64())
            .map(|value| value as f32)
    };

    let rest = sprite_dst(node, position, projector, to_screen, zoom, Rect::default());
    let (zero_x, zero_y) = to_screen(0.0, 0.0);
    let tw = projector.tile_width;
    let shift_x = track("offsetX").map_or(0.0, |x| to_screen(x * tw, 0.0).0 - zero_x);
    let shift_y = track("offsetY").map_or(0.0, |y| to_screen(0.0, y * tw).1 - zero_y);
    let scale = track("scale").unwrap_or(1.0);
    let (w, h) = (rest.w * scale, rest.h * scale);
    let dst = Rect {
        x: rest.x + rest.w / 2.0 - w / 2.0 + shift_x,
        y: rest.y + rest.h / 2.0 - h / 2.0 + shift_y,
        w,
        h,
    };

    let mut canvas = scene.clone();
    canvas
        .entry("mode")
        .or_insert_with(|| Value::String("3d".into()));
    canvas.entry("showGrid").or_insert(Value::Bool(false));
    let canvas = Value::Object(canvas);
    let theme = SceneTheme::default();
    let mut scene3d = if epoch_now_ms > 0.0 {
        scene_from_canvas_node_at_epoch(
            &canvas,
            rest.w,
            rest.h,
            &theme,
            epoch_now_ms,
            epoch_now_ms,
        )?
    } else {
        scene_from_canvas_node_themed(&canvas, rest.w, rest.h, &theme)?
    };
    if scene3d.time != 0.0 {
        scene3d.time = (epoch_now_ms.rem_euclid(SHADER_CLOCK_PERIOD_MS) / 1000.0) as f32;
    }

    let id = match node.get("id").and_then(Value::as_str) {
        Some(id) => format!("scene3d:{id}"),
        None => format!("scene3d:#{anonymous}"),
    };
    let pixels = |extent: f32| ((f64::from(extent) * ratio).ceil() as u32).max(1);
    out.push(DrawCommand::Target {
        id: id.clone(),
        width: pixels(rest.w),
        height: pixels(rest.h),
        live: false,
        commands: vec![DrawCommand::Scene3D {
            scene: Box::new(scene3d),
        }],
    });
    out.push(DrawCommand::Sprite {
        texture: format!("{}{id}", orbital_canvas_base::targets::TARGET_PREFIX),
        src: Rect::default(),
        dst,
        rotation: get_f32(node, "rotation", 0.0) + track("rotateZ").unwrap_or(0.0),
        opacity: get_f32(node, "opacity", 1.0) * track("opacity").unwrap_or(1.0),
        flip_x: false,
        flip_y: false,
        tint: None,
    });
    Ok(())
}

/// Whether a drawable's `scene` plays by itself: a mesh or group `animation`, a shader material, or an fx layer.
pub(super) fn scene_moves(scene: &Value) -> bool {
    match scene {
        Value::Array(items) => items.iter().any(scene_moves),
        Value::Object(fields) => {
            fields.get("animation").is_some_and(Value::is_object)
                || fields.get("kind").and_then(Value::as_str) == Some("shader")
                || fields.get("type").and_then(Value::as_str) == Some("draw-fx-layer")
                || fields.values().any(scene_moves)
        }
        _ => false,
    }
}
