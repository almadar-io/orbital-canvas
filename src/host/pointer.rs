//! Pointer input: taps, hovers, drags, multi-touch gestures, zoom and the 3D view's input, routed to the regions a scene declares.

use super::*;

/// Whose a pointer event is: the gesture's primary pointer, a further pointer, or a leftover to drop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TouchRoute {
    Primary,
    Further,
    Ignore,
}

/// One pointer held down: the host's id for it and its position in packet units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Touch {
    pub(crate) id: i64,
    pub(crate) x: f32,
    pub(crate) y: f32,
}

pub(super) fn contains(r: &packet::Rect, x: f32, y: f32) -> bool {
    x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h
}

pub(super) fn region_contains(region: &mappers::scene::PointerRegion, x: f32, y: f32) -> bool {
    let r = &region.rect;
    let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    let (sin, cos) = (-region.rotation).sin_cos();
    let (dx, dy) = (x - cx, y - cy);
    let (lx, ly) = (cx + dx * cos - dy * sin, cy + dx * sin + dy * cos);
    contains(r, lx, ly)
}

/// A drag in progress: which region, where inside it the pointer grabbed, and whether it has moved.
#[derive(Debug, Clone)]
pub(crate) struct ActiveDrag {
    pub(crate) id: String,
    pub(crate) grab: (f32, f32),
    pub(crate) moved: bool,
}

/// A pointer that moves farther than this (logical points) between down and up is a drag, not a tap.
pub const TAP_SLOP: f32 = 10.0;

impl SceneHost {
    /// A pointer in logical points as packet units: a scene laid out in its declared world (an orbital-painter
    /// canvas) is stretched onto the surface, so its regions sit at packet size over the viewport's points.
    pub(super) fn to_packet_units(&self, x: f32, y: f32) -> (f32, f32) {
        match (&self.packet, self.viewport) {
            (Some(packet), Some((vw, vh))) if packet.width > 0.0 && packet.height > 0.0 => {
                let (cw, ch) = (vw as f32 / self.density, vh as f32 / self.density);
                (x * packet.width / cw, y * packet.height / ch)
            }
            _ => (x, y),
        }
    }

    /// A double tap (the host's recognizer) at a point: the events of the region under it.
    pub fn double_tap(&mut self, x: f32, y: f32) {
        let (x, y) = self.to_packet_units(x, y);
        let emits = mappers::scene::emits_in(&self.double_hits, x, y).to_vec();
        self.scene_events.extend(emits);
    }

    /// A zoom-button press (`zoom-in` | `zoom-out` | `reset`) on a pannable scene; an unknown
    /// command is refused.
    pub fn view_command(&mut self, command: &str) -> std::result::Result<(), String> {
        let parsed = mappers::scene::ViewCommand::parse(command).ok_or_else(|| {
            format!("unknown view command `{command}` (zoom-in | zoom-out | reset)")
        })?;
        if self.pannable {
            self.memory.pending_view = Some(parsed);
            self.relayout();
        }
        Ok(())
    }

    /// A pinch: zoom a pannable scene by `factor` about the focus point (logical points).
    /// Whether the scene takes zoom (a 3D camera or a pannable 2D scene): a host keeps the page from scrolling then.
    pub fn zoom(&mut self, factor: f32, x: f32, y: f32) -> bool {
        if let Some(view) = self.view3d.as_mut() {
            // a pinch or wheel out (factor below 1) dollies the 3D camera away, as a wheel down does
            self.view3d_moving |= view.wheel(if factor < 1.0 { 1.0 } else { -1.0 });
            return true;
        }
        if self.pannable && factor.is_finite() && factor > 0.0 {
            self.memory.pending_zoom = Some((factor, x, y));
            self.relayout();
        }
        self.pannable
    }

    /// Forward an input event into the renderer's queue. Returns `true` always (event accepted).
    ///
    /// Events are drained at the start of each `render()` call. Callers that need to inspect
    /// queued events before rendering can read `input_queue` directly.
    ///
    /// `x`/`y` are logical points. A `touch-down` followed by a `touch-up` within [`TAP_SLOP`]
    /// is a tap: the scene's topmost tappable region under it queues its events for
    /// [`Self::take_scene_events`]; a `touch-up` that travelled farther is a drag and emits nothing.
    pub fn input(
        &mut self,
        event_type: &str,
        x: f32,
        y: f32,
        button: i32,
        key: Option<&str>,
    ) -> bool {
        self.input_pointer(event_type, x, y, button, key, 0)
    }

    /// [`Self::input`] from one of several pointers: `pointer_id` is the host's id for it. The pointer whose press
    /// began the gesture behaves as a single pointer does; a further one reaches the region that holds capture,
    /// and every event of a multi-touch gesture carries `touches` (contract 14.3).
    pub fn input_pointer(
        &mut self,
        event_type: &str,
        x: f32,
        y: f32,
        button: i32,
        key: Option<&str>,
        pointer_id: i64,
    ) -> bool {
        if self.view3d.is_some() {
            return self.input_3d(event_type, x, y, button);
        }
        let (x, y) = self.to_packet_units(x, y);
        let before = self.touches.len();
        match self.track_touch(event_type, pointer_id, x, y) {
            TouchRoute::Primary => {}
            TouchRoute::Further => {
                self.touch_payload = self.gesture_payload(before);
                self.further_pointer(event_type, x, y);
                self.touch_payload = None;
                return true;
            }
            TouchRoute::Ignore => return true,
        }
        self.touch_payload = self.gesture_payload(before);
        let accepted = self.input_primary(event_type, x, y, button, key);
        self.touch_payload = None;
        accepted
    }

    /// Update the held pointers for one event and say who it belongs to.
    pub(super) fn track_touch(&mut self, event_type: &str, id: i64, x: f32, y: f32) -> TouchRoute {
        let known = self.touches.iter().any(|t| t.id == id);
        let route = match (event_type, self.primary_pointer) {
            ("hover" | "leave", _) => return TouchRoute::Primary,
            ("touch-down", None) => {
                self.primary_pointer = Some(id);
                TouchRoute::Primary
            }
            (_, Some(primary)) if primary == id => TouchRoute::Primary,
            (_, Some(_)) => {
                if known || event_type == "touch-down" {
                    TouchRoute::Further
                } else {
                    TouchRoute::Ignore
                }
            }
            (_, None) if known => TouchRoute::Ignore,
            (_, None) => TouchRoute::Primary,
        };
        match event_type {
            "touch-down" | "touch-move" => {
                if route != TouchRoute::Ignore {
                    match self.touches.iter_mut().find(|t| t.id == id) {
                        Some(held) => (held.x, held.y) = (x, y),
                        None => {
                            self.touches.push(Touch { id, x, y });
                            self.touches.sort_by_key(|t| t.id);
                        }
                    }
                }
            }
            "touch-up" | "touch-cancel" => {
                self.touches.retain(|t| t.id != id);
                if self.primary_pointer == Some(id) {
                    self.primary_pointer = None;
                }
            }
            _ => {}
        }
        route
    }

    /// The `touches` payload of the event under way: the pointers still held, present while more than one
    /// pointer is or was down.
    pub(super) fn gesture_payload(&self, before: usize) -> Option<serde_json::Value> {
        (before > 1 || self.touches.len() > 1).then(|| {
            serde_json::Value::Array(
                self.touches
                    .iter()
                    .map(|t| serde_json::json!({ "pointerId": t.id, "x": t.x, "y": t.y }))
                    .collect(),
            )
        })
    }

    /// A further pointer's press, move or release, sent to the region that holds capture.
    pub(super) fn further_pointer(&mut self, event_type: &str, x: f32, y: f32) {
        let Some(id) = self.captured.clone() else {
            return;
        };
        match event_type {
            "touch-down" => self.emit_pointer(&id, x, y, |e| &e.down, false),
            "touch-move" | "touch-up" | "touch-cancel" => {
                self.emit_pointer(&id, x, y, |e| &e.moved, false)
            }
            _ => {}
        }
    }

    pub(super) fn input_primary(
        &mut self,
        event_type: &str,
        x: f32,
        y: f32,
        button: i32,
        key: Option<&str>,
    ) -> bool {
        if self.scroll_touch(event_type, x, y) {
            return true;
        }
        if matches!(event_type, "hover" | "leave") {
            self.pointer_input(event_type, x, y);
            return true;
        }
        self.pointer_input(event_type, x, y);
        match event_type {
            "touch-down" => {
                self.touch_down = Some((x, y));
                let wy = self.world_y(false, y);
                self.active_drag = self
                    .drags
                    .iter()
                    .rev()
                    .find(|d| contains(&d.rect, x, wy))
                    .map(|d| ActiveDrag {
                        id: d.id.clone(),
                        grab: (x - d.rect.x, wy - d.rect.y),
                        moved: false,
                    });
                self.pan_from = (self.active_drag.is_none() && self.pannable).then_some((x, y));
                if self.memory.pressed.take().is_some() {
                    self.relayout();
                }
            }
            "touch-cancel" => {
                self.touch_down = None;
                self.active_drag = None;
                self.pan_from = None;
            }
            "touch-move" => {
                if let Some((px, py)) = self.pan_from {
                    self.memory.pending_pan.0 += x - px;
                    self.memory.pending_pan.1 += y - py;
                    self.pan_from = Some((x, y));
                    self.relayout();
                } else {
                    self.drag_to(x, y);
                }
            }
            "double-tap" => self.double_tap(x, y),
            "long-press" => {
                // A press is not a tap or a drag: the finger lifting after it fires nothing.
                self.touch_down = None;
                self.active_drag = None;
                self.pan_from = None;
                self.memory.pending_press = Some((x, y));
                self.relayout();
            }
            "touch-up" => {
                self.pan_from = None;
                let dragged = self.active_drag.take().is_some_and(|d| d.moved);
                if let (Some((dx, dy)), false) = (self.touch_down.take(), dragged) {
                    if (x - dx).hypot(y - dy) <= TAP_SLOP {
                        let emits = mappers::scene::emits_in(&self.hits, x, self.world_y(false, y));
                        self.scene_events.extend(emits.iter().cloned());
                    }
                }
            }
            _ => {}
        }
        self.input_queue.push(InputEvent {
            event_type: event_type.to_owned(),
            x,
            y,
            button,
            key: key.map(str::to_owned),
        });
        true
    }

    pub(super) fn region_at(&self, x: f32, y: f32) -> Option<&mappers::scene::PointerRegion> {
        self.pointer_regions
            .iter()
            .rev()
            .find(|r| region_contains(r, x, self.world_y(r.fixed, y)))
    }

    pub(super) fn emit_pointer(
        &mut self,
        id: &str,
        x: f32,
        y: f32,
        pick: fn(&mappers::scene::PointerEvents) -> &Option<String>,
        with_index: bool,
    ) {
        let region = self.pointer_regions.iter().rev().find(|r| r.id == id);
        let event = region.and_then(|r| pick(&r.events).clone());
        let origin = region.and_then(|r| r.origin);
        let y = self.world_y(region.is_some_and(|r| r.fixed), y);
        if let Some(event) = event {
            self.scene_events.push(mappers::scene::Emit {
                event,
                arg: String::new(),
                // `target`, not `id`: a control payload's `id` seeds the dispatch on that row (client_core extract_entity_id)
                payload: {
                    let mut payload = serde_json::json!({ "target": id, "x": x, "y": y });
                    // the press in the space of the origin its drawable was placed in
                    if let Some((ox, oy)) = origin {
                        payload["originX"] = serde_json::json!(ox);
                        payload["originY"] = serde_json::json!(oy);
                    }
                    if let Some(touches) = &self.touch_payload {
                        payload["touches"] = touches.clone();
                    }
                    // a press on a hit draw-text carries the caret index nearest it (contract §11)
                    if let Some(index) = with_index
                        .then(|| self.text_hits.iter().rev().find(|hit| hit.id == id))
                        .flatten()
                        .and_then(|hit| hit.index_at(x, y))
                    {
                        payload["index"] = serde_json::json!(index);
                    }
                    payload
                },
            });
        }
    }

    pub(super) fn hover_to(&mut self, x: f32, y: f32) {
        let under = self.region_at(x, y).map(|r| r.id.clone());
        if under == self.hovered {
            return;
        }
        if let Some(old) = self.hovered.take() {
            self.emit_pointer(&old, x, y, |e| &e.leave, false);
        }
        if let Some(new) = under {
            self.emit_pointer(&new, x, y, |e| &e.enter, false);
            self.hovered = Some(new);
        }
    }

    pub(super) fn pointer_moved(&mut self, x: f32, y: f32) {
        self.hover_to(x, y);
        // `pointerMoves: "captured"` (declared on the canvas) sends a move only while a press holds capture.
        let captured_only = self.frame().is_some_and(|node| {
            node.get("props")
                .unwrap_or(node)
                .get("pointerMoves")
                .and_then(serde_json::Value::as_str)
                == Some("captured")
        });
        let target = if captured_only {
            self.captured.clone()
        } else {
            self.captured.clone().or_else(|| self.hovered.clone())
        };
        if let Some(id) = target {
            self.emit_pointer(&id, x, y, |e| &e.moved, false);
        }
    }

    /// The pointer-region events (enter, leave, down, up, move) of one input event.
    pub(super) fn pointer_input(&mut self, event_type: &str, x: f32, y: f32) {
        match event_type {
            "hover" | "touch-move" => {
                self.pointer = Some((x, y));
                self.pointer_moved(x, y);
            }
            "leave" => {
                self.pointer = None;
                if let Some(old) = self.hovered.take() {
                    self.emit_pointer(&old, x, y, |e| &e.leave, false);
                }
            }
            "touch-down" => {
                self.pointer = Some((x, y));
                self.hover_to(x, y);
                if let Some(id) = self.region_at(x, y).map(|r| r.id.clone()) {
                    self.emit_pointer(&id, x, y, |e| &e.down, true);
                    self.captured = Some(id);
                }
            }
            "touch-up" => {
                self.pointer = Some((x, y));
                if let Some(id) = self.captured.take() {
                    self.emit_pointer(&id, x, y, |e| &e.up, false);
                }
                self.hover_to(x, y);
            }
            "touch-cancel" => self.captured = None,
            _ => {}
        }
    }

    /// Move the active drag to the pointer: queue each of its move events with the new top-left in
    /// scene units, clamped to the region's bounds (the web clamps to the canvas the same way).
    pub(super) fn drag_to(&mut self, x: f32, y: f32) {
        let Some((dx, dy)) = self.touch_down else {
            return;
        };
        let Some(active) = self.active_drag.as_mut() else {
            return;
        };
        if !active.moved && (x - dx).hypot(y - dy) <= TAP_SLOP {
            return;
        }
        active.moved = true;
        let (id, grab) = (active.id.clone(), active.grab);
        let y = self.world_y(false, y);
        let Some(region) = self.drags.iter().rev().find(|d| d.id == id) else {
            return;
        };
        if region.pin {
            let at = (
                f64::from((x - region.scene_origin.0) / region.scene_scale),
                f64::from((y - region.scene_origin.1) / region.scene_scale),
            );
            self.memory.positions.insert(id, at);
            self.relayout();
            return;
        }
        let nx =
            ((x - grab.0 - region.scene_origin.0) / region.scene_scale).clamp(0.0, region.max.0);
        let ny =
            ((y - grab.1 - region.scene_origin.1) / region.scene_scale).clamp(0.0, region.max.1);
        for event in &region.moves {
            self.scene_events.push(mappers::scene::Emit {
                event: event.clone(),
                arg: String::new(),
                payload: serde_json::json!({ "id": id, "x": nx, "y": ny }),
            });
        }
    }

    /// Pointer input on a 3D scene: the view orbits, and a click becomes the scene's own unit or tile event.
    pub(super) fn input_3d(&mut self, event_type: &str, x: f32, y: f32, button: i32) -> bool {
        let button = match button {
            1 => scene3d::Button::Middle,
            2 => scene3d::Button::Secondary,
            _ => scene3d::Button::Primary,
        };
        let declared = |key: &str| {
            self.frame()
                .and_then(|node| node.get("props").unwrap_or(node).get(key))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        };
        let (unit, tile) = (declared("unitClickEvent"), declared("tileClickEvent"));
        let Some(view) = self.view3d.as_mut() else {
            return false;
        };
        match event_type {
            "touch-down" => view.pointer_down(0, x, y, button, false),
            "touch-move" => self.view3d_moving |= view.pointer_move(0, x, y),
            "touch-up" => {
                let released = view.pointer_release(0, x, y, unit.is_some(), tile.is_some());
                let emit = |event: String, payload: serde_json::Value| mappers::scene::Emit {
                    event,
                    arg: String::new(),
                    payload,
                };
                match released {
                    Some(scene3d::view::Release::Pick(scene3d::Pick::Unit { id, x, z })) => {
                        if let Some(event) = unit {
                            self.scene_events.push(emit(
                                event,
                                serde_json::json!({ "unitId": id, "x": x, "z": z }),
                            ));
                        }
                    }
                    Some(scene3d::view::Release::Pick(scene3d::Pick::Tile { x, z })) => {
                        if let Some(event) = tile {
                            self.scene_events
                                .push(emit(event, serde_json::json!({ "x": x, "z": z })));
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        true
    }
}
