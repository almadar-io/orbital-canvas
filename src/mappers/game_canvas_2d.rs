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

use crate::asset;
use crate::atlas;
use crate::mappers::learning::shape::parse_path_data;
use crate::mappers::learning::Xf;
use crate::packet::{
    DrawCommand, FillPattern, GameRenderPacket, Gradient, GradientStop, LineCap, LineJoin,
    PathSegment, Rect, Stroke, TextAlign, Vec2,
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
    let width = get_f32(props, "width", 800.0);
    let height = get_f32(props, "height", 600.0);
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
            &mut draw_commands,
        )?;
    }

    Ok(GameRenderPacket {
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
    })
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
    canvas_node_props(node).and_then(|props| from_props_at(&Value::Object(props), epoch_now_ms))
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
    let mut props = canvas_node_props(node)?;
    props.insert("width".into(), Value::from(width.max(1.0)));
    props.insert("height".into(), Value::from(height.max(1.0)));
    from_props_at(&Value::Object(props), epoch_now_ms)
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
fn walk(
    node: &Value,
    projector: &Projector,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
    zoom: f32,
    epoch_now_ms: f64,
    out: &mut Vec<DrawCommand>,
) -> Result<(), CanvasSceneError> {
    match node.get("type").and_then(Value::as_str) {
        Some("draw-sprite") => paint_sprite(node, projector, to_screen, zoom, epoch_now_ms, out)?,
        Some("draw-shape") => paint_shape(node, projector, to_screen, zoom, epoch_now_ms, out),
        Some("draw-text") => paint_text(node, projector, to_screen, zoom, out),
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
                paint_sprite(item, projector, to_screen, zoom, epoch_now_ms, out)?;
            }
        }
        Some("draw-shape-layer") => {
            for item in items(node) {
                paint_shape(item, projector, to_screen, zoom, epoch_now_ms, out);
            }
        }
        Some("draw-text-layer") => {
            for item in items(node) {
                paint_text(item, projector, to_screen, zoom, out);
            }
        }
        Some("draw-group") => paint_group(node, projector, to_screen, zoom, epoch_now_ms, out)?,
        Some("draw-fx-layer") => {
            for drawable in crate::fx_projection::expand_layer(
                node,
                epoch_now_ms,
                "2d",
                f64::from(projector.tile_width),
            )? {
                walk(&drawable, projector, to_screen, zoom, epoch_now_ms, out)?;
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
    for node in drawables
        .iter()
        .map(|n| resolve_drawable_colors(n, resolve))
    {
        walk(&node, projector, to_screen, zoom, 0.0, &mut out)?;
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
        Some("draw-shape" | "draw-group")
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
    for child in items(node) {
        walk(child, projector, to_screen, zoom, epoch_now_ms, out)?;
    }
    if radians != 0.0
        && out[start..]
            .iter()
            .any(|cmd| matches!(cmd, DrawCommand::Text { .. }))
    {
        out.truncate(start);
        gap_once(
            "group-rotated-text",
            "draw-group rotating text needs a vector glyph path",
        );
        return Ok(());
    }
    if let Some(clip) = node
        .get("clip")
        .and_then(Value::as_str)
        .filter(|clip| !clip.is_empty())
    {
        let Some(segments) = parse_path_data(
            clip,
            Xf {
                s: projector.tile_width * zoom,
                ox: zero_x,
                oy: zero_y,
            },
        ) else {
            out.truncate(start);
            gap_once("group-clip-path", "malformed draw-group SVG clip path");
            return Ok(());
        };
        let commands = out.drain(start..).collect();
        out.push(DrawCommand::ClipGroup {
            clip: segments,
            commands,
        });
    }
    crate::animate::transform_group_commands(
        &mut out[start..],
        Vec2 {
            x: zero_x,
            y: zero_y,
        },
        Vec2 {
            x: group_x - zero_x,
            y: group_y - zero_y,
        },
        get_f32(node, "scale", 1.0) * animated_number("scale", 1.0),
        radians,
        get_f32(node, "opacity", 1.0) * animated_number("opacity", 1.0),
    );
    Ok(())
}

/// The canvas's 2D text painter maps through the same vector font pipeline as
/// other Canvas labels. Unsupported CSS fonts/baselines are reported rather
/// than silently given a different face or anchor.
fn paint_text(
    node: &Value,
    projector: &Projector,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
    zoom: f32,
    out: &mut Vec<DrawCommand>,
) {
    let Some(position) = node.get("position").and_then(parse_scene_pos) else {
        gap_once("text-position", "draw-text without position");
        return;
    };
    let Some(text) = node.get("text").and_then(Value::as_str) else {
        gap_once("text-value", "draw-text without text");
        return;
    };
    let Some(mut color) = node
        .get("color")
        .and_then(Value::as_str)
        .and_then(parse_css_color)
    else {
        gap_once("text-color", "draw-text color is not a supported CSS color");
        return;
    };
    let (size, weight, mono) = match parse_text_font(node.get("font").and_then(Value::as_str)) {
        Some(font) => font,
        None => {
            gap_once(
                "text-font",
                "draw-text CSS font is not carried by the native font pack",
            );
            return;
        }
    };
    let align = match node.get("align").and_then(Value::as_str).unwrap_or("left") {
        "left" | "start" => TextAlign::Left,
        "center" => TextAlign::Center,
        "right" | "end" => TextAlign::Right,
        _ => {
            gap_once("text-align", "draw-text alignment is not supported");
            return;
        }
    };
    let anchor = node
        .get("anchor")
        .and_then(Value::as_str)
        .and_then(Anchor::from_str)
        .unwrap_or(Anchor::Center);
    let (x, y) = projector.anchor_point(position, anchor);
    let x = x + get_f32(node, "offsetX", 0.0);
    let y = y + get_f32(node, "offsetY", 0.0);
    let (sx, sy) = to_screen(x, y);
    let size = size * zoom;
    let Some((asc, desc)) = crate::vector::text_font_metrics(size) else {
        gap_once("text-face", "native text face is unavailable");
        return;
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
            return;
        }
    };
    color[3] *= get_f32(node, "opacity", 1.0);
    out.push(DrawCommand::Text {
        text: text.to_owned(),
        pos: Vec2 { x: sx, y: sy },
        size,
        color,
        align,
        weight,
        halo: None,
        mono,
    });
}

fn parse_text_font(font: Option<&str>) -> Option<(f32, u16, bool)> {
    let Some(font) = font else {
        return Some((10.0, 400, false));
    };
    let mut tokens = font.split_whitespace();
    let first = tokens.next()?;
    let (weight, size) = if first == "bold" {
        (700, tokens.next()?)
    } else {
        (400, first)
    };
    let size = size.strip_suffix("px")?.parse::<f32>().ok()?;
    if !size.is_finite() || size <= 0.0 {
        return None;
    }
    let family = tokens.collect::<Vec<_>>().join(" ");
    match family.as_str() {
        "sans-serif" | "system-ui" | "system-ui, sans-serif" => Some((size, weight, false)),
        "monospace" => Some((size, weight, true)),
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

/// Project one world shape through the shared Canvas vector commands.
fn paint_shape(
    node: &Value,
    projector: &Projector,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
    zoom: f32,
    epoch_now_ms: f64,
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
            let color = shadow.get("color").and_then(Value::as_str).and_then(parse_css_color);
            let blur = shadow.get("blur").and_then(Value::as_f64);
            match (color, blur) {
                (Some(color), Some(blur)) if blur.is_finite() && blur >= 0.0 => {
                    (blur > 0.0 && color[3] > 0.0).then_some((color, blur as f32))
                }
                _ => {
                    gap_once("shape-shadow", "draw-shape shadow needs a css `color` and a finite `blur` of zero or more");
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
    if fill.is_none() && stroke.is_none() && fill_gradient.is_none() && pattern.is_none() {
        return;
    }

    let start = out.len();
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
        out.insert(start, DrawCommand::Shadow { commands, color, sigma });
    }
    if additive && out.len() > start {
        let commands = out.drain(start..).collect();
        out.push(DrawCommand::AdditiveGroup { commands });
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
            "kind": "linear", "from": {"x": 0, "y": 0}, "to": {"x": 1, "y": 0},
            "stops": [{"offset": 0, "color": "#ff0000"}, {"offset": 1, "color": "#0000ff"}]
        });
        let p = packet_for(json!({
            "projection": "free", "scale": 1.0,
            "drawables": [
                {"type": "draw-shape", "shape": "rect", "position": {"x": 1, "y": 1}, "width": 1, "height": 1, "gradient": gradient},
                {"type": "draw-shape", "shape": "ellipse", "position": {"x": 2, "y": 1}, "radiusX": 0.5, "radiusY": 0.5, "gradient": gradient},
                {"type": "draw-shape", "shape": "poly", "position": {"x": 3, "y": 1}, "points": [{"x": 0, "y": 0}, {"x": 1, "y": 0}, {"x": 0, "y": 1}], "gradient": gradient},
                {"type": "draw-shape", "shape": "path", "position": {"x": 4, "y": 1}, "d": "M0 0 L1 0 L1 1 Z", "gradient": gradient}
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
                    "anchor": "center", "radiusX": 0.5, "fill": "#ffffff",
                    "shadow": {"color": "#00ff00", "blur": blur}}
            ]
        }))
    }

    #[test]
    fn a_shape_shadow_draws_its_coverage_blurred_under_the_shape() {
        let packet = shadowed_disc(0.25);
        let [DrawCommand::Clear { .. }, DrawCommand::Shadow { commands, color, sigma }, shape] =
            packet.draw_commands.as_slice()
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
        assert!(!matches!(packet.draw_commands[1], DrawCommand::Shadow { .. }));
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
                    mono,
                    ..
                } => Some((text.as_str(), *size, color[3], *align, *weight, *mono)),
                _ => None,
            })
            .collect();
        assert_eq!(texts.len(), 2);
        assert_eq!(texts[0], ("one", 14.0, 0.5, TextAlign::Center, 700, true));
        assert_eq!(texts[1], ("two", 10.0, 1.0, TextAlign::Left, 400, false));
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
                "position": {"x": 0, "y": 0}, "width": 1, "height": 1,
                "fill": "#ff0000", "rotate": std::f32::consts::FRAC_PI_2,
                "pivot": {"x": 0.5, "y": 0.5}}]
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
    fn draw_poly_offsets_are_world_units_like_path_offsets() {
        let p = packet_for(json!({
            "projection": "free", "scale": 1.0,
            "drawables": [{"type": "draw-shape", "shape": "poly",
                "position": {"x": 0, "y": 0},
                "points": [{"x": 0, "y": 0}, {"x": 1, "y": 0}, {"x": 0, "y": 1}],
                "offsetX": 0.25, "offsetY": -0.5, "fill": "#ff0000"}]
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
                "position": {"x": 0, "y": 0}, "height": 1,
                "width": 0, "fill": "#ff0000",
                "animation": {"durationMs": 1000, "loop": false,
                    "keyframes": [
                        {"at": 0, "width": 0, "fill": "#ff0000"},
                        {"at": 1, "width": 2, "fill": "#0000ff"}
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
                        {"at": 1, "offsetX": 1, "opacity": 0.5}
                    ]},
                "items": [{"type": "draw-shape", "shape": "rect",
                    "position": {"x": 0, "y": 0}, "width": 1, "height": 1,
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
}
