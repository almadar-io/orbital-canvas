//! Page scroll: the wheel and touch move the page, it glides to rest, and the offset reaches the program at most once per interval.

use super::*;

/// Where a canvas declaring `scroll: { height, event, reportMs }` is scrolled to, how fast it is still moving, and
/// what it last told the program.
#[derive(Debug, Clone, Default)]
pub(crate) struct PageScroll {
    pub(crate) offset: f32,
    /// Packet units per second, carried on after a touch lifts and decaying to rest.
    pub(crate) velocity: f32,
    /// A touch that may scroll: where it went down, where and when it last moved, and whether it took the page.
    pub(crate) touch: Option<ScrollTouch>,
    pub(crate) reported: Option<f32>,
    pub(crate) reported_at: Option<f64>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ScrollTouch {
    pub(crate) start_y: f32,
    pub(crate) last_y: f32,
    pub(crate) last_ms: f64,
    pub(crate) engaged: bool,
    /// The press began on a region declaring `wheel: true` or `drag: true`, which keeps its gestures.
    pub(crate) owned: bool,
}

/// How fast a released page scroll slows (per second, exponential) and the speed below which it rests (packet units
/// per second).
pub(super) const SCROLL_FRICTION: f32 = 4.0;

pub(super) const SCROLL_REST: f32 = 20.0;

impl SceneHost {
    /// A point's height in the scene's own coordinates: a fixed drawable is where it is drawn, the rest sit the page
    /// scroll further down.
    pub(super) fn world_y(&self, fixed: bool, y: f32) -> f32 {
        if fixed {
            y
        } else {
            y + self.page_scroll.offset
        }
    }

    /// The canvas's `scroll` declaration: the content height in packet units, the event it reports the offset on,
    /// and the least time in ms between reports while it moves (0: every change).
    pub(super) fn scroll_declared(&self) -> Option<(f32, Option<String>, f64)> {
        let node = self.scene.as_ref()?;
        let declared = node.get("props").unwrap_or(node).get("scroll")?;
        let height = declared.get("height")?.as_f64().filter(|h| h.is_finite())? as f32;
        let event = declared
            .get("event")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let report_ms = declared
            .get("reportMs")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        Some((height, event, report_ms))
    }

    pub(super) fn scroll_max(&self) -> Option<f32> {
        let (height, _, _) = self.scroll_declared()?;
        let view = self.packet.as_ref().map_or(0.0, |p| p.height);
        Some((height - view).max(0.0))
    }

    /// How far the page is scrolled, in packet units; 0 on a canvas that declares no `scroll`.
    pub fn page_scroll(&self) -> f32 {
        self.page_scroll.offset
    }

    /// Move the page by `delta` packet units, held inside the content; false on a canvas that declares no `scroll`.
    pub(super) fn scroll_by(&mut self, delta: f32) -> bool {
        let Some(max) = self.scroll_max() else {
            return false;
        };
        let next = (self.page_scroll.offset + delta).clamp(0.0, max);
        if next != self.page_scroll.offset {
            self.page_scroll.offset = next;
            if let Some((x, y)) = self.pointer {
                self.hover_to(x, y);
            }
        }
        self.report_scroll(false);
        true
    }

    /// Carry a released touch's velocity on, decaying to rest, and report the offset when it rests.
    pub(super) fn advance_scroll(&mut self, dt: f32) {
        let gliding =
            self.page_scroll.velocity != 0.0 && !self.page_scroll.touch.is_some_and(|t| t.engaged);
        if gliding {
            let max = self.scroll_max().unwrap_or(0.0);
            let v = self.page_scroll.velocity;
            let next = (self.page_scroll.offset + v * dt).clamp(0.0, max);
            let decayed = v * (-dt * SCROLL_FRICTION).exp();
            self.page_scroll.velocity = if next <= 0.0 || next >= max || decayed.abs() < SCROLL_REST
            {
                0.0
            } else {
                decayed
            };
            self.page_scroll.offset = next;
            if let Some((x, y)) = self.pointer {
                self.hover_to(x, y);
            }
        }
        self.report_scroll(gliding && self.page_scroll.velocity == 0.0);
    }

    /// Whether the offset moved since the program last heard it.
    pub(super) fn scroll_report_pending(&self) -> bool {
        self.scroll_declared()
            .is_some_and(|(_, event, _)| event.is_some())
            && self.page_scroll.reported != Some(self.page_scroll.offset)
    }

    /// Tell the program the offset on the declared event: at once when the page comes to rest, else at most once
    /// per `reportMs` on the host clock.
    pub(super) fn report_scroll(&mut self, at_rest: bool) {
        let Some((_, Some(event), report_ms)) = self.scroll_declared() else {
            return;
        };
        let offset = self.page_scroll.offset;
        if self.page_scroll.reported == Some(offset) {
            return;
        }
        let now = self.clock_ms;
        let due = at_rest
            || self
                .page_scroll
                .reported_at
                .is_none_or(|at| now - at >= report_ms);
        if !due {
            return;
        }
        let max = self.scroll_max().unwrap_or(0.0);
        self.scene_events.push(mappers::scene::Emit {
            event,
            arg: String::new(),
            payload: serde_json::json!({ "offset": offset, "max": max }),
        });
        self.page_scroll.reported = Some(offset);
        self.page_scroll.reported_at = Some(now);
    }

    /// A touch on a scrolling canvas: once it travels past [`TAP_SLOP`] the page takes it (unless it began on a
    /// region that declares `wheel: true` or `drag: true`), the pressed control loses the pointer as on a cancel, and the page
    /// follows the finger. Returns true when the event was the page's.
    pub(super) fn scroll_touch(&mut self, event_type: &str, x: f32, y: f32) -> bool {
        if self.scroll_declared().is_none() {
            return false;
        }
        match event_type {
            "touch-down" => {
                self.page_scroll.velocity = 0.0;
                let owned = self.region_at(x, y).is_some_and(|r| r.wheel || r.drag);
                self.page_scroll.touch = Some(ScrollTouch {
                    start_y: y,
                    last_y: y,
                    last_ms: self.clock_ms,
                    engaged: false,
                    owned,
                });
                false
            }
            "touch-move" => {
                let Some(mut touch) = self.page_scroll.touch else {
                    return false;
                };
                if touch.owned || self.active_drag.is_some() || self.pannable {
                    return false;
                }
                if !touch.engaged {
                    if (y - touch.start_y).abs() <= TAP_SLOP {
                        return false;
                    }
                    touch.engaged = true;
                    self.touch_down = None;
                    self.captured = None;
                    if let Some(old) = self.hovered.take() {
                        self.emit_pointer(&old, x, y, |e| &e.leave, false);
                    }
                }
                let delta = touch.last_y - y;
                let dt = ((self.clock_ms - touch.last_ms) / 1000.0) as f32;
                if dt > 0.0 {
                    self.page_scroll.velocity = if self.motion { delta / dt } else { 0.0 };
                }
                touch.last_y = y;
                touch.last_ms = self.clock_ms;
                self.page_scroll.touch = Some(touch);
                self.scroll_by(delta);
                true
            }
            "touch-up" | "touch-cancel" => {
                let engaged = self.page_scroll.touch.take().is_some_and(|t| t.engaged);
                if !engaged {
                    self.page_scroll.velocity = 0.0;
                } else {
                    self.pointer = Some((x, y));
                    if self.page_scroll.velocity == 0.0 {
                        self.report_scroll(true);
                    }
                }
                engaged
            }
            _ => false,
        }
    }

    /// The wheel turned at a point (logical points): when the canvas declares `wheelEvent` and the topmost region
    /// under the point declared `wheel: true`, queues `wheelEvent` for it and returns true, the host's cue to keep
    /// the wheel from the page. Anywhere else the wheel is the page's and nothing is sent.
    /// On a canvas that declares `scroll`, a wheel anywhere else scrolls the page (and the host keeps it from the
    /// document).
    pub fn wheel(&mut self, x: f32, y: f32, dx: f32, dy: f32) -> bool {
        let (x, y) = self.to_packet_units(x, y);
        if self.canvas_event("wheelEvent").is_some() {
            if let Some((target, fixed)) = self
                .region_at(x, y)
                .filter(|r| r.wheel)
                .map(|r| (r.id.clone(), r.fixed))
            {
                let y = self.world_y(fixed, y);
                self.queue_canvas_event(
                    "wheelEvent",
                    serde_json::json!({ "target": target, "x": x, "y": y, "dx": dx, "dy": dy }),
                );
                return true;
            }
        }
        self.page_scroll.velocity = 0.0;
        let (_, delta) = self.to_packet_units(0.0, dy);
        self.scroll_by(delta)
    }
}
