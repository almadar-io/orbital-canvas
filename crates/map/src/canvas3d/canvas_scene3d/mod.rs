//! `canvas` with `mode: "3d"` → [`Scene3D`]: the scene the three.js host (`Canvas3DHost`, `Drawable3D`, `DrawMesh3D`,
//! `Lighting3D`) builds today, rebuilt as a retained scene. Anything the pattern declares that this slice does not draw
//! is a typed error naming the field, never a silent drop.

use crate::error::CanvasSceneError;
use crate::game_canvas_2d::to_drawable_nodes;
use crate::skinned_mesh_2d;
use glam::{DMat4, DVec3, Mat4};
use orbital_canvas_base::color::parse_css_color;
use orbital_canvas_base::packet::DrawCommand;
use orbital_canvas_base::path_data::parse_path_data;
use orbital_canvas_base::projector::{Projection as Layout2D, Projector, TILE_WIDTH};
use orbital_canvas_base::scene3d::animation::{
    self, AnimatedState, Animation, AnimationError, MATERIAL_TRACKS,
};
use orbital_canvas_base::scene3d::geometry;
use orbital_canvas_base::scene3d::model::{
    srgb_to_linear, srgb_to_linear_rgb, AmbientLight, Camera, DirectionalLight, Flat, Grid,
    HemisphereLight, Lights, Material, MaterialKind, MeshData, MeshInstance, PointLight,
    Projection, Scene3D, ShaderBlend, ShadowSettings, Side, SurfaceShader, ToneMapping,
};
use orbital_canvas_base::scene3d::model::{CellId, Interaction};
use orbital_canvas_base::scene3d::skin;
use orbital_canvas_base::scene3d::tessellate::{self, Subpath};
use orbital_canvas_base::skinning_2d::{self, BoneDef, Pose, SkinMesh};
use orbital_canvas_base::transform::Xf;
use orbital_canvas_paint::asset;
use orbital_canvas_paint::atlas;
use orbital_canvas_paint::scene3d::gltf::{model_state, ModelState};
use orbital_canvas_paint::scene3d::label::{label, LabelStyle};
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

pub use orbital_canvas_base::scene3d::view::DragPreview;

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
    let props = crate::canvas3d_props::canvas_props_3d(node)?;

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
            Some("room") => Some(orbital_canvas_base::scene3d::model::EnvironmentKind::Room),
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
            let moved = crate::canvas3d_props::with_preview_position(&values, &p.id, p.x, p.y);
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
                crate::canvas3d_props::shade_fogged(&mut walk.meshes[first..]);
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
            meshes.push(crate::canvas3d_props::selection_ring(
                &interaction,
                at,
                color,
            )?);
        }
    }
    // only a shader material reads the clock: a scene without one is the same scene at every instant
    let time = if meshes.iter().any(|m| m.material.shader.is_some()) {
        elapsed_ms.map_or(0.0, |ms| (ms / 1000.0) as f32)
    } else {
        0.0
    };
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
        time,
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

fn check_canvas_keys(fields: &Fields) -> Result<(), CanvasSceneError> {
    for (key, value) in fields {
        let field = format!("canvas.{key}");
        match key.as_str() {
            "type" | "mode" | "drawables" | "children" | "camera" | "backgroundColor"
            | "pixelsPerUnit" | "showGrid" | "shadows" | "lighting" | "post" => {}
            // Interaction and DOM-wrapper props: they change no pixel of the scene.
            "tileClickEvent" | "unitClickEvent" | "tileHoverEvent" | "tileLeaveEvent"
            | "featureClickEvent" | "keyMap" | "keyUpMap" | "gestureMap" | "selectedId"
            | "onSelect" | "onMove" | "selectEvent" | "moveEvent" | "className" | "enter"
            | "enterDelay" | "skeleton" | "aria-label" | "aria-labelledby" | "aria-describedby"
            | "aria-live" | "aria-current" | "aria-pressed" | "aria-expanded" | "aria-selected"
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

mod fields;
mod flat;
mod material;
mod models;
/// `DrawFxLayer.tsx`'s declared defaults, and the family the bundled face answers to.
mod stage;
mod walk;
use self::fields::*;
use self::flat::*;
use self::material::*;
use self::models::*;
use self::stage::*;
use self::walk::*;

#[cfg(test)]
mod tests;
