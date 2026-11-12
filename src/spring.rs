//! Springs for a canvas scene's drawables: a drawable with an `id` and a `spring`
//! `{ stiffness, damping }` moves its animatable fields to a new scene's values instead of jumping.

use std::collections::HashMap;

use serde_json::{Number, Value};

use crate::mappers::game_canvas_2d::parse_css_color;

const SCALAR_FIELDS: [&str; 11] = [
    "/position/x",
    "/position/y",
    "/width",
    "/height",
    "/radiusX",
    "/radiusY",
    "/opacity",
    "/rotate",
    "/strokeWidth",
    "/offsetX",
    "/offsetY",
];
const CHILD_KEYS: [&str; 3] = ["drawables", "children", "items"];
const STEP: f64 = 1.0 / 120.0;
const REST: f64 = 0.01;

#[derive(Debug, Clone)]
struct Channel {
    color: bool,
    value: [f64; 4],
    velocity: [f64; 4],
    target: [f64; 4],
    stiffness: f64,
    damping: f64,
}

impl Channel {
    fn len(&self) -> usize {
        if self.color {
            4
        } else {
            1
        }
    }

    fn settled(&self) -> bool {
        (0..self.len())
            .all(|i| self.velocity[i].abs() < REST && (self.target[i] - self.value[i]).abs() < REST)
    }

    fn snap(&mut self) {
        self.value = self.target;
        self.velocity = [0.0; 4];
    }

    fn integrate(&mut self, dt: f64) {
        for i in 0..self.len() {
            let accel =
                self.stiffness * (self.target[i] - self.value[i]) - self.damping * self.velocity[i];
            self.velocity[i] += accel * dt;
            self.value[i] += self.velocity[i] * dt;
        }
    }

    fn json(&self) -> Option<Value> {
        if self.color {
            let byte = |c: f64| (c * 255.0).round().clamp(0.0, 255.0) as u8;
            Some(Value::String(format!(
                "rgba({},{},{},{:.3})",
                byte(self.value[0]),
                byte(self.value[1]),
                byte(self.value[2]),
                self.value[3].clamp(0.0, 1.0)
            )))
        } else {
            Number::from_f64(self.value[0]).map(Value::Number)
        }
    }
}

/// The displayed value of every spring-driven field of a scene.
#[derive(Debug, Clone)]
pub struct SpringBank {
    channels: HashMap<(String, String), Channel>,
    motion: bool,
}

impl Default for SpringBank {
    fn default() -> Self {
        Self::new()
    }
}

fn scene_root(scene: &Value) -> &Value {
    scene.get("props").unwrap_or(scene)
}

fn for_each_drawable(items: &Value, f: &mut dyn FnMut(&Value)) {
    let Some(items) = items.as_array() else {
        return;
    };
    for item in items {
        f(item);
        for key in CHILD_KEYS {
            if let Some(children) = item.get(key) {
                for_each_drawable(children, f);
            }
        }
    }
}

fn for_each_drawable_mut(items: &mut Value, f: &mut dyn FnMut(&mut Value)) {
    let Some(items) = items.as_array_mut() else {
        return;
    };
    for item in items {
        f(item);
        for key in CHILD_KEYS {
            if let Some(children) = item.get_mut(key) {
                for_each_drawable_mut(children, f);
            }
        }
    }
}

/// Every drawable under a canvas node's `drawables` and `children`, groups included.
pub(crate) fn drawables(scene: &Value, f: &mut dyn FnMut(&Value)) {
    let root = scene_root(scene);
    for key in ["drawables", "children"] {
        if let Some(items) = root.get(key) {
            for_each_drawable(items, f);
        }
    }
}

/// Whether any drawable paints a `material` that reads the clock (`live: true`).
pub(crate) fn has_live_material(scene: &Value) -> bool {
    let mut live = false;
    drawables(scene, &mut |d| {
        live |= d
            .pointer("/material/live")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    });
    live
}

fn animatable(d: &Value) -> Vec<(String, [f64; 4], bool)> {
    let mut fields = Vec::new();
    let scalar = |pointer: String, fields: &mut Vec<(String, [f64; 4], bool)>| {
        if let Some(n) = d.pointer(&pointer).and_then(Value::as_f64) {
            fields.push((pointer, [n, 0.0, 0.0, 0.0], false));
        }
    };
    for p in SCALAR_FIELDS {
        scalar(p.to_owned(), &mut fields);
    }
    for (array, count) in [("/corners/radius", 4), ("/material/params", 8)] {
        let len = d
            .pointer(array)
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
            .min(count);
        for i in 0..len {
            scalar(format!("{array}/{i}"), &mut fields);
        }
    }
    let text = d.get("type").and_then(Value::as_str) == Some("draw-text");
    let colors: &[&str] = if text {
        &["/color"]
    } else {
        &["/fill", "/stroke"]
    };
    for p in colors {
        if let Some(c) = d
            .pointer(p)
            .and_then(Value::as_str)
            .and_then(parse_css_color)
        {
            fields.push((
                (*p).to_owned(),
                [c[0] as f64, c[1] as f64, c[2] as f64, c[3] as f64],
                true,
            ));
        }
    }
    fields
}

impl SpringBank {
    /// An empty bank with motion allowed.
    pub fn new() -> Self {
        Self {
            channels: HashMap::new(),
            motion: true,
        }
    }

    /// Whether the bank drives no field at all.
    pub fn is_empty(&self) -> bool {
        self.channels.is_empty()
    }

    /// Drop every spring.
    pub fn clear(&mut self) {
        self.channels.clear();
    }

    /// Reduced motion (`false`) settles every spring at its target now and keeps them settled.
    pub fn set_motion(&mut self, enabled: bool) {
        self.motion = enabled;
        if !enabled {
            for channel in self.channels.values_mut() {
                channel.snap();
            }
        }
    }

    /// A new scene arrived: each animatable field of a drawable with an `id` and a `spring` keeps
    /// the value it displays now and heads for the scene's value.
    pub fn retarget(&mut self, scene: &Value) {
        let mut next = HashMap::new();
        drawables(scene, &mut |d| {
            let Some(id) = d.get("id").and_then(Value::as_str) else {
                return;
            };
            let Some(spring) = d.get("spring") else {
                return;
            };
            let (Some(stiffness), Some(damping)) = (
                spring.get("stiffness").and_then(Value::as_f64),
                spring.get("damping").and_then(Value::as_f64),
            ) else {
                return;
            };
            for (pointer, target, color) in animatable(d) {
                let key = (id.to_owned(), pointer);
                let mut channel = match self.channels.get(&key) {
                    Some(old) if old.color == color => Channel {
                        target,
                        stiffness,
                        damping,
                        ..old.clone()
                    },
                    _ => Channel {
                        color,
                        value: target,
                        velocity: [0.0; 4],
                        target,
                        stiffness,
                        damping,
                    },
                };
                if !self.motion {
                    channel.snap();
                }
                next.insert(key, channel);
            }
        });
        self.channels = next;
    }

    /// Advance every spring by `dt_seconds` (mass 1, semi-implicit Euler, in fixed substeps).
    /// Returns whether any value moved.
    pub fn step(&mut self, dt_seconds: f64) -> bool {
        if dt_seconds.is_nan() || dt_seconds <= 0.0 || self.settled() {
            return false;
        }
        let mut remaining = dt_seconds;
        while remaining > 0.0 && !self.settled() {
            let h = remaining.min(STEP);
            for channel in self.channels.values_mut() {
                channel.integrate(h);
            }
            remaining -= h;
        }
        for channel in self.channels.values_mut() {
            if channel.settled() {
                channel.snap();
            }
        }
        true
    }

    /// A copy of `scene` with every spring-driven field at its displayed value.
    pub fn apply(&self, scene: &Value) -> Value {
        let mut shown = scene.clone();
        if self.channels.is_empty() {
            return shown;
        }
        let mut by_id: HashMap<&str, Vec<(&str, &Channel)>> = HashMap::new();
        for ((id, pointer), channel) in &self.channels {
            by_id
                .entry(id.as_str())
                .or_default()
                .push((pointer.as_str(), channel));
        }
        let has_props = shown.get("props").is_some();
        let root = if has_props {
            &mut shown["props"]
        } else {
            &mut shown
        };
        let mut write = |d: &mut Value| {
            let Some(id) = d.get("id").and_then(Value::as_str).map(str::to_owned) else {
                return;
            };
            for (pointer, channel) in by_id.get(id.as_str()).into_iter().flatten() {
                if let (Some(slot), Some(value)) = (d.pointer_mut(pointer), channel.json()) {
                    *slot = value;
                }
            }
        };
        for key in ["drawables", "children"] {
            if let Some(items) = root.get_mut(key) {
                for_each_drawable_mut(items, &mut write);
            }
        }
        shown
    }

    /// Whether every spring rests at its target.
    pub fn settled(&self) -> bool {
        self.channels.values().all(Channel::settled)
    }
}
