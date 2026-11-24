//! Mapper for the `canvas-2d` pattern — walks the `drawables` descriptor
//! tree (`draw-sprite` / `draw-shape` / `draw-text` atoms + their layer
//! molecules) into a screen-space display-list packet.
//!
//! Exact port of `@almadar/ui`'s Canvas2D host:
//! - projection: `lib/drawable/projector.ts` (`create2DProjector`) → [`orbital_canvas_base::projector`]
//! - camera composition: `Canvas2D.tsx` draw (`translate(v/2) → scale(zoom) →
//!   translate(-v/2 - cam)`), with `cam = anchorPoint(cameraPos,'center') - v/2`
//!   when `cameraPos` is authored and `(0,0)` otherwise (no user pan here)
//! - sprite paint: `DrawSprite.tsx` `paintSprite` (src resolution order
//!   frame → atlas → whole image; anchor offsets; world-unit sizes)
//! - shape paint: `DrawShape.tsx` `paintShape` (cell/rect/ellipse/poly; fills)
//!
//! Gaps skip + log once per kind (namespace `almadar::game_renderer::mapper`):
//! custom text fonts/baselines, shape gradients/effects, and camera-tiled
//! whole-image backgrounds (cover-scale used instead). Unknown drawable types
//! skip silently — React parity (`paintDispatch`).

use crate::error::CanvasSceneError;
use crate::scene::Measured;
use orbital_canvas_base::packet::{
    ColorRun, DrawCommand, FillPattern, GameRenderPacket, Gradient, GradientStop, LineCap,
    LineJoin, PathSegment, Rect, SdfKind, SdfMaterial, SdfShadow, SdfStroke, Stroke, TextAlign,
    Vec2,
};
use orbital_canvas_base::path_data::{parse_path_data, path_bounds};
use orbital_canvas_base::projector::{Anchor, Projection, Projector, ScenePos, TILE_WIDTH};
use orbital_canvas_base::scene_doc::SceneDoc;
use orbital_canvas_base::transform::Xf;
use orbital_canvas_paint::asset;
use orbital_canvas_paint::atlas;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::{OnceLock, RwLock};

const NS: &str = "almadar::game_renderer::mapper";

/// `isometric.ts` BACKGROUND_FALLBACK_COLOR (`#1a1a2e`).
const BACKGROUND_FALLBACK: [f32; 4] = [26.0 / 255.0, 26.0 / 255.0, 46.0 / 255.0, 1.0];

use orbital_canvas_base::color::parse_css_color;
use orbital_canvas_base::diagnostics::gap_once;

/// `Canvas2D.tsx` `effectiveZoom` under `fit`: the grid's pixel extent (tile
/// space) fitted into 85% of the viewport on the tighter axis.
fn fit_zoom(projection: Projection, w: f32, h: f32, vw: f32, vh: f32) -> f32 {
    let (board_w, board_h) = match projection {
        Projection::Flat => (w * TILE_WIDTH, h * TILE_WIDTH),
        Projection::Hex => (
            (w + 0.5) * TILE_WIDTH,
            h * (TILE_WIDTH / 2.0) * 0.75 + TILE_WIDTH / 2.0,
        ),
        _ => ((w + h) * (TILE_WIDTH / 2.0), (w + h) * (TILE_WIDTH / 4.0)),
    };
    (vw * 0.85 / board_w).min(vh * 0.85 / board_h)
}

/// A drawable with `id` and `hit: true`, as a pointer region in packet coordinates: its painted bounds and the
/// rotation (radians) about their centre.
#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    pub id: String,
    pub rect: Rect,
    pub rotation: f32,
    /// The drawable declared `wheel: true`: the wheel over it is the canvas's, not the page's.
    pub wheel: bool,
    /// The drawable, or a group holding it, declared `fixed: true`: it stays put when the page scrolls.
    pub fixed: bool,
    /// The drawable declared `drag: true`: a press on it keeps the pointer's travel, the page does not scroll.
    pub drag: bool,
    /// The resolved offset of the frame origin its top-level drawable named (`origin`), `None` on the page.
    pub origin: Option<(f32, f32)>,
}

/// What the drawable walk gathers besides display commands: regions and measured texts, both in paint order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Gathered {
    pub regions: Vec<Region>,
    pub measured: Vec<Measured>,
    pub text_hits: Vec<crate::scene::TextHit>,
    pub semantic: Vec<crate::scene::SemanticNode>,
    /// Each `draw-text` id's painted text, and each semantic node a `labelledBy` names: resolved once every node is
    /// walked, so a label painted after its control still names it.
    texts: std::collections::HashMap<String, String>,
    labelled: Vec<(usize, String)>,
    valued: Vec<(usize, String)>,
    /// Command ranges of `fixed` drawables, which stay put when the page scrolls.
    pinned: Vec<(usize, usize)>,
    /// Drawable animations the packet plays per frame: scroll-clocked ones and path morphs.
    animations: Vec<orbital_canvas_base::packet::Animation>,
    /// `draw-scene3d` drawables without an `id` so far, which name their targets by order.
    scene3d_count: usize,
    /// The embedded 3D scenes that play by themselves, to re-map alone per frame.
    pub live_scenes3d: Vec<LiveScene3d>,
}

fn invalid(field: &str, detail: impl Into<String>) -> CanvasSceneError {
    CanvasSceneError::InvalidProp {
        field: field.into(),
        detail: detail.into(),
    }
}

/// The `id` of a drawable that is a pointer region (`hit: true`).
fn hit_id(node: &Value) -> Option<&str> {
    node.get("hit")
        .and_then(Value::as_bool)
        .filter(|hit| *hit)?;
    node.get("id").and_then(Value::as_str)
}

fn takes_wheel(node: &Value) -> bool {
    node.get("wheel").and_then(Value::as_bool).unwrap_or(false)
}

fn takes_drag(node: &Value) -> bool {
    node.get("drag").and_then(Value::as_bool).unwrap_or(false)
}

/// Build a display-list packet from `canvas-2d` props (kernel-resolved).
pub fn from_props(props: &Value) -> Result<GameRenderPacket, CanvasSceneError> {
    from_props_at(props, 0.0)
}

/// The same scene at one epoch paint instant; FX descriptors expand at this
/// clock while `std-fx-particles` keeps the authoritative ttl/removal state.
pub fn from_props_at(
    props: &Value,
    epoch_now_ms: f64,
) -> Result<GameRenderPacket, CanvasSceneError> {
    from_props_mapped(props, epoch_now_ms).map(|(packet, _)| packet)
}

/// [`from_props_at`] with the pointer regions and measured texts the walk gathered.
pub fn from_props_mapped(
    props: &Value,
    epoch_now_ms: f64,
) -> Result<(GameRenderPacket, Gathered), CanvasSceneError> {
    from_props_sized(props, None, epoch_now_ms)
}

/// [`from_props_mapped`] laid out in `viewport` (logical width and height) in place of the props' own size.
fn from_props_sized(
    props: &Value,
    viewport: Option<(f32, f32)>,
    epoch_now_ms: f64,
) -> Result<(GameRenderPacket, Gathered), CanvasSceneError> {
    from_props_cached(props, viewport, epoch_now_ms, &mut DrawableCache::default())
}

/// The frame's `origins` resolved to their page offsets, sorted by id: what a canvas reports (`originsEvent`) so the
/// program can read a placed rect's page position without resolving origin chains itself.
pub fn frame_origins(props: &Value) -> Result<Vec<(String, f32, f32)>, CanvasSceneError> {
    let mut resolved: Vec<(String, f32, f32)> = origin::resolve_origins(props)?
        .into_iter()
        .map(|(id, (x, y))| (id, x, y))
        .collect();
    resolved.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(resolved)
}

/// [`from_props_sized`] replaying each drawable `cache` kept from the last mapping when it and the walk context are
/// unchanged, and keeping what this mapping walks.
pub fn from_props_cached(
    props: &Value,
    viewport: Option<(f32, f32)>,
    epoch_now_ms: f64,
    cache: &mut DrawableCache,
) -> Result<(GameRenderPacket, Gathered), CanvasSceneError> {
    let generations = (orbital_canvas_paint::asset::generation(), orbital_canvas_paint::vector::font_generation());
    from_props_cached_at(props, viewport, epoch_now_ms, false, generations, cache)
}

/// [`from_props_cached`] with the asset and font generations the walk reads given: a kept output is stale once either moves.
/// `settled` says the scene's animations have all settled, so no drawable moves with the clock and a kept output
/// holds at any clock.
pub(crate) fn from_props_cached_at(
    props: &Value,
    viewport: Option<(f32, f32)>,
    epoch_now_ms: f64,
    settled: bool,
    generations: (u64, u64),
    cache: &mut DrawableCache,
) -> Result<(GameRenderPacket, Gathered), CanvasSceneError> {
    let no_drawables = Vec::new();
    let drawables = props
        .get("drawables")
        .and_then(Value::as_array)
        .unwrap_or(&no_drawables);
    map_drawables(props, drawables, viewport, epoch_now_ms, settled, generations, cache)
}

/// The walk of `props` (read for everything but its drawables) over `drawables`.
#[allow(clippy::too_many_arguments)]
fn map_drawables<D: SharedDrawable>(
    props: &Value,
    drawables: &[D],
    viewport: Option<(f32, f32)>,
    epoch_now_ms: f64,
    settled: bool,
    generations: (u64, u64),
    cache: &mut DrawableCache,
) -> Result<(GameRenderPacket, Gathered), CanvasSceneError> {
    let mut gathered = Gathered::default();
    // An orbital-painter canvas is sized by its declared world, as the web host sizes it.
    let orbital = props.get("painter").and_then(Value::as_str) == Some("orbital");
    let (width_key, height_key) = if orbital {
        ("worldWidth", "worldHeight")
    } else {
        ("width", "height")
    };
    let (width, height) = match viewport.filter(|_| !orbital) {
        Some((w, h)) => (w.max(1.0), h.max(1.0)),
        None => (
            get_f32(props, width_key, 800.0),
            get_f32(props, height_key, 600.0),
        ),
    };
    let scale = get_f32(props, "scale", 0.4);
    let projection = match props.get("projection").and_then(Value::as_str) {
        Some(s) => Projection::parse(s).unwrap_or_else(|| {
            gap_once(
                "projection",
                "unknown projection string — defaulting to isometric",
            );
            Projection::Isometric
        }),
        None => Projection::Isometric,
    };
    // Scene extent from drawn positions → baseOffsetX (Canvas2D gridExtent +
    // baseOffsetX useMemo): (maxY + 1 - 1) * scaledTileWidth / 2 for diamond
    // layouts, 0 for square/free.
    let mut max_x = 0.0f32;
    let mut max_y = 0.0f32;
    for node in drawables {
        collect_positions(node.value(), &mut |p| {
            if p.x > max_x {
                max_x = p.x;
            }
            if p.y > max_y {
                max_y = p.y;
            }
        });
    }
    // `Canvas2D.tsx` frames grid layouts (iso/hex/flat; not free/side): `fit`
    // scales the grid extent to 85% of the viewport, and with no authored
    // `cameraPos` the camera centres on the grid's centre cell. Both need a
    // grid at least 2x2 (a single row/column has no centre to frame).
    let is_grid = !matches!(projection, Projection::Free);
    let (extent_w, extent_h) = (max_x + 1.0, max_y + 1.0);
    let has_grid = is_grid && extent_w >= 2.0 && extent_h >= 2.0;
    let fit = props.get("fit").and_then(Value::as_bool).unwrap_or(false);
    // (projector scale, camera zoom): the on-screen cell is TILE_WIDTH * scale * zoom.
    // Authored zoom keeps the legacy-squared contract (scale twice); `fit` puts
    // the whole fitted zoom on the camera over a unit-scale projector.
    let (projector_scale, zoom) = if fit && has_grid {
        (1.0, fit_zoom(projection, extent_w, extent_h, width, height))
    } else {
        (scale, scale)
    };
    let base_offset_x = if matches!(projection, Projection::Flat | Projection::Free) {
        0.0
    } else {
        max_y * (TILE_WIDTH * projector_scale / 2.0)
    };

    let projector = Projector::new(projector_scale, base_offset_x, projection);

    // Camera (Canvas2D draw): cam centres on cameraPos when authored, else on
    // the grid's centre cell.
    let focus = props
        .get("cameraPos")
        .and_then(parse_scene_pos)
        .or_else(|| {
            has_grid.then(|| ScenePos {
                x: (extent_w - 1.0) / 2.0,
                y: (extent_h - 1.0) / 2.0,
            })
        });
    let cam = match focus {
        Some(cp) => {
            let (ax, ay) = projector.anchor_point(cp, Anchor::Center);
            (ax - width / 2.0, ay - height / 2.0)
        }
        None => (0.0, 0.0),
    };
    let to_screen = |x: f32, y: f32| -> (f32, f32) {
        (
            (x - width / 2.0 - cam.0) * zoom + width / 2.0,
            (y - height / 2.0 - cam.1) * zoom + height / 2.0,
        )
    };

    let mut draw_commands = Vec::new();

    // Background — drawn pre-camera (Canvas2D 359-383).
    let color = props
        .get("bgColor")
        .and_then(Value::as_str)
        .and_then(parse_css_color)
        .unwrap_or(BACKGROUND_FALLBACK);
    draw_commands.push(DrawCommand::Clear { color });
    if let Some(bg) = props.get("backgroundImage").filter(|bg| !bg.is_null()) {
        draw_commands.extend(background_sprites(bg, width, height, cam));
    }
    gathered.pinned.push((0, draw_commands.len()));

    cache.begin(drawable_cache::WalkContext {
        width,
        height,
        zoom,
        camera: cam,
        projector_scale,
        base_offset_x,
        projection,
        clock: (!settled).then_some(epoch_now_ms),
        asset_generation: generations.0,
        font_generation: generations.1,
    });
    let origins = origin::resolve_origins(props)?;
    for (index, node) in drawables.iter().enumerate() {
        let offset = origin::offset_of(node.value(), &origins)?;
        if cache.replay(node, index, offset, &mut gathered, &mut draw_commands) {
            continue;
        }
        cache.walk(
            node,
            index,
            offset,
            &projector,
            &to_screen,
            zoom,
            epoch_now_ms,
            &mut gathered,
            &mut draw_commands,
        )?;
    }
    for (index, target) in std::mem::take(&mut gathered.labelled) {
        let Some(text) = gathered.texts.get(&target) else {
            return Err(invalid(
                "semantic.labelledBy",
                format!("names `{target}`, which is no draw-text with an id"),
            ));
        };
        gathered.semantic[index].name = text.clone();
    }
    for (index, target) in std::mem::take(&mut gathered.valued) {
        let Some(text) = gathered.texts.get(&target) else {
            return Err(invalid(
                "semantic.valueFrom",
                format!("names `{target}`, which is no draw-text with an id"),
            ));
        };
        gathered.semantic[index].value = Some(text.clone());
    }

    crate::layer_props::check_target_cycles(&draw_commands)?;
    let pinned = std::mem::take(&mut gathered.pinned);
    let animations = std::mem::take(&mut gathered.animations);
    let packet = GameRenderPacket {
        pattern: "canvas-2d".into(),
        width,
        height,
        background_image: props.get("backgroundImage").and_then(|bg| match bg {
            Value::String(s) => Some(s.clone()),
            Value::Object(_) => bg.get("url").and_then(Value::as_str).map(str::to_string),
            _ => None,
        }),
        asset_base_url: get_string(props, "assetBaseUrl"),
        camera: None, // transform is baked into screen-space commands above
        draw_commands,
        tick_event: get_string(props, "tickEvent"),
        draw_event: get_string(props, "drawEvent"),
        fps: props.get("fps").and_then(Value::as_u64).map(|f| f as u32),
        animations,
        time_ms: 0.0,
        pointer: None,
        scroll: 0.0,
        pinned,
    };
    Ok((packet, gathered))
}

/// Build a display-list packet from a `canvas` / `canvas-2d` render-ui node.
///
/// The Rust twin of the two web stages that sit in front of `Canvas2D`:
/// `UISlotRenderer` routes a draw-host's authored `children` into its
/// `drawables` prop (`toDrawableNodes`: flat `{type, ...fields}` or nested
/// `{type, props}`; string children dropped), and `Canvas.tsx` maps the neutral
/// `camera` object onto the 2D host's vocabulary (`camera.zoom` → `scale`,
/// `camera.pos` → `cameraPos`, `backgroundColor` → `bgColor`). The result feeds
/// [`from_props`] — the one drawable walk. `mode: "3d"` is refused, not drawn flat.
pub fn from_canvas_node(node: &Value) -> Result<GameRenderPacket, CanvasSceneError> {
    from_canvas_node_at(node, 0.0)
}

pub fn from_canvas_node_at(
    node: &Value,
    epoch_now_ms: f64,
) -> Result<GameRenderPacket, CanvasSceneError> {
    from_canvas_node_mapped(node, epoch_now_ms).map(|(packet, _)| packet)
}

/// [`from_canvas_node_at`] with the pointer regions and measured texts.
pub fn from_canvas_node_mapped(
    node: &Value,
    epoch_now_ms: f64,
) -> Result<(GameRenderPacket, Gathered), CanvasSceneError> {
    from_canvas_node_cached(node, epoch_now_ms, &mut DrawableCache::default())
}

/// [`from_canvas_node_mapped`] replaying the unchanged drawables `cache` kept ([`from_props_cached`]).
pub fn from_canvas_node_cached(
    node: &Value,
    epoch_now_ms: f64,
    cache: &mut DrawableCache,
) -> Result<(GameRenderPacket, Gathered), CanvasSceneError> {
    canvas_node_props(node).and_then(|props| from_props_cached(&Value::Object(props), None, epoch_now_ms, cache))
}

/// [`from_canvas_node`] laid out in the host's own viewport. `Canvas2D.tsx`
/// sizes its surface from a `ResizeObserver` on its container and draws the
/// scene in that coordinate space (no contain/cover fit), so the native host
/// feeds its surface size here instead of the 800x600 default.
pub fn from_canvas_node_in_viewport(
    node: &Value,
    width: f32,
    height: f32,
) -> Result<GameRenderPacket, CanvasSceneError> {
    from_canvas_node_in_viewport_at(node, width, height, 0.0)
}

pub fn from_canvas_node_in_viewport_at(
    node: &Value,
    width: f32,
    height: f32,
    epoch_now_ms: f64,
) -> Result<GameRenderPacket, CanvasSceneError> {
    from_canvas_node_in_viewport_mapped(node, width, height, epoch_now_ms).map(|(packet, _)| packet)
}

/// [`from_canvas_node_in_viewport_at`] with the pointer regions and measured texts.
pub fn from_canvas_node_in_viewport_mapped(
    node: &Value,
    width: f32,
    height: f32,
    epoch_now_ms: f64,
) -> Result<(GameRenderPacket, Gathered), CanvasSceneError> {
    from_canvas_node_in_viewport_cached(node, width, height, epoch_now_ms, &mut DrawableCache::default())
}

/// [`from_canvas_node_in_viewport_mapped`] replaying the unchanged drawables `cache` kept ([`from_props_cached`]).
pub fn from_canvas_node_in_viewport_cached(
    node: &Value,
    width: f32,
    height: f32,
    epoch_now_ms: f64,
    cache: &mut DrawableCache,
) -> Result<(GameRenderPacket, Gathered), CanvasSceneError> {
    from_props_cached(&prepare_canvas_node(node)?, Some((width, height)), epoch_now_ms, cache)
}

/// A canvas node's props as the walk reads them (theme references resolved, children as drawables): prepare once,
/// then map at many clocks with [`from_prepared_at`].
pub fn prepare_canvas_node(node: &Value) -> Result<Value, CanvasSceneError> {
    canvas_node_props(node).map(Value::Object)
}

/// [`from_canvas_node_in_viewport_at`] (or, with no viewport, [`from_canvas_node_at`]) of a node
/// [`prepare_canvas_node`] prepared.
pub fn from_prepared_at(
    prepared: &Value,
    viewport: Option<(f32, f32)>,
    epoch_now_ms: f64,
) -> Result<GameRenderPacket, CanvasSceneError> {
    from_props_sized(prepared, viewport, epoch_now_ms).map(|(packet, _)| packet)
}

/// [`from_prepared_at`] for a scene whose animations have all settled: its drawables do not move with the clock, so
/// each one unchanged since the last settled mapping `cache` kept is replayed.
pub fn from_prepared_settled(
    prepared: &Value,
    viewport: Option<(f32, f32)>,
    epoch_now_ms: f64,
    cache: &mut DrawableCache,
) -> Result<GameRenderPacket, CanvasSceneError> {
    let generations = (orbital_canvas_paint::asset::generation(), orbital_canvas_paint::vector::font_generation());
    from_props_cached_at(prepared, viewport, epoch_now_ms, true, generations, cache).map(|(packet, _)| packet)
}

/// A split canvas node ([`SceneDoc`]) as the walk reads it: the node's own fields prepared as [`prepare_canvas_node`]
/// prepares them, and each listed drawable themed on its own, shared while it is unchanged.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PreparedDoc {
    pub props: Value,
    /// The drawables the walk reads; `None` when they are inside `props` (a node kept whole).
    pub drawables: Option<std::sync::Arc<Vec<std::sync::Arc<Value>>>>,
}

/// Each drawable's themed form by the drawable it came from, kept across preparations under one theme.
#[derive(Debug, Clone, Default)]
pub struct ThemeMemo {
    theme: Option<Value>,
    themed: std::collections::HashMap<usize, (std::sync::Weak<Value>, std::sync::Arc<Value>)>,
}

/// [`prepare_canvas_node`] of a split node: a drawable themed by an earlier call under the same theme is reused.
pub fn prepare_doc(doc: &SceneDoc, memo: &mut ThemeMemo) -> Result<PreparedDoc, CanvasSceneError> {
    let Some(listed) = &doc.drawables else {
        return prepare_canvas_node(&doc.frame).map(|props| PreparedDoc { props, drawables: None });
    };
    let (props, theme) = canvas_node_props_split(&doc.frame, true)?;
    let Some(theme) = theme else {
        memo.theme = None;
        memo.themed.clear();
        return Ok(PreparedDoc { props: Value::Object(props), drawables: Some(std::sync::Arc::clone(listed)) });
    };
    if memo.theme.as_ref() != Some(&theme) {
        memo.themed.clear();
        memo.theme = Some(theme.clone());
    }
    let families = crate::theme_refs::families(&theme);
    let mut themed = std::collections::HashMap::with_capacity(listed.len());
    let drawables = listed
        .iter()
        .map(|drawable| {
            let at = std::sync::Arc::as_ptr(drawable) as usize;
            let kept = memo
                .themed
                .remove(&at)
                .filter(|(from, _)| from.upgrade().is_some_and(|from| std::sync::Arc::ptr_eq(&from, drawable)));
            let shown = match kept {
                Some((_, shown)) => shown,
                None => {
                    let mut shown = (**drawable).clone();
                    crate::theme_refs::apply_with(&mut shown, &theme, &families);
                    std::sync::Arc::new(shown)
                }
            };
            themed.insert(at, (std::sync::Arc::downgrade(drawable), std::sync::Arc::clone(&shown)));
            shown
        })
        .collect();
    memo.themed = themed;
    Ok(PreparedDoc { props: Value::Object(props), drawables: Some(std::sync::Arc::new(drawables)) })
}

/// [`from_props_cached`] of a [`PreparedDoc`].
pub fn from_prepared_doc_cached(
    prepared: &PreparedDoc,
    viewport: Option<(f32, f32)>,
    epoch_now_ms: f64,
    cache: &mut DrawableCache,
) -> Result<(GameRenderPacket, Gathered), CanvasSceneError> {
    let generations = (orbital_canvas_paint::asset::generation(), orbital_canvas_paint::vector::font_generation());
    from_prepared_doc_cached_at(prepared, viewport, epoch_now_ms, false, generations, cache)
}

pub(crate) fn from_prepared_doc_cached_at(
    prepared: &PreparedDoc,
    viewport: Option<(f32, f32)>,
    epoch_now_ms: f64,
    settled: bool,
    generations: (u64, u64),
    cache: &mut DrawableCache,
) -> Result<(GameRenderPacket, Gathered), CanvasSceneError> {
    match &prepared.drawables {
        Some(listed) => map_drawables(&prepared.props, listed, viewport, epoch_now_ms, settled, generations, cache),
        None => from_props_cached_at(&prepared.props, viewport, epoch_now_ms, settled, generations, cache),
    }
}

/// [`from_prepared_at`] of a [`PreparedDoc`].
pub fn from_prepared_doc_at(
    prepared: &PreparedDoc,
    viewport: Option<(f32, f32)>,
    epoch_now_ms: f64,
) -> Result<GameRenderPacket, CanvasSceneError> {
    from_prepared_doc_cached(prepared, viewport, epoch_now_ms, &mut DrawableCache::default()).map(|(packet, _)| packet)
}

/// [`from_prepared_settled`] of a [`PreparedDoc`].
pub fn from_prepared_doc_settled(
    prepared: &PreparedDoc,
    viewport: Option<(f32, f32)>,
    epoch_now_ms: f64,
    cache: &mut DrawableCache,
) -> Result<GameRenderPacket, CanvasSceneError> {
    let generations = (orbital_canvas_paint::asset::generation(), orbital_canvas_paint::vector::font_generation());
    from_prepared_doc_cached_at(prepared, viewport, epoch_now_ms, true, generations, cache).map(|(packet, _)| packet)
}

fn canvas_node_props(node: &Value) -> Result<serde_json::Map<String, Value>, CanvasSceneError> {
    canvas_node_props_split(node, false).map(|(props, _)| props)
}

/// A canvas node's fields prepared, with its theme. `listed` says its drawables are held apart ([`SceneDoc`]), so its
/// `children` are not read as drawables in their place and the theme is still to be applied to them.
fn canvas_node_props_split(
    node: &Value,
    listed: bool,
) -> Result<(serde_json::Map<String, Value>, Option<Value>), CanvasSceneError> {
    let obj = node.as_object().ok_or(CanvasSceneError::NotAnObject)?;
    let fields = match obj.get("props") {
        Some(Value::Object(nested)) => nested,
        _ => obj,
    };
    if let Some(mode) = fields.get("mode").and_then(Value::as_str) {
        if mode != "2d" {
            return Err(CanvasSceneError::UnsupportedMode(mode.to_owned()));
        }
    }
    let mut props = fields.clone();
    props.remove("type");
    let host = props.remove("canvasTheme");
    let layer = props.remove("themeLayer");
    let theme = crate::theme_refs::layered(host, layer);
    if let Some(theme) = &theme {
        crate::theme_refs::apply(&mut props, theme);
    }
    if !listed && !props.contains_key("drawables") {
        if let Some(children) = props.get("children") {
            props.insert(
                "drawables".into(),
                Value::Array(to_drawable_nodes(children)),
            );
        }
    }
    if let Some(Value::Object(camera)) = props.remove("camera") {
        if let (false, Some(zoom)) = (props.contains_key("scale"), camera.get("zoom")) {
            props.insert("scale".into(), zoom.clone());
        }
        if let (false, Some(pos)) = (props.contains_key("cameraPos"), camera.get("pos")) {
            props.insert("cameraPos".into(), pos.clone());
        }
    }
    if !props.contains_key("bgColor") {
        if let Some(color) = props.get("backgroundColor") {
            props.insert("bgColor".into(), color.clone());
        }
    }
    Ok((props, theme))
}

/// `UISlotRenderer.toDrawableNodes`: each object child becomes its fields
/// (nested `props` when present, else the flat siblings) plus its `type`.
pub(crate) fn to_drawable_nodes(children: &Value) -> Vec<Value> {
    let list: Vec<&Value> = match children {
        Value::Array(items) => items.iter().collect(),
        other => vec![other],
    };
    list.into_iter()
        .filter_map(|child| {
            let rec = child.as_object()?;
            let ty = rec.get("type")?.clone();
            let mut fields = match rec.get("props") {
                Some(Value::Object(nested)) => nested.clone(),
                _ => {
                    let mut flat = rec.clone();
                    flat.remove("type");
                    flat.remove("props");
                    flat.remove("_id");
                    flat
                }
            };
            fields.insert("type".into(), ty);
            Some(Value::Object(fields))
        })
        .collect()
}

/// Dispatch one drawable node. Layer molecules paint their `items` with the
/// layer's own atom painter (the layer contract — `DrawSpriteLayer.tsx`), not
/// by the item's `type`.
/// Paint one drawable, then record it in the semantic tree when it carries `semantic` and an `id`.
fn walk(
    node: &Value,
    projector: &Projector,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
    zoom: f32,
    epoch_now_ms: f64,
    gathered: &mut Gathered,
    out: &mut Vec<DrawCommand>,
) -> Result<(), CanvasSceneError> {
    let (out_start, region_start) = (out.len(), gathered.regions.len());
    let (hit_start, semantic_start) = (gathered.text_hits.len(), gathered.semantic.len());
    let epoch = epoch_animation(node, projector, to_screen);
    let at_rest = epoch.as_ref().map(|_| epoch_rest_node(node));
    walk_paint(
        at_rest.as_ref().unwrap_or(node),
        projector,
        to_screen,
        zoom,
        epoch_now_ms,
        gathered,
        out,
    )?;
    if let Some(animation) = epoch {
        gathered
            .animations
            .push(orbital_canvas_base::packet::Animation {
                first: out_start,
                end: out.len(),
                path: Vec::new(),
                ..animation
            });
    }
    let fixed = node.get("fixed").and_then(Value::as_bool) == Some(true);
    if fixed {
        gathered.regions[region_start..]
            .iter_mut()
            .for_each(|r| r.fixed = true);
        gathered.text_hits[hit_start..]
            .iter_mut()
            .for_each(|h| h.fixed = true);
        gathered.semantic[semantic_start..]
            .iter_mut()
            .for_each(|n| n.fixed = true);
        gathered.pinned.retain(|&(first, _)| first < out_start);
        gathered.pinned.push((out_start, out.len()));
    }
    if let Some(animation) = packet_animation(node, projector, to_screen, zoom, &out[out_start..])?
    {
        gathered
            .animations
            .push(orbital_canvas_base::packet::Animation {
                first: out_start,
                end: out.len(),
                path: Vec::new(),
                ..animation
            });
    }
    let Some(id) = node.get("id").and_then(Value::as_str) else {
        return Ok(());
    };
    if node.get("type").and_then(Value::as_str) == Some("draw-text") {
        if let Some(text) = node.get("text").and_then(Value::as_str) {
            gathered.texts.insert(id.to_owned(), text.to_owned());
        }
    }
    let Some(semantic) = node.get("semantic").filter(|v| v.is_object()) else {
        return Ok(());
    };
    let rect = gathered.regions[region_start..]
        .iter()
        .rev()
        .find(|r| r.id == id)
        .map(|r| r.rect)
        .or_else(|| commands_bounds(&out[out_start..]));
    let Some(rect) = rect else {
        return Ok(());
    };
    let text = |k: &str| semantic.get(k).and_then(Value::as_str).map(str::to_owned);
    let flag = |k: &str| semantic.get(k).and_then(Value::as_bool);
    let index = |k: &str| semantic.get(k).and_then(Value::as_u64).map(|n| n as usize);
    let paint_order = gathered.semantic.len() as f32;
    if let Some(target) = text("labelledBy") {
        if semantic.get("name").is_some() {
            return Err(invalid(
                "semantic.labelledBy",
                "a semantic node takes its name from `name` or `labelledBy`, not both",
            ));
        }
        gathered.labelled.push((gathered.semantic.len(), target));
    }
    if let Some(target) = text("valueFrom") {
        if semantic.get("value").is_some() {
            return Err(invalid(
                "semantic.valueFrom",
                "a semantic node takes its value from `value` or `valueFrom`, not both",
            ));
        }
        gathered.valued.push((gathered.semantic.len(), target));
    }
    let level = match semantic.get("level") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_u64()
                .filter(|n| (1..=6).contains(n))
                .map(|n| n as u8)
                .ok_or_else(|| {
                    invalid(
                        "semantic.level",
                        "a semantic level is an integer from 1 to 6",
                    )
                })?,
        ),
    };
    let href = match semantic.get("href") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.is_empty() => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err(invalid("semantic.href", "a semantic href is a string")),
    };
    let expanded = match semantic.get("expanded") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(b)) => Some(*b),
        Some(_) => {
            return Err(invalid(
                "semantic.expanded",
                "a semantic expanded is true or false",
            ))
        }
    };
    let owner = match semantic.get("owner") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.is_empty() => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err(invalid("semantic.owner", "a semantic owner is a node id")),
    };
    gathered.semantic.push(crate::scene::SemanticNode {
        id: id.to_owned(),
        role: text("role")
            .ok_or_else(|| invalid("semantic.role", "a semantic node needs a role"))?,
        name: text("name").unwrap_or_default(),
        value: text("value"),
        checked: flag("checked"),
        disabled: flag("disabled").unwrap_or(false),
        focused: flag("focused").unwrap_or(false),
        caret: index("caret"),
        selection_start: index("selectionStart"),
        selection_end: index("selectionEnd"),
        multiline: flag("multiline").unwrap_or(false),
        level,
        href,
        expanded,
        owner,
        order: semantic
            .get("order")
            .and_then(Value::as_f64)
            .map_or(paint_order, |o| o as f32),
        rect,
        fixed,
    });
    Ok(())
}

fn walk_paint(
    node: &Value,
    projector: &Projector,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
    zoom: f32,
    epoch_now_ms: f64,
    gathered: &mut Gathered,
    out: &mut Vec<DrawCommand>,
) -> Result<(), CanvasSceneError> {
    if let Some(animation) = node.get("animation") {
        orbital_canvas_base::animate::start_ms(animation)
            .map_err(|detail| invalid("animation", detail))?;
    }
    if node.get("type").and_then(Value::as_str) == Some("draw-shape")
        && node.get("shape").and_then(Value::as_str) == Some("path")
    {
        match node.get("scale") {
            None | Some(Value::Null) => {}
            Some(scale) => {
                if !scale.as_f64().is_some_and(|n| n.is_finite() && n > 0.0) {
                    return Err(invalid("scale", "must be a number above zero"));
                }
            }
        }
    }
    match node.get("type").and_then(Value::as_str) {
        Some("draw-sprite") => paint_sprite_gathering(
            node,
            projector,
            to_screen,
            zoom,
            epoch_now_ms,
            gathered,
            out,
        )?,
        Some("draw-scene3d") => paint_scene3d_gathering(
            node,
            projector,
            to_screen,
            zoom,
            epoch_now_ms,
            gathered,
            out,
        )?,
        Some("draw-shape") => paint_shape(
            node,
            projector,
            to_screen,
            zoom,
            epoch_now_ms,
            gathered,
            out,
        )?,
        Some("draw-text") => paint_text(
            node,
            projector,
            to_screen,
            zoom,
            epoch_now_ms,
            gathered,
            out,
        )?,
        Some("draw-skinned-mesh") => {
            use crate::skinned_mesh_2d::SkinMeshError;
            match crate::skinned_mesh_2d::paint(node, projector, to_screen) {
                Ok(Some(commands)) => out.extend(commands),
                Ok(None) => {}
                Err(SkinMeshError::Skin(error)) => return Err(CanvasSceneError::Skin(error)),
                Err(error) => return Err(CanvasSceneError::SkinnedMesh(error)),
            }
        }
        Some("draw-sprite-layer") => {
            for item in items(node) {
                paint_sprite_gathering(
                    item,
                    projector,
                    to_screen,
                    zoom,
                    epoch_now_ms,
                    gathered,
                    out,
                )?;
            }
        }
        Some("draw-shape-layer") => {
            for item in items(node) {
                paint_shape(
                    item,
                    projector,
                    to_screen,
                    zoom,
                    epoch_now_ms,
                    gathered,
                    out,
                )?;
            }
        }
        Some("draw-text-layer") => {
            for item in items(node) {
                paint_text(
                    item,
                    projector,
                    to_screen,
                    zoom,
                    epoch_now_ms,
                    gathered,
                    out,
                )?;
            }
        }
        Some("draw-group") => paint_group(
            node,
            projector,
            to_screen,
            zoom,
            epoch_now_ms,
            gathered,
            out,
        )?,
        Some("draw-emitter") => {
            let particles = crate::emitter::expand(node, epoch_now_ms)
                .map_err(|e| invalid(e.field, e.detail))?;
            for particle in &particles {
                walk(
                    particle,
                    projector,
                    to_screen,
                    zoom,
                    epoch_now_ms,
                    gathered,
                    out,
                )?;
            }
        }
        Some("draw-fx-layer") => {
            for drawable in orbital_canvas_base::fx_projection::expand_layer(
                node,
                epoch_now_ms,
                "2d",
                f64::from(projector.tile_width),
            )? {
                walk(
                    &drawable,
                    projector,
                    to_screen,
                    zoom,
                    epoch_now_ms,
                    gathered,
                    out,
                )?;
            }
        }
        _ => {} // unknown types skipped silently — React parity
    }
    Ok(())
}

/// Paint `drawables` (a learning canvas's game drawables) through `projector`, then map canvas
/// pixels to the packet with `to_screen` / `zoom`. Colors pass through `resolve` first
/// (`resolveDrawableColors`), so tone names paint in the theme's hand.
pub fn paint_drawables(
    drawables: &[Value],
    projector: &Projector,
    resolve: &dyn Fn(&str) -> String,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
    zoom: f32,
) -> Result<Vec<DrawCommand>, CanvasSceneError> {
    let mut out = Vec::new();
    let mut gathered = Gathered::default();
    for node in drawables
        .iter()
        .map(|n| resolve_drawable_colors(n, resolve))
    {
        walk(
            &node,
            projector,
            to_screen,
            zoom,
            0.0,
            &mut gathered,
            &mut out,
        )?;
    }
    Ok(out)
}

/// `themeDrawables.ts` `resolveDrawableColors`: shape `fill`/`stroke` and text `color`, through
/// groups and layers.
pub fn resolve_drawable_colors(node: &Value, resolve: &dyn Fn(&str) -> String) -> Value {
    let mut node = node.clone();
    let recolor = |item: &mut Value, keys: &[&str]| {
        for key in keys {
            if let Some(v) = item.get(*key).and_then(Value::as_str).map(resolve) {
                item[*key] = Value::String(v);
            }
        }
    };
    match node.get("type").and_then(Value::as_str) {
        Some("draw-shape") => recolor(&mut node, &["fill", "stroke"]),
        Some("draw-text") => recolor(&mut node, &["color"]),
        Some("draw-shape-layer" | "draw-text-layer" | "draw-group") => {
            let keys: &[&str] = match node.get("type").and_then(Value::as_str) {
                Some("draw-text-layer") => &["color"],
                _ => &["fill", "stroke"],
            };
            let group = node.get("type").and_then(Value::as_str) == Some("draw-group");
            if let Some(items) = node.get_mut("items").and_then(Value::as_array_mut) {
                for item in items {
                    if group {
                        *item = resolve_drawable_colors(item, resolve);
                    } else {
                        recolor(item, keys);
                    }
                }
            }
        }
        _ => {}
    }
    node
}

fn items(node: &Value) -> &[Value] {
    node.get("items")
        .and_then(Value::as_array)
        .map(|a| a.as_slice())
        .unwrap_or(&[])
}

/// Collect every drawable's scene position (atoms directly; layers via `items`)
/// — `hitTest.collectDrawnItems`.
fn collect_positions(node: &Value, sink: &mut dyn FnMut(ScenePos)) {
    collect_positions_from(node, ScenePos { x: 0.0, y: 0.0 }, sink);
}

fn collect_positions_from(node: &Value, origin: ScenePos, sink: &mut dyn FnMut(ScenePos)) {
    let add = |p: ScenePos| ScenePos {
        x: p.x + origin.x,
        y: p.y + origin.y,
    };
    match node.get("type").and_then(Value::as_str) {
        Some("draw-sprite")
        | Some("draw-shape")
        | Some("draw-text")
        | Some("draw-scene3d")
        | Some("draw-skinned-mesh") => {
            if let Some(p) = node.get("position").and_then(parse_scene_pos) {
                sink(add(p));
            }
        }
        Some("draw-sprite-layer") | Some("draw-shape-layer") | Some("draw-text-layer") => {
            for item in items(node) {
                if let Some(p) = item.get("position").and_then(parse_scene_pos) {
                    sink(add(p));
                }
            }
        }
        Some("draw-group") => {
            if let Some(p) = node.get("position").and_then(parse_scene_pos) {
                for item in items(node) {
                    collect_positions_from(item, add(p), sink);
                }
            }
        }
        _ => {}
    }
}

fn parse_scene_pos(v: &Value) -> Option<ScenePos> {
    Some(ScenePos {
        x: v.get("x").and_then(Value::as_f64)? as f32,
        y: v.get("y").and_then(Value::as_f64)? as f32,
    })
}

fn parse_xywh(v: &Value) -> Option<Rect> {
    Some(Rect {
        x: v.get("x").and_then(Value::as_f64)? as f32,
        y: v.get("y").and_then(Value::as_f64)? as f32,
        w: v.get("w").and_then(Value::as_f64)? as f32,
        h: v.get("h").and_then(Value::as_f64)? as f32,
    })
}

pub fn get_f32(props: &Value, key: &str, default: f32) -> f32 {
    props
        .get(key)
        .and_then(Value::as_f64)
        .map(|f| f as f32)
        .unwrap_or(default)
}

pub fn get_string(props: &Value, key: &str) -> Option<String> {
    props.get(key).and_then(Value::as_str).map(str::to_string)
}

mod animation;
mod drawable_cache;
pub use self::drawable_cache::{DrawableCache, SharedDrawable};
mod editing;
mod group;
mod origin;
mod paragraph;
mod scene3d;
mod shape;
mod sprite;
mod text;
pub use self::animation::*;
use self::editing::*;
use self::group::*;
pub(crate) use self::paragraph::*;
pub use self::scene3d::LiveScene3d;
use self::scene3d::*;
use self::shape::*;
pub(crate) use self::sprite::*;
pub(crate) use self::text::*;

#[cfg(test)]
mod tests;
