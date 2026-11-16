//! The stage a 3D canvas sets: its camera and orbit pose, lights, grid, post-processing, bounds and interaction.

use super::*;

/// `post`: Effects3D mounts bloom and vignette with these defaults when the key is present.
pub(super) fn post_settings(
    post: Option<&Fields>,
) -> Result<Option<orbital_canvas_base::scene3d::model::PostSettings>, CanvasSceneError> {
    use orbital_canvas_base::scene3d::model::{BloomSettings, PostSettings, VignetteSettings};
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
pub(super) fn interaction(
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
pub(super) fn grid(
    bounds: &Bounds,
    cell: f64,
    theme: &SceneTheme,
) -> Result<Grid, CanvasSceneError> {
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

#[derive(Debug, Clone, Copy)]
pub(super) struct Bounds {
    pub(super) min_x: f64,
    pub(super) max_x: f64,
    pub(super) min_z: f64,
    pub(super) max_z: f64,
}

/// `collectDrawnItems` over the top-level list: scene `x` spans world X, scene `y` spans world Z.
pub(super) fn scene_bounds(nodes: &[(String, Value)]) -> Result<Bounds, CanvasSceneError> {
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

pub(super) fn camera(
    fields: Option<&Fields>,
    b: &Bounds,
    cell: f64,
) -> Result<Camera, CanvasSceneError> {
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
pub(super) fn orbit_controls_pose(position: DVec3, target: DVec3) -> DVec3 {
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

pub(super) fn lights(
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
