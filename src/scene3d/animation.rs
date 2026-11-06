//! The keyframe `animation` a `draw-mesh` or `draw-group` declares, sampled as the three host's `applyMeshAnimation`
//! samples it: one cycle of `durationMs`, looping unless `loop` is false (then held at the end), each track linear
//! between the keyframes that define it, holding its first value before them and its last after, colours stepped.
//! The time is the canvas clock (`clock.elapsedTime`), not a per-mesh one: see `scene_from_canvas_node_at`.

use crate::animate::drawable_track;
use serde_json::{Map, Value};

pub const TRANSFORM_TRACKS: [&str; 7] = [
    "offsetX", "offsetY", "offsetZ", "rotateX", "rotateY", "rotateZ", "scale",
];
pub const MATERIAL_TRACKS: [&str; 4] = ["opacity", "emissiveIntensity", "color", "emissive"];
const COLOR_TRACKS: [&str; 2] = ["color", "emissive"];

#[derive(Debug, Clone, PartialEq)]
pub struct AnimationError {
    pub field: String,
    pub why: String,
}

impl std::fmt::Display for AnimationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.field, self.why)
    }
}

impl std::error::Error for AnimationError {}

fn error(field: String, why: impl Into<String>) -> AnimationError {
    AnimationError {
        field,
        why: why.into(),
    }
}

/// A declared animation the three host plays: `durationMs` above zero and at least one keyframe.
#[derive(Debug, Clone, PartialEq)]
pub struct Animation {
    declared: Value,
}

/// The animation at one time: transform deltas (`offset` in cells on scene axes, `rotate` in radians added to the
/// Euler angles, `scale` a multiplier) and the material tracks that override, `None` where no keyframe defines one.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimatedState {
    pub offset: [f64; 3],
    pub rotate: [f64; 3],
    pub scale: f64,
    pub opacity: Option<f64>,
    pub emissive_intensity: Option<f64>,
    pub color: Option<String>,
    pub emissive: Option<String>,
}

impl Animation {
    pub fn parse(value: &Value, path: &str) -> Result<Self, AnimationError> {
        let fields = value
            .as_object()
            .ok_or_else(|| error(path.to_owned(), "must be an object"))?;
        for key in fields.keys() {
            if !matches!(key.as_str(), "durationMs" | "loop" | "keyframes") {
                return Err(error(format!("{path}.{key}"), "not a declared prop"));
            }
        }
        let duration = fields
            .get("durationMs")
            .filter(|v| !v.is_null())
            .ok_or_else(|| error(format!("{path}.durationMs"), "is required"))?;
        let duration = finite(duration, format!("{path}.durationMs"))?;
        if duration <= 0.0 {
            return Err(error(
                format!("{path}.durationMs"),
                format!("{duration} must be above zero (the three host does not play it)"),
            ));
        }
        if let Some(looping) = fields.get("loop").filter(|v| !v.is_null()) {
            if !looping.is_boolean() {
                return Err(error(format!("{path}.loop"), "must be a boolean"));
            }
        }
        let frames_path = format!("{path}.keyframes");
        let frames = fields
            .get("keyframes")
            .and_then(Value::as_array)
            .ok_or_else(|| error(frames_path.clone(), "is required as an array"))?;
        if frames.is_empty() {
            return Err(error(
                frames_path,
                "is empty (the three host does not play it)",
            ));
        }
        for (i, frame) in frames.iter().enumerate() {
            keyframe(frame, &format!("{frames_path}[{i}]"))?;
        }
        Ok(Self {
            declared: value.clone(),
        })
    }

    /// Whether any keyframe defines `track`.
    pub fn declares(&self, track: &str) -> bool {
        !self.values(track).is_empty()
    }

    /// Each keyframe's value for `track`, by keyframe index as declared.
    pub fn values(&self, track: &str) -> Vec<(usize, &Value)> {
        self.frames()
            .iter()
            .enumerate()
            .filter_map(|(i, frame)| frame.get(track).map(|v| (i, v)))
            .collect()
    }

    fn frames(&self) -> &[Value] {
        self.declared
            .get("keyframes")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice)
    }
}

fn keyframe(frame: &Value, path: &str) -> Result<(), AnimationError> {
    let fields: &Map<String, Value> = frame
        .as_object()
        .ok_or_else(|| error(path.to_owned(), "must be an object"))?;
    if !fields.contains_key("at") {
        return Err(error(format!("{path}.at"), "is required"));
    }
    for (key, value) in fields {
        let field = format!("{path}.{key}");
        let is_number = key == "at"
            || TRANSFORM_TRACKS.contains(&key.as_str())
            || matches!(key.as_str(), "opacity" | "emissiveIntensity");
        if is_number {
            finite(value, field)?;
        } else if COLOR_TRACKS.contains(&key.as_str()) {
            if !value.is_string() {
                return Err(error(field, "must be a CSS color string"));
            }
        } else {
            return Err(error(field, "not a declared keyframe prop"));
        }
    }
    Ok(())
}

fn finite(value: &Value, field: String) -> Result<f64, AnimationError> {
    value
        .as_f64()
        .filter(|n| n.is_finite())
        .ok_or_else(|| error(field, "must be a finite number"))
}

/// `applyMeshAnimation(node, elapsed_ms)`.
pub fn sample(track: &Animation, elapsed_ms: f64) -> AnimatedState {
    let value = |key: &str| drawable_track(&track.declared, elapsed_ms, key);
    let number = |key: &str| value(key).and_then(|v| v.as_f64());
    let text = |key: &str| value(key).and_then(|v| v.as_str().map(str::to_owned));
    AnimatedState {
        offset: [
            number("offsetX").unwrap_or(0.0),
            number("offsetY").unwrap_or(0.0),
            number("offsetZ").unwrap_or(0.0),
        ],
        rotate: [
            number("rotateX").unwrap_or(0.0),
            number("rotateY").unwrap_or(0.0),
            number("rotateZ").unwrap_or(0.0),
        ],
        scale: number("scale").unwrap_or(1.0),
        opacity: number("opacity"),
        emissive_intensity: number("emissiveIntensity"),
        color: text("color"),
        emissive: text("emissive"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(value: Value) -> Animation {
        Animation::parse(&value, "a").unwrap()
    }

    fn bounce(looping: bool) -> Animation {
        parse(json!({
            "durationMs": 1000,
            "loop": looping,
            "keyframes": [
                {"at": 1, "offsetX": 0, "scale": 1, "opacity": 1},
                {"at": 0, "offsetX": 0, "scale": 1, "color": "#ff0000"},
                {"at": 0.5, "offsetX": 2, "scale": 2, "opacity": 0.5, "color": "#00ff00"}
            ]
        }))
    }

    fn state(
        offset: [f64; 3],
        scale: f64,
        opacity: Option<f64>,
        color: Option<&str>,
    ) -> AnimatedState {
        AnimatedState {
            offset,
            rotate: [0.0; 3],
            scale,
            opacity,
            emissive_intensity: None,
            color: color.map(str::to_owned),
            emissive: None,
        }
    }

    // Expected values are applyMeshAnimation's output for the same declarations.
    #[test]
    fn a_looping_track_matches_the_three_host_sampler() {
        let a = bounce(true);
        assert_eq!(
            sample(&a, 0.0),
            state([0.0; 3], 1.0, Some(0.5), Some("#ff0000"))
        );
        assert_eq!(
            sample(&a, 250.0),
            state([1.0, 0.0, 0.0], 1.5, Some(0.5), Some("#ff0000"))
        );
        assert_eq!(
            sample(&a, 750.0),
            state([1.0, 0.0, 0.0], 1.5, Some(0.75), Some("#00ff00"))
        );
        assert_eq!(sample(&a, 1250.0), sample(&a, 250.0));
        assert_eq!(
            sample(&a, 2999.0),
            state(
                [0.0039999999999995595, 0.0, 0.0],
                1.0019999999999998,
                Some(0.9990000000000001),
                Some("#00ff00")
            )
        );
    }

    #[test]
    fn a_track_played_once_holds_its_last_keyframes() {
        let a = bounce(false);
        assert_eq!(
            sample(&a, 250.0),
            state([1.0, 0.0, 0.0], 1.5, Some(0.5), Some("#ff0000"))
        );
        let end = state([0.0; 3], 1.0, Some(1.0), Some("#00ff00"));
        assert_eq!(sample(&a, 1000.0), end);
        assert_eq!(sample(&a, 1500.0), end);
    }

    #[test]
    fn rotation_and_emissive_tracks_hold_their_first_value_before_it() {
        let a = parse(json!({
            "durationMs": 400,
            "keyframes": [
                {"at": 0.25, "rotateX": 0.4, "rotateY": -1, "rotateZ": 2, "offsetY": 1, "offsetZ": -2,
                 "emissiveIntensity": 3, "emissive": "#112233"},
                {"at": 0.75, "rotateX": 1.2, "rotateY": 1, "rotateZ": 0, "offsetY": 3, "offsetZ": 2,
                 "emissiveIntensity": 1, "emissive": "#445566"}
            ]
        }));
        let at =
            |offset: [f64; 3], rotate: [f64; 3], intensity: f64, emissive: &str| AnimatedState {
                offset,
                rotate,
                scale: 1.0,
                opacity: None,
                emissive_intensity: Some(intensity),
                color: None,
                emissive: Some(emissive.to_owned()),
            };
        let first = at([0.0, 1.0, -2.0], [0.4, -1.0, 2.0], 3.0, "#112233");
        assert_eq!(sample(&a, 0.0), first);
        assert_eq!(
            sample(&a, 200.0),
            at([0.0, 2.0, 0.0], [0.8, 0.0, 1.0], 2.0, "#112233")
        );
        assert_eq!(
            sample(&a, 350.0),
            at([0.0, 3.0, 2.0], [1.2, 1.0, 0.0], 1.0, "#445566")
        );
        assert_eq!(sample(&a, 500.0), first);
    }

    #[test]
    fn control_an_animation_without_tracks_is_the_identity() {
        let a = parse(json!({"durationMs": 500, "keyframes": [{"at": 0}]}));
        assert_eq!(sample(&a, 123.0), state([0.0; 3], 1.0, None, None));
    }

    #[test]
    fn an_unplayable_or_undeclared_animation_errors_by_field() {
        let err = |value: Value| Animation::parse(&value, "a").unwrap_err().to_string();
        assert_eq!(
            err(json!({"durationMs": 0, "keyframes": [{"at": 0}]})),
            "a.durationMs: 0 must be above zero (the three host does not play it)"
        );
        assert_eq!(
            err(json!({"durationMs": 10, "keyframes": []})),
            "a.keyframes: is empty (the three host does not play it)"
        );
        assert_eq!(
            err(json!({"durationMs": 10, "keyframes": [{"offsetX": 1}]})),
            "a.keyframes[0].at: is required"
        );
        assert_eq!(
            err(json!({"durationMs": 10, "keyframes": [{"at": 0, "wobble": 1}]})),
            "a.keyframes[0].wobble: not a declared keyframe prop"
        );
        assert_eq!(
            err(json!({"durationMs": 10, "keyframes": [{"at": 0, "color": 3}]})),
            "a.keyframes[0].color: must be a CSS color string"
        );
        assert_eq!(
            err(json!({"durationMs": 10, "keyframes": [{"at": 0, "scale": null}]})),
            "a.keyframes[0].scale: must be a finite number"
        );
    }
}
