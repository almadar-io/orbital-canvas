//! The canvas props `Canvas3DHost` declares and three never draws, with the meaning each takes on every host. A prop
//! the 2D canvas draws keeps its 2D meaning (`showMinimap`, `backgroundImage`, `isLoading`); a prop neither 2D host
//! draws either (`unitScale`, `worldWidth`/`worldHeight`, `fogOfWar`, `showCoordinates`, `showTileInfo`) takes the
//! meaning fixed here, which the 2D hosts owe as a twin. The overlays are 2D packet commands painted after the
//! [`DrawCommand::Scene3D`] they cover.

use super::canvas_scene3d::SceneTheme;
use super::game_canvas_2d::{parse_css_color, CanvasSceneError};
use crate::packet::{
    DrawCommand, Halo, LineCap, LineJoin, PathSegment, Rect, Stroke, TextAlign, Vec2,
};
use crate::scene3d::model::{
    Flat, Interaction, Material, MaterialKind, MeshInstance, Scene3D, Side,
};
use crate::scene3d::render::view_projection;
use crate::scene3d::view::{js_round, EditMode};
use glam::{DMat4, DVec3, Mat4, Vec3, Vec4};
use serde_json::{json, Map, Value};

type Fields = Map<String, Value>;

/// A fogged cell's drawn content renders at this fraction of its brightness (linear), what a 75% black veil gives.
pub const FOG_SHADE: f32 = 0.25;
/// `Canvas3DHost`'s selection ring: a stroked `draw-shape` ellipse of radius 0.6, drawn by `Shape3D` as a
/// `ringGeometry(radius - 0.08, radius, 32)` lifted 0.02 off the ground.
const SELECTION_RADIUS: f32 = 0.6;
const RING_WIDTH: f32 = 0.08;
const RING_SEGMENTS: usize = 32;
const SHAPE_LIFT: f32 = 0.02;
/// `Canvas2D`'s minimap: a 150x100 `MiniMap` with a 1px border, 8px in from the bottom-right corner.
const MINIMAP_SIZE: (f32, f32) = (150.0, 100.0);
const MINIMAP_INSET: f32 = 8.0;
const MINIMAP_BORDER: f32 = 1.0;
/// `MiniMap`'s `#111` ground, `MINIMAP_TERRAIN_COLORS.default` (`#4ade80`) and default viewport rect.
const MINIMAP_GROUND: [f32; 4] = [17.0 / 255.0, 17.0 / 255.0, 17.0 / 255.0, 1.0];
const MINIMAP_TILE: [f32; 4] = [74.0 / 255.0, 222.0 / 255.0, 128.0 / 255.0, 1.0];
const MINIMAP_VIEWPORT: [f32; 4] = [20.0, 20.0, 40.0, 40.0];
/// The `border-border/20` class: the border color at 20%.
const MINIMAP_BORDER_ALPHA: f32 = 0.2;
const COORDINATE_SIZE: f32 = 10.0;
const TILE_INFO_SIZE: f32 = 12.0;
const TILE_INFO_OFFSET: f32 = 12.0;
const HALO_WIDTH: f32 = 2.0;
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// The dead props of one `canvas` node, validated.
#[derive(Debug, Clone, PartialEq)]
pub struct CanvasProps3D {
    /// Every top-level drawable that carries an `id` (a unit, as `unitClickEvent` names it) scales by this about its
    /// placement point. Default 1.
    pub unit_scale: f64,
    /// The board's scene extent along scene `x`: bounds become `0..worldWidth`, replacing the drawn bounds.
    pub world_width: Option<f64>,
    /// Likewise along scene `y` (world Z).
    pub world_height: Option<f64>,
    /// The 2D canvas's backdrop (a URL or an `Asset`), painted behind the scene.
    pub background_image: Option<Value>,
    pub fog_of_war: Option<FogOfWar>,
    pub show_minimap: bool,
    pub show_coordinates: bool,
    pub show_tile_info: bool,
    /// The host shows its loading state instead of the scene.
    pub loading: bool,
    /// `editable` with the controlled `selectedId`.
    pub edit: Option<EditMode>,
}

/// `fogOfWar[y][x]`: `true` fogs the scene cell (`x`, `y`); cells outside the grid are clear.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FogOfWar {
    rows: Vec<Vec<bool>>,
}

impl FogOfWar {
    /// Whether the cell a scene position rounds to (`Math.round`, as the hit index keys cells) is fogged.
    pub fn fogged(&self, x: f64, y: f64) -> bool {
        let (cx, cy) = (js_round(x as f32), js_round(y as f32));
        if cx < 0.0 || cy < 0.0 {
            return false;
        }
        self.rows
            .get(cy as usize)
            .and_then(|row| row.get(cx as usize))
            .copied()
            .unwrap_or(false)
    }
}

pub fn canvas_props_3d(node: &Value) -> Result<CanvasProps3D, CanvasSceneError> {
    let object = node.as_object().ok_or(CanvasSceneError::NotAnObject)?;
    let f = object
        .get("props")
        .and_then(Value::as_object)
        .unwrap_or(object);
    let editable = flag(f, "editable")?.unwrap_or(false);
    let selected = match present(f, "selectedId") {
        None => None,
        Some(v) => Some(
            v.as_str()
                .ok_or_else(|| bad("canvas.selectedId", "must be a drawable id string"))?
                .to_string(),
        ),
    };
    Ok(CanvasProps3D {
        unit_scale: positive(f, "unitScale")?.unwrap_or(1.0),
        world_width: positive(f, "worldWidth")?,
        world_height: positive(f, "worldHeight")?,
        background_image: present(f, "backgroundImage").map(backdrop).transpose()?,
        fog_of_war: present(f, "fogOfWar").map(fog).transpose()?.flatten(),
        show_minimap: flag(f, "showMinimap")?.unwrap_or(false),
        show_coordinates: flag(f, "showCoordinates")?.unwrap_or(false),
        show_tile_info: flag(f, "showTileInfo")?.unwrap_or(false),
        loading: flag(f, "isLoading")?.unwrap_or(false),
        edit: editable.then_some(EditMode { selected }),
    })
}

impl CanvasProps3D {
    /// The board bounds `[min_x, max_x, min_z, max_z]` once `worldWidth`/`worldHeight` replace the drawn ones.
    pub fn world_bounds(&self, drawn: [f64; 4]) -> [f64; 4] {
        let [min_x, max_x, min_z, max_z] = drawn;
        let (min_x, max_x) = self.world_width.map_or((min_x, max_x), |w| (0.0, w));
        let (min_z, max_z) = self.world_height.map_or((min_z, max_z), |h| (0.0, h));
        [min_x, max_x, min_z, max_z]
    }

    /// The parent transform of a top-level drawable placed at `anchor` (world): a unit scales about it.
    pub fn unit_transform(&self, has_id: bool, anchor: DVec3) -> DMat4 {
        if has_id && self.unit_scale != 1.0 {
            unit_scale_about(anchor, self.unit_scale)
        } else {
            DMat4::IDENTITY
        }
    }
}

pub fn unit_scale_about(anchor: DVec3, k: f64) -> DMat4 {
    DMat4::from_translation(anchor)
        * DMat4::from_scale(DVec3::splat(k))
        * DMat4::from_translation(-anchor)
}

/// Darken meshes a fogged cell's drawable produced.
pub fn shade_fogged(meshes: &mut [MeshInstance]) {
    for mesh in meshes {
        mesh.material.color = mesh.material.color.map(|c| c * FOG_SHADE);
        mesh.material.emissive = mesh.material.emissive.map(|c| c * FOG_SHADE);
    }
}

/// The 2D layer's first command under a backdropped scene: the 2D canvas paints `backgroundColor` (else the theme
/// ground, else nothing) before the image.
pub fn backdrop_clear(node: &Value, theme: &SceneTheme) -> Result<DrawCommand, CanvasSceneError> {
    let object = node.as_object().ok_or(CanvasSceneError::NotAnObject)?;
    let f = object
        .get("props")
        .and_then(Value::as_object)
        .unwrap_or(object);
    let color = match present(f, "backgroundColor") {
        Some(v) => css(v, "canvas.backgroundColor")?,
        None => match &theme.ground {
            Some(ground) => css(&Value::String(ground.clone()), "theme --surface-diagram")?,
            None => [0.0; 4],
        },
    };
    Ok(DrawCommand::Clear { color })
}

/// The edit-mode ring around the selected drawable at scene `position` (its drag preview while dragged), in the
/// theme's `highlight` tone (linear RGB).
pub fn selection_ring(
    interaction: &Interaction,
    position: [f64; 3],
    color: [f32; 3],
) -> Result<MeshInstance, CanvasSceneError> {
    let cell = interaction.cell;
    let world = Vec3::new(
        (position[0] as f32 - interaction.min_x) * cell,
        position[2] as f32 + SHAPE_LIFT,
        (position[1] as f32 - interaction.min_z) * cell,
    );
    let inner = SELECTION_RADIUS - RING_WIDTH;
    let mesh =
        crate::scene3d::tessellate::ring([inner, inner], [SELECTION_RADIUS; 2], RING_SEGMENTS)
            .map_err(|e| CanvasSceneError::Unsupported3DContract(format!("selection ring: {e}")))?;
    Ok(MeshInstance {
        mesh,
        transform: (Mat4::from_translation(world)
            * Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2))
        .to_cols_array(),
        material: Material {
            kind: MaterialKind::Flat(Flat::default()),
            color,
            side: Side::Double,
            ..Material::default()
        },
        cast_shadow: false,
        receive_shadow: false,
        id: None,
        glow: None,
    })
}

/// `withPreviewPosition`: the drawables with the one carrying `id` (top level, a layer item or nested in a group)
/// moved to scene (`x`, `y`).
pub fn with_preview_position(nodes: &[Value], id: &str, x: i64, y: i64) -> Vec<Value> {
    nodes.iter().map(|n| preview_node(n, id, x, y)).collect()
}

fn preview_node(node: &Value, id: &str, x: i64, y: i64) -> Value {
    let Some(f) = node.as_object() else {
        return node.clone();
    };
    let has_id = |v: &Value| v.get("id").and_then(Value::as_str) == Some(id);
    let items = f.get("items").and_then(Value::as_array);
    match f.get("type").and_then(Value::as_str) {
        Some("draw-sprite-layer" | "draw-shape-layer" | "draw-text-layer") => {
            let Some(items) = items.filter(|items| items.iter().any(has_id)) else {
                return node.clone();
            };
            let moved = items
                .iter()
                .map(|it| {
                    if has_id(it) {
                        placed(it, x, y)
                    } else {
                        it.clone()
                    }
                })
                .collect();
            let mut out = f.clone();
            out.insert("items".into(), Value::Array(moved));
            Value::Object(out)
        }
        Some("draw-group") => match items {
            Some(items) => {
                let mut out = f.clone();
                out.insert(
                    "items".into(),
                    Value::Array(with_preview_position(items, id, x, y)),
                );
                Value::Object(out)
            }
            None => node.clone(),
        },
        _ if has_id(node) && f.contains_key("position") => placed(node, x, y),
        _ => node.clone(),
    }
}

fn placed(node: &Value, x: i64, y: i64) -> Value {
    let mut out = node.clone();
    out["position"] = json!({ "x": x, "y": y });
    out
}

/// Overlay colors from the scene theme: labels in the `label` tone with a halo of the ground, the minimap border in
/// the `ink` tone at 20%; an absent tone is white and an absent ground black.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverlayColors {
    pub label: [f32; 4],
    pub halo: [f32; 4],
    pub border: [f32; 4],
}

impl OverlayColors {
    pub fn from_theme(theme: &SceneTheme) -> Result<Self, CanvasSceneError> {
        let tone = |name: &str| -> Result<[f32; 4], CanvasSceneError> {
            match theme.tones.get(name) {
                Some(c) => css(&Value::String(c.clone()), &format!("theme tone {name}")),
                None => Ok(WHITE),
            }
        };
        let halo = match &theme.ground {
            Some(ground) => css(&Value::String(ground.clone()), "theme --surface-diagram")?,
            None => BLACK,
        };
        let [r, g, b, a] = tone("ink")?;
        Ok(Self {
            label: tone("label")?,
            halo,
            border: [r, g, b, a * MINIMAP_BORDER_ALPHA],
        })
    }
}

/// `Canvas2D`'s minimap over the viewport's bottom-right corner: a dot per drawn scene position on a map
/// `max + 1` cells wide and tall (10 with nothing drawn), and `MiniMap`'s default viewport outline.
pub fn minimap(
    drawn: &[[f64; 2]],
    viewport: (f32, f32),
    colors: &OverlayColors,
) -> Vec<DrawCommand> {
    let (w, h) = MINIMAP_SIZE;
    let outer = Rect {
        x: viewport.0 - MINIMAP_INSET - w - 2.0 * MINIMAP_BORDER,
        y: viewport.1 - MINIMAP_INSET - h - 2.0 * MINIMAP_BORDER,
        w: w + 2.0 * MINIMAP_BORDER,
        h: h + 2.0 * MINIMAP_BORDER,
    };
    let (ox, oy) = (outer.x + MINIMAP_BORDER, outer.y + MINIMAP_BORDER);
    let extent = |axis: usize| {
        if drawn.is_empty() {
            10.0
        } else {
            drawn.iter().fold(0.0_f64, |m, p| m.max(p[axis])) as f32 + 1.0
        }
    };
    let (sx, sy) = (w / extent(0), h / extent(1));
    let mut out = vec![
        DrawCommand::Quad {
            rect: outer,
            color: colors.border,
        },
        DrawCommand::Quad {
            rect: Rect { x: ox, y: oy, w, h },
            color: MINIMAP_GROUND,
        },
        DrawCommand::Scissor {
            rect: Rect { x: ox, y: oy, w, h },
        },
    ];
    for p in drawn {
        out.push(DrawCommand::Quad {
            rect: Rect {
                x: ox + (p[0] as f32 * sx).floor(),
                y: oy + (p[1] as f32 * sy).floor(),
                w: sx.ceil().max(1.0),
                h: sy.ceil().max(1.0),
            },
            color: MINIMAP_TILE,
        });
    }
    let [vx, vy, vw, vh] = MINIMAP_VIEWPORT;
    let (x0, y0) = (ox + (vx * sx).floor(), oy + (vy * sy).floor());
    let (x1, y1) = (x0 + (vw * sx).floor(), y0 + (vh * sy).floor());
    out.push(DrawCommand::Path {
        segments: vec![
            PathSegment::MoveTo { x: x0, y: y0 },
            PathSegment::LineTo { x: x1, y: y0 },
            PathSegment::LineTo { x: x1, y: y1 },
            PathSegment::LineTo { x: x0, y: y1 },
            PathSegment::Close,
        ],
        fill: None,
        stroke: Some(Stroke {
            color: WHITE,
            width: 1.0,
            dash: Vec::new(),
            dash_offset: 0.0,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
        }),
    });
    out.push(DrawCommand::ResetScissor);
    out
}

/// Each drawn scene position's `x,y` label at its ground point, through the camera the scene is framed with (the
/// view's, so labels follow the orbit). A point behind the camera has no screen position and draws no label.
pub fn coordinate_labels(
    scene: &Scene3D,
    viewport: (f32, f32),
    drawn: &[[f64; 2]],
    colors: &OverlayColors,
) -> Vec<DrawCommand> {
    let projection = view_projection(scene, viewport.0 / viewport.1.max(1.0));
    let interaction = &scene.interaction;
    let mut seen: Vec<[f64; 2]> = Vec::new();
    let mut out = Vec::new();
    for p in drawn {
        if seen.contains(p) {
            continue;
        }
        seen.push(*p);
        let world = Vec3::new(
            (p[0] as f32 - interaction.min_x) * interaction.cell,
            0.0,
            (p[1] as f32 - interaction.min_z) * interaction.cell,
        );
        let Some(pos) = to_screen(projection, world, viewport) else {
            continue;
        };
        out.push(label(
            format!("{},{}", scene_number(p[0]), scene_number(p[1])),
            pos,
            COORDINATE_SIZE,
            TextAlign::Center,
            colors,
        ));
    }
    out
}

/// The hovered ground cell's `x,y` and the id the hit index keys there (the later drawable wins), beside the pointer.
pub fn tile_info(
    interaction: &Interaction,
    cell: (i64, i64),
    pointer: (f32, f32),
    colors: &OverlayColors,
) -> Vec<DrawCommand> {
    let id = interaction
        .cells
        .iter()
        .rev()
        .find(|c| c.x == cell.0 && c.y == cell.1)
        .map(|c| c.id.as_str());
    let text = match id {
        Some(id) => format!("{},{} {id}", cell.0, cell.1),
        None => format!("{},{}", cell.0, cell.1),
    };
    vec![label(
        text,
        Vec2 {
            x: pointer.0 + TILE_INFO_OFFSET,
            y: pointer.1 - TILE_INFO_OFFSET,
        },
        TILE_INFO_SIZE,
        TextAlign::Left,
        colors,
    )]
}

fn label(
    text: String,
    pos: Vec2,
    size: f32,
    align: TextAlign,
    colors: &OverlayColors,
) -> DrawCommand {
    DrawCommand::Text {
        text,
        pos,
        size,
        color: colors.label,
        align,
        weight: 400,
        halo: Some(Halo {
            color: colors.halo,
            width: HALO_WIDTH,
        }),
        mono: true,
    }
}

fn to_screen(projection: Mat4, world: Vec3, viewport: (f32, f32)) -> Option<Vec2> {
    let clip = projection * Vec4::new(world.x, world.y, world.z, 1.0);
    if clip.w <= 0.0 {
        return None;
    }
    let (nx, ny) = (clip.x / clip.w, clip.y / clip.w);
    Some(Vec2 {
        x: (nx + 1.0) / 2.0 * viewport.0,
        y: (1.0 - ny) / 2.0 * viewport.1,
    })
}

/// A scene coordinate as JavaScript prints it.
fn scene_number(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e21 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

fn present<'a>(f: &'a Fields, key: &str) -> Option<&'a Value> {
    f.get(key).filter(|v| !v.is_null())
}

fn bad(field: &str, detail: impl std::fmt::Display) -> CanvasSceneError {
    CanvasSceneError::InvalidProp {
        field: field.to_string(),
        detail: detail.to_string(),
    }
}

fn flag(f: &Fields, key: &str) -> Result<Option<bool>, CanvasSceneError> {
    present(f, key)
        .map(|v| {
            v.as_bool()
                .ok_or_else(|| bad(&format!("canvas.{key}"), "must be a boolean"))
        })
        .transpose()
}

fn positive(f: &Fields, key: &str) -> Result<Option<f64>, CanvasSceneError> {
    let field = format!("canvas.{key}");
    present(f, key)
        .map(|v| {
            let n = v.as_f64().ok_or_else(|| bad(&field, "must be a number"))?;
            if n.is_finite() && n > 0.0 {
                Ok(n)
            } else {
                Err(bad(
                    &field,
                    format!("{n} must be a finite number above zero"),
                ))
            }
        })
        .transpose()
}

fn backdrop(v: &Value) -> Result<Value, CanvasSceneError> {
    let field = "canvas.backgroundImage";
    match v {
        Value::String(url) if !url.is_empty() => Ok(v.clone()),
        Value::Object(asset) => {
            match asset.get("url").and_then(Value::as_str) {
                Some(url) if !url.is_empty() => {}
                _ => {
                    return Err(bad(
                        &format!("{field}.url"),
                        "must be a non-empty URL string",
                    ))
                }
            }
            for key in ["atlas", "sprite"] {
                if asset
                    .get(key)
                    .is_some_and(|k| !k.is_null() && !k.is_string())
                {
                    return Err(bad(&format!("{field}.{key}"), "must be a string"));
                }
            }
            Ok(v.clone())
        }
        _ => Err(bad(
            field,
            "must be a non-empty URL or an Asset {url, atlas?, sprite?}",
        )),
    }
}

fn fog(v: &Value) -> Result<Option<FogOfWar>, CanvasSceneError> {
    let field = "canvas.fogOfWar";
    let rows = v
        .as_array()
        .ok_or_else(|| bad(field, "must be rows of booleans, fogOfWar[y][x]"))?;
    if rows.is_empty() {
        return Ok(None);
    }
    let rows = rows
        .iter()
        .enumerate()
        .map(|(y, row)| {
            row.as_array()
                .ok_or_else(|| bad(&format!("{field}[{y}]"), "must be a row of booleans"))?
                .iter()
                .enumerate()
                .map(|(x, cell)| {
                    cell.as_bool()
                        .ok_or_else(|| bad(&format!("{field}[{y}][{x}]"), "must be a boolean"))
                })
                .collect::<Result<Vec<bool>, _>>()
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(FogOfWar { rows }))
}

fn css(v: &Value, field: &str) -> Result<[f32; 4], CanvasSceneError> {
    let s = v
        .as_str()
        .ok_or_else(|| bad(field, "must be a CSS color string"))?;
    parse_css_color(s).ok_or_else(|| bad(field, format!("`{s}` is not a CSS color")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene3d::model::{Camera, CellId, Lights, Projection, ToneMapping};

    fn props(extra: Value) -> Result<CanvasProps3D, CanvasSceneError> {
        let mut node = json!({"type": "canvas", "mode": "3d", "drawables": []});
        for (k, v) in extra.as_object().unwrap() {
            node[k] = v.clone();
        }
        canvas_props_3d(&node)
    }

    fn field(e: CanvasSceneError) -> String {
        match e {
            CanvasSceneError::InvalidProp { field, .. } => field,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn unset_props_mean_nothing_and_each_invalid_one_names_its_field() {
        let none = props(json!({})).unwrap();
        assert_eq!(none.unit_scale, 1.0);
        assert!(
            !none.show_minimap && !none.loading && none.edit.is_none() && none.fog_of_war.is_none()
        );
        assert_eq!(
            field(props(json!({"unitScale": 0})).unwrap_err()),
            "canvas.unitScale"
        );
        assert_eq!(
            field(props(json!({"worldWidth": -4})).unwrap_err()),
            "canvas.worldWidth"
        );
        assert_eq!(
            field(props(json!({"showMinimap": "yes"})).unwrap_err()),
            "canvas.showMinimap"
        );
        assert_eq!(
            field(props(json!({"fogOfWar": [[true], [false, 1]]})).unwrap_err()),
            "canvas.fogOfWar[1][1]"
        );
        assert_eq!(
            field(props(json!({"backgroundImage": {"atlas": "a.json"}})).unwrap_err()),
            "canvas.backgroundImage.url"
        );
        assert_eq!(
            field(props(json!({"selectedId": 3})).unwrap_err()),
            "canvas.selectedId"
        );
        assert!(props(json!({"fogOfWar": [], "backgroundImage": null}))
            .unwrap()
            .fog_of_war
            .is_none());
    }

    #[test]
    fn editable_carries_the_controlled_selection_and_control_without_it_none() {
        let p = props(json!({"editable": true, "selectedId": "u1"})).unwrap();
        assert_eq!(
            p.edit,
            Some(EditMode {
                selected: Some("u1".into())
            })
        );
        assert_eq!(props(json!({"selectedId": "u1"})).unwrap().edit, None);
    }

    #[test]
    fn fog_is_row_major_from_the_scene_origin_and_control_outside_the_grid_is_clear() {
        let p = props(json!({"fogOfWar": [[false, true], [true, false]]})).unwrap();
        let fog = p.fog_of_war.unwrap();
        assert!(fog.fogged(1.0, 0.0) && fog.fogged(0.0, 1.0) && fog.fogged(0.6, 0.4));
        assert!(!fog.fogged(0.0, 0.0) && !fog.fogged(5.0, 0.0) && !fog.fogged(-1.0, 1.0));
        let mut meshes = vec![selection_ring(
            &Interaction {
                cell: 1.0,
                ..Default::default()
            },
            [0.0; 3],
            [0.8, 0.4, 1.0],
        )
        .unwrap()];
        shade_fogged(&mut meshes);
        assert_eq!(meshes[0].material.color, [0.2, 0.1, 0.25]);
    }

    #[test]
    fn world_size_replaces_the_drawn_bounds_per_axis_and_unit_scale_grows_units_about_their_anchor()
    {
        let p = props(json!({"worldWidth": 20, "unitScale": 2})).unwrap();
        assert_eq!(p.world_bounds([3.0, 5.0, 1.0, 4.0]), [0.0, 20.0, 1.0, 4.0]);
        let anchor = DVec3::new(2.0, 0.0, 3.0);
        let m = p.unit_transform(true, anchor);
        assert_eq!(m.transform_point3(anchor), anchor);
        assert_eq!(
            m.transform_point3(DVec3::new(2.0, 1.0, 3.0)),
            DVec3::new(2.0, 2.0, 3.0)
        );
        assert_eq!(
            p.unit_transform(false, anchor),
            DMat4::IDENTITY,
            "an untagged drawable is no unit"
        );
    }

    #[test]
    fn the_selection_ring_is_three_s_flat_ring_at_the_cell_and_control_the_vertex_layout() {
        let interaction = Interaction {
            cell: 2.0,
            min_x: 1.0,
            min_z: -1.0,
            ..Default::default()
        };
        let ring = selection_ring(&interaction, [3.0, 1.0, 0.5], [1.0, 1.0, 0.0]).unwrap();
        assert_eq!(ring.mesh.positions.len(), 66);
        assert_eq!(ring.mesh.indices.len(), 192);
        let m = Mat4::from_cols_array(&ring.transform);
        let center = m.transform_point3(Vec3::ZERO);
        assert!((center - Vec3::new(4.0, 0.52, 4.0)).length() < 1e-5);
        let up = m.transform_vector3(Vec3::Z);
        assert!((up - Vec3::Y).length() < 1e-5, "faces up");
        let outer = m.transform_point3(Vec3::from(ring.mesh.positions[33]));
        assert!(((outer - center).length() - 0.6).abs() < 1e-5);
        assert_eq!(
            (ring.material.kind, ring.material.side, ring.id),
            (MaterialKind::Flat(Flat::default()), Side::Double, None)
        );
    }

    #[test]
    fn the_preview_moves_only_the_dragged_drawable_wherever_it_nests() {
        let nodes = vec![
            json!({"type": "draw-mesh", "id": "a", "position": {"x": 0, "y": 0, "z": 1}}),
            json!({"type": "draw-group", "position": {"x": 0, "y": 0}, "items": [
                {"type": "draw-sprite", "id": "b", "position": {"x": 1, "y": 1}}
            ]}),
            json!({"type": "draw-shape-layer", "items": [
                {"id": "c", "position": {"x": 2, "y": 2}}, {"id": "d", "position": {"x": 3, "y": 3}}
            ]}),
        ];
        let a = with_preview_position(&nodes, "a", 5, 6);
        assert_eq!(
            a[0]["position"],
            json!({"x": 5, "y": 6}),
            "the preview drops z, as the web host's does"
        );
        assert_eq!(a[1], nodes[1]);
        let b = with_preview_position(&nodes, "b", 7, 8);
        assert_eq!(b[1]["items"][0]["position"], json!({"x": 7, "y": 8}));
        assert_eq!(b[1]["position"], json!({"x": 0, "y": 0}));
        let c = with_preview_position(&nodes, "c", 9, 9);
        assert_eq!(c[2]["items"][0]["position"], json!({"x": 9, "y": 9}));
        assert_eq!(c[2]["items"][1], nodes[2]["items"][1]);
        assert_eq!(with_preview_position(&nodes, "zz", 1, 1), nodes);
    }

    fn colors() -> OverlayColors {
        OverlayColors::from_theme(&SceneTheme::default()).unwrap()
    }

    #[test]
    fn the_minimap_is_the_2d_minimap_in_the_bottom_right_and_control_an_empty_board_maps_ten_cells()
    {
        let cmds = minimap(&[[0.0, 0.0], [9.0, 4.0]], (800.0, 600.0), &colors());
        let DrawCommand::Quad { rect: outer, .. } = &cmds[0] else {
            panic!("{:?}", cmds[0])
        };
        assert_eq!(
            (outer.x, outer.y, outer.w, outer.h),
            (640.0, 490.0, 152.0, 102.0)
        );
        let DrawCommand::Quad { rect: last, color } = &cmds[4] else {
            panic!("{:?}", cmds[4])
        };
        assert_eq!(
            (last.x, last.y, last.w, last.h),
            (641.0 + 135.0, 491.0 + 80.0, 15.0, 20.0)
        );
        assert_eq!(*color, MINIMAP_TILE);
        assert!(matches!(cmds.last(), Some(DrawCommand::ResetScissor)));
        let empty = minimap(&[], (800.0, 600.0), &colors());
        let Some(DrawCommand::Path { segments, .. }) = empty.get(empty.len() - 2) else {
            panic!()
        };
        assert!(
            matches!(segments[0], PathSegment::MoveTo { x, y } if x == 641.0 + 300.0 && y == 491.0 + 200.0)
        );
    }

    fn framed() -> Scene3D {
        Scene3D {
            camera: Camera {
                eye: [0.0, 10.0, 10.0],
                target: [0.0, 0.0, 0.0],
                up: [0.0, 1.0, 0.0],
                projection: Projection::Perspective {
                    fov_y_degrees: 45.0,
                },
                near: 0.1,
                far: 1000.0,
            },
            background: [0.0; 4],
            lights: Lights::default(),
            shadows: false,
            tone_mapping: ToneMapping::None,
            meshes: Vec::new(),
            grid: None,
            interaction: Interaction {
                orbit: true,
                cell: 1.0,
                min_x: 0.0,
                min_z: 0.0,
                cells: vec![
                    CellId {
                        x: 0,
                        y: 0,
                        id: "tile".into(),
                    },
                    CellId {
                        x: 0,
                        y: 0,
                        id: "knight".into(),
                    },
                ],
                ..Default::default()
            },
            animated: false,
            environment: None,
            post: None,
            time: 0.0,
        }
    }

    #[test]
    fn coordinates_label_each_drawn_cell_once_at_its_projection_and_control_behind_the_camera_none()
    {
        let cmds = coordinate_labels(
            &framed(),
            (400.0, 400.0),
            &[[0.0, 0.0], [0.0, 0.0], [1.5, 2.0]],
            &colors(),
        );
        assert_eq!(cmds.len(), 2);
        let DrawCommand::Text { text, pos, .. } = &cmds[0] else {
            panic!()
        };
        assert_eq!(text, "0,0");
        assert!((pos.x - 200.0).abs() < 1e-3 && (pos.y - 200.0).abs() < 1e-3);
        let DrawCommand::Text { text, .. } = &cmds[1] else {
            panic!()
        };
        assert_eq!(text, "1.5,2");
        assert!(coordinate_labels(&framed(), (400.0, 400.0), &[[0.0, 40.0]], &colors()).is_empty());
    }

    #[test]
    fn tile_info_names_the_later_drawable_on_the_cell_and_control_an_empty_cell_only_its_coordinates(
    ) {
        let interaction = framed().interaction;
        let DrawCommand::Text { text, pos, .. } =
            &tile_info(&interaction, (0, 0), (50.0, 60.0), &colors())[0]
        else {
            panic!()
        };
        assert_eq!((text.as_str(), pos.x, pos.y), ("0,0 knight", 62.0, 48.0));
        let DrawCommand::Text { text, .. } =
            &tile_info(&interaction, (2, 3), (0.0, 0.0), &colors())[0]
        else {
            panic!()
        };
        assert_eq!(text, "2,3");
    }

    #[test]
    fn the_backdrop_clears_to_the_background_color_then_the_ground_then_nothing() {
        let theme = SceneTheme::default();
        let DrawCommand::Clear { color } =
            backdrop_clear(&json!({"backgroundColor": "#ff0000"}), &theme).unwrap()
        else {
            panic!()
        };
        assert_eq!(color, [1.0, 0.0, 0.0, 1.0]);
        let DrawCommand::Clear { color } = backdrop_clear(&json!({}), &theme).unwrap() else {
            panic!()
        };
        assert_eq!(color, [0.0; 4]);
        assert!(backdrop_clear(&json!({"backgroundColor": "ink"}), &theme).is_err());
    }
}
