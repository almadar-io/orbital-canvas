//! Events a scene sends back to the program: the canvas events it declares and the pointer, scroll and measure reports.

use super::*;

impl SceneHost {
    /// The prop `name` of the current canvas node, when it declares it.
    pub(super) fn canvas_event(&self, name: &str) -> Option<String> {
        self.frame().and_then(|node| {
            node.get("props")
                .unwrap_or(node)
                .get(name)
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
    }

    pub(super) fn queue_canvas_event(&mut self, name: &str, payload: serde_json::Value) {
        if let Some(event) = self.canvas_event(name) {
            self.scene_events.push(mappers::scene::Emit {
                event,
                arg: String::new(),
                payload,
            });
        }
    }

    /// Queue one `directionEvent` when the canvas's declared `direction` (the program renders it from the page
    /// locale) differs from the one last sent.
    pub(super) fn report_direction(&mut self) {
        if self.canvas_event("directionEvent").is_none() {
            return;
        }
        let Some(direction) = self.canvas_event("direction") else {
            return;
        };
        if self.direction_reported.as_deref() == Some(direction.as_str()) {
            return;
        }
        self.queue_canvas_event(
            "directionEvent",
            serde_json::json!({ "direction": direction }),
        );
        self.direction_reported = Some(direction);
    }

    /// Queue one `motionEvent` with the host's reduced-motion preference when it differs from the one last sent.
    pub(super) fn report_motion(&mut self) {
        if self.canvas_event("motionEvent").is_none() {
            return;
        }
        let reduced = !self.motion;
        if self.motion_reported == Some(reduced) {
            return;
        }
        self.queue_canvas_event("motionEvent", serde_json::json!({ "reduced": reduced }));
        self.motion_reported = Some(reduced);
    }

    /// Queue one `resizeEvent` (CSS pixels) when the size differs from the one last sent.
    pub(super) fn report_resize(&mut self) {
        let Some((w, h)) = self.viewport else { return };
        let size = (w as f32 / self.density, h as f32 / self.density);
        if self.resize_reported == Some(size) || self.canvas_event("resizeEvent").is_none() {
            return;
        }
        self.queue_canvas_event(
            "resizeEvent",
            serde_json::json!({ "width": size.0, "height": size.1 }),
        );
        self.resize_reported = Some(size);
    }

    /// Queue one `measureEvent` when the measured sizes differ from the list last reported, once no theme face is still
    /// downloading (sizes shaped in the fallback face are not the page's).
    pub(super) fn report_measured(&mut self) {
        if asset::faces_loading() {
            return;
        }
        let current: Vec<(String, f32, f32, u32)> = self
            .measured
            .iter()
            .map(|m| (m.id.clone(), m.width, m.height, m.lines))
            .collect();
        if current == self.measure_reported {
            return;
        }
        let event = self.frame().and_then(|node| {
            node.get("props")
                .unwrap_or(node)
                .get("measureEvent")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        });
        if let Some(event) = event {
            let items: Vec<serde_json::Value> = current
                .iter()
                .map(|(id, width, height, lines)| {
                    serde_json::json!({ "id": id, "width": width, "height": height, "lines": lines })
                })
                .collect();
            self.scene_events.push(mappers::scene::Emit {
                event,
                arg: String::new(),
                payload: serde_json::json!({ "items": items }),
            });
        }
        self.measure_reported = current;
    }

    /// Whether scene events are queued that the host has not taken yet.
    pub fn has_scene_events(&self) -> bool {
        !self.scene_events.is_empty()
    }

    /// Drain the scene events that taps queued (`[{event, payload}]` for the host to send).
    pub fn take_scene_events(&mut self) -> Vec<mappers::scene::Emit> {
        std::mem::take(&mut self.scene_events)
    }

    /// Drain and return all queued input events since the last call to this method or `render`.
    pub fn take_input_events(&mut self) -> Vec<InputEvent> {
        std::mem::take(&mut self.input_queue)
    }
}
