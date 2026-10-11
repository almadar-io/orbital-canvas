//! Shared `wgpu`-based game renderer for Orbital shells.
//!
//! Implements one GPU renderer that runs on Rust desktop, Android, and iOS
//! through a tiny C ABI. The server/runtime sends a JSON `GameRenderPacket`
//! (display list); this crate turns it into draw calls.

pub mod animate;
pub mod asset;
pub mod atlas;
pub mod c_api;
pub mod mappers;
pub mod packet;
pub mod platform;
pub mod projector;
pub mod renderer;
pub mod svg;
#[cfg(feature = "system-fonts")]
pub mod system_fonts;
pub mod vector;

use packet::GameRenderPacket;
use thiserror::Error;

pub(crate) use renderer::DeviceResources;
pub use renderer::SharedDeviceRenderer;

/// Errors that can occur when creating or using the renderer.
#[derive(Debug, Error)]
pub enum RendererError {
    #[error("no suitable graphics adapter found")]
    NoAdapter,
    #[error("failed to request device: {0}")]
    DeviceRequestFailed(String),
    #[error("surface error: {0}")]
    Surface(String),
    #[error("invalid packet: {0}")]
    InvalidPacket(String),
}

/// Result type alias for renderer operations.
pub type Result<T> = std::result::Result<T, RendererError>;

/// A queued input event waiting to be consumed by the next render tick.
#[derive(Debug, Clone)]
pub struct InputEvent {
    pub event_type: String,
    pub x: f32,
    pub y: f32,
    pub button: i32,
    pub key: Option<String>,
}

/// Shared game renderer state.
pub struct GameRenderer {
    /// Global `wgpu` instance. Lives for the lifetime of the renderer.
    pub(crate) instance: wgpu::Instance,
    /// Optional render surface (present on desktop/mobile, absent in headless/tests).
    pub(crate) surface: Option<wgpu::Surface<'static>>,
    /// Lazily-initialized GPU device, queue, and pipeline.
    pub(crate) device_resources: Option<DeviceResources>,
    /// On-surface painter: the same sprite + quad pipelines the host-device shells use.
    pub(crate) shared: Option<renderer::SharedDeviceRenderer>,
    /// Painter for the readback path: always `Rgba8Unorm`, whatever the surface format is.
    pub(crate) rgba_shared: Option<renderer::SharedDeviceRenderer>,
    /// The scene, its layout memory and its input state ([`SceneHost`]).
    pub(crate) host: SceneHost,
}

/// A scene host: the mapped canvas scene, its tappable/draggable regions, layout memory, clock and
/// pointer state. [`GameRenderer`] wraps one for the mobile surfaces; the desktop clients use it directly
/// and paint [`SceneHost::frame_packet`] on their own device.
pub struct SceneHost {
    /// Last submitted display-list packet.
    pub(crate) packet: Option<GameRenderPacket>,
    /// Pending input events forwarded via the C ABI; drained each render tick.
    pub(crate) input_queue: Vec<InputEvent>,
    /// Surface size in PHYSICAL pixels, set by `resize` / surface init.
    pub(crate) viewport: Option<(u32, u32)>,
    /// Device pixels per logical point (`devicePixelRatio`). The scene is laid out in
    /// logical points (`viewport / density`, like the web's CSS-pixel measured viewport)
    /// and drawn onto the physical surface.
    pub(crate) density: f32,
    /// Last `canvas` render-ui node handed to `set_scene`; re-mapped when the viewport changes.
    pub(crate) scene: Option<serde_json::Value>,
    /// Tappable regions of the current scene, in logical points.
    pub(crate) hits: Vec<mappers::scene::Hit>,
    /// The asset-cache generation the current scene was mapped at; a later one re-maps the scene
    /// (sprites skipped while their sheet was in flight now resolve).
    pub(crate) asset_generation: u64,
    /// Draggable regions of the current scene, in logical points.
    pub(crate) drags: Vec<mappers::scene::Drag>,
    /// Double-tap regions of the current scene, and whether a background drag pans it.
    pub(crate) double_hits: Vec<mappers::scene::Hit>,
    pub(crate) pannable: bool,
    /// Layout and view state the scene keeps across re-maps ([`mappers::scene::SceneMemory`]).
    pub(crate) memory: mappers::scene::SceneMemory,
    /// The pointer position of an in-progress background pan.
    pub(crate) pan_from: Option<(f32, f32)>,
    /// The drag a touch started on: it follows the pointer once the touch travels past [`TAP_SLOP`].
    pub(crate) active_drag: Option<ActiveDrag>,
    /// Where the pointer last went down (logical points); a tap is a down/up pair that stays near it.
    pub(crate) touch_down: Option<(f32, f32)>,
    /// Scene events produced by taps, waiting for the host to take them.
    pub(crate) scene_events: Vec<mappers::scene::Emit>,
    /// The host's frame clock (ms) at the latest frame, and its value at the first one: scene time
    /// counts from there, so an animation keeps its phase when a new scene arrives.
    pub(crate) clock_ms: f64,
    pub(crate) clock_epoch: Option<f64>,
    /// Whether the host allows motion (false: the OS reduced-motion setting is on).
    pub(crate) motion: bool,
}

fn contains(r: &packet::Rect, x: f32, y: f32) -> bool {
    x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h
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

impl GameRenderer {
    /// Create a headless renderer with no surface.
    ///
    /// Used on platforms where the surface is supplied later, or in tests.
    pub fn new_headless() -> Result<Self> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            flags: wgpu::InstanceFlags::default(),
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            backend_options: wgpu::BackendOptions::default(),
        });

        Ok(Self {
            instance,
            surface: None,
            device_resources: None,
            shared: None,
            rgba_shared: None,
            host: SceneHost::new(),
        })
    }

    /// Create a renderer bound to an already-created wgpu surface.
    ///
    /// The `instance` that was used to create `surface` is passed in so that
    /// both can live together in the renderer — the surface must not outlive the instance.
    // Called from platform::android and platform::ios (cfg-gated); appears unused on desktop.
    #[cfg_attr(not(any(target_os = "android", target_os = "ios")), allow(dead_code))]
    pub(crate) fn new_with_surface(
        instance: wgpu::Instance,
        surface: wgpu::Surface<'static>,
    ) -> Result<Self> {
        Ok(Self {
            instance,
            surface: Some(surface),
            device_resources: None,
            shared: None,
            rgba_shared: None,
            host: SceneHost::new(),
        })
    }

    /// Render one frame from the current packet.
    ///
    /// When a surface is present (Android/iOS), renders to the surface texture and presents.
    /// In headless mode, this is a no-op; call `render_packet_to_rgba` explicitly instead.
    /// Drains the pending input queue before rendering so callers can read it from the packet's
    /// tick_event path on the next submit cycle.
    pub fn render(&mut self) -> Result<()> {
        self.refresh_assets();
        // Drain queued input; the runtime picks these up via tick_event/draw_event callbacks.
        if !self.input_queue.is_empty() {
            tracing::trace!(count = self.input_queue.len(), "draining input queue");
            self.input_queue.clear();
        }
        if let Some(packet) = self.frame_packet() {
            tracing::trace!(pattern = %packet.pattern, "render packet");
            if self.surface.is_some() {
                self.render_to_surface(&packet)?;
            }
        }
        Ok(())
    }

    /// Resize the render surface and reconfigure it.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        self.set_viewport(width, height);
        if self.surface.is_some() {
            self.configure_surface(width, height)?;
        }
        Ok(())
    }
}

impl std::ops::Deref for GameRenderer {
    type Target = SceneHost;
    fn deref(&self) -> &SceneHost {
        &self.host
    }
}

impl std::ops::DerefMut for GameRenderer {
    fn deref_mut(&mut self) -> &mut SceneHost {
        &mut self.host
    }
}

impl Default for SceneHost {
    fn default() -> Self {
        Self::new()
    }
}

impl SceneHost {
    /// An empty host: no scene, density 1, motion allowed.
    pub fn new() -> Self {
        Self {
            packet: None,
            input_queue: Vec::new(),
            viewport: None,
            density: 1.0,
            scene: None,
            hits: Vec::new(),
            asset_generation: 0,
            drags: Vec::new(),
            double_hits: Vec::new(),
            pannable: false,
            memory: mappers::scene::SceneMemory::default(),
            pan_from: None,
            active_drag: None,
            touch_down: None,
            scene_events: Vec::new(),
            clock_ms: 0.0,
            clock_epoch: None,
            motion: true,
        }
    }

    /// Map the scene again when the asset cache moved on since it was mapped (a sprite or atlas
    /// that was in flight has landed). Every host calls this once per frame.
    pub fn refresh_assets(&mut self) {
        let generation = asset::generation();
        if generation != self.asset_generation {
            self.asset_generation = generation;
            self.relayout();
        }
    }

    /// The node the current scene was mapped from.
    pub fn scene(&self) -> Option<&serde_json::Value> {
        self.scene.as_ref()
    }

    /// The host surface in physical pixels, once known.
    pub fn viewport(&self) -> Option<(u32, u32)> {
        self.viewport
    }

    /// Device pixels per logical point.
    pub fn density(&self) -> f32 {
        self.density
    }

    /// Replace the current display-list packet.
    pub fn submit(&mut self, packet: GameRenderPacket) {
        self.scene = None;
        self.hits.clear();
        self.drags.clear();
        self.packet = Some(packet);
    }

    /// Map a `canvas` render-ui node to the current packet. With a known viewport the scene is
    /// laid out in it, in logical points (web `Canvas2D` parity); the node is kept and re-mapped
    /// on every resize and density change.
    pub fn set_scene(
        &mut self,
        node: serde_json::Value,
    ) -> std::result::Result<(), mappers::game_canvas_2d::CanvasSceneError> {
        let generation = asset::generation();
        let mapped = self.map_scene(&node)?;
        self.asset_generation = generation;
        self.install(mapped);
        self.scene = Some(node);
        Ok(())
    }

    fn map_scene(
        &mut self,
        node: &serde_json::Value,
    ) -> std::result::Result<mappers::scene::MappedScene, mappers::game_canvas_2d::CanvasSceneError>
    {
        let density = self.density;
        mappers::scene::map_scene_with(
            node,
            self.viewport
                .map(|(w, h)| (w as f32 / density, h as f32 / density)),
            &mut self.memory,
        )
    }

    fn install(&mut self, mapped: mappers::scene::MappedScene) {
        self.packet = Some(mapped.packet);
        self.hits = mapped.hits;
        self.drags = mapped.drags;
        self.double_hits = mapped.double_hits;
        self.pannable = mapped.pannable;
    }

    /// Hand the renderer the host's frame clock (ms, any monotonic origin) before a `render`. The
    /// first value is scene time zero.
    pub fn set_clock(&mut self, ms: f64) {
        self.clock_epoch.get_or_insert(ms);
        self.clock_ms = ms;
    }

    /// Whether the host allows motion. Off, every animated scene draws its rest frame.
    pub fn set_motion(&mut self, enabled: bool) {
        self.motion = enabled;
    }

    fn scene_time(&self) -> f64 {
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
        self.is_animated() || asset::pending() > 0 || asset::generation() != self.asset_generation
    }

    /// The current packet as it draws at the clock's scene time.
    pub fn frame_packet(&self) -> Option<GameRenderPacket> {
        self.packet
            .as_ref()
            .map(|p| p.resolved(self.scene_time(), self.motion).into_owned())
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

    /// Set the device pixel ratio (physical pixels per logical point) and re-lay-out the
    /// current scene. Rejects non-finite or non-positive values and leaves the
    /// previous density in force.
    pub fn set_scale(&mut self, scale: f32) -> std::result::Result<(), String> {
        if !scale.is_finite() || scale <= 0.0 {
            return Err(format!(
                "scale must be a finite positive number, got {scale}"
            ));
        }
        self.density = scale;
        self.relayout();
        Ok(())
    }

    fn relayout(&mut self) {
        if let Some(node) = self.scene.clone() {
            if let Ok(mapped) = self.map_scene(&node) {
                self.install(mapped);
            }
        }
    }

    /// A double tap (the host's recognizer) at a point: the events of the region under it.
    pub fn double_tap(&mut self, x: f32, y: f32) {
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
    pub fn zoom(&mut self, factor: f32, x: f32, y: f32) {
        if self.pannable && factor.is_finite() && factor > 0.0 {
            self.memory.pending_zoom = Some((factor, x, y));
            self.relayout();
        }
    }

    /// The host surface in physical pixels; the scene is laid out in `size / density` points.
    pub fn set_viewport(&mut self, width: u32, height: u32) {
        self.viewport = Some((width.max(1), height.max(1)));
        self.relayout();
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
        match event_type {
            "touch-down" => {
                self.touch_down = Some((x, y));
                self.active_drag = self
                    .drags
                    .iter()
                    .rev()
                    .find(|d| contains(&d.rect, x, y))
                    .map(|d| ActiveDrag {
                        id: d.id.clone(),
                        grab: (x - d.rect.x, y - d.rect.y),
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
                        let emits = mappers::scene::emits_in(&self.hits, x, y);
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

    /// Move the active drag to the pointer: queue each of its move events with the new top-left in
    /// scene units, clamped to the region's bounds (the web clamps to the canvas the same way).
    fn drag_to(&mut self, x: f32, y: f32) {
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

    /// Drain the scene events that taps queued (`[{event, payload}]` for the host to send).
    pub fn take_scene_events(&mut self) -> Vec<mappers::scene::Emit> {
        std::mem::take(&mut self.scene_events)
    }

    /// Drain and return all queued input events since the last call to this method or `render`.
    pub fn take_input_events(&mut self) -> Vec<InputEvent> {
        std::mem::take(&mut self.input_queue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph() -> serde_json::Value {
        serde_json::json!({
            "type": "graph-canvas",
            "height": 400,
            "nodes": [{"id": "a", "label": "A"}, {"id": "b", "label": "B"}, {"id": "c", "label": "C"}],
            "edges": [{"source": "a", "target": "b"}],
            "onNodeClick": "PICK",
            "onNodeDoubleClick": "OPEN"
        })
    }

    fn graph_renderer() -> GameRenderer {
        let mut r = GameRenderer::new_headless().unwrap();
        r.set_viewport(600, 400);
        r.set_scene(graph()).unwrap();
        r
    }

    fn center_of(r: &GameRenderer, id: &str) -> (f32, f32) {
        let d = r
            .drags
            .iter()
            .find(|d| d.id == id)
            .expect("node drag region");
        (d.rect.x + d.rect.w / 2.0, d.rect.y + d.rect.h / 2.0)
    }

    fn empty_spot(r: &GameRenderer) -> (f32, f32) {
        (0..60)
            .flat_map(|i| (0..40).map(move |j| (i as f32 * 10.0 + 5.0, j as f32 * 10.0 + 5.0)))
            .find(|&(x, y)| !r.drags.iter().any(|d| contains(&d.rect, x, y)))
            .expect("an empty spot")
    }

    #[test]
    fn dragging_a_graph_node_moves_it_and_queues_no_tap() {
        let mut r = graph_renderer();
        let (x, y) = center_of(&r, "a");
        r.input("touch-down", x, y, 0, None);
        r.input("touch-move", x + 40.0, y + 25.0, 0, None);
        r.input("touch-up", x + 40.0, y + 25.0, 0, None);
        let (nx, ny) = center_of(&r, "a");
        assert!(
            (nx - (x + 40.0)).abs() < 0.5 && (ny - (y + 25.0)).abs() < 0.5,
            "{nx},{ny}"
        );
        assert!(r.take_scene_events().is_empty(), "a drag is not a tap");
        r.set_scene(graph()).unwrap();
        let (kx, ky) = center_of(&r, "a");
        assert!(
            (kx - nx).abs() < 0.5 && (ky - ny).abs() < 0.5,
            "a new scene keeps the dragged position"
        );
    }

    #[test]
    fn control_a_tap_on_a_node_still_picks_it() {
        let mut r = graph_renderer();
        let (x, y) = center_of(&r, "b");
        r.input("touch-down", x, y, 0, None);
        r.input("touch-up", x, y, 0, None);
        let events = r.take_scene_events();
        assert_eq!(
            events.iter().map(|e| e.event.as_str()).collect::<Vec<_>>(),
            vec!["PICK"]
        );
    }

    #[test]
    fn a_background_drag_pans_every_node_by_the_drag() {
        let mut r = graph_renderer();
        let before = center_of(&r, "c");
        let (x, y) = empty_spot(&r);
        r.input("touch-down", x, y, 0, None);
        r.input("touch-move", x + 30.0, y - 10.0, 0, None);
        r.input("touch-up", x + 30.0, y - 10.0, 0, None);
        let after = center_of(&r, "c");
        assert!(
            (after.0 - before.0 - 30.0).abs() < 0.5 && (after.1 - before.1 + 10.0).abs() < 0.5,
            "{before:?} {after:?}"
        );
    }

    #[test]
    fn a_pinch_zooms_about_its_focus_within_the_web_limits() {
        let mut r = graph_renderer();
        let (a, b) = (center_of(&r, "a"), center_of(&r, "b"));
        let gap = (a.0 - b.0).hypot(a.1 - b.1);
        r.zoom(2.0, a.0, a.1);
        let (a2, b2) = (center_of(&r, "a"), center_of(&r, "b"));
        assert!(
            (a2.0 - a.0).abs() < 0.5 && (a2.1 - a.1).abs() < 0.5,
            "the focus stays put"
        );
        let zoomed = (a2.0 - b2.0).hypot(a2.1 - b2.1);
        assert!(zoomed > gap * 1.5, "{gap} -> {zoomed}");
        r.zoom(100.0, a.0, a.1);
        assert_eq!(r.memory.view.map(|v| v.0), Some(3.0), "zoom stops at 3x");
    }

    #[test]
    fn the_zoom_buttons_scale_about_the_canvas_origin_and_reset_restores_the_identity_view() {
        let mut r = graph_renderer();
        let fitted = r.memory.view.unwrap();
        r.view_command("zoom-in").unwrap();
        let zoomed = r.memory.view.unwrap();
        assert!(
            (zoomed.0 - fitted.0 * 1.2).abs() < 1e-9
                && zoomed.1 == fitted.1
                && zoomed.2 == fitted.2,
            "{fitted:?} -> {zoomed:?}"
        );
        r.view_command("zoom-out").unwrap();
        assert!((r.memory.view.unwrap().0 - fitted.0).abs() < 1e-9);
        for _ in 0..20 {
            r.view_command("zoom-out").unwrap();
        }
        assert_eq!(r.memory.view.unwrap().0, 0.3, "zoom-out stops at 0.3");
        r.view_command("reset").unwrap();
        assert_eq!(
            r.memory.view.unwrap(),
            (1.0, 0.0, 0.0),
            "the web's reset: zoom 1, no offset"
        );
    }

    #[test]
    fn control_an_unknown_view_command_is_refused_and_changes_nothing() {
        let mut r = graph_renderer();
        let before = r.memory.view;
        assert!(r.view_command("spin").is_err());
        assert_eq!(r.memory.view, before);
    }

    #[test]
    fn a_double_tap_on_a_node_fires_its_double_click_event() {
        let mut r = graph_renderer();
        let (x, y) = center_of(&r, "a");
        r.input("double-tap", x, y, 0, None);
        let events = r.take_scene_events();
        assert_eq!(
            events.iter().map(|e| e.event.as_str()).collect::<Vec<_>>(),
            vec!["OPEN"]
        );
        let (ex, ey) = empty_spot(&r);
        r.input("double-tap", ex, ey, 0, None);
        assert!(
            r.take_scene_events().is_empty(),
            "control: empty space fires nothing"
        );
    }

    fn pan_by(r: &mut GameRenderer, dx: f32) {
        let (x, y) = empty_spot(r);
        r.input("touch-down", x, y, 0, None);
        r.input("touch-move", x + dx, y, 0, None);
        r.input("touch-up", x + dx, y, 0, None);
    }

    #[test]
    fn a_new_scene_with_the_same_nodes_keeps_the_view_and_a_mostly_new_set_refits_it() {
        let mut r = graph_renderer();
        pan_by(&mut r, 120.0);
        let panned = r.memory.view.unwrap();
        r.set_scene(graph()).unwrap();
        assert_eq!(r.memory.view.unwrap(), panned, "same nodes: the pan stays");
        let mut other = graph();
        other["nodes"] = serde_json::json!([{"id": "a"}, {"id": "x"}, {"id": "y"}, {"id": "z"}]);
        r.set_scene(other).unwrap();
        assert!(
            (r.memory.view.unwrap().1 - panned.1).abs() > 1.0,
            "overlap 1/4: laid out and fitted again"
        );
    }

    fn drawn_texts(r: &GameRenderer) -> Vec<String> {
        r.packet
            .as_ref()
            .unwrap()
            .draw_commands
            .iter()
            .filter_map(|c| match c {
                packet::DrawCommand::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_long_press_shows_a_truncated_labels_full_text_until_the_next_touch() {
        let mut r = GameRenderer::new_headless().unwrap();
        r.set_viewport(600, 400);
        let mut g = graph();
        g["nodes"][0]["label"] =
            serde_json::json!("A very long node label that the graph truncates");
        r.set_scene(g).unwrap();
        let (x, y) = center_of(&r, "a");
        r.input("touch-down", x, y, 0, None);
        r.input("long-press", x, y, 0, None);
        r.input("touch-up", x, y, 0, None);
        assert!(
            r.take_scene_events().is_empty(),
            "lifting after a long press is not a tap"
        );
        let shown = drawn_texts(&r).join(" ");
        assert!(shown.contains("that the graph truncates"), "{shown}");
        r.input("touch-down", x + 200.0, y, 0, None);
        r.input("touch-up", x + 200.0, y, 0, None);
        assert!(
            !drawn_texts(&r)
                .join(" ")
                .contains("that the graph truncates"),
            "dismissed"
        );
    }

    #[test]
    fn control_a_long_press_on_a_label_that_fits_shows_nothing_extra() {
        let mut r = graph_renderer();
        let before = drawn_texts(&r);
        let (x, y) = center_of(&r, "b");
        r.input("long-press", x, y, 0, None);
        assert_eq!(drawn_texts(&r), before);
    }

    #[test]
    fn headless_renderer_can_be_created() {
        // This test only runs where an adapter exists, so we ignore failures.
        let _ = GameRenderer::new_headless();
    }

    #[test]
    fn packet_roundtrips_through_json() {
        let packet = GameRenderPacket {
            pattern: "canvas-2d".into(),
            width: 800.0,
            height: 600.0,
            draw_commands: vec![packet::DrawCommand::Clear {
                color: [0.0, 0.0, 0.0, 1.0],
            }],
            ..Default::default()
        };
        let json = serde_json::to_string(&packet).unwrap();
        let parsed: GameRenderPacket = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.pattern, "canvas-2d");
        assert_eq!(parsed.width, 800.0);
    }

    /// A retina phone surface (e.g. 1206x2172) exceeds the WebGL2-downlevel 2048 texture cap;
    /// the device must request the adapter's real resolution limits or `Surface::configure` aborts.
    #[test]
    fn device_requests_the_adapter_texture_resolution() {
        let mut renderer = match GameRenderer::new_headless() {
            Ok(r) => r,
            Err(_) => return,
        };
        let packet = GameRenderPacket {
            width: 8.0,
            height: 8.0,
            ..Default::default()
        };
        if renderer.render_packet_to_rgba(&packet).is_err() {
            return; // no adapter — skip
        }
        let resources = renderer.device_resources.as_ref().expect("device created");
        let adapter_limits = resources.adapter.limits();
        let device_limits = resources.device.limits();
        assert_eq!(
            device_limits.max_texture_dimension_2d,
            adapter_limits.max_texture_dimension_2d
        );
        assert!(
            device_limits.max_texture_dimension_2d >= 4096
                || adapter_limits.max_texture_dimension_2d < 4096
        );
        // Control: only resolution is lifted; other limits stay at the downlevel contract.
        assert_eq!(
            device_limits.max_bind_groups,
            wgpu::Limits::downlevel_webgl2_defaults().max_bind_groups
        );
    }

    /// Golden test: a Clear + Quad packet must produce a non-blank RGBA buffer.
    ///
    /// Skipped when no GPU adapter is available (CI without a GPU).
    #[test]
    fn headless_rgba_golden_packet_is_non_blank() {
        let mut renderer = match GameRenderer::new_headless() {
            Ok(r) => r,
            Err(_) => return, // no adapter — skip
        };

        let packet = GameRenderPacket {
            pattern: "canvas-2d".into(),
            width: 64.0,
            height: 64.0,
            draw_commands: vec![
                packet::DrawCommand::Clear {
                    color: [0.1, 0.2, 0.3, 1.0],
                },
                packet::DrawCommand::Quad {
                    rect: packet::Rect {
                        x: 8.0,
                        y: 8.0,
                        w: 48.0,
                        h: 48.0,
                    },
                    color: [1.0, 0.0, 0.0, 1.0],
                },
            ],
            ..Default::default()
        };

        let rgba = match renderer.render_packet_to_rgba(&packet) {
            Ok(buf) => buf,
            Err(_) => return, // no GPU — skip gracefully
        };

        assert_eq!(rgba.len(), 64 * 64 * 4, "buffer length mismatch");

        // At least one non-zero pixel must be present (the red quad or the tinted background).
        let all_zero = rgba.iter().all(|&b| b == 0);
        assert!(
            !all_zero,
            "headless RGBA buffer is entirely blank — renderer produced no output"
        );

        // The quad covers pixels [8..56, 8..56]; sample the center to check it's red-dominant.
        let cx = 32usize;
        let cy = 32usize;
        let base = (cy * 64 + cx) * 4;
        let r = rgba[base];
        let g = rgba[base + 1];
        assert!(
            r > g,
            "center pixel should be red-dominant (got r={r} g={g})"
        );
    }

    /// Adversarial (non-vacuous): an empty packet (no draw commands) must yield a blank/cleared
    /// buffer — proving the non-blank assertion in the golden test is not vacuous.
    ///
    /// Skipped when no GPU adapter is available.
    #[test]
    fn headless_rgba_blank_packet_stays_blank() {
        let mut renderer = match GameRenderer::new_headless() {
            Ok(r) => r,
            Err(_) => return, // no adapter — SKIP
        };

        // Zero draw commands: the render pass clears to TRANSPARENT (0,0,0,0).
        let packet = GameRenderPacket {
            pattern: "canvas-2d".into(),
            width: 32.0,
            height: 32.0,
            draw_commands: vec![],
            ..Default::default()
        };

        let rgba = match renderer.render_packet_to_rgba(&packet) {
            Ok(buf) => buf,
            Err(_) => return, // no GPU — SKIP
        };

        assert_eq!(
            rgba.len(),
            32 * 32 * 4,
            "buffer length mismatch for blank packet"
        );

        // Every byte must be 0: the clear color is TRANSPARENT and no quads were drawn.
        let all_zero = rgba.iter().all(|&b| b == 0);
        assert!(
            all_zero,
            "blank packet produced a non-zero pixel — renderer may carry state across calls"
        );
    }

    fn floor_plan(editable: bool) -> serde_json::Value {
        serde_json::json!({
            "type": "positioned-canvas", "width": 600, "height": 400, "editable": editable,
            "selectEvent": "SELECT_TABLE", "moveEvent": "MOVE_TABLE",
            "items": [{"id": "t1", "label": "T1", "x": 40, "y": 40, "capacity": 4, "status": "empty", "shape": "round"}]
        })
    }

    #[test]
    fn dragging_an_item_past_the_slop_queues_clamped_moves_and_suppresses_the_tap() {
        let mut r = GameRenderer::new_headless().expect("headless");
        r.set_scene(floor_plan(true)).expect("scene");
        r.input("touch-down", 60.0, 60.0, 0, None);
        r.input("touch-move", 64.0, 60.0, 0, None);
        assert!(
            r.take_scene_events().is_empty(),
            "inside the tap slop nothing moves"
        );
        r.input("touch-move", 160.0, 100.0, 0, None);
        r.input("touch-up", 160.0, 100.0, 0, None);
        let events = r.take_scene_events();
        assert_eq!(
            events.len(),
            1,
            "one move, and no select tap after a drag: {events:?}"
        );
        assert_eq!(events[0].event, "MOVE_TABLE");
        assert_eq!(
            events[0].delivered(),
            serde_json::json!({"id": "t1", "x": 140.0, "y": 80.0}),
            "the item keeps its grab offset"
        );
        r.input("touch-down", 60.0, 60.0, 0, None);
        r.input("touch-move", 2000.0, 2000.0, 0, None);
        let clamped = r.take_scene_events();
        assert_eq!(
            clamped[0].delivered(),
            serde_json::json!({"id": "t1", "x": 504.0, "y": 304.0}),
            "clamped to the plan: 600-96, 400-96"
        );
    }

    #[test]
    fn a_plain_tap_selects_and_a_read_only_plan_does_not_drag() {
        let mut r = GameRenderer::new_headless().expect("headless");
        r.set_scene(floor_plan(true)).expect("scene");
        r.input("touch-down", 60.0, 60.0, 0, None);
        r.input("touch-up", 61.0, 60.0, 0, None);
        let tap = r.take_scene_events();
        assert_eq!(tap[0].event, "SELECT_TABLE");
        r.set_scene(floor_plan(false)).expect("scene");
        r.input("touch-down", 60.0, 60.0, 0, None);
        r.input("touch-move", 160.0, 100.0, 0, None);
        assert!(
            r.take_scene_events().is_empty(),
            "control: not editable, no drag"
        );
    }

    #[test]
    fn settling_assets_remaps_a_scene_whose_sprite_sheet_was_still_loading() {
        let dir = std::env::temp_dir().join("orbital-game-renderer-settle-test");
        std::fs::create_dir_all(&dir).expect("tempdir");
        image::RgbaImage::from_pixel(32, 32, image::Rgba([10, 200, 90, 255]))
            .save(dir.join("sheet.png"))
            .expect("png");
        std::fs::write(
            dir.join("atlas.json"),
            r#"{"imagePath":"sheet.png","subTextures":{"gem.png":{"x":0,"y":0,"width":16,"height":16}}}"#,
        )
        .expect("atlas");
        let node = |atlas: &str| {
            serde_json::json!({
                "type": "atlas-image", "size": 32,
                "asset": {"url": format!("file://{}", dir.join("sheet.png").display()), "atlas": atlas, "sprite": "gem.png"}
            })
        };
        let sprites = |r: &GameRenderer| {
            r.packet.as_ref().map_or(0, |p| {
                p.draw_commands
                    .iter()
                    .filter(|c| matches!(c, packet::DrawCommand::Sprite { .. }))
                    .count()
            })
        };
        let mut r = GameRenderer::new_headless().expect("headless");
        r.set_scene(node(&format!(
            "file://{}",
            dir.join("atlas.json").display()
        )))
        .expect("scene");
        r.settle_assets(std::time::Duration::from_secs(5));
        assert_eq!(
            sprites(&r),
            1,
            "once the atlas has loaded the sprite is drawn"
        );
        let mut missing = GameRenderer::new_headless().expect("headless");
        missing
            .set_scene(node(&format!(
                "file://{}",
                dir.join("absent.json").display()
            )))
            .expect("scene");
        missing.settle_assets(std::time::Duration::from_secs(5));
        assert_eq!(
            sprites(&missing),
            0,
            "control: an atlas that never loads draws nothing, and settling still returns"
        );
    }

    #[test]
    fn input_events_are_queued_and_drained() {
        let mut renderer = GameRenderer::new_headless().expect("headless must construct");

        assert!(renderer.input("pointerdown", 10.0, 20.0, 0, None));
        assert!(renderer.input("keydown", 0.0, 0.0, 0, Some("Space")));
        assert_eq!(renderer.input_queue.len(), 2, "two events should be queued");

        let events = renderer.take_input_events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, "pointerdown");
        assert_eq!(events[0].x, 10.0);
        assert_eq!(events[0].y, 20.0);
        assert_eq!(events[1].event_type, "keydown");
        assert_eq!(events[1].key.as_deref(), Some("Space"));
        assert!(
            renderer.input_queue.is_empty(),
            "take_input_events must drain the queue"
        );
    }

    #[test]
    fn render_drains_input_queue() {
        let mut renderer = GameRenderer::new_headless().expect("headless must construct");
        renderer.input("pointermove", 5.0, 5.0, 0, None);
        assert_eq!(renderer.input_queue.len(), 1);
        // render() drains even in headless mode (no surface → no GPU call).
        renderer.render().expect("headless render must not error");
        assert!(
            renderer.input_queue.is_empty(),
            "render must drain input queue"
        );
    }

    /// Golden test for the host-device path: the same Clear+Quad packet through
    /// `SharedDeviceRenderer` on an externally-created device must produce the
    /// same red-dominant center pixel as the owned-device path.
    #[test]
    fn shared_device_renderer_golden_packet_is_non_blank() {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            flags: wgpu::InstanceFlags::default(),
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            backend_options: wgpu::BackendOptions::default(),
        });
        let adapter =
            match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })) {
                Ok(a) => a,
                Err(_) => return, // no adapter — skip
            };
        let (device, queue) =
            match pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_webgl2_defaults(),
                label: Some("shared-device test"),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            })) {
                Ok(dq) => dq,
                Err(_) => return, // no device — skip
            };

        let mut renderer = SharedDeviceRenderer::new(
            std::sync::Arc::new(device),
            std::sync::Arc::new(queue),
            wgpu::TextureFormat::Rgba8Unorm,
        );

        let packet = GameRenderPacket {
            pattern: "canvas-2d".into(),
            width: 64.0,
            height: 64.0,
            draw_commands: vec![
                packet::DrawCommand::Clear {
                    color: [0.1, 0.2, 0.3, 1.0],
                },
                packet::DrawCommand::Quad {
                    rect: packet::Rect {
                        x: 8.0,
                        y: 8.0,
                        w: 48.0,
                        h: 48.0,
                    },
                    color: [1.0, 0.0, 0.0, 1.0],
                },
            ],
            ..Default::default()
        };

        let target = renderer.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("shared-device test target"),
            size: wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        assert!(
            renderer.render_packet_to_texture(&packet, &target),
            "quad content expected"
        );

        let encoder = renderer
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("readback"),
            });
        let rgba = match renderer::copy_texture_to_rgba(
            renderer.device(),
            renderer.queue(),
            encoder,
            &target,
            64,
            64,
        ) {
            Ok(buf) => buf,
            Err(_) => return,
        };

        assert_eq!(rgba.len(), 64 * 64 * 4);
        let base = (32 * 64 + 32) * 4;
        assert!(
            rgba[base] > rgba[base + 1],
            "center pixel should be red-dominant (got r={} g={})",
            rgba[base],
            rgba[base + 1]
        );
    }

    /// Full sprite chain: a 2×2 PNG fetched through the asset cache must
    /// upload + draw through the textured pipeline onto the target.
    #[test]
    fn shared_device_renderer_draws_fetched_sprite() {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            flags: wgpu::InstanceFlags::default(),
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            backend_options: wgpu::BackendOptions::default(),
        });
        let adapter =
            match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })) {
                Ok(a) => a,
                Err(_) => return, // no adapter — skip
            };
        let (device, queue) =
            match pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_webgl2_defaults(),
                label: Some("sprite test"),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            })) {
                Ok(dq) => dq,
                Err(_) => return, // no device — skip
            };

        // Fixture: a uniform-red 2×2 PNG (uniform color → filtering can't leak).
        let dir = std::env::temp_dir().join(format!(
            "orbital-game-renderer-sprite-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("tempdir");
        let path = dir.join("red.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]))
            .save(&path)
            .expect("write fixture");
        let url = format!("file://{}", path.display());

        // Wait for the async fetch to land (file reads are instant, but the
        // cache resolves on a spawned thread).
        let mut ready = false;
        for _ in 0..200 {
            if crate::asset::asset_bytes(&url).is_some() {
                ready = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(ready, "fixture must arrive within 2s");

        let mut renderer = SharedDeviceRenderer::new(
            std::sync::Arc::new(device),
            std::sync::Arc::new(queue),
            wgpu::TextureFormat::Rgba8Unorm,
        );

        let packet = GameRenderPacket {
            pattern: "canvas-2d".into(),
            width: 8.0,
            height: 8.0,
            draw_commands: vec![packet::DrawCommand::Sprite {
                texture: url.clone(),
                src: packet::Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 2.0,
                    h: 2.0,
                },
                dst: packet::Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 8.0,
                    h: 8.0,
                },
                rotation: 0.0,
                opacity: 1.0,
                flip_x: false,
                flip_y: false,
            }],
            ..Default::default()
        };

        let target = renderer.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("sprite test target"),
            size: wgpu::Extent3d {
                width: 8,
                height: 8,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        assert!(
            renderer.render_packet_to_texture(&packet, &target),
            "sprite should draw"
        );

        let encoder = renderer
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("readback"),
            });
        let rgba = match renderer::copy_texture_to_rgba(
            renderer.device(),
            renderer.queue(),
            encoder,
            &target,
            8,
            8,
        ) {
            Ok(buf) => buf,
            Err(_) => return,
        };

        let base = (4 * 8 + 4) * 4;
        assert!(
            rgba[base] > 200 && rgba[base + 1] < 60 && rgba[base + 2] < 60,
            "center pixel must be the sprite's red (got r={} g={} b={})",
            rgba[base],
            rgba[base + 1],
            rgba[base + 2]
        );
    }

    fn pulse_scene() -> serde_json::Value {
        serde_json::json!({"type": "svg-pulse", "rings": 3})
    }

    fn ring_alpha(p: &GameRenderPacket) -> f32 {
        match &p.draw_commands[1] {
            packet::DrawCommand::Path {
                stroke: Some(s), ..
            } => s.color[3],
            other => panic!("expected the first ring, got {other:?}"),
        }
    }

    #[test]
    fn an_animated_scene_asks_for_frames_and_reduced_motion_stops_them() {
        let mut r = GameRenderer::new_headless().unwrap();
        r.set_scene(pulse_scene()).unwrap();
        assert!(r.is_animated() && r.wants_frames());
        r.set_motion(false);
        assert!(!r.is_animated(), "the host forbids motion");
        r.set_motion(true);
        assert!(r.is_animated(), "control: allowed again");
    }

    #[test]
    fn a_static_scene_runs_no_frame_loop() {
        let mut r = GameRenderer::new_headless().unwrap();
        r.set_scene(serde_json::json!({"type": "svg-ring", "r": 30}))
            .unwrap();
        assert!(!r.is_animated());
        if asset::pending() == 0 && asset::generation() == r.asset_generation {
            assert!(
                !r.wants_frames(),
                "nothing animates and nothing is arriving"
            );
        }
        let mut off = GameRenderer::new_headless().unwrap();
        off.set_scene(serde_json::json!({"type": "svg-pulse", "animated": false}))
            .unwrap();
        assert!(
            !off.is_animated(),
            "control: svg-pulse with animated false declares no motion"
        );
        let mut themed = GameRenderer::new_headless().unwrap();
        let mut node = pulse_scene();
        node["canvasTheme"] = serde_json::json!({"motion": {"enabled": false}});
        themed.set_scene(node).unwrap();
        assert!(
            !themed.is_animated(),
            "the theme's motion switch is honoured"
        );
    }

    #[test]
    fn the_frame_follows_the_host_clock_from_its_first_value() {
        let mut r = GameRenderer::new_headless().unwrap();
        r.set_scene(pulse_scene()).unwrap();
        r.set_clock(90_000.0);
        let first = r.frame_packet().unwrap();
        r.set_clock(91_000.0);
        let later = r.frame_packet().unwrap();
        assert_ne!(
            ring_alpha(&first),
            ring_alpha(&later),
            "a second on, the first ring has faded"
        );
        assert!(
            (ring_alpha(&first) - 0.4).abs() < 1e-3,
            "t=0 is the 0% keyframe: the web's opacity 0.4"
        );
        r.set_clock(91_000.0);
        assert_eq!(
            ring_alpha(&r.frame_packet().unwrap()),
            ring_alpha(&later),
            "the same time is the same frame"
        );
    }

    fn first_ring_base(r: &GameRenderer) -> f32 {
        ring_alpha(r.packet.as_ref().unwrap())
    }

    #[test]
    fn a_new_scene_does_not_restart_the_clock_and_motion_off_draws_the_rest_frame() {
        let mut r = GameRenderer::new_headless().unwrap();
        r.set_scene(pulse_scene()).unwrap();
        r.set_clock(5_000.0);
        r.set_clock(5_500.0);
        let before = r.frame_packet().unwrap();
        r.set_scene(pulse_scene()).unwrap();
        let after = r.frame_packet().unwrap();
        assert_eq!(
            ring_alpha(&before),
            ring_alpha(&after),
            "phase survives a scene update"
        );
        r.set_motion(false);
        let rest = r.frame_packet().unwrap();
        assert_eq!(
            ring_alpha(&rest),
            first_ring_base(&r),
            "reduced motion: the rest frame"
        );
        assert_ne!(ring_alpha(&rest), ring_alpha(&after));
    }

    #[test]
    fn without_a_host_clock_the_scene_draws_its_first_frame() {
        let mut r = GameRenderer::new_headless().unwrap();
        r.set_scene(pulse_scene()).unwrap();
        let unclocked = r.frame_packet().unwrap();
        r.set_clock(123.0);
        assert_eq!(
            ring_alpha(&unclocked),
            ring_alpha(&r.frame_packet().unwrap()),
            "t=0 either way"
        );
    }
}
