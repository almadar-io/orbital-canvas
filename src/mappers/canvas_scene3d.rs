//! `canvas` with `mode: "3d"` → [`Scene3D`]: the scene the three.js host (`Canvas3DHost`, `Drawable3D`, `DrawMesh3D`,
//! `Lighting3D`) builds today, rebuilt as a retained scene. Anything the pattern declares that this slice does not draw
//! is a typed error naming the field, never a silent drop.

use super::game_canvas_2d::{parse_css_color, to_drawable_nodes, CanvasSceneError};
use crate::scene3d::geometry;
use crate::scene3d::model::{
    srgb_to_linear, srgb_to_linear_rgb, AmbientLight, Camera, DirectionalLight, Grid,
    HemisphereLight, Lights, Material, MaterialKind, MeshData, MeshInstance, PointLight,
    Projection, Scene3D, ShadowSettings, Side, ToneMapping,
};
use glam::{DMat4, DVec3};
use serde_json::{Map, Value};
use std::f64::consts::FRAC_PI_2;

type Fields = Map<String, Value>;

const STEP_HOST: u8 = 2;
const STEP_PARITY: u8 = 4;

/// The theme's scene axis as `canvasTheme.ts` resolves it from CSS variables. [`Default`] holds that resolver's
/// fallbacks, which is what the three host draws under a theme that declares none of these variables.
#[derive(Debug, Clone, PartialEq)]
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

pub fn scene_from_canvas_node(
    node: &Value,
    width: f32,
    height: f32,
) -> Result<Scene3D, CanvasSceneError> {
    scene_from_canvas_node_themed(node, width, height, &SceneTheme::default())
}

pub fn scene_from_canvas_node_themed(
    node: &Value,
    width: f32,
    height: f32,
    theme: &SceneTheme,
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
    if let Some(post) = opt_object(fields, "post", "canvas")? {
        check_keys(post, &["bloom", "vignette"], "canvas.post")?;
        for key in ["bloom", "vignette"] {
            if present(post, key).is_some() {
                return Err(undrawn(&format!("canvas.post.{key}"), STEP_PARITY));
            }
        }
    }

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

    let bounds = scene_bounds(&nodes)?;
    let shadows = opt_bool(fields, "shadows", "canvas")?.unwrap_or(true);
    let mut walk = Walk {
        theme,
        meshes: Vec::new(),
    };
    let place = Placement {
        cell,
        off_x: -bounds.min_x * cell,
        off_z: -bounds.min_z * cell,
    };
    for (path, node) in &nodes {
        walk.drawable(node, path, DMat4::IDENTITY, place, 1.0)?;
    }

    let camera = camera(opt_object(fields, "camera", "canvas")?, &bounds, cell)?;
    let (lights, tone_mapping) = lights(opt_object(fields, "lighting", "canvas")?, shadows, theme)?;
    let background = match present(fields, "backgroundColor") {
        Some(value) => rgba(value, "canvas.backgroundColor")?,
        None => match &theme.ground {
            Some(ground) => rgba(&Value::String(ground.clone()), "theme --surface-diagram")?,
            None => [0.0; 4],
        },
    };
    let grid = if show_grid {
        Some(grid(&bounds, cell, theme)?)
    } else {
        None
    };
    Ok(Scene3D {
        camera,
        background,
        lights,
        shadows,
        tone_mapping,
        meshes: walk.meshes,
        grid,
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
            "isLoading" | "editable" => {
                if opt_bool(fields, key, "canvas")? == Some(true) {
                    return Err(undrawn(&field, STEP_HOST));
                }
            }
            "showMinimap" | "fit" | "showCoordinates" | "showTileInfo" => {
                if opt_bool(fields, key, "canvas")? == Some(true) {
                    return Err(undrawn(&field, STEP_PARITY));
                }
            }
            "fogOfWar" => {
                if !value.is_null() && !value.as_array().is_some_and(Vec::is_empty) {
                    return Err(undrawn(&field, STEP_PARITY));
                }
            }
            "projection" | "tileWidth" | "backgroundImage" | "worldWidth" | "worldHeight"
            | "unitScale" => {
                if !value.is_null() {
                    return Err(undrawn(&field, STEP_PARITY));
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
    for (path, node) in nodes {
        let Some(fields) = node.as_object() else {
            continue;
        };
        if !matches!(
            fields.get("type").and_then(Value::as_str),
            Some("draw-mesh" | "draw-group")
        ) {
            continue;
        }
        let pos = required_pos(fields, path)?;
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
    if present(c, "pos").is_some() {
        return Err(undrawn("canvas.camera.pos", STEP_HOST));
    }
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
    if follow {
        for (key, set) in [
            ("zoom", zoom.is_some()),
            ("azimuth", azimuth.is_some()),
            ("elevation", elevation.is_some()),
        ] {
            if set {
                return Err(undrawn(
                    &format!("canvas.camera.{key} in {mode} mode"),
                    STEP_HOST,
                ));
            }
        }
    } else {
        if target.is_some() {
            return Err(undrawn(
                &format!("canvas.camera.target in {mode} mode"),
                STEP_HOST,
            ));
        }
        if elevation.is_some() && mode != "perspective" {
            return Err(undrawn(
                &format!("canvas.camera.elevation in {mode} mode"),
                STEP_HOST,
            ));
        }
    }

    let (span_x, span_z) = (b.max_x - b.min_x, b.max_z - b.min_z);
    let center = DVec3::new(span_x / 2.0 * cell, 0.0, span_z / 2.0 * cell);
    let (eye, look) = if follow {
        let look = match target {
            Some(t) => DVec3::new(
                t[0] * cell - b.min_x * cell,
                t[2],
                t[1] * cell - b.min_z * cell,
            ),
            None => center,
        };
        let offset = if mode == "chase" {
            DVec3::new(0.0, 4.0, -7.0)
        } else {
            DVec3::new(0.0, 12.0, 14.0)
        };
        (look + offset, look)
    } else {
        let size = (span_x * cell).max(span_z * cell).max(4.0);
        let d = size / zoom.unwrap_or(1.0);
        let (cx, cz) = (center.x, center.z);
        let base = match mode {
            "isometric" => DVec3::new(cx + d, d * 0.8, cz + d),
            "top-down" => DVec3::new(cx, d * 2.0, cz + d * 0.35),
            "front" => DVec3::new(cx, d * 0.32, cz + d * 1.15),
            _ => match elevation {
                None => DVec3::new(cx + d, d, cz + d),
                Some(el) => {
                    let el = el.clamp(0.0, FRAC_PI_2 - 0.1);
                    DVec3::new(cx + d, d * el.tan(), cz + d)
                }
            },
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
        Some("room") => return Err(undrawn("canvas.lighting.environment", STEP_PARITY)),
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
}

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
        match ty {
            "draw-mesh" => self.mesh(fields, path, parent, place, opacity),
            "draw-group" => self.group(fields, path, parent, place, opacity),
            "draw-sprite" | "draw-shape" | "draw-text" | "draw-sprite-layer"
            | "draw-shape-layer" | "draw-text-layer" | "draw-fx-layer" | "draw-skinned-mesh" => {
                Err(undrawn(&format!("{ty} at {path}"), STEP_PARITY))
            }
            other => Err(bad(
                &join(path, "type"),
                format!("`{other}` is not a drawable"),
            )),
        }
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
        for key in ["bone", "animation", "clip"] {
            if present(f, key).is_some() {
                return Err(undrawn(&join(path, key), STEP_PARITY));
            }
        }
        if opt_bool(f, "skeleton", path)? == Some(true) {
            return Err(undrawn(&join(path, "skeleton"), STEP_PARITY));
        }
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
        let matrix = parent
            * DMat4::from_translation(world)
            * euler_xyz(rotation)
            * DMat4::from_scale(DVec3::splat(scale));
        let opacity = opt_number(f, "opacity", path)?.unwrap_or(1.0) * opacity;
        let items = present(f, "items").ok_or_else(|| bad(&join(path, "items"), "is required"))?;
        if !items.is_array() {
            return Err(bad(&join(path, "items"), "must be an array"));
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
        for key in ["skin", "outline", "animation"] {
            if present(f, key).is_some() {
                return Err(undrawn(&join(path, key), STEP_PARITY));
            }
        }
        if self.theme.outline > 0.0 {
            return Err(undrawn(
                &format!("{path}.outline (theme --material-outline)"),
                STEP_PARITY,
            ));
        }
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
        let (mesh, lift) = match shape {
            "box" => (
                geo(geometry::cuboid(w as f32, h as f32, d as f32))?,
                h / 2.0,
            ),
            "sphere" => (geo(geometry::sphere(r as f32, seg, seg))?, r),
            "capsule" => {
                let caps = js_round(seg as f64 / 2.0).max(2.0) as usize;
                (
                    geo(geometry::capsule(r as f32, h as f32, caps, seg))?,
                    h / 2.0 + r,
                )
            }
            "cylinder" => {
                let top = dim("radiusTop", radius)? * cell;
                let bottom = dim("radiusBottom", radius)? * cell;
                (
                    geo(geometry::cylinder(top as f32, bottom as f32, h as f32, seg))?,
                    h / 2.0,
                )
            }
            "cone" => (
                geo(geometry::cylinder(0.0, r as f32, h as f32, seg))?,
                h / 2.0,
            ),
            "torus" => {
                let tube = dim("tube", radius / 3.0)? * cell;
                let radial = js_round(seg as f64 / 2.0).max(3.0) as usize;
                (
                    geo(geometry::torus(r as f32, tube as f32, radial, seg))?,
                    r + tube,
                )
            }
            "plane" => (geo(geometry::plane(w as f32, d as f32))?, 0.0),
            "circle" => (geo(geometry::circle(r as f32, seg))?, 0.0),
            _ => polyhedron(f, path, cell)?,
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
        // three's GROUND_ROTATION lays plane and circle flat; it adds to the authored Euler angles.
        let oriented = match shape {
            "plane" | "circle" => euler_xyz([-FRAC_PI_2 + rx, ry, rz]),
            _ => euler_xyz([rx, ry, rz]),
        };
        let transform = parent
            * DMat4::from_translation(world)
            * DMat4::from_translation(DVec3::new(0.0, lift, 0.0))
            * oriented;

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
        self.meshes.push(MeshInstance {
            mesh,
            transform: transform.as_mat4().to_cols_array(),
            material,
            cast_shadow: opt_bool(f, "castShadow", path)?.unwrap_or(true),
            receive_shadow: opt_bool(f, "receiveShadow", path)?.unwrap_or(true),
        });
        Ok(())
    }
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
        MaterialKind::Physical => MATERIAL_KEYS,
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

fn kind_name(kind: MaterialKind) -> &'static str {
    match kind {
        MaterialKind::Basic => "basic",
        MaterialKind::Standard => "standard",
        MaterialKind::Physical => "physical",
        MaterialKind::Toon => "toon",
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
            format!("`{s}` is not #hex, rgb() or rgba() (theme tones and var() resolve only in the web host)"),
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
        let outline = message(scene(json!({
            "drawables": [{"type": "draw-mesh", "position": {"x": 0, "y": 0}, "outline": {"width": 0.1}}]
        })));
        assert_eq!(
            outline,
            "drawables[0].outline: not yet drawn by orbital-canvas (P-025 step 4)"
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
        let sprite = message(scene(
            json!({"drawables": [{"type": "draw-sprite", "position": {"x": 0, "y": 0}}]}),
        ));
        assert_eq!(
            sprite,
            "draw-sprite at drawables[0]: not yet drawn by orbital-canvas (P-025 step 4)"
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
}
