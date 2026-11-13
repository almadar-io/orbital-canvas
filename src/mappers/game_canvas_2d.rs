//! Mapper for the `canvas-2d` pattern — walks the `drawables` descriptor
//! tree (`draw-sprite` / `draw-shape` / `draw-text` atoms + their layer
//! molecules) into a screen-space display-list packet.
//!
//! Exact port of `@almadar/ui`'s Canvas2D host:
//! - projection: `lib/drawable/projector.ts` (`create2DProjector`) → [`crate::projector`]
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

use super::scene::Measured;
use crate::asset;
use crate::atlas;
use crate::mappers::learning::shape::parse_path_data;
use crate::mappers::learning::Xf;
use crate::packet::{
    DrawCommand, FillPattern, GameRenderPacket, Gradient, GradientStop, LineCap, LineJoin,
    PathSegment, Rect, SdfKind, SdfMaterial, SdfShadow, SdfStroke, Stroke, TextAlign, Vec2,
};
use crate::projector::{Anchor, Projection, Projector, ScenePos, TILE_WIDTH};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::{OnceLock, RwLock};

const NS: &str = "almadar::game_renderer::mapper";

/// `isometric.ts` BACKGROUND_FALLBACK_COLOR (`#1a1a2e`).
const BACKGROUND_FALLBACK: [f32; 4] = [26.0 / 255.0, 26.0 / 255.0, 46.0 / 255.0, 1.0];

/// Segments used to approximate an ellipse fill with a polygon fan.
const ELLIPSE_SEGMENTS: usize = 24;

static GAPS: OnceLock<RwLock<HashSet<&'static str>>> = OnceLock::new();

fn gap_once(key: &'static str, detail: &str) {
    let mut set = GAPS
        .get_or_init(Default::default)
        .write()
        .expect("gap log poisoned");
    if set.insert(key) {
        tracing::warn!(target: NS, gap = key, detail = %detail, "mapper gap — drawable skipped");
    }
}

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
}

/// What the drawable walk gathers besides display commands: regions and measured texts, both in paint order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Gathered {
    pub regions: Vec<Region>,
    pub measured: Vec<Measured>,
    pub text_hits: Vec<super::scene::TextHit>,
    pub semantic: Vec<super::scene::SemanticNode>,
    /// Each `draw-text` id's painted text, and each semantic node a `labelledBy` names: resolved once every node is
    /// walked, so a label painted after its control still names it.
    texts: std::collections::HashMap<String, String>,
    labelled: Vec<(usize, String)>,
    valued: Vec<(usize, String)>,
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
    let mut gathered = Gathered::default();
    // An orbital-painter canvas is sized by its declared world, as the web host sizes it.
    let orbital = props.get("painter").and_then(Value::as_str) == Some("orbital");
    let (width_key, height_key) = if orbital {
        ("worldWidth", "worldHeight")
    } else {
        ("width", "height")
    };
    let width = get_f32(props, width_key, 800.0);
    let height = get_f32(props, height_key, 600.0);
    let scale = get_f32(props, "scale", 0.4);
    let projection = match props.get("projection").and_then(Value::as_str) {
        Some(s) => Projection::from_str(s).unwrap_or_else(|| {
            gap_once(
                "projection",
                "unknown projection string — defaulting to isometric",
            );
            Projection::Isometric
        }),
        None => Projection::Isometric,
    };
    let drawables = props
        .get("drawables")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    // Scene extent from drawn positions → baseOffsetX (Canvas2D gridExtent +
    // baseOffsetX useMemo): (maxY + 1 - 1) * scaledTileWidth / 2 for diamond
    // layouts, 0 for square/free.
    let mut max_x = 0.0f32;
    let mut max_y = 0.0f32;
    for node in &drawables {
        collect_positions(node, &mut |p| {
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

    for node in &drawables {
        walk(
            node,
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
            return Err(invalid("semantic.labelledBy", format!("names `{target}`, which is no draw-text with an id")));
        };
        gathered.semantic[index].name = text.clone();
    }
    for (index, target) in std::mem::take(&mut gathered.valued) {
        let Some(text) = gathered.texts.get(&target) else {
            return Err(invalid("semantic.valueFrom", format!("names `{target}`, which is no draw-text with an id")));
        };
        gathered.semantic[index].value = Some(text.clone());
    }

    super::layer_props::check_target_cycles(&draw_commands)?;
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
        animations: Vec::new(),
        time_ms: 0.0,
        pointer: None,
    };
    Ok((packet, gathered))
}

/// Why a `canvas` render-ui node cannot become a 2D display-list packet.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CanvasSceneError {
    #[error("scene is not an object")]
    NotAnObject,
    #[error("canvas mode `{0}` has no native painter (only `2d`)")]
    UnsupportedMode(String),
    #[error("`{0}` has nothing the Rust renderer can draw")]
    NothingToDraw(String),
    #[error("3D canvas contract is unsupported: {0}")]
    Unsupported3DContract(String),
    #[error("`{field}`: {detail}")]
    InvalidProp { field: String, detail: String },
    #[error(transparent)]
    Fx(#[from] crate::fx_projection::FxError),
    #[error(transparent)]
    Skin(#[from] crate::skinning_2d::SkinError),
    #[error(transparent)]
    SkinnedMesh(super::skinned_mesh_2d::SkinMeshError),
    #[error("`{pattern}` requires `{field}`")]
    MissingField { pattern: String, field: String },
    #[error("draw-sprite does not resolve: {0}")]
    UnresolvedSprite(String),
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
    canvas_node_props(node).and_then(|props| from_props_mapped(&Value::Object(props), epoch_now_ms))
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
    let mut props = canvas_node_props(node)?;
    props.insert("width".into(), Value::from(width.max(1.0)));
    props.insert("height".into(), Value::from(height.max(1.0)));
    from_props_mapped(&Value::Object(props), epoch_now_ms)
}

fn canvas_node_props(node: &Value) -> Result<serde_json::Map<String, Value>, CanvasSceneError> {
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
    if !props.contains_key("drawables") {
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
    Ok(props)
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
    walk_paint(node, projector, to_screen, zoom, epoch_now_ms, gathered, out)?;
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
            return Err(invalid("semantic.labelledBy", "a semantic node takes its name from `name` or `labelledBy`, not both"));
        }
        gathered.labelled.push((gathered.semantic.len(), target));
    }
    if let Some(target) = text("valueFrom") {
        if semantic.get("value").is_some() {
            return Err(invalid("semantic.valueFrom", "a semantic node takes its value from `value` or `valueFrom`, not both"));
        }
        gathered.valued.push((gathered.semantic.len(), target));
    }
    gathered.semantic.push(super::scene::SemanticNode {
        id: id.to_owned(),
        role: text("role").ok_or_else(|| invalid("semantic.role", "a semantic node needs a role"))?,
        name: text("name").unwrap_or_default(),
        value: text("value"),
        checked: flag("checked"),
        disabled: flag("disabled").unwrap_or(false),
        focused: flag("focused").unwrap_or(false),
        caret: index("caret"),
        selection_start: index("selectionStart"),
        selection_end: index("selectionEnd"),
        multiline: flag("multiline").unwrap_or(false),
        order: semantic
            .get("order")
            .and_then(Value::as_f64)
            .map_or(paint_order, |o| o as f32),
        rect,
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
            use super::skinned_mesh_2d::SkinMeshError;
            match super::skinned_mesh_2d::paint(node, projector, to_screen) {
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
            let particles = super::emitter::expand(node, epoch_now_ms)
                .map_err(|e| invalid(e.field, e.detail))?;
            for particle in &particles {
                walk(particle, projector, to_screen, zoom, epoch_now_ms, gathered, out)?;
            }
        }
        Some("draw-fx-layer") => {
            for drawable in crate::fx_projection::expand_layer(
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

/// A live world FX layer needs the host's paint clock even when no authored
/// packet keyframe animation is present.
pub fn has_active_fx(node: &Value) -> bool {
    let animated_sprite = |sprite: &Value| {
        sprite.get("animation").and_then(Value::as_str).is_some()
            && sprite
                .get("asset")
                .and_then(|asset| asset.get("atlas"))
                .and_then(Value::as_str)
                .is_some()
    };
    if node.get("type").and_then(Value::as_str) == Some("draw-sprite") && animated_sprite(node) {
        return true;
    }
    if node.get("type").and_then(Value::as_str) == Some("draw-sprite-layer")
        && items(node).iter().any(animated_sprite)
    {
        return true;
    }
    if node.get("type").and_then(Value::as_str) == Some("draw-fx-layer")
        && node
            .get("items")
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty())
    {
        return true;
    }
    if matches!(
        node.get("type").and_then(Value::as_str),
        Some("draw-shape" | "draw-group" | "draw-text")
    ) && node.get("animation").is_some_and(|animation| {
        animation
            .get("durationMs")
            .and_then(Value::as_f64)
            .is_some_and(|v| v > 0.0)
            && animation
                .get("keyframes")
                .and_then(Value::as_array)
                .is_some_and(|v| !v.is_empty())
    }) {
        return true;
    }
    if node
        .pointer("/material/anchor")
        .is_some_and(|anchor| anchor.as_f64().is_some())
    {
        return true;
    }
    if node.get("type").and_then(Value::as_str) == Some("draw-text")
        && node.get("caret").and_then(Value::as_f64).is_some_and(|caret| caret >= 0.0)
    {
        return true;
    }
    ["props", "drawables", "children", "items"]
        .into_iter()
        .filter_map(|key| node.get(key))
        .any(|child| match child {
            Value::Array(items) => items.iter().any(has_active_fx),
            Value::Object(_) => has_active_fx(child),
            _ => false,
        })
}

fn paint_group(
    node: &Value,
    projector: &Projector,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
    zoom: f32,
    epoch_now_ms: f64,
    gathered: &mut Gathered,
    out: &mut Vec<DrawCommand>,
) -> Result<(), CanvasSceneError> {
    let Some(position) = node.get("position").and_then(parse_scene_pos) else {
        gap_once("group-position", "draw-group without position");
        return Ok(());
    };
    let (gx, gy) = projector.project(position);
    let (zero_x, zero_y) = to_screen(0.0, 0.0);
    let animation = (epoch_now_ms > 0.0)
        .then(|| node.get("animation"))
        .flatten();
    let animated_number = |key: &str, default: f32| {
        animation
            .and_then(|animation| crate::animate::drawable_track(animation, epoch_now_ms, key))
            .and_then(|value| value.as_f64())
            .map(|value| value as f32)
            .unwrap_or(default)
    };
    let (group_x, group_y) = to_screen(
        gx + animated_number("offsetX", 0.0) * projector.tile_width,
        gy + animated_number("offsetY", 0.0) * projector.tile_width,
    );
    let rotation_z = node
        .get("rotation")
        .and_then(Value::as_array)
        .and_then(|angles| angles.get(2))
        .and_then(Value::as_f64)
        .unwrap_or(0.0) as f32;
    let radians = get_f32(node, "rotate", 0.0) + rotation_z + animated_number("rotateZ", 0.0);
    let start = out.len();
    let (region_start, measured_start, hits_start) = (gathered.regions.len(), gathered.measured.len(), gathered.text_hits.len());
    for child in items(node) {
        walk(
            child,
            projector,
            to_screen,
            zoom,
            epoch_now_ms,
            gathered,
            out,
        )?;
    }
    if let Some(spec) = node.get("target").filter(|t| !t.is_null()) {
        gathered.regions.truncate(region_start);
        gathered.text_hits.truncate(hits_start);
        return super::layer_props::collect_target(spec, (zero_x, zero_y), out, start);
    }
    if radians != 0.0
        && out[start..]
            .iter()
            .any(|cmd| matches!(cmd, DrawCommand::Text { .. }))
    {
        out.truncate(start);
        gathered.regions.truncate(region_start);
        gathered.measured.truncate(measured_start);
        gathered.text_hits.truncate(hits_start);
        gap_once(
            "group-rotated-text",
            "draw-group rotating text needs a vector glyph path",
        );
        return Ok(());
    }
    if let Some(radius) = node.get("smoothUnion").filter(|r| !r.is_null()) {
        blend_group_shapes(radius, projector.tile_width * zoom, out, start)?;
    }
    let mut rect_clip = false;
    let clip = match node.get("clip").filter(|clip| !clip.is_null()) {
        Some(Value::String(clip)) if !clip.is_empty() => {
            let parsed = parse_path_data(
                clip,
                Xf {
                    s: projector.tile_width * zoom,
                    ox: zero_x,
                    oy: zero_y,
                },
            );
            if parsed.is_none() {
                out.truncate(start);
                gap_once("group-clip-path", "malformed draw-group SVG clip path");
                return Ok(());
            }
            parsed
        }
        Some(Value::Object(spec)) => {
            rect_clip = true;
            Some(rect_clip_segments(spec, projector, to_screen, zoom)?)
        }
        _ => None,
    };
    if let Some(segments) = clip {
        let commands = out.drain(start..).collect();
        out.push(DrawCommand::ClipGroup {
            clip: segments,
            commands,
        });
    }
    let origin = Vec2 {
        x: zero_x,
        y: zero_y,
    };
    let translation = Vec2 {
        x: group_x - zero_x,
        y: group_y - zero_y,
    };
    let scale = get_f32(node, "scale", 1.0) * animated_number("scale", 1.0);
    let opacity = get_f32(node, "opacity", 1.0) * animated_number("opacity", 1.0);
    crate::animate::transform_group_commands(
        &mut out[start..],
        origin,
        translation,
        scale,
        radians,
        opacity,
    );
    // regions ride the group transform as sprites, which turn about their own centre like the group's marks
    let mut carriers: Vec<DrawCommand> = gathered.regions[region_start..]
        .iter()
        .map(|region| DrawCommand::Sprite {
            texture: String::new(),
            src: Rect::default(),
            dst: region.rect,
            rotation: region.rotation,
            opacity: 1.0,
            flip_x: false,
            flip_y: false,
        })
        .collect();
    crate::animate::transform_group_commands(
        &mut carriers,
        origin,
        translation,
        scale,
        radians,
        1.0,
    );
    for (region, carrier) in gathered.regions[region_start..].iter_mut().zip(&carriers) {
        if let DrawCommand::Sprite { dst, rotation, .. } = carrier {
            region.rect = *dst;
            region.rotation = *rotation;
        }
    }
    // a text hit's lines ride the same transform (text in a rotated group is refused above, so it only scales and moves)
    for hit in &mut gathered.text_hits[hits_start..] {
        for line in &mut hit.lines {
            let (x0, x1) = (line.edges.iter().copied().fold(f32::INFINITY, f32::min), line.edges.iter().copied().fold(f32::NEG_INFINITY, f32::max));
            let before = Rect { x: x0, y: line.top, w: (x1 - x0).max(f32::EPSILON), h: (line.bottom - line.top).max(f32::EPSILON) };
            let mut carrier = vec![DrawCommand::Sprite { texture: String::new(), src: Rect::default(), dst: before, rotation: 0.0, opacity: 1.0, flip_x: false, flip_y: false }];
            crate::animate::transform_group_commands(&mut carrier, origin, translation, scale, radians, 1.0);
            if let Some(DrawCommand::Sprite { dst, .. }) = carrier.first() {
                for edge in &mut line.edges {
                    *edge = dst.x + (*edge - before.x) * dst.w / before.w;
                }
                line.top = dst.y;
                line.bottom = dst.y + dst.h;
            }
        }
    }
    for entry in &mut gathered.measured[measured_start..] {
        entry.width *= scale.abs();
        entry.height *= scale.abs();
    }
    if rect_clip {
        if let Some(DrawCommand::ClipGroup { clip, .. }) = out.get(start) {
            if let Some(bounds) = segments_bounds(clip) {
                cut_regions_to(&mut gathered.regions, region_start, bounds);
            }
        }
    }
    Ok(())
}

/// Replace the two SDF shapes a `smoothUnion` group painted by one [`DrawCommand::SdfBlend`] (contract 14.4).
fn blend_group_shapes(
    radius: &Value,
    unit: f32,
    out: &mut Vec<DrawCommand>,
    start: usize,
) -> Result<(), CanvasSceneError> {
    let radius = radius
        .as_f64()
        .filter(|r| r.is_finite() && *r > 0.0)
        .ok_or_else(|| invalid("smoothUnion", "a smooth union radius is a number above zero"))?;
    if !matches!(&out[start..], [DrawCommand::Sdf { .. }, DrawCommand::Sdf { .. }]) {
        return Err(invalid(
            "smoothUnion",
            "a smooth union group holds exactly two SDF draw-shape children and nothing else",
        ));
    }
    let second = out.pop();
    let first = out.pop();
    let (Some(first), Some(second)) = (first, second) else {
        return Err(invalid("smoothUnion", "a smooth union group holds two SDF shapes"));
    };
    let compatible = match (&first, &second) {
        (
            DrawCommand::Sdf { stroke: first_stroke, opacity: first_opacity, material: first_material, .. },
            DrawCommand::Sdf { stroke, shadow, opacity, material, .. },
        ) => {
            if first_stroke.is_some_and(|s| s.dash != [0.0, 0.0]) {
                Err("a blended union draws a solid stroke, not a dashed one")
            } else if stroke.is_some() || shadow.is_some() {
                Err("the second shape carries no stroke and no shadow")
            } else if opacity != first_opacity {
                Err("both shapes carry the same opacity")
            } else if material != first_material {
                Err("both shapes carry the same material, or none")
            } else {
                Ok(())
            }
        }
        _ => Err("a smooth union group holds two SDF shapes"),
    };
    compatible.map_err(|detail| invalid("smoothUnion", detail))?;
    out.push(DrawCommand::SdfBlend {
        first: Box::new(first),
        second: Box::new(second),
        radius: radius as f32 * unit,
    });
    Ok(())
}

/// The box of an authored rect clip: `x`, `y`, `width`, `height` in group units, `radius` one number or four.
fn rect_clip_segments(
    spec: &serde_json::Map<String, Value>,
    projector: &Projector,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
    zoom: f32,
) -> Result<Vec<PathSegment>, CanvasSceneError> {
    let number = |key: &str| spec.get(key).and_then(Value::as_f64).filter(|v| v.is_finite());
    let size = |key: &str| {
        number(key)
            .filter(|v| *v > 0.0)
            .ok_or_else(|| invalid("clip", format!("a rect clip needs `{key}` above zero")))
    };
    let (width, height) = (size("width")?, size("height")?);
    let origin = ScenePos {
        x: number("x").unwrap_or(0.0) as f32,
        y: number("y").unwrap_or(0.0) as f32,
    };
    let (px, py) = projector.project(origin);
    let (x, y) = to_screen(px, py);
    let unit = projector.tile_width * zoom;
    let (w, h) = (width as f32 * unit, height as f32 * unit);
    let radii = match spec.get("radius").filter(|r| !r.is_null()) {
        None => [0.0; 4],
        Some(Value::Number(n)) => [n.as_f64().unwrap_or(-1.0) as f32; 4],
        Some(Value::Array(items)) if items.len() == 4 => {
            let mut radii = [0.0; 4];
            for (slot, item) in radii.iter_mut().zip(items) {
                *slot = item.as_f64().unwrap_or(-1.0) as f32;
            }
            radii
        }
        Some(_) => {
            return Err(invalid("clip", "`radius` is one number or four (tl, tr, br, bl)"));
        }
    };
    if radii.iter().any(|r| !r.is_finite() || *r < 0.0) {
        return Err(invalid("clip", "`radius` values are zero or more"));
    }
    let limit = w.min(h) / 2.0;
    let [tl, tr, br, bl] = radii.map(|r| (r * unit).min(limit));
    const KAPPA: f32 = 0.552_284_75;
    let corner = |c1: (f32, f32), c2: (f32, f32), to: (f32, f32)| PathSegment::CubicTo {
        x1: c1.0,
        y1: c1.1,
        x2: c2.0,
        y2: c2.1,
        x: to.0,
        y: to.1,
    };
    let mut path = vec![PathSegment::MoveTo { x: x + tl, y }];
    path.push(PathSegment::LineTo { x: x + w - tr, y });
    if tr > 0.0 {
        path.push(corner((x + w - tr + tr * KAPPA, y), (x + w, y + tr - tr * KAPPA), (x + w, y + tr)));
    }
    path.push(PathSegment::LineTo { x: x + w, y: y + h - br });
    if br > 0.0 {
        path.push(corner((x + w, y + h - br + br * KAPPA), (x + w - br + br * KAPPA, y + h), (x + w - br, y + h)));
    }
    path.push(PathSegment::LineTo { x: x + bl, y: y + h });
    if bl > 0.0 {
        path.push(corner((x + bl - bl * KAPPA, y + h), (x, y + h - bl + bl * KAPPA), (x, y + h - bl)));
    }
    path.push(PathSegment::LineTo { x, y: y + tl });
    if tl > 0.0 {
        path.push(corner((x, y + tl - tl * KAPPA), (x + tl - tl * KAPPA, y), (x + tl, y)));
    }
    path.push(PathSegment::Close);
    Ok(path)
}

/// The box that holds every point of a path (curve control points included).
fn segments_bounds(segments: &[PathSegment]) -> Option<Rect> {
    let mut bounds: Option<(f32, f32, f32, f32)> = None;
    let mut add = |x: f32, y: f32| {
        bounds = Some(match bounds {
            None => (x, y, x, y),
            Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
        });
    };
    for segment in segments {
        match *segment {
            PathSegment::MoveTo { x, y } | PathSegment::LineTo { x, y } => add(x, y),
            PathSegment::CubicTo { x1, y1, x2, y2, x, y } => {
                add(x1, y1);
                add(x2, y2);
                add(x, y);
            }
            PathSegment::Arc { cx, cy, rx, ry, .. } => {
                add(cx - rx, cy - ry);
                add(cx + rx, cy + ry);
            }
            PathSegment::Close => {}
        }
    }
    bounds.map(|(x0, y0, x1, y1)| Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
}

/// Cut the regions a clipped group gathered to the clip's bounds: one wholly outside is dropped, a straddling
/// one is intersected, a rotated one stays whole.
fn cut_regions_to(regions: &mut Vec<Region>, from: usize, bounds: Rect) {
    let mut kept: Vec<Region> = Vec::new();
    for mut region in regions.drain(from..) {
        if region.rotation == 0.0 {
            let x0 = region.rect.x.max(bounds.x);
            let y0 = region.rect.y.max(bounds.y);
            let x1 = (region.rect.x + region.rect.w).min(bounds.x + bounds.w);
            let y1 = (region.rect.y + region.rect.h).min(bounds.y + bounds.h);
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            region.rect = Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
        }
        kept.push(region);
    }
    regions.extend(kept);
}

fn paint_sprite_gathering(
    node: &Value,
    projector: &Projector,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
    zoom: f32,
    epoch_now_ms: f64,
    gathered: &mut Gathered,
    out: &mut Vec<DrawCommand>,
) -> Result<(), CanvasSceneError> {
    let start = out.len();
    paint_sprite(node, projector, to_screen, zoom, epoch_now_ms, out)?;
    if let Some(id) = hit_id(node) {
        let placed = out[start..].iter().find_map(|command| match command {
            DrawCommand::Sprite { dst, rotation, .. } => Some((*dst, *rotation)),
            _ => None,
        });
        if let Some((rect, rotation)) = placed {
            gathered.regions.push(Region {
                id: id.to_string(),
                rect,
                rotation,
            });
        }
    }
    Ok(())
}

/// Axis-aligned bounds of the marks a shape painted, in packet coordinates.
fn commands_bounds(commands: &[DrawCommand]) -> Option<Rect> {
    let mut bounds: Option<(f32, f32, f32, f32)> = None;
    let mut add = |x0: f32, y0: f32, x1: f32, y1: f32| {
        bounds = Some(match bounds {
            None => (x0, y0, x1, y1),
            Some((a, b, c, d)) => (a.min(x0), b.min(y0), c.max(x1), d.max(y1)),
        });
    };
    fn walk_commands(commands: &[DrawCommand], add: &mut dyn FnMut(f32, f32, f32, f32)) {
        for command in commands {
            match command {
                DrawCommand::Quad { rect, .. } => {
                    add(rect.x, rect.y, rect.x + rect.w, rect.y + rect.h)
                }
                DrawCommand::Sdf { rect, .. } => {
                    add(rect.x, rect.y, rect.x + rect.w, rect.y + rect.h)
                }
                DrawCommand::SdfBackdrop { shape, .. } => {
                    walk_commands(std::slice::from_ref(shape.as_ref()), add)
                }
                DrawCommand::SdfBlend { first, second, .. } => {
                    walk_commands(std::slice::from_ref(first.as_ref()), add);
                    walk_commands(std::slice::from_ref(second.as_ref()), add);
                }
                DrawCommand::Poly { points, .. } => {
                    for p in points {
                        add(p.x, p.y, p.x, p.y);
                    }
                }
                DrawCommand::Path { segments, .. }
                | DrawCommand::GradientPath { segments, .. }
                | DrawCommand::PatternPath { segments, .. } => {
                    for segment in segments {
                        match segment {
                            PathSegment::MoveTo { x, y } | PathSegment::LineTo { x, y } => {
                                add(*x, *y, *x, *y)
                            }
                            PathSegment::Arc { cx, cy, rx, ry, .. } => {
                                add(cx - rx, cy - ry, cx + rx, cy + ry)
                            }
                            PathSegment::CubicTo {
                                x1,
                                y1,
                                x2,
                                y2,
                                x,
                                y,
                            } => {
                                add(*x1, *y1, *x1, *y1);
                                add(*x2, *y2, *x2, *y2);
                                add(*x, *y, *x, *y);
                            }
                            PathSegment::Close => {}
                        }
                    }
                }
                DrawCommand::Blurred { command, .. } => {
                    walk_commands(std::slice::from_ref(command.as_ref()), add)
                }
                DrawCommand::AdditiveGroup { commands } => walk_commands(commands, add),
                _ => {}
            }
        }
    }
    walk_commands(commands, &mut add);
    bounds.map(|(x0, y0, x1, y1)| Rect {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    })
}

/// The text with its keyframe `animation` resolved at the paint clock: `opacity`, `offsetX` and `offsetY`.
fn animated_text(node: &Value, epoch_now_ms: f64) -> Option<Value> {
    if epoch_now_ms <= 0.0 {
        return None;
    }
    let animation = node.get("animation")?;
    let mut animated = node.clone();
    let object = animated.as_object_mut()?;
    for key in ["opacity", "offsetX", "offsetY"] {
        if let Some(value) = crate::animate::drawable_track(animation, epoch_now_ms, key) {
            object.insert(key.to_owned(), value);
        }
    }
    Some(animated)
}

/// The canvas's 2D text painter maps through the same vector font pipeline as
/// other Canvas labels. Unsupported CSS fonts/baselines are reported rather
/// than silently given a different face or anchor. With a `width` the text is a
/// paragraph laid out from the top-left of `position` (see [`layout_paragraph`]).
/// A `draw-text`'s editing decorations (contract §11): caret, selection band and the input method's preedit.
struct TextEditing {
    caret: Option<usize>,
    selection: Option<(usize, usize)>,
    preedit: String,
    caret_color: Option<[f32; 4]>,
    selection_color: Option<[f32; 4]>,
}

impl TextEditing {
    fn read(node: &Value) -> Result<Self, CanvasSceneError> {
        let index = |field: &str| -> Result<Option<usize>, CanvasSceneError> {
            match node.get(field) {
                None | Some(Value::Null) => Ok(None),
                Some(value) => match value.as_f64() {
                    Some(v) if v.is_finite() && v < 0.0 => Ok(None),
                    Some(v) if v.is_finite() && v.fract() == 0.0 => Ok(Some(v as usize)),
                    _ => Err(invalid(field, "must be a whole number")),
                },
            }
        };
        let color = |field: &str| node.get(field).and_then(Value::as_str).and_then(parse_css_color);
        let selection = match (index("selectionStart")?, index("selectionEnd")?) {
            (Some(a), Some(b)) if a != b => Some((a.min(b), a.max(b))),
            _ => None,
        };
        Ok(Self {
            caret: index("caret")?,
            selection,
            preedit: node.get("preedit").and_then(Value::as_str).unwrap_or_default().to_string(),
            caret_color: color("caretColor"),
            selection_color: color("selectionColor"),
        })
    }

    fn active(&self) -> bool {
        self.caret.is_some() || self.selection.is_some()
    }

    /// The text as painted: the preedit is shown at the caret, though it is not part of the text.
    fn shown(&self, text: &str) -> String {
        match self.caret.filter(|_| !self.preedit.is_empty()) {
            Some(caret) => {
                let at = text.char_indices().nth(caret).map_or(text.len(), |(i, _)| i);
                format!("{}{}{}", &text[..at], self.preedit, &text[at..])
            }
            None => text.to_string(),
        }
    }

    /// A text index as an index of the shown text.
    fn shown_index(&self, index: usize) -> usize {
        match self.caret {
            Some(caret) if index >= caret => index + self.preedit.chars().count(),
            _ => index,
        }
    }
}

/// The layout of each painted line of an editable text: its first character index in `shown`, width, caret edges and
/// per-character intervals from the shaped, bidi-reordered line. `None` when the lines do not join back into `shown`
/// exactly or a character has no shaped glyph.
fn caret_positions(lines: &[String], shown: &str, size: f32, bold: bool, family: crate::packet::TextFamily) -> Option<Vec<super::edit_layout::EditLayout>> {
    if lines.concat() != shown {
        return None;
    }
    let mut start = 0;
    lines
        .iter()
        .map(|line| {
            let laid = super::edit_layout::layout_line(line, start, size, bold, family);
            start += line.chars().count();
            laid
        })
        .collect()
}

/// One painted line of an editable text, in screen units: left edge, band top and height, first index, edges.
struct EditLine {
    left: f32,
    top: f32,
    height: f32,
    start: usize,
    edges: Vec<f32>,
    spans: Vec<(f32, f32)>,
}

/// The merged visual intervals of the characters `from..to` of `line`, relative to its left edge.
fn visual_ranges(line: &EditLine, from: usize, to: usize) -> Vec<(f32, f32)> {
    super::edit_layout::EditLayout { start: line.start, width: 0.0, edges: Vec::new(), spans: line.spans.clone() }.ranges(from, to)
}

/// Paint `editing`'s selection bands behind the text (inserted at `text_start`) and its preedit underline and caret
/// over it. The caret blinks on the scene clock (530 ms on, 530 ms off); a scene drawn with no clock shows it steady.
fn paint_editing(
    editing: &TextEditing,
    lines: &[EditLine],
    text_color: [f32; 4],
    size: f32,
    epoch_now_ms: f64,
    text_start: usize,
    out: &mut Vec<DrawCommand>,
) {
    let x_of = |line: &EditLine, index: usize| line.left + line.edges[(index - line.start).min(line.edges.len() - 1)];
    let holds = |line: &EditLine, index: usize| index >= line.start && index < line.start + line.edges.len();
    if let Some((from, to)) = editing.selection {
        let (from, to) = (editing.shown_index(from), editing.shown_index(to));
        let band = editing.selection_color.unwrap_or([text_color[0], text_color[1], text_color[2], text_color[3] * 0.3]);
        let bands: Vec<DrawCommand> = lines
            .iter()
            .flat_map(|line| {
                let end = line.start + line.edges.len() - 1;
                let (a, b) = (from.max(line.start), to.min(end));
                let ranges = if a < b { visual_ranges(line, a - line.start, b - line.start) } else { Vec::new() };
                ranges.into_iter().map(move |(x0, x1)| DrawCommand::Quad {
                    rect: Rect { x: line.left + x0, y: line.top, w: x1 - x0, h: line.height },
                    color: band,
                })
            })
            .collect();
        out.splice(text_start..text_start, bands);
    }
    let Some(caret) = editing.caret else { return };
    let preedit = editing.preedit.chars().count();
    let at = editing.shown_index(caret);
    let line = lines.iter().rev().find(|line| holds(line, at)).or_else(|| lines.last());
    let Some(line) = line else { return };
    if preedit > 0 {
        for (x0, x1) in visual_ranges(line, caret.saturating_sub(line.start), (caret + preedit).saturating_sub(line.start)) {
            out.push(DrawCommand::Quad {
                rect: Rect { x: line.left + x0, y: line.top + line.height - (size * 0.08).max(1.0), w: x1 - x0, h: (size * 0.06).max(1.0) },
                color: text_color,
            });
        }
    }
    let on = epoch_now_ms <= 0.0 || (epoch_now_ms / 530.0).floor() as i64 % 2 == 0;
    if on {
        let w = (size * 0.07).max(1.0);
        out.push(DrawCommand::Quad {
            rect: Rect { x: x_of(line, at) - w / 2.0, y: line.top, w, h: line.height },
            color: editing.caret_color.unwrap_or(text_color),
        });
    }
}

fn paint_text(
    node: &Value,
    projector: &Projector,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
    zoom: f32,
    epoch_now_ms: f64,
    gathered: &mut Gathered,
    out: &mut Vec<DrawCommand>,
) -> Result<(), CanvasSceneError> {
    let animated = animated_text(node, epoch_now_ms);
    let node = animated.as_ref().unwrap_or(node);
    let Some(position) = node.get("position").and_then(parse_scene_pos) else {
        gap_once("text-position", "draw-text without position");
        return Ok(());
    };
    let Some(text) = node.get("text").and_then(Value::as_str) else {
        gap_once("text-value", "draw-text without text");
        return Ok(());
    };
    let Some(mut color) = node
        .get("color")
        .and_then(Value::as_str)
        .and_then(parse_css_color)
    else {
        gap_once("text-color", "draw-text color is not a supported CSS color");
        return Ok(());
    };
    let (size, weight, family) = match parse_text_font(node.get("font").and_then(Value::as_str)) {
        Some(font) => font,
        None => {
            gap_once(
                "text-font",
                "draw-text CSS font is not carried by the native font pack",
            );
            return Ok(());
        }
    };
    let align = match node.get("align").and_then(Value::as_str).unwrap_or("left") {
        "left" | "start" => TextAlign::Left,
        "center" => TextAlign::Center,
        "right" | "end" => TextAlign::Right,
        _ => {
            gap_once("text-align", "draw-text alignment is not supported");
            return Ok(());
        }
    };
    let size = size * zoom;
    color[3] *= get_f32(node, "opacity", 1.0);
    let line_height = match node.get("lineHeight") {
        None | Some(Value::Null) => 1.3,
        Some(value) => match value.as_f64() {
            Some(v) if v.is_finite() && v > 0.0 => v as f32,
            _ => return Err(invalid("lineHeight", "must be a number above zero")),
        },
    };
    let bold = weight >= 600;
    let editing = TextEditing::read(node)?;
    let original = text;
    let shown = editing.shown(text);
    let text = shown.as_str();
    let id = node.get("id").and_then(Value::as_str);
    let measure = node
        .get("measure")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    if let Some(width) = node.get("width").filter(|w| !w.is_null()) {
        let width = match width.as_f64() {
            Some(w) if w.is_finite() && w > 0.0 => w as f32 * zoom,
            _ => return Err(invalid("width", "must be a number above zero")),
        };
        let max_lines = match node.get("maxLines") {
            None | Some(Value::Null) => None,
            Some(value) => match value.as_u64() {
                Some(n) if n >= 1 => Some(n as usize),
                _ => return Err(invalid("maxLines", "must be a whole number of one or more")),
            },
        };
        let wrap = node.get("wrap").and_then(Value::as_bool).unwrap_or(true);
        let truncate = node
            .get("truncate")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let (x, y) = projector.anchor_point(position, Anchor::TopLeft);
        let (sx, sy) = to_screen(
            x + get_f32(node, "offsetX", 0.0),
            y + get_f32(node, "offsetY", 0.0),
        );
        let paragraph = layout_paragraph(
            text,
            size,
            bold,
            family,
            ParagraphBox {
                width: width,
                wrap: wrap,
                max_lines: max_lines,
                truncate: truncate,
            },
        );
        let line_px = size * line_height;
        let text_start = out.len();
        for (i, line) in paragraph.lines.iter().enumerate() {
            if line.is_empty() {
                continue;
            }
            let x = match align {
                TextAlign::Left => sx,
                TextAlign::Center => sx + width / 2.0,
                TextAlign::Right => sx + width,
            };
            out.push(DrawCommand::Text {
                text: line.clone(),
                pos: Vec2 {
                    x,
                    y: sy + (i as f32 + 0.5) * line_px,
                },
                size,
                color,
                align,
                weight,
                halo: None,
                family,
            });
        }
        if let Some(positions) = editing.active().then(|| caret_positions(&paragraph.lines, text, size, bold, family)) {
            match positions {
                Some(positions) => {
                    let edit_lines: Vec<EditLine> = positions
                        .into_iter()
                        .enumerate()
                        .map(|(i, laid)| {
                            let w = laid.width;
                            let left = match align {
                                TextAlign::Left => sx,
                                TextAlign::Center => sx + (width - w) / 2.0,
                                TextAlign::Right => sx + width - w,
                            };
                            EditLine { left, top: sy + i as f32 * line_px, height: line_px, start: laid.start, edges: laid.edges, spans: laid.spans }
                        })
                        .collect();
                    paint_editing(&editing, &edit_lines, color, size, epoch_now_ms, text_start, out);
                }
                None => gap_once("text-caret-layout", "draw-text caret and selection need lines that join back into the text and shape every character"),
            }
        }
        let lines = paragraph.lines.len();
        let height = lines as f32 * line_px;
        if let Some(id) = id {
            if hit_id(node).is_some() {
                gathered.regions.push(Region {
                    id: id.to_string(),
                    rect: Rect {
                        x: sx,
                        y: sy,
                        w: width,
                        h: height,
                    },
                    rotation: 0.0,
                });
                if text == original {
                    if let Some(positions) = caret_positions(&paragraph.lines, text, size, bold, family) {
                        gathered.text_hits.push(super::scene::TextHit {
                            id: id.to_string(),
                            lines: positions
                                .into_iter()
                                .enumerate()
                                .map(|(i, laid)| {
                                    let (start, w) = (laid.start, laid.width);
                                    let edges = laid.edges;
                                    let left = match align {
                                        TextAlign::Left => sx,
                                        TextAlign::Center => sx + (width - w) / 2.0,
                                        TextAlign::Right => sx + width - w,
                                    };
                                    super::scene::TextHitLine {
                                        top: sy + i as f32 * line_px,
                                        bottom: sy + (i + 1) as f32 * line_px,
                                        start,
                                        edges: edges.into_iter().map(|e| left + e).collect(),
                                    }
                                })
                                .collect(),
                        });
                    }
                }
            }
            if measure {
                gathered.measured.push(Measured {
                    id: id.to_string(),
                    width: paragraph.widths.iter().copied().fold(0.0, f32::max),
                    height,
                    lines: lines as u32,
                });
            }
        }
        return Ok(());
    }

    let anchor = node
        .get("anchor")
        .and_then(Value::as_str)
        .and_then(Anchor::from_str)
        .unwrap_or(Anchor::Center);
    let (x, y) = projector.anchor_point(position, anchor);
    let x = x + get_f32(node, "offsetX", 0.0);
    let y = y + get_f32(node, "offsetY", 0.0);
    let (sx, sy) = to_screen(x, y);
    let Some((asc, desc)) = crate::vector::text_font_metrics(size) else {
        gap_once("text-face", "native text face is unavailable");
        return Ok(());
    };
    let mid = (asc + desc) / 2.0;
    let sy = match node
        .get("baseline")
        .and_then(Value::as_str)
        .unwrap_or("alphabetic")
    {
        "alphabetic" => sy - mid,
        "middle" => sy,
        "top" => sy + asc - mid,
        "bottom" => sy + desc - mid,
        _ => {
            gap_once(
                "text-baseline",
                "draw-text baseline is not carried by the native font pack",
            );
            return Ok(());
        }
    };
    if let Some(id) = id {
        let tracked = hit_id(node).is_some();
        if tracked || measure {
            let w = crate::vector::shape(text, size, bold, family).width;
            let h = size * line_height;
            let left = match align {
                TextAlign::Left => sx,
                TextAlign::Center => sx - w / 2.0,
                TextAlign::Right => sx - w,
            };
            if tracked {
                gathered.regions.push(Region {
                    id: id.to_string(),
                    rect: Rect {
                        x: left,
                        y: sy - h / 2.0,
                        w,
                        h,
                    },
                    rotation: 0.0,
                });
                if text == original {
                    if let Some(mut positions) = caret_positions(&[text.to_string()], text, size, bold, family) {
                        let laid = positions.remove(0);
                        let (start, edges) = (laid.start, laid.edges);
                        gathered.text_hits.push(super::scene::TextHit {
                            id: id.to_string(),
                            lines: vec![super::scene::TextHitLine {
                                top: sy - h / 2.0,
                                bottom: sy + h / 2.0,
                                start,
                                edges: edges.into_iter().map(|e| left + e).collect(),
                            }],
                        });
                    }
                }
            }
            if measure {
                gathered.measured.push(Measured {
                    id: id.to_string(),
                    width: w,
                    height: h,
                    lines: 1,
                });
            }
        }
    }
    let text_start = out.len();
    out.push(DrawCommand::Text {
        text: text.to_owned(),
        pos: Vec2 { x: sx, y: sy },
        size,
        color,
        align,
        weight,
        halo: None,
        family,
    });
    if editing.active() {
        match caret_positions(&[text.to_string()], text, size, bold, family) {
            Some(mut positions) => {
                let laid = positions.remove(0);
                let w = laid.width;
                let left = match align {
                    TextAlign::Left => sx,
                    TextAlign::Center => sx - w / 2.0,
                    TextAlign::Right => sx - w,
                };
                let h = size * line_height;
                let line = EditLine { left, top: sy - h / 2.0, height: h, start: laid.start, edges: laid.edges, spans: laid.spans };
                paint_editing(&editing, &[line], color, size, epoch_now_ms, text_start, out);
            }
            None => gap_once("text-caret-layout", "draw-text caret and selection need lines that join back into the text and shape every character"),
        }
    }
    Ok(())
}

/// The lines of a paragraph and each line's advance width, in the units of `size`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Paragraph {
    pub lines: Vec<String>,
    pub widths: Vec<f32>,
}

/// Lay `text` out in lines of at most `width`. Line breaks fall at the opportunities of [`break_segments`]; a
/// segment wider than the line overflows it (or is cut with an ellipsis when `truncate`). `max_lines` keeps the
/// first lines, the last ending in an ellipsis when `truncate`. Widths come from the shaper's advances.
/// How a paragraph breaks: its line width and the wrap, line-cap and ellipsis rules.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ParagraphBox {
    pub width: f32,
    pub wrap: bool,
    pub max_lines: Option<usize>,
    pub truncate: bool,
}

pub(crate) fn layout_paragraph(
    text: &str,
    size: f32,
    bold: bool,
    family: crate::packet::TextFamily,
    frame: ParagraphBox,
) -> Paragraph {
    let ParagraphBox {
        width,
        wrap,
        max_lines,
        truncate,
    } = frame;
    let measure = |s: &str| crate::vector::shape(s, size, bold, family).width;
    let mut lines: Vec<String> = Vec::new();
    for hard in text.split('\n') {
        if !wrap {
            lines.push(hard.trim_end().to_string());
            continue;
        }
        let mut line = String::new();
        for segment in break_segments(hard) {
            let candidate = format!("{line}{segment}");
            if line.is_empty() || measure(candidate.trim_end()) <= width {
                line = candidate;
            } else {
                lines.push(line.trim_end().to_string());
                line = segment.to_string();
            }
        }
        lines.push(line.trim_end().to_string());
    }
    let mut cut = false;
    if let Some(max) = max_lines {
        if lines.len() > max {
            lines.truncate(max);
            cut = true;
        }
    }
    if truncate {
        let last = lines.len().saturating_sub(1);
        for (i, line) in lines.iter_mut().enumerate() {
            if (i == last && cut) || measure(line.as_str()) > width {
                *line = ellipsize(line, width, &measure);
            }
        }
    }
    let widths = lines.iter().map(|line| measure(line.as_str())).collect();
    Paragraph { lines, widths }
}

/// `line` with its tail replaced by an ellipsis so that it fits `width`.
fn ellipsize(line: &str, width: f32, measure: &dyn Fn(&str) -> f32) -> String {
    let mut kept: Vec<char> = line.trim_end().chars().collect();
    loop {
        let mut candidate: String = kept.iter().collect();
        candidate.push('\u{2026}');
        if kept.is_empty() || measure(&candidate) <= width {
            return candidate;
        }
        kept.pop();
        while kept.last().is_some_and(|c| c.is_whitespace()) {
            kept.pop();
        }
    }
}

/// Split one hard line into pieces a line may break between: after spaces (UAX 14 LB18, none before a space or
/// around a no-break space), after a hyphen unless a digit follows (LB21, LB25) or a zero-width space, and between
/// ideographs, kana and Hangul syllables (LB31).
fn break_segments(text: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    let mut start = 0;
    let mut previous: Option<char> = None;
    for (i, c) in text.char_indices() {
        if let Some(p) = previous {
            if breaks_between(p, c) {
                segments.push(&text[start..i]);
                start = i;
            }
        }
        previous = Some(c);
    }
    if start < text.len() {
        segments.push(&text[start..]);
    }
    segments
}

fn is_ideographic(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0x20000..=0x2FFFF)
}

fn breaks_between(previous: char, next: char) -> bool {
    if previous == '\u{00A0}' || next == '\u{00A0}' || next.is_whitespace() {
        return false;
    }
    if previous.is_whitespace() || previous == '\u{200B}' {
        return true;
    }
    if matches!(previous, '-' | '\u{2010}' | '\u{2013}') {
        return !next.is_ascii_digit();
    }
    is_ideographic(previous) || is_ideographic(next)
}

pub(crate) fn parse_text_font(
    font: Option<&str>,
) -> Option<(f32, u16, crate::packet::TextFamily)> {
    use crate::packet::TextFamily;
    let Some(font) = font else {
        return Some((10.0, 400, TextFamily::Sans));
    };
    let mut tokens = font.split_whitespace();
    let first = tokens.next()?;
    let (weight, size) = match first {
        "bold" => (700, tokens.next()?),
        "normal" => (400, tokens.next()?),
        numeric if numeric.bytes().all(|b| b.is_ascii_digit()) => {
            let weight = numeric
                .parse::<u16>()
                .ok()
                .filter(|w| (1..=1000).contains(w))?;
            (weight, tokens.next()?)
        }
        _ => (400, first),
    };
    let size = size.strip_suffix("px")?.parse::<f32>().ok()?;
    if !size.is_finite() || size <= 0.0 {
        return None;
    }
    let family = tokens.collect::<Vec<_>>().join(" ");
    match family.as_str() {
        "sans-serif" | "system-ui" | "system-ui, sans-serif" => {
            Some((size, weight, TextFamily::Sans))
        }
        "monospace" => Some((size, weight, TextFamily::Mono)),
        "serif" => Some((size, weight, TextFamily::Serif)),
        _ => None,
    }
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

/// `DrawSprite.tsx paintSprite`.
fn paint_sprite(
    node: &Value,
    projector: &Projector,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
    zoom: f32,
    epoch_now_ms: f64,
    out: &mut Vec<DrawCommand>,
) -> Result<(), CanvasSceneError> {
    let Some(position) = node.get("position").and_then(parse_scene_pos) else {
        gap_once("sprite-position", "draw-sprite without position");
        return Ok(());
    };
    if let Some(texture) = node.get("texture").filter(|t| !t.is_null()) {
        let texture = super::layer_props::target_texture(texture)?;
        super::layer_props::require_sprite_size(node)?;
        let src = node.get("frame").and_then(parse_xywh).unwrap_or_default();
        out.push(DrawCommand::Sprite {
            texture,
            src,
            dst: sprite_dst(node, position, projector, to_screen, zoom, src),
            rotation: get_f32(node, "rotation", 0.0),
            opacity: get_f32(node, "opacity", 1.0),
            flip_x: node.get("flipX").and_then(Value::as_bool).unwrap_or(false),
            flip_y: false,
        });
        return Ok(());
    }
    let asset = node.get("asset");
    let url = asset
        .and_then(|a| a.get("url"))
        .and_then(Value::as_str)
        .or_else(|| asset.and_then(Value::as_str));
    let Some(url) = url else {
        return Err(CanvasSceneError::UnresolvedSprite(
            "no resolvable asset.url".into(),
        ));
    };
    let _ = asset::image_info(url);
    if asset::image_failed(url) {
        return Err(CanvasSceneError::UnresolvedSprite(format!(
            "image `{url}` failed to load"
        )));
    }
    if node.get("shadow").is_some_and(|shadow| !shadow.is_null()) {
        gap_once(
            "sprite-shadow",
            "draw-sprite shadow needs a GPU silhouette blur pass",
        );
        return Ok(());
    }

    // Source rect resolution order (DrawSprite.tsx 49-59): explicit frame →
    // atlas sub-rect → whole image. Unresolved = resource in-flight → skip.
    let src = if let Some(frame) = node.get("frame").and_then(parse_xywh) {
        frame
    } else if let (Some(animation), Some(atlas_url)) = (
        node.get("animation").and_then(Value::as_str),
        asset.and_then(|a| a.get("atlas")).and_then(Value::as_str),
    ) {
        let Some(atlas) = atlas::parsed_atlas(atlas_url) else {
            if asset::fetch_failed(atlas_url) || asset::asset_bytes(atlas_url).is_some() {
                return Err(CanvasSceneError::UnresolvedSprite(format!(
                    "atlas `{atlas_url}` does not parse"
                )));
            }
            return Ok(());
        };
        if matches!(&*atlas, atlas::ParsedAtlas::SpriteSheet(_)) {
            let time_ms = node
                .get("clockMs")
                .and_then(Value::as_f64)
                .unwrap_or(epoch_now_ms);
            let frame = atlas::sprite_sheet_frame(
                &atlas,
                animation,
                node.get("loop").and_then(Value::as_bool),
                time_ms,
            );
            let Some(frame) = frame else {
                return Err(CanvasSceneError::UnresolvedSprite(format!(
                    "atlas `{atlas_url}` has no animation `{animation}`"
                )));
            };
            frame
        } else if let Some(sprite) = asset.and_then(|a| a.get("sprite")).and_then(Value::as_str) {
            let Some(frame) = atlas::sub_rect_for(&atlas, sprite) else {
                return Err(CanvasSceneError::UnresolvedSprite(format!(
                    "atlas `{atlas_url}` has no sprite `{sprite}`"
                )));
            };
            frame
        } else {
            let Some((w, h)) = asset::image_info(url) else {
                return Ok(());
            };
            Rect {
                x: 0.0,
                y: 0.0,
                w: w as f32,
                h: h as f32,
            }
        }
    } else if let (Some(atlas_url), Some(sprite)) = (
        asset.and_then(|a| a.get("atlas")).and_then(Value::as_str),
        asset.and_then(|a| a.get("sprite")).and_then(Value::as_str),
    ) {
        asset::ensure_asset(atlas_url);
        let Some(r) = atlas::resolve_atlas_rect(atlas_url, sprite) else {
            if asset::fetch_failed(atlas_url) || asset::asset_bytes(atlas_url).is_some() {
                return Err(CanvasSceneError::UnresolvedSprite(format!(
                    "atlas `{atlas_url}` has no sprite `{sprite}`"
                )));
            }
            return Ok(());
        };
        r
    } else {
        let Some((w, h)) = asset::image_info(url) else {
            if asset::image_failed(url) {
                return Err(CanvasSceneError::UnresolvedSprite(format!(
                    "image `{url}` failed to load"
                )));
            }
            return Ok(());
        };
        Rect {
            x: 0.0,
            y: 0.0,
            w: w as f32,
            h: h as f32,
        }
    };

    // `spriteRect` (lib/drawable/contract.ts): an omitted width/height is one
    // cell on tile grids and the source's native px on world-pixel `free` boards.
    out.push(DrawCommand::Sprite {
        texture: url.to_string(),
        src,
        dst: sprite_dst(node, position, projector, to_screen, zoom, src),
        rotation: get_f32(node, "rotation", 0.0),
        opacity: get_f32(node, "opacity", 1.0),
        flip_x: node.get("flipX").and_then(Value::as_bool).unwrap_or(false),
        flip_y: false,
    });
    Ok(())
}

fn sprite_dst(
    node: &Value,
    position: ScenePos,
    projector: &Projector,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
    zoom: f32,
    source: Rect,
) -> Rect {
    let tw = projector.tile_width;
    let free = projector.layout == Projection::Free;
    let w = node
        .get("width")
        .and_then(Value::as_f64)
        .map(|value| value as f32 * tw)
        .unwrap_or(if free { source.w } else { tw });
    let h = node
        .get("height")
        .and_then(Value::as_f64)
        .map(|value| value as f32 * tw)
        .unwrap_or(if free { source.h } else { tw });
    let anchor = node
        .get("anchor")
        .and_then(Value::as_str)
        .and_then(Anchor::from_str)
        .unwrap_or(Anchor::TopLeft);
    let (px, py) = projector.anchor_point(position, anchor);
    let x = if anchor == Anchor::TopLeft {
        px
    } else {
        px - w / 2.0
    };
    let y = match anchor {
        Anchor::Ground => py - h,
        Anchor::Center => py - h / 2.0,
        Anchor::TopLeft => py,
    };
    let (x, y) = to_screen(x, y);
    Rect {
        x,
        y,
        w: w * zoom,
        h: h * zoom,
    }
}

fn animated_shape(node: &Value, epoch_now_ms: f64) -> Option<Value> {
    if epoch_now_ms <= 0.0 {
        return None;
    }
    let animation = node.get("animation")?;
    let mut animated = node.clone();
    let object = animated.as_object_mut()?;
    for key in [
        "offsetX",
        "offsetY",
        "rotate",
        "opacity",
        "radiusX",
        "radiusY",
        "width",
        "height",
        "strokeWidth",
        "strokeDashOffset",
        "blur",
        "fill",
        "stroke",
    ] {
        if let Some(value) = crate::animate::drawable_track(animation, epoch_now_ms, key) {
            object.insert(key.to_owned(), value);
        }
    }
    if let Some(color) = crate::animate::drawable_track(animation, epoch_now_ms, "shadow.color") {
        let shadow = object
            .entry("shadow")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(shadow) = shadow.as_object_mut() {
            shadow.insert("color".into(), color);
        }
    }
    if let Some(blur) = crate::animate::drawable_track(animation, epoch_now_ms, "shadow.blur") {
        let shadow = object
            .entry("shadow")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(shadow) = shadow.as_object_mut() {
            shadow.insert("blur".into(), blur);
        }
    }
    Some(animated)
}

fn shape_gradient(
    source: &Value,
    position: ScenePos,
    projector: &Projector,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
    zoom: f32,
    opacity: f32,
) -> Option<Gradient> {
    let stops = source
        .get("stops")?
        .as_array()?
        .iter()
        .map(|stop| {
            let offset = stop.get("offset")?.as_f64()? as f32;
            let mut color = parse_css_color(stop.get("color")?.as_str()?)?;
            color[3] *= opacity;
            Some(GradientStop { offset, color })
        })
        .collect::<Option<Vec<_>>>()?;
    if stops.is_empty() {
        return None;
    }
    let (origin_x, origin_y) = projector.project(position);
    let point = |key: &str| -> Option<Vec2> {
        let value = source.get(key)?;
        let x = value.get("x")?.as_f64()? as f32;
        let y = value.get("y")?.as_f64()? as f32;
        let (x, y) = to_screen(
            origin_x + x * projector.tile_width,
            origin_y + y * projector.tile_width,
        );
        Some(Vec2 { x, y })
    };
    match source.get("kind")?.as_str()? {
        "linear" => Some(Gradient::Linear {
            from: point("from")?,
            to: point("to")?,
            stops,
        }),
        "radial" => Some(Gradient::Radial {
            center: point("center")?,
            radius: source.get("radius")?.as_f64()? as f32 * projector.tile_width * zoom,
            stops,
        }),
        "conic" => Some(Gradient::Conic {
            center: point("center")?,
            angle: source.get("angle").and_then(Value::as_f64).unwrap_or(0.0) as f32,
            stops,
        }),
        _ => None,
    }
}

fn polygon_segments(points: &[Vec2]) -> Vec<PathSegment> {
    let Some(first) = points.first() else {
        return Vec::new();
    };
    let mut segments = vec![PathSegment::MoveTo {
        x: first.x,
        y: first.y,
    }];
    segments.extend(points.iter().skip(1).map(|point| PathSegment::LineTo {
        x: point.x,
        y: point.y,
    }));
    segments.push(PathSegment::Close);
    segments
}

fn shape_fill_pattern(source: &Value) -> Option<FillPattern> {
    match source.get("kind")?.as_str()? {
        "noise" => Some(FillPattern::Noise {
            scale: get_f32(source, "scale", 2.0),
            alpha: get_f32(source, "alpha", 0.12),
            color: source
                .get("color")
                .and_then(Value::as_str)
                .and_then(parse_css_color)
                .unwrap_or([0.0, 0.0, 0.0, 1.0]),
        }),
        "image" => Some(FillPattern::Image {
            url: source.get("url")?.as_str()?.to_string(),
            scale: get_f32(source, "scale", 1.0),
        }),
        _ => None,
    }
}

fn push_styled_path(
    segments: Vec<PathSegment>,
    fill: Option<[f32; 4]>,
    stroke: Option<Stroke>,
    fill_gradient: Option<Gradient>,
    stroke_gradient: Option<Gradient>,
    pattern: Option<FillPattern>,
    opacity: f32,
    separate_paint: bool,
    out: &mut Vec<DrawCommand>,
) {
    if pattern.is_some() || separate_paint {
        if fill.is_some() || fill_gradient.is_some() {
            out.push(DrawCommand::GradientPath {
                segments: segments.clone(),
                fill,
                stroke: None,
                fill_gradient,
                stroke_gradient: None,
            });
        }
        if let Some(pattern) = pattern {
            out.push(DrawCommand::PatternPath {
                segments: segments.clone(),
                pattern,
                opacity,
            });
        }
        if stroke.is_some() || stroke_gradient.is_some() {
            out.push(DrawCommand::GradientPath {
                segments,
                fill: None,
                stroke,
                fill_gradient: None,
                stroke_gradient,
            });
        }
    } else {
        out.push(DrawCommand::GradientPath {
            segments,
            fill,
            stroke,
            fill_gradient,
            stroke_gradient,
        });
    }
}

/// Whether a draw-shape is drawn by the SDF pass: a shape only it draws, or `corners` or a `material`.
fn is_sdf_shape(node: &Value, shape: &str) -> bool {
    matches!(shape, "capsule" | "arc" | "polygon")
        || ["corners", "material", "backdrop"]
            .iter()
            .any(|key| node.get(*key).is_some_and(|v| !v.is_null()))
}

fn number_array(value: &Value, field: &str) -> Result<Vec<f32>, CanvasSceneError> {
    let items = value
        .as_array()
        .ok_or_else(|| invalid(field, "must be an array of numbers"))?;
    items
        .iter()
        .map(|v| {
            v.as_f64()
                .filter(|f| f.is_finite())
                .map(|f| f as f32)
                .ok_or_else(|| invalid(field, "entries must be finite numbers"))
        })
        .collect()
}

/// Project one draw-shape: the SDF pass takes the shapes that need it, every other shape keeps the vector path.
fn paint_shape(
    node: &Value,
    projector: &Projector,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
    zoom: f32,
    epoch_now_ms: f64,
    gathered: &mut Gathered,
    out: &mut Vec<DrawCommand>,
) -> Result<(), CanvasSceneError> {
    let sdf = node
        .get("shape")
        .and_then(Value::as_str)
        .is_some_and(|shape| is_sdf_shape(node, shape));
    if !sdf {
        paint_vector_shape(
            node,
            projector,
            to_screen,
            zoom,
            epoch_now_ms,
            gathered,
            out,
        );
        return Ok(());
    }
    let animated = animated_shape(node, epoch_now_ms);
    paint_sdf_shape(
        animated.as_ref().unwrap_or(node),
        projector,
        to_screen,
        zoom,
        epoch_now_ms,
        gathered,
        out,
    )
}

fn paint_sdf_shape(
    node: &Value,
    projector: &Projector,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
    zoom: f32,
    epoch_now_ms: f64,
    gathered: &mut Gathered,
    out: &mut Vec<DrawCommand>,
) -> Result<(), CanvasSceneError> {
    let shape = node
        .get("shape")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some(position) = node.get("position").and_then(parse_scene_pos) else {
        gap_once("shape-position", "draw-shape without position");
        return Ok(());
    };
    if ["gradient", "strokeGradient", "fillPattern"]
        .iter()
        .any(|key| node.get(*key).is_some_and(|v| !v.is_null()))
        || get_f32(node, "blur", 0.0) > 0.0
        || node
            .get("blendMode")
            .and_then(Value::as_str)
            .is_some_and(|mode| mode != "source-over")
    {
        gap_once(
            "sdf-style",
            "an SDF draw-shape carries no gradient, fillPattern, blur or blendMode",
        );
        return Ok(());
    }
    let kind = match shape {
        "rect" => SdfKind::Rect,
        "capsule" => SdfKind::Capsule,
        "arc" => SdfKind::Arc,
        "polygon" => SdfKind::Polygon,
        "ellipse" => SdfKind::Ellipse,
        other => {
            return Err(invalid(
                "shape",
                format!("`{other}` has no SDF form (rect, capsule, arc, polygon, ellipse)"),
            ))
        }
    };
    let tw = projector.tile_width;
    let opacity = get_f32(node, "opacity", 1.0);
    let mut rect = if kind == SdfKind::Ellipse {
        let anchor = node
            .get("anchor")
            .and_then(Value::as_str)
            .and_then(Anchor::from_str)
            .unwrap_or(Anchor::Ground);
        let (px, py) = projector.anchor_point(position, anchor);
        let cx = px + get_f32(node, "offsetX", 0.0) * tw;
        let cy = py + get_f32(node, "offsetY", 0.0) * tw;
        let rx = get_f32(node, "radiusX", 0.0) * tw;
        let ry = node
            .get("radiusY")
            .and_then(Value::as_f64)
            .map(|v| v as f32 * tw)
            .unwrap_or(rx);
        let (cx, cy) = to_screen(cx, cy);
        Rect {
            x: cx - rx * zoom,
            y: cy - ry * zoom,
            w: 2.0 * rx * zoom,
            h: 2.0 * ry * zoom,
        }
    } else {
        let anchor = node
            .get("anchor")
            .and_then(Value::as_str)
            .and_then(Anchor::from_str)
            .unwrap_or(Anchor::TopLeft);
        let (px, py) = projector.anchor_point(position, anchor);
        let x = px + get_f32(node, "offsetX", 0.0) * tw;
        let y = py + get_f32(node, "offsetY", 0.0) * tw;
        let (sx, sy) = to_screen(x, y);
        Rect {
            x: sx,
            y: sy,
            w: get_f32(node, "width", 0.0) * tw * zoom,
            h: get_f32(node, "height", 0.0) * tw * zoom,
        }
    };

    let (corners, corner_kind) = match node.get("corners").filter(|c| !c.is_null()) {
        None => ([0.0; 4], 0),
        Some(spec) => {
            if kind != SdfKind::Rect {
                return Err(invalid("corners", "corners apply to rect shapes only"));
            }
            let radius = number_array(
                spec.get("radius")
                    .ok_or_else(|| invalid("corners.radius", "needs four radii"))?,
                "corners.radius",
            )?;
            if radius.len() != 4 || radius.iter().any(|r| *r < 0.0) {
                return Err(invalid(
                    "corners.radius",
                    "needs four radii of zero or more (top-left, top-right, bottom-right, bottom-left)",
                ));
            }
            let corner_kind = match spec.get("kind").and_then(Value::as_str).unwrap_or("round") {
                "round" => 0,
                "bevel" => 1,
                "notch" => 2,
                "scoop" => 3,
                other => {
                    return Err(invalid(
                        "corners.kind",
                        format!("`{other}` is not round, bevel, notch or scoop"),
                    ))
                }
            };
            (
                [
                    radius[0] * zoom,
                    radius[1] * zoom,
                    radius[2] * zoom,
                    radius[3] * zoom,
                ],
                corner_kind,
            )
        }
    };
    let arc = if kind == SdfKind::Arc {
        let thickness = node
            .get("thickness")
            .and_then(Value::as_f64)
            .filter(|t| t.is_finite() && *t > 0.0)
            .ok_or_else(|| invalid("thickness", "an arc needs a thickness in px above zero"))?;
        [
            get_f32(node, "startAngle", 0.0),
            get_f32(node, "sweep", std::f32::consts::TAU),
            thickness as f32 * zoom,
        ]
    } else {
        [0.0; 3]
    };
    let polygon = if kind == SdfKind::Polygon {
        let sides = node
            .get("sides")
            .and_then(Value::as_f64)
            .filter(|s| s.fract() == 0.0 && (3.0..=64.0).contains(s))
            .ok_or_else(|| invalid("sides", "a polygon needs a whole number of 3 to 64 sides"))?;
        let ratio = get_f32(node, "starRatio", 1.0);
        if !(ratio > 0.0 && ratio <= 1.0) {
            return Err(invalid("starRatio", "must be above 0 and at most 1"));
        }
        [sides as f32, ratio]
    } else {
        [0.0; 2]
    };

    let material = match node.get("material").filter(|m| !m.is_null()) {
        None => None,
        Some(spec) => {
            let wgsl = spec
                .get("wgsl")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("material.wgsl", "needs the program's WGSL text"))?;
            let params = match spec.get("params").filter(|p| !p.is_null()) {
                None => Vec::new(),
                Some(params) => number_array(params, "material.params")?,
            };
            if params.len() > 8 {
                return Err(invalid("material.params", "at most 8 parameters"));
            }
            crate::sdf::check_material(wgsl).map_err(|detail| invalid("material.wgsl", detail))?;
            let since = match spec.get("anchor").filter(|a| !a.is_null()) {
                None => 0.0,
                Some(anchor) => {
                    let anchor = anchor
                        .as_f64()
                        .filter(|a| a.is_finite())
                        .ok_or_else(|| invalid("material.anchor", "must be a finite number of epoch milliseconds"))?;
                    if epoch_now_ms > 0.0 {
                        ((epoch_now_ms - anchor) / 1000.0) as f32
                    } else {
                        0.0
                    }
                }
            };
            Some(SdfMaterial {
                wgsl: wgsl.to_owned(),
                params,
                live: spec.get("live").and_then(Value::as_bool).unwrap_or(false),
                since,
                targets: super::layer_props::material_targets(spec)?,
            })
        }
    };

    let fill = node
        .get("fill")
        .and_then(Value::as_str)
        .and_then(parse_css_color);
    let stroke = node
        .get("stroke")
        .and_then(Value::as_str)
        .and_then(parse_css_color)
        .map(|color| {
            let dash = node
                .get("strokeDash")
                .and_then(Value::as_array)
                .map(|dash| dash.iter().filter_map(Value::as_f64).collect::<Vec<_>>())
                .unwrap_or_default();
            let dash = match dash.as_slice() {
                [] => [0.0, 0.0],
                [one] => [*one as f32 * zoom, *one as f32 * zoom],
                [on, off, ..] => [*on as f32 * zoom, *off as f32 * zoom],
            };
            SdfStroke {
                color,
                width: get_f32(node, "strokeWidth", 1.0) * zoom,
                dash,
                dash_offset: get_f32(node, "strokeDashOffset", 0.0) * zoom,
            }
        });
    let shadow = match node.get("shadow").filter(|s| !s.is_null()) {
        None => None,
        Some(spec) => {
            let color = spec
                .get("color")
                .and_then(Value::as_str)
                .and_then(parse_css_color);
            match (color, spec.get("blur").and_then(Value::as_f64)) {
                (Some(color), Some(blur)) if blur.is_finite() && blur >= 0.0 => {
                    (blur > 0.0 && color[3] > 0.0).then(|| SdfShadow {
                        color,
                        sigma: blur as f32 * tw * zoom / 2.0,
                        offset: Vec2::default(),
                    })
                }
                _ => {
                    return Err(invalid(
                        "shadow",
                        "needs a css `color` and a finite `blur` of zero or more",
                    ))
                }
            }
        }
    };
    if fill.is_none() && stroke.is_none() && material.is_none() && !super::layer_props::has_backdrop(node) {
        return Ok(());
    }

    let rotation = get_f32(node, "rotate", 0.0);
    if rotation != 0.0 {
        let pivot = node.get("pivot");
        let (origin_x, origin_y) = projector.project(position);
        let (pivot_x, pivot_y) = to_screen(
            origin_x
                + pivot
                    .and_then(|p| p.get("x"))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.5) as f32
                    * tw,
            origin_y
                + pivot
                    .and_then(|p| p.get("y"))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.5) as f32
                    * tw,
        );
        let (dx, dy) = (
            rect.x + rect.w / 2.0 - pivot_x,
            rect.y + rect.h / 2.0 - pivot_y,
        );
        let (cs, sn) = (rotation.cos(), rotation.sin());
        let (cx, cy) = (pivot_x + dx * cs - dy * sn, pivot_y + dx * sn + dy * cs);
        rect.x = cx - rect.w / 2.0;
        rect.y = cy - rect.h / 2.0;
    }

    if let Some(id) = hit_id(node) {
        gathered.regions.push(Region {
            id: id.to_string(),
            rect,
            rotation,
        });
    }
    out.push(DrawCommand::Sdf {
        kind,
        rect,
        corners,
        corner_kind,
        arc,
        polygon,
        fill: fill.unwrap_or([0.0; 4]),
        stroke,
        shadow,
        rotation,
        opacity,
        material,
    });
    super::layer_props::wrap_backdrop(node, zoom, out)
}

/// Project one world shape through the shared Canvas vector commands.
fn paint_vector_shape(
    node: &Value,
    projector: &Projector,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
    zoom: f32,
    epoch_now_ms: f64,
    gathered: &mut Gathered,
    out: &mut Vec<DrawCommand>,
) {
    let animated = animated_shape(node, epoch_now_ms);
    let node = animated.as_ref().unwrap_or(node);
    let Some(shape) = node.get("shape").and_then(Value::as_str) else {
        gap_once("shape-kind", "draw-shape without shape");
        return;
    };
    let Some(position) = node.get("position").and_then(parse_scene_pos) else {
        gap_once("shape-position", "draw-shape without position");
        return;
    };
    let blend_mode = node.get("blendMode").and_then(Value::as_str);
    if blend_mode.is_some_and(|mode| !matches!(mode, "source-over" | "lighter")) {
        gap_once(
            "shape-blend",
            "draw-shape blendMode needs scoped GPU compositing",
        );
        return;
    }
    let blur = get_f32(node, "blur", 0.0).max(0.0);
    let additive = blend_mode == Some("lighter");
    let shadow = node.get("shadow").filter(|shadow| !shadow.is_null());
    let shadow = match shadow {
        None => None,
        Some(shadow) => {
            let color = shadow
                .get("color")
                .and_then(Value::as_str)
                .and_then(parse_css_color);
            let blur = shadow.get("blur").and_then(Value::as_f64);
            match (color, blur) {
                (Some(color), Some(blur)) if blur.is_finite() && blur >= 0.0 => {
                    (blur > 0.0 && color[3] > 0.0).then_some((color, blur as f32))
                }
                _ => {
                    gap_once(
                        "shape-shadow",
                        "draw-shape shadow needs a css `color` and a finite `blur` of zero or more",
                    );
                    return;
                }
            }
        }
    };
    let opacity = get_f32(node, "opacity", 1.0);
    let fill_gradient = node
        .get("gradient")
        .and_then(|source| shape_gradient(source, position, projector, to_screen, zoom, opacity));
    let stroke_gradient = node
        .get("strokeGradient")
        .and_then(|source| shape_gradient(source, position, projector, to_screen, zoom, opacity));
    let pattern = node.get("fillPattern").and_then(shape_fill_pattern);
    let uses_vector_style = fill_gradient.is_some()
        || stroke_gradient.is_some()
        || pattern.is_some()
        || blur > 0.0
        || additive
        || shadow.is_some();
    let fill = node
        .get("fill")
        .and_then(Value::as_str)
        .and_then(parse_css_color)
        .map(|mut color| {
            color[3] *= opacity;
            color
        });
    let stroke = node
        .get("stroke")
        .and_then(Value::as_str)
        .and_then(parse_css_color)
        .or_else(|| stroke_gradient.as_ref().map(|_| [1.0, 1.0, 1.0, 1.0]))
        .map(|mut color| {
            color[3] *= opacity;
            Stroke {
                color,
                width: get_f32(node, "strokeWidth", 1.0) * zoom,
                dash: node
                    .get("strokeDash")
                    .and_then(Value::as_array)
                    .map(|dash| {
                        dash.iter()
                            .filter_map(Value::as_f64)
                            .map(|length| length as f32 * zoom)
                            .collect()
                    })
                    .unwrap_or_default(),
                dash_offset: get_f32(node, "strokeDashOffset", 0.0) * zoom,
                cap: LineCap::Butt,
                join: LineJoin::Miter,
            }
        });
    if fill.is_none()
        && stroke.is_none()
        && fill_gradient.is_none()
        && pattern.is_none()
        && hit_id(node).is_none()
    {
        return;
    }

    let start = out.len();
    // A hit area may paint nothing (no fill, no stroke); its region is its geometry.
    let mut geometry_bounds: Option<Rect> = None;
    let tw = projector.tile_width;
    match shape {
        "cell" => {
            let pts = projector.cell_path(position);
            let points: Vec<Vec2> = pts
                .iter()
                .map(|&(x, y)| {
                    let (sx, sy) = to_screen(x, y);
                    Vec2 { x: sx, y: sy }
                })
                .collect();
            if uses_vector_style {
                push_styled_path(
                    polygon_segments(&points),
                    fill,
                    stroke,
                    fill_gradient,
                    stroke_gradient,
                    pattern,
                    opacity,
                    blur > 0.0,
                    out,
                );
            } else {
                if let Some(color) = fill {
                    out.push(DrawCommand::Poly {
                        points: points.clone(),
                        color,
                    });
                }
                push_shape_stroke(&points, stroke, out);
            }
        }
        "rect" => {
            let anchor = node
                .get("anchor")
                .and_then(Value::as_str)
                .and_then(Anchor::from_str)
                .unwrap_or(Anchor::TopLeft);
            let (px, py) = projector.anchor_point(position, anchor);
            let x = px + get_f32(node, "offsetX", 0.0) * tw;
            let y = py + get_f32(node, "offsetY", 0.0) * tw;
            let w = get_f32(node, "width", 0.0) * tw;
            let h = get_f32(node, "height", 0.0) * tw;
            let (sx, sy) = to_screen(x, y);
            let rect = Rect {
                x: sx,
                y: sy,
                w: w * zoom,
                h: h * zoom,
            };
            geometry_bounds = Some(rect);
            let points = [
                Vec2 {
                    x: rect.x,
                    y: rect.y,
                },
                Vec2 {
                    x: rect.x + rect.w,
                    y: rect.y,
                },
                Vec2 {
                    x: rect.x + rect.w,
                    y: rect.y + rect.h,
                },
                Vec2 {
                    x: rect.x,
                    y: rect.y + rect.h,
                },
            ];
            if uses_vector_style {
                push_styled_path(
                    polygon_segments(&points),
                    fill,
                    stroke,
                    fill_gradient,
                    stroke_gradient,
                    pattern,
                    opacity,
                    blur > 0.0,
                    out,
                );
            } else {
                if let Some(color) = fill {
                    out.push(DrawCommand::Quad { rect, color });
                }
                push_shape_stroke(&points, stroke, out);
            }
        }
        "ellipse" => {
            let anchor = node
                .get("anchor")
                .and_then(Value::as_str)
                .and_then(Anchor::from_str)
                .unwrap_or(Anchor::Ground);
            let (px, py) = projector.anchor_point(position, anchor);
            let cx = px + get_f32(node, "offsetX", 0.0) * tw;
            let cy = py + get_f32(node, "offsetY", 0.0) * tw;
            let rx = get_f32(node, "radiusX", 0.0) * tw;
            // Parity with DrawShape.tsx: radiusY defaults to radiusX (a circle) in
            // WORLD units — the old post-scale fallback double-multiplied by tw.
            let ry = node
                .get("radiusY")
                .and_then(Value::as_f64)
                .map(|v| v as f32 * tw)
                .unwrap_or(rx);
            let mut points = Vec::with_capacity(ELLIPSE_SEGMENTS);
            for i in 0..ELLIPSE_SEGMENTS {
                let a = (i as f32 / ELLIPSE_SEGMENTS as f32) * std::f32::consts::TAU;
                let (sx, sy) = to_screen(cx + rx * a.cos(), cy + ry * a.sin());
                points.push(Vec2 { x: sx, y: sy });
            }
            let (cx, cy) = to_screen(cx, cy);
            let arc = vec![
                PathSegment::Arc {
                    cx,
                    cy,
                    rx: rx * zoom,
                    ry: ry * zoom,
                    start: 0.0,
                    end: std::f32::consts::TAU,
                },
                PathSegment::Close,
            ];
            if uses_vector_style {
                push_styled_path(
                    arc,
                    fill,
                    stroke,
                    fill_gradient,
                    stroke_gradient,
                    pattern,
                    opacity,
                    blur > 0.0,
                    out,
                );
            } else {
                if let Some(color) = fill {
                    out.push(DrawCommand::Poly { points, color });
                }
                if let Some(stroke) = stroke {
                    out.push(DrawCommand::Path {
                        segments: arc,
                        fill: None,
                        stroke: Some(stroke),
                    });
                }
            }
        }
        "poly" => {
            let (bx, by) = projector.project(position);
            let offset_x = get_f32(node, "offsetX", 0.0);
            let offset_y = get_f32(node, "offsetY", 0.0);
            let points: Vec<Vec2> = node
                .get("points")
                .and_then(Value::as_array)
                .map(|pts| {
                    pts.iter()
                        .filter_map(|pt| {
                            let x = pt.get("x").and_then(Value::as_f64)? as f32;
                            let y = pt.get("y").and_then(Value::as_f64)? as f32;
                            let (sx, sy) =
                                to_screen(bx + (offset_x + x) * tw, by + (offset_y + y) * tw);
                            Some(Vec2 { x: sx, y: sy })
                        })
                        .collect()
                })
                .unwrap_or_default();
            if uses_vector_style {
                push_styled_path(
                    polygon_segments(&points),
                    fill,
                    stroke,
                    fill_gradient,
                    stroke_gradient,
                    pattern,
                    opacity,
                    blur > 0.0,
                    out,
                );
            } else {
                if let Some(color) = fill {
                    out.push(DrawCommand::Poly {
                        points: points.clone(),
                        color,
                    });
                }
                push_shape_stroke(&points, stroke, out);
            }
        }
        "path" => {
            let Some(d) = node.get("d").and_then(Value::as_str) else {
                gap_once("shape-path", "draw-shape path without d");
                return;
            };
            let (bx, by) = projector.project(position);
            let x = bx + get_f32(node, "offsetX", 0.0) * tw;
            let y = by + get_f32(node, "offsetY", 0.0) * tw;
            let (ox, oy) = to_screen(x, y);
            let Some(segments) = parse_path_data(
                d,
                Xf {
                    s: tw * zoom,
                    ox,
                    oy,
                },
            ) else {
                gap_once("shape-path-data", "malformed draw-shape SVG path");
                return;
            };
            if uses_vector_style {
                push_styled_path(
                    segments,
                    fill,
                    stroke,
                    fill_gradient,
                    stroke_gradient,
                    pattern,
                    opacity,
                    blur > 0.0,
                    out,
                );
            } else {
                out.push(DrawCommand::Path {
                    segments,
                    fill,
                    stroke,
                });
            }
        }
        _ => gap_once("shape-kind-unknown", "unknown draw-shape kind"),
    }
    if blur > 0.0 {
        let filtered: Vec<_> = out
            .drain(start..)
            .map(|command| DrawCommand::Blurred {
                command: Box::new(command),
                radius: blur,
            })
            .collect();
        out.extend(filtered);
    }
    let radians = get_f32(node, "rotate", 0.0);
    if radians != 0.0 {
        let pivot = node.get("pivot");
        let (origin_x, origin_y) = projector.project(position);
        let (x, y) = to_screen(
            origin_x
                + pivot
                    .and_then(|p| p.get("x"))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.5) as f32
                    * tw,
            origin_y
                + pivot
                    .and_then(|p| p.get("y"))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.5) as f32
                    * tw,
        );
        crate::animate::transform_group_commands(
            &mut out[start..],
            Vec2 { x, y },
            Vec2 { x: 0.0, y: 0.0 },
            1.0,
            radians,
            1.0,
        );
    }
    if let Some((color, blur)) = shadow.filter(|_| out.len() > start) {
        // canvas shadowBlur is twice the Gaussian sigma; `blur` is in world units
        let commands = out[start..].to_vec();
        let sigma = blur * projector.tile_width * zoom / 2.0;
        out.insert(
            start,
            DrawCommand::Shadow {
                commands,
                color,
                sigma,
            },
        );
    }
    if additive && out.len() > start {
        let commands = out.drain(start..).collect();
        out.push(DrawCommand::AdditiveGroup { commands });
    }
    if let Some(id) = hit_id(node) {
        if let Some(rect) = commands_bounds(&out[start..]).or(geometry_bounds) {
            gathered.regions.push(Region {
                id: id.to_string(),
                rect,
                rotation: 0.0,
            });
        }
    }
}

fn push_shape_stroke(points: &[Vec2], stroke: Option<Stroke>, out: &mut Vec<DrawCommand>) {
    let Some(stroke) = stroke else { return };
    let Some(first) = points.first() else { return };
    let mut segments = vec![PathSegment::MoveTo {
        x: first.x,
        y: first.y,
    }];
    segments.extend(
        points
            .iter()
            .skip(1)
            .map(|p| PathSegment::LineTo { x: p.x, y: p.y }),
    );
    segments.push(PathSegment::Close);
    out.push(DrawCommand::Path {
        segments,
        fill: None,
        stroke: Some(stroke),
    });
}

/// Background image: atlas slices cover the viewport; whole images tile at
/// native pixel size with the camera phase, as in the existing web painter.
pub(crate) fn background_sprites(
    bg: &Value,
    width: f32,
    height: f32,
    cam: (f32, f32),
) -> Vec<DrawCommand> {
    let url = match bg {
        Value::String(s) => s.as_str(),
        Value::Object(_) => match bg.get("url").and_then(Value::as_str) {
            Some(url) => url,
            None => return Vec::new(),
        },
        _ => return Vec::new(),
    };
    let atlas_slice = if let (Some(atlas_url), Some(sprite)) = (
        bg.get("atlas").and_then(Value::as_str),
        bg.get("sprite").and_then(Value::as_str),
    ) {
        asset::ensure_asset(atlas_url);
        atlas::resolve_atlas_rect(atlas_url, sprite)
    } else {
        None
    };
    if let Some(src) = atlas_slice {
        if src.w <= 0.0 || src.h <= 0.0 {
            return Vec::new();
        }
        let scale = (width / src.w).max(height / src.h);
        let w = src.w * scale;
        let h = src.h * scale;
        return vec![DrawCommand::Sprite {
            texture: url.to_string(),
            src,
            dst: Rect {
                x: (width - w) / 2.0,
                y: (height - h) / 2.0,
                w,
                h,
            },
            rotation: 0.0,
            opacity: 1.0,
            flip_x: false,
            flip_y: false,
        }];
    }
    if bg.get("atlas").is_some() && bg.get("sprite").is_some() {
        return Vec::new();
    }
    let Some((image_w, image_h)) = asset::image_info(url) else {
        return Vec::new();
    };
    let (image_w, image_h) = (image_w as f32, image_h as f32);
    if image_w <= 0.0 || image_h <= 0.0 {
        return Vec::new();
    }
    let src = Rect {
        x: 0.0,
        y: 0.0,
        w: image_w,
        h: image_h,
    };
    let mut sprites = Vec::new();
    let start_x = -cam.0.rem_euclid(image_w);
    let start_y = -cam.1.rem_euclid(image_h);
    let mut y = start_y - image_h;
    while y < height {
        let mut x = start_x - image_w;
        while x < width {
            sprites.push(DrawCommand::Sprite {
                texture: url.to_string(),
                src,
                dst: Rect {
                    x,
                    y,
                    w: image_w,
                    h: image_h,
                },
                rotation: 0.0,
                opacity: 1.0,
                flip_x: false,
                flip_y: false,
            });
            x += image_w;
        }
        y += image_h;
    }
    sprites
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

/// Deterministic CSS color subset: `#rgb`, `#rrggbb`, `#rrggbbaa`,
/// `rgb(r,g,b)`, `rgba(r,g,b,a)` (a in 0..1). Anything else → None (the caller
/// skips the fill — no heuristics).
pub(crate) fn parse_css_color(s: &str) -> Option<[f32; 4]> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        let v = u32::from_str_radix(hex, 16).ok()?;
        return match hex.len() {
            3 => Some([
                ((v >> 8) & 0xf) as f32 * 17.0 / 255.0,
                ((v >> 4) & 0xf) as f32 * 17.0 / 255.0,
                (v & 0xf) as f32 * 17.0 / 255.0,
                1.0,
            ]),
            6 => Some([
                ((v >> 16) & 0xff) as f32 / 255.0,
                ((v >> 8) & 0xff) as f32 / 255.0,
                (v & 0xff) as f32 / 255.0,
                1.0,
            ]),
            8 => Some([
                ((v >> 24) & 0xff) as f32 / 255.0,
                ((v >> 16) & 0xff) as f32 / 255.0,
                ((v >> 8) & 0xff) as f32 / 255.0,
                (v & 0xff) as f32 / 255.0,
            ]),
            _ => None,
        };
    }
    let rgb = s
        .strip_prefix("rgba(")
        .or_else(|| s.strip_prefix("rgb("))
        .and_then(|r| r.strip_suffix(')'))
        .and_then(|inner| {
            inner
                .split(',')
                .map(|p| p.trim().parse::<f32>())
                .collect::<Result<Vec<f32>, _>>()
                .ok()
        });
    match rgb.as_deref() {
        Some([r, g, b]) => Some([r / 255.0, g / 255.0, b / 255.0, 1.0]),
        Some([r, g, b, a]) => Some([r / 255.0, g / 255.0, b / 255.0, *a]),
        // every other CSS color the browser paints: named colors, `transparent`, hsl(), hwb(), space-separated rgb()
        _ => csscolorparser::parse(s).ok().map(|c| [c.r, c.g, c.b, c.a]),
    }
}

pub(crate) fn get_f32(props: &Value, key: &str, default: f32) -> f32 {
    props
        .get(key)
        .and_then(Value::as_f64)
        .map(|f| f as f32)
        .unwrap_or(default)
}

pub(crate) fn get_string(props: &Value, key: &str) -> Option<String> {
    props.get(key).and_then(Value::as_str).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::TextFamily;
    use serde_json::json;

    fn packet_for(props: Value) -> GameRenderPacket {
        from_props(&props).unwrap()
    }

    #[test]
    fn parses_css_colors() {
        assert_eq!(parse_css_color("#1a1a2e").unwrap()[0], 26.0 / 255.0);
        assert_eq!(parse_css_color("#fff").unwrap(), [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(
            parse_css_color("rgba(0,150,255,0.6)").unwrap(),
            [0.0, 150.0 / 255.0, 1.0, 0.6]
        );
        assert_eq!(
            parse_css_color("rgb(255, 0, 0)").unwrap(),
            [1.0, 0.0, 0.0, 1.0]
        );
        assert_eq!(parse_css_color("#ff000080").unwrap()[3], 128.0 / 255.0);
        let purple = parse_css_color("rebeccapurple").unwrap();
        assert_eq!(
            purple,
            [
                0x66 as f32 / 255.0,
                0x33 as f32 / 255.0,
                0x99 as f32 / 255.0,
                1.0
            ],
            "named colors, as browsers paint them"
        );
        assert_eq!(parse_css_color("transparent").unwrap()[3], 0.0);
        assert!(
            parse_css_color("var(--tone)").is_none(),
            "control: a theme variable is the web host's to resolve"
        );
        assert!(parse_css_color("not a color").is_none());
    }

    #[test]
    fn shape_gradients_use_shared_vector_commands_and_world_coordinates() {
        let gradient = json!({
            "kind": "linear", "from": {"x": 0, "y": 0}, "to": {"x": 256, "y": 0},
            "stops": [{"offset": 0, "color": "#ff0000"}, {"offset": 1, "color": "#0000ff"}]
        });
        let p = packet_for(json!({
            "projection": "free", "scale": 1.0,
            "drawables": [
                {"type": "draw-shape", "shape": "rect", "position": {"x": 1, "y": 1}, "width": 256, "height": 256, "gradient": gradient},
                {"type": "draw-shape", "shape": "ellipse", "position": {"x": 2, "y": 1}, "radiusX": 128, "radiusY": 128, "gradient": gradient},
                {"type": "draw-shape", "shape": "poly", "position": {"x": 3, "y": 1}, "points": [{"x": 0, "y": 0}, {"x": 256, "y": 0}, {"x": 0, "y": 256}], "gradient": gradient},
                {"type": "draw-shape", "shape": "path", "position": {"x": 4, "y": 1}, "d": "M0 0 L256 0 L256 256 Z", "gradient": gradient}
            ]
        }));
        let gradients: Vec<_> = p
            .draw_commands
            .iter()
            .filter_map(|cmd| match cmd {
                DrawCommand::GradientPath {
                    fill_gradient: Some(Gradient::Linear { from, to, .. }),
                    ..
                } => Some((*from, *to)),
                _ => None,
            })
            .collect();
        assert_eq!(gradients.len(), 4);
        assert!((gradients[0].1.x - gradients[0].0.x - TILE_WIDTH).abs() < 0.01);
    }

    #[test]
    fn shape_fill_pattern_paints_between_fill_and_stroke() {
        let p = packet_for(json!({
            "projection": "free", "scale": 1.0,
            "drawables": [{"type": "draw-shape", "shape": "rect",
                "position": {"x": 0, "y": 0}, "width": 1, "height": 1,
                "fill": "#ffffff", "stroke": "#ff0000",
                "fillPattern": {"kind": "noise", "alpha": 0.5, "color": "#000000"}}]
        }));
        let styles: Vec<_> = p
            .draw_commands
            .iter()
            .filter_map(|command| match command {
                DrawCommand::GradientPath { fill, .. } if fill.is_some() => Some("fill"),
                DrawCommand::PatternPath { .. } => Some("pattern"),
                DrawCommand::GradientPath { fill, stroke, .. }
                    if fill.is_none() && stroke.is_some() =>
                {
                    Some("stroke")
                }
                _ => None,
            })
            .collect();
        assert_eq!(styles, ["fill", "pattern", "stroke"]);
    }

    #[test]
    fn draw_sprite_sheet_uses_host_clock_and_authored_clock_override() {
        let data_url = |mime: &str, data: &str| {
            format!(
                "data:{mime},{}",
                data.bytes()
                    .map(|byte| format!("%{byte:02X}"))
                    .collect::<String>()
            )
        };
        let sheet = data_url(
            "image/svg+xml",
            "<svg xmlns='http://www.w3.org/2000/svg' width='32' height='36'/>",
        );
        let manifest = data_url(
            "application/json",
            r#"{
            "frameWidth": 8, "frameHeight": 12, "columns": 4,
            "animations": {"walk": {"row": 2, "frames": 3, "frameRate": 10, "loop": true}}
        }"#,
        );
        let mut scene = json!({
            "projection": "free", "scale": 1.0,
            "drawables": [{"type": "draw-sprite", "position": {"x": 0, "y": 0},
                "asset": {"url": sheet, "atlas": manifest}, "animation": "walk"}]
        });
        assert!(has_active_fx(&scene));
        let frame = |scene: &Value, time| {
            from_props_at(scene, time)
                .unwrap()
                .draw_commands
                .into_iter()
                .find_map(|command| match command {
                    DrawCommand::Sprite { src, .. } => Some(src),
                    _ => None,
                })
                .expect("sprite frame")
        };
        assert_eq!(frame(&scene, 150.0).x, 8.0);
        scene["drawables"][0]["clockMs"] = json!(350.0);
        scene["drawables"][0]["loop"] = json!(false);
        let stopped = frame(&scene, 150.0);
        assert_eq!(
            (stopped.x, stopped.y, stopped.w, stopped.h),
            (16.0, 24.0, 8.0, 12.0)
        );
        scene["drawables"][0]["loop"] = json!(true);
        assert_eq!(frame(&scene, 150.0).x, 0.0);
    }

    fn shadowed_disc(blur: f64) -> GameRenderPacket {
        packet_for(json!({
            "projection": "free", "scale": 1.0,
            "drawables": [
                {"type": "draw-shape", "shape": "ellipse", "position": {"x": 1, "y": 1},
                    "anchor": "center", "radiusX": 128, "fill": "#ffffff",
                    "shadow": {"color": "#00ff00", "blur": blur}}
            ]
        }))
    }

    #[test]
    fn a_shape_shadow_draws_its_coverage_blurred_under_the_shape() {
        let packet = shadowed_disc(64.0);
        let [DrawCommand::Clear { .. }, DrawCommand::Shadow {
            commands,
            color,
            sigma,
        }, shape] = packet.draw_commands.as_slice()
        else {
            panic!("expected clear, shadow, shape: {:?}", packet.draw_commands);
        };
        assert_eq!(*color, [0.0, 1.0, 0.0, 1.0]);
        // canvas shadowBlur is twice the Gaussian sigma; blur is in world units
        assert_eq!(*sigma, 0.25 * crate::projector::TILE_WIDTH / 2.0);
        assert_eq!(format!("{commands:?}"), format!("{:?}", [shape]));
    }

    #[test]
    fn control_a_shadow_without_blur_draws_only_the_shape() {
        let packet = shadowed_disc(0.0);
        assert_eq!(packet.draw_commands.len(), 2);
        assert!(!matches!(
            packet.draw_commands[1],
            DrawCommand::Shadow { .. }
        ));
    }

    #[test]
    fn blur_is_scoped_and_unsupported_sprite_shadow_and_blend_do_not_paint_base() {
        let scene = json!({
            "projection": "free", "scale": 1.0,
            "drawables": [
                {"type": "draw-shape", "shape": "rect", "position": {"x": 0, "y": 0},
                    "width": 1, "height": 1, "fill": "#ffffff", "blur": 4},
                {"type": "draw-shape", "shape": "rect", "position": {"x": 1, "y": 0},
                    "width": 1, "height": 1, "fill": "#ffffff", "blendMode": "multiply"},
                {"type": "draw-sprite", "position": {"x": 3, "y": 0},
                    "asset": {"url": "data:image/svg+xml,%3Csvg%20xmlns%3D%27http%3A%2F%2Fwww.w3.org%2F2000%2Fsvg%27%20width%3D%271%27%20height%3D%271%27%2F%3E"},
                    "shadow": {"color": "#000000", "blur": 1}}
            ]
        });
        let packet = packet_for(scene);
        assert_eq!(packet.draw_commands.len(), 2);
        assert!(matches!(packet.draw_commands[0], DrawCommand::Clear { .. }));
        assert!(matches!(
            packet.draw_commands[1],
            DrawCommand::Blurred { .. }
        ));
    }

    #[test]
    fn text_atom_and_layer_use_the_shared_vector_font_pipeline() {
        let p = packet_for(json!({
            "projection": "free",
            "scale": 1.0,
            "drawables": [
                { "type": "draw-text", "text": "one", "position": {"x": 1, "y": 2},
                  "color": "#ffffff", "font": "bold 14px monospace", "align": "center",
                  "baseline": "middle", "opacity": 0.5 },
                { "type": "draw-text-layer", "items": [
                    { "type": "draw-text", "text": "two", "position": {"x": 2, "y": 3},
                      "color": "#000000" }
                ] }
            ]
        }));
        let texts: Vec<_> = p
            .draw_commands
            .iter()
            .filter_map(|cmd| match cmd {
                DrawCommand::Text {
                    text,
                    size,
                    color,
                    align,
                    weight,
                    family,
                    ..
                } => Some((text.as_str(), *size, color[3], *align, *weight, *family)),
                _ => None,
            })
            .collect();
        assert_eq!(texts.len(), 2);
        assert_eq!(
            texts[0],
            ("one", 14.0, 0.5, TextAlign::Center, 700, TextFamily::Mono)
        );
        assert_eq!(
            texts[1],
            ("two", 10.0, 1.0, TextAlign::Left, 400, TextFamily::Sans)
        );
    }

    #[test]
    fn a_serif_font_yields_a_serif_text_command_and_no_font_the_default_family() {
        let family_of = |font: Option<&str>| {
            let mut atom = json!({ "type": "draw-text", "text": "x", "position": {"x": 1, "y": 2},
                "color": "#ffffff" });
            if let Some(f) = font {
                atom["font"] = json!(f);
            }
            let p = packet_for(json!({
                "projection": "free", "scale": 1.0, "drawables": [atom]
            }));
            p.draw_commands
                .iter()
                .find_map(|c| match c {
                    DrawCommand::Text { family, .. } => Some(*family),
                    _ => None,
                })
                .expect("a text command")
        };
        assert_eq!(family_of(Some("400 16px serif")), TextFamily::Serif);
        assert_eq!(family_of(Some("400 16px sans-serif")), TextFamily::Sans);
        assert_eq!(family_of(None), TextFamily::Sans);
        assert_eq!(
            parse_text_font(Some("700 20px serif")),
            Some((20.0, 700, TextFamily::Serif))
        );
    }

    fn editable(extra: Value) -> Value {
        let mut text = json!({ "type": "draw-text", "id": "t", "text": "ab", "position": {"x": 0, "y": 0}, "anchor": "top-left",
            "color": "#ffffff", "font": "20px sans-serif", "align": "left", "baseline": "middle" });
        for (k, v) in extra.as_object().unwrap() {
            text[k] = v.clone();
        }
        json!({ "projection": "free", "scale": 1.0, "drawables": [text] })
    }

    fn quads(p: &GameRenderPacket) -> Vec<Rect> {
        p.draw_commands.iter().filter_map(|c| match c { DrawCommand::Quad { rect, .. } => Some(*rect), _ => None }).collect()
    }

    #[test]
    fn a_caret_is_a_bar_at_the_shaped_width_of_the_text_before_it() {
        let p = packet_for(editable(json!({ "caret": 1 })));
        let q = quads(&p);
        assert_eq!(q.len(), 1, "one caret bar: {:?}", p.draw_commands);
        let before = crate::vector::shape("a", 20.0, false, TextFamily::Sans).width;
        assert!((q[0].x + q[0].w / 2.0 - before).abs() < 0.01, "the bar stands after 'a': {:?} vs {before}", q[0]);
        assert!(quads(&packet_for(editable(json!({ "caret": -1 })))).is_empty(), "control: no caret, no bar");
        assert!(quads(&packet_for(editable(json!({})))).is_empty(), "control: no decorations at all");
    }

    #[test]
    fn a_selection_band_is_painted_behind_its_characters() {
        let p = packet_for(editable(json!({ "selectionStart": 0, "selectionEnd": 2, "selectionColor": "#3366ff" })));
        let band = p.draw_commands.iter().position(|c| matches!(c, DrawCommand::Quad { .. })).expect("a band");
        let text = p.draw_commands.iter().position(|c| matches!(c, DrawCommand::Text { .. })).expect("the text");
        assert!(band < text, "the band is drawn first, under the text: {:?}", p.draw_commands);
        let w = crate::vector::shape("ab", 20.0, false, TextFamily::Sans).width;
        assert!((quads(&p)[0].w - w).abs() < 0.01);
        assert!(quads(&packet_for(editable(json!({ "selectionStart": 1, "selectionEnd": 1 })))).is_empty(), "control: an empty selection paints no band");
    }

    #[test]
    fn a_preedit_is_shown_at_the_caret_underlined_and_the_caret_follows_it() {
        let p = packet_for(editable(json!({ "caret": 1, "preedit": "X" })));
        let text = p.draw_commands.iter().find_map(|c| match c { DrawCommand::Text { text, .. } => Some(text.clone()), _ => None });
        assert_eq!(text.as_deref(), Some("aXb"));
        let q = quads(&p);
        assert_eq!(q.len(), 2, "an underline and the caret: {:?}", p.draw_commands);
        let after = crate::vector::shape("aX", 20.0, false, TextFamily::Sans).width;
        assert!((q[1].x + q[1].w / 2.0 - after).abs() < 0.01, "the caret stands after the preedit");
    }

    #[test]
    fn the_caret_blinks_on_the_scene_clock_and_a_caret_keeps_the_scene_drawing() {
        let scene = editable(json!({ "caret": 1 }));
        let at = |ms: f64| quads(&from_props_at(&scene, ms).unwrap()).len();
        assert_eq!(at(100.0), 1, "on in the first 530 ms");
        assert_eq!(at(600.0), 0, "off in the next 530 ms");
        assert_eq!(at(1100.0), 1, "on again");
        assert!(has_active_fx(&scene), "a caret keeps frames coming");
        assert!(!has_active_fx(&editable(json!({}))), "control: plain text does not");
    }

    fn edit_hit(extra: Value) -> (Vec<f32>, super::super::scene::TextHit) {
        let mut props = extra;
        props["hit"] = json!(true);
        let (_, gathered) = from_props_mapped(&editable(props), 0.0).unwrap();
        let hit = gathered.text_hits.into_iter().next().expect("a text hit");
        (hit.lines[0].edges.clone(), hit)
    }

    fn sans_width(text: &str) -> f32 {
        crate::vector::shape(text, 20.0, false, TextFamily::Sans).width
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn pure_arabic_caret_positions_run_right_to_left() {
        let (edges, _) = edit_hit(json!({ "text": "سلم" }));
        let w = sans_width("سلم");
        assert_eq!(edges.len(), 4);
        assert!(near(edges[0], w) && near(edges[3], 0.0), "the first index is at the right edge, the end at the left: {edges:?}");
        assert!(edges.windows(2).all(|p| p[0] > p[1]), "strictly leftward: {edges:?}");
        let p = packet_for(editable(json!({ "text": "سلم", "caret": 1 })));
        let q = quads(&p);
        assert_eq!(q.len(), 1, "one caret bar");
        assert!(near(q[0].x + q[0].w / 2.0, edges[1]), "the bar stands at the index-1 edge: {:?} vs {}", q[0], edges[1]);
    }

    #[test]
    fn a_ligature_shares_its_advance_between_its_characters() {
        let (edges, _) = edit_hit(json!({ "text": "سلام" }));
        assert_eq!(edges.len(), 5);
        assert!(near(edges[0], sans_width("سلام")) && near(edges[4], 0.0), "{edges:?}");
        assert!(edges.windows(2).all(|p| p[0] > p[1]), "strictly leftward through the lam-alef ligature: {edges:?}");
    }

    #[test]
    fn a_mixed_line_has_its_carets_in_visual_order_of_the_bidi_runs() {
        let (edges, _) = edit_hit(json!({ "text": "aسلb" }));
        let (wa, run, total) = (sans_width("a"), sans_width("سل"), sans_width("aسلb"));
        assert_eq!(edges.len(), 5);
        assert!(near(edges[0], 0.0) && near(edges[4], total), "{edges:?}");
        assert!(near(edges[1], wa), "the boundary after 'a' is at the lower-level character's edge: {edges:?}");
        assert!(near(edges[3], wa + run), "the boundary before 'b' is at b's leading edge: {edges:?}");
        assert!(edges[1] < edges[2] && edges[2] < edges[3], "inside the run the index sits between: {edges:?}");
    }

    #[test]
    fn a_selection_across_a_direction_boundary_paints_two_rects() {
        let (edges, _) = edit_hit(json!({ "text": "aسلمb" }));
        let wa = sans_width("a");
        let split = quads(&packet_for(editable(json!({ "text": "aسلمb", "selectionStart": 2, "selectionEnd": 5 }))));
        assert_eq!(split.len(), 2, "the run's left part and 'b' are not visually adjacent: {split:?}");
        let mut rects = split.clone();
        rects.sort_by(|a, b| a.x.total_cmp(&b.x));
        assert!(near(rects[0].x, wa) && near(rects[0].x + rects[0].w, edges[2]), "{rects:?} {edges:?}");
        assert!(near(rects[1].x, edges[4]) && near(rects[1].x + rects[1].w, edges[5]), "{rects:?} {edges:?}");
        let whole = quads(&packet_for(editable(json!({ "text": "aسلمb", "selectionStart": 0, "selectionEnd": 4 }))));
        assert_eq!(whole.len(), 1, "control: a range that stays visually contiguous paints one rect: {whole:?}");
    }

    #[test]
    fn a_pointer_x_maps_back_to_the_logical_index_of_its_caret_edge() {
        for text in ["سلم", "سلام", "aسلb", "aسلمb", "ab"] {
            let (edges, hit) = edit_hit(json!({ "text": text }));
            let y = hit.lines[0].top + 1.0;
            for (i, x) in edges.iter().enumerate() {
                assert_eq!(hit.index_at(*x, y), Some(i), "{text}: index {i} at x {x} in {edges:?}");
            }
        }
    }

    #[test]
    fn left_to_right_text_keeps_its_prefix_width_edges_and_right_alignment_ends_at_the_anchor() {
        let (edges, _) = edit_hit(json!({ "text": "abc" }));
        let expected: Vec<f32> = ["", "a", "ab", "abc"].iter().map(|p| sans_width(p)).collect();
        assert!(edges.iter().zip(&expected).all(|(a, b)| near(*a, *b)), "{edges:?} vs {expected:?}");
        let (right, _) = edit_hit(json!({ "text": "abc", "align": "right", "position": {"x": 200, "y": 0} }));
        let w = sans_width("abc");
        assert!(near(right[0], 200.0 - w) && near(right[3], 200.0), "{right:?}");
        let (rtl, _) = edit_hit(json!({ "text": "سلم", "align": "right", "position": {"x": 200, "y": 0} }));
        let w = sans_width("سلم");
        assert!(near(rtl[0], 200.0) && near(rtl[3], 200.0 - w), "an RTL line ends at the anchor too: {rtl:?}");
    }

    #[test]
    fn a_css_numeric_weight_is_read_like_bold_and_an_out_of_range_one_is_refused() {
        assert_eq!(
            parse_text_font(Some("600 26px sans-serif")),
            Some((26.0, 600, TextFamily::Sans))
        );
        assert_eq!(
            parse_text_font(Some("400 15px monospace")),
            Some((15.0, 400, TextFamily::Mono))
        );
        assert_eq!(
            parse_text_font(Some("bold 12px sans-serif")),
            Some((12.0, 700, TextFamily::Sans))
        );
        assert_eq!(parse_text_font(Some("1200 12px sans-serif")), None);
        assert_eq!(parse_text_font(Some("600 12px Custom")), None);
    }

    #[test]
    fn unsupported_text_font_and_baseline_do_not_silently_change_contract() {
        let p = packet_for(json!({
            "projection": "free",
            "scale": 1.0,
            "drawables": [
                { "type": "draw-text", "text": "custom", "position": {"x": 1, "y": 2},
                  "color": "#ffffff", "font": "12px Custom" },
                { "type": "draw-text", "text": "hanging", "position": {"x": 1, "y": 2},
                  "color": "#ffffff", "baseline": "hanging" }
            ]
        }));
        assert!(!p
            .draw_commands
            .iter()
            .any(|cmd| matches!(cmd, DrawCommand::Text { .. })));
    }

    #[test]
    fn stroke_only_shapes_and_svg_paths_reuse_vector_primitives() {
        let p = packet_for(json!({
            "projection": "free",
            "scale": 1.0,
            "drawables": [
                { "type": "draw-shape", "shape": "rect", "position": {"x": 1, "y": 2},
                  "width": 1, "height": 1, "stroke": "#ff0000", "strokeWidth": 3,
                  "strokeDash": [4, 2], "strokeDashOffset": 1 },
                { "type": "draw-shape", "shape": "path", "position": {"x": 2, "y": 3},
                  "d": "M0 0 L1 0 L1 1 Z", "fill": "#00ff00", "stroke": "#0000ff" }
            ]
        }));
        let paths: Vec<_> = p
            .draw_commands
            .iter()
            .filter_map(|cmd| match cmd {
                DrawCommand::Path {
                    segments,
                    fill,
                    stroke,
                } => Some((segments, fill, stroke)),
                _ => None,
            })
            .collect();
        assert_eq!(paths.len(), 2);
        assert!(paths[0].1.is_none());
        let rect_stroke = paths[0].2.as_ref().expect("rect stroke");
        assert_eq!(rect_stroke.width, 3.0);
        assert_eq!(rect_stroke.dash, vec![4.0, 2.0]);
        assert_eq!(rect_stroke.dash_offset, 1.0);
        assert!(matches!(paths[0].0.last(), Some(PathSegment::Close)));
        assert_eq!(paths[1].1.expect("path fill")[1], 1.0);
        assert!(paths[1].2.is_some());
        assert!(paths[1].0.len() >= 4);
    }

    #[test]
    fn draw_shape_rotation_uses_its_world_pivot() {
        let p = packet_for(json!({
            "projection": "free", "scale": 1.0,
            "drawables": [{"type": "draw-shape", "shape": "rect",
                "position": {"x": 0, "y": 0}, "width": 256, "height": 256,
                "fill": "#ff0000", "rotate": std::f32::consts::FRAC_PI_2,
                "pivot": {"x": 128, "y": 128}}]
        }));
        let points = p
            .draw_commands
            .iter()
            .find_map(|cmd| match cmd {
                DrawCommand::Poly { points, .. } => Some(points),
                _ => None,
            })
            .expect("rotated rectangle polygon");
        assert!((points[0].x - 256.0).abs() < 1e-3);
        assert!(points[0].y.abs() < 1e-3);
    }

    #[test]
    fn draw_poly_offsets_are_pixels_in_a_free_scene_like_path_offsets() {
        let p = packet_for(json!({
            "projection": "free", "scale": 1.0,
            "drawables": [{"type": "draw-shape", "shape": "poly",
                "position": {"x": 0, "y": 0},
                "points": [{"x": 0, "y": 0}, {"x": 256, "y": 0}, {"x": 0, "y": 256}],
                "offsetX": 64, "offsetY": -128, "fill": "#ff0000"}]
        }));
        let points = p
            .draw_commands
            .iter()
            .find_map(|command| match command {
                DrawCommand::Poly { points, .. } => Some(points),
                _ => None,
            })
            .expect("filled polygon");
        assert_eq!(points[0].x, 0.25 * TILE_WIDTH);
        assert_eq!(points[0].y, -0.5 * TILE_WIDTH);
    }

    #[test]
    fn draw_shape_animation_repaints_authored_geometry_and_color() {
        let scene = json!({
            "projection": "free", "scale": 1.0,
            "drawables": [{"type": "draw-shape", "shape": "rect",
                "position": {"x": 0, "y": 0}, "height": 256,
                "width": 0, "fill": "#ff0000",
                "animation": {"durationMs": 1000, "loop": false,
                    "keyframes": [
                        {"at": 0, "width": 0, "fill": "#ff0000"},
                        {"at": 1, "width": 512, "fill": "#0000ff"}
                    ]}}]
        });
        assert!(has_active_fx(&scene));
        let halfway = from_props_at(&scene, 500.0).unwrap();
        let finished = from_props_at(&scene, 1500.0).unwrap();
        let quad = |packet: &GameRenderPacket| {
            packet
                .draw_commands
                .iter()
                .find_map(|command| match command {
                    DrawCommand::Quad { rect, color } => Some((*rect, *color)),
                    _ => None,
                })
                .expect("animated rectangle")
        };
        let (half_rect, half_color) = quad(&halfway);
        let (final_rect, final_color) = quad(&finished);
        assert_eq!(half_rect.w, TILE_WIDTH);
        assert_eq!(final_rect.w, 2.0 * TILE_WIDTH);
        assert_eq!(half_color[0], 1.0);
        assert_eq!(final_color[2], 1.0);
    }

    #[test]
    fn unresolved_sprite_is_a_typed_error() {
        let scene = json!({
            "projection": "free", "scale": 1.0,
            "drawables": [{"type": "draw-sprite", "position": {"x": 0, "y": 0},
                "width": 1, "height": 0.5, "opacity": 0.5}]
        });
        assert!(matches!(
            from_props(&scene),
            Err(CanvasSceneError::UnresolvedSprite(_))
        ));
        let resolved = json!({
            "projection": "free", "scale": 1.0,
            "drawables": [{"type": "draw-sprite", "position": {"x": 0, "y": 0},
                "asset": {"url": "data:image/svg+xml;utf8,%3Csvg%20xmlns%3D%22http%3A%2F%2Fwww.w3.org%2F2000%2Fsvg%22%20width%3D%2230%22%20height%3D%2210%22%2F%3E"}}]
        });
        assert!(from_props(&resolved).is_ok());
    }

    #[test]
    fn skinned_mesh_errors_reach_the_scene_error() {
        let bad = json!({
            "projection": "free", "scale": 1.0,
            "drawables": [{"type": "draw-skinned-mesh", "position": {"x": 0, "y": 0},
                "asset": {"url": "unused"}, "weightsOverlay": true,
                "mesh": {"vertices": [{"x":0,"y":0,"u":0,"v":0,"weights":[{"bone":3,"w":1}]}],
                    "triangles": [[0, 0, 0]]},
                "bones": [{"name":"root","pivot":[0,0],"tip":[1,0]}]}]
        });
        assert!(matches!(
            from_props(&bad),
            Err(CanvasSceneError::Skin(
                crate::skinning_2d::SkinError::BoneIndex { .. }
            ))
        ));
        let no_mesh = json!({
            "projection": "free", "scale": 1.0,
            "drawables": [{"type": "draw-skinned-mesh", "position": {"x": 0, "y": 0},
                "asset": {"url": "unused"}, "weightsOverlay": true,
                "bones": [{"name":"root","pivot":[0,0],"tip":[1,0]}]}]
        });
        assert!(matches!(
            from_props(&no_mesh),
            Err(CanvasSceneError::SkinnedMesh(_))
        ));
    }

    #[test]
    fn fx_layer_errors_reach_the_scene_error() {
        let props = json!({
            "projection": "free", "scale": 1.0,
            "drawables": [{ "type": "draw-fx-layer", "items": [] }]
        });
        assert!(matches!(
            from_props(&props),
            Err(CanvasSceneError::Fx(crate::fx_projection::FxError::Layer {
                field: "tickMs",
                ..
            }))
        ));
    }

    #[test]
    fn background_color_remains_under_camera_tiled_whole_image() {
        let background = "data:image/svg+xml;utf8,%3Csvg%20xmlns%3D%22http%3A%2F%2Fwww.w3.org%2F2000%2Fsvg%22%20width%3D%2230%22%20height%3D%2210%22%2F%3E";
        let packet = packet_for(json!({
            "width": 60, "height": 20, "bgColor": "#112233",
            "backgroundImage": background, "drawables": []
        }));
        assert!(
            matches!(packet.draw_commands.first(), Some(DrawCommand::Clear { color }) if color[0] == 17.0 / 255.0)
        );
        let tiles: Vec<_> = packet
            .draw_commands
            .iter()
            .filter_map(|command| match command {
                DrawCommand::Sprite { dst, .. } => Some(dst),
                _ => None,
            })
            .collect();
        assert_eq!(tiles.len(), 9);
        assert!(tiles.iter().all(|tile| tile.w == 30.0 && tile.h == 10.0));
        assert_eq!(tiles[0].x, -30.0);
        assert_eq!(tiles[0].y, -10.0);
    }

    #[test]
    fn draw_group_translates_nested_2d_items_and_multiplies_alpha() {
        let plain = packet_for(json!({
            "projection": "free", "scale": 1.0,
            "drawables": [{"type": "draw-shape", "shape": "rect",
                "position": {"x": 0, "y": 0}, "width": 1, "height": 1,
                "fill": "#ff0000"}]
        }));
        let grouped = packet_for(json!({
            "projection": "free", "scale": 1.0,
            "drawables": [{"type": "draw-group", "position": {"x": 2, "y": 3},
                "opacity": 0.5, "items": [{"type": "draw-shape", "shape": "rect",
                    "position": {"x": 0, "y": 0}, "width": 1, "height": 1,
                    "fill": "#ff0000"}]}]
        }));
        let rect = |packet: &GameRenderPacket| {
            packet
                .draw_commands
                .iter()
                .find_map(|cmd| match cmd {
                    DrawCommand::Quad { rect, color } => Some((*rect, *color)),
                    _ => None,
                })
                .expect("filled rect")
        };
        let (plain_rect, _) = rect(&plain);
        let (group_rect, group_color) = rect(&grouped);
        assert!(group_rect.x > plain_rect.x);
        assert!(group_rect.y > plain_rect.y);
        assert_eq!(group_color[3], 0.5);
    }

    #[test]
    fn draw_group_animation_moves_children_on_the_host_clock() {
        let scene = json!({
            "projection": "free", "scale": 1.0,
            "drawables": [{"type": "draw-group", "position": {"x": 0, "y": 0},
                "animation": {"durationMs": 1000, "loop": false,
                    "keyframes": [
                        {"at": 0, "offsetX": 0, "opacity": 1},
                        {"at": 1, "offsetX": 256, "opacity": 0.5}
                    ]},
                "items": [{"type": "draw-shape", "shape": "rect",
                    "position": {"x": 0, "y": 0}, "width": 256, "height": 256,
                    "fill": "#ff0000"}]}]
        });
        assert!(has_active_fx(&scene));
        let initial = from_props_at(&scene, 0.0).unwrap();
        let final_frame = from_props_at(&scene, 1000.0).unwrap();
        let quad = |packet: &GameRenderPacket| {
            packet
                .draw_commands
                .iter()
                .find_map(|command| match command {
                    DrawCommand::Quad { rect, color } => Some((*rect, *color)),
                    _ => None,
                })
                .expect("grouped rectangle")
        };
        let (initial_rect, _) = quad(&initial);
        let (final_rect, final_color) = quad(&final_frame);
        assert!((final_rect.x - initial_rect.x - TILE_WIDTH).abs() < 0.1);
        assert_eq!(final_color[3], 0.5);
    }

    #[test]
    fn draw_group_clip_scopes_mixed_child_commands() {
        let p = packet_for(json!({
            "projection": "free", "scale": 1.0,
            "drawables": [{"type": "draw-group", "position": {"x": 2, "y": 3},
                "clip": "M0 0 L1 0 L1 1 Z", "items": [{"type": "draw-shape",
                    "shape": "rect", "position": {"x": 0, "y": 0},
                    "width": 1, "height": 1, "fill": "#ff0000"}]}]
        }));
        let group = p
            .draw_commands
            .iter()
            .find_map(|command| match command {
                DrawCommand::ClipGroup { clip, commands } => Some((clip, commands)),
                _ => None,
            })
            .expect("scoped clip group");
        assert!(group.0.len() >= 4);
        assert!(group
            .1
            .iter()
            .any(|command| matches!(command, DrawCommand::Quad { .. })));
        assert!(!p
            .draw_commands
            .iter()
            .any(|command| matches!(command, DrawCommand::Quad { .. })));
    }

    #[test]
    fn draw_group_uses_shared_affine_transform_for_rotated_shape() {
        let p = packet_for(json!({
            "projection": "free", "scale": 1.0,
            "drawables": [{"type": "draw-group", "position": {"x": 2, "y": 3},
                "scale": 2, "rotate": 1.5707963267948966,
                "items": [{"type": "draw-shape", "shape": "rect",
                    "position": {"x": 0, "y": 0}, "width": 1, "height": 1,
                    "fill": "#ff0000"}]}]
        }));
        assert!(p
            .draw_commands
            .iter()
            .any(|cmd| matches!(cmd, DrawCommand::Poly { points, .. } if points.len() == 4)));
        assert!(!p
            .draw_commands
            .iter()
            .any(|cmd| matches!(cmd, DrawCommand::Quad { .. })));
    }

    #[test]
    fn fx_layer_uses_core_projection_and_epoch_paint_clock() {
        let props = json!({
            "projection": "free", "scale": 1.0,
            "drawables": [{ "type": "draw-fx-layer", "tickMs": 500,
                "fontFamily": "system-ui, sans-serif", "textColor": "#ffe066",
                "items": [{ "id": "fx-1", "type": "hit", "position": { "x": 2, "y": 3 },
                    "ttl": 2, "maxTtl": 4, "bornAt": 1000, "message": "-2" }],
                "presets": [{ "type": "hit", "shape": "ring", "color": "#ff0000", "size": 0.5,
                    "vx": 0, "vz": 0, "gravity": 0 }]
            }]
        });
        let p = from_props_at(&props, 1500.0).unwrap();
        let stroke_alpha = p
            .draw_commands
            .iter()
            .find_map(|cmd| match cmd {
                DrawCommand::Path {
                    stroke: Some(stroke),
                    ..
                } => Some(stroke.color[3]),
                DrawCommand::AdditiveGroup { commands } => {
                    commands.iter().find_map(|inner| match inner {
                        DrawCommand::Path {
                            stroke: Some(stroke),
                            ..
                        }
                        | DrawCommand::GradientPath {
                            stroke: Some(stroke),
                            ..
                        } => Some(stroke.color[3]),
                        _ => None,
                    })
                }
                _ => None,
            })
            .expect("ring stroke");
        assert_eq!(stroke_alpha, 0.75);
        assert!(p
            .draw_commands
            .iter()
            .any(|cmd| matches!(cmd, DrawCommand::Text { text, .. } if text == "-2")));
        let expired = from_props_at(&props, 3000.0).unwrap();
        assert!(!expired.draw_commands.iter().any(|cmd| matches!(
            cmd,
            DrawCommand::Text { .. } | DrawCommand::Path { .. } | DrawCommand::AdditiveGroup { .. }
        )));
    }

    #[test]
    fn clear_uses_bgcolor_then_fallback() {
        let p = packet_for(json!({}));
        match &p.draw_commands[0] {
            DrawCommand::Clear { color } => assert_eq!(*color, BACKGROUND_FALLBACK),
            other => panic!("expected Clear, got {other:?}"),
        }
        let p = packet_for(json!({ "bgColor": "#000000" }));
        match &p.draw_commands[0] {
            DrawCommand::Clear { color } => assert_eq!(*color, [0.0, 0.0, 0.0, 1.0]),
            other => panic!("expected Clear, got {other:?}"),
        }
    }

    #[test]
    fn cell_shape_emits_diamond_poly_in_screen_space() {
        // Single cell at (0,0), iso, scale 1, viewport 800x600, no cameraPos.
        // Camera: zoom=1, cam=(0,0) → screen = (world - 400, world - 300) + (400,300) = world.
        let p = packet_for(json!({
            "scale": 1.0,
            "drawables": [
                { "type": "draw-shape", "shape": "cell",
                  "position": { "x": 0, "y": 0 }, "fill": "#4ade80" }
            ]
        }));
        let poly = p
            .draw_commands
            .iter()
            .find_map(|c| match c {
                DrawCommand::Poly { points, color } => Some((points, color)),
                _ => None,
            })
            .expect("a Poly for the cell");
        assert_eq!(poly.0.len(), 4);
        // Diamond top vertex at (tw/2, diamondTopY) = (128, 374).
        assert!((poly.0[0].x - 128.0).abs() < 1e-3);
        assert!((poly.0[0].y - 374.0).abs() < 1e-3);
        assert!((poly.1[0] - 74.0 / 255.0).abs() < 1e-3); // #4ade80 → 74,222,128
        assert!((poly.1[1] - 222.0 / 255.0).abs() < 1e-3);
    }

    #[test]
    fn camera_pos_centers_viewport() {
        // cameraPos (0,0) iso scale 1: anchor 'center' = (128, 374+64) = (128, 438);
        // cam = (128-400, 438-300) = (-272, 138).
        // screen(p) = (p - (400,300) - cam) + (400,300) = p - cam.
        let p = packet_for(json!({
            "scale": 1.0,
            "cameraPos": { "x": 0, "y": 0 },
            "drawables": [
                { "type": "draw-shape", "shape": "rect",
                  "position": { "x": 0, "y": 0 }, "width": 1, "height": 1,
                  "fill": "#ffffff" }
            ]
        }));
        let rect = p
            .draw_commands
            .iter()
            .find_map(|c| match c {
                DrawCommand::Quad { rect, .. } => Some(rect),
                _ => None,
            })
            .expect("a Quad for the rect");
        // world (0,0) → screen (0 - (-272), 0 - 138) = (272, -138).
        assert!((rect.x - 272.0).abs() < 1e-3);
        assert!((rect.y + 138.0).abs() < 1e-3);
        assert!((rect.w - 256.0).abs() < 1e-3); // width 1 * tw, zoom 1
    }

    #[test]
    fn layer_items_paint_with_layer_atom() {
        let p = packet_for(json!({
            "scale": 1.0,
            "drawables": [
                { "type": "draw-shape-layer", "items": [
                    { "shape": "rect", "position": { "x": 0, "y": 0 },
                      "width": 0.5, "height": 0.5, "fill": "#ff0000" },
                    { "shape": "rect", "position": { "x": 1, "y": 0 },
                      "width": 0.5, "height": 0.5, "fill": "#00ff00" }
                ]}
            ]
        }));
        let quads = p
            .draw_commands
            .iter()
            .filter(|c| matches!(c, DrawCommand::Quad { .. }))
            .count();
        assert_eq!(quads, 2);
    }

    #[test]
    fn base_offset_applies_for_iso_grids() {
        // Two rows (max y = 1) → baseOffsetX = 1 * 256/2 = 128 at scale 1.
        let p = packet_for(json!({
            "scale": 1.0,
            "drawables": [
                { "type": "draw-shape", "shape": "cell",
                  "position": { "x": 0, "y": 0 }, "fill": "#000000" },
                { "type": "draw-shape", "shape": "cell",
                  "position": { "x": 0, "y": 1 }, "fill": "#000000" }
            ]
        }));
        let first = p
            .draw_commands
            .iter()
            .find_map(|c| match c {
                DrawCommand::Poly { points, .. } => Some(points),
                _ => None,
            })
            .expect("first cell poly");
        // Cell (0,0) project: iso (0*128 + 128, 0) → top vertex (128+128, 374).
        assert!((first[0].x - 256.0).abs() < 1e-3);
    }

    fn evade_like_node() -> Value {
        json!({
            "type": "canvas",
            "mode": "2d",
            "projection": "flat",
            "camera": { "zoom": 0.5, "pos": { "x": 2.0, "y": 1.0 } },
            "backgroundColor": "#0f172a",
            "children": [
                { "type": "draw-shape", "shape": "rect", "position": { "x": 0, "y": 0 },
                  "width": 1, "height": 1, "fill": "#ff0000" },
                { "type": "draw-shape-layer", "items": [
                    { "shape": "rect", "position": { "x": 1, "y": 0 },
                      "width": 1, "height": 1, "fill": "#00ff00" } ] },
                "@trait.Ignored"
            ]
        })
    }

    #[test]
    fn canvas_node_children_route_to_drawables() {
        let routed = from_canvas_node(&evade_like_node()).unwrap();
        let direct = from_props(&json!({
            "projection": "flat", "scale": 0.5, "cameraPos": { "x": 2.0, "y": 1.0 },
            "bgColor": "#0f172a",
            "drawables": [
                { "type": "draw-shape", "shape": "rect", "position": { "x": 0, "y": 0 },
                  "width": 1, "height": 1, "fill": "#ff0000" },
                { "type": "draw-shape-layer", "items": [
                    { "shape": "rect", "position": { "x": 1, "y": 0 },
                      "width": 1, "height": 1, "fill": "#00ff00" } ] }
            ]
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(&routed).unwrap(),
            serde_json::to_value(&direct).unwrap()
        );
        let quads = routed
            .draw_commands
            .iter()
            .filter(|c| matches!(c, DrawCommand::Quad { .. }))
            .count();
        assert_eq!(quads, 2, "string child dropped, both shapes painted");
    }

    #[test]
    fn canvas_node_control_without_children_paints_background_only() {
        let p =
            from_canvas_node(&json!({ "type": "canvas", "backgroundColor": "#000000" })).unwrap();
        assert_eq!(p.draw_commands.len(), 1);
        match &p.draw_commands[0] {
            DrawCommand::Clear { color } => assert_eq!(*color, [0.0, 0.0, 0.0, 1.0]),
            other => panic!("expected Clear, got {other:?}"),
        }
    }

    #[test]
    fn canvas_node_camera_zoom_scales_the_scene() {
        let at = |zoom: f64| {
            let p = from_canvas_node(&json!({
                "type": "canvas", "projection": "flat", "camera": { "zoom": zoom },
                "children": [ { "type": "draw-shape", "shape": "rect", "position": { "x": 0, "y": 0 },
                                "width": 1, "height": 1, "fill": "#ffffff" } ]
            })).unwrap();
            match p.draw_commands.iter().find_map(|c| match c {
                DrawCommand::Quad { rect, .. } => Some(*rect),
                _ => None,
            }) {
                Some(r) => r.w,
                None => panic!("no quad"),
            }
        };
        assert!((at(1.0) - 256.0).abs() < 1e-3);
        assert!(
            at(0.5) < at(1.0),
            "camera.zoom must reach the mapper's scale"
        );
        assert!(at(0.25) < at(0.5));
    }

    #[test]
    fn canvas_2d_string_camera_is_not_a_camera_object() {
        let p = from_canvas_node(&json!({
            "type": "canvas-2d", "camera": "fixed", "scale": 1.0, "projection": "flat",
            "drawables": [ { "type": "draw-shape", "shape": "rect", "position": { "x": 0, "y": 0 },
                             "width": 1, "height": 1, "fill": "#ffffff" } ]
        }))
        .unwrap();
        assert!(p
            .draw_commands
            .iter()
            .any(|c| matches!(c, DrawCommand::Quad { rect, .. } if (rect.w - 256.0).abs() < 1e-3)));
    }

    #[test]
    fn canvas_node_explicit_drawables_win_over_children() {
        let p = from_canvas_node(&json!({
            "type": "canvas", "projection": "flat", "scale": 1.0,
            "drawables": [ { "type": "draw-shape", "shape": "rect", "position": { "x": 0, "y": 0 },
                             "width": 1, "height": 1, "fill": "#ffffff" } ],
            "children": [ { "type": "draw-shape", "shape": "rect", "position": { "x": 1, "y": 0 },
                            "width": 1, "height": 1, "fill": "#ffffff" },
                          { "type": "draw-shape", "shape": "rect", "position": { "x": 2, "y": 0 },
                            "width": 1, "height": 1, "fill": "#ffffff" } ]
        }))
        .unwrap();
        let quads = p
            .draw_commands
            .iter()
            .filter(|c| matches!(c, DrawCommand::Quad { .. }))
            .count();
        assert_eq!(quads, 1);
    }

    #[test]
    fn canvas_node_nested_props_shape_matches_flat() {
        let flat = json!({ "type": "canvas", "projection": "flat", "scale": 1.0,
            "children": [ { "type": "draw-shape", "shape": "rect", "position": { "x": 0, "y": 0 },
                            "width": 1, "height": 1, "fill": "#ffffff" } ] });
        let nested = json!({ "type": "canvas", "props": { "projection": "flat", "scale": 1.0,
            "children": [ { "type": "draw-shape", "props": { "shape": "rect", "position": { "x": 0, "y": 0 },
                            "width": 1, "height": 1, "fill": "#ffffff" } } ] } });
        assert_eq!(
            serde_json::to_value(from_canvas_node(&flat).unwrap()).unwrap(),
            serde_json::to_value(from_canvas_node(&nested).unwrap()).unwrap()
        );
    }

    #[test]
    fn canvas_node_refuses_3d_and_non_objects() {
        assert_eq!(
            from_canvas_node(&json!({ "type": "canvas", "mode": "3d" })).unwrap_err(),
            CanvasSceneError::UnsupportedMode("3d".into())
        );
        assert_eq!(
            from_canvas_node(&json!([1, 2])).unwrap_err(),
            CanvasSceneError::NotAnObject
        );
        assert!(from_canvas_node(&json!({ "type": "canvas", "mode": "2d" })).is_ok());
    }

    #[test]
    fn viewport_overrides_size_and_keeps_scene_undistorted() {
        let node = evade_like_node();
        let tall = from_canvas_node_in_viewport(&node, 400.0, 800.0).unwrap();
        assert_eq!((tall.width, tall.height), (400.0, 800.0));
        let quad_dims = |p: &GameRenderPacket| -> Vec<(f32, f32)> {
            p.draw_commands
                .iter()
                .filter_map(|c| match c {
                    DrawCommand::Quad { rect, .. } => Some((rect.w, rect.h)),
                    _ => None,
                })
                .collect()
        };
        let wide = from_canvas_node_in_viewport(&node, 800.0, 400.0).unwrap();
        assert_eq!(
            quad_dims(&tall),
            quad_dims(&wide),
            "cell size is viewport-independent"
        );
        assert!(!quad_dims(&tall).is_empty());
    }

    #[test]
    fn viewport_equal_to_default_matches_plain_node_mapping() {
        let node = evade_like_node();
        let a = from_canvas_node(&node).unwrap();
        let b = from_canvas_node_in_viewport(&node, 800.0, 600.0).unwrap();
        assert_eq!(
            serde_json::to_value(&a).unwrap(),
            serde_json::to_value(&b).unwrap()
        );
    }

    #[test]
    fn viewport_still_refuses_3d() {
        assert_eq!(
            from_canvas_node_in_viewport(&json!({ "type": "canvas", "mode": "3d" }), 10.0, 10.0)
                .unwrap_err(),
            CanvasSceneError::UnsupportedMode("3d".into())
        );
    }

    fn first_sprite_dst(p: &GameRenderPacket) -> Rect {
        p.draw_commands
            .iter()
            .find_map(|c| match c {
                DrawCommand::Sprite { dst, .. } => Some(dst.clone()),
                _ => None,
            })
            .expect("a sprite command")
    }

    fn sprite_scene(projection: &str, extra: Value) -> Value {
        let mut sprite = json!({
            "type": "draw-sprite", "position": { "x": 0, "y": 0 },
            "asset": { "url": "data:image/svg+xml,%3Csvg%20xmlns%3D%27http%3A%2F%2Fwww.w3.org%2F2000%2Fsvg%27%20width%3D%2764%27%20height%3D%2764%27%2F%3E" },
            "frame": { "x": 0, "y": 0, "w": 64, "h": 64 }
        });
        if let (Some(dst), Some(src)) = (sprite.as_object_mut(), extra.as_object()) {
            dst.extend(src.clone());
        }
        json!({ "projection": projection, "scale": 0.5, "drawables": [sprite] })
    }

    /// `spriteRect`: no width/height = one cell on a tile grid, whatever the art's px size.
    #[test]
    fn omitted_sprite_size_is_one_cell_on_tile_grids_not_the_source_px() {
        let dst = first_sprite_dst(&packet_for(sprite_scene("isometric", json!({}))));
        let cell = TILE_WIDTH * 0.5 * 0.5;
        assert!(
            (dst.w - cell).abs() < 1e-3 && (dst.h - cell).abs() < 1e-3,
            "{dst:?}"
        );
    }

    #[test]
    fn explicit_sprite_size_is_in_cell_fractions() {
        let dst = first_sprite_dst(&packet_for(sprite_scene(
            "isometric",
            json!({ "width": 2 }),
        )));
        let cell = TILE_WIDTH * 0.5 * 0.5;
        assert!(
            (dst.w - 2.0 * cell).abs() < 1e-3 && (dst.h - cell).abs() < 1e-3,
            "{dst:?}"
        );
    }

    /// Control: a world-pixel `free` board keeps the source's native px size.
    #[test]
    fn omitted_sprite_size_is_native_px_on_free_boards() {
        let dst = first_sprite_dst(&packet_for(sprite_scene("free", json!({}))));
        assert!(
            (dst.w - 64.0 * 0.5).abs() < 1e-3 && (dst.h - 64.0 * 0.5).abs() < 1e-3,
            "{dst:?}"
        );
    }

    fn free_props(drawables: Value) -> Value {
        json!({"width": 200, "height": 100, "scale": 1.0, "projection": "free",
            "bgColor": "#00000000", "drawables": drawables})
    }

    fn rect_shape(extra: Value) -> Value {
        let mut shape = json!({"type": "draw-shape", "shape": "rect",
            "position": {"x": 10, "y": 20}, "width": 64, "height": 32, "fill": "#ff0000"});
        for (key, value) in extra.as_object().unwrap() {
            shape[key] = value.clone();
        }
        shape
    }

    fn sdf_commands(packet: &GameRenderPacket) -> Vec<&DrawCommand> {
        packet
            .draw_commands
            .iter()
            .filter(|c| matches!(c, DrawCommand::Sdf { .. }))
            .collect()
    }

    #[test]
    fn corners_a_material_and_the_new_kinds_map_to_sdf_and_a_plain_rect_keeps_its_quad() {
        let plain = packet_for(free_props(json!([rect_shape(json!({}))])));
        assert!(plain
            .draw_commands
            .iter()
            .any(|c| matches!(c, DrawCommand::Quad { .. })));
        assert!(
            sdf_commands(&plain).is_empty(),
            "control: no new field, no Sdf"
        );

        let cornered = packet_for(free_props(json!([rect_shape(
            json!({"corners": {"radius": [4, 4, 4, 4], "kind": "bevel"}})
        )])));
        assert!(!cornered
            .draw_commands
            .iter()
            .any(|c| matches!(c, DrawCommand::Quad { .. })));
        match sdf_commands(&cornered).as_slice() {
            [DrawCommand::Sdf {
                kind,
                rect,
                corners,
                corner_kind,
                fill,
                ..
            }] => {
                assert_eq!(*kind, SdfKind::Rect);
                assert_eq!(
                    *rect,
                    Rect {
                        x: 10.0,
                        y: 20.0,
                        w: 64.0,
                        h: 32.0
                    }
                );
                assert_eq!((*corners, *corner_kind), ([4.0; 4], 1));
                assert_eq!(*fill, [1.0, 0.0, 0.0, 1.0]);
            }
            other => panic!("one Sdf: {other:?}"),
        }
        for (shape, extra, expected) in [
            ("capsule", json!({}), SdfKind::Capsule),
            ("arc", json!({"sweep": 3.0, "thickness": 4}), SdfKind::Arc),
            (
                "polygon",
                json!({"sides": 8, "starRatio": 0.7}),
                SdfKind::Polygon,
            ),
        ] {
            let mut node = rect_shape(extra);
            node["shape"] = json!(shape);
            let packet = packet_for(free_props(json!([node])));
            assert!(
                matches!(sdf_commands(&packet).as_slice(), [DrawCommand::Sdf { kind, .. }] if *kind == expected),
                "{shape}"
            );
        }
    }

    #[test]
    fn invalid_sdf_fields_are_scene_errors_and_a_bad_wgsl_names_material_wgsl() {
        let bad = from_props(&free_props(json!([rect_shape(
            json!({"material": {"wgsl": "fn nope() {}", "params": [], "live": false}})
        )])))
        .unwrap_err();
        assert!(
            matches!(&bad, CanvasSceneError::InvalidProp { field, .. } if field == "material.wgsl"),
            "{bad:?}"
        );
        let good = packet_for(free_props(json!([rect_shape(json!({"material": {
            "wgsl": "fn shade(px: Pixel) -> vec4<f32> { return px.color; }",
            "params": [1, 2], "live": true}}))])));
        assert!(matches!(
            sdf_commands(&good).as_slice(),
            [DrawCommand::Sdf { material: Some(m), .. }] if m.params == vec![1.0, 2.0] && m.live
        ));
        let kind = from_props(&free_props(json!([rect_shape(
            json!({"corners": {"radius": [1, 1, 1, 1], "kind": "wavy"}})
        )])))
        .unwrap_err();
        assert!(
            matches!(&kind, CanvasSceneError::InvalidProp { field, .. } if field == "corners.kind"),
            "{kind:?}"
        );
    }

    #[test]
    fn anchor_since_reaches_an_sdf_material_in_seconds_and_rests_without_an_anchor_or_epoch() {
        const WGSL: &str = "fn shade(px: Pixel) -> vec4<f32> { return vec4<f32>(px.since, 0.0, 0.0, 1.0); }";
        let since_at = |material: Value, epoch: f64| {
            let packet =
                from_props_at(&free_props(json!([rect_shape(json!({"material": material}))])), epoch)
                    .unwrap();
            match sdf_commands(&packet).as_slice() {
                [DrawCommand::Sdf { material: Some(m), .. }] => m.since,
                other => panic!("one material shape, got {other:?}"),
            }
        };
        let anchored = json!({"wgsl": WGSL, "anchor": 1_700_000_000_000.0});
        assert_eq!(since_at(anchored.clone(), 1_700_000_002_500.0), 2.5);
        assert_eq!(since_at(anchored, 0.0), 0.0);
        assert_eq!(since_at(json!({"wgsl": WGSL}), 1_700_000_002_500.0), 0.0);
        let bad = from_props(&free_props(json!([rect_shape(
            json!({"material": {"wgsl": WGSL, "anchor": "soon"}})
        )])))
        .unwrap_err();
        assert!(
            matches!(&bad, CanvasSceneError::InvalidProp { field, .. } if field == "material.anchor"),
            "{bad:?}"
        );
    }

    #[test]
    fn a_semantic_value_from_a_text_takes_that_text_as_its_value() {
        let combo = |sem: Value| {
            json!({"type": "draw-shape", "shape": "rect", "id": "season", "hit": true, "anchor": "top-left",
                "position": {"x": 0, "y": 0}, "width": 40, "height": 20, "fill": "#000000", "semantic": sem})
        };
        let shown = json!({"type": "draw-text", "id": "season:value", "text": "الربيع", "anchor": "top-left",
            "position": {"x": 0, "y": 0}, "color": "#ffffff"});
        let (_, gathered) = from_props_mapped(
            &free_props(json!([combo(json!({"role": "combobox", "name": "Season", "valueFrom": "season:value"})), shown.clone()])),
            0.0,
        )
        .unwrap();
        assert_eq!(gathered.semantic[0].value.as_deref(), Some("الربيع"));
        let (_, plain) = from_props_mapped(
            &free_props(json!([combo(json!({"role": "combobox", "name": "Season", "value": "spring"})), shown.clone()])),
            0.0,
        )
        .unwrap();
        assert_eq!(plain.semantic[0].value.as_deref(), Some("spring"), "control: a literal value is kept");
        let both = from_props_mapped(
            &free_props(json!([combo(json!({"role": "combobox", "name": "Season", "value": "spring", "valueFrom": "season:value"})), shown])),
            0.0,
        )
        .unwrap_err();
        assert!(matches!(&both, CanvasSceneError::InvalidProp { field, .. } if field == "semantic.valueFrom"), "{both:?}");
    }

    #[test]
    fn a_semantic_node_labelled_by_a_text_takes_that_text_as_its_name() {
        let button = |sem: Value| {
            json!({"type": "draw-shape", "shape": "rect", "id": "begin", "hit": true, "anchor": "top-left",
                "position": {"x": 0, "y": 0}, "width": 40, "height": 20, "fill": "#000000", "semantic": sem})
        };
        let label = json!({"type": "draw-text", "id": "begin:label", "text": "ابدأ", "anchor": "top-left",
            "position": {"x": 0, "y": 0}, "color": "#ffffff"});
        let (_, gathered) = from_props_mapped(
            &free_props(json!([button(json!({"role": "button", "labelledBy": "begin:label"})), label.clone()])),
            0.0,
        )
        .unwrap();
        assert_eq!(gathered.semantic[0].name, "ابدأ", "the text painted after the node still names it");
        let (_, plain) =
            from_props_mapped(&free_props(json!([button(json!({"role": "button", "name": "Begin"})), label.clone()])), 0.0)
                .unwrap();
        assert_eq!(plain.semantic[0].name, "Begin", "control: a literal name is kept");
        let missing = from_props_mapped(
            &free_props(json!([button(json!({"role": "button", "labelledBy": "nowhere"})), label.clone()])),
            0.0,
        )
        .unwrap_err();
        assert!(matches!(&missing, CanvasSceneError::InvalidProp { field, .. } if field == "semantic.labelledBy"), "{missing:?}");
        let both = from_props_mapped(
            &free_props(json!([button(json!({"role": "button", "name": "Begin", "labelledBy": "begin:label"})), label])),
            0.0,
        )
        .unwrap_err();
        assert!(matches!(&both, CanvasSceneError::InvalidProp { field, .. } if field == "semantic.labelledBy"), "{both:?}");
    }

    #[test]
    fn semantic_drawables_form_the_tree_in_reading_order_at_their_regions() {
        let node = |id: &str, hit: bool, x: i32, sem: Value| {
            json!({"type": "draw-shape", "shape": "rect", "id": id, "hit": hit, "anchor": "top-left",
                "position": {"x": x, "y": 0}, "width": 40, "height": 20, "fill": "#000000", "semantic": sem})
        };
        let (_, gathered) = from_props_mapped(
            &free_props(json!([
                node("second", true, 0, json!({"role": "button", "name": "Two", "order": 2})),
                node("first", false, 50, json!({"role": "textbox", "name": "One", "value": "hi", "caret": 1, "focused": true, "order": 1})),
                json!({"type": "draw-shape", "shape": "rect", "anchor": "top-left", "position": {"x": 0, "y": 0},
                    "width": 10, "height": 10, "fill": "#000000", "semantic": {"role": "button", "name": "no id"}}),
            ])),
            0.0,
        )
        .unwrap();
        let mut nodes = gathered.semantic.clone();
        nodes.sort_by(|a, b| a.order.total_cmp(&b.order));
        let ids: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["first", "second"], "control: a semantic drawable with no id is no node");
        assert_eq!(nodes[0].caret, Some(1));
        assert!(nodes[0].focused && nodes[0].value.as_deref() == Some("hi"));
        assert_eq!((nodes[0].rect.x, nodes[0].rect.w), (50.0, 40.0), "painted bounds without a hit");
        assert_eq!((nodes[1].rect.x, nodes[1].rect.w), (0.0, 40.0), "the pointer region with a hit");
    }

    #[test]
    fn a_semantic_node_without_a_role_is_a_scene_error() {
        let result = from_props_mapped(
            &free_props(json!([{"type": "draw-shape", "shape": "rect", "id": "x", "anchor": "top-left",
                "position": {"x": 0, "y": 0}, "width": 10, "height": 10, "fill": "#000000", "semantic": {"name": "nameless role"}}])),
            0.0,
        );
        assert!(result.is_err());
    }

    #[test]
    fn an_unpainted_hit_rect_is_still_a_region_and_an_unpainted_plain_rect_is_not() {
        let hit_area = |id: &str, hit: bool| {
            json!({"type": "draw-shape", "shape": "rect", "id": id, "hit": hit, "anchor": "top-left",
                "position": {"x": 10, "y": 20}, "width": 64, "height": 32})
        };
        let (_, gathered) = from_props_mapped(
            &free_props(json!([hit_area("area", true), hit_area("plain", false)])),
            0.0,
        )
        .unwrap();
        let ids: Vec<&str> = gathered.regions.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["area"]);
        assert_eq!(gathered.regions[0].rect.w, 64.0);
        assert_eq!(gathered.regions[0].rect.h, 32.0);
    }

    #[test]
    fn a_region_per_hit_drawable_with_the_right_rect_and_topmost_last() {
        let (packet, gathered) = from_props_mapped(
            &free_props(json!([
                rect_shape(json!({"id": "a", "hit": true, "corners": {"radius": [2, 2, 2, 2]}})),
                rect_shape(json!({"id": "b", "hit": true, "position": {"x": 30, "y": 30}})),
                rect_shape(json!({"id": "c", "hit": false})),
                rect_shape(json!({"hit": true})),
            ])),
            0.0,
        )
        .unwrap();
        assert_eq!(packet.draw_commands.len(), 5, "clear and four shapes");
        let ids: Vec<&str> = gathered.regions.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids,
            ["a", "b"],
            "no region without hit and id; later is topmost"
        );
        assert_eq!(
            gathered.regions[0].rect,
            Rect {
                x: 10.0,
                y: 20.0,
                w: 64.0,
                h: 32.0
            }
        );
        assert_eq!(
            gathered.regions[1].rect,
            Rect {
                x: 30.0,
                y: 30.0,
                w: 64.0,
                h: 32.0
            }
        );
    }

    #[test]
    fn a_measured_text_reports_its_size_and_control_without_measure_reports_none() {
        let text = |measure: bool| {
            json!({"type": "draw-text", "id": "t", "text": "hello", "color": "#000000",
                "font": "12px sans-serif", "position": {"x": 5, "y": 5}, "measure": measure})
        };
        let (_, gathered) = from_props_mapped(&free_props(json!([text(true)])), 0.0).unwrap();
        match gathered.measured.as_slice() {
            [m] => {
                assert_eq!(m.id, "t");
                assert_eq!(m.lines, 1);
                assert!((m.width - crate::vector::text_width("hello", 12.0)).abs() < 1e-3);
                assert!((m.height - 12.0 * 1.3).abs() < 1e-3);
            }
            other => panic!("one entry: {other:?}"),
        }
        let (_, none) = from_props_mapped(&free_props(json!([text(false)])), 0.0).unwrap();
        assert!(none.measured.is_empty());
    }

    #[test]
    fn a_paragraph_wraps_into_lines_within_its_width_and_control_without_width_is_one_line() {
        let width = crate::vector::text_width("alpha beta", 12.0) + 0.5;
        let node = |extra: Value| {
            let mut n = json!({"type": "draw-text", "id": "p", "text": "alpha beta go on",
                "color": "#000000", "font": "12px sans-serif", "position": {"x": 5, "y": 5},
                "measure": true});
            for (key, value) in extra.as_object().unwrap() {
                n[key] = value.clone();
            }
            n
        };
        let (packet, gathered) =
            from_props_mapped(&free_props(json!([node(json!({"width": width}))])), 0.0).unwrap();
        let lines: Vec<&str> = packet
            .draw_commands
            .iter()
            .filter_map(|c| match c {
                DrawCommand::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(lines, ["alpha beta", "go on"]);
        assert_eq!(gathered.measured[0].lines, 2);
        assert!(gathered.measured[0].width <= width);
        let (single, _) = from_props_mapped(&free_props(json!([node(json!({}))])), 0.0).unwrap();
        assert_eq!(
            single
                .draw_commands
                .iter()
                .filter(|c| matches!(c, DrawCommand::Text { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn layout_wraps_within_width_truncates_with_an_ellipsis_and_overflows_without_truncate() {
        let text = "alpha beta go on";
        let width = crate::vector::text_width("alpha beta", 12.0) + 0.5;
        let wrapped = layout_paragraph(
            text,
            12.0,
            false,
            crate::packet::TextFamily::Sans,
            ParagraphBox {
                width: width,
                wrap: true,
                max_lines: None,
                truncate: false,
            },
        );
        assert_eq!(wrapped.lines, ["alpha beta", "go on"]);
        assert!(wrapped.widths.iter().all(|w| *w <= width));
        let flat = layout_paragraph(
            text,
            12.0,
            false,
            crate::packet::TextFamily::Sans,
            ParagraphBox {
                width: width,
                wrap: false,
                max_lines: None,
                truncate: false,
            },
        );
        assert_eq!(flat.lines, [text], "control: wrap off keeps one line");

        let cut = layout_paragraph(
            text,
            12.0,
            false,
            crate::packet::TextFamily::Sans,
            ParagraphBox {
                width: width,
                wrap: true,
                max_lines: Some(1),
                truncate: true,
            },
        );
        assert_eq!(cut.lines.len(), 1);
        assert!(cut.lines[0].ends_with('\u{2026}'), "{:?}", cut.lines);
        assert!(cut.widths[0] <= width);
        let capped = layout_paragraph(
            text,
            12.0,
            false,
            crate::packet::TextFamily::Sans,
            ParagraphBox {
                width: width,
                wrap: true,
                max_lines: Some(1),
                truncate: false,
            },
        );
        assert_eq!(
            capped.lines,
            ["alpha beta"],
            "control: no truncate, no ellipsis"
        );

        let narrow = crate::vector::text_width("alpha", 12.0) * 0.6;
        let one_line = layout_paragraph(
            "alphabet",
            12.0,
            false,
            crate::packet::TextFamily::Sans,
            ParagraphBox {
                width: narrow,
                wrap: true,
                max_lines: None,
                truncate: true,
            },
        );
        assert!(one_line.lines[0].ends_with('\u{2026}') && one_line.widths[0] <= narrow);
        let overflow = layout_paragraph(
            "alphabet",
            12.0,
            false,
            crate::packet::TextFamily::Sans,
            ParagraphBox {
                width: narrow,
                wrap: true,
                max_lines: None,
                truncate: false,
            },
        );
        assert_eq!(
            overflow.lines,
            ["alphabet"],
            "control: overflows without truncate"
        );

        let hard = layout_paragraph(
            "a\nb",
            12.0,
            false,
            crate::packet::TextFamily::Sans,
            ParagraphBox {
                width: 1000.0,
                wrap: true,
                max_lines: None,
                truncate: false,
            },
        );
        assert_eq!(hard.lines, ["a", "b"]);
    }

    fn animated_text_props(animation: Value) -> Value {
        free_props(
            json!([{"type": "draw-text", "text": "hi", "color": "#000000",
            "font": "12px sans-serif", "position": {"x": 5, "y": 5}, "animation": animation}]),
        )
    }

    fn text_alpha_and_x(packet: &GameRenderPacket) -> (f32, f32) {
        packet
            .draw_commands
            .iter()
            .find_map(|c| match c {
                DrawCommand::Text { color, pos, .. } => Some((color[3], pos.x)),
                _ => None,
            })
            .expect("a text")
    }

    #[test]
    fn a_text_opacity_animation_paints_at_half_opacity_halfway() {
        let props = animated_text_props(json!({"durationMs": 1000, "keyframes": [
            {"at": 0, "opacity": 0, "offsetX": 0}, {"at": 1, "opacity": 1, "offsetX": 10}]}));
        let (alpha, x) = text_alpha_and_x(&from_props_at(&props, 500.0).unwrap());
        let (rest_alpha, rest_x) = text_alpha_and_x(&from_props_at(&props, 0.0).unwrap());
        assert!((alpha - 0.5).abs() < 1e-3, "{alpha}");
        assert!((x - rest_x - 5.0).abs() < 1e-3, "offsetX is halfway");
        assert_eq!(rest_alpha, 1.0, "control: no clock, no animation");
        assert!(has_active_fx(&props), "the canvas asks for frames");
    }

    #[test]
    fn a_non_looping_text_animation_holds_its_final_values_and_a_plain_text_is_unchanged() {
        let props = animated_text_props(json!({"durationMs": 1000, "loop": false, "keyframes": [
            {"at": 0, "opacity": 0}, {"at": 1, "opacity": 0.25}]}));
        let (held, _) = text_alpha_and_x(&from_props_at(&props, 5000.0).unwrap());
        assert!((held - 0.25).abs() < 1e-3, "{held}");
        let looping = animated_text_props(json!({"durationMs": 1000, "keyframes": [
            {"at": 0, "opacity": 0}, {"at": 1, "opacity": 0.25}]}));
        let (wrapped, _) = text_alpha_and_x(&from_props_at(&looping, 5500.0).unwrap());
        assert!(
            (wrapped - 0.125).abs() < 1e-3,
            "control: a looping one wraps: {wrapped}"
        );
        let plain = free_props(
            json!([{"type": "draw-text", "text": "hi", "color": "#000000",
            "font": "12px sans-serif", "position": {"x": 5, "y": 5}}]),
        );
        let (alpha, _) = text_alpha_and_x(&from_props_at(&plain, 500.0).unwrap());
        assert_eq!(alpha, 1.0);
        assert!(
            !has_active_fx(&plain),
            "control: a plain text needs no frames"
        );
    }

    fn clipped_group(clip: Value) -> Value {
        let mut group = json!({"type": "draw-group", "position": {"x": 2, "y": 3}, "items": [
            {"type": "draw-shape", "shape": "rect", "id": "inside", "hit": true, "anchor": "top-left",
             "position": {"x": 0, "y": 0}, "width": 30, "height": 10, "fill": "#ff0000", "corners": {"radius": [2, 2, 2, 2]}},
            {"type": "draw-shape", "shape": "rect", "id": "edge", "hit": true, "anchor": "top-left",
             "position": {"x": 40, "y": 0}, "width": 30, "height": 10, "fill": "#ff0000", "corners": {"radius": [2, 2, 2, 2]}},
            {"type": "draw-shape", "shape": "rect", "id": "outside", "hit": true, "anchor": "top-left",
             "position": {"x": 0, "y": 40}, "width": 30, "height": 10, "fill": "#ff0000", "corners": {"radius": [2, 2, 2, 2]}},
            {"type": "draw-text", "text": "hello", "position": {"x": 0, "y": 5}, "color": "#ffffff",
             "font": "12px sans-serif", "align": "left", "baseline": "middle"}]});
        if !clip.is_null() {
            group["clip"] = clip;
        }
        free_props(json!([group]))
    }

    #[test]
    fn a_rect_clip_scopes_sdf_shapes_and_text_and_cuts_the_regions() {
        let (packet, gathered) = from_props_mapped(
            &clipped_group(json!({"x": 0, "y": 0, "width": 50, "height": 20})),
            0.0,
        )
        .unwrap();
        let Some((clip, commands)) = packet.draw_commands.iter().find_map(|c| match c {
            DrawCommand::ClipGroup { clip, commands } => Some((clip, commands)),
            _ => None,
        }) else {
            panic!("a scoped clip group: {:?}", packet.draw_commands)
        };
        assert!(commands.iter().any(|c| matches!(c, DrawCommand::Text { .. })), "text rides the clip");
        let sdf = commands.iter().find_map(|c| match c {
            DrawCommand::Sdf { rect, .. } => Some(*rect),
            _ => None,
        });
        let sdf = sdf.expect("an SDF shape rides the clip, not refused by the group");
        let bounds = segments_bounds(clip).expect("clip box");
        assert_eq!((bounds.x, bounds.y), (sdf.x, sdf.y), "the clip is anchored at the group origin");
        assert!((bounds.w - sdf.w * 50.0 / 30.0).abs() < 1e-3 && (bounds.h - sdf.h * 2.0).abs() < 1e-3, "{bounds:?} {sdf:?}");
        let ids: Vec<&str> = gathered.regions.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["inside", "edge"], "a region wholly outside the clip is dropped");
        let edge = gathered.regions.iter().find(|r| r.id == "edge").unwrap().rect;
        assert!((edge.x + edge.w - (bounds.x + bounds.w)).abs() < 1e-3 && edge.w < sdf.w, "the straddling region is cut at the edge: {edge:?}");
        let inside = gathered.regions.iter().find(|r| r.id == "inside").unwrap().rect;
        assert_eq!(inside, sdf, "control: a region inside the clip is untouched");
    }

    #[test]
    fn control_a_group_without_a_clip_keeps_every_region_and_no_clip_group() {
        let (packet, gathered) = from_props_mapped(&clipped_group(Value::Null), 0.0).unwrap();
        assert!(!packet.draw_commands.iter().any(|c| matches!(c, DrawCommand::ClipGroup { .. })));
        assert_eq!(gathered.regions.len(), 3);
        assert!(packet.draw_commands.iter().filter(|c| matches!(c, DrawCommand::Sdf { .. })).count() == 3);
    }

    #[test]
    fn a_rounded_rect_clip_has_curved_corners_and_a_square_one_has_none() {
        let curves = |clip: Value| {
            let packet = packet_for(clipped_group(clip));
            packet.draw_commands.iter().find_map(|c| match c {
                DrawCommand::ClipGroup { clip, .. } => Some(
                    clip.iter()
                        .filter(|s| matches!(s, PathSegment::CubicTo { .. }))
                        .count(),
                ),
                _ => None,
            })
        };
        assert_eq!(curves(json!({"x": 0, "y": 0, "width": 50, "height": 20, "radius": 4})), Some(4));
        assert_eq!(curves(json!({"x": 0, "y": 0, "width": 50, "height": 20, "radius": [4, 0, 0, 4]})), Some(2));
        assert_eq!(curves(json!({"x": 0, "y": 0, "width": 50, "height": 20})), Some(0), "control: no radius, no curve");
    }

    #[test]
    fn a_malformed_rect_clip_is_a_scene_error_naming_clip() {
        for clip in [
            json!({"x": 0, "y": 0, "height": 20}),
            json!({"x": 0, "y": 0, "width": 0, "height": 20}),
            json!({"x": 0, "y": 0, "width": 50, "height": 20, "radius": [1, 2, 3]}),
            json!({"x": 0, "y": 0, "width": 50, "height": 20, "radius": -2}),
        ] {
            let error = from_props(&clipped_group(clip.clone())).unwrap_err();
            assert!(matches!(&error, CanvasSceneError::InvalidProp { field, .. } if field == "clip"), "{clip}: {error:?}");
        }
    }

    fn union_group(extra: Value, second: Value) -> Value {
        let mut group = json!({"type": "draw-group", "position": {"x": 0, "y": 0}, "smoothUnion": 8, "items": [
            {"type": "draw-shape", "shape": "capsule", "id": "a", "hit": true, "position": {"x": 10, "y": 20},
             "width": 24, "height": 24, "fill": "#ff0000"},
            {"type": "draw-shape", "shape": "capsule", "id": "b", "hit": true, "position": {"x": 40, "y": 20},
             "width": 24, "height": 24, "fill": "#0000ff"}]});
        for (key, value) in extra.as_object().unwrap() {
            group[key] = value.clone();
        }
        for (key, value) in second.as_object().unwrap() {
            group["items"][1][key] = value.clone();
        }
        free_props(json!([group]))
    }

    #[test]
    fn a_smooth_union_group_paints_its_two_shapes_as_one_blend_and_keeps_both_regions() {
        let (packet, gathered) = from_props_mapped(&union_group(json!({}), json!({})), 0.0).unwrap();
        let blends: Vec<&DrawCommand> = packet.draw_commands.iter().filter(|c| matches!(c, DrawCommand::SdfBlend { .. })).collect();
        let [DrawCommand::SdfBlend { first, second, radius }] = blends.as_slice() else {
            panic!("one blend: {:?}", packet.draw_commands)
        };
        assert_eq!(*radius, 8.0);
        assert!(matches!(first.as_ref(), DrawCommand::Sdf { fill, .. } if fill[0] == 1.0));
        assert!(matches!(second.as_ref(), DrawCommand::Sdf { fill, .. } if fill[2] == 1.0));
        assert!(sdf_commands(&packet).is_empty(), "the two shapes are not also painted alone");
        let ids: Vec<&str> = gathered.regions.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b"], "each shape keeps its own pointer region");
    }

    #[test]
    fn control_a_group_without_smooth_union_paints_two_separate_shapes() {
        let mut props = union_group(json!({}), json!({}));
        props["drawables"][0].as_object_mut().unwrap().remove("smoothUnion");
        let packet = packet_for(props);
        assert!(!packet.draw_commands.iter().any(|c| matches!(c, DrawCommand::SdfBlend { .. })));
        assert_eq!(sdf_commands(&packet).len(), 2);
    }

    #[test]
    fn a_smooth_union_that_cannot_blend_is_a_scene_error_naming_smooth_union() {
        let bad = [
            union_group(json!({"smoothUnion": 0}), json!({})),
            union_group(json!({"smoothUnion": "wide"}), json!({})),
            union_group(json!({}), json!({"stroke": "#ffffff"})),
            union_group(json!({}), json!({"shadow": {"color": "#000000", "blur": 4}})),
            union_group(json!({}), json!({"opacity": 0.5})),
            union_group(json!({}), json!({"material": {"wgsl": "fn shade(px: Pixel) -> vec4<f32> { return px.color; }", "params": [], "live": false}})),
        ];
        for props in bad {
            let error = from_props(&props).unwrap_err();
            assert!(matches!(&error, CanvasSceneError::InvalidProp { field, .. } if field == "smoothUnion"), "{error:?}");
        }
        let mut three = union_group(json!({}), json!({}));
        let extra = three["drawables"][0]["items"][0].clone();
        three["drawables"][0]["items"].as_array_mut().unwrap().push(extra);
        assert!(matches!(from_props(&three).unwrap_err(), CanvasSceneError::InvalidProp { field, .. } if field == "smoothUnion"), "three children");
        let mut dashed = union_group(json!({}), json!({}));
        dashed["drawables"][0]["items"][0]["stroke"] = json!("#ffffff");
        dashed["drawables"][0]["items"][0]["strokeDash"] = json!([4, 2]);
        assert!(matches!(from_props(&dashed).unwrap_err(), CanvasSceneError::InvalidProp { field, .. } if field == "smoothUnion"), "dashed stroke");
        let mut solid = union_group(json!({}), json!({}));
        solid["drawables"][0]["items"][0]["stroke"] = json!("#ffffff");
        assert!(from_props(&solid).is_ok(), "control: a solid stroke on the first shape blends");
    }
}
