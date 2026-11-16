//! Animations a drawable declares: the packet animation the frame plays, and whether a scene has animations, reveals or fx still running.

use super::*;

/// A drawable `animation` the packet plays per frame (see [`orbital_canvas_base::animate::plays_on_packet`]) as a packet
/// animation over the drawable's commands. Progress follows the page scroll between `from` and `to` with
/// `clock: "scroll"`, else the paint clock over `durationMs` (looping unless `loop: false`). Keyframes carry `at`,
/// `opacity`, `scale`, `rotate`, `offsetX` and `offsetY` (scene units, as in a paint-clock drawable animation) and,
/// on a `path` shape, `d`: the outline at that offset, every `d` with the same number of subpaths.
pub(super) fn packet_animation(
    node: &Value,
    projector: &Projector,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
    zoom: f32,
    painted: &[DrawCommand],
) -> Result<Option<orbital_canvas_base::packet::Animation>, CanvasSceneError> {
    let Some(animation) = node
        .get("animation")
        .filter(|a| orbital_canvas_base::animate::plays_on_packet(a))
    else {
        return Ok(None);
    };
    let number = |key: &str| {
        animation
            .get(key)
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite())
            .map(|n| n as f32)
    };
    let scrolled = orbital_canvas_base::animate::is_scroll_clocked(animation);
    let (clock, duration_ms, iterations) = if scrolled {
        let need = |key: &str| {
            number(key).ok_or_else(|| {
                invalid(
                    &format!("animation.{key}"),
                    "a scroll-clocked animation needs a finite number",
                )
            })
        };
        (
            orbital_canvas_base::packet::AnimationClock::Scroll {
                from: need("from")?,
                to: need("to")?,
            },
            1.0,
            None,
        )
    } else {
        if animation.get("startMs").is_some_and(|v| !v.is_null()) {
            return Err(invalid("animation.startMs", "a morphing animation on the paint clock plays from the scene's first frame and takes no startMs"));
        }
        let duration = number("durationMs").filter(|d| *d > 0.0).ok_or_else(|| {
            invalid(
                "animation.durationMs",
                "a morphing animation needs a duration above zero",
            )
        })?;
        let looping = animation.get("loop").and_then(Value::as_bool) != Some(false);
        (
            orbital_canvas_base::packet::AnimationClock::Time,
            duration,
            (!looping).then_some(1),
        )
    };
    let frames = animation
        .get("keyframes")
        .and_then(Value::as_array)
        .filter(|f| !f.is_empty())
        .ok_or_else(|| {
            invalid(
                "animation.keyframes",
                "a packet-played animation needs keyframes",
            )
        })?;
    let tw = projector.tile_width;
    let (zero_x, zero_y) = to_screen(0.0, 0.0);
    let at = |frame: &Value| {
        frame
            .get("at")
            .and_then(Value::as_f64)
            .map(|n| n as f32)
            .ok_or_else(|| invalid("animation.keyframes.at", "each keyframe needs `at`"))
    };
    let keyframes = frames
        .iter()
        .map(|frame| {
            let get = |key: &str| frame.get(key).and_then(Value::as_f64).map(|n| n as f32);
            Ok(orbital_canvas_base::packet::Keyframe {
                offset: at(frame)?,
                scale: get("scale"),
                rotate: get("rotate"),
                opacity: get("opacity"),
                offset_x: get("offsetX").map(|x| to_screen(x * tw, 0.0).0 - zero_x),
                offset_y: get("offsetY").map(|y| to_screen(0.0, y * tw).1 - zero_y),
                ..orbital_canvas_base::packet::Keyframe::default()
            })
        })
        .collect::<Result<Vec<_>, CanvasSceneError>>()?;
    let mut morph = Vec::new();
    for frame in frames {
        let Some(d) = frame.get("d") else {
            continue;
        };
        if node.get("shape").and_then(Value::as_str) != Some("path") {
            return Err(invalid(
                "animation.keyframes.d",
                "only a `path` shape morphs",
            ));
        }
        let d = d
            .as_str()
            .ok_or_else(|| invalid("animation.keyframes.d", "an outline is SVG path data"))?;
        let segments = parse_path_data(d, path_xf(node, projector, to_screen, zoom))
            .ok_or_else(|| invalid("animation.keyframes.d", format!("malformed SVG path `{d}`")))?;
        let mut contours = orbital_canvas_base::morph::contours(&segments);
        if let Some(previous) = morph
            .last()
            .map(|f: &orbital_canvas_base::packet::MorphFrame| f.contours.clone())
        {
            orbital_canvas_base::morph::align_to(&previous, &mut contours);
        }
        if let Some(first) = morph
            .first()
            .map(|f: &orbital_canvas_base::packet::MorphFrame| f.contours.len())
        {
            if contours.len() != first {
                return Err(invalid(
                    "animation.keyframes.d",
                    format!("every outline of a morph has the same number of subpaths ({first}), `{d}` has {}", contours.len()),
                ));
            }
        }
        morph.push(orbital_canvas_base::packet::MorphFrame {
            offset: at(frame)?,
            contours,
        });
    }
    let origin = commands_bounds(painted)
        .map(|r| Vec2 {
            x: r.x + r.w / 2.0,
            y: r.y + r.h / 2.0,
        })
        .unwrap_or(Vec2 {
            x: zero_x,
            y: zero_y,
        });
    Ok(Some(orbital_canvas_base::packet::Animation {
        first: 0,
        end: 0,
        duration_ms,
        delay_ms: 0.0,
        iterations,
        easing: orbital_canvas_base::packet::Easing::Linear,
        origin,
        base_rotation: 0.0,
        keyframes,
        clock,
        morph,
    }))
}

pub(super) fn has_start(animation: &Value) -> bool {
    matches!(
        orbital_canvas_base::animate::start_ms(animation),
        Ok(Some(_))
    )
}

pub(super) fn started_animations(node: &Value, f: &mut dyn FnMut(&Value)) {
    if matches!(
        node.get("type").and_then(Value::as_str),
        Some("draw-shape" | "draw-group" | "draw-text")
    ) {
        if let Some(animation) = node.get("animation").filter(|a| has_start(a)) {
            let plays = animation
                .get("durationMs")
                .and_then(Value::as_f64)
                .is_some_and(|v| v > 0.0)
                && animation
                    .get("keyframes")
                    .and_then(Value::as_array)
                    .is_some_and(|v| !v.is_empty());
            if plays {
                f(animation);
            }
        }
    }
    for key in ["props", "drawables", "children", "items"] {
        match node.get(key) {
            Some(Value::Array(items)) => items.iter().for_each(|i| started_animations(i, f)),
            Some(child @ Value::Object(_)) => started_animations(child, f),
            _ => {}
        }
    }
}

/// Whether any drawable under `node` declares an animation with a `startMs`.
pub fn has_started_animation(node: &Value) -> bool {
    let mut found = false;
    started_animations(node, &mut |_| found = true);
    found
}

/// Whether any `startMs` animation under `node` has yet to start, still plays or loops at the clock.
pub fn started_animation_live(node: &Value, epoch_now_ms: f64) -> bool {
    let mut live = false;
    started_animations(node, &mut |animation| {
        let (Ok(Some(start)), Some(duration)) = (
            orbital_canvas_base::animate::start_ms(animation),
            animation.get("durationMs").and_then(Value::as_f64),
        ) else {
            return;
        };
        let looping = animation.get("loop").and_then(Value::as_bool) != Some(false);
        live |= looping || epoch_now_ms < start + duration;
    });
    live
}

/// A live world FX layer needs the host's paint clock even when no authored
/// packet keyframe animation is present.
pub fn has_active_fx(node: &Value) -> bool {
    let animated_sprite = |sprite: &Value| {
        sprite.get("animation").and_then(Value::as_str).is_some()
            && sprite
                .get("asset")
                .and_then(|asset| asset.get("atlas"))
                .and_then(Value::as_str)
                .is_some()
    };
    if node.get("type").and_then(Value::as_str) == Some("draw-sprite") && animated_sprite(node) {
        return true;
    }
    if node.get("type").and_then(Value::as_str) == Some("draw-sprite-layer")
        && items(node).iter().any(animated_sprite)
    {
        return true;
    }
    if node.get("type").and_then(Value::as_str) == Some("draw-fx-layer")
        && node
            .get("items")
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty())
    {
        return true;
    }
    if matches!(
        node.get("type").and_then(Value::as_str),
        Some("draw-shape" | "draw-group" | "draw-text")
    ) && node.get("animation").is_some_and(|animation| {
        !has_start(animation)
            && animation
                .get("durationMs")
                .and_then(Value::as_f64)
                .is_some_and(|v| v > 0.0)
            && animation
                .get("keyframes")
                .and_then(Value::as_array)
                .is_some_and(|v| !v.is_empty())
    }) {
        return true;
    }
    if node
        .pointer("/material/anchor")
        .is_some_and(|anchor| anchor.as_f64().is_some())
    {
        return true;
    }
    if node.get("type").and_then(Value::as_str) == Some("draw-text")
        && node
            .get("caret")
            .and_then(Value::as_f64)
            .is_some_and(|caret| caret >= 0.0)
    {
        return true;
    }
    ["props", "drawables", "children", "items"]
        .into_iter()
        .filter_map(|key| node.get(key))
        .any(|child| match child {
            Value::Array(items) => items.iter().any(has_active_fx),
            Value::Object(_) => has_active_fx(child),
            _ => false,
        })
}
