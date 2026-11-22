//! The scene's clock and frames: when it wants one, and the packet it draws at the clock.

use super::*;

impl SceneHost {
    /// Hand the renderer the host's frame clock (ms, any monotonic origin) before a `render`. The
    /// first value is scene time zero.
    pub fn set_clock(&mut self, ms: f64) {
        let first = self.clock_epoch.is_none();
        self.clock_epoch.get_or_insert(ms);
        let elapsed = ((ms - self.clock_ms) / 1000.0).max(0.0);
        let dt = elapsed as f32;
        self.clock_ms = ms;
        if !first && self.springs.step(elapsed) {
            self.relayout();
        }
        if !first {
            self.advance_scroll(dt);
        }
        if let Some(view) = self.view3d.as_mut() {
            self.view3d_moving = view.advance(dt);
        }
    }

    pub fn set_epoch_clock(&mut self, epoch_ms: f64) {
        if epoch_ms.is_finite() && epoch_ms > 0.0 {
            self.epoch_clock_ms = epoch_ms;
        }
    }

    /// Whether the host allows motion. Off, every animated scene draws its rest frame.
    pub fn set_motion(&mut self, enabled: bool) {
        self.motion = enabled;
        self.report_motion();
        let was_settled = self.springs.settled();
        self.springs.set_motion(enabled);
        if !was_settled && self.springs.settled() {
            self.relayout();
        }
    }

    pub(super) fn scene_time(&self) -> f64 {
        self.clock_ms - self.clock_epoch.unwrap_or(self.clock_ms)
    }

    /// Whether the current scene plays motion the host's reduced-motion setting allows.
    pub fn is_animated(&self) -> bool {
        self.motion
            && self
                .packet
                .as_ref()
                .is_some_and(GameRenderPacket::is_animated)
    }

    /// Whether the host should keep drawing frames: the scene is animated, or assets it asked for
    /// are still arriving. A static, settled scene needs no loop; a new scene or a resize wakes it.
    pub fn wants_frames(&self) -> bool {
        self.view3d_moving
            || self.page_scroll.velocity != 0.0
            || self.scroll_report_pending()
            || self.is_animated()
            || (self.motion
                && self
                    .packet
                    .as_ref()
                    .is_some_and(|p| p.epoch_live(self.epoch_clock_ms)))
            || (self.motion && !self.springs.settled())
            || self.clocked_material_wants_frames()
            || (self.motion && self.epoch_clock_ms > 0.0 && !self.live_scenes3d.is_empty())
            || (self.motion
                && self.epoch_clock_ms > 0.0
                && self.liveness.live_at(self.epoch_clock_ms))
            || asset::pending() > 0
            || asset::generation() != self.asset_generation
    }

    /// Whether a material that reads the clock (`live`, a live target, or an `anchor`) is drawn where it shows. On a
    /// canvas that scrolls as a page, a top-level rect is judged by its place against the view; anything the host
    /// cannot place that way (nested, another anchor, animated) counts wherever it is, as on a canvas with no scroll.
    fn clocked_material_wants_frames(&self) -> bool {
        if !self.motion {
            return false;
        }
        if self.scene.is_none() {
            return self.live_material;
        }
        let view = self
            .scroll_declared()
            .map(|_| self.packet.as_ref().map_or(0.0, |p| p.height));
        self.liveness
            .materials
            .iter()
            .any(|&(live, anchored, rows)| {
                let counts = live || (anchored && self.epoch_clock_ms > 0.0);
                counts
                    && match (view, rows) {
                        (Some(height), Some((top, bottom, fixed))) => {
                            let offset = if fixed { 0.0 } else { self.page_scroll.offset };
                            bottom - offset > 0.0 && top - offset < height
                        }
                        _ => true,
                    }
            })
    }

    /// The current packet as it draws at the clock's scene time.
    pub fn frame_packet(&self) -> Option<GameRenderPacket> {
        self.frame_packet_at(false)
    }

    /// [`Self::frame_packet`] with every command of the page, the ones scrolled far from the view too (the audits read it).
    pub(crate) fn whole_frame_packet(&self) -> Option<GameRenderPacket> {
        self.frame_packet_at(true)
    }

    fn frame_packet_at(&self, whole: bool) -> Option<GameRenderPacket> {
        let mut packet = self.mapped_frame_packet(whole)?;
        if self.motion && self.epoch_clock_ms > 0.0 {
            for live in &self.live_scenes3d {
                let Ok(scene) = live.scene_at(self.epoch_clock_ms) else {
                    continue;
                };
                replace_target_scene(&mut packet.draw_commands, &live.id, scene);
            }
        }
        Some(packet)
    }

    fn mapped_frame_packet(&self, whole: bool) -> Option<GameRenderPacket> {
        if self.motion && self.epoch_clock_ms > 0.0 {
            if let Some(scene) = self.scene.as_ref().filter(|scene| {
                self.liveness.timed
                    && matches!(
                        scene.get("type").and_then(serde_json::Value::as_str),
                        Some("canvas" | "canvas-2d")
                    )
            }) {
                let live = self.liveness.live_at(self.epoch_clock_ms);
                let generation = asset::generation();
                let mut settled = self
                    .settled
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if live {
                    *settled = None;
                } else if let Some((_, packet)) = settled
                    .as_ref()
                    .filter(|(mapped_under, _)| *mapped_under == generation)
                {
                    return Some(self.present(packet, whole));
                }
                #[cfg(test)]
                self.timed_maps
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let viewport = self
                    .viewport
                    .map(|(w, h)| (w as f32 / self.density, h as f32 / self.density));
                let mut prepared = self
                    .prepared
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if prepared.is_none() {
                    let shown = (!self.springs.is_empty()).then(|| self.springs.apply(scene));
                    *prepared = mappers::game_canvas_2d::prepare_canvas_node(
                        shown.as_ref().unwrap_or(scene),
                    )
                    .ok();
                }
                let remapped = prepared.as_ref().map(|props| {
                    mappers::game_canvas_2d::from_prepared_at(props, viewport, self.epoch_clock_ms)
                });
                if let Some(Ok(packet)) = remapped {
                    if !live {
                        *settled = Some((generation, packet.clone()));
                    }
                    return Some(self.present(&packet, whole));
                }
            }
        }
        let mut packet = self.present(self.packet.as_ref()?, whole);
        if let Some(camera) = self.view3d.as_ref().and_then(scene3d::Scene3DView::camera) {
            for command in &mut packet.draw_commands {
                if let DrawCommand::Scene3D { scene } = command {
                    scene.camera = camera;
                }
            }
        }
        Some(packet)
    }

    /// A mapped packet as this frame draws it: its animations played at the scene time and the page scroll, the
    /// clock and pointer for materials, and every unpinned command moved up by the scroll. On a canvas that scrolls as
    /// a page, unless `whole`, only the pinned commands and the ones within a view above or below the view are kept.
    pub(super) fn present(&self, packet: &GameRenderPacket, whole: bool) -> GameRenderPacket {
        let offset = self.page_scroll.offset;
        let resolved =
            packet.resolved_at_scroll(self.scene_time(), self.epoch_clock_ms, self.motion, offset);
        let mut packet = if whole || self.scroll_declared().is_none() {
            resolved.into_owned()
        } else {
            let view = resolved.height;
            near_the_view(resolved, offset - view, offset + 2.0 * view)
        };
        packet.scroll = offset;
        packet.time_ms = self.scene_time();
        if self.epoch_clock_ms > 0.0 {
            crate::targets::stamp_since(&mut packet.draw_commands, self.epoch_clock_ms);
        }
        packet.pointer = self.pointer.map(|(x, y)| packet::Vec2 { x, y });
        packet.scrolled(offset)
    }

    /// Map the current scene again once every asset it asked for has landed (or `timeout` passes):
    /// the one-shot raster path draws a single frame, so it cannot wait for a later `render`.
    pub fn settle_assets(&mut self, timeout: std::time::Duration) {
        let deadline = std::time::Instant::now() + timeout;
        while asset::pending() > 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        self.asset_generation = asset::generation();
        self.relayout();
    }
}

/// The packet with only its pinned commands and those that may paint between `top` and `bottom` (unscrolled
/// coordinates); its pinned and animated ranges follow the commands kept.
fn near_the_view(
    packet: std::borrow::Cow<'_, GameRenderPacket>,
    top: f32,
    bottom: f32,
) -> GameRenderPacket {
    let pinned = |i: usize| {
        packet
            .pinned
            .iter()
            .any(|&(first, end)| i >= first && i < end)
    };
    let keep: Vec<bool> = packet
        .draw_commands
        .iter()
        .enumerate()
        .map(|(i, command)| {
            pinned(i)
                || match crate::vector::command_bounds(command) {
                    crate::vector::RunBounds::Empty => false,
                    crate::vector::RunBounds::Anywhere => true,
                    crate::vector::RunBounds::Box { y0, y1, .. } => y1 >= top && y0 <= bottom,
                }
        })
        .collect();
    let kept_before = |end: usize| keep[..end.min(keep.len())].iter().filter(|k| **k).count();
    let ranges: Vec<(usize, usize)> = packet
        .pinned
        .iter()
        .map(|&(first, end)| (kept_before(first), kept_before(end)))
        .collect();
    let mut out = match packet {
        std::borrow::Cow::Owned(mut owned) => {
            let commands = std::mem::take(&mut owned.draw_commands);
            owned.draw_commands = commands
                .into_iter()
                .zip(&keep)
                .filter_map(|(command, k)| k.then_some(command))
                .collect();
            owned
        }
        std::borrow::Cow::Borrowed(shared) => GameRenderPacket {
            pattern: shared.pattern.clone(),
            width: shared.width,
            height: shared.height,
            background_image: shared.background_image.clone(),
            asset_base_url: shared.asset_base_url.clone(),
            camera: shared.camera,
            draw_commands: shared
                .draw_commands
                .iter()
                .zip(&keep)
                .filter(|(_, k)| **k)
                .map(|(command, _)| command.clone())
                .collect(),
            tick_event: shared.tick_event.clone(),
            draw_event: shared.draw_event.clone(),
            fps: shared.fps,
            animations: shared.animations.clone(),
            time_ms: shared.time_ms,
            pointer: shared.pointer,
            scroll: shared.scroll,
            pinned: Vec::new(),
        },
    };
    out.pinned = ranges;
    for animation in &mut out.animations {
        animation.first = kept_before(animation.first);
        animation.end = kept_before(animation.end);
    }
    out
}

/// A top-level drawable with a clocked material: live, anchored, and the rows it covers (top, bottom, fixed) when plain to read.
pub(crate) type ClockedMaterial = (bool, bool, Option<(f32, f32, bool)>);

/// What keeps a scene moving, read once when it is set: the epoch instant its fx, emitters, text reveals and
/// `startMs` animations have all ended by (infinite while one runs on), whether it has any of them, and each top-level
/// drawable that paints a clocked material.
#[derive(Debug, Clone, Default)]
pub(crate) struct Liveness {
    until: Option<f64>,
    /// The scene has fx, emitters, reveals or `startMs` animations: frames map it again at the epoch clock.
    pub(crate) timed: bool,
    /// Per top-level drawable with a clocked material: live, anchored, and the rows it covers when plain to read.
    pub(crate) materials: Vec<ClockedMaterial>,
}

impl Liveness {
    pub(crate) fn read(scene: &serde_json::Value) -> Self {
        let until = [
            mappers::game_canvas_2d::active_fx_until(scene),
            mappers::emitter::live_until(scene),
            mappers::text_reveal::live_until(scene),
            mappers::game_canvas_2d::started_animation_until(scene),
        ]
        .into_iter()
        .flatten()
        .reduce(f64::max);
        let timed = mappers::game_canvas_2d::has_active_fx(scene)
            || mappers::emitter::has_emitter(scene)
            || mappers::text_reveal::has_reveal(scene)
            || mappers::game_canvas_2d::has_started_animation(scene);
        let root = scene.get("props").unwrap_or(scene);
        let materials = ["drawables", "children"]
            .into_iter()
            .filter_map(|key| root.get(key).and_then(serde_json::Value::as_array))
            .flatten()
            .filter_map(|drawable| {
                let (live, anchored) = clocked(drawable);
                (live || anchored).then(|| (live, anchored, placed_rows(drawable)))
            })
            .collect();
        Self {
            until,
            timed,
            materials,
        }
    }

    /// Whether a fx, emitter, reveal or `startMs` animation still runs at `epoch_ms`.
    pub(crate) fn live_at(&self, epoch_ms: f64) -> bool {
        self.until.is_some_and(|until| epoch_ms < until)
    }
}

/// Put `scene` in the render target `id`, wherever the command tree holds it.
fn replace_target_scene(
    commands: &mut [DrawCommand],
    id: &str,
    scene: orbital_canvas_base::scene3d::Scene3D,
) -> bool {
    let mut scene = Some(scene);
    replace_in(commands, id, &mut scene)
}

fn replace_in(
    commands: &mut [DrawCommand],
    id: &str,
    scene: &mut Option<orbital_canvas_base::scene3d::Scene3D>,
) -> bool {
    for command in commands {
        match command {
            DrawCommand::Target {
                id: target,
                commands,
                ..
            } if target == id => {
                let Some(scene) = scene.take() else {
                    return true;
                };
                *commands = vec![DrawCommand::Scene3D {
                    scene: Box::new(scene),
                }];
                return true;
            }
            DrawCommand::Target { commands, .. }
            | DrawCommand::Shadow { commands, .. }
            | DrawCommand::AdditiveGroup { commands }
            | DrawCommand::Faded { commands, .. }
            | DrawCommand::ClipGroup { commands, .. } => {
                if replace_in(commands, id, scene) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// Whether `drawable` or anything under it paints a live material or live target, and whether one carries an anchor.
fn clocked(drawable: &serde_json::Value) -> (bool, bool) {
    let flag = |pointer: &str| {
        drawable
            .pointer(pointer)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    };
    let mut live = flag("/material/live") || flag("/target/live");
    let mut anchored = drawable
        .pointer("/material/anchor")
        .is_some_and(|anchor| anchor.as_f64().is_some());
    for key in ["props", "drawables", "children", "items"] {
        match drawable.get(key) {
            Some(serde_json::Value::Array(items)) => {
                for item in items {
                    let (l, a) = clocked(item);
                    live |= l;
                    anchored |= a;
                }
            }
            Some(child @ serde_json::Value::Object(_)) => {
                let (l, a) = clocked(child);
                live |= l;
                anchored |= a;
            }
            _ => {}
        }
    }
    (live, anchored)
}

/// The rows a top-level drawable covers and whether it is fixed, when that is plain to read: a `draw-shape` anchored
/// top-left with a numeric place and height, no animation and nothing nested.
fn placed_rows(drawable: &serde_json::Value) -> Option<(f32, f32, bool)> {
    let text = |key: &str| drawable.get(key).and_then(serde_json::Value::as_str);
    if text("type") != Some("draw-shape") || text("anchor") != Some("top-left") {
        return None;
    }
    if [
        "animation",
        "rotate",
        "props",
        "drawables",
        "children",
        "items",
    ]
    .into_iter()
    .any(|key| drawable.get(key).is_some())
    {
        return None;
    }
    let top = drawable.pointer("/position/y")?.as_f64()? as f32;
    let height = drawable.get("height")?.as_f64()? as f32;
    let fixed = drawable
        .get("fixed")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    Some((top, top + height, fixed))
}
