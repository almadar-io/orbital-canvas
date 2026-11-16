//! A drawable's 3D material and its animated colours.

use super::*;

/// Seconds from `material.anchor` (epoch ms) to the frame's epoch, in f64 before the f32 narrowing; 0 without either.
pub(super) fn anchor_since(
    m: &Fields,
    path: &str,
    epoch_ms: Option<f64>,
) -> Result<f32, CanvasSceneError> {
    let Some(anchor) = opt_number(m, "anchor", path)? else {
        return Ok(0.0);
    };
    Ok(epoch_ms.map_or(0.0, |epoch| ((epoch - anchor) / 1000.0) as f32))
}

pub(super) fn material(
    m: Option<&Fields>,
    path: &str,
    opacity: f64,
    theme: &SceneTheme,
    epoch_ms: Option<f64>,
) -> Result<Material, CanvasSceneError> {
    let empty = Fields::new();
    let m = m.unwrap_or(&empty);
    check_keys(m, MATERIAL_KEYS, path)?;
    let kind = match opt_str(m, "kind", path)? {
        None | Some("standard") => MaterialKind::Standard,
        Some("physical") => MaterialKind::Physical,
        Some("toon") => MaterialKind::Toon,
        Some("basic") => MaterialKind::Basic,
        Some("shader") => MaterialKind::Shader,
        Some(_) => {
            return Err(bad(
                &join(path, "kind"),
                "must be standard, physical, toon, basic or shader",
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
        MaterialKind::Shader => &[
            "kind",
            "color",
            "opacity",
            "side",
            "emissive",
            "emissiveIntensity",
            "wgsl",
            "uniforms",
            "anchor",
            "blend",
        ],
        MaterialKind::Physical | MaterialKind::Flat(_) => &[
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
        ],
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
    let shader = if matches!(kind, MaterialKind::Shader) {
        let wgsl = match opt_str(m, "wgsl", path)? {
            Some(code) if !code.trim().is_empty() => code,
            _ => return Err(bad(
                &join(path, "wgsl"),
                "a shader material needs its WGSL, declaring `fn shade(px: Pixel) -> vec4<f32>`",
            )),
        };
        orbital_canvas_base::shaders::check_surface(wgsl)
            .map_err(|e| bad(&join(path, "wgsl"), e))?;
        let uniforms = opt_object(m, "uniforms", path)?;
        let uniforms_path = join(path, "uniforms");
        if let Some(u) = uniforms {
            check_keys(u, &["a", "b"], &uniforms_path)?;
        }
        let vector = |key: &str| -> Result<[f32; 4], CanvasSceneError> {
            let Some(value) = uniforms.and_then(|u| present(u, key)) else {
                return Ok([0.0; 4]);
            };
            let field = join(&uniforms_path, key);
            let numbers: Option<Vec<f32>> = value
                .as_array()
                .map(|list| list.iter().map(|n| n.as_f64().map(|n| n as f32)).collect())
                .unwrap_or(None);
            match numbers.as_deref() {
                Some(&[x, y, z, w]) => Ok([x, y, z, w]),
                _ => Err(bad(&field, "must be four numbers")),
            }
        };
        let blend = match opt_str(m, "blend", path)? {
            None | Some("opaque") => ShaderBlend::Opaque,
            Some("alpha") => ShaderBlend::Alpha,
            Some("additive") => ShaderBlend::Additive,
            Some(_) => {
                return Err(bad(
                    &join(path, "blend"),
                    "must be opaque, alpha or additive",
                ))
            }
        };
        Some(SurfaceShader {
            wgsl: wgsl.to_owned(),
            a: vector("a")?,
            b: vector("b")?,
            since: anchor_since(m, path, epoch_ms)?,
            blend,
        })
    } else {
        None
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
        shader,
    })
}

/// Every keyframe colour parses as the static colours do, so a bad one is refused whatever the clock reads.
pub(super) fn animated_colors(declared: &Animation, path: &str) -> Result<(), CanvasSceneError> {
    for track in ["color", "emissive"] {
        for (i, value) in declared.values(track) {
            rgb(value, &format!("{path}.keyframes[{i}].{track}"))?;
        }
    }
    Ok(())
}

pub(super) fn animation_error(e: AnimationError) -> CanvasSceneError {
    bad(&e.field, e.why)
}

pub(super) fn kind_name(kind: MaterialKind) -> &'static str {
    match kind {
        MaterialKind::Basic => "basic",
        MaterialKind::Standard => "standard",
        MaterialKind::Physical => "physical",
        MaterialKind::Toon => "toon",
        MaterialKind::Flat(_) => "flat",
        MaterialKind::Shader => "shader",
    }
}
