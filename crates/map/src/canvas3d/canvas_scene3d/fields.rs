//! Reading a drawable's fields: numbers, vectors, colours and positions, and the errors that name a bad field.

use super::*;

/// three's `Euler` order `XYZ`: the matrix is Rx · Ry · Rz.
pub(super) fn euler_xyz([x, y, z]: [f64; 3]) -> DMat4 {
    DMat4::from_rotation_x(x) * DMat4::from_rotation_y(y) * DMat4::from_rotation_z(z)
}

/// `clampSegments`: default 24, JS `Math.round`, clamped to 3..64.
pub(super) fn clamp_segments(segments: Option<f64>) -> usize {
    js_round(segments.unwrap_or(24.0)).clamp(3.0, 64.0) as usize
}

pub(super) fn js_round(value: f64) -> f64 {
    (value + 0.5).floor()
}

pub(super) fn drawable_nodes(value: &Value, path: &str) -> Result<Vec<Value>, CanvasSceneError> {
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

pub(super) fn unsupported(message: String) -> CanvasSceneError {
    CanvasSceneError::Unsupported3DContract(message)
}

pub(super) fn undrawn(field: &str, step: u8) -> CanvasSceneError {
    unsupported(format!(
        "{field}: not yet drawn by orbital-canvas (P-025 step {step})"
    ))
}

pub(super) fn bad(field: &str, why: impl std::fmt::Display) -> CanvasSceneError {
    unsupported(format!("{field}: {why}"))
}

pub(super) fn join(path: &str, key: &str) -> String {
    format!("{path}.{key}")
}

pub(super) fn present<'a>(f: &'a Fields, key: &str) -> Option<&'a Value> {
    f.get(key).filter(|v| !v.is_null())
}

pub(super) fn check_keys(f: &Fields, allowed: &[&str], path: &str) -> Result<(), CanvasSceneError> {
    match f.keys().find(|key| !allowed.contains(&key.as_str())) {
        Some(key) => Err(bad(&join(path, key), "not a declared prop")),
        None => Ok(()),
    }
}

pub(super) fn number(v: &Value, field: &str) -> Result<f64, CanvasSceneError> {
    v.as_f64()
        .filter(|n| n.is_finite())
        .ok_or_else(|| bad(field, "must be a finite number"))
}

pub(super) fn opt_number(
    f: &Fields,
    key: &str,
    path: &str,
) -> Result<Option<f64>, CanvasSceneError> {
    present(f, key)
        .map(|v| number(v, &join(path, key)))
        .transpose()
}

pub(super) fn opt_bool(
    f: &Fields,
    key: &str,
    path: &str,
) -> Result<Option<bool>, CanvasSceneError> {
    present(f, key)
        .map(|v| {
            v.as_bool()
                .ok_or_else(|| bad(&join(path, key), "must be a boolean"))
        })
        .transpose()
}

pub(super) fn opt_str<'a>(
    f: &'a Fields,
    key: &str,
    path: &str,
) -> Result<Option<&'a str>, CanvasSceneError> {
    present(f, key)
        .map(|v| {
            v.as_str()
                .ok_or_else(|| bad(&join(path, key), "must be a string"))
        })
        .transpose()
}

pub(super) fn opt_object<'a>(
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

pub(super) fn triple(v: &Value, field: &str) -> Result<[f64; 3], CanvasSceneError> {
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

pub(super) fn f32x3(v: [f64; 3]) -> [f32; 3] {
    v.map(|c| c as f32)
}

pub(super) fn scene_pos(v: &Value, field: &str) -> Result<[f64; 3], CanvasSceneError> {
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

pub(super) fn required_pos(f: &Fields, path: &str) -> Result<[f64; 3], CanvasSceneError> {
    let field = join(path, "position");
    scene_pos(
        present(f, "position").ok_or_else(|| bad(&field, "is required"))?,
        &field,
    )
}

/// A CSS color as three's `Color.set` takes it under ColorManagement: sRGB to linear; three drops alpha, so an alpha
/// below one is refused rather than lost.
pub(super) fn rgb(v: &Value, field: &str) -> Result<[f32; 3], CanvasSceneError> {
    let [r, g, b, a] = css(v, field)?;
    if a != 1.0 {
        return Err(bad(
            field,
            "a 3D color has no alpha (the three host drops it)",
        ));
    }
    Ok(srgb_to_linear_rgb([r, g, b]))
}

pub(super) fn opt_rgb(
    f: &Fields,
    key: &str,
    path: &str,
) -> Result<Option<[f32; 3]>, CanvasSceneError> {
    present(f, key)
        .map(|v| rgb(v, &join(path, key)))
        .transpose()
}

pub(super) fn rgba(v: &Value, field: &str) -> Result<[f32; 4], CanvasSceneError> {
    let [r, g, b, a] = css(v, field)?;
    Ok([srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b), a])
}

pub(super) fn css(v: &Value, field: &str) -> Result<[f32; 4], CanvasSceneError> {
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
