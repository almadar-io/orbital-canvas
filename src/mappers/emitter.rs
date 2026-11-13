//! `draw-emitter` (rasm floor contract §14.8): particles that instantiate a template subtree, each a pure function of
//! the paint clock, the emitter's `seed` and the particle's index.

use serde_json::Value;

#[derive(Debug, PartialEq)]
pub(crate) struct EmitterError {
    pub field: &'static str,
    pub detail: String,
}

const MAX_LIVE: f64 = 10_000.0;
const BURST_BASE: u64 = 1 << 40;

struct Spec<'a> {
    position: &'a Value,
    template: &'a Value,
    rate: f64,
    burst: u64,
    lifetime: f64,
    spread: [f64; 2],
    speed: [f64; 2],
    gravity: f64,
    seed: u64,
    start_ms: f64,
    over: &'a Value,
}

fn err(field: &'static str, detail: &str) -> EmitterError {
    EmitterError {
        field,
        detail: detail.to_owned(),
    }
}

fn number(node: &Value, key: &'static str) -> Result<Option<f64>, EmitterError> {
    match node.get(key).filter(|v| !v.is_null()) {
        None => Ok(None),
        Some(v) => v
            .as_f64()
            .filter(|n| n.is_finite())
            .map(Some)
            .ok_or_else(|| err(key, "must be a finite number")),
    }
}

fn pair(node: &Value, key: &'static str) -> Result<Option<[f64; 2]>, EmitterError> {
    let Some(v) = node.get(key).filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let bad = || err(key, "must be [min, max]");
    match v.as_array().map(Vec::as_slice) {
        Some([a, b]) => Ok(Some([
            a.as_f64().ok_or_else(bad)?,
            b.as_f64().ok_or_else(bad)?,
        ])),
        _ => Err(bad()),
    }
}

fn spec(node: &Value) -> Result<Spec<'_>, EmitterError> {
    let position = node
        .get("position")
        .filter(|p| {
            p.get("x").and_then(Value::as_f64).is_some()
                && p.get("y").and_then(Value::as_f64).is_some()
        })
        .ok_or_else(|| err("position", "a draw-emitter needs position {x, y}"))?;
    let template = node
        .get("template")
        .filter(|t| t.is_array())
        .ok_or_else(|| err("template", "a draw-emitter needs a template list"))?;
    let lifetime = number(node, "lifetime")?
        .filter(|l| *l > 0.0)
        .ok_or_else(|| err("lifetime", "must be above zero seconds"))?;
    let rate = number(node, "rate")?
        .filter(|r| *r >= 0.0)
        .ok_or_else(|| err("rate", "must be zero or more per second"))?;
    let burst = number(node, "burst")?.unwrap_or(0.0);
    if burst < 0.0 || burst.fract() != 0.0 {
        return Err(err("burst", "must be a whole number of particles"));
    }
    if rate * lifetime + burst > MAX_LIVE {
        return Err(err("rate", "more than 10000 particles would live at once"));
    }
    let speed =
        pair(node, "speed")?.ok_or_else(|| err("speed", "must be [min, max] px per second"))?;
    let spread = pair(node, "spread")?.unwrap_or([0.0, std::f64::consts::TAU]);
    let seed = number(node, "seed")?.ok_or_else(|| err("seed", "a draw-emitter needs a seed"))?;
    let over = node
        .get("over")
        .filter(|o| o.is_object())
        .ok_or_else(|| err("over", "a draw-emitter needs an over object"))?;
    for key in ["opacity", "scale", "rotate"] {
        if let Some(v) = over.get(key).filter(|v| !v.is_null()) {
            if !v
                .as_array()
                .is_some_and(|a| !a.is_empty() && a.iter().all(|n| n.as_f64().is_some()))
            {
                return Err(err("over", "each track is a non-empty list of numbers"));
            }
        }
    }
    Ok(Spec {
        position,
        template,
        rate,
        burst: burst as u64,
        lifetime,
        spread,
        speed,
        gravity: number(node, "gravity")?.unwrap_or(0.0),
        seed: seed as i64 as u64,
        start_ms: number(node, "startMs")?.unwrap_or(0.0),
        over,
    })
}

fn local_seconds(spec: &Spec, epoch_now_ms: f64) -> f64 {
    (epoch_now_ms.max(0.0) - spec.start_ms) / 1000.0
}

fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn unit(seed: u64, index: u64, channel: u64) -> f64 {
    let h = mix(mix(seed ^ mix(index)) ^ channel);
    (h >> 11) as f64 / (1u64 << 53) as f64
}

fn sample(track: Option<&Value>, age: f64, rest: f64) -> f64 {
    let Some(values) = track.and_then(Value::as_array) else {
        return rest;
    };
    let at = |i: usize| values[i].as_f64().unwrap_or(rest);
    if values.len() == 1 {
        return at(0);
    }
    let pos = age.clamp(0.0, 1.0) * (values.len() - 1) as f64;
    let i = (pos.floor() as usize).min(values.len() - 2);
    at(i) + (at(i + 1) - at(i)) * (pos - i as f64)
}

fn particle(spec: &Spec, index: u64, age: f64) -> Value {
    let angle = spec.spread[0] + (spec.spread[1] - spec.spread[0]) * unit(spec.seed, index, 1);
    let speed = spec.speed[0] + (spec.speed[1] - spec.speed[0]) * unit(spec.seed, index, 2);
    let x = spec.position["x"].as_f64().unwrap_or(0.0) + angle.cos() * speed * age;
    let y = spec.position["y"].as_f64().unwrap_or(0.0)
        + angle.sin() * speed * age
        + 0.5 * spec.gravity * age * age;
    let u = age / spec.lifetime;
    serde_json::json!({
        "type": "draw-group",
        "position": {"x": x, "y": y},
        "scale": sample(spec.over.get("scale"), u, 1.0),
        "rotate": sample(spec.over.get("rotate"), u, 0.0),
        "opacity": sample(spec.over.get("opacity"), u, 1.0),
        "items": spec.template,
    })
}

pub(crate) fn expand(node: &Value, epoch_now_ms: f64) -> Result<Vec<Value>, EmitterError> {
    let spec = spec(node)?;
    let t = local_seconds(&spec, epoch_now_ms);
    let mut out = Vec::new();
    if t < 0.0 {
        return Ok(out);
    }
    if spec.rate > 0.0 {
        let first = (((t - spec.lifetime) * spec.rate).floor() + 1.0).max(0.0) as u64;
        let last = (t * spec.rate).floor() as u64;
        for index in first..=last {
            let age = t - index as f64 / spec.rate;
            if (0.0..spec.lifetime).contains(&age) {
                out.push(particle(&spec, index, age));
            }
        }
    }
    if t < spec.lifetime {
        for j in 0..spec.burst {
            out.push(particle(&spec, BURST_BASE + j, t));
        }
    }
    Ok(out)
}

fn emitters(node: &Value, f: &mut dyn FnMut(&Value)) {
    if node.get("type").and_then(Value::as_str) == Some("draw-emitter") {
        f(node);
    }
    for key in ["props", "drawables", "children", "items"] {
        match node.get(key) {
            Some(Value::Array(items)) => items.iter().for_each(|i| emitters(i, f)),
            Some(child @ Value::Object(_)) => emitters(child, f),
            _ => {}
        }
    }
}

pub(crate) fn has_emitter(node: &Value) -> bool {
    let mut found = false;
    emitters(node, &mut |_| found = true);
    found
}

/// Whether any emitter under `node` still has particles to paint or to be born at the clock.
pub(crate) fn any_live(node: &Value, epoch_now_ms: f64) -> bool {
    let mut live = false;
    emitters(node, &mut |e| {
        if let Ok(spec) = spec(e) {
            live |= spec.rate > 0.0
                || (spec.burst > 0 && local_seconds(&spec, epoch_now_ms) < spec.lifetime);
        }
    });
    live
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mappers::game_canvas_2d::from_props_mapped;
    use crate::packet::{DrawCommand, GameRenderPacket};
    use crate::SceneHost;
    use serde_json::json;

    fn emitter(extra: Value) -> Value {
        let mut node = json!({
            "type": "draw-emitter", "id": "sparks", "position": {"x": 100, "y": 100},
            "template": [{"type": "draw-shape", "shape": "rect", "anchor": "top-left",
                "position": {"x": 0, "y": 0}, "width": 10, "height": 10, "fill": "#ff0000"}],
            "rate": 10, "lifetime": 1.0, "speed": [50, 50], "spread": [0, 6.2831], "seed": 7,
            "over": {}
        });
        for (key, value) in extra.as_object().unwrap() {
            node[key] = value.clone();
        }
        node
    }

    fn scene(node: Value) -> Value {
        json!({"width": 400, "height": 400, "scale": 1.0, "projection": "free",
            "bgColor": "#00000000", "drawables": [node]})
    }

    fn frame(node: Value, at_ms: f64) -> GameRenderPacket {
        from_props_mapped(&scene(node), at_ms).unwrap().0
    }

    fn rects(packet: &GameRenderPacket) -> Vec<(f32, f32, f32, f32)> {
        packet
            .draw_commands
            .iter()
            .filter_map(|c| match c {
                DrawCommand::Quad { rect, color } => Some((rect.x, rect.y, rect.w, color[3])),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_same_seed_and_clock_give_the_same_frame_and_another_seed_a_different_one() {
        let a = rects(&frame(emitter(json!({})), 2500.0));
        let b = rects(&frame(emitter(json!({})), 2500.0));
        assert!(!a.is_empty(), "particles are alive at 2.5 s");
        assert_eq!(a, b);
        let other = rects(&frame(emitter(json!({"seed": 8})), 2500.0));
        assert_eq!(a.len(), other.len(), "control: the same count");
        assert_ne!(a, other);
    }

    #[test]
    fn a_particle_past_its_lifetime_is_gone() {
        let burst = |at: f64| rects(&frame(emitter(json!({"rate": 0, "burst": 5})), at));
        assert_eq!(burst(0.0).len(), 5, "a burst is born at t0");
        assert_eq!(
            burst(999.0).len(),
            5,
            "control: still alive just before the lifetime"
        );
        assert!(burst(1000.0).is_empty());
        let continuous = rects(&frame(
            emitter(json!({"rate": 10, "lifetime": 0.5})),
            3000.0,
        ));
        assert_eq!(
            continuous.len(),
            5,
            "rate x lifetime particles live at once"
        );
    }

    #[test]
    fn over_interpolates_opacity_and_scale_by_age() {
        let over = json!({"over": {"opacity": [1.0, 0.0], "scale": [1.0, 3.0]}});
        let at = |ms: f64| {
            let r = rects(&frame(
                emitter(json!({"rate": 0, "burst": 1, "over": over["over"]})),
                ms,
            ));
            (r[0].2, r[0].3)
        };
        let (w0, o0) = at(0.0);
        let (w1, o1) = at(500.0);
        assert!(
            (w0 - 10.0).abs() < 0.01 && (o0 - 1.0).abs() < 0.01,
            "{w0} {o0}"
        );
        assert!(
            (w1 - 20.0).abs() < 0.01 && (o1 - 0.5).abs() < 0.01,
            "{w1} {o1}"
        );
        let flat = rects(&frame(emitter(json!({"rate": 0, "burst": 1})), 500.0));
        assert!(
            (flat[0].2 - 10.0).abs() < 0.01 && (flat[0].3 - 1.0).abs() < 0.01,
            "control: no `over` leaves both"
        );
    }

    #[test]
    fn gravity_bends_the_path() {
        let still = json!({"rate": 0, "burst": 1, "spread": [0, 0], "speed": [100, 100]});
        let y_at = |extra: Value| {
            let mut node = emitter(still.clone());
            for (k, v) in extra.as_object().unwrap() {
                node[k] = v.clone();
            }
            rects(&frame(node, 500.0))[0]
        };
        let free = y_at(json!({}));
        let pulled = y_at(json!({"gravity": 200}));
        assert!(
            (free.0 - 150.0).abs() < 0.01 && (free.1 - 100.0).abs() < 0.01,
            "{free:?}"
        );
        assert!((pulled.0 - 150.0).abs() < 0.01, "gravity leaves x alone");
        assert!(
            (pulled.1 - 125.0).abs() < 0.01,
            "y = 0.5 g t^2 = 25: {pulled:?}"
        );
    }

    #[test]
    fn start_ms_moves_t0_and_nothing_paints_before_it() {
        let burst = emitter(json!({"rate": 0, "burst": 3, "startMs": 5000}));
        assert!(rects(&frame(burst.clone(), 4000.0)).is_empty());
        assert_eq!(rects(&frame(burst, 5200.0)).len(), 3);
    }

    #[test]
    fn a_malformed_emitter_is_a_scene_error() {
        let bad = from_props_mapped(&scene(emitter(json!({"lifetime": 0}))), 1000.0).unwrap_err();
        assert!(format!("{bad:?}").contains("lifetime"), "{bad:?}");
        let bad = from_props_mapped(&scene(emitter(json!({"speed": [1]}))), 1000.0).unwrap_err();
        assert!(format!("{bad:?}").contains("speed"), "{bad:?}");
    }

    #[test]
    fn wants_frames_while_particles_live_and_not_after() {
        let host = |node: Value, at: f64| {
            let mut host = SceneHost::new();
            host.set_epoch_clock(at);
            host.set_scene(
                json!({"type": "canvas", "mode": "2d", "projection": "free", "scale": 1.0,
                "drawables": [node]}),
            )
            .unwrap();
            host
        };
        let burst = emitter(json!({"rate": 0, "burst": 4, "startMs": 5000}));
        assert!(host(burst.clone(), 5400.0).wants_frames());
        assert!(!host(burst, 6100.0).wants_frames(), "the burst has died");
        assert!(
            host(emitter(json!({})), 60_000.0).wants_frames(),
            "a rate emitter keeps emitting"
        );
        let still = json!({"type": "draw-shape", "shape": "rect", "position": {"x": 0, "y": 0}, "width": 4, "height": 4, "fill": "#fff"});
        assert!(
            !host(still, 6100.0).wants_frames(),
            "control: a static scene needs none"
        );
    }
}
