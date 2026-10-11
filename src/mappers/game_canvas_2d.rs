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
//! strokes, draw-text (glyph pipeline), camera-tiled whole-image backgrounds
//! (cover-scale used instead). Unknown drawable types skip silently — React
//! parity (`paintDispatch`).

use crate::asset;
use crate::atlas;
use crate::packet::{DrawCommand, GameRenderPacket, Rect, Vec2};
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
pub fn from_props(props: &Value) -> GameRenderPacket {
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
    match props.get("backgroundImage") {
        Some(bg) if !bg.is_null() => {
            if let Some(cmd) = background_sprite(bg, width, height) {
                draw_commands.push(cmd);
            }
        }
        _ => {
            let color = props
                .get("bgColor")
                .and_then(Value::as_str)
                .and_then(parse_css_color)
                .unwrap_or(BACKGROUND_FALLBACK);
            draw_commands.push(DrawCommand::Clear { color });
        }
    }

    for node in &drawables {
        walk(node, &projector, &to_screen, zoom, &mut draw_commands);
    }

    GameRenderPacket {
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
    }
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
    canvas_node_props(node).map(|props| from_props(&Value::Object(props)))
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
    let mut props = canvas_node_props(node)?;
    props.insert("width".into(), Value::from(width.max(1.0)));
    props.insert("height".into(), Value::from(height.max(1.0)));
    Ok(from_props(&Value::Object(props)))
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
fn to_drawable_nodes(children: &Value) -> Vec<Value> {
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
    out: &mut Vec<DrawCommand>,
) {
    match node.get("type").and_then(Value::as_str) {
        Some("draw-sprite") => paint_sprite(node, projector, to_screen, zoom, out),
        Some("draw-shape") => paint_shape(node, projector, to_screen, zoom, out),
        Some("draw-text") => gap_once("draw-text", "glyph pipeline (glyphon) deferred"),
        Some("draw-sprite-layer") => {
            for item in items(node) {
                paint_sprite(item, projector, to_screen, zoom, out);
            }
        }
        Some("draw-shape-layer") => {
            for item in items(node) {
                paint_shape(item, projector, to_screen, zoom, out);
            }
        }
        Some("draw-text-layer") => gap_once("draw-text", "glyph pipeline (glyphon) deferred"),
        Some("draw-fx-layer") => gap_once(
            "draw-fx-layer",
            "fx expander deferred — native painter pending (docs/Almadar_Compiler_Gaps.md C-GAME-FX-LAYER)",
        ),
        _ => {} // unknown types skipped silently — React parity
    }
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
) -> Vec<DrawCommand> {
    let mut out = Vec::new();
    for node in drawables
        .iter()
        .map(|n| resolve_drawable_colors(n, resolve))
    {
        walk(&node, projector, to_screen, zoom, &mut out);
    }
    out
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
    match node.get("type").and_then(Value::as_str) {
        Some("draw-sprite") | Some("draw-shape") | Some("draw-text") => {
            if let Some(p) = node.get("position").and_then(parse_scene_pos) {
                sink(p);
            }
        }
        Some("draw-sprite-layer") | Some("draw-shape-layer") | Some("draw-text-layer") => {
            for item in items(node) {
                if let Some(p) = item.get("position").and_then(parse_scene_pos) {
                    sink(p);
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
    out: &mut Vec<DrawCommand>,
) {
    let Some(position) = node.get("position").and_then(parse_scene_pos) else {
        gap_once("sprite-position", "draw-sprite without position");
        return;
    };
    let asset = node.get("asset");
    let url = asset
        .and_then(|a| a.get("url"))
        .and_then(Value::as_str)
        .or_else(|| asset.and_then(Value::as_str));
    let Some(url) = url else {
        gap_once("sprite-asset", "draw-sprite without a resolvable asset.url");
        return;
    };

    // Source rect resolution order (DrawSprite.tsx 49-59): explicit frame →
    // atlas sub-rect → whole image. Unresolved = resource in-flight → skip.
    let src = if let Some(frame) = node.get("frame").and_then(parse_xywh) {
        frame
    } else if let (Some(atlas_url), Some(sprite)) = (
        asset.and_then(|a| a.get("atlas")).and_then(Value::as_str),
        asset.and_then(|a| a.get("sprite")).and_then(Value::as_str),
    ) {
        asset::ensure_asset(atlas_url);
        let Some(r) = atlas::resolve_atlas_rect(atlas_url, sprite) else {
            return;
        };
        r
    } else {
        let Some((w, h)) = asset::image_info(url) else {
            return;
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
    let tw = projector.tile_width;
    let free = projector.layout == Projection::Free;
    let w = node
        .get("width")
        .and_then(Value::as_f64)
        .map(|v| v as f32 * tw)
        .unwrap_or(if free { src.w } else { tw });
    let h = node
        .get("height")
        .and_then(Value::as_f64)
        .map(|v| v as f32 * tw)
        .unwrap_or(if free { src.h } else { tw });

    let anchor = node
        .get("anchor")
        .and_then(Value::as_str)
        .and_then(Anchor::from_str)
        .unwrap_or(Anchor::TopLeft);
    let (px, py) = projector.anchor_point(position, anchor);
    let dx = if anchor == Anchor::TopLeft {
        px
    } else {
        px - w / 2.0
    };
    let dy = match anchor {
        Anchor::Ground => py - h,
        Anchor::Center => py - h / 2.0,
        Anchor::TopLeft => py,
    };

    let (sx, sy) = to_screen(dx, dy);
    out.push(DrawCommand::Sprite {
        texture: url.to_string(),
        src,
        dst: Rect {
            x: sx,
            y: sy,
            w: w * zoom,
            h: h * zoom,
        },
        rotation: get_f32(node, "rotation", 0.0),
        opacity: get_f32(node, "opacity", 1.0),
        flip_x: node.get("flipX").and_then(Value::as_bool).unwrap_or(false),
        flip_y: false,
    });
}

/// `DrawShape.tsx paintShape` (fills; strokes are a logged gap).
fn paint_shape(
    node: &Value,
    projector: &Projector,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
    zoom: f32,
    out: &mut Vec<DrawCommand>,
) {
    let Some(shape) = node.get("shape").and_then(Value::as_str) else {
        gap_once("shape-kind", "draw-shape without shape");
        return;
    };
    let Some(position) = node.get("position").and_then(parse_scene_pos) else {
        gap_once("shape-position", "draw-shape without position");
        return;
    };
    if node.get("stroke").is_some() {
        gap_once("shape-stroke", "strokes deferred — fill-only pass");
    }
    let opacity = get_f32(node, "opacity", 1.0);
    let Some(mut color) = node
        .get("fill")
        .and_then(Value::as_str)
        .and_then(parse_css_color)
    else {
        return; // fill-only pass: no fill → nothing to emit
    };
    color[3] *= opacity;

    let tw = projector.tile_width;
    match shape {
        "cell" => {
            let pts = projector.cell_path(position);
            out.push(DrawCommand::Poly {
                points: pts
                    .iter()
                    .map(|&(x, y)| {
                        let (sx, sy) = to_screen(x, y);
                        Vec2 { x: sx, y: sy }
                    })
                    .collect(),
                color,
            });
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
            out.push(DrawCommand::Quad {
                rect: Rect {
                    x: sx,
                    y: sy,
                    w: w * zoom,
                    h: h * zoom,
                },
                color,
            });
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
            out.push(DrawCommand::Poly { points, color });
        }
        "poly" => {
            let (bx, by) = projector.project(position);
            let points: Vec<Vec2> = node
                .get("points")
                .and_then(Value::as_array)
                .map(|pts| {
                    pts.iter()
                        .filter_map(|pt| {
                            let x = pt.get("x").and_then(Value::as_f64)? as f32;
                            let y = pt.get("y").and_then(Value::as_f64)? as f32;
                            let (sx, sy) = to_screen(bx + x * tw, by + y * tw);
                            Some(Vec2 { x: sx, y: sy })
                        })
                        .collect()
                })
                .unwrap_or_default();
            out.push(DrawCommand::Poly { points, color });
        }
        _ => gap_once("shape-kind-unknown", "unknown draw-shape kind"),
    }
}

/// Background image → cover-scaled sprite (Canvas2D 362-368). Whole-image
/// (non-atlas) backgrounds are camera-TILED in React; that is deferred and
/// cover-scale is used instead (logged once).
fn background_sprite(bg: &Value, width: f32, height: f32) -> Option<DrawCommand> {
    let url = match bg {
        Value::String(s) => s.as_str(),
        Value::Object(_) => bg.get("url").and_then(Value::as_str)?,
        _ => return None,
    };
    let src = if let (Some(atlas_url), Some(sprite)) = (
        bg.get("atlas").and_then(Value::as_str),
        bg.get("sprite").and_then(Value::as_str),
    ) {
        asset::ensure_asset(atlas_url);
        atlas::resolve_atlas_rect(atlas_url, sprite)?
    } else {
        gap_once(
            "tiled-background",
            "camera-tiled whole-image background deferred — cover-scaled",
        );
        let (w, h) = asset::image_info(url)?;
        Rect {
            x: 0.0,
            y: 0.0,
            w: w as f32,
            h: h as f32,
        }
    };
    let k = (width / src.w).max(height / src.h);
    let dw = src.w * k;
    let dh = src.h * k;
    Some(DrawCommand::Sprite {
        texture: url.to_string(),
        src,
        dst: Rect {
            x: (width - dw) / 2.0,
            y: (height - dh) / 2.0,
            w: dw,
            h: dh,
        },
        rotation: 0.0,
        opacity: 1.0,
        flip_x: false,
        flip_y: false,
    })
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
    let inner = s
        .strip_prefix("rgba(")
        .or_else(|| s.strip_prefix("rgb("))
        .and_then(|r| r.strip_suffix(')'))?;
    let parts: Vec<f32> = inner
        .split(',')
        .map(|p| p.trim().parse::<f32>())
        .collect::<Result<_, _>>()
        .ok()?;
    match parts.as_slice() {
        [r, g, b] => Some([r / 255.0, g / 255.0, b / 255.0, 1.0]),
        [r, g, b, a] => Some([r / 255.0, g / 255.0, b / 255.0, *a]),
        _ => None,
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
        from_props(&props)
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
        assert!(parse_css_color("rebeccapurple").is_none());
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
        }));
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
            "asset": { "url": "t://sprite.svg" },
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
