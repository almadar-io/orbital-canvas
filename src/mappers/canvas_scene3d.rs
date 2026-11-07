//! `canvas` with `mode: "3d"` → [`Scene3D`]: the scene the three.js host (`Canvas3DHost`, `Drawable3D`, `DrawMesh3D`,
//! `Lighting3D`) builds today, rebuilt as a retained scene. Anything the pattern declares that this slice does not draw
//! is a typed error naming the field, never a silent drop.

use super::game_canvas_2d::{
    parse_css_color, to_drawable_nodes, CanvasSceneError,
};
use super::learning::shape::parse_path_data;
use super::learning::Xf;
use super::skinned_mesh_2d;
use crate::asset;
use crate::atlas;
use crate::packet::DrawCommand;
use crate::projector::{Projection as Layout2D, Projector, TILE_WIDTH};
use crate::scene3d::animation::{self, AnimatedState, Animation, AnimationError, MATERIAL_TRACKS};
use crate::scene3d::geometry;
use crate::scene3d::gltf::{model_state, ModelState};
use crate::scene3d::label::{label, LabelStyle};
use crate::scene3d::model::{
    srgb_to_linear, srgb_to_linear_rgb, AmbientLight, Camera, DirectionalLight, Flat, Grid,
    HemisphereLight, Lights, Material, MaterialKind, MeshData, MeshInstance, PointLight,
    Projection, Scene3D, ShadowSettings, Side, ToneMapping,
};
use crate::scene3d::model::{CellId, Interaction};
use crate::scene3d::skin;
use crate::scene3d::tessellate::{self, Subpath};
use crate::skinning_2d::{self, BoneDef, Pose, SkinMesh};
use glam::{DMat4, DVec3, Mat4};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::f64::consts::FRAC_PI_2;

type Fields = Map<String, Value>;

const STEP_PARITY: u8 = 4;

/// The theme's scene axis as `canvasTheme.ts` resolves it from CSS variables. [`Default`] holds that resolver's
/// fallbacks, which is what the three host draws under a theme that declares none of these variables.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SceneTheme {
    /// `--surface-diagram`; `None` is transparent.
    pub ground: Option<String>,
    /// `--light-ambient`.
    pub ambient: f32,
    /// `--light-key`.
    pub key: f32,
    /// `--light-key-color`.
    pub key_color: String,
    /// `--material-roughness`, for standard/physical meshes that leave it unset.
    pub roughness: f32,
    /// `--material-metalness`, likewise.
    pub metalness: f32,
    /// `--material-flat` >= 1.
    pub flat: bool,
    /// `--material-outline`; above zero gives every mesh an outline.
    pub outline: f32,
    /// The diagram tones (`grid`, `axis`, `ink`, `series-N`, …) as CSS colors; a tone the theme leaves out draws
    /// white, which is what three keeps when the web host hands it `currentColor`.
    pub tones: std::collections::BTreeMap<String, String>,
}

impl Default for SceneTheme {
    fn default() -> Self {
        Self {
            ground: None,
            ambient: 0.55,
            key: 1.0,
            key_color: "#ffffff".into(),
            roughness: 0.6,
            metalness: 0.0,
            flat: false,
            outline: 0.0,
            tones: std::collections::BTreeMap::new(),
        }
    }
}

impl SceneTheme {
    /// The scene theme in a host's `canvasTheme`: its `ground` (`transparent` is none), its `tones` (one left at
    /// `currentColor` is unset) and its `scene` axis; an axis field the canvas theme does not carry keeps the
    /// resolver's fallback, as the web resolver keeps it for an undeclared CSS variable.
    pub fn from_canvas_theme(theme: &Value) -> Result<Self, CanvasSceneError> {
        let path = "canvasTheme";
        let fields = theme
            .as_object()
            .ok_or_else(|| bad(path, "must be the resolved canvas theme"))?;
        let mut out = SceneTheme::default();
        if let Some(ground) = opt_str(fields, "ground", path)? {
            out.ground = (ground != "transparent").then(|| ground.to_owned());
        }
        if let Some(tones) = opt_object(fields, "tones", path)? {
            for (tone, css) in tones {
                let css = css
                    .as_str()
                    .ok_or_else(|| bad(&join(&join(path, "tones"), tone), "must be a CSS color"))?;
                if css != "currentColor" {
                    out.tones.insert(tone.clone(), css.to_owned());
                }
            }
        }
        if let Some(scene) = opt_object(fields, "scene", path)? {
            let at = join(path, "scene");
            let num = |k: &str| opt_number(scene, k, &at);
            out.ambient = num("ambient")?.map_or(out.ambient, |v| v as f32);
            out.key = num("key")?.map_or(out.key, |v| v as f32);
            out.roughness = num("roughness")?.map_or(out.roughness, |v| v as f32);
            out.metalness = num("metalness")?.map_or(out.metalness, |v| v as f32);
            out.outline = num("outline")?.map_or(out.outline, |v| v as f32);
            if let Some(color) = opt_str(scene, "keyColor", &at)? {
                out.key_color = color.to_owned();
            }
            if let Some(flat) = present(scene, "flat") {
                out.flat = flat
                    .as_bool()
                    .ok_or_else(|| bad(&join(&at, "flat"), "must be a boolean"))?;
            }
        }
        Ok(out)
    }
}

pub fn scene_from_canvas_node(
    node: &Value,
    width: f32,
    height: f32,
) -> Result<Scene3D, CanvasSceneError> {
    scene_from_canvas_node_themed(node, width, height, &SceneTheme::default())
}

/// The scene at rest: every declared `animation` is left unapplied, as the three host draws before its first frame
/// and under reduced motion. [`Scene3D::animated`] still reports it, so a host that plays motion maps with
/// [`scene_from_canvas_node_at`] instead.
pub fn scene_from_canvas_node_themed(
    node: &Value,
    width: f32,
    height: f32,
    theme: &SceneTheme,
) -> Result<Scene3D, CanvasSceneError> {
    compose(node, width, height, theme, None, None)
}

/// The scene with every `animation` sampled at `elapsed_ms` on the canvas clock: the three host's
/// `clock.elapsedTime`, one clock per canvas that starts at its first frame and is never reset when the drawables
/// change, so every mesh and group shares the phase whenever it mounted.
pub fn scene_from_canvas_node_at(
    node: &Value,
    width: f32,
    height: f32,
    theme: &SceneTheme,
    elapsed_ms: f64,
) -> Result<Scene3D, CanvasSceneError> {
    if !(elapsed_ms.is_finite() && elapsed_ms >= 0.0) {
        return Err(bad(
            "elapsedMs",
            format!("{elapsed_ms} is not a time on the canvas clock (finite, at least zero)"),
        ));
    }
    compose(node, width, height, theme, Some(elapsed_ms), None)
}

/// [`scene_from_canvas_node_at`] with the page's wall clock as well: `epoch_ms` is the three host's
/// `performance.timeOrigin + performance.now()`, the clock `draw-fx-layer` entries are born on (`bornAt`) and sprite
/// sheets play on. Without it an fx entry's age comes from its ttl, as `fx_projection` defines for a clockless paint.
pub fn scene_from_canvas_node_at_epoch(
    node: &Value,
    width: f32,
    height: f32,
    theme: &SceneTheme,
    elapsed_ms: f64,
    epoch_ms: f64,
) -> Result<Scene3D, CanvasSceneError> {
    for (field, ms) in [("elapsedMs", elapsed_ms), ("epochMs", epoch_ms)] {
        if !(ms.is_finite() && ms >= 0.0) {
            return Err(bad(
                field,
                format!("{ms} is not a time on the canvas clock (finite, at least zero)"),
            ));
        }
    }
    compose(node, width, height, theme, Some(elapsed_ms), Some(epoch_ms))
}

fn compose(
    node: &Value,
    width: f32,
    height: f32,
    theme: &SceneTheme,
    elapsed_ms: Option<f64>,
    epoch_ms: Option<f64>,
) -> Result<Scene3D, CanvasSceneError> {
    compose_with(node, width, height, theme, elapsed_ms, epoch_ms, None)
}

pub use crate::scene3d::view::DragPreview;

/// [`scene_from_canvas_node_at_epoch`] with an edit-mode drag preview.
pub fn scene_from_canvas_node_with_preview(
    node: &Value,
    width: f32,
    height: f32,
    theme: &SceneTheme,
    elapsed_ms: Option<f64>,
    epoch_ms: Option<f64>,
    preview: Option<&DragPreview>,
) -> Result<Scene3D, CanvasSceneError> {
    compose_with(node, width, height, theme, elapsed_ms, epoch_ms, preview)
}

#[allow(clippy::too_many_arguments)]
fn compose_with(
    node: &Value,
    width: f32,
    height: f32,
    theme: &SceneTheme,
    elapsed_ms: Option<f64>,
    epoch_ms: Option<f64>,
    preview: Option<&DragPreview>,
) -> Result<Scene3D, CanvasSceneError> {
    let object = node.as_object().ok_or(CanvasSceneError::NotAnObject)?;
    let fields = object
        .get("props")
        .and_then(Value::as_object)
        .unwrap_or(object);
    let mode = present(fields, "mode").and_then(Value::as_str);
    if mode != Some("3d") {
        return Err(CanvasSceneError::UnsupportedMode(
            mode.unwrap_or("2d").to_owned(),
        ));
    }
    if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
        return Err(bad(
            "viewport",
            format!("{width}x{height} is not a positive size"),
        ));
    }
    check_canvas_keys(fields)?;
    let props = super::canvas3d_props::canvas_props_3d(node)?;

    let cell = match opt_number(fields, "pixelsPerUnit", "canvas")? {
        Some(ppu) if ppu > 0.0 => 1.0 / ppu,
        Some(ppu) => {
            return Err(bad(
                "canvas.pixelsPerUnit",
                format!("{ppu} must be above zero (the three host ignores it otherwise)"),
            ))
        }
        None => 1.0,
    };
    let show_grid = opt_bool(fields, "showGrid", "canvas")?.unwrap_or(true);
    let post = post_settings(opt_object(fields, "post", "canvas")?)?;
    let environment = match opt_object(fields, "lighting", "canvas")? {
        Some(l) => match opt_str(l, "environment", "canvas.lighting")? {
            Some("room") => Some(crate::scene3d::model::EnvironmentKind::Room),
            _ => None,
        },
        None => None,
    };

    let mut nodes = Vec::new();
    if let Some(drawables) = present(fields, "drawables") {
        if !drawables.is_array() {
            return Err(bad("canvas.drawables", "must be an array"));
        }
        for (i, node) in drawable_nodes(drawables, "drawables")?
            .into_iter()
            .enumerate()
        {
            nodes.push((format!("drawables[{i}]"), node));
        }
    }
    if let Some(children) = present(fields, "children") {
        for (i, node) in drawable_nodes(children, "children")?
            .into_iter()
            .enumerate()
        {
            nodes.push((format!("children[{i}]"), node));
        }
    }

    let drawn_bounds = scene_bounds(&nodes)?;
    let [min_x, max_x, min_z, max_z] = props.world_bounds([
        drawn_bounds.min_x,
        drawn_bounds.max_x,
        drawn_bounds.min_z,
        drawn_bounds.max_z,
    ]);
    let bounds = Bounds {
        min_x,
        max_x,
        min_z,
        max_z,
    };
    let drawn: Vec<[f64; 2]> = drawn_items(&nodes)?
        .into_iter()
        .map(|(pos, _)| [pos[0], pos[1]])
        .collect();
    let paint_nodes: Vec<(String, Value)> = match preview {
        Some(p) => {
            let values: Vec<Value> = nodes.iter().map(|(_, n)| n.clone()).collect();
            let moved = super::canvas3d_props::with_preview_position(&values, &p.id, p.x, p.y);
            nodes
                .iter()
                .map(|(path, _)| path.clone())
                .zip(moved)
                .collect()
        }
        None => nodes.clone(),
    };
    let shadows = opt_bool(fields, "shadows", "canvas")?.unwrap_or(true);
    let mut walk = Walk {
        theme,
        meshes: Vec::new(),
        pick_id: None,
        elapsed_ms,
        epoch_ms,
        animated: false,
        scopes: vec![HashMap::new()],
        scope: 0,
        skins: Vec::new(),
    };
    let place = Placement {
        cell,
        off_x: -bounds.min_x * cell,
        off_z: -bounds.min_z * cell,
    };
    for (path, node) in &paint_nodes {
        let fields_of = node.as_object();
        let has_id = fields_of
            .and_then(|f| f.get("id"))
            .and_then(Value::as_str)
            .is_some();
        let pos = fields_of
            .and_then(|f| present(f, "position"))
            .map(|p| scene_pos(p, path))
            .transpose()?;
        let parent = match pos {
            Some(pos) => props.unit_transform(has_id, place.world(pos)),
            None => DMat4::IDENTITY,
        };
        let first = walk.meshes.len();
        walk.drawable(node, path, parent, place, 1.0)?;
        if let (Some(fog), Some(pos)) = (&props.fog_of_war, pos) {
            if fog.fogged(pos[0], pos[1]) {
                super::canvas3d_props::shade_fogged(&mut walk.meshes[first..]);
            }
        }
    }
    walk.bind_skins()?;

    let camera = camera(opt_object(fields, "camera", "canvas")?, &bounds, cell)?;
    let (lights, tone_mapping) = lights(opt_object(fields, "lighting", "canvas")?, shadows, theme)?;
    // a backdrop image paints behind the scene in the 2D layer, so the scene itself clears to transparent
    let background = match (&props.background_image, present(fields, "backgroundColor")) {
        (Some(_), _) => [0.0; 4],
        (None, Some(value)) => rgba(value, "canvas.backgroundColor")?,
        (None, None) => match &theme.ground {
            Some(ground) => rgba(&Value::String(ground.clone()), "theme --surface-diagram")?,
            None => [0.0; 4],
        },
    };
    let grid = if show_grid {
        Some(grid(&bounds, cell, theme)?)
    } else {
        None
    };
    let mut interaction = interaction(fields, &nodes, &bounds, cell)?;
    interaction.max_x = bounds.max_x as f32;
    interaction.max_z = bounds.max_z as f32;
    interaction.drawn = drawn;
    interaction.editable = props.edit.is_some();
    interaction.follow_from = follow_start(opt_object(fields, "camera", "canvas")?, &bounds, cell)?;
    let mut meshes = walk.meshes;
    if let Some(selected) = props.edit.as_ref().and_then(|e| e.selected.as_deref()) {
        let at = match preview.filter(|p| p.id == selected) {
            Some(p) => Some([p.x as f64, p.y as f64, 0.0]),
            None => drawn_position_of(&nodes, selected)?,
        };
        if let Some(at) = at {
            let color = match theme.tones.get("highlight") {
                Some(css) => rgb(&Value::String(css.clone()), "theme tone highlight")?,
                None => [1.0, 1.0, 1.0],
            };
            meshes.push(super::canvas3d_props::selection_ring(
                &interaction,
                at,
                color,
            )?);
        }
    }
    Ok(Scene3D {
        camera,
        background,
        lights,
        shadows,
        tone_mapping,
        meshes,
        grid,
        interaction,
        animated: walk.animated,
        environment,
        post,
    })
}

/// A following camera's first-frame pose (the host's CameraPose `follow`/`chase` framing, `[cx, d*0.5, cz+d]` turned by
/// `azimuth`), which GameCamera3D then eases toward the follow pose; `None` for a camera that does not follow.
fn follow_start(
    c: Option<&Fields>,
    b: &Bounds,
    cell: f64,
) -> Result<Option<[f32; 3]>, CanvasSceneError> {
    let Some(c) = c else { return Ok(None) };
    if !matches!(
        present(c, "mode").and_then(Value::as_str),
        Some("follow" | "chase")
    ) {
        return Ok(None);
    }
    let (span_x, span_z) = (b.max_x - b.min_x, b.max_z - b.min_z);
    let (cx, cz) = (span_x / 2.0 * cell, span_z / 2.0 * cell);
    let size = (span_x * cell).max(span_z * cell).max(4.0);
    let d = size
        / opt_number(c, "zoom", "canvas.camera")?
            .filter(|z| *z > 0.0)
            .unwrap_or(1.0);
    let (x, y, z) = (cx, d * 0.5, cz + d);
    let (x, z) = match opt_number(c, "azimuth", "canvas.camera")?.filter(|a| *a != 0.0) {
        Some(a) => {
            let (ox, oz) = (x - cx, z - cz);
            let (s, co) = a.sin_cos();
            (cx + ox * co - oz * s, cz + ox * s + oz * co)
        }
        None => (x, z),
    };
    Ok(Some([x as f32, y as f32, z as f32]))
}

/// Where the first drawable carrying `id` is drawn (scene units).
fn drawn_position_of(
    nodes: &[(String, Value)],
    id: &str,
) -> Result<Option<[f64; 3]>, CanvasSceneError> {
    Ok(drawn_items(nodes)?
        .into_iter()
        .find(|(_, item)| item.as_deref() == Some(id))
        .map(|(pos, _)| pos))
}

/// `post`: Effects3D mounts bloom and vignette with these defaults when the key is present.
fn post_settings(
    post: Option<&Fields>,
) -> Result<Option<crate::scene3d::model::PostSettings>, CanvasSceneError> {
    use crate::scene3d::model::{BloomSettings, PostSettings, VignetteSettings};
    let Some(post) = post else { return Ok(None) };
    check_keys(post, &["bloom", "vignette"], "canvas.post")?;
    let bloom = match opt_object(post, "bloom", "canvas.post")? {
        Some(b) => {
            check_keys(
                b,
                &["intensity", "threshold", "smoothing"],
                "canvas.post.bloom",
            )?;
            Some(BloomSettings {
                intensity: opt_number(b, "intensity", "canvas.post.bloom")?.unwrap_or(1.0) as f32,
                threshold: opt_number(b, "threshold", "canvas.post.bloom")?.unwrap_or(0.9) as f32,
                smoothing: opt_number(b, "smoothing", "canvas.post.bloom")?.unwrap_or(0.3) as f32,
            })
        }
        None => None,
    };
    let vignette = match opt_object(post, "vignette", "canvas.post")? {
        Some(v) => {
            check_keys(v, &["offset", "darkness"], "canvas.post.vignette")?;
            Some(VignetteSettings {
                offset: opt_number(v, "offset", "canvas.post.vignette")?.unwrap_or(0.3) as f32,
                darkness: opt_number(v, "darkness", "canvas.post.vignette")?.unwrap_or(0.6) as f32,
            })
        }
        None => None,
    };
    Ok((bloom.is_some() || vignette.is_some()).then_some(PostSettings { bloom, vignette }))
}

/// OrbitControls are on unless `camera.controls` is false or the camera follows; the cell index is the host's
/// `buildHitIndex`, whose keys only an integral position can match.
fn interaction(
    fields: &Fields,
    nodes: &[(String, Value)],
    bounds: &Bounds,
    cell: f64,
) -> Result<Interaction, CanvasSceneError> {
    let camera = opt_object(fields, "camera", "canvas")?;
    let controls = match camera {
        Some(c) => opt_bool(c, "controls", "canvas.camera")?.unwrap_or(true),
        None => true,
    };
    let follows = camera
        .and_then(|c| c.get("mode"))
        .and_then(Value::as_str)
        .is_some_and(|m| m == "follow" || m == "chase");
    let mut cells = Vec::new();
    for (pos, id) in drawn_items(nodes)? {
        let Some(id) = id else { continue };
        if pos[0].fract() == 0.0 && pos[1].fract() == 0.0 {
            cells.push(CellId {
                x: pos[0] as i64,
                y: pos[1] as i64,
                id,
            });
        }
    }
    Ok(Interaction {
        orbit: controls && !follows,
        cell: cell as f32,
        min_x: bounds.min_x as f32,
        min_z: bounds.min_z as f32,
        cells,
        ..Default::default()
    })
}

/// The host's drei `Grid`: sized to the scene bounds plus a cell on each side (at least 10), centered on them.
fn grid(bounds: &Bounds, cell: f64, theme: &SceneTheme) -> Result<Grid, CanvasSceneError> {
    let tone = |name: &str| -> Result<[f32; 3], CanvasSceneError> {
        match theme.tones.get(name) {
            Some(css) => rgb(&Value::String(css.clone()), &format!("theme tone {name}")),
            None => Ok([1.0, 1.0, 1.0]),
        }
    };
    let (span_x, span_z) = (bounds.max_x - bounds.min_x, bounds.max_z - bounds.min_z);
    Ok(Grid {
        center: [
            (span_x / 2.0 * cell - cell / 2.0) as f32,
            0.0,
            (span_z / 2.0 * cell - cell / 2.0) as f32,
        ],
        size: [
            ((span_x + 2.0) * cell).max(10.0) as f32,
            ((span_z + 2.0) * cell).max(10.0) as f32,
        ],
        cell_size: 1.0,
        cell_thickness: 1.0,
        cell_color: tone("grid")?,
        section_size: 5.0,
        section_thickness: 1.5,
        section_color: tone("axis")?,
        fade_distance: 50.0,
        fade_strength: 1.0,
        fade_from: 1.0,
    })
}

fn check_canvas_keys(fields: &Fields) -> Result<(), CanvasSceneError> {
    for (key, value) in fields {
        let field = format!("canvas.{key}");
        match key.as_str() {
            "type" | "mode" | "drawables" | "children" | "camera" | "backgroundColor"
            | "pixelsPerUnit" | "showGrid" | "shadows" | "lighting" | "post" => {}
            // Interaction and DOM-wrapper props: they change no pixel of the scene.
            "tileClickEvent" | "unitClickEvent" | "tileHoverEvent" | "tileLeaveEvent"
            | "featureClickEvent" | "keyMap" | "keyUpMap" | "selectedId" | "onSelect"
            | "onMove" | "selectEvent" | "moveEvent" | "className" | "enter" | "enterDelay"
            | "skeleton" | "aria-label" | "aria-labelledby" | "aria-describedby" | "aria-live"
            | "aria-current" | "aria-pressed" | "aria-expanded" | "aria-selected"
            | "aria-hidden" | "aria-busy" | "role" | "lang" | "dir" | "tabIndex" => {}
            // given their 3D meaning by canvas3d_props (owner ruling 2026-11-06), which validates them
            "isLoading" | "editable" | "showMinimap" | "showCoordinates" | "showTileInfo"
            | "fogOfWar" | "backgroundImage" | "worldWidth" | "worldHeight" | "unitScale" => {}
            "fit" => {
                if opt_bool(fields, key, "canvas")? == Some(true) {
                    return Err(bad(
                        &field,
                        "fits a 2D grid to the viewport; a 3D canvas frames with its camera",
                    ));
                }
            }
            "projection" | "tileWidth" => {
                if !value.is_null() {
                    return Err(bad(&field, "a 2D grid projection; a 3D canvas has none"));
                }
            }
            _ => return Err(bad(&field, "not a declared canvas prop")),
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct Bounds {
    min_x: f64,
    max_x: f64,
    min_z: f64,
    max_z: f64,
}

/// `collectDrawnItems` over the top-level list: scene `x` spans world X, scene `y` spans world Z.
fn scene_bounds(nodes: &[(String, Value)]) -> Result<Bounds, CanvasSceneError> {
    let mut bounds: Option<Bounds> = None;
    for (pos, _) in drawn_items(nodes)? {
        let b = bounds.get_or_insert(Bounds {
            min_x: pos[0],
            max_x: pos[0],
            min_z: pos[1],
            max_z: pos[1],
        });
        b.min_x = b.min_x.min(pos[0]);
        b.max_x = b.max_x.max(pos[0]);
        b.min_z = b.min_z.min(pos[1]);
        b.max_z = b.max_z.max(pos[1]);
    }
    Ok(bounds.unwrap_or(Bounds {
        min_x: 0.0,
        max_x: 10.0,
        min_z: 0.0,
        max_z: 10.0,
    }))
}

fn camera(fields: Option<&Fields>, b: &Bounds, cell: f64) -> Result<Camera, CanvasSceneError> {
    let empty = Fields::new();
    let c = fields.unwrap_or(&empty);
    let path = "canvas.camera";
    check_keys(
        c,
        &[
            "pos",
            "target",
            "zoom",
            "fov",
            "mode",
            "azimuth",
            "elevation",
            "controls",
        ],
        path,
    )?;
    let pos = present(c, "pos")
        .map(|v| scene_pos(v, "canvas.camera.pos"))
        .transpose()?;
    opt_bool(c, "controls", path)?;
    let mode = match present(c, "mode") {
        None => "isometric",
        Some(value) => match value.as_str() {
            Some(m @ ("isometric" | "perspective" | "top-down" | "front" | "follow" | "chase")) => {
                m
            }
            _ => return Err(bad("canvas.camera.mode", "not a declared camera mode")),
        },
    };
    let zoom = opt_number(c, "zoom", path)?;
    if let Some(z) = zoom.filter(|z| *z <= 0.0) {
        return Err(bad(
            "canvas.camera.zoom",
            format!("{z} must be above zero (the three host ignores it otherwise)"),
        ));
    }
    let fov = opt_number(c, "fov", path)?.unwrap_or(45.0);
    if !(fov > 0.0 && fov < 180.0) {
        return Err(bad(
            "canvas.camera.fov",
            format!("{fov} is outside (0, 180)"),
        ));
    }
    let azimuth = opt_number(c, "azimuth", path)?;
    let elevation = opt_number(c, "elevation", path)?;
    let target = present(c, "target")
        .map(|v| scene_pos(v, "canvas.camera.target"))
        .transpose()?;
    let follow = matches!(mode, "follow" | "chase");
    let world = |t: [f64; 3]| {
        DVec3::new(
            t[0] * cell - b.min_x * cell,
            t[2],
            t[1] * cell - b.min_z * cell,
        )
    };
    let (span_x, span_z) = (b.max_x - b.min_x, b.max_z - b.min_z);
    let scene_center = DVec3::new(span_x / 2.0 * cell, 0.0, span_z / 2.0 * cell);
    // `target` replaces the scene center as the point a mode frames (and a follow camera follows).
    let center = target.map(world).unwrap_or(scene_center);
    let (eye, look) = if follow {
        let base = if mode == "chase" {
            DVec3::new(0.0, 4.0, -7.0)
        } else {
            DVec3::new(0.0, 12.0, 14.0)
        };
        let horizontal = base.x.hypot(base.z);
        let lifted = match elevation {
            Some(el) => DVec3::new(
                base.x,
                horizontal * el.clamp(-FRAC_PI_2 + 0.1, FRAC_PI_2 - 0.1).tan(),
                base.z,
            ),
            None => base,
        };
        let turned = match azimuth.filter(|a| *a != 0.0) {
            Some(a) => {
                let (s, c) = a.sin_cos();
                DVec3::new(
                    lifted.x * c - lifted.z * s,
                    lifted.y,
                    lifted.x * s + lifted.z * c,
                )
            }
            None => lifted,
        };
        // three's follow and chase offsets are fixed; zoom frames the other modes (G-ORBITAL-GAME-RENDERER-011)
        (center + turned, center)
    } else {
        let size = (span_x * cell).max(span_z * cell).max(4.0);
        let d = size / zoom.unwrap_or(1.0);
        let (cx, cz) = (center.x, center.z);
        let framed = match mode {
            "isometric" => DVec3::new(cx + d, d * 0.8, cz + d),
            "top-down" => DVec3::new(cx, d * 2.0, cz + d * 0.35),
            "front" => DVec3::new(cx, d * 0.32, cz + d * 1.15),
            _ => DVec3::new(cx + d, d, cz + d),
        };
        let base = match elevation.map(|el| el.clamp(0.0, FRAC_PI_2 - 0.1)) {
            None => framed,
            // three's perspective formula; the other modes keep their horizontal offset and take the angle over it
            Some(el) if mode == "perspective" => DVec3::new(cx + d, d * el.tan(), cz + d),
            Some(el) => {
                let horizontal = (framed.x - cx).hypot(framed.z - cz);
                DVec3::new(framed.x, horizontal * el.tan(), framed.z)
            }
        };
        let posed = match azimuth.filter(|a| *a != 0.0) {
            None => base,
            Some(a) => {
                let (ox, oz) = (base.x - cx, base.z - cz);
                let (s, c) = a.sin_cos();
                DVec3::new(cx + ox * c - oz * s, base.y, cz + ox * s + oz * c)
            }
        };
        (orbit_controls_pose(posed, center), center)
    };
    // an authored `pos` places the eye itself, looking at the framed point
    let eye = pos.map(world).unwrap_or(eye);
    Ok(Camera {
        eye: eye.as_vec3().to_array(),
        target: look.as_vec3().to_array(),
        up: [0.0, 1.0, 0.0],
        projection: Projection::Perspective {
            fov_y_degrees: fov as f32,
        },
        near: 0.1,
        far: 1000.0,
    })
}

/// `OrbitControls.update()` at rest (three-stdlib): polar angle capped at π/2 − 0.1, `makeSafe`, distance in [2, 100].
fn orbit_controls_pose(position: DVec3, target: DVec3) -> DVec3 {
    const EPS: f64 = 0.000001;
    let offset = position - target;
    let radius = offset.length();
    let (theta, phi) = if radius == 0.0 {
        (0.0, 0.0)
    } else {
        (
            offset.x.atan2(offset.z),
            (offset.y / radius).clamp(-1.0, 1.0).acos(),
        )
    };
    let phi = phi
        .clamp(0.0, FRAC_PI_2 - 0.1)
        .clamp(EPS, std::f64::consts::PI - EPS);
    let radius = radius.clamp(2.0, 100.0);
    let ring = phi.sin() * radius;
    target + DVec3::new(ring * theta.sin(), phi.cos() * radius, ring * theta.cos())
}

fn lights(
    fields: Option<&Fields>,
    shadows: bool,
    theme: &SceneTheme,
) -> Result<(Lights, ToneMapping), CanvasSceneError> {
    let empty = Fields::new();
    let l = fields.unwrap_or(&empty);
    let path = "canvas.lighting";
    check_keys(
        l,
        &[
            "ambient",
            "directional",
            "hemisphere",
            "points",
            "environment",
            "toneMapping",
        ],
        path,
    )?;
    match opt_str(l, "environment", path)? {
        None | Some("none") => {}
        Some("room") => {}
        Some(_) => return Err(bad("canvas.lighting.environment", "must be room or none")),
    }
    let tone_mapping = match opt_str(l, "toneMapping", path)? {
        None | Some("aces") => ToneMapping::Aces,
        Some("none") => ToneMapping::None,
        Some(_) => return Err(bad("canvas.lighting.toneMapping", "must be aces or none")),
    };

    let ambient = opt_object(l, "ambient", path)?.unwrap_or(&empty);
    let ambient_path = "canvas.lighting.ambient";
    check_keys(ambient, &["intensity", "color", "position"], ambient_path)?;
    if present(ambient, "position").is_some() {
        return Err(bad(
            "canvas.lighting.ambient.position",
            "an ambient light has no position (the three host ignores it)",
        ));
    }
    let ambient = AmbientLight {
        color: opt_rgb(ambient, "color", ambient_path)?.unwrap_or([1.0; 3]),
        intensity: opt_number(ambient, "intensity", ambient_path)?
            .map_or(theme.ambient, |v| v as f32),
    };

    let directional = opt_object(l, "directional", path)?.unwrap_or(&empty);
    let dir_path = "canvas.lighting.directional";
    check_keys(directional, &["intensity", "color", "position"], dir_path)?;
    let directional = DirectionalLight {
        position: match present(directional, "position") {
            Some(v) => f32x3(triple(v, "canvas.lighting.directional.position")?),
            None => [10.0, 20.0, 10.0],
        },
        target: [0.0; 3],
        color: match opt_rgb(directional, "color", dir_path)? {
            Some(color) => color,
            None => rgb(
                &Value::String(theme.key_color.clone()),
                "theme --light-key-color",
            )?,
        },
        intensity: opt_number(directional, "intensity", dir_path)?.map_or(theme.key, |v| v as f32),
        shadow: shadows.then_some(ShadowSettings {
            map_size: 2048,
            bias: -0.0004,
            normal_bias: 0.04,
            extent: 5.0,
            near: 0.5,
            far: 500.0,
        }),
    };

    let hemisphere = opt_object(l, "hemisphere", path)?.unwrap_or(&empty);
    let hemi_path = "canvas.lighting.hemisphere";
    check_keys(
        hemisphere,
        &["intensity", "color", "groundColor"],
        hemi_path,
    )?;
    let hemisphere = HemisphereLight {
        sky: match opt_rgb(hemisphere, "color", hemi_path)? {
            Some(c) => c,
            None => rgb(&Value::String("#87ceeb".into()), "hemisphere default")?,
        },
        ground: match opt_rgb(hemisphere, "groundColor", hemi_path)? {
            Some(c) => c,
            None => rgb(&Value::String("#362d1d".into()), "hemisphere default")?,
        },
        intensity: opt_number(hemisphere, "intensity", hemi_path)?.map_or(0.3, |v| v as f32),
    };

    let mut points = Vec::new();
    if let Some(list) = present(l, "points") {
        let list = list
            .as_array()
            .ok_or_else(|| bad("canvas.lighting.points", "must be an array"))?;
        for (i, point) in list.iter().enumerate() {
            let p_path = format!("canvas.lighting.points[{i}]");
            let p = point
                .as_object()
                .ok_or_else(|| bad(&p_path, "must be an object"))?;
            check_keys(
                p,
                &["intensity", "color", "position", "distance", "decay"],
                &p_path,
            )?;
            let position = present(p, "position")
                .ok_or_else(|| bad(&join(&p_path, "position"), "is required"))?;
            points.push(PointLight {
                position: f32x3(triple(position, &join(&p_path, "position"))?),
                color: opt_rgb(p, "color", &p_path)?.unwrap_or([1.0; 3]),
                intensity: opt_number(p, "intensity", &p_path)?.map_or(1.0, |v| v as f32),
                distance: opt_number(p, "distance", &p_path)?.map_or(0.0, |v| v as f32),
                decay: opt_number(p, "decay", &p_path)?.map_or(2.0, |v| v as f32),
            });
        }
    }
    Ok((
        Lights {
            ambient: Some(ambient),
            hemisphere: Some(hemisphere),
            directional: Some(directional),
            points,
        },
        tone_mapping,
    ))
}

/// `create3DProjector`: scene `x`/`y` scale by the cell onto world X/Z (offset by the bounds anchor at the top
/// level, unanchored inside a group); scene `z` is world Y as authored.
#[derive(Debug, Clone, Copy)]
struct Placement {
    cell: f64,
    off_x: f64,
    off_z: f64,
}

impl Placement {
    fn world(self, p: [f64; 3]) -> DVec3 {
        DVec3::new(
            p[0] * self.cell + self.off_x,
            p[2],
            p[1] * self.cell + self.off_z,
        )
    }

    /// An animation offset in cells on scene axes, projected like a position: `x` to X, `y` to Z, `z` to Y, all
    /// three scaled by the cell (unlike a position's `z`).
    fn offset(self, [x, y, z]: [f64; 3]) -> DVec3 {
        DVec3::new(x * self.cell, z * self.cell, y * self.cell)
    }

    fn inner(self) -> Self {
        Self {
            cell: self.cell,
            off_x: 0.0,
            off_z: 0.0,
        }
    }
}

struct Walk<'a> {
    theme: &'a SceneTheme,
    meshes: Vec<MeshInstance>,
    /// The innermost enclosing drawable id, as three's click walks up to it.
    pick_id: Option<String>,
    /// The canvas clock; `None` leaves animations at rest.
    elapsed_ms: Option<f64>,
    /// The page's wall clock, when the host gives it.
    epoch_ms: Option<f64>,
    animated: bool,
    /// `BoneStore`s: the canvas's at 0, one per `skeleton: true` group after it; name → bone world matrix.
    scopes: Vec<HashMap<String, DMat4>>,
    scope: usize,
    /// Skinned meshes, bound once every bone in the tree is known (three re-binds on each registration).
    skins: Vec<PendingSkin>,
}

struct PendingSkin {
    mesh: usize,
    /// The outline shell's mesh and how far it stands off the posed surface.
    hull: Option<(usize, f32)>,
    scope: usize,
    path: String,
    bones: Vec<String>,
    inverse_bind: Vec<DMat4>,
    joints: Vec<[u16; 4]>,
    weights: Vec<[f32; 4]>,
    bind_inverse: DMat4,
}

const SPRITE_KEYS: &[&str] = &[
    "type",
    "id",
    "position",
    "asset",
    "anchor",
    "width",
    "height",
    "frame",
    "animation",
    "loop",
    "clockMs",
    "flipX",
    "rotation",
    "opacity",
    "shadow",
];

const SKINNED_MESH_KEYS: &[&str] = &[
    "type",
    "id",
    "position",
    "asset",
    "mesh",
    "meshUrl",
    "bones",
    "pose",
    "clip",
    "clipName",
    "frame",
    "opacity",
    "weightsOverlay",
    "ghost",
];

const MESH_KEYS: &[&str] = &[
    "type",
    "id",
    "shape",
    "position",
    "width",
    "height",
    "depth",
    "radius",
    "radiusTop",
    "radiusBottom",
    "tube",
    "segments",
    "vertices",
    "faces",
    "uvs",
    "skin",
    "rotation",
    "pivot",
    "material",
    "outline",
    "castShadow",
    "receiveShadow",
    "animation",
    "opacity",
];

const GROUP_KEYS: &[&str] = &[
    "type",
    "id",
    "position",
    "scale",
    "rotate",
    "rotation",
    "bone",
    "skeleton",
    "animation",
    "opacity",
    "clip",
    "items",
];

const MATERIAL_KEYS: &[&str] = &[
    "kind",
    "color",
    "metalness",
    "roughness",
    "emissive",
    "emissiveIntensity",
    "opacity",
    "transmission",
    "ior",
    "flatShading",
    "side",
    "map",
];

impl Walk<'_> {
    /// The node's declared `animation`, and its state on the canvas clock when there is one.
    fn animation(
        &mut self,
        f: &Fields,
        path: &str,
    ) -> Result<Option<(Animation, Option<AnimatedState>)>, CanvasSceneError> {
        let Some(value) = present(f, "animation") else {
            return Ok(None);
        };
        let declared =
            Animation::parse(value, &join(path, "animation")).map_err(animation_error)?;
        self.animated = true;
        let state = self.elapsed_ms.map(|t| animation::sample(&declared, t));
        Ok(Some((declared, state)))
    }

    fn drawable(
        &mut self,
        node: &Value,
        path: &str,
        parent: DMat4,
        place: Placement,
        opacity: f64,
    ) -> Result<(), CanvasSceneError> {
        let fields = node
            .as_object()
            .ok_or_else(|| bad(path, "not a drawable node"))?;
        let ty = fields.get("type").and_then(Value::as_str).unwrap_or("");
        let outer = self.pick_id.clone();
        if let Some(id) = fields.get("id").and_then(Value::as_str) {
            self.pick_id = Some(id.to_string());
        }
        let drawn = match ty {
            "draw-mesh" => self.mesh(fields, path, parent, place, opacity),
            "draw-group" => self.group(fields, path, parent, place, opacity),
            "draw-sprite" => self.sprite(fields, path, parent, place, opacity),
            "draw-skinned-mesh" => self.skinned_mesh(fields, path, parent, place, opacity),
            "draw-shape" => self.shape(fields, path, parent, place, opacity),
            "draw-text" => self.text(fields, path, parent, place, opacity),
            "draw-sprite-layer" => self.layer(fields, path, parent, place, opacity, "draw-sprite"),
            "draw-shape-layer" => self.layer(fields, path, parent, place, opacity, "draw-shape"),
            "draw-text-layer" => self.layer(fields, path, parent, place, opacity, "draw-text"),
            "draw-fx-layer" => self.fx_layer(fields, path, parent, place, opacity),
            other => Err(bad(
                &join(path, "type"),
                format!("`{other}` is not a drawable"),
            )),
        };
        self.pick_id = outer;
        drawn
    }

    fn group(
        &mut self,
        f: &Fields,
        path: &str,
        parent: DMat4,
        place: Placement,
        opacity: f64,
    ) -> Result<(), CanvasSceneError> {
        check_keys(f, GROUP_KEYS, path)?;
        if present(f, "clip").is_some() {
            return Err(undrawn(&join(path, "clip"), STEP_PARITY));
        }
        let skeleton = opt_bool(f, "skeleton", path)? == Some(true);
        let bone = match opt_str(f, "bone", path)? {
            Some("") => {
                return Err(bad(
                    &join(path, "bone"),
                    "is empty (the three host mounts no bone)",
                ))
            }
            other => other,
        };
        let world = place.world(required_pos(f, path)?);
        let scale = opt_number(f, "scale", path)?.unwrap_or(1.0);
        let rotate = opt_number(f, "rotate", path)?;
        let rotation = present(f, "rotation")
            .map(|v| triple(v, &join(path, "rotation")))
            .transpose()?;
        if rotate.is_some() && rotation.is_some() {
            return Err(undrawn(
                &format!("{path}.rotate alongside rotation"),
                STEP_PARITY,
            ));
        }
        let rotation = rotation.unwrap_or([0.0, -rotate.unwrap_or(0.0), 0.0]);
        let animated = self.animation(f, path)?;
        if let Some((declared, _)) = &animated {
            // `Group3D` animates the transform alone; a material or opacity track belongs on a leaf mesh.
            for track in MATERIAL_TRACKS {
                if let Some((i, _)) = declared.values(track).first() {
                    return Err(bad(
                        &format!("{path}.animation.keyframes[{i}].{track}"),
                        "a group animates only its transform (the three host ignores it)",
                    ));
                }
            }
        }
        let state = animated.and_then(|(_, state)| state);
        let (offset, delta, scale) = match &state {
            Some(s) => (place.offset(s.offset), s.rotate, scale * s.scale),
            None => (DVec3::ZERO, [0.0; 3], scale),
        };
        let matrix = parent
            * DMat4::from_translation(world + offset)
            * euler_xyz([
                rotation[0] + delta[0],
                rotation[1] + delta[1],
                rotation[2] + delta[2],
            ])
            * DMat4::from_scale(DVec3::splat(scale));
        let opacity = opt_number(f, "opacity", path)?.unwrap_or(1.0) * opacity;
        let items = present(f, "items").ok_or_else(|| bad(&join(path, "items"), "is required"))?;
        if !items.is_array() {
            return Err(bad(&join(path, "items"), "must be an array"));
        }
        let outer_scope = self.scope;
        if skeleton {
            self.scopes.push(HashMap::new());
            self.scope = self.scopes.len() - 1;
        }
        if let Some(name) = bone {
            if self.scopes[self.scope]
                .insert(name.to_string(), matrix)
                .is_some()
            {
                return Err(bad(
                    &join(path, "bone"),
                    format!("`{name}` is already a bone in this skeleton scope (the three host keeps whichever registers last)"),
                ));
            }
        }
        let items_path = join(path, "items");
        for (i, item) in drawable_nodes(items, &items_path)?.iter().enumerate() {
            self.drawable(
                item,
                &format!("{items_path}[{i}]"),
                matrix,
                place.inner(),
                opacity,
            )?;
        }
        self.scope = outer_scope;
        Ok(())
    }

    fn mesh(
        &mut self,
        f: &Fields,
        path: &str,
        parent: DMat4,
        place: Placement,
        group_opacity: f64,
    ) -> Result<(), CanvasSceneError> {
        check_keys(f, MESH_KEYS, path)?;
        let outline = outline(f, path, self.theme)?;
        let world = place.world(required_pos(f, path)?);
        let shape = match present(f, "shape") {
            None => "box",
            Some(v) => v
                .as_str()
                .ok_or_else(|| bad(&join(path, "shape"), "must be a string"))?,
        };
        let dims: &[&str] = match shape {
            "box" => &["width", "height", "depth"],
            "sphere" | "circle" => &["radius", "segments"],
            "capsule" | "cone" => &["radius", "height", "segments"],
            "cylinder" => &["radius", "radiusTop", "radiusBottom", "height", "segments"],
            "torus" => &["radius", "tube", "segments"],
            "plane" => &["width", "depth"],
            "polyhedron" => &[],
            _ => {
                return Err(bad(
                    &join(path, "shape"),
                    format!("`{shape}` is not a mesh shape"),
                ))
            }
        };
        for key in [
            "width",
            "height",
            "depth",
            "radius",
            "radiusTop",
            "radiusBottom",
            "tube",
            "segments",
        ] {
            if present(f, key).is_some() && !dims.contains(&key) {
                return Err(bad(
                    &join(path, key),
                    format!("shape `{shape}` has no {key} (the three host ignores it)"),
                ));
            }
        }
        if shape != "polyhedron" && present(f, "skin").is_some() {
            return Err(bad(
                &join(path, "skin"),
                format!(
                    "only a polyhedron is skinned; shape is `{shape}` (the three host ignores it)"
                ),
            ));
        }
        if shape == "polyhedron" && present(f, "skin").is_some() {
            return self.skinned_polyhedron(f, path, parent, place, group_opacity);
        }
        if shape != "polyhedron" {
            for key in ["vertices", "faces", "uvs"] {
                if present(f, key).is_some_and(|v| !v.as_array().is_some_and(Vec::is_empty)) {
                    return Err(bad(
                        &join(path, key),
                        format!("only a polyhedron has {key}; shape is `{shape}`"),
                    ));
                }
            }
        }

        let cell = place.cell;
        let dim = |key: &str, default: f64| -> Result<f64, CanvasSceneError> {
            Ok(opt_number(f, key, path)?.unwrap_or(default))
        };
        let radius = dim("radius", 0.4)?;
        let r = radius * cell;
        let w = dim("width", 1.0)?;
        let h = dim("height", 1.0)? * cell;
        let d = dim("depth", w)? * cell;
        let w = w * cell;
        let seg = clamp_segments(opt_number(f, "segments", path)?);
        let geo = |result: Result<MeshData, geometry::GeometryError>| {
            result.map_err(|e| bad(path, format!("{shape} geometry: {}", e.0)))
        };
        // `size` is `meshGeometry`'s representative extent, which scales the outline shell; a polyhedron has none
        let (mesh, lift, size) = match shape {
            "box" => (
                geo(geometry::cuboid(w as f32, h as f32, d as f32))?,
                h / 2.0,
                Some(w.max(h).max(d)),
            ),
            "sphere" => (geo(geometry::sphere(r as f32, seg, seg))?, r, Some(r * 2.0)),
            "capsule" => {
                let caps = js_round(seg as f64 / 2.0).max(2.0) as usize;
                (
                    geo(geometry::capsule(r as f32, h as f32, caps, seg))?,
                    h / 2.0 + r,
                    Some(h + r * 2.0),
                )
            }
            "cylinder" => {
                let top = dim("radiusTop", radius)? * cell;
                let bottom = dim("radiusBottom", radius)? * cell;
                (
                    geo(geometry::cylinder(top as f32, bottom as f32, h as f32, seg))?,
                    h / 2.0,
                    Some(h.max(r * 2.0)),
                )
            }
            "cone" => (
                geo(geometry::cylinder(0.0, r as f32, h as f32, seg))?,
                h / 2.0,
                Some(h.max(r * 2.0)),
            ),
            "torus" => {
                let tube = dim("tube", radius / 3.0)? * cell;
                let radial = js_round(seg as f64 / 2.0).max(3.0) as usize;
                (
                    geo(geometry::torus(r as f32, tube as f32, radial, seg))?,
                    r + tube,
                    Some((r + tube) * 2.0),
                )
            }
            "plane" => (
                geo(geometry::plane(w as f32, d as f32))?,
                0.0,
                Some(w.max(d)),
            ),
            "circle" => (geo(geometry::circle(r as f32, seg))?, 0.0, Some(r * 2.0)),
            _ => {
                let (mesh, lift) = polyhedron(f, path, cell)?;
                (mesh, lift, None)
            }
        };
        let lift = match opt_str(f, "pivot", path)? {
            None | Some("bottom") => lift,
            Some("center") => 0.0,
            Some(_) => return Err(bad(&join(path, "pivot"), "must be bottom or center")),
        };
        let [rx, ry, rz] = present(f, "rotation")
            .map(|v| triple(v, &join(path, "rotation")))
            .transpose()?
            .unwrap_or([0.0; 3]);
        let animated = self.animation(f, path)?;
        let state = animated.as_ref().and_then(|(_, state)| state.as_ref());
        let [ax, ay, az] = state.map_or([0.0; 3], |s| s.rotate);
        // three's GROUND_ROTATION lays plane and circle flat; it adds to the authored Euler angles.
        let ground = if matches!(shape, "plane" | "circle") {
            -FRAC_PI_2
        } else {
            0.0
        };
        let oriented = euler_xyz([ground + rx + ax, ry + ay, rz + az]);
        // `Mesh3D` moves and scales its outer group, so the scale also carries the pivot lift.
        let (offset, scale) =
            state.map_or((DVec3::ZERO, 1.0), |s| (place.offset(s.offset), s.scale));
        let group = parent
            * DMat4::from_translation(world + offset)
            * DMat4::from_scale(DVec3::splat(scale))
            * DMat4::from_translation(DVec3::new(0.0, lift, 0.0));
        let transform = group * oriented;

        let material_fields = opt_object(f, "material", path)?;
        let material_opacity = match material_fields {
            Some(m) => opt_number(m, "opacity", &join(path, "material"))?.unwrap_or(1.0),
            None => 1.0,
        };
        let node_opacity = opt_number(f, "opacity", path)?;
        let opacity = state
            .and_then(|s| s.opacity)
            .or(node_opacity)
            .unwrap_or(1.0)
            * material_opacity
            * group_opacity;
        let still_opacity = node_opacity.unwrap_or(1.0) * material_opacity * group_opacity;
        let mut material = material(
            material_fields,
            &join(path, "material"),
            opacity,
            self.theme,
        )?;
        if let Some((declared, _)) = &animated {
            let anim_path = join(path, "animation");
            animated_colors(declared, &anim_path)?;
            let base_emissive = material_fields.is_some_and(|m| present(m, "emissive").is_some());
            for track in ["emissive", "emissiveIntensity"] {
                let Some((i, _)) = declared.values(track).first().copied() else {
                    continue;
                };
                let field = format!("{anim_path}.keyframes[{i}].{track}");
                if material.kind == MaterialKind::Basic {
                    return Err(bad(
                        &field,
                        "material kind `basic` has no emissive (the three host ignores it)",
                    ));
                }
                if track == "emissiveIntensity" && !base_emissive && !declared.declares("emissive")
                {
                    return Err(bad(
                        &field,
                        "has no effect without material.emissive or an emissive track (the three host ignores it)",
                    ));
                }
            }
        }
        if let Some(s) = state {
            if let Some(color) = &s.color {
                material.color = rgb(
                    &Value::String(color.clone()),
                    &join(path, "animation.color"),
                )?;
            }
            if let Some(emissive) = &s.emissive {
                material.emissive = rgb(
                    &Value::String(emissive.clone()),
                    &join(path, "animation.emissive"),
                )?;
            }
            if let Some(intensity) = s.emissive_intensity {
                material.emissive_intensity = intensity as f32;
            }
        }
        // `Mesh3D`'s hull is a sibling of the mesh in the animated group: it keeps the authored rotation and opacity
        let hull = match (&outline, size) {
            (Some(o), Some(size)) if size > 0.0 => Some((
                mesh.clone(),
                group
                    * euler_xyz([ground + rx, ry, rz])
                    * DMat4::from_scale(DVec3::splat(1.0 + o.width * cell / size)),
            )),
            (Some(_), Some(_)) => Some((mesh.clone(), group * euler_xyz([ground + rx, ry, rz]))),
            (Some(o), None) => Some((
                tessellate::inflate(&mesh, (o.width * cell) as f32)
                    .map_err(|e| bad(&join(path, "outline"), e))?,
                group * euler_xyz([ground + rx, ry, rz]),
            )),
            (None, _) => None,
        };
        self.meshes.push(MeshInstance {
            mesh,
            transform: transform.as_mat4().to_cols_array(),
            material,
            cast_shadow: opt_bool(f, "castShadow", path)?.unwrap_or(true),
            receive_shadow: opt_bool(f, "receiveShadow", path)?.unwrap_or(true),
            id: self.pick_id.clone(),
            glow: None,
        });
        if let (Some(o), Some((mesh, at))) = (outline, hull) {
            self.meshes
                .push(o.instance(mesh, at, still_opacity, self.pick_id.clone()));
        }
        Ok(())
    }
}

impl Walk<'_> {
    /// `SkinnedPolyhedron3D`: a `SkinnedMesh` under a group at the mesh's position, bound with an identity bind
    /// matrix in attached mode, so the named bones alone place every vertex.
    fn skinned_polyhedron(
        &mut self,
        f: &Fields,
        path: &str,
        parent: DMat4,
        place: Placement,
        group_opacity: f64,
    ) -> Result<(), CanvasSceneError> {
        for key in ["rotation", "pivot", "animation"] {
            if present(f, key).is_some() {
                return Err(bad(
                    &join(path, key),
                    "a skinned mesh is placed by its bones alone (the three host ignores it)",
                ));
            }
        }
        let world = place.world(required_pos(f, path)?);
        let (mesh, _) = polyhedron(f, path, place.cell)?;
        let skin_path = join(path, "skin");
        let skin =
            opt_object(f, "skin", path)?.ok_or_else(|| bad(&skin_path, "must be an object"))?;
        check_keys(
            skin,
            &["bones", "inverseBindMatrices", "indices", "weights"],
            &skin_path,
        )?;
        let bones = skin_list(skin, "bones", &skin_path)?
            .iter()
            .enumerate()
            .map(|(i, b)| {
                b.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| bad(&format!("{skin_path}.bones[{i}]"), "must be a bone name"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let ibm_path = join(&skin_path, "inverseBindMatrices");
        let mut inverse_bind = Vec::new();
        for (i, m) in skin_list(skin, "inverseBindMatrices", &skin_path)?
            .iter()
            .enumerate()
        {
            let field = format!("{ibm_path}[{i}]");
            let values = m
                .as_array()
                .filter(|v| v.len() == 16)
                .ok_or_else(|| bad(&field, "must be 16 numbers, column-major"))?;
            let mut cols = [0.0; 16];
            for (k, v) in values.iter().enumerate() {
                cols[k] = number(v, &field)?;
            }
            // translations are authored in cells, like the vertices
            for c in &mut cols[12..15] {
                *c *= place.cell;
            }
            inverse_bind.push(DMat4::from_cols_array(&cols));
        }
        if inverse_bind.len() != bones.len() {
            return Err(bad(
                &ibm_path,
                format!(
                    "{} matrices for {} bones (the three host never binds the skin)",
                    inverse_bind.len(),
                    bones.len()
                ),
            ));
        }
        let n = mesh.positions.len();
        let mut joints = Vec::with_capacity(n);
        for (i, row) in skin_rows(skin, "indices", &skin_path, n)?
            .into_iter()
            .enumerate()
        {
            let mut j = [0u16; 4];
            for k in 0..4 {
                if row[k].fract() != 0.0 || !(0.0..=65535.0).contains(&row[k]) {
                    return Err(bad(
                        &format!("{skin_path}.indices[{i}]"),
                        "bone indices must be integers in 0..=65535",
                    ));
                }
                j[k] = row[k] as u16;
            }
            joints.push(j);
        }
        let weights: Vec<[f32; 4]> = skin_rows(skin, "weights", &skin_path, n)?
            .into_iter()
            .map(|row| row.map(|w| w as f32))
            .collect();

        let material_fields = opt_object(f, "material", path)?;
        let material_opacity = match material_fields {
            Some(m) => opt_number(m, "opacity", &join(path, "material"))?.unwrap_or(1.0),
            None => 1.0,
        };
        let opacity =
            opt_number(f, "opacity", path)?.unwrap_or(1.0) * material_opacity * group_opacity;
        let material = material(
            material_fields,
            &join(path, "material"),
            opacity,
            self.theme,
        )?;
        let transform = parent * DMat4::from_translation(world);
        let outline = outline(f, path, self.theme)?;
        self.meshes.push(MeshInstance {
            mesh: mesh.clone(),
            transform: transform.as_mat4().to_cols_array(),
            material,
            cast_shadow: opt_bool(f, "castShadow", path)?.unwrap_or(true),
            receive_shadow: opt_bool(f, "receiveShadow", path)?.unwrap_or(true),
            id: self.pick_id.clone(),
            glow: None,
        });
        let skinned = self.meshes.len() - 1;
        // three draws no hull on a skinned mesh; ours is the posed surface pushed out along its normals
        let hull = outline.map(|o| {
            let width = (o.width * place.cell) as f32;
            self.meshes
                .push(o.instance(mesh, transform, opacity, self.pick_id.clone()));
            (self.meshes.len() - 1, width)
        });
        self.skins.push(PendingSkin {
            hull,
            mesh: skinned,
            scope: self.scope,
            path: path.to_string(),
            bones,
            inverse_bind,
            joints,
            weights,
            bind_inverse: transform.inverse(),
        });
        Ok(())
    }

    /// Pose each skinned mesh by its scope's bones: three's bone matrices are bone world × inverse bind, and the
    /// attached bind mode's `bindMatrixInverse` is the inverse of the mesh's world matrix.
    fn bind_skins(&mut self) -> Result<(), CanvasSceneError> {
        for pending in std::mem::take(&mut self.skins) {
            let scope = &self.scopes[pending.scope];
            let mut bones = Vec::with_capacity(pending.bones.len());
            for (i, name) in pending.bones.iter().enumerate() {
                let world = scope.get(name).ok_or_else(|| {
                    bad(
                        &format!("{}.skin.bones[{i}]", pending.path),
                        format!("no draw-group in this skeleton scope declares bone `{name}` (the three host never binds the skin)"),
                    )
                })?;
                bones.push((*world * pending.inverse_bind[i]).as_mat4());
            }
            let instance = &mut self.meshes[pending.mesh];
            instance.mesh = skin::skin_mesh(
                &instance.mesh,
                &pending.joints,
                &pending.weights,
                &bones,
                Mat4::IDENTITY,
                pending.bind_inverse.as_mat4(),
            )
            .map_err(|e| bad(&join(&pending.path, "skin"), e))?;
            if let Some((hull, width)) = pending.hull {
                let shell = tessellate::inflate(&self.meshes[pending.mesh].mesh, width)
                    .map_err(|e| bad(&join(&pending.path, "outline"), e))?;
                self.meshes[hull].mesh = shell;
            }
        }
        Ok(())
    }

    /// `Sprite3D`: the fallback box when the asset has no url (`spriteAssetState`), a model when it is 3D, else a
    /// flat image.
    fn sprite(
        &mut self,
        f: &Fields,
        path: &str,
        parent: DMat4,
        place: Placement,
        group_opacity: f64,
    ) -> Result<(), CanvasSceneError> {
        check_keys(f, SPRITE_KEYS, path)?;
        let asset_path = join(path, "asset");
        let asset = opt_object(f, "asset", path)?;
        // an asset is a record of the program's art data: its declared fields are read, a row's own extras are its own

        let url = match asset {
            Some(a) => opt_str(a, "url", &asset_path)?.filter(|u| !u.is_empty()),
            None => None,
        };
        let (Some(asset), Some(url)) = (asset, url) else {
            return self.sprite_fallback(f, path, parent, place);
        };
        match opt_str(asset, "dimension", &asset_path)? {
            Some("3d") => self.model_sprite(f, path, parent, place, group_opacity, asset, url),
            None | Some("2d") => {
                self.flat_sprite(f, path, parent, place, group_opacity, asset, url)
            }
            Some(_) => Err(bad(&join(&asset_path, "dimension"), "must be 2d or 3d")),
        }
    }

    /// `Sprite3D` with `asset.dimension: "3d"`: `ModelLoader` at the sprite's position, turned by `rotation` (which
    /// it reads in degrees), scaled to the sprite's size over the model's rest-pose extent, playing `animation`.
    #[allow(clippy::too_many_arguments)]
    fn model_sprite(
        &mut self,
        f: &Fields,
        path: &str,
        parent: DMat4,
        place: Placement,
        group_opacity: f64,
        asset: &Fields,
        url: &str,
    ) -> Result<(), CanvasSceneError> {
        let asset_path = join(path, "asset");
        for key in ["atlas", "sprite"] {
            if present(asset, key).is_some() {
                return Err(bad(
                    &join(&asset_path, key),
                    "a 3D model has no atlas (the three host ignores it)",
                ));
            }
        }
        for key in [
            "anchor", "frame", "loop", "clockMs", "flipX", "opacity", "shadow",
        ] {
            if present(f, key).is_some() {
                return Err(bad(
                    &join(path, key),
                    "a 3D model does not draw it (the three host ignores it)",
                ));
            }
        }
        let width = opt_number(f, "width", path)?;
        let height = opt_number(f, "height", path)?;
        if width.is_none() && height.is_some() {
            return Err(bad(
                &join(path, "height"),
                "a 3D model reads height only alongside width (the three host ignores it)",
            ));
        }
        if group_opacity != 1.0 {
            return Err(bad(
                path,
                "an enclosing draw-group's opacity does not reach a 3D model's own materials (the three host ignores it)",
            ));
        }
        let rotation = opt_number(f, "rotation", path)?.unwrap_or(0.0);
        let animation = opt_str(f, "animation", path)?;
        let at = parent * DMat4::from_translation(place.world(required_pos(f, path)?));

        let model = match model_state(url)
            .map_err(|e| bad(&join(&asset_path, "url"), format!("{url}: {e}")))?
        {
            ModelState::Loading => return Ok(()),
            ModelState::Failed(why) => {
                tracing::warn!(target: "almadar::scene3d", url, %why, "model failed; drawing the error fallback");
                return self.model_fallback(at);
            }
            ModelState::Ready(model) => model,
        };
        // GLTFLoader resolves only once every texture settled; a texture that failed leaves its material unmapped.
        let mut dropped = Vec::new();
        for map in model.map_urls() {
            if asset::image_info(&map).is_some() {
                continue;
            }
            if asset::image_failed(&map) {
                dropped.push(map);
                continue;
            }
            return Ok(());
        }
        let clip = animation.and_then(|name| model.clip_index(name));
        if clip.is_some() {
            self.animated = true;
        }
        let posed = match self.elapsed_ms {
            Some(ms) => model.pose(clip, (ms / 1000.0) as f32),
            None => model.pose(None, 0.0),
        }
        .map_err(|e| bad(&join(&asset_path, "url"), format!("{url}: {e}")))?;
        let norm = match model.rest_bounds() {
            Some((lo, hi)) => {
                let max = f64::from((hi - lo).max_element());
                if !max.is_finite() || max < 0.05 {
                    1.0
                } else {
                    1.0 / max
                }
            }
            None => 1.0,
        };
        let size = match (width, height) {
            (None, _) => DVec3::ONE,
            (Some(w), None) => DVec3::splat(w * place.cell),
            (Some(w), Some(h)) => DVec3::new(w * place.cell, h * place.cell, w * place.cell),
        };
        let world =
            at * DMat4::from_rotation_y(rotation.to_radians()) * DMat4::from_scale(size * norm);
        for mut instance in posed {
            let local = Mat4::from_cols_array(&instance.transform).as_dmat4();
            instance.transform = (world * local).as_mat4().to_cols_array();
            if instance
                .material
                .map
                .as_ref()
                .is_some_and(|m| dropped.contains(m))
            {
                instance.material.map = None;
            }
            instance.id = self.pick_id.clone();
            self.meshes.push(instance);
        }
        Ok(())
    }

    /// `ModelLoader`'s error fallback (`fallbackGeometry: 'box'`): a 0.8 red standard box lifted 0.5, unrotated.
    fn model_fallback(&mut self, at: DMat4) -> Result<(), CanvasSceneError> {
        let mesh = geometry::cuboid(0.8, 0.8, 0.8).map_err(|e| bad("model fallback", e.0))?;
        let color = rgb(&Value::String("#ff4444".into()), "model fallback")?;
        self.meshes.push(MeshInstance {
            mesh,
            transform: (at * DMat4::from_translation(DVec3::new(0.0, 0.5, 0.0)))
                .as_mat4()
                .to_cols_array(),
            material: Material {
                color,
                ..Material::default()
            },
            cast_shadow: false,
            receive_shadow: false,
            id: self.pick_id.clone(),
            glow: None,
        });
        Ok(())
    }

    /// The three host skips `draw-skinned-mesh` in 3D; this draws its 2D semantics on the scene plane instead: the
    /// rig the 2D painter poses (same mesh, bones, pose, clip and frame resolution), its scene `x`/`y` laid on world
    /// X/Z at the position's height, unlit and double-sided as a canvas paints it.
    fn skinned_mesh(
        &mut self,
        f: &Fields,
        path: &str,
        parent: DMat4,
        place: Placement,
        group_opacity: f64,
    ) -> Result<(), CanvasSceneError> {
        check_keys(f, SKINNED_MESH_KEYS, path)?;
        if present(f, "ghost").is_some() {
            return Err(undrawn(
                &format!("{path}.ghost (an onion skin coplanar with the live pass has no depth order in 3D)"),
                STEP_PARITY,
            ));
        }
        let height = required_pos(f, path)?[2];
        let projector = Projector::new(1.0, 0.0, Layout2D::Flat);
        let unit = f64::from(projector.tile_width);
        let Some(commands) =
            skinned_mesh_2d::paint(&Value::Object(f.clone()), &projector, &|x, y| (x, y))
                .map_err(|e| bad(path, e))?
        else {
            return Ok(());
        };
        let mut batches: Vec<(Material, MeshData)> = Vec::new();
        for command in commands {
            let (points, uv, material) = match command {
                DrawCommand::TexturedTriangle {
                    texture,
                    points,
                    uv,
                    uv_in_pixels,
                    opacity,
                } => {
                    let (w, h) = asset::image_info(&texture)
                        .ok_or_else(|| bad(&join(path, "asset.url"), "the texture has no size"))?;
                    let pixel = uv.map(|c| {
                        if uv_in_pixels {
                            c
                        } else {
                            [c[0] * w as f32, c[1] * h as f32]
                        }
                    });
                    let area = (pixel[1][0] - pixel[0][0]) * (pixel[2][1] - pixel[0][1])
                        - (pixel[1][1] - pixel[0][1]) * (pixel[2][0] - pixel[0][0]);
                    // the 2D renderer skips a triangle that cuts no texture area
                    if area.abs() < f32::EPSILON {
                        continue;
                    }
                    let uv = pixel.map(|c| [c[0] / w.max(1) as f32, c[1] / h.max(1) as f32]);
                    let material = Material {
                        kind: MaterialKind::Basic,
                        opacity: (f64::from(opacity.clamp(0.0, 1.0)) * group_opacity) as f32,
                        side: Side::Double,
                        map: Some(texture),
                        ..Material::default()
                    };
                    (points.to_vec(), uv, material)
                }
                DrawCommand::Poly { points, color } => {
                    let material = Material {
                        kind: MaterialKind::Basic,
                        color: srgb_to_linear_rgb([color[0], color[1], color[2]]),
                        opacity: (f64::from(color[3]) * group_opacity) as f32,
                        side: Side::Double,
                        ..Material::default()
                    };
                    (points, [[0.0; 2]; 3], material)
                }
                _ => {
                    return Err(bad(
                        path,
                        "the 2D rig painted something other than a triangle",
                    ))
                }
            };
            if points.len() != 3 {
                return Err(bad(
                    path,
                    "the 2D rig painted something other than a triangle",
                ));
            }
            if batches.last().is_none_or(|(m, _)| *m != material) {
                batches.push((material, MeshData::default()));
            }
            let (_, mesh) = batches.last_mut().expect("a batch was just pushed");
            for (i, p) in points.iter().enumerate() {
                let at = place.world([f64::from(p.x) / unit, f64::from(p.y) / unit, height]);
                mesh.indices.push(mesh.positions.len() as u32);
                mesh.positions.push(f32x3(at.to_array()));
                mesh.normals.push([0.0, 1.0, 0.0]);
                mesh.uvs.push(uv[i]);
            }
        }
        for (material, mesh) in batches {
            self.meshes.push(MeshInstance {
                mesh,
                transform: parent.as_mat4().to_cols_array(),
                material,
                cast_shadow: false,
                receive_shadow: false,
                id: self.pick_id.clone(),
                glow: None,
            });
        }
        Ok(())
    }
}

const SHAPE_KEYS: &[&str] = &[
    "type",
    "id",
    "shape",
    "position",
    "anchor",
    "width",
    "height",
    "radiusX",
    "radiusY",
    "offsetX",
    "offsetY",
    "points",
    "d",
    "fill",
    "gradient",
    "fillPattern",
    "stroke",
    "strokeWidth",
    "strokeGradient",
    "strokeDash",
    "strokeDashOffset",
    "blendMode",
    "blur",
    "rotate",
    "pivot",
    "shadow",
    "animation",
    "opacity",
];

const TEXT_KEYS: &[&str] = &[
    "type", "id", "text", "position", "anchor", "offsetX", "offsetY", "color", "font", "align",
    "baseline", "opacity",
];

const LAYER_KEYS: &[&str] = &["type", "id", "items"];

const FX_LAYER_KEYS: &[&str] = &[
    "type",
    "id",
    "items",
    "presets",
    "art",
    "sprites",
    "tickMs",
    "textColor",
];

/// The shape props a keyframe animates, all drawn here.
const SHAPE_TRACKS: &[&str] = &[
    "offsetX",
    "offsetY",
    "rotate",
    "opacity",
    "radiusX",
    "radiusY",
    "width",
    "height",
    "strokeWidth",
    "fill",
    "stroke",
];

/// `Shape3D` lifts every shape off the ground; a stroke sits a hair above its own fill so the two never depth-fight.
const SHAPE_LIFT: f64 = 0.02;
const STROKE_LIFT: f64 = 0.001;
/// `Shape3D`'s `ringGeometry(radiusX - 0.08, radiusX, 32)` and `circleGeometry(radiusX, 32)`; a cell is 0.95 wide.
const RING_WIDTH: f64 = 0.08;
const SHAPE_SEGMENTS: usize = 32;
const CELL_FILL: f64 = 0.95;
/// A path flattens to within a quarter of the 2D painter's pixel at one cell.
const PATH_TOLERANCE: f32 = 0.25 / TILE_WIDTH;
/// `SpriteBillboard`: a ground decal floats 0.01 off the ground, its loading placeholder 0.02, at 0.6 opacity.
const DECAL_LIFT: f64 = 0.01;
const PLACEHOLDER_LIFT: f64 = 0.02;
const PLACEHOLDER_OPACITY: f64 = 0.6;
/// `Text3D`: drei `<Text>` in a `<Billboard>` 1.2 above the position, font size 0.32, a 0.02 black outline.
const LABEL_LIFT: f64 = 1.2;
const LABEL_EM: f32 = 0.32;
const LABEL_OUTLINE: f32 = 0.02;
/// `DrawFxLayer.tsx`'s declared defaults, and the family the bundled face answers to.

/// `Mesh3D`'s outline: an unlit back-face shell in `color`, standing `width` cells off the surface.
struct Outline {
    color: [f32; 3],
    width: f64,
}

impl Outline {
    fn instance(
        &self,
        mesh: MeshData,
        at: DMat4,
        opacity: f64,
        id: Option<String>,
    ) -> MeshInstance {
        MeshInstance {
            mesh,
            transform: at.as_mat4().to_cols_array(),
            material: Material {
                kind: MaterialKind::Basic,
                color: self.color,
                opacity: opacity as f32,
                side: Side::Back,
                ..Material::default()
            },
            cast_shadow: false,
            receive_shadow: false,
            id,
            glow: None,
        }
    }
}

/// The mesh's authored outline, else the theme's when it draws one (`applySceneMaterials`: the `ink` tone,
/// `--material-outline` × 0.02 wide).
fn outline(
    f: &Fields,
    path: &str,
    theme: &SceneTheme,
) -> Result<Option<Outline>, CanvasSceneError> {
    let field = join(path, "outline");
    match opt_object(f, "outline", path)? {
        Some(o) => {
            check_keys(o, &["color", "width"], &field)?;
            let width = opt_number(o, "width", &field)?.unwrap_or(0.05);
            if width < 0.0 {
                return Err(bad(&join(&field, "width"), format!("{width} is negative")));
            }
            let color = match opt_rgb(o, "color", &field)? {
                Some(c) => c,
                None => rgb(&Value::String("#101014".into()), "outline default")?,
            };
            Ok(Some(Outline { color, width }))
        }
        None if theme.outline > 0.0 => {
            let color = match theme.tones.get("ink") {
                Some(css) => rgb(&Value::String(css.clone()), "theme tone ink")?,
                None => [1.0, 1.0, 1.0],
            };
            Ok(Some(Outline {
                color,
                width: f64::from(theme.outline) * 0.02,
            }))
        }
        None => Ok(None),
    }
}

/// An unlit, double-sided, blended surface: how `Shape3D` and `SpriteBillboard` draw.
fn flat_material(flat: Flat, srgb: [f32; 3], opacity: f64, map: Option<String>) -> Material {
    Material {
        kind: MaterialKind::Flat(flat),
        color: srgb_to_linear_rgb(srgb),
        opacity: opacity as f32,
        side: Side::Double,
        map,
        ..Material::default()
    }
}

fn flat_instance(
    mesh: MeshData,
    at: DMat4,
    material: Material,
    id: Option<String>,
) -> MeshInstance {
    MeshInstance {
        mesh,
        transform: at.as_mat4().to_cols_array(),
        material,
        cast_shadow: false,
        receive_shadow: false,
        id,
        glow: None,
    }
}

/// A shape's `shadow` as straight sRGB RGBA and its blur in cells; `None` when canvas draws none (no blur, since a
/// shadow here has no offset, or a transparent color).
fn shadow_of(f: &Fields, path: &str) -> Result<Option<([f32; 4], f64)>, CanvasSceneError> {
    let Some(v) = present(f, "shadow").filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let field = join(path, "shadow");
    let o = v
        .as_object()
        .ok_or_else(|| bad(&field, "must be {color, blur}"))?;
    check_keys(o, &["color", "blur"], &field)?;
    let color =
        opt_css(o, "color", &field)?.ok_or_else(|| bad(&join(&field, "color"), "is required"))?;
    let blur =
        opt_number(o, "blur", &field)?.ok_or_else(|| bad(&join(&field, "blur"), "is required"))?;
    if !(blur.is_finite() && blur >= 0.0) {
        return Err(bad(
            &join(&field, "blur"),
            format!("{blur} is not a finite blur of zero or more"),
        ));
    }
    Ok((blur > 0.0 && color[3] > 0.0).then_some((color, blur)))
}

/// `a` and `b` as one mesh.
fn merged(mut a: MeshData, b: &MeshData) -> MeshData {
    let base = a.positions.len() as u32;
    a.positions.extend_from_slice(&b.positions);
    a.normals.extend_from_slice(&b.normals);
    a.uvs.extend_from_slice(&b.uvs);
    a.indices.extend(b.indices.iter().map(|i| i + base));
    a
}

/// three's flat shapes are built in XY and laid on the ground facing up: local +Y runs to world −Z.
fn lay_flat() -> DMat4 {
    DMat4::from_rotation_x(-FRAC_PI_2)
}

/// An optional CSS color as straight sRGB RGBA, alpha included.
fn opt_css(f: &Fields, key: &str, path: &str) -> Result<Option<[f32; 4]>, CanvasSceneError> {
    present(f, key)
        .map(|v| css(v, &join(path, key)))
        .transpose()
}

fn tess<'a>(path: &'a str, what: &str) -> impl Fn(tessellate::TessError) -> CanvasSceneError + 'a {
    let what = what.to_owned();
    move |e| bad(path, format!("{what}: {e}"))
}

/// A shape `animation`: the keyframe contract `DrawShape` plays, its tracks restricted to what draws here.
fn shape_animation(value: &Value, path: &str) -> Result<(), CanvasSceneError> {
    let a = value
        .as_object()
        .ok_or_else(|| bad(path, "must be an object"))?;
    check_keys(a, &["durationMs", "loop", "keyframes"], path)?;
    let duration = opt_number(a, "durationMs", path)?
        .ok_or_else(|| bad(&join(path, "durationMs"), "is required"))?;
    if duration <= 0.0 {
        return Err(bad(
            &join(path, "durationMs"),
            format!("{duration} must be above zero (the three host does not play it)"),
        ));
    }
    opt_bool(a, "loop", path)?;
    let frames_path = join(path, "keyframes");
    let frames = present(a, "keyframes")
        .and_then(Value::as_array)
        .ok_or_else(|| bad(&frames_path, "is required as an array"))?;
    if frames.is_empty() {
        return Err(bad(
            &frames_path,
            "is empty (the three host does not play it)",
        ));
    }
    for (i, frame) in frames.iter().enumerate() {
        let frame_path = format!("{frames_path}[{i}]");
        let k = frame
            .as_object()
            .ok_or_else(|| bad(&frame_path, "must be an object"))?;
        if present(k, "at").is_none() {
            return Err(bad(&join(&frame_path, "at"), "is required"));
        }
        for (key, v) in k {
            let field = join(&frame_path, key);
            match key.as_str() {
                "fill" | "stroke" => {
                    css(v, &field)?;
                }
                "at" => {
                    number(v, &field)?;
                }
                "strokeDashOffset" | "blur" | "shadow" => {
                    return Err(undrawn(&field, STEP_PARITY));
                }
                track if SHAPE_TRACKS.contains(&track) => {
                    number(v, &field)?;
                }
                _ => return Err(bad(&field, "not a declared keyframe prop")),
            }
        }
    }
    Ok(())
}

/// A sprite `frame`: `{x, y, w, h}` image pixels, a positive size.
fn frame_rect(f: &Fields, path: &str) -> Result<[f64; 4], CanvasSceneError> {
    check_keys(f, &["x", "y", "w", "h"], path)?;
    let mut out = [0.0; 4];
    for (i, key) in ["x", "y", "w", "h"].into_iter().enumerate() {
        out[i] = opt_number(f, key, path)?.ok_or_else(|| bad(&join(path, key), "is required"))?;
    }
    if out[2] <= 0.0 || out[3] <= 0.0 {
        return Err(bad(path, "w and h must be above zero"));
    }
    Ok(out)
}

/// Where a flat sprite's pixels come from, in the 2D painter's order.
enum Source {
    Rect([f64; 4]),
    Whole,
    /// The atlas is still loading.
    Pending,
}

impl Walk<'_> {
    /// The clock a sprite sheet plays on when the sprite names none: the page's wall clock as the 2D painter uses it,
    /// else the canvas clock, else rest.
    fn paint_clock(&self) -> f64 {
        self.epoch_ms.or(self.elapsed_ms).unwrap_or(0.0)
    }

    /// `Sprite3D`'s unresolvable-asset fallback: a `#9b8f7f` standard box at the position, `width` (0.6) square and
    /// `height` (0.6) tall.
    fn sprite_fallback(
        &mut self,
        f: &Fields,
        path: &str,
        parent: DMat4,
        place: Placement,
    ) -> Result<(), CanvasSceneError> {
        let side = opt_number(f, "width", path)?.unwrap_or(0.6) * place.cell;
        let height = opt_number(f, "height", path)?.unwrap_or(0.6) * place.cell;
        let mesh = geometry::cuboid(side as f32, height as f32, side as f32)
            .map_err(|e| bad(path, format!("fallback box: {}", e.0)))?;
        let color = rgb(&Value::String("#9b8f7f".into()), "sprite fallback")?;
        self.meshes.push(MeshInstance {
            mesh,
            transform: (parent * DMat4::from_translation(place.world(required_pos(f, path)?)))
                .as_mat4()
                .to_cols_array(),
            material: Material {
                color,
                ..Material::default()
            },
            cast_shadow: false,
            receive_shadow: false,
            id: self.pick_id.clone(),
            glow: None,
        });
        Ok(())
    }

    /// `SpriteBillboard`: a `top-left` sprite (the default) is a ground decal `width` × `height` cells with its corner
    /// at the position; a `ground` or `center` one stands up as a camera-facing billboard `height` cells tall and
    /// `width` (or the image's aspect) wide. Nearest-sampled, cut out under alpha 0.1, grey while loading and red when
    /// the image fails. Beyond three: the 2D painter's frame/atlas/sheet source, `rotation` about the center,
    /// `flipX`, and the enclosing group's opacity on the billboard too.
    #[allow(clippy::too_many_arguments)]
    fn flat_sprite(
        &mut self,
        f: &Fields,
        path: &str,
        parent: DMat4,
        place: Placement,
        group_opacity: f64,
        asset: &Fields,
        url: &str,
    ) -> Result<(), CanvasSceneError> {
        let asset_path = join(path, "asset");
        if present(f, "shadow").is_some() {
            return Err(undrawn(&join(path, "shadow"), STEP_PARITY));
        }
        let decal = match opt_str(f, "anchor", path)? {
            None | Some("top-left") => true,
            Some("ground" | "center") => false,
            Some(_) => {
                return Err(bad(
                    &join(path, "anchor"),
                    "must be top-left, ground or center",
                ))
            }
        };
        let looping = opt_bool(f, "loop", path)?;
        let clock_ms = opt_number(f, "clockMs", path)?;
        let flip = opt_bool(f, "flipX", path)?.unwrap_or(false);
        let animation = opt_str(f, "animation", path)?;
        let sprite_name = opt_str(asset, "sprite", &asset_path)?;
        let source = match opt_object(f, "frame", path)? {
            Some(frame) => Source::Rect(frame_rect(frame, &join(path, "frame"))?),
            None => match opt_str(asset, "atlas", &asset_path)?.filter(|u| !u.is_empty()) {
                None => Source::Whole,
                Some(atlas_url) => match atlas::parsed_atlas(atlas_url) {
                    None if asset::fetch_failed(atlas_url)
                        || asset::asset_bytes(atlas_url).is_some() =>
                    {
                        return Err(bad(
                            &join(&asset_path, "atlas"),
                            format!("`{atlas_url}` does not load as an atlas"),
                        ))
                    }
                    None => Source::Pending,
                    Some(parsed) => match (&*parsed, animation, sprite_name) {
                        (atlas::ParsedAtlas::SpriteSheet(_), Some(row), _) => {
                            if clock_ms.is_none() {
                                self.animated = true;
                            }
                            let at = clock_ms.unwrap_or_else(|| self.paint_clock());
                            let r = atlas::sprite_sheet_frame(&parsed, row, looping, at)
                                .ok_or_else(|| {
                                    bad(
                                        &join(path, "animation"),
                                        format!("atlas `{atlas_url}` has no animation `{row}`"),
                                    )
                                })?;
                            Source::Rect([r.x, r.y, r.w, r.h].map(f64::from))
                        }
                        (_, _, Some(name)) => {
                            let r = atlas::sub_rect_for(&parsed, name).ok_or_else(|| {
                                bad(
                                    &join(&asset_path, "sprite"),
                                    format!("atlas `{atlas_url}` has no sprite `{name}`"),
                                )
                            })?;
                            Source::Rect([r.x, r.y, r.w, r.h].map(f64::from))
                        }
                        _ => Source::Whole,
                    },
                },
            },
        };
        let dims = asset::image_info(url);
        let failed = dims.is_none() && asset::image_failed(url);
        let (iw, ih) = dims.map_or((1.0, 1.0), |(w, h)| (f64::from(w), f64::from(h)));
        let crop = match source {
            Source::Rect(r) => Some(r),
            Source::Whole | Source::Pending => None,
        };
        let cell = place.cell;
        let width = opt_number(f, "width", path)?;
        let height = opt_number(f, "height", path)?;
        let (w, h) = if decal {
            (width.unwrap_or(1.0) * cell, height.unwrap_or(1.0) * cell)
        } else {
            let tall = height.unwrap_or(1.0);
            let aspect = crop.map_or(iw / ih, |r| r[2] / r[3]);
            (width.unwrap_or(tall * aspect) * cell, tall * cell)
        };
        let rotation = opt_number(f, "rotation", path)?.unwrap_or(0.0);
        let world = place.world(required_pos(f, path)?);
        let at = if decal {
            let lift = if dims.is_some() && !matches!(source, Source::Pending) {
                DECAL_LIFT
            } else {
                PLACEHOLDER_LIFT
            };
            parent
                * DMat4::from_translation(DVec3::new(world.x + w / 2.0, lift, world.z + h / 2.0))
                * DMat4::from_rotation_y(-rotation)
                * lay_flat()
        } else {
            parent * DMat4::from_translation(world + DVec3::new(0.0, h / 2.0, 0.0))
        };
        let quad = |uv: [f32; 4]| {
            let mut mesh =
                tessellate::quad(w as f32, h as f32, uv).map_err(tess(path, "sprite"))?;
            if !decal {
                tessellate::rotate(&mut mesh, -rotation as f32);
            }
            Ok::<_, CanvasSceneError>(mesh)
        };
        let ready = dims.is_some() && !matches!(source, Source::Pending);
        if !ready {
            if failed {
                tracing::warn!(target: "almadar::scene3d", url, "sprite image failed; drawing the error placeholder");
            }
            let hex = if failed { "#ff4444" } else { "#888888" };
            let color = css(&Value::String(hex.into()), "sprite placeholder")?;
            let material = flat_material(
                Flat::default(),
                [color[0], color[1], color[2]],
                PLACEHOLDER_OPACITY,
                None,
            );
            self.meshes.push(flat_instance(
                quad([0.0, 0.0, 1.0, 1.0])?,
                at,
                material,
                self.pick_id.clone(),
            ));
            return Ok(());
        }
        let [x0, y0, x1, y1] = match crop {
            Some([x, y, cw, ch]) => [x / iw, y / ih, (x + cw) / iw, (y + ch) / ih],
            None => [0.0, 0.0, 1.0, 1.0],
        };
        let uv = if flip {
            [x1, y0, x0, y1]
        } else {
            [x0, y0, x1, y1]
        };
        let opacity = opt_number(f, "opacity", path)?.unwrap_or(1.0) * group_opacity;
        let flat = Flat {
            nearest: true,
            cutout: true,
            additive: false,
            billboard: !decal,
        };
        let material = flat_material(flat, [1.0; 3], opacity, Some(url.to_owned()));
        self.meshes.push(flat_instance(
            quad(uv.map(|v| v as f32))?,
            at,
            material,
            self.pick_id.clone(),
        ));
        Ok(())
    }

    /// `Shape3D`: the shape built flat and laid on the ground 0.02 above the position, unlit, double-sided, blended,
    /// in `fill` (else `stroke`, else white): a cell 0.95 wide, a `width` × `height` rect, an ellipse of `radiusX`
    /// (a 0.08 ring when only stroked), a `points` polygon; and `path` (`d`), which three skips, filled by the nonzero
    /// rule and stroked as the 2D painter defines it. Beyond three: `radiusY`, `offsetX`/`offsetY` and `rotate` about
    /// `pivot` (as three's own animation moves the shape), a stroke drawn alongside a fill, `strokeWidth` in painter
    /// pixels (one cell is `TILE_WIDTH` of them), `lighter` blending, and every keyframe track the 2D painter plays.
    fn shape(
        &mut self,
        f: &Fields,
        path: &str,
        parent: DMat4,
        place: Placement,
        group_opacity: f64,
    ) -> Result<(), CanvasSceneError> {
        check_keys(f, SHAPE_KEYS, path)?;
        for key in [
            "gradient",
            "fillPattern",
            "strokeGradient",
            "strokeDashOffset",
            "blur",
        ] {
            if present(f, key).is_some() {
                return Err(undrawn(&join(path, key), STEP_PARITY));
            }
        }
        if present(f, "strokeDash").is_some_and(|v| !v.as_array().is_some_and(Vec::is_empty)) {
            return Err(undrawn(&join(path, "strokeDash"), STEP_PARITY));
        }
        let additive = match opt_str(f, "blendMode", path)? {
            None | Some("source-over") => false,
            Some("lighter") => true,
            Some(_) => return Err(undrawn(&join(path, "blendMode"), STEP_PARITY)),
        };
        match opt_str(f, "anchor", path)? {
            None | Some("center") => {}
            Some(_) => {
                return Err(bad(
                    &join(path, "anchor"),
                    "a 3D shape is centered on its position (only `center` places it so)",
                ))
            }
        }
        let world = place.world(required_pos(f, path)?);
        let animated;
        let view: &Fields = match present(f, "animation") {
            None => f,
            Some(declared) => {
                shape_animation(declared, &join(path, "animation"))?;
                self.animated = true;
                match self.elapsed_ms {
                    None => f,
                    Some(t) => {
                        let mut at = f.clone();
                        for key in SHAPE_TRACKS {
                            if let Some(v) = crate::animate::drawable_track(declared, t, key) {
                                at.insert((*key).to_owned(), v);
                            }
                        }
                        animated = at;
                        &animated
                    }
                }
            }
        };
        let shape = opt_str(view, "shape", path)?
            .ok_or_else(|| bad(&join(path, "shape"), "is required"))?;
        let own: &[&str] = match shape {
            "cell" => &[],
            "rect" => &["width", "height"],
            "ellipse" => &["radiusX", "radiusY"],
            "poly" => &["points"],
            "path" => &["d"],
            _ => {
                return Err(bad(
                    &join(path, "shape"),
                    format!("`{shape}` is not a shape"),
                ))
            }
        };
        for key in ["width", "height", "radiusX", "radiusY", "points", "d"] {
            if present(view, key).is_some_and(|v| !v.as_array().is_some_and(Vec::is_empty))
                && !own.contains(&key)
            {
                return Err(bad(
                    &join(path, key),
                    format!("shape `{shape}` has no {key}"),
                ));
            }
        }
        let cell = place.cell;
        let num = |key: &str| opt_number(view, key, path);
        let fill = opt_css(view, "fill", path)?;
        let stroke = opt_css(view, "stroke", path)?;
        let stroke_width = num("strokeWidth")?;
        if let Some(sw) = stroke_width {
            if sw <= 0.0 {
                return Err(bad(
                    &join(path, "strokeWidth"),
                    format!("{sw} is not above zero"),
                ));
            }
            if stroke.is_none() {
                return Err(bad(
                    &join(path, "strokeWidth"),
                    "sizes no stroke (stroke is unset)",
                ));
            }
            if fill.is_none() && matches!(shape, "cell" | "rect" | "poly") {
                return Err(bad(
                    &join(path, "strokeWidth"),
                    format!("a stroke-only {shape} is filled in its stroke color (three's Shape3D), so a width sizes nothing"),
                ));
            }
        }
        // painter pixels to world units: one cell is TILE_WIDTH of them
        let band = stroke_width.unwrap_or(1.0) * cell / f64::from(TILE_WIDTH);
        let outline_of = |points: Vec<[f32; 2]>| {
            tessellate::stroke(
                &[Subpath {
                    points,
                    closed: true,
                }],
                band as f32,
            )
            .map_err(tess(path, shape))
        };
        let rect_points = |w: f64, h: f64| {
            let (x, y) = ((w / 2.0) as f32, (h / 2.0) as f32);
            vec![[-x, -y], [x, -y], [x, y], [-x, y]]
        };
        let (body, edge) = match shape {
            "cell" | "rect" => {
                let (w, h) = if shape == "cell" {
                    (cell * CELL_FILL, cell * CELL_FILL)
                } else {
                    (
                        num("width")?.unwrap_or(1.0) * cell,
                        num("height")?.unwrap_or(1.0) * cell,
                    )
                };
                // a zero-wide rect is three's degenerate PlaneGeometry: nothing drawn, no error
                if w == 0.0 || h == 0.0 {
                    (None, None)
                } else {
                    let body = geometry::plane(w as f32, h as f32)
                        .map_err(|e| bad(path, format!("{shape}: {}", e.0)))?;
                    let edge = match (fill, stroke) {
                        (Some(_), Some(_)) => Some(outline_of(rect_points(w, h))?),
                        _ => None,
                    };
                    (Some(body), edge)
                }
            }
            "ellipse" => {
                let rx = num("radiusX")?.unwrap_or(0.4);
                let ry = num("radiusY")?.unwrap_or(rx);
                match (fill, stroke) {
                    (None, Some(_)) => {
                        let width = stroke_width.map_or(RING_WIDTH, |_| band);
                        let ring = tessellate::ring(
                            [(rx - width).max(0.0) as f32, (ry - width).max(0.0) as f32],
                            [rx as f32, ry as f32],
                            SHAPE_SEGMENTS,
                        )
                        .map_err(tess(path, shape))?;
                        (Some(ring), None)
                    }
                    _ => {
                        let body = tessellate::ellipse(rx as f32, ry as f32, SHAPE_SEGMENTS)
                            .map_err(tess(path, shape))?;
                        let edge = match (fill, stroke) {
                            (Some(_), Some(_)) => Some(
                                tessellate::ring(
                                    [
                                        (rx - band / 2.0).max(0.0) as f32,
                                        (ry - band / 2.0).max(0.0) as f32,
                                    ],
                                    [(rx + band / 2.0) as f32, (ry + band / 2.0) as f32],
                                    SHAPE_SEGMENTS,
                                )
                                .map_err(tess(path, shape))?,
                            ),
                            _ => None,
                        };
                        (Some(body), edge)
                    }
                }
            }
            "poly" => {
                let p_path = join(path, "points");
                let list = present(view, "points")
                    .and_then(Value::as_array)
                    .ok_or_else(|| bad(&p_path, "is required as an array"))?;
                if list.len() < 3 {
                    return Err(bad(
                        &p_path,
                        format!("{} points enclose no area (a poly needs 3)", list.len()),
                    ));
                }
                let mut points = Vec::with_capacity(list.len());
                for (i, p) in list.iter().enumerate() {
                    let field = format!("{p_path}[{i}]");
                    let o = p.as_object().ok_or_else(|| bad(&field, "must be {x, y}"))?;
                    check_keys(o, &["x", "y"], &field)?;
                    let axis = |k: &str| {
                        opt_number(o, k, &field)?
                            .ok_or_else(|| bad(&join(&field, k), "is required"))
                    };
                    points.push([axis("x")? as f32, axis("y")? as f32]);
                }
                let body = tessellate::fill(&[points.clone()]).map_err(tess(path, shape))?;
                let edge = match (fill, stroke) {
                    (Some(_), Some(_)) => Some(outline_of(points)?),
                    _ => None,
                };
                (Some(body), edge)
            }
            _ => {
                let d = opt_str(view, "d", path)?
                    .ok_or_else(|| bad(&join(path, "d"), "is required"))?;
                let segments = parse_path_data(d, Xf::identity()).ok_or_else(|| {
                    bad(
                        &join(path, "d"),
                        "is not SVG path data the 2D painter reads",
                    )
                })?;
                let subpaths =
                    tessellate::flatten(&segments, PATH_TOLERANCE).map_err(tess(path, shape))?;
                let body = match (fill, stroke) {
                    (None, Some(_)) => None,
                    _ => {
                        let rings: Vec<Vec<[f32; 2]>> =
                            subpaths.iter().map(|s| s.points.clone()).collect();
                        Some(tessellate::fill(&rings).map_err(tess(path, shape))?)
                    }
                };
                let edge = match stroke {
                    Some(_) => Some(
                        tessellate::stroke(&subpaths, band as f32).map_err(tess(path, shape))?,
                    ),
                    None => None,
                };
                (body, edge)
            }
        };
        let offset = DVec3::new(
            num("offsetX")?.unwrap_or(0.0) * cell,
            SHAPE_LIFT,
            num("offsetY")?.unwrap_or(0.0) * cell,
        );
        let pivot = match opt_object(f, "pivot", path)? {
            None => DVec3::ZERO,
            Some(p) => {
                let field = join(path, "pivot");
                check_keys(p, &["x", "y"], &field)?;
                let axis = |k: &str| {
                    opt_number(p, k, &field)?.ok_or_else(|| bad(&join(&field, k), "is required"))
                };
                DVec3::new((axis("x")? - 0.5) * cell, 0.0, (axis("y")? - 0.5) * cell)
            }
        };
        let at = parent
            * DMat4::from_translation(world + offset + pivot)
            * DMat4::from_rotation_y(-num("rotate")?.unwrap_or(0.0))
            * DMat4::from_translation(-pivot)
            * lay_flat();
        let opacity = num("opacity")?.unwrap_or(1.0) * group_opacity;
        let flat = Flat {
            additive,
            ..Flat::default()
        };
        let paint =
            |c: [f32; 4]| flat_material(flat, [c[0], c[1], c[2]], opacity * f64::from(c[3]), None);
        let white = [1.0; 4];
        if let Some((color, blur)) = shadow_of(view, path)? {
            let coverage = body
                .iter()
                .chain(edge.iter())
                .fold(MeshData::default(), merged);
            if !coverage.indices.is_empty() {
                // canvas shadowBlur is twice the Gaussian sigma; `blur` is in cells
                let mut glow = flat_instance(coverage, at, paint(color), None);
                glow.glow = Some((blur * cell / 2.0) as f32);
                self.meshes.push(glow);
            }
        }
        if let Some(mesh) = body {
            let color = match shape {
                "path" => fill.unwrap_or(white),
                _ => fill.or(stroke).unwrap_or(white),
            };
            self.meshes
                .push(flat_instance(mesh, at, paint(color), self.pick_id.clone()));
        }
        if let (Some(mesh), Some(color)) = (edge, stroke) {
            let lifted = at * DMat4::from_translation(DVec3::new(0.0, 0.0, STROKE_LIFT));
            self.meshes.push(flat_instance(
                mesh,
                lifted,
                paint(color),
                self.pick_id.clone(),
            ));
        }
        Ok(())
    }

    /// `Text3D`: drei `<Text>` (size 0.32, centered, a 0.02 black outline) in a camera-facing `<Billboard>` 1.2 above
    /// the position, in `color` (white when unset). The glyphs are the 2D painter's bundled faces, not troika's
    /// Roboto. Beyond three: `opacity`, the 2D `font` (its pixels at `TILE_WIDTH` per cell) and weight, `align`,
    /// `baseline`, and `offsetX`/`offsetY` in cells across and down the label's own plane.
    fn text(
        &mut self,
        f: &Fields,
        path: &str,
        parent: DMat4,
        place: Placement,
        group_opacity: f64,
    ) -> Result<(), CanvasSceneError> {
        check_keys(f, TEXT_KEYS, path)?;
        let world = place.world(required_pos(f, path)?);
        match opt_str(f, "anchor", path)? {
            None | Some("center") => {}
            Some(_) => {
                return Err(bad(
                    &join(path, "anchor"),
                    "a 3D label stands over its position (only `center` places it so)",
                ))
            }
        }
        let color = match opt_str(f, "color", path)? {
            None | Some("") => [1.0; 4],
            Some(c) => css(&Value::String(c.to_owned()), &join(path, "color"))?,
        };
        let (em, bold, mono) = match opt_str(f, "font", path)? {
            None => (LABEL_EM, false, false),
            Some(font) => {
                let (px, weight, mono) = super::game_canvas_2d::parse_text_font(Some(font)).ok_or_else(|| {
                    bad(
                        &join(path, "font"),
                        format!("`{font}` is not a font the bundled faces draw"),
                    )
                })?;
                (
                    (f64::from(px) * place.cell / f64::from(TILE_WIDTH)) as f32,
                    weight >= 600,
                    mono,
                )
            }
        };
        let across = match opt_str(f, "align", path)? {
            None | Some("center") => 0.0,
            Some("left" | "start") => 0.5,
            Some("right" | "end") => -0.5,
            Some(_) => {
                return Err(bad(
                    &join(path, "align"),
                    "must be center, left, start, right or end",
                ))
            }
        };
        let baseline = opt_str(f, "baseline", path)?;
        if let Some(b) =
            baseline.filter(|b| !matches!(*b, "middle" | "alphabetic" | "top" | "bottom"))
        {
            return Err(match b {
                "hanging" | "ideographic" => undrawn(&join(path, "baseline"), STEP_PARITY),
                _ => bad(&join(path, "baseline"), "not a declared baseline"),
            });
        }
        let opacity = opt_number(f, "opacity", path)?.unwrap_or(1.0) * group_opacity;
        let (ox, oy) = (
            opt_number(f, "offsetX", path)?.unwrap_or(0.0) * place.cell,
            opt_number(f, "offsetY", path)?.unwrap_or(0.0) * place.cell,
        );
        let text = opt_str(f, "text", path)?.unwrap_or("");
        if text.is_empty() {
            return Ok(());
        }
        let style = LabelStyle {
            em,
            bold,
            mono,
            color,
            halo: LABEL_OUTLINE,
            halo_color: [0.0, 0.0, 0.0, 1.0],
        };
        let l = label(text, &style).map_err(|e| bad(&join(path, "text"), e))?;
        let (asc, desc) = (l.ascent, l.descent);
        let up = match baseline {
            Some("alphabetic") => (asc + desc) / 2.0,
            Some("top") => -(asc - desc) / 2.0,
            Some("bottom") => (asc - desc) / 2.0,
            _ => 0.0,
        };
        let mut mesh = tessellate::quad(l.width, l.height, [0.0, 0.0, 1.0, 1.0])
            .map_err(tess(path, "label"))?;
        tessellate::translate(
            &mut mesh,
            [across * l.text_width + ox as f32, up - oy as f32],
        );
        let flat = Flat {
            billboard: true,
            ..Flat::default()
        };
        let at = parent * DMat4::from_translation(world + DVec3::new(0.0, LABEL_LIFT, 0.0));
        let material = flat_material(flat, [1.0; 3], opacity, Some(l.key));
        self.meshes
            .push(flat_instance(mesh, at, material, self.pick_id.clone()));
        Ok(())
    }

    /// `draw-sprite-layer` / `draw-shape-layer` / `draw-text-layer`: each item drawn as its atom, picked by its own id.
    #[allow(clippy::too_many_arguments)]
    fn layer(
        &mut self,
        f: &Fields,
        path: &str,
        parent: DMat4,
        place: Placement,
        opacity: f64,
        atom: &str,
    ) -> Result<(), CanvasSceneError> {
        check_keys(f, LAYER_KEYS, path)?;
        let items_path = join(path, "items");
        let items = present(f, "items")
            .ok_or_else(|| bad(&items_path, "is required"))?
            .as_array()
            .ok_or_else(|| bad(&items_path, "must be an array"))?;
        let outer = self.pick_id.clone();
        for (i, item) in items.iter().enumerate() {
            let item_path = format!("{items_path}[{i}]");
            let fields = item
                .as_object()
                .ok_or_else(|| bad(&item_path, "not a drawable node"))?;
            if fields.get("type").and_then(Value::as_str) != Some(atom) {
                return Err(bad(&join(&item_path, "type"), format!("must be `{atom}`")));
            }
            self.pick_id = fields
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| outer.clone());
            let drawn = match atom {
                "draw-sprite" => self.sprite(fields, &item_path, parent, place, opacity),
                "draw-shape" => self.shape(fields, &item_path, parent, place, opacity),
                _ => self.text(fields, &item_path, parent, place, opacity),
            };
            self.pick_id = outer.clone();
            drawn?;
        }
        Ok(())
    }

    /// `FxLayer3D`: the layer expanded by the shared projection (`fx_projection`, as the 2D painter expands it) on the
    /// page clock, its pieces drawn as their atoms; a live layer keeps the host mapping frames.
    fn fx_layer(
        &mut self,
        f: &Fields,
        path: &str,
        parent: DMat4,
        place: Placement,
        opacity: f64,
    ) -> Result<(), CanvasSceneError> {
        check_keys(f, FX_LAYER_KEYS, path)?;
        let layer = crate::fx_projection::with_host_defaults(f);
        if layer["items"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
        {
            self.animated = true;
        }
        let pieces = crate::fx_projection::expand_layer(
            &layer,
            self.epoch_ms.unwrap_or(0.0),
            "3d",
            f64::from(TILE_WIDTH),
        )?;
        for (i, piece) in pieces.iter().enumerate() {
            self.drawable(piece, &format!("{path}.fx[{i}]"), parent, place, opacity)?;
        }
        Ok(())
    }
}

fn skin_list<'a>(
    skin: &'a Fields,
    key: &str,
    path: &str,
) -> Result<&'a Vec<Value>, CanvasSceneError> {
    present(skin, key)
        .and_then(Value::as_array)
        .ok_or_else(|| bad(&join(path, key), "is required as an array"))
}

/// Per-vertex rows of up to four numbers; three reads `[k] ?? 0`, so a short row pads with zeros.
fn skin_rows(
    skin: &Fields,
    key: &str,
    path: &str,
    vertices: usize,
) -> Result<Vec<[f64; 4]>, CanvasSceneError> {
    let field = join(path, key);
    let items = skin_list(skin, key, path)?;
    if items.len() != vertices {
        return Err(bad(
            &field,
            format!(
                "{} entries for {vertices} vertices (the three host drops the skin)",
                items.len()
            ),
        ));
    }
    items
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let row_field = format!("{field}[{i}]");
            let row = row.as_array().filter(|r| r.len() <= 4).ok_or_else(|| {
                bad(
                    &row_field,
                    "must be up to 4 numbers (the three host reads four)",
                )
            })?;
            let mut out = [0.0; 4];
            for (k, v) in row.iter().enumerate() {
                out[k] = number(v, &row_field)?;
            }
            Ok(out)
        })
        .collect()
}

/// `collectDrawnItems` over the top-level list: each drawable's scene position and hit id for the bounds and the
/// hit index, a layer's items by their own, an fx layer none (its effects are never click targets). A skinned mesh's
/// position is offset by its posed hull's local minimum (`skinnedMeshDrawnItem`).
fn drawn_items(
    nodes: &[(String, Value)],
) -> Result<Vec<([f64; 3], Option<String>)>, CanvasSceneError> {
    let id_of = |f: &Fields| f.get("id").and_then(Value::as_str).map(str::to_string);
    let mut out = Vec::new();
    for (path, node) in nodes {
        let Some(f) = node.as_object() else { continue };
        match f.get("type").and_then(Value::as_str) {
            Some("draw-mesh" | "draw-group" | "draw-sprite" | "draw-shape" | "draw-text") => {
                out.push((required_pos(f, path)?, id_of(f)));
            }
            Some("draw-skinned-mesh") => out.push((skinned_mesh_pos(f, path)?, id_of(f))),
            Some("draw-sprite-layer" | "draw-shape-layer" | "draw-text-layer") => {
                let items = present(f, "items").and_then(Value::as_array);
                for (i, item) in items.into_iter().flatten().enumerate() {
                    // a malformed item is refused by name when the walk draws it
                    let Some(it) = item.as_object() else { continue };
                    out.push((required_pos(it, &format!("{path}.items[{i}]"))?, id_of(it)));
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

fn skinned_mesh_pos(f: &Fields, path: &str) -> Result<[f64; 3], CanvasSceneError> {
    let pos = required_pos(f, path)?;
    let bones = present(f, "bones")
        .and_then(Value::as_array)
        .filter(|b| !b.is_empty());
    let (Some(mesh), Some(bones)) = (present(f, "mesh"), bones) else {
        return Ok(pos);
    };
    let parse = |e: serde_json::Error| bad(path, format!("draw-skinned-mesh: {e}"));
    let mesh: SkinMesh = serde_json::from_value(mesh.clone()).map_err(parse)?;
    let bones: Vec<BoneDef> = serde_json::from_value(Value::Array(bones.clone())).map_err(parse)?;
    let frames: Option<Vec<Pose>> = present(f, "clip")
        .and_then(|c| c.get("frames"))
        .map(|v| serde_json::from_value(v.clone()))
        .transpose()
        .map_err(parse)?;
    let frame = present(f, "frame").and_then(Value::as_u64);
    let pose = match (frames.as_deref(), frame) {
        (Some(frames), Some(i)) if !frames.is_empty() => frames[i as usize % frames.len()].clone(),
        _ => present(f, "pose")
            .map(|p| serde_json::from_value::<Pose>(p.clone()))
            .transpose()
            .map_err(parse)?
            .unwrap_or_default(),
    };
    let skin_error = |e: skinning_2d::SkinError| bad(path, format!("draw-skinned-mesh: {e}"));
    let bind = skinning_2d::compute_world_matrices(&bones, &Pose::new()).map_err(skin_error)?;
    let current = skinning_2d::compute_world_matrices(&bones, &pose).map_err(skin_error)?;
    let points = skinning_2d::skin_vertices(&mesh, &bind, &current).map_err(skin_error)?;
    Ok(match skinning_2d::vertex_bounds(&points) {
        Some(b) => [pos[0] + b[0], pos[1] + b[1], pos[2]],
        None => pos,
    })
}

/// `DrawMesh3D.polyhedronGeometry`: scene `[x, y, z]` (z up) to three `[x, z, y]` scaled by the cell; that swap
/// mirrors space, so each face `[a, b, c]` is emitted `[a, c, b]` to keep it outward.
fn polyhedron(f: &Fields, path: &str, cell: f64) -> Result<(MeshData, f64), CanvasSceneError> {
    let v_path = join(path, "vertices");
    let vertices = present(f, "vertices")
        .and_then(Value::as_array)
        .ok_or_else(|| bad(&v_path, "is required as an array"))?;
    if vertices.len() < 3 {
        return Err(bad(&v_path, "a polyhedron needs at least 3 vertices"));
    }
    let scene = vertices
        .iter()
        .enumerate()
        .map(|(i, v)| triple(v, &format!("{v_path}[{i}]")))
        .collect::<Result<Vec<_>, _>>()?;
    let min_z = scene.iter().map(|v| v[2]).fold(f64::INFINITY, f64::min);
    let positions = scene
        .iter()
        .map(|v| f32x3([v[0] * cell, v[2] * cell, v[1] * cell]))
        .collect::<Vec<_>>();

    let f_path = join(path, "faces");
    let faces = present(f, "faces")
        .and_then(Value::as_array)
        .filter(|faces| !faces.is_empty())
        .ok_or_else(|| bad(&f_path, "is required as a non-empty array"))?;
    let mut indices = Vec::with_capacity(faces.len());
    for (i, face) in faces.iter().enumerate() {
        let face_path = format!("{f_path}[{i}]");
        let tri = face
            .as_array()
            .filter(|t| t.len() == 3)
            .ok_or_else(|| bad(&face_path, "must be 3 vertex indices"))?;
        let mut abc = [0u32; 3];
        for (k, index) in tri.iter().enumerate() {
            abc[k] = index
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| (*n as usize) < scene.len())
                .ok_or_else(|| {
                    bad(
                        &face_path,
                        format!("indices must be integers below {}", scene.len()),
                    )
                })?;
        }
        let [a, b, c] = abc;
        if a == b || b == c || a == c {
            return Err(bad(
                &face_path,
                "repeats a vertex (the three host skips the face)",
            ));
        }
        indices.push([a, c, b]);
    }

    let uv_path = join(path, "uvs");
    let uvs = match present(f, "uvs").and_then(Value::as_array) {
        None => None,
        Some(list) if list.is_empty() => None,
        Some(list) => {
            if list.len() != scene.len() {
                return Err(bad(
                    &uv_path,
                    format!(
                        "{} uvs for {} vertices (the three host drops them)",
                        list.len(),
                        scene.len()
                    ),
                ));
            }
            Some(
                list.iter()
                    .enumerate()
                    .map(|(i, uv)| {
                        let pair = uv
                            .as_array()
                            .filter(|p| p.len() == 2)
                            .ok_or_else(|| bad(&format!("{uv_path}[{i}]"), "must be [u, v]"))?;
                        Ok([
                            number(&pair[0], &format!("{uv_path}[{i}]"))? as f32,
                            number(&pair[1], &format!("{uv_path}[{i}]"))? as f32,
                        ])
                    })
                    .collect::<Result<Vec<_>, CanvasSceneError>>()?,
            )
        }
    };
    if present(f, "uvs").is_some_and(|v| !v.is_array()) {
        return Err(bad(&uv_path, "must be an array"));
    }
    let mesh = geometry::polyhedron(positions, &indices, uvs)
        .map_err(|e| bad(path, format!("polyhedron geometry: {}", e.0)))?;
    Ok((mesh, -min_z * cell))
}

fn material(
    m: Option<&Fields>,
    path: &str,
    opacity: f64,
    theme: &SceneTheme,
) -> Result<Material, CanvasSceneError> {
    let empty = Fields::new();
    let m = m.unwrap_or(&empty);
    check_keys(m, MATERIAL_KEYS, path)?;
    let kind = match opt_str(m, "kind", path)? {
        None | Some("standard") => MaterialKind::Standard,
        Some("physical") => MaterialKind::Physical,
        Some("toon") => MaterialKind::Toon,
        Some("basic") => MaterialKind::Basic,
        Some(_) => {
            return Err(bad(
                &join(path, "kind"),
                "must be standard, physical, toon or basic",
            ))
        }
    };
    let drawn: &[&str] = match kind {
        MaterialKind::Basic => &["kind", "color", "opacity", "side", "map"],
        MaterialKind::Toon => &[
            "kind",
            "color",
            "opacity",
            "side",
            "map",
            "emissive",
            "emissiveIntensity",
        ],
        MaterialKind::Standard => &[
            "kind",
            "color",
            "opacity",
            "side",
            "map",
            "emissive",
            "emissiveIntensity",
            "metalness",
            "roughness",
            "flatShading",
        ],
        MaterialKind::Physical | MaterialKind::Flat(_) => MATERIAL_KEYS,
    };
    for (key, value) in m {
        if !value.is_null() && !drawn.contains(&key.as_str()) {
            return Err(bad(
                &join(path, key),
                format!(
                    "material kind `{}` does not draw it (the three host ignores it)",
                    kind_name(kind)
                ),
            ));
        }
    }
    let emissive = opt_rgb(m, "emissive", path)?;
    let emissive_intensity = opt_number(m, "emissiveIntensity", path)?;
    if emissive.is_none() && emissive_intensity.is_some() {
        return Err(bad(
            &join(path, "emissiveIntensity"),
            "has no effect without material.emissive (the three host ignores it)",
        ));
    }
    let side = match opt_str(m, "side", path)? {
        None | Some("front") => Side::Front,
        Some("back") => Side::Back,
        Some("double") => Side::Double,
        Some(_) => return Err(bad(&join(path, "side"), "must be front, back or double")),
    };
    let map = match opt_str(m, "map", path)? {
        None => None,
        Some("") => {
            return Err(bad(
                &join(path, "map"),
                "is empty (the three host drops it)",
            ))
        }
        Some(url) => Some(url.to_owned()),
    };
    let pbr = matches!(kind, MaterialKind::Standard | MaterialKind::Physical);
    let defaults = Material::default();
    Ok(Material {
        kind,
        color: opt_rgb(m, "color", path)?.unwrap_or([1.0; 3]),
        opacity: opacity as f32,
        metalness: if pbr {
            opt_number(m, "metalness", path)?.map_or(theme.metalness, |v| v as f32)
        } else {
            defaults.metalness
        },
        roughness: if pbr {
            opt_number(m, "roughness", path)?.map_or(theme.roughness, |v| v as f32)
        } else {
            defaults.roughness
        },
        emissive: emissive.unwrap_or([0.0; 3]),
        emissive_intensity: emissive_intensity.map_or(1.0, |v| v as f32),
        transmission: opt_number(m, "transmission", path)?.map_or(0.0, |v| v as f32),
        ior: opt_number(m, "ior", path)?.map_or(1.5, |v| v as f32),
        flat_shading: pbr && opt_bool(m, "flatShading", path)?.unwrap_or(theme.flat),
        side,
        map,
    })
}

/// Every keyframe colour parses as the static colours do, so a bad one is refused whatever the clock reads.
fn animated_colors(declared: &Animation, path: &str) -> Result<(), CanvasSceneError> {
    for track in ["color", "emissive"] {
        for (i, value) in declared.values(track) {
            rgb(value, &format!("{path}.keyframes[{i}].{track}"))?;
        }
    }
    Ok(())
}

fn animation_error(e: AnimationError) -> CanvasSceneError {
    bad(&e.field, e.why)
}

fn kind_name(kind: MaterialKind) -> &'static str {
    match kind {
        MaterialKind::Basic => "basic",
        MaterialKind::Standard => "standard",
        MaterialKind::Physical => "physical",
        MaterialKind::Toon => "toon",
        MaterialKind::Flat(_) => "flat",
    }
}

/// three's `Euler` order `XYZ`: the matrix is Rx · Ry · Rz.
fn euler_xyz([x, y, z]: [f64; 3]) -> DMat4 {
    DMat4::from_rotation_x(x) * DMat4::from_rotation_y(y) * DMat4::from_rotation_z(z)
}

/// `clampSegments`: default 24, JS `Math.round`, clamped to 3..64.
fn clamp_segments(segments: Option<f64>) -> usize {
    js_round(segments.unwrap_or(24.0)).clamp(3.0, 64.0) as usize
}

fn js_round(value: f64) -> f64 {
    (value + 0.5).floor()
}

fn drawable_nodes(value: &Value, path: &str) -> Result<Vec<Value>, CanvasSceneError> {
    let list: Vec<&Value> = match value {
        Value::Array(items) => items.iter().collect(),
        other => vec![other],
    };
    for (i, item) in list.iter().enumerate() {
        if !item
            .as_object()
            .is_some_and(|o| o.get("type").is_some_and(Value::is_string))
        {
            return Err(bad(&format!("{path}[{i}]"), "not a drawable node"));
        }
    }
    Ok(to_drawable_nodes(value))
}

fn unsupported(message: String) -> CanvasSceneError {
    CanvasSceneError::Unsupported3DContract(message)
}

fn undrawn(field: &str, step: u8) -> CanvasSceneError {
    unsupported(format!(
        "{field}: not yet drawn by orbital-canvas (P-025 step {step})"
    ))
}

fn bad(field: &str, why: impl std::fmt::Display) -> CanvasSceneError {
    unsupported(format!("{field}: {why}"))
}

fn join(path: &str, key: &str) -> String {
    format!("{path}.{key}")
}

fn present<'a>(f: &'a Fields, key: &str) -> Option<&'a Value> {
    f.get(key).filter(|v| !v.is_null())
}

fn check_keys(f: &Fields, allowed: &[&str], path: &str) -> Result<(), CanvasSceneError> {
    match f.keys().find(|key| !allowed.contains(&key.as_str())) {
        Some(key) => Err(bad(&join(path, key), "not a declared prop")),
        None => Ok(()),
    }
}

fn number(v: &Value, field: &str) -> Result<f64, CanvasSceneError> {
    v.as_f64()
        .filter(|n| n.is_finite())
        .ok_or_else(|| bad(field, "must be a finite number"))
}

fn opt_number(f: &Fields, key: &str, path: &str) -> Result<Option<f64>, CanvasSceneError> {
    present(f, key)
        .map(|v| number(v, &join(path, key)))
        .transpose()
}

fn opt_bool(f: &Fields, key: &str, path: &str) -> Result<Option<bool>, CanvasSceneError> {
    present(f, key)
        .map(|v| {
            v.as_bool()
                .ok_or_else(|| bad(&join(path, key), "must be a boolean"))
        })
        .transpose()
}

fn opt_str<'a>(f: &'a Fields, key: &str, path: &str) -> Result<Option<&'a str>, CanvasSceneError> {
    present(f, key)
        .map(|v| {
            v.as_str()
                .ok_or_else(|| bad(&join(path, key), "must be a string"))
        })
        .transpose()
}

fn opt_object<'a>(
    f: &'a Fields,
    key: &str,
    path: &str,
) -> Result<Option<&'a Fields>, CanvasSceneError> {
    present(f, key)
        .map(|v| {
            v.as_object()
                .ok_or_else(|| bad(&join(path, key), "must be an object"))
        })
        .transpose()
}

fn triple(v: &Value, field: &str) -> Result<[f64; 3], CanvasSceneError> {
    let list = v
        .as_array()
        .filter(|l| l.len() == 3)
        .ok_or_else(|| bad(field, "must be [x, y, z]"))?;
    Ok([
        number(&list[0], field)?,
        number(&list[1], field)?,
        number(&list[2], field)?,
    ])
}

fn f32x3(v: [f64; 3]) -> [f32; 3] {
    v.map(|c| c as f32)
}

fn scene_pos(v: &Value, field: &str) -> Result<[f64; 3], CanvasSceneError> {
    let o = v
        .as_object()
        .ok_or_else(|| bad(field, "must be {x, y, z?}"))?;
    check_keys(o, &["x", "y", "z"], field)?;
    let axis = |key: &str| {
        present(o, key)
            .ok_or_else(|| bad(&join(field, key), "is required"))
            .and_then(|v| number(v, &join(field, key)))
    };
    Ok([
        axis("x")?,
        axis("y")?,
        opt_number(o, "z", field)?.unwrap_or(0.0),
    ])
}

fn required_pos(f: &Fields, path: &str) -> Result<[f64; 3], CanvasSceneError> {
    let field = join(path, "position");
    scene_pos(
        present(f, "position").ok_or_else(|| bad(&field, "is required"))?,
        &field,
    )
}

/// A CSS color as three's `Color.set` takes it under ColorManagement: sRGB to linear; three drops alpha, so an alpha
/// below one is refused rather than lost.
fn rgb(v: &Value, field: &str) -> Result<[f32; 3], CanvasSceneError> {
    let [r, g, b, a] = css(v, field)?;
    if a != 1.0 {
        return Err(bad(
            field,
            "a 3D color has no alpha (the three host drops it)",
        ));
    }
    Ok(srgb_to_linear_rgb([r, g, b]))
}

fn opt_rgb(f: &Fields, key: &str, path: &str) -> Result<Option<[f32; 3]>, CanvasSceneError> {
    present(f, key)
        .map(|v| rgb(v, &join(path, key)))
        .transpose()
}

fn rgba(v: &Value, field: &str) -> Result<[f32; 4], CanvasSceneError> {
    let [r, g, b, a] = css(v, field)?;
    Ok([srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b), a])
}

fn css(v: &Value, field: &str) -> Result<[f32; 4], CanvasSceneError> {
    let s = v
        .as_str()
        .ok_or_else(|| bad(field, "must be a CSS color string"))?;
    parse_css_color(s).ok_or_else(|| {
        bad(
            field,
            format!(
                "`{s}` is not a CSS color (theme tones and var() resolve only in the web host)"
            ),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const TOL: f32 = 1e-4;

    fn close(actual: [f32; 3], expected: [f32; 3]) {
        for i in 0..3 {
            assert!(
                (actual[i] - expected[i]).abs() < TOL,
                "{actual:?} != {expected:?}"
            );
        }
    }

    fn scene(props: Value) -> Result<Scene3D, CanvasSceneError> {
        let mut canvas = json!({"type": "canvas", "mode": "3d", "showGrid": false});
        for (key, value) in props.as_object().unwrap() {
            canvas[key] = value.clone();
        }
        scene_from_canvas_node(&canvas, 800.0, 600.0)
    }

    fn scene_at(props: Value, elapsed_ms: f64) -> Result<Scene3D, CanvasSceneError> {
        let mut canvas = json!({"type": "canvas", "mode": "3d", "showGrid": false});
        for (key, value) in props.as_object().unwrap() {
            canvas[key] = value.clone();
        }
        scene_from_canvas_node_at(&canvas, 800.0, 600.0, &SceneTheme::default(), elapsed_ms)
    }

    fn message(result: Result<Scene3D, CanvasSceneError>) -> String {
        match result {
            Err(CanvasSceneError::Unsupported3DContract(m)) => m,
            other => panic!("expected an unsupported-contract error, got {other:?}"),
        }
    }

    fn two_meshes_camera(camera: Value) -> Camera {
        scene(json!({
            "camera": camera,
            "drawables": [
                {"type": "draw-mesh", "position": {"x": 0, "y": 0}},
                {"type": "draw-mesh", "position": {"x": 6, "y": 2}}
            ]
        }))
        .unwrap()
        .camera
    }

    #[test]
    fn camera_framing_matches_each_three_host_mode() {
        let target = [3.0, 0.0, 1.0];
        for (mode, eye) in [
            ("isometric", [9.0, 4.8, 7.0]),
            ("perspective", [9.0, 6.0, 7.0]),
            ("top-down", [3.0, 12.0, 3.1]),
            ("front", [3.0, 1.92, 7.9]),
        ] {
            let camera = two_meshes_camera(json!({"mode": mode}));
            close(camera.eye, eye);
            close(camera.target, target);
            assert_eq!(camera.up, [0.0, 1.0, 0.0]);
            assert_eq!(
                camera.projection,
                Projection::Perspective {
                    fov_y_degrees: 45.0
                }
            );
            assert_eq!((camera.near, camera.far), (0.1, 1000.0));
        }
        let follow = two_meshes_camera(json!({"mode": "follow"}));
        close(follow.eye, [3.0, 12.0, 15.0]);
        close(follow.target, target);
        let chase = two_meshes_camera(json!({"mode": "chase", "fov": 60}));
        close(chase.eye, [3.0, 4.0, -6.0]);
        assert_eq!(
            chase.projection,
            Projection::Perspective {
                fov_y_degrees: 60.0
            }
        );
        let tracked =
            two_meshes_camera(json!({"mode": "follow", "target": {"x": 2, "y": 1, "z": 0.5}}));
        close(tracked.eye, [2.0, 12.5, 15.0]);
        close(tracked.target, [2.0, 0.5, 1.0]);
        let orbited = two_meshes_camera(
            json!({"mode": "perspective", "zoom": 2, "azimuth": std::f64::consts::FRAC_PI_2}),
        );
        close(orbited.eye, [0.0, 3.0, 4.0]);
    }

    #[test]
    fn orbit_controls_cap_the_polar_angle_and_the_distance() {
        let grazing = two_meshes_camera(json!({"mode": "perspective", "elevation": 0}));
        close(grazing.eye, [8.970025, 0.847115, 6.970025]);
        let close_in = two_meshes_camera(json!({"mode": "isometric", "zoom": 10}));
        let offset = [
            close_in.eye[0] - 3.0,
            close_in.eye[1],
            close_in.eye[2] - 1.0,
        ];
        let distance =
            (offset[0] * offset[0] + offset[1] * offset[1] + offset[2] * offset[2]).sqrt();
        assert!((distance - 2.0).abs() < TOL);
        assert!((offset[1] / offset[0] - 0.8).abs() < TOL);
        assert!((offset[2] - offset[0]).abs() < TOL);
    }

    #[test]
    fn pixels_per_unit_scales_framing_and_positions() {
        let s = scene(json!({
            "pixelsPerUnit": 32,
            "drawables": [
                {"type": "draw-mesh", "position": {"x": 0, "y": 0}, "pivot": "center"},
                {"type": "draw-mesh", "position": {"x": 320, "y": 64, "z": 3}, "pivot": "center"}
            ]
        }))
        .unwrap();
        close(s.camera.eye, [15.0, 8.0, 11.0]);
        close(s.camera.target, [5.0, 0.0, 1.0]);
        let t = s.meshes[1].transform;
        close([t[12], t[13], t[14]], [10.0, 3.0, 2.0]);
    }

    #[test]
    fn polyhedron_swaps_scene_axes_and_rewinds_faces() {
        let s = scene(json!({
            "drawables": [{
                "type": "draw-mesh",
                "shape": "polyhedron",
                "position": {"x": 2, "y": 3, "z": 0.5},
                "vertices": [[0, 0, -1], [1, 0, -1], [0, 1, -1]],
                "faces": [[0, 1, 2]]
            }]
        }))
        .unwrap();
        let mesh = &s.meshes[0];
        assert_eq!(
            mesh.mesh.positions,
            vec![[0.0, -1.0, 0.0], [1.0, -1.0, 0.0], [0.0, -1.0, 1.0]]
        );
        assert_eq!(mesh.mesh.indices, vec![0, 2, 1]);
        for normal in &mesh.mesh.normals {
            close(*normal, [0.0, 1.0, 0.0]);
        }
        let t = mesh.transform;
        close([t[12], t[13], t[14]], [0.0, 1.5, 0.0]);
    }

    #[test]
    fn groups_compose_transforms_and_multiply_opacity() {
        let s = scene(json!({
            "drawables": [{
                "type": "draw-group",
                "position": {"x": 0, "y": 0},
                "rotate": std::f64::consts::FRAC_PI_2,
                "scale": 2,
                "opacity": 0.5,
                "items": [{
                    "type": "draw-group",
                    "position": {"x": 1, "y": 0},
                    "opacity": 0.5,
                    "items": [{
                        "type": "draw-mesh",
                        "position": {"x": 1, "y": 0},
                        "pivot": "center",
                        "opacity": 0.8,
                        "material": {"opacity": 0.5}
                    }]
                }]
            }]
        }))
        .unwrap();
        let mesh = &s.meshes[0];
        let t = mesh.transform;
        close([t[12], t[13], t[14]], [0.0, 0.0, 4.0]);
        close([t[0], t[1], t[2]], [0.0, 0.0, 2.0]);
        close([t[4], t[5], t[6]], [0.0, 2.0, 0.0]);
        assert!((mesh.material.opacity - 0.1).abs() < 1e-6);
        assert!(mesh.material.is_transparent());
    }

    #[test]
    fn omitted_lighting_is_the_three_host_rig_under_the_theme_fallbacks() {
        let s = scene(json!({})).unwrap();
        let lights = s.lights;
        assert_eq!(
            lights.ambient,
            Some(AmbientLight {
                color: [1.0; 3],
                intensity: 0.55
            })
        );
        assert_eq!(
            lights.directional,
            Some(DirectionalLight {
                position: [10.0, 20.0, 10.0],
                target: [0.0; 3],
                color: [1.0; 3],
                intensity: 1.0,
                shadow: Some(ShadowSettings {
                    map_size: 2048,
                    bias: -0.0004,
                    normal_bias: 0.04,
                    extent: 5.0,
                    near: 0.5,
                    far: 500.0,
                }),
            })
        );
        let channel = |byte: u8| srgb_to_linear(byte as f32 / 255.0);
        assert_eq!(
            lights.hemisphere,
            Some(HemisphereLight {
                sky: [channel(0x87), channel(0xce), channel(0xeb)],
                ground: [channel(0x36), channel(0x2d), channel(0x1d)],
                intensity: 0.3,
            })
        );
        assert!(lights.points.is_empty());
        assert!(s.shadows);
        assert_eq!(s.tone_mapping, ToneMapping::Aces);
        assert_eq!(s.background, [0.0; 4]);
        close(s.camera.target, [5.0, 0.0, 5.0]);
        assert!(s.meshes.is_empty());

        let unshadowed = scene(json!({"shadows": false})).unwrap();
        assert!(!unshadowed.shadows);
        assert_eq!(unshadowed.lights.directional.unwrap().shadow, None);
    }

    #[test]
    fn a_material_with_every_field_maps_each_one() {
        let s = scene(json!({
            "drawables": [{
                "type": "draw-mesh",
                "shape": "sphere",
                "position": {"x": 0, "y": 0},
                "opacity": 0.5,
                "castShadow": false,
                "receiveShadow": false,
                "material": {
                    "kind": "physical",
                    "color": "#ff0000",
                    "metalness": 0.25,
                    "roughness": 0.75,
                    "emissive": "#00ff00",
                    "emissiveIntensity": 2,
                    "opacity": 0.5,
                    "transmission": 0.3,
                    "ior": 1.33,
                    "flatShading": true,
                    "side": "double",
                    "map": "textures/rock.png"
                }
            }]
        }))
        .unwrap();
        let mesh = &s.meshes[0];
        assert_eq!(
            mesh.material,
            Material {
                kind: MaterialKind::Physical,
                color: [1.0, 0.0, 0.0],
                opacity: 0.25,
                metalness: 0.25,
                roughness: 0.75,
                emissive: [0.0, 1.0, 0.0],
                emissive_intensity: 2.0,
                transmission: 0.3,
                ior: 1.33,
                flat_shading: true,
                side: Side::Double,
                map: Some("textures/rock.png".into()),
            }
        );
        assert!(!mesh.cast_shadow);
        assert!(!mesh.receive_shadow);
        let t = mesh.transform;
        close([t[12], t[13], t[14]], [0.0, 0.4, 0.0]);

        let defaulted =
            scene(json!({"drawables": [{"type": "draw-mesh", "position": {"x": 0, "y": 0}}]}))
                .unwrap();
        let m = &defaulted.meshes[0].material;
        assert_eq!(
            (m.kind, m.roughness, m.metalness, m.flat_shading),
            (MaterialKind::Standard, 0.6, 0.0, false)
        );
        assert!(defaulted.meshes[0].cast_shadow && defaulted.meshes[0].receive_shadow);
    }

    #[test]
    fn a_ground_shape_lies_flat_facing_up() {
        let s = scene(json!({"drawables": [{"type": "draw-mesh", "shape": "plane", "position": {"x": 0, "y": 0}}]})).unwrap();
        let t = s.meshes[0].transform;
        // three's plane faces +Z in its own frame; the ground rotation turns that face up (+Y)
        close([t[0], t[1], t[2]], [1.0, 0.0, 0.0]);
        close([t[8], t[9], t[10]], [0.0, 1.0, 0.0]);
        close([t[4], t[5], t[6]], [0.0, 0.0, -1.0]);
    }

    #[test]
    fn control_unknown_or_undrawn_fields_error_by_name() {
        let unknown = message(scene(json!({
            "drawables": [{"type": "draw-mesh", "position": {"x": 0, "y": 0}, "wobble": 1}]
        })));
        assert_eq!(unknown, "drawables[0].wobble: not a declared prop");
        let gradient = message(scene(json!({
            "drawables": [{"type": "draw-shape", "shape": "rect", "position": {"x": 0, "y": 0},
                "gradient": {"kind": "linear", "stops": []}}]
        })));
        assert_eq!(
            gradient,
            "drawables[0].gradient: not yet drawn by orbital-canvas (P-025 step 4)"
        );
        let grid = scene_from_canvas_node(&json!({"type": "canvas", "mode": "3d"}), 800.0, 600.0)
            .unwrap()
            .grid
            .unwrap();
        assert_eq!(
            (grid.center, grid.size),
            ([4.5, 0.0, 4.5], [12.0, 12.0]),
            "the empty scene's 0..10 bounds"
        );
        let shadow = message(scene(json!({"drawables": [{"type": "draw-sprite",
            "position": {"x": 0, "y": 0}, "asset": {"url": "a.png", "role": "unit", "category": "c"},
            "shadow": {"color": "#000000", "blur": 2}}]})));
        assert_eq!(
            shadow,
            "drawables[0].shadow: not yet drawn by orbital-canvas (P-025 step 4)"
        );
        let ignored = message(scene(json!({
            "drawables": [{"type": "draw-mesh", "position": {"x": 0, "y": 0}, "material": {"kind": "basic", "metalness": 1}}]
        })));
        assert!(
            ignored.starts_with("drawables[0].material.metalness: "),
            "{ignored}"
        );
        assert!(scene(json!({"keyMap": {"KeyW": "UP"}, "tileClickEvent": "TILE", "camera": {"controls": false}})).is_ok());
    }

    fn pulsing_box(animation: Value) -> Value {
        json!({"drawables": [{
            "type": "draw-mesh",
            "position": {"x": 0, "y": 0},
            "opacity": 0.9,
            "material": {"color": "#ffffff", "emissive": "#000000"},
            "animation": animation
        }]})
    }

    fn pulse() -> Value {
        json!({
            "durationMs": 1000,
            "keyframes": [
                {"at": 0, "offsetX": 0, "offsetY": 0, "offsetZ": 0, "scale": 1, "opacity": 1,
                 "emissive": "#ff0000", "emissiveIntensity": 0},
                {"at": 1, "offsetX": 2, "offsetY": 4, "offsetZ": 1, "scale": 3, "opacity": 0.5,
                 "emissive": "#00ff00", "emissiveIntensity": 2}
            ]
        })
    }

    #[test]
    fn a_mesh_animation_moves_scales_fades_and_glows_at_the_canvas_clock() {
        let s = scene_at(pulsing_box(pulse()), 500.0).unwrap();
        assert!(s.animated);
        let mesh = &s.meshes[0];
        let t = mesh.transform;
        // offset (1, 2, 0.5) cells → world (1, 0.5, 2); the ×2 scale also doubles the 0.5 pivot lift
        close([t[12], t[13], t[14]], [1.0, 1.5, 2.0]);
        close([t[0], t[5], t[10]], [2.0, 2.0, 2.0]);
        assert!(
            (mesh.material.opacity - 0.75).abs() < 1e-6,
            "the track replaces node opacity"
        );
        assert!(mesh.material.is_transparent());
        close(mesh.material.emissive, [1.0, 0.0, 0.0]);
        assert!((mesh.material.emissive_intensity - 1.0).abs() < 1e-6);
        close(mesh.material.color, [1.0, 1.0, 1.0]);

        let looped = scene_at(pulsing_box(pulse()), 1500.0).unwrap();
        assert_eq!(looped.meshes, s.meshes);
    }

    #[test]
    fn a_mesh_rotation_track_adds_to_the_authored_and_ground_rotation() {
        let spin = json!({"durationMs": 1000, "keyframes": [
            {"at": 0, "rotateX": 0, "color": "#ff0000"},
            {"at": 1, "rotateX": std::f64::consts::PI, "color": "#0000ff"}
        ]});
        let s = scene_at(json!({"drawables": [{
            "type": "draw-mesh", "shape": "plane", "position": {"x": 0, "y": 0}, "animation": spin
        }]}), 500.0)
        .unwrap();
        let t = s.meshes[0].transform;
        // −π/2 ground rotation + π/2 from the track: the plane stands back up, facing +Z
        close([t[8], t[9], t[10]], [0.0, 0.0, 1.0]);
        close(s.meshes[0].material.color, [1.0, 0.0, 0.0]);
    }

    #[test]
    fn a_group_animation_carries_its_children() {
        let s = scene_at(
            json!({"drawables": [{
                "type": "draw-group",
                "position": {"x": 0, "y": 0},
                "scale": 2,
                "animation": {"durationMs": 1000, "keyframes": [
                    {"at": 0, "rotateY": 0, "offsetZ": 0, "scale": 1},
                    {"at": 1, "rotateY": std::f64::consts::PI, "offsetZ": 2, "scale": 2}
                ]},
                "items": [{"type": "draw-mesh", "position": {"x": 1, "y": 0}, "pivot": "center"}]
            }]}),
            500.0,
        )
        .unwrap();
        assert!(s.animated);
        let t = s.meshes[0].transform;
        // yaw π/2 turns +X to −Z; scale 2 × 1.5; offsetZ 1 cell up
        close([t[12], t[13], t[14]], [0.0, 1.0, -3.0]);
        close([t[1], t[5], t[9]], [0.0, 3.0, 0.0]);
    }

    #[test]
    fn control_no_animation_or_no_clock_is_the_unanimated_scene() {
        let still = json!({"drawables": [{
            "type": "draw-group", "position": {"x": 1, "y": 2}, "rotate": 0.3, "scale": 1.5, "opacity": 0.5,
            "items": [{"type": "draw-mesh", "shape": "circle", "position": {"x": 1, "y": 0},
                       "rotation": [0.1, 0.2, 0.3], "opacity": 0.8,
                       "material": {"emissive": "#102030", "emissiveIntensity": 2}}]
        }]});
        let rest = scene(still.clone()).unwrap();
        assert!(!rest.animated);
        assert_eq!(scene_at(still, 1234.0).unwrap(), rest);

        let at_rest = scene(pulsing_box(pulse())).unwrap();
        assert!(at_rest.animated);
        let mut plain = pulsing_box(Value::Null);
        plain["drawables"][0]
            .as_object_mut()
            .unwrap()
            .remove("animation");
        let plain = scene(plain).unwrap();
        assert_eq!(at_rest.meshes, plain.meshes);
    }

    #[test]
    fn control_animation_tracks_three_ignores_error_by_name() {
        let group_fade = message(scene_at(
            json!({"drawables": [{
                "type": "draw-group", "position": {"x": 0, "y": 0}, "items": [],
                "animation": {"durationMs": 100, "keyframes": [{"at": 0, "scale": 1}, {"at": 1, "opacity": 0}]}
            }]}),
            0.0,
        ));
        assert_eq!(
            group_fade,
            "drawables[0].animation.keyframes[1].opacity: a group animates only its transform (the three host ignores it)"
        );
        let basic_glow = message(scene(json!({"drawables": [{
            "type": "draw-mesh", "position": {"x": 0, "y": 0}, "material": {"kind": "basic"},
            "animation": {"durationMs": 100, "keyframes": [{"at": 0, "emissive": "#ff0000"}]}
        }]})));
        assert!(
            basic_glow.starts_with("drawables[0].animation.keyframes[0].emissive: "),
            "{basic_glow}"
        );
        let dark_intensity = message(scene(json!({"drawables": [{
            "type": "draw-mesh", "position": {"x": 0, "y": 0},
            "animation": {"durationMs": 100, "keyframes": [{"at": 0, "emissiveIntensity": 2}]}
        }]})));
        assert!(
            dark_intensity.starts_with("drawables[0].animation.keyframes[0].emissiveIntensity: "),
            "{dark_intensity}"
        );
        let bad_color = message(scene(pulsing_box(json!({
            "durationMs": 100, "keyframes": [{"at": 0, "color": "not-a-color"}]
        }))));
        assert!(
            bad_color.starts_with("drawables[0].animation.keyframes[0].color: "),
            "{bad_color}"
        );
        assert!(scene_at(pulsing_box(pulse()), -1.0).is_err());
    }

    fn rig_group(tip_scope: bool, bones: Value) -> Value {
        let tip = json!({"type": "draw-group", "position": {"x": 2, "y": 0}, "bone": "tip",
            "rotation": [0, 0, std::f64::consts::FRAC_PI_2], "items": []});
        let tip = if tip_scope {
            json!({"type": "draw-group", "position": {"x": 0, "y": 0}, "skeleton": true, "items": [tip]})
        } else {
            tip
        };
        json!({"drawables": [{"type": "draw-group", "position": {"x": 0, "y": 0}, "skeleton": true, "items": [
            {"type": "draw-group", "position": {"x": 0, "y": 0}, "bone": "root", "items": []},
            tip,
            {"type": "draw-mesh", "shape": "polyhedron", "position": {"x": 0, "y": 0},
                "vertices": [[0, 0, 0], [1, 0, 0], [0, 1, 0]], "faces": [[0, 1, 2]],
                "skin": {"bones": bones,
                    "inverseBindMatrices": [
                        [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1],
                        [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, -2, 0, 0, 1]
                    ],
                    "indices": [[0], [1], [0]],
                    "weights": [[1], [1], [1]]}}
        ]}]})
    }

    #[test]
    fn a_skinned_polyhedron_follows_its_named_bone_groups() {
        let s = scene(rig_group(false, json!(["root", "tip"]))).unwrap();
        assert_eq!(s.meshes.len(), 1);
        let mesh = &s.meshes[0];
        // scene [x, y, z] -> three [x, z, y]; vertex 1 rides `tip`: (1,0,0) - 2 = (-1,0,0), turned a quarter about
        // Z is (0,-1,0), back at x = 2
        close(mesh.mesh.positions[0], [0.0, 0.0, 0.0]);
        close(mesh.mesh.positions[1], [2.0, -1.0, 0.0]);
        close(mesh.mesh.positions[2], [0.0, 0.0, 1.0]);
        assert_eq!(mesh.transform, Mat4::IDENTITY.to_cols_array());
    }

    #[test]
    fn control_skin_bones_resolve_only_inside_their_skeleton_scope() {
        let scoped_away = message(scene(rig_group(true, json!(["root", "tip"]))));
        assert_eq!(
            scoped_away,
            "drawables[0].items[2].skin.bones[1]: no draw-group in this skeleton scope declares bone `tip` (the three host never binds the skin)"
        );
        let unknown = message(scene(rig_group(false, json!(["root", "ghost"]))));
        assert!(unknown.contains("bone `ghost`"), "{unknown}");
        let mut turned = rig_group(false, json!(["root", "tip"]));
        turned["drawables"][0]["items"][2]["rotation"] = json!([0, 1, 0]);
        assert_eq!(
            message(scene(turned)),
            "drawables[0].items[2].rotation: a skinned mesh is placed by its bones alone (the three host ignores it)"
        );
        let boxed = message(scene(
            json!({"drawables": [{"type": "draw-mesh", "position": {"x": 0, "y": 0},
            "skin": {"bones": [], "inverseBindMatrices": [], "indices": [], "weights": []}}]}),
        ));
        assert!(
            boxed.starts_with("drawables[0].skin: only a polyhedron is skinned"),
            "{boxed}"
        );
    }

    fn register_model(url: &str, bytes: Vec<u8>) {
        crate::asset::register_asset(url, bytes.into(), Some("model/gltf-binary"));
    }

    fn model_sprite(url: &str, extra: Value) -> Value {
        let mut sprite = json!({"type": "draw-sprite", "id": "knight", "position": {"x": 0, "y": 0},
            "asset": {"url": url, "role": "unit", "category": "character", "dimension": "3d"},
            "width": 2, "animation": "walk"});
        for (key, value) in extra.as_object().unwrap() {
            sprite[key] = value.clone();
        }
        json!({"drawables": [sprite]})
    }

    #[test]
    fn a_3d_sprite_draws_its_model_posed_by_the_named_clip_on_the_canvas_clock() {
        use crate::scene3d::gltf::tests::{rig, rig_material};
        let url = "test://canvas-scene3d/knight.glb";
        register_model(url, rig(rig_material()));
        let rest = scene(model_sprite(url, json!({}))).unwrap();
        assert_eq!(rest.meshes.len(), 1);
        let mesh = &rest.meshes[0];
        close(mesh.mesh.positions[2], [0.0, 1.0, 0.0]);
        // the rest pose spans one unit, so the sprite's width 2 is the scale
        let t = mesh.transform;
        close([t[0], t[5], t[10]], [2.0, 2.0, 2.0]);
        assert_eq!(mesh.id.as_deref(), Some("knight"));
        assert_eq!(
            mesh.material.map.as_deref(),
            Some("test://canvas-scene3d/knight.glb#gltf-image-0")
        );
        assert!(
            rest.animated,
            "a playing clip keeps the host mapping frames"
        );

        let walking = scene_at(model_sprite(url, json!({})), 500.0).unwrap();
        close(
            walking.meshes[0].mesh.positions[2],
            [1.0 - std::f32::consts::SQRT_2, 0.0, 0.0],
        );
        let turned = scene(model_sprite(url, json!({"rotation": 90}))).unwrap();
        let t = turned.meshes[0].transform;
        // ModelLoader reads the rotation in degrees: a quarter turn about Y
        close([t[0], t[1], t[2]], [0.0, 0.0, -2.0]);
        let unknown = scene_at(model_sprite(url, json!({"animation": "run"})), 500.0).unwrap();
        close(unknown.meshes[0].mesh.positions[2], [0.0, 1.0, 0.0]);
        assert!(
            !unknown.animated,
            "control: an unknown clip leaves the model at rest"
        );
    }

    #[test]
    fn a_broken_model_draws_the_three_error_box_and_an_undrawn_one_errors_by_name() {
        let broken = "test://canvas-scene3d/broken.glb";
        register_model(broken, b"not a glb".to_vec());
        let s = scene(model_sprite(broken, json!({}))).unwrap();
        assert_eq!(s.meshes.len(), 1);
        let t = s.meshes[0].transform;
        close([t[12], t[13], t[14]], [0.0, 0.5, 0.0]);
        let channel = |byte: u8| srgb_to_linear(byte as f32 / 255.0);
        assert_eq!(
            s.meshes[0].material.color,
            [channel(0xff), channel(0x44), channel(0x44)]
        );

        use crate::scene3d::gltf::tests::rig;
        let masked = "test://canvas-scene3d/masked.glb";
        register_model(masked, rig(json!({"alphaMode": "MASK"})));
        assert_eq!(
            message(scene(model_sprite(masked, json!({})))),
            "drawables[0].asset.url: test://canvas-scene3d/masked.glb: glTF materials[0].alphaMode MASK (three's alphaTest): not yet drawn by orbital-canvas"
        );
        let faded = message(scene(model_sprite(broken, json!({"opacity": 0.5}))));
        assert_eq!(
            faded,
            "drawables[0].opacity: a 3D model does not draw it (the three host ignores it)"
        );
        let dimension = message(scene(
            json!({"drawables": [{"type": "draw-sprite", "position": {"x": 0, "y": 0},
            "asset": {"url": "a.png", "role": "unit", "category": "c", "dimension": "4d"}}]}),
        ));
        assert_eq!(dimension, "drawables[0].asset.dimension: must be 2d or 3d");
    }

    #[test]
    fn a_skinned_2d_rig_lies_on_the_scene_plane_as_the_2d_painter_poses_it() {
        let s = scene(json!({"drawables": [{
            "type": "draw-skinned-mesh",
            "position": {"x": 1, "y": 2, "z": 0.5},
            "asset": {"url": "unused", "role": "unit", "category": "c"},
            "weightsOverlay": true,
            "mesh": {"vertices": [
                {"x": 0, "y": 0, "u": 0, "v": 0, "weights": [{"bone": 0, "w": 1}]},
                {"x": 1, "y": 0, "u": 1, "v": 0, "weights": [{"bone": 0, "w": 1}]},
                {"x": 0, "y": 1, "u": 0, "v": 1, "weights": [{"bone": 0, "w": 1}]}
            ], "triangles": [[0, 1, 2]]},
            "bones": [{"name": "root", "pivot": [0, 0], "tip": [1, 0]}],
            "pose": {"root": 90}
        }]}))
        .unwrap();
        assert_eq!(s.meshes.len(), 1);
        let mesh = &s.meshes[0];
        // posed (0,0), (0,1), (-1,0) about position (1,2); the hull's minimum (0,2) anchors the bounds
        close(mesh.mesh.positions[0], [1.0, 0.5, 0.0]);
        close(mesh.mesh.positions[1], [1.0, 0.5, 1.0]);
        close(mesh.mesh.positions[2], [0.0, 0.5, 0.0]);
        assert_eq!(
            (
                mesh.material.kind,
                mesh.material.side,
                mesh.material.opacity
            ),
            (MaterialKind::Basic, Side::Double, 1.0)
        );
        let ghost = message(scene(json!({"drawables": [{
            "type": "draw-skinned-mesh", "position": {"x": 0, "y": 0},
            "ghost": {"frame": 0, "opacity": 0.5}
        }]})));
        assert!(ghost.starts_with("drawables[0].ghost"), "{ghost}");
    }

    fn flat(m: &Material) -> Flat {
        match m.kind {
            MaterialKind::Flat(flat) => flat,
            other => panic!("expected a flat material, got {other:?}"),
        }
    }

    fn origin(t: [f32; 16]) -> [f32; 3] {
        [t[12], t[13], t[14]]
    }

    fn png(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba(rgba));
        let mut out = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    fn register_png(url: &str, width: u32, height: u32) {
        crate::asset::register_asset(
            url,
            png(width, height, [255, 255, 255, 255]).into(),
            Some("image/png"),
        );
    }

    fn shape_scene(shape: Value) -> Result<Scene3D, CanvasSceneError> {
        let mut node = json!({"type": "draw-shape", "position": {"x": 0, "y": 0}});
        for (key, value) in shape.as_object().unwrap() {
            node[key] = value.clone();
        }
        scene(json!({ "drawables": [node] }))
    }

    #[test]
    fn a_shape_shadow_draws_a_glow_of_its_coverage_under_it() {
        let s = shape_scene(
            json!({"shape": "rect", "width": 1, "height": 1, "fill": "#ffffff",
            "stroke": "#000000", "strokeWidth": 4, "shadow": {"color": "#ff0000", "blur": 0.2}}),
        )
        .unwrap();
        let [glow, body, edge] = s.meshes.as_slice() else {
            panic!("expected glow, body, edge: {} meshes", s.meshes.len());
        };
        let xs = body.mesh.positions.iter().map(|p| p[0]);
        let cell = xs.clone().fold(f32::MIN, f32::max) - xs.fold(f32::MAX, f32::min);
        assert!(
            (glow.glow.unwrap() - 0.2 * cell / 2.0).abs() < 1e-6,
            "sigma is half the blur, in cells"
        );
        assert_eq!(
            glow.mesh.indices.len(),
            body.mesh.indices.len() + edge.mesh.indices.len()
        );
        assert_eq!(glow.transform, body.transform);
        assert_eq!(glow.material.color, [1.0, 0.0, 0.0]);
        assert!(glow.id.is_none(), "a glow is never picked");
        assert_eq!((body.glow, edge.glow), (None, None));
    }

    #[test]
    fn control_a_shadow_canvas_would_not_draw_adds_no_glow_and_a_bad_one_is_refused() {
        for shadow in [
            json!({"color": "#ff0000", "blur": 0}),
            json!({"color": "rgba(255,0,0,0)", "blur": 1}),
        ] {
            let s = shape_scene(json!({"shape": "ellipse", "fill": "#ffffff", "shadow": shadow}))
                .unwrap();
            assert_eq!(s.meshes.len(), 1);
            assert_eq!(s.meshes[0].glow, None);
        }
        let refused = message(shape_scene(json!({"shape": "ellipse", "fill": "#ffffff",
            "shadow": {"color": "#ff0000", "blur": -1}})));
        assert!(refused.starts_with("drawables[0].shadow.blur"), "{refused}");
    }

    #[test]
    fn each_shape_kind_lies_flat_lifted_unlit_in_its_fill() {
        for (shape, extra, vertices) in [
            ("cell", json!({}), 4),
            ("rect", json!({"width": 2, "height": 1}), 4),
            ("ellipse", json!({"radiusX": 0.5}), 34),
            (
                "poly",
                json!({"points": [{"x": 0, "y": 0}, {"x": 1, "y": 0}, {"x": 0, "y": 1}]}),
                3,
            ),
            ("path", json!({"d": "M0 0 L1 0 L1 1 Z"}), 3),
        ] {
            let mut node = json!({"shape": shape, "fill": "#ff0000", "opacity": 0.5});
            for (key, value) in extra.as_object().unwrap() {
                node[key] = value.clone();
            }
            let s = shape_scene(node).unwrap();
            assert_eq!(s.meshes.len(), 1, "{shape}");
            let mesh = &s.meshes[0];
            assert_eq!(mesh.mesh.positions.len(), vertices, "{shape}");
            assert_eq!(flat(&mesh.material), Flat::default(), "{shape}");
            assert_eq!(mesh.material.color, [1.0, 0.0, 0.0], "{shape}");
            assert_eq!(
                (mesh.material.opacity, mesh.material.side),
                (0.5, Side::Double)
            );
            assert!(!mesh.cast_shadow && !mesh.receive_shadow);
            let t = mesh.transform;
            close(origin(t), [0.0, 0.02, 0.0]);
            // three's local +Y runs to world −Z, its face up
            close([t[4], t[5], t[6]], [0.0, 0.0, -1.0]);
            close([t[8], t[9], t[10]], [0.0, 1.0, 0.0]);
        }
        let cell = shape_scene(json!({"shape": "cell"})).unwrap();
        assert_eq!(cell.meshes[0].mesh.positions[1], [0.475, 0.475, 0.0]);
        assert_eq!(
            cell.meshes[0].material.color,
            [1.0, 1.0, 1.0],
            "control: no paint is white"
        );
    }

    #[test]
    fn a_stroke_only_ellipse_is_three_ring_and_control_a_filled_one_adds_a_stroke_band() {
        let ring =
            shape_scene(json!({"shape": "ellipse", "radiusX": 0.5, "stroke": "#00ff00"})).unwrap();
        assert_eq!(ring.meshes.len(), 1);
        let radii: Vec<f32> = ring.meshes[0]
            .mesh
            .positions
            .iter()
            .map(|p| p[0].hypot(p[1]))
            .collect();
        let (lo, hi) = radii
            .iter()
            .fold((f32::MAX, 0.0f32), |(a, b), r| (a.min(*r), b.max(*r)));
        assert!(
            (lo - 0.42).abs() < 1e-5 && (hi - 0.5).abs() < 1e-5,
            "{lo} {hi}"
        );
        assert_eq!(ring.meshes[0].material.color, [0.0, 1.0, 0.0]);

        let both = shape_scene(
            json!({"shape": "rect", "fill": "#ff0000", "stroke": "#0000ff", "strokeWidth": 25.6}),
        )
        .unwrap();
        assert_eq!(both.meshes.len(), 2);
        assert_eq!(both.meshes[1].material.color, [0.0, 0.0, 1.0]);
        let edge_x = both.meshes[1]
            .mesh
            .positions
            .iter()
            .map(|p| p[0])
            .fold(0.0f32, f32::max);
        // 25.6 painter px is a tenth of a cell, centered on the 0.5 edge
        assert!((edge_x - 0.55).abs() < 1e-5, "{edge_x}");
        assert!(both.meshes[1].transform[13] > both.meshes[0].transform[13]);
    }

    #[test]
    fn a_shape_offset_rotation_and_animation_move_it_on_the_ground() {
        let s = shape_scene(json!({"shape": "cell", "offsetX": 1, "offsetY": 2, "rotate": std::f64::consts::FRAC_PI_2}))
            .unwrap();
        let t = s.meshes[0].transform;
        close(origin(t), [1.0, 0.02, 2.0]);
        // the yaw is −rotate (2D turns clockwise): a quarter turn takes the shape's X to world +Z
        close([t[0], t[1], t[2]], [0.0, 0.0, 1.0]);
        let moving = json!({"shape": "cell", "animation": {"durationMs": 1000, "keyframes": [
            {"at": 0, "offsetX": 0, "fill": "#ff0000"}, {"at": 1, "offsetX": 2, "fill": "#0000ff"}]}});
        let at_rest = shape_scene(moving.clone()).unwrap();
        assert!(at_rest.animated);
        close(origin(at_rest.meshes[0].transform), [0.0, 0.02, 0.0]);
        let mut canvas = json!({"type": "canvas", "mode": "3d", "showGrid": false,
            "drawables": [{"type": "draw-shape", "position": {"x": 0, "y": 0}}]});
        for (key, value) in moving.as_object().unwrap() {
            canvas["drawables"][0][key] = value.clone();
        }
        let half = scene_from_canvas_node_at(&canvas, 800.0, 600.0, &SceneTheme::default(), 500.0)
            .unwrap();
        close(origin(half.meshes[0].transform), [1.0, 0.02, 0.0]);
        assert_eq!(
            half.meshes[0].material.color,
            [1.0, 0.0, 0.0],
            "a color track holds until the next key"
        );
    }

    #[test]
    fn control_shape_props_without_a_3d_drawing_error_by_name() {
        let cases = [
            (
                json!({"shape": "rect", "anchor": "top-left"}),
                "drawables[0].anchor: ",
            ),
            (
                json!({"shape": "rect", "stroke": "#fff", "strokeWidth": 2}),
                "drawables[0].strokeWidth: ",
            ),
            (
                json!({"shape": "cell", "radiusX": 1}),
                "drawables[0].radiusX: ",
            ),
            (
                json!({"shape": "rect", "blendMode": "multiply"}),
                "drawables[0].blendMode: ",
            ),
            (
                json!({"shape": "rect", "strokeDash": [2, 2]}),
                "drawables[0].strokeDash: ",
            ),
            (
                json!({"shape": "poly", "points": [{"x": 0, "y": 0}, {"x": 1, "y": 1}]}),
                "drawables[0].points: ",
            ),
            (
                json!({"shape": "path", "d": "M0 0 L1 1 L1 0 L0 1 Z"}),
                "drawables[0]: path: ",
            ),
            (
                json!({"shape": "cell", "animation": {"durationMs": 100, "keyframes": [{"at": 0, "blur": 2}]}}),
                "drawables[0].animation.keyframes[0].blur: ",
            ),
        ];
        for (shape, prefix) in cases {
            let m = message(shape_scene(shape));
            assert!(m.starts_with(prefix), "{m}");
        }
        let lighter = shape_scene(json!({"shape": "cell", "blendMode": "lighter"})).unwrap();
        assert!(flat(&lighter.meshes[0].material).additive);
    }

    #[test]
    fn a_label_is_a_billboard_lifted_over_its_position_and_control_empty_text_draws_nothing() {
        let s = scene(
            json!({"drawables": [{"type": "draw-text", "id": "hp", "text": "HP 3",
            "position": {"x": 0, "y": 0}, "color": "#ffcc00", "opacity": 0.5}]}),
        )
        .unwrap();
        assert_eq!(s.meshes.len(), 1);
        let mesh = &s.meshes[0];
        assert_eq!(
            flat(&mesh.material),
            Flat {
                billboard: true,
                ..Flat::default()
            }
        );
        close(origin(mesh.transform), [0.0, 1.2, 0.0]);
        assert_eq!(mesh.material.opacity, 0.5);
        assert_eq!(mesh.id.as_deref(), Some("hp"));
        let key = mesh.material.map.clone().unwrap();
        assert!(
            crate::asset::image_info(&key).is_some(),
            "the label raster is in the asset cache"
        );
        // the quad is centered: troika's anchorX center / anchorY middle
        let xs: Vec<f32> = mesh.mesh.positions.iter().map(|p| p[0]).collect();
        assert!((xs[0] + xs[1]).abs() < 1e-5);
        let empty = scene(
            json!({"drawables": [{"type": "draw-text", "text": "", "position": {"x": 0, "y": 0}}]}),
        )
        .unwrap();
        assert!(empty.meshes.is_empty());
        let aligned = scene(
            json!({"drawables": [{"type": "draw-text", "text": "HP 3", "align": "left",
            "position": {"x": 0, "y": 0}}]}),
        )
        .unwrap();
        let left = aligned.meshes[0]
            .mesh
            .positions
            .iter()
            .map(|p| p[0])
            .fold(f32::MAX, f32::min);
        assert!(
            left < 0.0 && left > -0.05,
            "a left-aligned label starts at its position: {left}"
        );
        let font = message(scene(
            json!({"drawables": [{"type": "draw-text", "text": "x",
            "font": "12px Papyrus", "position": {"x": 0, "y": 0}}]}),
        ));
        assert!(font.starts_with("drawables[0].font: "), "{font}");
    }

    #[test]
    fn a_sprite_is_a_nearest_cut_out_ground_decal_by_default_and_a_billboard_when_standing() {
        let url = "test://canvas-scene3d/tile.png";
        register_png(url, 32, 16);
        let asset = json!({"url": url, "role": "tile", "category": "c"});
        let decal = scene(
            json!({"drawables": [{"type": "draw-sprite", "position": {"x": 0, "y": 0},
            "asset": asset, "width": 2}]}),
        )
        .unwrap();
        let mesh = &decal.meshes[0];
        assert_eq!(
            flat(&mesh.material),
            Flat {
                nearest: true,
                cutout: true,
                ..Flat::default()
            }
        );
        assert_eq!(mesh.material.map.as_deref(), Some(url));
        close(origin(mesh.transform), [1.0, 0.01, 0.5]);
        close(
            [mesh.transform[8], mesh.transform[9], mesh.transform[10]],
            [0.0, 1.0, 0.0],
        );

        let standing = scene(
            json!({"drawables": [{"type": "draw-sprite", "position": {"x": 0, "y": 0},
            "asset": asset, "anchor": "ground", "height": 2, "flipX": true,
            "frame": {"x": 8, "y": 0, "w": 8, "h": 16}}]}),
        )
        .unwrap();
        let mesh = &standing.meshes[0];
        assert!(flat(&mesh.material).billboard);
        close(origin(mesh.transform), [0.0, 1.0, 0.0]);
        // the frame's 1:2 aspect makes it one cell wide; flipX mirrors its u range
        assert!((mesh.mesh.positions[1][0] - 0.5).abs() < 1e-5);
        assert_eq!(mesh.mesh.uvs[0], [0.5, 0.0]);
        assert_eq!(mesh.mesh.uvs[1], [0.25, 0.0]);
    }

    #[test]
    fn a_failed_sprite_draws_three_red_placeholder_and_no_asset_draws_the_fallback_box() {
        let broken = "test://canvas-scene3d/broken.png";
        crate::asset::register_asset(broken, b"not a png".to_vec().into(), Some("image/png"));
        let s = scene(
            json!({"drawables": [{"type": "draw-sprite", "position": {"x": 0, "y": 0},
            "asset": {"url": broken, "role": "unit", "category": "c"}, "anchor": "center"}]}),
        )
        .unwrap();
        let mesh = &s.meshes[0];
        let channel = |byte: u8| srgb_to_linear(byte as f32 / 255.0);
        assert_eq!(
            mesh.material.color,
            [channel(0xff), channel(0x44), channel(0x44)]
        );
        assert!((mesh.material.opacity - 0.6).abs() < 1e-6);
        assert!(
            !flat(&mesh.material).billboard,
            "three's placeholder is a plain plane"
        );
        assert_eq!(mesh.material.map, None);

        let unset =
            scene(json!({"drawables": [{"type": "draw-sprite", "position": {"x": 0, "y": 0}}]}))
                .unwrap();
        let mesh = &unset.meshes[0];
        assert_eq!(mesh.material.kind, MaterialKind::Standard);
        assert_eq!(
            mesh.material.color,
            [channel(0x9b), channel(0x8f), channel(0x7f)]
        );
        assert_eq!(mesh.mesh.positions.len(), 24, "a box");
    }

    #[test]
    fn layers_draw_each_item_as_its_atom_picked_by_its_id() {
        let s = scene(json!({"drawables": [
            {"type": "draw-shape-layer", "items": [
                {"type": "draw-shape", "id": "a", "shape": "cell", "position": {"x": 0, "y": 0}},
                {"type": "draw-shape", "id": "b", "shape": "cell", "position": {"x": 4, "y": 2}}
            ]},
            {"type": "draw-text-layer", "items": [
                {"type": "draw-text", "text": "hi", "position": {"x": 1, "y": 1}}
            ]}
        ]}))
        .unwrap();
        assert_eq!(s.meshes.len(), 3);
        assert_eq!(s.meshes[0].id.as_deref(), Some("a"));
        assert_eq!(s.meshes[1].id.as_deref(), Some("b"));
        close(origin(s.meshes[1].transform), [4.0, 0.02, 2.0]);
        // the layer items frame the camera as collectDrawnItems counts them
        close(s.camera.target, [2.0, 0.0, 1.0]);
        assert_eq!(s.interaction.cells.len(), 2);
        let wrong = message(scene(
            json!({"drawables": [{"type": "draw-shape-layer", "items": [
            {"type": "draw-text", "text": "x", "position": {"x": 0, "y": 0}}]}]}),
        ));
        assert_eq!(wrong, "drawables[0].items[0].type: must be `draw-shape`");
    }

    #[test]
    fn an_fx_layer_expands_on_the_page_clock_and_keeps_the_host_mapping() {
        let fx = json!({"type": "draw-fx-layer", "items": [{"id": "hit-1", "type": "hit",
            "position": {"x": 2, "y": 3}, "ttl": 2, "maxTtl": 4, "bornAt": 1000}],
            "presets": [{"type": "hit", "shape": "puff", "size": 0.5, "count": 3, "color": "#00ff00",
                "vx": 0, "vy": 0, "vz": 0, "gravity": 0}]});
        let canvas = json!({"type": "canvas", "mode": "3d", "showGrid": false, "drawables": [fx]});
        let s = scene_from_canvas_node_at_epoch(
            &canvas,
            800.0,
            600.0,
            &SceneTheme::default(),
            0.0,
            1500.0,
        )
        .unwrap();
        assert!(s.animated);
        assert_eq!(s.meshes.len(), 3, "three puffs");
        for mesh in &s.meshes {
            assert_eq!(mesh.material.color, [0.0, 1.0, 0.0]);
            // fade 0.75 × the puff's 0.6
            assert!((mesh.material.opacity - 0.45).abs() < 1e-6);
        }
        let empty = json!({"type": "canvas", "mode": "3d", "showGrid": false,
            "drawables": [{"type": "draw-fx-layer", "items": []}]});
        let rest = scene_from_canvas_node(&empty, 800.0, 600.0).unwrap();
        assert!(
            !rest.animated && rest.meshes.is_empty(),
            "control: no live entry, nothing to redraw"
        );
    }

    #[test]
    fn a_mesh_outline_is_a_scaled_back_face_hull_and_on_a_polyhedron_a_pushed_out_shell() {
        let s = scene(
            json!({"drawables": [{"type": "draw-mesh", "position": {"x": 0, "y": 0},
            "opacity": 0.5, "outline": {"color": "#00ff00", "width": 0.1}}]}),
        )
        .unwrap();
        assert_eq!(s.meshes.len(), 2);
        let hull = &s.meshes[1];
        assert_eq!(
            (
                hull.material.kind,
                hull.material.side,
                hull.material.color,
                hull.material.opacity
            ),
            (MaterialKind::Basic, Side::Back, [0.0, 1.0, 0.0], 0.5)
        );
        assert_eq!(hull.mesh, s.meshes[0].mesh);
        let t = hull.transform;
        close([t[0], t[5], t[10]], [1.1, 1.1, 1.1]);
        close(origin(t), [0.0, 0.5, 0.0]);
        assert!(!hull.cast_shadow);

        let poly = scene(json!({"drawables": [{"type": "draw-mesh", "shape": "polyhedron",
            "position": {"x": 0, "y": 0}, "vertices": [[0, 0, 0], [1, 0, 0], [0, 1, 0]], "faces": [[0, 1, 2]],
            "outline": {"width": 0.25}}]}))
        .unwrap();
        let (body, shell) = (&poly.meshes[0].mesh, &poly.meshes[1].mesh);
        for (p, q) in body.positions.iter().zip(&shell.positions) {
            close(*q, [p[0], p[1] + 0.25, p[2]]);
        }

        let theme = SceneTheme {
            outline: 2.0,
            ..SceneTheme::default()
        };
        let canvas = json!({"type": "canvas", "mode": "3d", "showGrid": false,
            "drawables": [{"type": "draw-mesh", "position": {"x": 0, "y": 0}}]});
        let themed = scene_from_canvas_node_themed(&canvas, 800.0, 600.0, &theme).unwrap();
        assert_eq!(themed.meshes.len(), 2, "the theme's outline");
        close([themed.meshes[1].transform[0], 0.0, 0.0], [1.04, 0.0, 0.0]);
        assert_eq!(
            scene_from_canvas_node(&canvas, 800.0, 600.0)
                .unwrap()
                .meshes
                .len(),
            1,
            "control: no outline drawn when neither the mesh nor the theme asks"
        );
    }

    #[test]
    fn a_skinned_polyhedron_outline_follows_the_posed_surface() {
        let mut rig = rig_group(false, json!(["root", "tip"]));
        rig["drawables"][0]["items"][2]["outline"] = json!({"width": 0.1});
        let s = scene(rig).unwrap();
        assert_eq!(s.meshes.len(), 2);
        let (body, shell) = (&s.meshes[0].mesh, &s.meshes[1].mesh);
        for (i, (p, q)) in body.positions.iter().zip(&shell.positions).enumerate() {
            let n = body.normals[i];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            close(
                *q,
                [
                    p[0] + n[0] / len * 0.1,
                    p[1] + n[1] / len * 0.1,
                    p[2] + n[2] / len * 0.1,
                ],
            );
        }
    }
}
