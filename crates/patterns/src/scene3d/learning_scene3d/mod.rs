//! `physics-canvas`, `biology-canvas` and `chemistry-canvas` with `mode: "3d"`: the `canvas` mode:"3d" node the web's
//! `LearningScene3D` hands `Canvas3DHost` (the `learningScene3D.tsx` builders and each molecule's 3D branch), then
//! the [`Scene3D`] the canvas mapper builds from it. A declared prop the three host ignores in 3D is a typed error
//! naming it, never a silent drop.

use orbital_canvas_base::scene3d::model::Scene3D;
use orbital_canvas_map::canvas_scene3d::{scene_from_canvas_node_themed, SceneTheme};
use orbital_canvas_map::error::CanvasSceneError;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::f64::consts::PI;

type Fields = Map<String, Value>;
type P3 = [f64; 3];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pattern {
    Physics,
    Biology,
    Chemistry,
}

const SERIES_TONES: [&str; 8] = [
    "series-1", "series-2", "series-3", "series-4", "series-5", "series-6", "series-7", "series-8",
];

/// `@almadar/core` `DIAGRAM_TONES`.
const DIAGRAM_TONES: &[&str] = &[
    "ink",
    "label",
    "guide",
    "axis",
    "grid",
    "highlight",
    "fill",
    "series-1",
    "series-2",
    "series-3",
    "series-4",
    "series-5",
    "series-6",
    "series-7",
    "series-8",
    "primary",
    "accent",
    "success",
    "warning",
    "error",
    "info",
    "muted",
];

const LABEL_COLOR: &str = "ink";

/// Props every learning canvas declares: the scene's own, and the DOM wrapper's (they change no pixel of the scene).
const COMMON_KEYS: &[&str] = &[
    "type",
    "mode",
    "width",
    "height",
    "title",
    "backgroundColor",
    "series",
    "keyMap",
    "keyUpMap",
    "camera",
    "lighting",
    "post",
    "shapes",
    "readouts",
    "traces",
    "showGrid",
    "shadows",
    "interactive",
    "animate",
    "onShapeClick",
    "isLoading",
    "error",
    "className",
    "enter",
    "enterDelay",
    "skeleton",
    "aria-label",
    "aria-labelledby",
    "aria-describedby",
    "aria-live",
    "aria-current",
    "aria-pressed",
    "aria-expanded",
    "aria-selected",
    "aria-hidden",
    "aria-busy",
    "role",
    "lang",
    "dir",
    "tabIndex",
];

const PHYSICS_KEYS: &[&str] = &[
    "bodies",
    "particles",
    "constraints",
    "showVelocity",
    "showForces",
    "velocityScale",
    "forceScale",
    "sceneObjects",
    "trails",
    "vectors",
    "surface3d",
    "vectors3d",
    "vectorScale",
    "angles",
    "field",
];

const BIOLOGY_KEYS: &[&str] = &[
    "nodes",
    "edges",
    "compartments",
    "bands",
    "stages",
    "stageStyle",
    "helix",
    "helix3d",
];

const CHEMISTRY_KEYS: &[&str] = &[
    "atoms",
    "bonds",
    "arrows",
    "bondStyle",
    "containers",
    "equation",
    "equationColor",
    "lattice3d",
];

/// The 2D-only families each molecule logs and skips in 3D.
const PHYSICS_2D: &[&str] = &[
    "particles",
    "sceneObjects",
    "vectors",
    "angles",
    "field",
    "meters",
];
const BIOLOGY_2D: &[&str] = &["compartments", "bands", "stages", "helix"];
const CHEMISTRY_2D: &[&str] = &["containers"];
// shapes, readouts, traces, meters and the equation draw as the 2D overlay over the scene (learning_overlay3d)
const COMMON_2D: &[&str] = &["animate"];

/// The `canvas` mode:"3d" node `LearningScene3D` hands `Canvas3DHost` for a physics, biology or chemistry node:
/// drawables in scene cells with colors as authored (tone names resolve in the host stage), camera, lighting, post,
/// grid, shadows and orbit controls.
pub fn learning_canvas_node_3d(node: &Value) -> Result<Value, CanvasSceneError> {
    let object = node.as_object().ok_or(CanvasSceneError::NotAnObject)?;
    let fields = object
        .get("props")
        .and_then(Value::as_object)
        .unwrap_or(object);
    let name = object
        .get("type")
        .or_else(|| fields.get("type"))
        .and_then(Value::as_str)
        .ok_or_else(|| bad("type", "is required"))?;
    let pattern = match name {
        "physics-canvas" => Pattern::Physics,
        "biology-canvas" => Pattern::Biology,
        "chemistry-canvas" => Pattern::Chemistry,
        other => return Err(bad("type", format!("`{other}` has no learning 3D scene"))),
    };
    let path = name;
    let mode = opt_str(fields, "mode", path)?;
    if mode != Some("3d") {
        return Err(CanvasSceneError::UnsupportedMode(
            mode.unwrap_or("2d").to_owned(),
        ));
    }
    let (own, own_2d) = match pattern {
        Pattern::Physics => (PHYSICS_KEYS, PHYSICS_2D),
        Pattern::Biology => (BIOLOGY_KEYS, BIOLOGY_2D),
        Pattern::Chemistry => (CHEMISTRY_KEYS, CHEMISTRY_2D),
    };
    for key in fields.keys() {
        let key_str = key.as_str();
        let declared = [COMMON_KEYS, COMMON_2D, own, own_2d]
            .iter()
            .any(|list| list.contains(&key_str));
        if !declared {
            return Err(bad(&join(path, key), "not a declared prop"));
        }
    }
    for key in COMMON_2D.iter().chain(own_2d) {
        if present(fields, key).is_some_and(|v| !inert(v)) {
            return Err(bad(
                &join(path, key),
                "2D only: the three host ignores it in 3D",
            ));
        }
    }
    for key in ["width", "height"] {
        opt_num(fields, key, path)?;
    }
    if present(fields, "error").is_some() {
        return Err(bad(
            &join(path, "error"),
            "not yet drawn by orbital-canvas (P-025 step 2)",
        ));
    }

    let drawables = match pattern {
        Pattern::Physics => physics(fields, path)?,
        Pattern::Biology => biology(fields, path)?,
        Pattern::Chemistry => chemistry(fields, path)?,
    };

    let mut out = Fields::new();
    out.insert("type".into(), json!("canvas"));
    out.insert("mode".into(), json!("3d"));
    out.insert("drawables".into(), Value::Array(drawables));
    out.insert("camera".into(), camera(fields, path)?);
    out.insert(
        "showGrid".into(),
        json!(opt_bool(fields, "showGrid", path)?.unwrap_or(false)),
    );
    if let Some(shadows) = opt_bool(fields, "shadows", path)? {
        out.insert("shadows".into(), json!(shadows));
    }
    for key in [
        "backgroundColor",
        "series",
        "lighting",
        "post",
        "isLoading",
        "keyMap",
        "keyUpMap",
    ] {
        if let Some(value) = present(fields, key) {
            out.insert(key.into(), value.clone());
        }
    }
    Ok(Value::Object(out))
}

/// [`learning_canvas_node_3d`] through the host stage (`Canvas3DHost`'s color resolution) into the canvas mapper.
pub fn learning_scene3d(
    node: &Value,
    width: f32,
    height: f32,
    theme: &SceneTheme,
) -> Result<Scene3D, CanvasSceneError> {
    let mut canvas = learning_canvas_node_3d(node)?;
    resolve_host_colors(&mut canvas, theme)?;
    scene_from_canvas_node_themed(&canvas, width, height, theme)
}

/// `Canvas3DHost`'s front stage the canvas mapper does not run yet: `withSeries` overrides the series tones,
/// `resolveDrawableColors` + `markColor` (fallback `ink`) resolve every drawable color, and an empty
/// `backgroundColor` is the CSS `background: ""` the host paints, transparent.
pub fn resolve_host_colors(canvas: &mut Value, theme: &SceneTheme) -> Result<(), CanvasSceneError> {
    let fields = canvas
        .as_object_mut()
        .ok_or(CanvasSceneError::NotAnObject)?;
    let mut tones = theme.tones.clone();
    if let Some(series) = fields.remove("series").filter(|v| !v.is_null()) {
        let list = series
            .as_array()
            .ok_or_else(|| bad("canvas.series", "must be an array"))?;
        let mut colors = Vec::with_capacity(list.len());
        for (i, c) in list.iter().enumerate() {
            let field = format!("canvas.series[{i}]");
            let c = c
                .as_str()
                .ok_or_else(|| bad(&field, "must be a color string"))?;
            colors.push(mark_color(&theme.tones, c, SERIES_TONES[i % 8], &field)?);
        }
        if !colors.is_empty() {
            for (k, tone) in SERIES_TONES.iter().enumerate() {
                tones.insert((*tone).to_string(), colors[k % colors.len()].clone());
            }
        }
    }
    if fields.get("backgroundColor").and_then(Value::as_str) == Some("") {
        fields.insert("backgroundColor".into(), json!("rgba(0, 0, 0, 0)"));
    }
    if let Some(Value::Array(items)) = fields.get_mut("drawables") {
        resolve_items(items, &tones, "canvas.drawables")?;
    }
    Ok(())
}

fn resolve_items(
    items: &mut [Value],
    tones: &std::collections::BTreeMap<String, String>,
    path: &str,
) -> Result<(), CanvasSceneError> {
    for (i, item) in items.iter_mut().enumerate() {
        let item_path = format!("{path}[{i}]");
        let Some(f) = item.as_object_mut() else {
            continue;
        };
        let ty = f.get("type").and_then(Value::as_str).map(str::to_owned);
        match ty.as_deref() {
            Some("draw-mesh") => {
                if let Some(Value::Object(m)) = f.get_mut("material") {
                    for key in ["color", "emissive"] {
                        let field = format!("{item_path}.material.{key}");
                        if let Some(Value::String(c)) = m.get(key) {
                            let resolved = mark_color(tones, c, "ink", &field)?;
                            m.insert(key.into(), Value::String(resolved));
                        }
                    }
                }
            }
            Some("draw-text") => {
                if let Some(Value::String(c)) = f.get("color") {
                    let resolved = mark_color(tones, c, "ink", &join(&item_path, "color"))?;
                    f.insert("color".into(), Value::String(resolved));
                }
            }
            Some("draw-group") => {
                if let Some(Value::Array(inner)) = f.get_mut("items") {
                    resolve_items(inner, tones, &join(&item_path, "items"))?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// `markColor`: empty takes the fallback tone, a tone name the theme's color (white when the theme leaves it out,
/// what three keeps for `currentColor`), a literal passes through.
fn mark_color(
    tones: &std::collections::BTreeMap<String, String>,
    value: &str,
    fallback: &str,
    field: &str,
) -> Result<String, CanvasSceneError> {
    let tone = |name: &str| tones.get(name).cloned().unwrap_or_else(|| "#ffffff".into());
    if value.is_empty() {
        return Ok(tone(fallback));
    }
    if DIAGRAM_TONES.contains(&value) {
        return Ok(tone(value));
    }
    if value.starts_with("var(") || value.starts_with("color-mix(") {
        return Err(bad(
            field,
            format!("`{value}` resolves against the page's CSS, which the canvas mapper has no access to"),
        ));
    }
    Ok(value.to_owned())
}

/// `LearningScene3D`'s camera: mode defaults to perspective, zoom/fov/azimuth/elevation/target pass through,
/// `interactive` drives the orbit controls.
fn camera(fields: &Fields, path: &str) -> Result<Value, CanvasSceneError> {
    let camera_path = join(path, "camera");
    let mut out = Fields::new();
    let mut mode = json!("perspective");
    if let Some(c) = opt_obj(fields, "camera", path)? {
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
            &camera_path,
        )?;
        if present(c, "pos").is_some() {
            return Err(bad(
                &join(&camera_path, "pos"),
                "the three host ignores it for a learning canvas",
            ));
        }
        if present(c, "controls").is_some() {
            return Err(bad(
                &join(&camera_path, "controls"),
                "the three host ignores it for a learning canvas (orbit follows `interactive`)",
            ));
        }
        if let Some(m) = present(c, "mode") {
            mode = m.clone();
        }
        for key in ["zoom", "fov", "azimuth", "elevation", "target"] {
            if let Some(v) = present(c, key) {
                out.insert(key.into(), v.clone());
            }
        }
    }
    out.insert("mode".into(), mode);
    if let Some(interactive) = opt_bool(fields, "interactive", path)? {
        out.insert("controls".into(), json!(interactive));
    }
    Ok(Value::Object(out))
}

// ---------------------------------------------------------------------------
// physics-canvas
// ---------------------------------------------------------------------------

struct Body<'a> {
    id: Option<&'a str>,
    x: f64,
    y: f64,
    z: f64,
    radius: Option<f64>,
    color: Option<&'a str>,
    label: Option<&'a str>,
    v: P3,
    f: P3,
}

fn physics(fields: &Fields, path: &str) -> Result<Vec<Value>, CanvasSceneError> {
    let mut bodies = Vec::new();
    for (p, b) in items(fields, "bodies", path)? {
        check_keys(
            b,
            &[
                "id", "x", "y", "z", "radius", "color", "label", "vx", "vy", "vz", "fx", "fy", "fz",
            ],
            &p,
        )?;
        bodies.push(Body {
            id: opt_str(b, "id", &p)?,
            x: req_num(b, "x", &p)?,
            y: req_num(b, "y", &p)?,
            z: opt_num(b, "z", &p)?.unwrap_or(0.0),
            radius: opt_num(b, "radius", &p)?,
            color: opt_str(b, "color", &p)?,
            label: opt_str(b, "label", &p)?,
            v: [
                opt_num(b, "vx", &p)?.unwrap_or(0.0),
                opt_num(b, "vy", &p)?.unwrap_or(0.0),
                opt_num(b, "vz", &p)?.unwrap_or(0.0),
            ],
            f: [
                opt_num(b, "fx", &p)?.unwrap_or(0.0),
                opt_num(b, "fy", &p)?.unwrap_or(0.0),
                opt_num(b, "fz", &p)?.unwrap_or(0.0),
            ],
        });
    }
    let show_velocity = opt_bool(fields, "showVelocity", path)?.unwrap_or(true);
    let show_forces = opt_bool(fields, "showForces", path)?.unwrap_or(false);
    let velocity_scale = opt_num(fields, "velocityScale", path)?.unwrap_or(20.0);
    let force_scale = opt_num(fields, "forceScale", path)?.unwrap_or(20.0);
    let vector_scale = opt_num(fields, "vectorScale", path)?.unwrap_or(1.0);

    let mut out = Vec::new();
    let mut by_id: HashMap<&str, &Body> = HashMap::new();
    for b in &bodies {
        if let Some(id) = b.id.filter(|id| !id.is_empty()) {
            by_id.insert(id, b);
        }
    }

    for (p, c) in items(fields, "constraints", path)? {
        check_keys(c, &["from", "to", "color", "kind"], &p)?;
        let from = req_str(c, "from", &p)?;
        let to = req_str(c, "to", &p)?;
        match opt_str(c, "kind", &p)? {
            None | Some("rod") => {}
            Some("spring" | "string") => {
                return Err(bad(
                    &join(&p, "kind"),
                    "2D only: the three host draws every constraint as a rod in 3D",
                ))
            }
            Some(_) => return Err(bad(&join(&p, "kind"), "must be rod, spring or string")),
        }
        let color = opt_str(c, "color", &p)?.unwrap_or("ink");
        let (Some(a), Some(b)) = (by_id.get(from), by_id.get(to)) else {
            continue;
        };
        let rod_radius = (a.radius.unwrap_or(0.5).min(b.radius.unwrap_or(0.5)) * 0.15).max(0.05);
        if let Some(rod) = cylinder_between([a.x, a.y, a.z], [b.x, b.y, b.z], rod_radius, color) {
            out.push(rod);
        }
    }

    for (bi, b) in bodies.iter().enumerate() {
        let radius = b.radius.unwrap_or(0.5);
        let color = b.color.unwrap_or(SERIES_TONES[bi % 8]);
        out.push(mesh_sphere(
            b.id,
            [b.x, b.y, b.z],
            radius,
            color,
            None,
            None,
        ));
        if let Some(label) = b.label.filter(|l| !l.is_empty()) {
            out.push(billboard_label(
                label,
                [b.x, b.y, b.z + radius],
                LABEL_COLOR,
            ));
        }
        let arrow_radius = (radius * 0.15).max(0.05);
        let from = [b.x, b.y, b.z];
        if show_velocity && b.v.iter().any(|c| *c != 0.0) {
            let to = [
                b.x + b.v[0] * velocity_scale,
                b.y + b.v[1] * velocity_scale,
                b.z + b.v[2] * velocity_scale,
            ];
            out.extend(arrow_between(from, to, "series-3", arrow_radius, None));
        }
        if show_forces && b.f.iter().any(|c| *c != 0.0) {
            let to = [
                b.x + b.f[0] * force_scale,
                b.y + b.f[1] * force_scale,
                b.z + b.f[2] * force_scale,
            ];
            out.extend(arrow_between(from, to, "series-5", arrow_radius, None));
        }
    }

    for (p, t) in items(fields, "trails", path)? {
        check_keys(
            t,
            &["id", "points", "color", "width", "opacity", "fade"],
            &p,
        )?;
        opt_str(t, "id", &p)?;
        if opt_bool(t, "fade", &p)? == Some(true) {
            return Err(bad(
                &join(&p, "fade"),
                "2D only: the three host draws an opaque tube in 3D",
            ));
        }
        let mut points = Vec::new();
        for (pp, pt) in items(t, "points", &p)? {
            check_keys(pt, &["x", "y", "z"], &pp)?;
            points.push([
                req_num(pt, "x", &pp)?,
                req_num(pt, "y", &pp)?,
                opt_num(pt, "z", &pp)?.unwrap_or(0.0),
            ]);
        }
        if present(t, "points").is_none() {
            return Err(bad(&join(&p, "points"), "is required"));
        }
        out.extend(polyline_tube(
            &points,
            opt_num(t, "width", &p)?.unwrap_or(0.05),
            opt_str(t, "color", &p)?.unwrap_or("guide"),
            opt_num(t, "opacity", &p)?,
            128,
        ));
    }

    if let Some(s) = opt_obj(fields, "surface3d", path)? {
        let p = join(path, "surface3d");
        out.extend(height_field_mesh(s, &p)?);
    }

    for (p, v) in items(fields, "vectors3d", path)? {
        check_keys(
            v,
            &[
                "id", "x", "y", "z", "dx", "dy", "dz", "color", "label", "width",
            ],
            &p,
        )?;
        let from = [
            req_num(v, "x", &p)?,
            req_num(v, "y", &p)?,
            opt_num(v, "z", &p)?.unwrap_or(0.0),
        ];
        let delta = [
            req_num(v, "dx", &p)?,
            req_num(v, "dy", &p)?,
            opt_num(v, "dz", &p)?.unwrap_or(0.0),
        ];
        let to = [
            from[0] + delta[0] * vector_scale,
            from[1] + delta[1] * vector_scale,
            from[2] + delta[2] * vector_scale,
        ];
        let color = opt_str(v, "color", &p)?.unwrap_or("series-5");
        let width = opt_num(v, "width", &p)?.unwrap_or(0.08);
        out.extend(arrow_between(from, to, color, width, opt_str(v, "id", &p)?));
        if let Some(label) = opt_str(v, "label", &p)?.filter(|l| !l.is_empty()) {
            out.push(billboard_label(label, to, LABEL_COLOR));
        }
    }
    Ok(out)
}

/// `heightFieldMesh`: an `nx`×`ny` vertex sheet, two triangles per quad, one polyhedron per elevation band.
fn height_field_mesh(s: &Fields, path: &str) -> Result<Vec<Value>, CanvasSceneError> {
    check_keys(
        s,
        &[
            "nx",
            "ny",
            "heights",
            "spacing",
            "x",
            "y",
            "bands",
            "color",
            "opacity",
            "flatShading",
        ],
        path,
    )?;
    let nx = req_count(s, "nx", path)?;
    let ny = req_count(s, "ny", path)?;
    let heights_path = join(path, "heights");
    let heights = present(s, "heights")
        .ok_or_else(|| bad(&heights_path, "is required"))?
        .as_array()
        .ok_or_else(|| bad(&heights_path, "must be an array"))?
        .iter()
        .enumerate()
        .map(|(i, h)| number(h, &format!("{heights_path}[{i}]")))
        .collect::<Result<Vec<_>, _>>()?;
    if heights.len() != nx * ny {
        return Err(bad(
            &heights_path,
            format!(
                "holds {} heights for an {nx}x{ny} grid of {}",
                heights.len(),
                nx * ny
            ),
        ));
    }
    let spacing = opt_num(s, "spacing", path)?.unwrap_or(1.0);
    let cx = opt_num(s, "x", path)?.unwrap_or(0.0);
    let cy = opt_num(s, "y", path)?.unwrap_or(0.0);
    let color = opt_str(s, "color", path)?.unwrap_or("series-1");
    let opacity = opt_num(s, "opacity", path)?;
    let flat_shading = opt_bool(s, "flatShading", path)?.unwrap_or(true);
    let mut bands = Vec::new();
    for (p, b) in items(s, "bands", path)? {
        check_keys(b, &["min", "color"], &p)?;
        bands.push((opt_num(b, "min", &p)?, req_str(b, "color", &p)?));
    }
    bands.sort_by(|a, b| {
        a.0.unwrap_or(f64::NEG_INFINITY)
            .total_cmp(&b.0.unwrap_or(f64::NEG_INFINITY))
    });

    let mut vertices = Vec::with_capacity(nx * ny);
    for iy in 0..ny {
        for ix in 0..nx {
            vertices.push([
                cx + (ix as f64 - (nx as f64 - 1.0) / 2.0) * spacing,
                cy + (iy as f64 - (ny as f64 - 1.0) / 2.0) * spacing,
                heights[iy * nx + ix],
            ]);
        }
    }
    let mut groups: Vec<(Option<usize>, Vec<[usize; 3]>)> = Vec::new();
    for iy in 0..ny.saturating_sub(1) {
        for ix in 0..nx.saturating_sub(1) {
            let v00 = iy * nx + ix;
            let v10 = iy * nx + ix + 1;
            let v01 = (iy + 1) * nx + ix;
            let v11 = (iy + 1) * nx + ix + 1;
            for face in [[v00, v10, v01], [v10, v11, v01]] {
                let centroid =
                    (vertices[face[0]][2] + vertices[face[1]][2] + vertices[face[2]][2]) / 3.0;
                let mut band = None;
                for (bi, b) in bands.iter().enumerate() {
                    if b.0.unwrap_or(f64::NEG_INFINITY) <= centroid {
                        band = Some(bi);
                    }
                }
                match groups.iter_mut().find(|g| g.0 == band) {
                    Some(g) => g.1.push(face),
                    None => groups.push((band, vec![face])),
                }
            }
        }
    }
    let vertex_json: Vec<Value> = vertices.iter().map(|v| json!(v)).collect();
    Ok(groups
        .into_iter()
        .map(|(band, faces)| {
            let mut mesh = json!({
                "type": "draw-mesh",
                "shape": "polyhedron",
                "position": pos([0.0, 0.0, 0.0]),
                "vertices": vertex_json.clone(),
                "faces": faces,
                "pivot": "center",
                "material": {
                    "color": band.map_or(color, |bi| bands[bi].1),
                    "flatShading": flat_shading,
                    "side": "double",
                },
            });
            if let (Some(o), Some(m)) = (opacity, mesh.as_object_mut()) {
                m.insert("opacity".into(), json!(o));
            }
            mesh
        })
        .collect())
}

// ---------------------------------------------------------------------------
// biology-canvas
// ---------------------------------------------------------------------------

struct Node<'a> {
    id: Option<&'a str>,
    x: f64,
    y: f64,
    z: f64,
    radius: Option<f64>,
    color: Option<&'a str>,
    label: Option<&'a str>,
    shape: Option<&'a str>,
    opacity: Option<f64>,
}

const MESH_SHAPES: &[&str] = &[
    "box",
    "sphere",
    "capsule",
    "cylinder",
    "cone",
    "torus",
    "plane",
    "circle",
    "polyhedron",
];

fn biology(fields: &Fields, path: &str) -> Result<Vec<Value>, CanvasSceneError> {
    match opt_str(fields, "stageStyle", path)? {
        None | Some("timeline" | "ring") => {}
        Some(_) => return Err(bad(&join(path, "stageStyle"), "must be timeline or ring")),
    }
    let mut nodes = Vec::new();
    for (p, n) in items(fields, "nodes", path)? {
        check_keys(
            n,
            &[
                "id", "x", "y", "z", "radius", "color", "label", "kind", "shape", "opacity",
                "state",
            ],
            &p,
        )?;
        match opt_str(n, "kind", &p)? {
            None | Some("cell" | "organelle" | "molecule" | "organism") => {}
            Some(_) => {
                return Err(bad(
                    &join(&p, "kind"),
                    "must be cell, organelle, molecule or organism",
                ))
            }
        }
        match opt_str(n, "state", &p)? {
            None | Some("default") => {}
            Some("highlight" | "muted") => {
                return Err(bad(
                    &join(&p, "state"),
                    "2D only: the three host ignores it in 3D",
                ))
            }
            Some(_) => {
                return Err(bad(
                    &join(&p, "state"),
                    "must be default, highlight or muted",
                ))
            }
        }
        let shape = opt_str(n, "shape", &p)?;
        if let Some(s) = shape.filter(|s| !MESH_SHAPES.contains(s)) {
            return Err(bad(
                &join(&p, "shape"),
                format!("`{s}` is not a mesh shape"),
            ));
        }
        nodes.push(Node {
            id: opt_str(n, "id", &p)?,
            x: req_num(n, "x", &p)?,
            y: req_num(n, "y", &p)?,
            z: opt_num(n, "z", &p)?.unwrap_or(0.0),
            radius: opt_num(n, "radius", &p)?,
            color: opt_str(n, "color", &p)?,
            label: opt_str(n, "label", &p)?,
            shape,
            opacity: opt_num(n, "opacity", &p)?,
        });
    }
    let mut by_id: HashMap<&str, &Node> = HashMap::new();
    for n in &nodes {
        if let Some(id) = n.id.filter(|id| !id.is_empty()) {
            by_id.insert(id, n);
        }
    }

    let mut out = Vec::new();
    for (p, e) in items(fields, "edges", path)? {
        check_keys(e, &["from", "to", "color", "label", "directed"], &p)?;
        if opt_bool(e, "directed", &p)? == Some(true) {
            return Err(bad(
                &join(&p, "directed"),
                "2D only: the three host ignores it in 3D",
            ));
        }
        let from = req_str(e, "from", &p)?;
        let to = req_str(e, "to", &p)?;
        let color = opt_str(e, "color", &p)?.unwrap_or("ink");
        let label = opt_str(e, "label", &p)?;
        let (Some(a), Some(b)) = (by_id.get(from), by_id.get(to)) else {
            continue;
        };
        let edge_radius = (a.radius.unwrap_or(0.5).min(b.radius.unwrap_or(0.5)) * 0.12).max(0.04);
        if let Some(edge) = cylinder_between([a.x, a.y, a.z], [b.x, b.y, b.z], edge_radius, color) {
            out.push(edge);
        }
        if let Some(label) = label.filter(|l| !l.is_empty()) {
            out.push(billboard_label(
                label,
                [(a.x + b.x) / 2.0, (a.y + b.y) / 2.0, (a.z + b.z) / 2.0],
                LABEL_COLOR,
            ));
        }
    }

    for (ni, n) in nodes.iter().enumerate() {
        let radius = n.radius.unwrap_or(0.5);
        let color = n.color.unwrap_or(SERIES_TONES[ni % 8]);
        out.push(mesh_sphere(
            n.id,
            [n.x, n.y, n.z],
            radius,
            color,
            n.shape,
            n.opacity,
        ));
        if let Some(label) = n.label.filter(|l| !l.is_empty()) {
            out.push(billboard_label(
                label,
                [n.x, n.y, n.z + radius],
                LABEL_COLOR,
            ));
        }
    }

    if let Some(h) = opt_obj(fields, "helix3d", path)? {
        out.extend(helix(h, &join(path, "helix3d"))?);
    }
    Ok(out)
}

struct Rung<'a> {
    id: Option<&'a str>,
    color: &'a str,
    radius: Option<f64>,
    label: Option<&'a str>,
}

/// `helixDrawables`: two beaded backbone strands along scene y plus a rung, marker and label per index.
fn helix(h: &Fields, path: &str) -> Result<Vec<Value>, CanvasSceneError> {
    check_keys(
        h,
        &[
            "count",
            "rungs",
            "radius",
            "rise",
            "twistDeg",
            "strandAColor",
            "strandBColor",
            "backboneRadius",
            "rungRadius",
            "x",
            "y",
            "z",
            "unwoundCount",
            "unwindSpread",
        ],
        path,
    )?;
    let mut authored = Vec::new();
    for (p, r) in items(h, "rungs", path)? {
        check_keys(r, &["id", "color", "radius", "label"], &p)?;
        authored.push(Rung {
            id: opt_str(r, "id", &p)?,
            color: opt_str(r, "color", &p)?.unwrap_or("muted"),
            radius: opt_num(r, "radius", &p)?,
            label: opt_str(r, "label", &p)?,
        });
    }
    let count = match opt_count(h, "count", path)? {
        Some(c) => c,
        None => authored.len(),
    };
    let radius = opt_num(h, "radius", path)?.unwrap_or(1.0);
    let rise = opt_num(h, "rise", path)?.unwrap_or(0.34);
    let twist = opt_num(h, "twistDeg", path)?.unwrap_or(36.0) * (PI / 180.0);
    let color_a = opt_str(h, "strandAColor", path)?.unwrap_or("series-1");
    let color_b = opt_str(h, "strandBColor", path)?.unwrap_or("series-2");
    let backbone = opt_num(h, "backboneRadius", path)?.unwrap_or(0.16);
    let rung_radius = opt_num(h, "rungRadius", path)?.unwrap_or(0.12);
    let cx = opt_num(h, "x", path)?.unwrap_or(0.0);
    let cy = opt_num(h, "y", path)?.unwrap_or(0.0);
    let cz = opt_num(h, "z", path)?.unwrap_or(0.0);
    let unwound = opt_num(h, "unwoundCount", path)?.unwrap_or(0.0);
    let spread = opt_num(h, "unwindSpread", path)?.unwrap_or(1.8);

    let mut strand_a = Vec::with_capacity(count);
    let mut strand_b = Vec::with_capacity(count);
    for i in 0..count {
        let yi = cy + (i as f64 - (count as f64 - 1.0) / 2.0) * rise;
        let theta = i as f64 * twist;
        let s = if (i as f64) < unwound { spread } else { 1.0 };
        strand_a.push([
            cx + s * radius * theta.cos(),
            yi,
            cz + s * radius * theta.sin(),
        ]);
        strand_b.push([
            cx + s * radius * (theta + PI).cos(),
            yi,
            cz + s * radius * (theta + PI).sin(),
        ]);
    }

    let padding = Rung {
        id: None,
        color: "muted",
        radius: None,
        label: None,
    };
    let mut out = Vec::new();
    for i in 0..count {
        let id_a = format!("hx-a-{i}");
        let id_b = format!("hx-b-{i}");
        out.push(mesh_sphere(
            Some(id_a.as_str()),
            strand_a[i],
            backbone,
            color_a,
            None,
            None,
        ));
        out.push(mesh_sphere(
            Some(id_b.as_str()),
            strand_b[i],
            backbone,
            color_b,
            None,
            None,
        ));
        if i > 0 {
            out.extend(cylinder_between(
                strand_a[i - 1],
                strand_a[i],
                backbone,
                color_a,
            ));
            out.extend(cylinder_between(
                strand_b[i - 1],
                strand_b[i],
                backbone,
                color_b,
            ));
        }
        let rung = authored.get(i).unwrap_or(&padding);
        out.extend(cylinder_between(
            strand_a[i],
            strand_b[i],
            rung_radius,
            rung.color,
        ));
        let mid = [
            (strand_a[i][0] + strand_b[i][0]) / 2.0,
            (strand_a[i][1] + strand_b[i][1]) / 2.0,
            (strand_a[i][2] + strand_b[i][2]) / 2.0,
        ];
        let marker = rung.radius.unwrap_or(rung_radius);
        out.push(mesh_sphere(rung.id, mid, marker, rung.color, None, None));
        if let Some(label) = rung.label.filter(|l| !l.is_empty()) {
            out.push(billboard_label(
                label,
                [mid[0], mid[1], mid[2] + marker],
                LABEL_COLOR,
            ));
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// chemistry-canvas
// ---------------------------------------------------------------------------

struct Atom<'a> {
    id: Option<&'a str>,
    x: f64,
    y: f64,
    z: f64,
    element: Option<&'a str>,
    radius: Option<f64>,
    color: Option<&'a str>,
}

fn chemistry(fields: &Fields, path: &str) -> Result<Vec<Value>, CanvasSceneError> {
    match opt_str(fields, "bondStyle", path)? {
        None | Some("thick" | "parallel") => {}
        Some(_) => return Err(bad(&join(path, "bondStyle"), "must be thick or parallel")),
    }
    opt_str(fields, "equationColor", path)?;
    let mut atoms = Vec::new();
    for (p, a) in items(fields, "atoms", path)? {
        check_keys(
            a,
            &[
                "id",
                "x",
                "y",
                "z",
                "element",
                "radius",
                "color",
                "charge",
                "lonePairs",
            ],
            &p,
        )?;
        if opt_str(a, "charge", &p)?.is_some_and(|c| !c.is_empty()) {
            return Err(bad(
                &join(&p, "charge"),
                "2D only: the three host ignores it in 3D",
            ));
        }
        if opt_num(a, "lonePairs", &p)?.is_some_and(|n| n != 0.0) {
            return Err(bad(
                &join(&p, "lonePairs"),
                "2D only: the three host ignores it in 3D",
            ));
        }
        atoms.push(Atom {
            id: opt_str(a, "id", &p)?,
            x: req_num(a, "x", &p)?,
            y: req_num(a, "y", &p)?,
            z: opt_num(a, "z", &p)?.unwrap_or(0.0),
            element: opt_str(a, "element", &p)?,
            radius: opt_num(a, "radius", &p)?,
            color: opt_str(a, "color", &p)?,
        });
    }
    let mut element_tones: Vec<(&str, &str)> = Vec::new();
    for a in &atoms {
        if let Some(e) = a.element {
            if !element_tones.iter().any(|(k, _)| *k == e) {
                let tone = SERIES_TONES[element_tones.len() % 8];
                element_tones.push((e, tone));
            }
        }
    }
    let mut by_id: HashMap<&str, &Atom> = HashMap::new();
    for a in &atoms {
        if let Some(id) = a.id.filter(|id| !id.is_empty()) {
            by_id.insert(id, a);
        }
    }

    let mut out = Vec::new();
    for (p, b) in items(fields, "bonds", path)? {
        check_keys(b, &["from", "to", "type", "color", "state"], &p)?;
        match opt_str(b, "state", &p)? {
            None | Some("default") => {}
            Some("forming" | "breaking" | "highlight") => {
                return Err(bad(
                    &join(&p, "state"),
                    "2D only: the three host ignores it in 3D",
                ))
            }
            Some(_) => {
                return Err(bad(
                    &join(&p, "state"),
                    "must be default, forming, breaking or highlight",
                ))
            }
        }
        let order = opt_str(b, "type", &p)?;
        if let Some(t) = order.filter(|t| !matches!(*t, "single" | "double" | "triple")) {
            return Err(bad(
                &join(&p, "type"),
                format!("`{t}` must be single, double or triple"),
            ));
        }
        let from = req_str(b, "from", &p)?;
        let to = req_str(b, "to", &p)?;
        let color = opt_str(b, "color", &p)?.unwrap_or("ink");
        let (Some(a), Some(c)) = (by_id.get(from), by_id.get(to)) else {
            continue;
        };
        let start = [a.x, a.y, a.z];
        let end = [c.x, c.y, c.z];
        let perp = bond_perpendicular(start, end);
        let bond_radius = (a.radius.unwrap_or(0.45).min(c.radius.unwrap_or(0.45)) * 0.22).max(0.06);
        let step = bond_radius * 2.2;
        let offsets = match order {
            Some("double") => vec![-step, step],
            Some("triple") => vec![-step, 0.0, step],
            _ => vec![0.0],
        };
        for off in offsets {
            out.extend(cylinder_between(
                [
                    start[0] + perp[0] * off,
                    start[1] + perp[1] * off,
                    start[2] + perp[2] * off,
                ],
                [
                    end[0] + perp[0] * off,
                    end[1] + perp[1] * off,
                    end[2] + perp[2] * off,
                ],
                bond_radius,
                color,
            ));
        }
    }

    for (p, a) in items(fields, "arrows", path)? {
        check_keys(a, &["x", "y", "angle", "length", "color", "label"], &p)?;
        let x = req_num(a, "x", &p)?;
        let y = req_num(a, "y", &p)?;
        let angle = opt_num(a, "angle", &p)?.unwrap_or(0.0) * (PI / 180.0);
        let len = opt_num(a, "length", &p)?.unwrap_or(60.0);
        let x2 = x + angle.cos() * len;
        let y2 = y + angle.sin() * len;
        let color = opt_str(a, "color", &p)?.unwrap_or("ink");
        out.extend(arrow_between([x, y, 0.0], [x2, y2, 0.0], color, 0.08, None));
        if let Some(label) = opt_str(a, "label", &p)?.filter(|l| !l.is_empty()) {
            out.push(billboard_label(
                label,
                [(x + x2) / 2.0, (y + y2) / 2.0, 0.0],
                LABEL_COLOR,
            ));
        }
    }

    for a in &atoms {
        let radius = a.radius.unwrap_or(0.45);
        let tone = a
            .element
            .and_then(|e| element_tones.iter().find(|(k, _)| *k == e))
            .map_or("series-1", |(_, t)| *t);
        let color = a.color.unwrap_or(tone);
        out.push(mesh_sphere(
            a.id,
            [a.x, a.y, a.z],
            radius,
            color,
            None,
            None,
        ));
        if let Some(element) = a.element.filter(|e| !e.is_empty()) {
            out.push(billboard_label(
                element,
                [a.x, a.y, a.z + radius],
                LABEL_COLOR,
            ));
        }
    }

    if let Some(l) = opt_obj(fields, "lattice3d", path)? {
        out.extend(lattice(l, &join(path, "lattice3d"))?);
    }
    Ok(out)
}

/// `bondPerpendicular`: the unit vector across the bond on the ground plane; any horizontal axis for a vertical bond.
fn bond_perpendicular(a: P3, b: P3) -> P3 {
    let px = b[1] - a[1];
    let py = -(b[0] - a[0]);
    let len = (px * px + py * py).sqrt();
    if len < 1e-6 {
        return [1.0, 0.0, 0.0];
    }
    [px / len, py / len, 0.0]
}

struct Site<'a> {
    key: &'a str,
    d: P3,
    element: Option<&'a str>,
    color: &'a str,
    radius: f64,
    edge: [bool; 3],
}

/// `latticeDrawables`: a block of unit cells centered on the origin from a fractional basis and integer-offset bonds.
fn lattice(l: &Fields, path: &str) -> Result<Vec<Value>, CanvasSceneError> {
    check_keys(
        l,
        &[
            "basis",
            "nx",
            "ny",
            "nz",
            "latticeConstant",
            "bonds",
            "bondRadius",
            "highlightCell",
            "dimColor",
            "showLabels",
            "selectedId",
            "selectedColor",
        ],
        path,
    )?;
    if present(l, "basis").is_none() {
        return Err(bad(&join(path, "basis"), "is required"));
    }
    let mut basis = Vec::new();
    for (i, (p, s)) in items(l, "basis", path)?.into_iter().enumerate() {
        check_keys(
            s,
            &[
                "key", "dx", "dy", "dz", "element", "color", "radius", "xEdge", "yEdge", "zEdge",
            ],
            &p,
        )?;
        basis.push(Site {
            key: req_str(s, "key", &p)?,
            d: [
                req_num(s, "dx", &p)?,
                req_num(s, "dy", &p)?,
                req_num(s, "dz", &p)?,
            ],
            element: opt_str(s, "element", &p)?,
            color: opt_str(s, "color", &p)?.unwrap_or(SERIES_TONES[i % 8]),
            radius: opt_num(s, "radius", &p)?.unwrap_or(0.3),
            edge: [
                opt_bool(s, "xEdge", &p)?.unwrap_or(false),
                opt_bool(s, "yEdge", &p)?.unwrap_or(false),
                opt_bool(s, "zEdge", &p)?.unwrap_or(false),
            ],
        });
    }
    let n = [
        opt_count(l, "nx", path)?.unwrap_or(2),
        opt_count(l, "ny", path)?.unwrap_or(2),
        opt_count(l, "nz", path)?.unwrap_or(2),
    ];
    let a = opt_num(l, "latticeConstant", path)?.unwrap_or(2.0);
    let bond_radius = opt_num(l, "bondRadius", path)?.unwrap_or(0.06);
    let highlight_cell = opt_bool(l, "highlightCell", path)?.unwrap_or(false);
    let dim_color = opt_str(l, "dimColor", path)?.unwrap_or("muted");
    let show_labels = opt_bool(l, "showLabels", path)?.unwrap_or(false);
    let selected_id = opt_str(l, "selectedId", path)?;
    let selected_color = opt_str(l, "selectedColor", path)?.unwrap_or("highlight");
    let extent = |site: &Site, axis: usize| n[axis] + usize::from(site.edge[axis]);

    let mut pos_by_key: HashMap<String, P3> = HashMap::new();
    let mut in_cell_by_key: HashMap<String, bool> = HashMap::new();
    let mut out = Vec::new();
    for site in &basis {
        for i in 0..extent(site, 0) {
            for j in 0..extent(site, 1) {
                for k in 0..extent(site, 2) {
                    let key = format!("{}-{i}-{j}-{k}", site.key);
                    let ijk = [i as f64, j as f64, k as f64];
                    let in_cell = (0..3).all(|ax| ijk[ax] + site.d[ax] <= 1.0);
                    let p: P3 = std::array::from_fn(|ax| {
                        (ijk[ax] + site.d[ax]) * a - (n[ax] as f64 * a) / 2.0
                    });
                    let id = format!("lat-{key}");
                    let selected = selected_id == Some(id.as_str());
                    let color = if selected {
                        selected_color
                    } else if highlight_cell && !in_cell {
                        dim_color
                    } else {
                        site.color
                    };
                    let radius = site.radius * if selected { 1.4 } else { 1.0 };
                    out.push(mesh_sphere(Some(id.as_str()), p, radius, color, None, None));
                    if let Some(element) = site.element.filter(|e| show_labels && !e.is_empty()) {
                        out.push(billboard_label(
                            element,
                            [p[0], p[1], p[2] + radius],
                            LABEL_COLOR,
                        ));
                    }
                    pos_by_key.insert(key.clone(), p);
                    in_cell_by_key.insert(key, in_cell);
                }
            }
        }
    }

    let mut basis_by_key: HashMap<&str, &Site> = HashMap::new();
    for site in &basis {
        basis_by_key.insert(site.key, site);
    }
    for (p, b) in items(l, "bonds", path)? {
        check_keys(b, &["from", "to", "dx", "dy", "dz", "color"], &p)?;
        let from = req_str(b, "from", &p)?;
        let to = req_str(b, "to", &p)?;
        let shift = [
            opt_int(b, "dx", &p)?.unwrap_or(0),
            opt_int(b, "dy", &p)?.unwrap_or(0),
            opt_int(b, "dz", &p)?.unwrap_or(0),
        ];
        let color = opt_str(b, "color", &p)?.unwrap_or("ink");
        let (Some(&fs), Some(&ts)) = (basis_by_key.get(from), basis_by_key.get(to)) else {
            continue;
        };
        for i in 0..extent(fs, 0) {
            for j in 0..extent(fs, 1) {
                for k in 0..extent(fs, 2) {
                    let t = [
                        i as i64 + shift[0],
                        j as i64 + shift[1],
                        k as i64 + shift[2],
                    ];
                    if (0..3).any(|ax| t[ax] < 0 || t[ax] >= extent(ts, ax) as i64) {
                        continue;
                    }
                    let from_key = format!("{}-{i}-{j}-{k}", fs.key);
                    let to_key = format!("{}-{}-{}-{}", ts.key, t[0], t[1], t[2]);
                    let (Some(fp), Some(tp)) = (pos_by_key.get(&from_key), pos_by_key.get(&to_key))
                    else {
                        continue;
                    };
                    let dimmed = highlight_cell
                        && !(in_cell_by_key.get(&from_key) == Some(&true)
                            && in_cell_by_key.get(&to_key) == Some(&true));
                    let c = if dimmed { dim_color } else { color };
                    out.extend(cylinder_between(*fp, *tp, bond_radius, c));
                }
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// The learningScene3D builders.
// ---------------------------------------------------------------------------

fn pos(p: P3) -> Value {
    json!({ "x": p[0], "y": p[1], "z": p[2] })
}

/// `meshSphere`: a primitive centred at the scene point, `id`-tagged for click.
fn mesh_sphere(
    id: Option<&str>,
    p: P3,
    radius: f64,
    color: &str,
    shape: Option<&str>,
    opacity: Option<f64>,
) -> Value {
    let mut m = Fields::new();
    m.insert("type".into(), json!("draw-mesh"));
    if let Some(id) = id {
        m.insert("id".into(), json!(id));
    }
    m.insert("shape".into(), json!(shape.unwrap_or("sphere")));
    m.insert("position".into(), pos(p));
    m.insert("radius".into(), json!(radius));
    m.insert("pivot".into(), json!("center"));
    m.insert("segments".into(), json!(24));
    m.insert("material".into(), json!({ "color": color }));
    if let Some(o) = opacity {
        m.insert("opacity".into(), json!(o));
    }
    Value::Object(m)
}

/// `axisRotation`: the Euler angles that align a Y-axis primitive with from→to (scene z is world Y, scene y world Z).
fn axis_rotation(from: P3, to: P3) -> [f64; 3] {
    let wx = to[0] - from[0];
    let wy = to[2] - from[2];
    let wz = to[1] - from[1];
    let len = (wx * wx + wy * wy + wz * wz).sqrt();
    let cos = if len > 0.0 { wy / len } else { 1.0 };
    let tilt = cos.clamp(-1.0, 1.0).acos();
    [0.0, wz.atan2(-wx), tilt]
}

fn segment_length(from: P3, to: P3) -> f64 {
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// `cylinderBetween`: a cylinder spanning from→to centred on the midpoint; none for a degenerate segment.
fn cylinder_between(from: P3, to: P3, radius: f64, color: &str) -> Option<Value> {
    let len = segment_length(from, to);
    if len < 1e-6 {
        return None;
    }
    Some(json!({
        "type": "draw-mesh",
        "shape": "cylinder",
        "position": pos([(from[0] + to[0]) / 2.0, (from[1] + to[1]) / 2.0, (from[2] + to[2]) / 2.0]),
        "radius": radius,
        "height": len,
        "rotation": axis_rotation(from, to),
        "pivot": "center",
        "segments": 12,
        "material": { "color": color },
    }))
}

/// `arrowBetween`: a draw-group at `from` holding a shaft cylinder and a tip cone (radius 3× the shaft, length
/// min(8× the shaft, 35% of the arrow)).
fn arrow_between(
    from: P3,
    to: P3,
    color: &str,
    shaft_radius: f64,
    id: Option<&str>,
) -> Option<Value> {
    let len = segment_length(from, to);
    if len < 1e-6 {
        return None;
    }
    let tip_len = (shaft_radius * 8.0).min(len * 0.35);
    let k = (len - tip_len) / len;
    let delta = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let shaft_end = [delta[0] * k, delta[1] * k, delta[2] * k];
    let shaft = cylinder_between([0.0; 3], shaft_end, shaft_radius, color);
    let tip_actual = segment_length(shaft_end, delta);
    let tip = json!({
        "type": "draw-mesh",
        "shape": "cone",
        "position": pos([
            (shaft_end[0] + delta[0]) / 2.0,
            (shaft_end[1] + delta[1]) / 2.0,
            (shaft_end[2] + delta[2]) / 2.0,
        ]),
        "radius": shaft_radius * 3.0,
        "height": tip_actual,
        "rotation": axis_rotation(shaft_end, delta),
        "pivot": "center",
        "segments": 12,
        "material": { "color": color },
    });
    let items: Vec<Value> = if tip_actual < 1e-6 {
        shaft.into_iter().collect()
    } else {
        shaft.into_iter().chain([tip]).collect()
    };
    let mut g = Fields::new();
    g.insert("type".into(), json!("draw-group"));
    if let Some(id) = id {
        g.insert("id".into(), json!(id));
    }
    g.insert("position".into(), pos(from));
    g.insert("items".into(), Value::Array(items));
    Some(Value::Object(g))
}

/// `billboardLabel`: a text label at the scene point.
fn billboard_label(text: &str, p: P3, color: &str) -> Value {
    json!({ "type": "draw-text", "text": text, "position": pos(p), "color": color })
}

/// `polylineTube`: chained cylinders through `points`, uniformly downsampled to `max_segments` (first and last kept).
fn polyline_tube(
    points: &[P3],
    radius: f64,
    color: &str,
    opacity: Option<f64>,
    max_segments: usize,
) -> Vec<Value> {
    let kept: Vec<P3>;
    let pts: &[P3] = if points.len() > max_segments + 1 {
        let step = (points.len() - 1) as f64 / max_segments as f64;
        let mut picked = Vec::with_capacity(max_segments + 1);
        picked.push(points[0]);
        for s in 1..max_segments {
            picked.push(points[(s as f64 * step + 0.5).floor() as usize]);
        }
        picked.push(points[points.len() - 1]);
        kept = picked;
        &kept
    } else {
        points
    };
    pts.windows(2)
        .filter_map(|w| cylinder_between(w[0], w[1], radius, color))
        .map(|mut seg| {
            if let (Some(o), Some(m)) = (opacity, seg.as_object_mut()) {
                m.insert("opacity".into(), json!(o));
            }
            seg
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Reading the node.
// ---------------------------------------------------------------------------

fn bad(field: &str, why: impl std::fmt::Display) -> CanvasSceneError {
    CanvasSceneError::Unsupported3DContract(format!("{field}: {why}"))
}

fn join(path: &str, key: &str) -> String {
    format!("{path}.{key}")
}

fn present<'a>(f: &'a Fields, key: &str) -> Option<&'a Value> {
    f.get(key).filter(|v| !v.is_null())
}

/// A value that asks nothing of the scene: an empty list, map or string, or false.
fn inert(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Bool(b) => !b,
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        Value::Number(_) => false,
    }
}

/// A list item (`bodies[0]`, `nodes[2]`, …) is a row of the program's data: the fields the pattern draws are read and
/// a row's own other fields (`mass`, `period`, …) are left alone, as three's builders leave them; the pattern's own props
/// are checked strictly.
fn check_keys(f: &Fields, allowed: &[&str], path: &str) -> Result<(), CanvasSceneError> {
    if path.ends_with(']') {
        return Ok(());
    }
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

fn opt_num(f: &Fields, key: &str, path: &str) -> Result<Option<f64>, CanvasSceneError> {
    present(f, key)
        .map(|v| number(v, &join(path, key)))
        .transpose()
}

fn req_num(f: &Fields, key: &str, path: &str) -> Result<f64, CanvasSceneError> {
    opt_num(f, key, path)?.ok_or_else(|| bad(&join(path, key), "is required"))
}

fn opt_int(f: &Fields, key: &str, path: &str) -> Result<Option<i64>, CanvasSceneError> {
    opt_num(f, key, path)?
        .map(|n| {
            if n.fract() == 0.0 && n.abs() < 9.0e15 {
                Ok(n as i64)
            } else {
                Err(bad(&join(path, key), format!("{n} must be an integer")))
            }
        })
        .transpose()
}

fn opt_count(f: &Fields, key: &str, path: &str) -> Result<Option<usize>, CanvasSceneError> {
    opt_int(f, key, path)?
        .map(|n| {
            usize::try_from(n)
                .map_err(|_| bad(&join(path, key), format!("{n} must be zero or more")))
        })
        .transpose()
}

fn req_count(f: &Fields, key: &str, path: &str) -> Result<usize, CanvasSceneError> {
    opt_count(f, key, path)?.ok_or_else(|| bad(&join(path, key), "is required"))
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

fn req_str<'a>(f: &'a Fields, key: &str, path: &str) -> Result<&'a str, CanvasSceneError> {
    opt_str(f, key, path)?.ok_or_else(|| bad(&join(path, key), "is required"))
}

fn opt_obj<'a>(
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

/// A list of objects under `key` (absent is empty), each with its field path.
fn items<'a>(
    f: &'a Fields,
    key: &str,
    path: &str,
) -> Result<Vec<(String, &'a Fields)>, CanvasSceneError> {
    let Some(list) = present(f, key) else {
        return Ok(Vec::new());
    };
    let list_path = join(path, key);
    list.as_array()
        .ok_or_else(|| bad(&list_path, "must be an array"))?
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let item_path = format!("{list_path}[{i}]");
            item.as_object()
                .map(|o| (item_path.clone(), o))
                .ok_or_else(|| bad(&item_path, "must be an object"))
        })
        .collect()
}

#[cfg(test)]
mod tests;
