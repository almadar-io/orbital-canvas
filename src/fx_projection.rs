//! Pure world FX presentation projection shared by native hosts and the web WASM host.

use serde_json::{json, Map, Value};
use std::f64::consts::TAU;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FxError {
    #[error("draw-fx-layer item #{index} has no `id`")]
    MissingId { index: usize },
    #[error("draw-fx-layer item `{id}`: `{field}` is missing or not {expected}")]
    Item {
        id: String,
        field: &'static str,
        expected: &'static str,
    },
    #[error("draw-fx-layer: `{field}` is missing or not {expected}")]
    Layer {
        field: &'static str,
        expected: &'static str,
    },
    #[error("draw-fx-layer item `{id}`: `position` must be {{x, y}} numbers (plus `z` in 3d) and finite")]
    Position { id: String },
    #[error("draw-fx-layer item `{id}`: unknown shape `{shape}` (ring, puff, streak, spark)")]
    UnknownShape { id: String, shape: String },
    #[error("draw-fx-layer: unknown dimension `{0}` (2d, 3d)")]
    Dimension(String),
}

struct Pos {
    x: f64,
    y: f64,
    z: Option<f64>,
}

impl Pos {
    fn value(&self) -> Value {
        let mut pos = json!({ "x": self.x, "y": self.y });
        if let Some(z) = self.z {
            pos["z"] = json!(z);
        }
        pos
    }

    fn offset(&self, dx: f64, dy: f64) -> Value {
        Pos {
            x: self.x + dx,
            y: self.y + dy,
            z: self.z,
        }
        .value()
    }
}

fn number(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(Value::as_f64)
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn id_of(view: &Value) -> String {
    text(view, "id").unwrap_or_default().to_string()
}

fn item_number(view: &Value, field: &'static str) -> Result<f64, FxError> {
    number(view, field).ok_or_else(|| FxError::Item {
        id: id_of(view),
        field,
        expected: "a number",
    })
}

fn item_text<'a>(view: &'a Value, field: &'static str) -> Result<&'a str, FxError> {
    text(view, field).ok_or_else(|| FxError::Item {
        id: id_of(view),
        field,
        expected: "a string",
    })
}

fn layer_number(layer: &Value, field: &'static str) -> Result<f64, FxError> {
    number(layer, field).ok_or(FxError::Layer {
        field,
        expected: "a number",
    })
}

fn layer_text<'a>(layer: &'a Value, field: &'static str) -> Result<&'a str, FxError> {
    text(layer, field).ok_or(FxError::Layer {
        field,
        expected: "a string",
    })
}

pub fn resolve_view(item: &Value, presets: Option<&[Value]>) -> Value {
    let Some(row) = presets.and_then(|rows| {
        rows.iter()
            .find(|row| text(row, "type") == text(item, "type"))
    }) else {
        return item.clone();
    };
    let Some(mut merged) = item.as_object().cloned() else {
        return item.clone();
    };
    for key in [
        "space",
        "effect",
        "color",
        "size",
        "vx",
        "vy",
        "vz",
        "particleCount",
        "animation",
    ] {
        if merged.get(key).is_none_or(Value::is_null) {
            if let Some(value) = row.get(key) {
                merged.insert(key.into(), value.clone());
            }
        }
    }
    for key in ["shape", "count", "glow", "gravity", "color2"] {
        if let Some(value) = row.get(key) {
            merged.insert(key.into(), value.clone());
        }
    }
    Value::Object(merged)
}

fn lifecycle(
    view: &Value,
    epoch_now_ms: f64,
    tick_ms: f64,
) -> Result<(f64, f64, f64, f64), FxError> {
    let ttl = item_number(view, "ttl")?;
    let max_ttl = item_number(view, "maxTtl")?.max(ttl).max(1.0);
    let life_ms = max_ttl * tick_ms;
    let age_ms = match number(view, "bornAt") {
        Some(born_at) if epoch_now_ms > 0.0 => (epoch_now_ms - born_at).max(0.0),
        _ => (1.0 - ttl / max_ttl) * life_ms,
    };
    let progress = (age_ms / life_ms).clamp(0.0, 1.0);
    Ok((life_ms, age_ms, progress, 1.0 - progress))
}

pub fn lifecycle_view(view: &Value, epoch_now_ms: f64, tick_ms: f64) -> Result<Value, FxError> {
    let (life_ms, age_ms, progress, fade) = lifecycle(view, epoch_now_ms, tick_ms)?;
    Ok(json!({ "lifeMs": life_ms, "ageMs": age_ms,
        "progress": progress, "fade": fade }))
}

/// The web FX seed is FNV-1a over JavaScript UTF-16 code units, then one avalanche.
pub fn fx_hash01(seed: &str, salt: u32) -> f64 {
    let mut hash = 2_166_136_261u32 ^ salt;
    for unit in seed.encode_utf16() {
        hash ^= u32::from(unit);
        hash = hash.wrapping_mul(16_777_619);
    }
    hash ^= hash >> 13;
    hash = hash.wrapping_mul(1_274_126_177);
    hash ^= hash >> 16;
    f64::from(hash) / 4_294_967_296.0
}

fn position(view: &Value, dimension: &str, age_sec: f64) -> Result<Pos, FxError> {
    let bad = || FxError::Position { id: id_of(view) };
    let three_d = match dimension {
        "2d" => false,
        "3d" => true,
        other => return Err(FxError::Dimension(other.to_string())),
    };
    let gravity = item_number(view, "gravity")?;
    let vx = item_number(view, "vx")?;
    let vz = item_number(view, "vz")?;
    let base = view.get("position").ok_or_else(bad)?;
    let x = number(base, "x").ok_or_else(bad)? + vx * age_sec;
    let y = number(base, "y").ok_or_else(bad)? + vz * age_sec;
    let fall = 0.5 * gravity * age_sec * age_sec;
    let pos = if three_d {
        let vy = item_number(view, "vy")?;
        let height = number(base, "z");
        let z = (height.is_some() || vy != 0.0 || gravity != 0.0)
            .then(|| height.unwrap_or(0.0) + vy * age_sec - fall);
        Pos { x, y, z }
    } else {
        Pos {
            x,
            y: y + fall,
            z: None,
        }
    };
    if pos.x.is_finite() && pos.y.is_finite() {
        Ok(pos)
    } else {
        Err(bad())
    }
}

fn ease_out(progress: f64) -> f64 {
    1.0 - (1.0 - progress) * (1.0 - progress)
}

fn shadow(view: &Value, color: &str) -> Option<Value> {
    number(view, "glow")
        .filter(|v| *v != 0.0)
        .map(|blur| json!({ "color": color, "blur": blur }))
}

fn shape_node(mut fields: Map<String, Value>, view: &Value, color: &str) -> Value {
    if let Some(shadow) = shadow(view, color) {
        fields.insert("shadow".into(), shadow);
    }
    Value::Object(fields)
}

fn count(view: &Value) -> Result<usize, FxError> {
    Ok(item_number(view, "count")?.max(0.0) as usize)
}

fn procedural(view: &Value, pos: &Pos, fade: f64, progress: f64) -> Result<Vec<Value>, FxError> {
    let id = item_text(view, "id")?;
    let shape = item_text(view, "shape")?;
    let size = item_number(view, "size")?;
    let color = item_text(view, "color")?;
    let pos_value = pos.value();
    match shape {
        "ring" => {
            // `color2` is the ring/puff accent; absent, the accent is `color`.
            let stroke = text(view, "color2").unwrap_or(color);
            let mut fields = Map::new();
            fields.insert("type".into(), json!("draw-shape"));
            fields.insert("shape".into(), json!("ellipse"));
            fields.insert("position".into(), pos_value);
            fields.insert("anchor".into(), json!("center"));
            fields.insert("radiusX".into(), json!(size * ease_out(progress)));
            fields.insert("stroke".into(), json!(stroke));
            fields.insert("strokeWidth".into(), json!(2));
            fields.insert("blendMode".into(), json!("lighter"));
            fields.insert("opacity".into(), json!(fade));
            Ok(vec![shape_node(fields, view, color)])
        }
        "puff" => Ok((0..count(view)?)
            .flat_map(|i| {
                let salt = i as u32 * 5;
                let angle = fx_hash01(id, salt) * TAU;
                let drift = size * 0.3 * fx_hash01(id, salt + 1) * ease_out(progress);
                let p = pos.offset(angle.cos() * drift, angle.sin() * drift);
                let fill = if i == 0 {
                    text(view, "color2").unwrap_or(color)
                } else {
                    color
                };
                let radius =
                    size * (0.15 + 0.35 * progress) * (0.7 + 0.6 * fx_hash01(id, salt + 2));
                let mut fields = Map::new();
                fields.insert("type".into(), json!("draw-shape"));
                fields.insert("shape".into(), json!("ellipse"));
                fields.insert("position".into(), p);
                fields.insert("anchor".into(), json!("center"));
                fields.insert("radiusX".into(), json!(radius));
                fields.insert("fill".into(), json!(fill));
                fields.insert("opacity".into(), json!(fade * 0.6));
                vec![shape_node(fields, view, color)]
            })
            .collect()),
        "streak" => Ok((0..count(view)?)
            .flat_map(|i| {
                let salt = i as u32 * 3;
                let angle = fx_hash01(id, salt) * TAU;
                let inner =
                    size * (0.2 + 0.8 * ease_out(progress)) * (0.6 + 0.4 * fx_hash01(id, salt + 1));
                let length = size * 0.35;
                let mut fields = Map::new();
                fields.insert("type".into(), json!("draw-shape"));
                fields.insert("shape".into(), json!("ellipse"));
                fields.insert("position".into(), pos_value.clone());
                fields.insert("anchor".into(), json!("center"));
                fields.insert("offsetX".into(), json!(inner + length / 2.0));
                fields.insert("offsetY".into(), json!(0));
                fields.insert("radiusX".into(), json!(length / 2.0));
                fields.insert("radiusY".into(), json!(size * 0.04));
                fields.insert("rotate".into(), json!(angle));
                fields.insert("fill".into(), json!(color));
                fields.insert("blendMode".into(), json!("lighter"));
                fields.insert("opacity".into(), json!(fade));
                vec![shape_node(fields, view, color)]
            })
            .collect()),
        "spark" => Ok((0..count(view)?)
            .flat_map(|i| {
                let salt = i as u32 * 3;
                let angle = fx_hash01(id, salt) * TAU;
                let dist = size * (0.4 + 0.6 * fx_hash01(id, salt + 1)) * ease_out(progress);
                let radius = size * (0.06 + 0.06 * fx_hash01(id, salt + 2));
                let p = pos.offset(angle.cos() * dist, angle.sin() * dist);
                let mut fields = Map::new();
                fields.insert("type".into(), json!("draw-shape"));
                fields.insert("shape".into(), json!("ellipse"));
                fields.insert("position".into(), p);
                fields.insert("anchor".into(), json!("center"));
                fields.insert("radiusX".into(), json!(radius));
                fields.insert("fill".into(), json!(color));
                fields.insert("blendMode".into(), json!("lighter"));
                fields.insert("opacity".into(), json!(fade));
                vec![shape_node(fields, view, color)]
            })
            .collect()),
        other => Err(FxError::UnknownShape {
            id: id.to_string(),
            shape: other.to_string(),
        }),
    }
}

const DEFAULT_TICK_MS: f64 = 500.0;
const DEFAULT_TEXT_COLOR: &str = "#ffe066";
const DEFAULT_FONT_FAMILY: &str = "system-ui, sans-serif";

/// A layer with the host's `DrawFxLayer` defaults laid over it: `tickMs` and `textColor` when absent (or null), and the
/// host's `fontFamily` always.
pub fn with_host_defaults(layer: &Map<String, Value>) -> Value {
    let mut layer = layer.clone();
    let absent = |layer: &Map<String, Value>, key: &str| layer.get(key).is_none_or(Value::is_null);
    if absent(&layer, "tickMs") {
        layer.insert("tickMs".into(), DEFAULT_TICK_MS.into());
    }
    if absent(&layer, "textColor") {
        layer.insert("textColor".into(), DEFAULT_TEXT_COLOR.into());
    }
    layer.insert("fontFamily".into(), DEFAULT_FONT_FAMILY.into());
    Value::Object(layer)
}

/// Expand one `draw-fx-layer` descriptor at the host's paint clock instant.
/// `std-fx-particles` still owns ttl decay and removal; this only derives pixels.
pub fn expand_layer(
    layer: &Value,
    epoch_now_ms: f64,
    dimension: &str,
    tile_width: f64,
) -> Result<Vec<Value>, FxError> {
    let items = layer
        .get("items")
        .and_then(Value::as_array)
        .ok_or(FxError::Layer {
            field: "items",
            expected: "an array",
        })?;
    let tick_ms = layer_number(layer, "tickMs")?;
    let presets = layer
        .get("presets")
        .and_then(Value::as_array)
        .map(Vec::as_slice);
    let mut out = Vec::new();
    for (index, item) in items.iter().enumerate() {
        if text(item, "id").is_none() {
            return Err(FxError::MissingId { index });
        }
        item_text(item, "type")?;
        let view = resolve_view(item, presets);
        if text(&view, "space") == Some("screen") {
            continue;
        }
        let (_, age_ms, progress, fade) = lifecycle(&view, epoch_now_ms, tick_ms)?;
        if progress >= 1.0 {
            continue;
        }
        let pos = position(&view, dimension, age_ms / 1000.0)?;
        let kind = item_text(&view, "type")?;
        let art = layer
            .get("art")
            .and_then(|art| art.get(kind))
            .and_then(|art| art.get("idle"));
        let sprite = layer.get("sprites").and_then(|sprites| sprites.get(kind));
        if let Some(art) = art.and_then(Value::as_array) {
            let mut group = json!({ "type": "draw-group", "position": pos.value(),
                "opacity": fade, "items": art });
            if let Some(size) = number(&view, "size") {
                group["scale"] = json!(size);
            }
            out.push(group);
        } else if sprite.and_then(|asset| text(asset, "url")).is_some() {
            let size = item_number(&view, "size")?;
            let mut node = json!({
                "type": "draw-sprite", "position": pos.value(), "asset": sprite,
                "anchor": "center", "width": size,
                "height": size, "opacity": fade,
                "clockMs": age_ms,
            });
            if let Some(animation) = view.get("animation").filter(|v| !v.is_null()).or_else(|| {
                sprite
                    .and_then(|s| s.get("animations"))
                    .and_then(Value::as_array)
                    .and_then(|v| v.first())
            }) {
                node["animation"] = animation.clone();
            }
            out.push(node);
        } else {
            out.extend(procedural(&view, &pos, fade, progress)?);
        }
        if let Some(message) = text(&view, "message").filter(|message| !message.is_empty()) {
            let family = layer_text(layer, "fontFamily")?;
            let color = layer_text(layer, "textColor")?;
            let px_size = (item_number(&view, "size")? * tile_width).round().max(10.0);
            out.push(json!({
                "type": "draw-text", "text": message, "position": pos.value(),
                "offsetY": -(0.15 + 0.55 * progress),
                "color": color,
                "opacity": fade, "font": format!("bold {px_size}px {family}"),
            }));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer() -> Value {
        json!({
            "items": [{ "id": "hit-1", "type": "hit", "position": { "x": 2, "y": 3 },
                "ttl": 2, "maxTtl": 4, "bornAt": 1000, "message": "-2", "color": "#ff0000" }],
            "presets": [{ "type": "hit", "shape": "ring", "size": 0.5,
                "color": "#00ff00", "vx": 1, "vz": 0, "gravity": 0 }],
            "tickMs": 500, "fontFamily": "system-ui, sans-serif", "textColor": "#ffe066"
        })
    }

    #[test]
    fn g_orbital_game_renderer_006_host_defaults_fill_only_what_the_layer_leaves_out() {
        let bare = with_host_defaults(json!({ "items": [] }).as_object().unwrap());
        assert_eq!((bare["tickMs"].clone(), bare["textColor"].clone()), (json!(500.0), json!("#ffe066")));
        assert_eq!(bare["fontFamily"], "system-ui, sans-serif");
        let set = with_host_defaults(json!({ "tickMs": 250, "textColor": "#fff" }).as_object().unwrap());
        assert_eq!((set["tickMs"].clone(), set["textColor"].clone()), (json!(250), json!("#fff")));
    }

    #[test]
    fn fx_projection_uses_preset_under_item_and_smooth_birth_clock() {
        let out = expand_layer(&layer(), 1500.0, "2d", 32.0).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["shape"], "ellipse");
        assert_eq!(out[0]["stroke"], "#ff0000");
        assert_eq!(out[0]["position"]["x"], 2.5);
        assert_eq!(out[0]["opacity"], 0.75);
        assert_eq!(out[1]["font"], "bold 16px system-ui, sans-serif");
        assert_eq!(out[1]["color"], "#ffe066");
    }

    #[test]
    fn fx_projection_skips_screen_and_expired_entries() {
        let layer = json!({ "tickMs": 500, "items": [
            {"id":"a","type":"spark","ttl":1,"space":"screen"},
            {"id":"b","type":"spark","ttl":0,"maxTtl":1,"bornAt":1000}
        ] });
        assert!(expand_layer(&layer, 2000.0, "2d", 32.0).unwrap().is_empty());
    }

    #[test]
    fn fx_projection_rejects_a_missing_position() {
        let mut layer = layer();
        layer["items"][0]
            .as_object_mut()
            .unwrap()
            .remove("position");
        assert_eq!(
            expand_layer(&layer, 1500.0, "2d", 32.0),
            Err(FxError::Position { id: "hit-1".into() })
        );
        layer["items"][0]["position"] = json!({ "x": 2 });
        assert!(matches!(
            expand_layer(&layer, 1500.0, "2d", 32.0),
            Err(FxError::Position { .. })
        ));
    }

    #[test]
    fn fx_projection_rejects_a_missing_or_unknown_shape() {
        let mut bad = layer();
        bad["presets"][0]["shape"] = json!("confetti");
        assert_eq!(
            expand_layer(&bad, 1500.0, "2d", 32.0),
            Err(FxError::UnknownShape {
                id: "hit-1".into(),
                shape: "confetti".into()
            })
        );
        bad["presets"][0].as_object_mut().unwrap().remove("shape");
        assert!(matches!(
            expand_layer(&bad, 1500.0, "2d", 32.0),
            Err(FxError::Item { field: "shape", .. })
        ));
        let mut control = layer();
        control["presets"][0]["shape"] = json!("spark");
        control["presets"][0]["count"] = json!(4);
        assert_eq!(expand_layer(&control, 1500.0, "2d", 32.0).unwrap().len(), 5);
    }

    #[test]
    fn fx_projection_rejects_a_missing_tick_ms() {
        let mut bad = layer();
        bad.as_object_mut().unwrap().remove("tickMs");
        assert_eq!(
            expand_layer(&bad, 1500.0, "2d", 32.0),
            Err(FxError::Layer {
                field: "tickMs",
                expected: "a number"
            })
        );
    }

    #[test]
    fn fx_projection_rejects_a_message_without_font_family() {
        let mut bad = layer();
        bad.as_object_mut().unwrap().remove("fontFamily");
        assert_eq!(
            expand_layer(&bad, 1500.0, "2d", 32.0),
            Err(FxError::Layer {
                field: "fontFamily",
                expected: "a string"
            })
        );
        bad["items"][0].as_object_mut().unwrap().remove("message");
        assert_eq!(expand_layer(&bad, 1500.0, "2d", 32.0).unwrap().len(), 1);
    }

    #[test]
    fn fx_hash_matches_utf16_seed_contract() {
        assert_eq!(fx_hash01("", 0), 0.24220648407936096);
        assert!(fx_hash01("💥", 2) >= 0.0 && fx_hash01("💥", 2) < 1.0);
    }
}
