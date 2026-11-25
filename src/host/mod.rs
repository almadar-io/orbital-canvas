//! What a host embeds: a canvas on its device ([`Canvas`]) and the scene it shows with the scene's input, scroll,
//! clock and the events it sends back to the program ([`SceneHost`]).

use crate::mappers;
use crate::packet::{self, DrawCommand, GameRenderPacket};
use crate::renderer;
use crate::{asset, scene3d, spring};

/// A queued input event waiting to be consumed by the next render tick.
#[derive(Debug, Clone)]
pub struct InputEvent {
    pub event_type: String,
    pub x: f32,
    pub y: f32,
    pub button: i32,
    pub key: Option<String>,
}

/// A scene host: the mapped canvas scene, its tappable/draggable regions, layout memory, clock and
/// pointer state. [`Canvas`] wraps one for the mobile surfaces; the desktop clients use it directly
/// and paint [`SceneHost::frame_packet`] on their own device.
pub struct SceneHost {
    /// Last submitted display-list packet.
    pub(crate) packet: Option<GameRenderPacket>,
    /// A timed scene mapped at the epoch clock once nothing in it plays any more, with the asset generation it was
    /// mapped under; dropped when a new mapping is installed.
    pub(crate) settled: std::sync::Mutex<Option<(u64, GameRenderPacket)>>,
    /// The current scene's props prepared for timed re-maps ([`mappers::game_canvas_2d::prepare_canvas_node`]), made
    /// on the first one and dropped with the scene.
    pub(crate) prepared: std::sync::Mutex<Option<mappers::game_canvas_2d::PreparedDoc>>,
    /// Each drawable of the timed re-maps, replayed while it is unchanged and no longer moves with the clock
    /// ([`mappers::game_canvas_2d::from_prepared_settled`], and the live re-maps since 2026-11-25).
    pub(crate) settled_drawables: std::sync::Mutex<mappers::game_canvas_2d::DrawableCache>,
    /// Timed mappings made for frames (observability for tests).
    #[cfg(test)]
    pub(crate) timed_maps: std::sync::atomic::AtomicUsize,
    /// Props prepared by a frame rather than handed over by the mapping (observability for tests).
    #[cfg(test)]
    pub(crate) timed_prepares: std::sync::atomic::AtomicUsize,
    /// Pending input events forwarded via the C ABI; drained each render tick.
    pub(crate) input_queue: Vec<InputEvent>,
    /// Surface size in PHYSICAL pixels, set by `resize` / surface init.
    pub(crate) viewport: Option<(u32, u32)>,
    /// Device pixels per logical point (`devicePixelRatio`). The scene is laid out in
    /// logical points (`viewport / density`, like the web's CSS-pixel measured viewport)
    /// and drawn onto the physical surface.
    pub(crate) density: f32,
    /// Last `canvas` render-ui node handed to `set_scene`, its drawables held apart when the 2D walk draws it;
    /// re-mapped when the viewport changes.
    pub(crate) scene: Option<orbital_canvas_base::scene_doc::SceneDoc>,
    /// Each top-level drawable's part of the scene's liveness, by the drawable it was read from.
    pub(crate) liveness_parts: std::collections::HashMap<usize, (std::sync::Weak<serde_json::Value>, frame::LivenessPart)>,
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
    /// Epoch milliseconds from the host paint clock, for `bornAt` world FX.
    pub(crate) epoch_clock_ms: f64,
    /// Whether the host allows motion (false: the OS reduced-motion setting is on).
    pub(crate) motion: bool,
    /// A 3D scene's camera and picking (OrbitControls, follow easing, unit/tile clicks), the same view the
    /// browser host drives; set while the current packet draws a `Scene3D`.
    pub(crate) view3d: Option<scene3d::Scene3DView>,
    /// Whether the 3D camera moved on the last clock tick (damping or follow easing still running).
    pub(crate) view3d_moving: bool,
    /// Springs of the current scene's drawables; the host maps the scene as they display it.
    pub(crate) springs: spring::SpringBank,
    /// Each top-level drawable's spring fields, read again only for a drawable new since the last scene.
    pub(crate) spring_parts: spring::SpringParts,
    /// Pointer regions of the current scene (draw order), in logical points.
    pub(crate) pointer_regions: Vec<mappers::scene::PointerRegion>,
    /// The caret positions of each hit `draw-text`, so a press reports the caret index nearest it.
    pub(crate) text_hits: Vec<mappers::scene::TextHit>,
    /// The sizes of the measured texts at the latest layout, and the list last sent as `measureEvent`.
    pub(crate) measured: Vec<mappers::scene::Measured>,
    pub(crate) measure_reported: Vec<(String, f32, f32, u32)>,
    /// How many times the semantic nodes were replaced, and the (replacements, scroll offset) pair last named by
    /// [`Self::semantic_revision`] with the revision it was given.
    pub(crate) semantic_installs: u64,
    /// What the scene's text was last read as ([`Self::set_scene_text`]).
    pub(crate) scene_text: orbital_canvas_base::scene_doc::SceneTextMemo,
    pub(crate) semantic_seen: std::sync::Mutex<((u64, u32), u32)>,
    /// The JSON of the semantic tree at the revision it was written for ([`Self::semantic_tree_json`]).
    pub(crate) semantic_json: std::sync::Mutex<Option<(u32, String)>>,
    /// Times the semantic tree was written as JSON (observability for tests).
    #[cfg(test)]
    pub(crate) semantic_json_builds: std::sync::atomic::AtomicUsize,
    /// The resolved page offset of every frame origin the last `originsEvent` reported.
    pub(crate) origins_reported: Vec<(String, f32, f32)>,
    /// The semantic tree of the latest layout, and the size last sent as `resizeEvent`.
    pub(crate) semantic: Vec<mappers::scene::SemanticNode>,
    pub(crate) resize_reported: Option<(f32, f32)>,
    /// The reading direction last sent as `directionEvent`.
    pub(crate) direction_reported: Option<String>,
    /// The reduced-motion preference last sent as `motionEvent`.
    pub(crate) motion_reported: Option<bool>,
    /// The region under the pointer, the region holding a press, and the last pointer position.
    pub(crate) hovered: Option<String>,
    pub(crate) captured: Option<String>,
    pub(crate) pointer: Option<(f32, f32)>,
    /// Whether a drawable paints a material that reads the clock.
    pub(crate) live_material: bool,
    /// When what keeps the current scene moving ends, read once when the scene is set.
    pub(crate) liveness: frame::Liveness,
    /// Scenes read for [`frame::Liveness`] (observability for tests).
    #[cfg(test)]
    pub(crate) liveness_reads: std::sync::atomic::AtomicUsize,
    /// The embedded 3D scenes of the current packet that play by themselves.
    pub(crate) live_scenes3d: Vec<mappers::game_canvas_2d::LiveScene3d>,
    /// The pointers held down (ascending id), the one whose press began the gesture, and the `touches` payload
    /// of the event being dispatched when a multi-touch gesture is in progress.
    pub(crate) touches: Vec<Touch>,
    pub(crate) primary_pointer: Option<i64>,
    pub(crate) touch_payload: Option<serde_json::Value>,
    /// The page scroll of a canvas that declares `scroll`.
    pub(crate) page_scroll: PageScroll,
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
            epoch_clock_ms: 0.0,
            settled: std::sync::Mutex::new(None),
            prepared: std::sync::Mutex::new(None),
            settled_drawables: std::sync::Mutex::new(Default::default()),
            #[cfg(test)]
            timed_maps: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            timed_prepares: std::sync::atomic::AtomicUsize::new(0),
            motion: true,
            view3d: None,
            view3d_moving: false,
            springs: spring::SpringBank::new(),
            spring_parts: spring::SpringParts::default(),
            pointer_regions: Vec::new(),
            text_hits: Vec::new(),
            measured: Vec::new(),
            measure_reported: Vec::new(),
            semantic_installs: 0,
            scene_text: Default::default(),
            semantic_seen: std::sync::Mutex::new(((0, 0), 0)),
            semantic_json: Default::default(),
            #[cfg(test)]
            semantic_json_builds: Default::default(),
            origins_reported: Vec::new(),
            semantic: Vec::new(),
            resize_reported: None,
            direction_reported: None,
            motion_reported: None,
            hovered: None,
            captured: None,
            pointer: None,
            live_material: false,
            liveness: frame::Liveness::default(),
            liveness_parts: std::collections::HashMap::new(),
            #[cfg(test)]
            liveness_reads: std::sync::atomic::AtomicUsize::new(0),
            live_scenes3d: Vec::new(),
            touches: Vec::new(),
            primary_pointer: None,
            touch_payload: None,
            page_scroll: PageScroll::default(),
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

    /// Scenes read for what keeps them moving so far.
    #[cfg(test)]
    pub(crate) fn liveness_reads(&self) -> usize {
        self.liveness_reads
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Timed mappings made for frames so far.
    #[cfg(test)]
    pub(crate) fn timed_maps(&self) -> usize {
        self.timed_maps.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The node the current scene was mapped from.
    pub fn scene(&self) -> Option<serde_json::Value> {
        self.scene.as_ref().map(orbital_canvas_base::scene_doc::SceneDoc::node)
    }

    /// The current scene's own fields (its drawables aside).
    pub(crate) fn frame(&self) -> Option<&serde_json::Value> {
        self.scene.as_ref().map(|doc| &*doc.frame)
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
        *self
            .prepared
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        self.hits.clear();
        self.drags.clear();
        self.springs.clear();
        self.pointer_regions.clear();
        self.measured.clear();
        self.live_material = false;
        self.liveness = frame::Liveness::default();
        self.live_scenes3d.clear();
        self.packet = Some(packet);
    }

    /// Map a `canvas` render-ui node to the current packet. With a known viewport the scene is
    /// laid out in it, in logical points (web `Canvas2D` parity); the node is kept and re-mapped
    /// on every resize and density change.
    pub fn set_scene(
        &mut self,
        node: impl Into<std::sync::Arc<serde_json::Value>>,
    ) -> std::result::Result<(), mappers::error::CanvasSceneError> {
        let node: std::sync::Arc<serde_json::Value> = node.into();
        let doc = if mappers::scene::walks_drawables(&node) {
            orbital_canvas_base::scene_doc::SceneDoc::split(std::sync::Arc::try_unwrap(node).unwrap_or_else(|node| (*node).clone()))
        } else {
            orbital_canvas_base::scene_doc::SceneDoc::whole(node)
        };
        self.set_scene_doc(doc)
    }

    /// [`Self::set_scene`] of the node `text` holds, for a host that has the scene as text (the web): the frame and
    /// each top-level drawable whose text is the one last read are the values already held, so a scene sent again
    /// costs the drawables that changed.
    pub fn set_scene_text(&mut self, text: &str) -> std::result::Result<(), String> {
        let doc = orbital_canvas_base::scene_doc::SceneDoc::split_text(text, &mut self.scene_text)
            .map_err(|e| format!("node_json: {e}"))?;
        let doc = if mappers::scene::walks_drawables(&doc.frame) {
            doc
        } else {
            orbital_canvas_base::scene_doc::SceneDoc::whole(doc.node())
        };
        self.set_scene_doc(doc).map_err(|e| e.to_string())
    }

    /// [`Self::set_scene`] of a scene whose drawables the caller already holds apart: a drawable shared with the
    /// last scene is not copied, themed, compared or walked again.
    pub fn set_scene_doc(
        &mut self,
        node: orbital_canvas_base::scene_doc::SceneDoc,
    ) -> std::result::Result<(), mappers::error::CanvasSceneError> {
        let generation = asset::generation();
        let mut bank = self.springs.clone();
        bank.retarget_doc_with(&node, &mut self.spring_parts);
        let previous = std::mem::replace(&mut self.springs, bank);
        let mapped = match self.map_scene(&node) {
            Ok(mapped) => mapped,
            Err(err) => {
                self.springs = previous;
                return Err(err);
            }
        };
        self.asset_generation = generation;
        self.live_material = spring::has_live_material_doc(&node);
        self.liveness = self.read_liveness(&node);
        #[cfg(test)]
        self.liveness_reads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        retire::retire(self.scene.replace(node));
        self.install(mapped);
        Ok(())
    }

    fn map_scene(
        &mut self,
        node: &orbital_canvas_base::scene_doc::SceneDoc,
    ) -> std::result::Result<mappers::scene::MappedScene, mappers::error::CanvasSceneError> {
        let density = self.density;
        let shown = (!self.springs.is_empty()).then(|| self.springs.apply_doc_with(node, &self.spring_parts));
        mappers::scene::map_scene_doc_with(
            shown.as_ref().unwrap_or(node),
            self.viewport
                .map(|(w, h)| (w as f32 / density, h as f32 / density)),
            &mut self.memory,
        )
    }

    fn install(&mut self, mut mapped: mappers::scene::MappedScene) {
        retire::retire(
            self.settled
                .get_mut()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take(),
        );
        retire::retire(std::mem::replace(
            self.prepared
                .get_mut()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            mapped.prepared.take(),
        ));
        let scene = mapped.packet.draw_commands.iter().find_map(|c| match c {
            DrawCommand::Scene3D { scene } => Some(scene.as_ref().clone()),
            _ => None,
        });
        match scene {
            Some(scene) => {
                let density = self.density;
                let view = self.view3d.get_or_insert_with(scene3d::Scene3DView::new);
                if let Some((w, h)) = self.viewport {
                    view.resize(w as f32 / density, h as f32 / density);
                }
                view.set_reduced_motion(!self.motion);
                view.set_follow(
                    scene
                        .interaction
                        .follow_from
                        .map(|start_eye| scene3d::view::Follow { start_eye }),
                );
                let interaction = &scene.interaction;
                let ground_events = self.scene.as_ref().is_some_and(|doc| {
                    let f = doc.root();
                    f.get("tileClickEvent").is_some() || f.get("unitClickEvent").is_some()
                });
                let picks_ground =
                    interaction.editable || !interaction.cells.is_empty() || ground_events;
                view.set_ground_extent(picks_ground.then_some(scene3d::view::GroundExtent {
                    min_x: interaction.min_x,
                    max_x: interaction.max_x,
                    min_z: interaction.min_z,
                    max_z: interaction.max_z,
                }));
                view.set_scene(scene);
            }
            None => self.view3d = None,
        }
        retire::retire(self.packet.replace(mapped.packet));
        self.hits = mapped.hits;
        self.drags = mapped.drags;
        self.double_hits = mapped.double_hits;
        self.pannable = mapped.pannable;
        self.pointer_regions = mapped.pointer_regions;
        self.text_hits = mapped.text_hits;
        self.measured = mapped.measured;
        self.semantic = mapped.semantic_nodes;
        self.semantic_installs += 1;
        self.live_scenes3d = mapped.live_scenes3d;
        match self.scroll_max() {
            Some(max) => {
                self.page_scroll.offset = self.page_scroll.offset.min(max);
                self.page_scroll.reported.get_or_insert(0.0);
            }
            None => self.page_scroll = PageScroll::default(),
        }
        self.apply_scroll_to();
        self.report_measured();
        self.report_origins();
        self.report_resize();
        self.report_direction();
        self.report_motion();
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

    /// The host surface in physical pixels with its scale, given together: the scene is laid out once, in
    /// `size / scale` points, and that is the one size it reports.
    pub fn set_surface(&mut self, width: u32, height: u32, scale: f32) -> std::result::Result<(), String> {
        if !scale.is_finite() || scale <= 0.0 {
            return Err(format!(
                "scale must be a finite positive number, got {scale}"
            ));
        }
        self.viewport = Some((width.max(1), height.max(1)));
        self.density = scale;
        self.relayout();
        Ok(())
    }

    /// The host surface in physical pixels; the scene is laid out in `size / density` points.
    pub fn set_viewport(&mut self, width: u32, height: u32) {
        self.viewport = Some((width.max(1), height.max(1)));
        self.relayout();
    }
}

mod access;
mod audit;
mod canvas;
mod events;
mod frame;
mod pointer;
mod retire;
mod scroll;
pub use self::access::*;
pub use self::audit::*;
pub use self::canvas::*;
pub use self::pointer::*;
pub(crate) use self::scroll::*;

#[cfg(test)]
mod tests;
