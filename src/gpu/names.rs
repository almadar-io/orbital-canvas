//! The closed string vocabularies of the `gpu/*` descriptors. Anything outside a list is `invalid`.

use super::invalid;
use orbital_core_base::os::OsError;
use orbital_core_base::value::Value;

pub(super) const BUFFER_USAGES: &[&str] = &[
    "map-read",
    "map-write",
    "copy-src",
    "copy-dst",
    "index",
    "vertex",
    "uniform",
    "storage",
    "indirect",
    "query-resolve",
];
pub(super) const TEXTURE_USAGES: &[&str] = &[
    "copy-src",
    "copy-dst",
    "texture-binding",
    "storage-binding",
    "render-attachment",
];
pub(super) const FORMATS: &[&str] = &[
    "r8unorm",
    "rg8unorm",
    "rgba8unorm",
    "rgba8unorm-srgb",
    "bgra8unorm",
    "bgra8unorm-srgb",
    "r16float",
    "rg16float",
    "rgba16float",
    "r32float",
    "rg32float",
    "rgba32float",
    "r32uint",
    "r32sint",
    "rgba8uint",
    "rgba8sint",
    "rgba32uint",
    "rgba32sint",
    "depth32float",
];
/// (contract name, features it needs).
pub(super) const FEATURES: &[(&str, fn() -> wgpu::Features)] = &[
    ("timestamp-query", || {
        wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS
    }),
    ("shader-f16", || wgpu::Features::SHADER_F16),
    ("float32-filterable", || wgpu::Features::FLOAT32_FILTERABLE),
];

pub(super) fn buffer_usage(name: &str) -> Option<wgpu::BufferUsages> {
    use wgpu::BufferUsages as U;
    Some(match name {
        "copy-src" => U::COPY_SRC,
        "copy-dst" => U::COPY_DST,
        "storage" => U::STORAGE,
        "uniform" => U::UNIFORM,
        "vertex" => U::VERTEX,
        "index" => U::INDEX,
        "map-read" => U::MAP_READ,
        "map-write" => U::MAP_WRITE,
        "indirect" => U::INDIRECT,
        "query-resolve" => U::QUERY_RESOLVE,
        _ => return None,
    })
}

pub(super) fn texture_usage(name: &str) -> Option<wgpu::TextureUsages> {
    use wgpu::TextureUsages as U;
    Some(match name {
        "copy-src" => U::COPY_SRC,
        "copy-dst" => U::COPY_DST,
        "texture-binding" => U::TEXTURE_BINDING,
        "storage-binding" => U::STORAGE_BINDING,
        "render-attachment" => U::RENDER_ATTACHMENT,
        _ => return None,
    })
}

pub(super) fn format(name: &str) -> Option<wgpu::TextureFormat> {
    use wgpu::TextureFormat as F;
    Some(match name {
        "r8unorm" => F::R8Unorm,
        "rg8unorm" => F::Rg8Unorm,
        "rgba8unorm" => F::Rgba8Unorm,
        "rgba8unorm-srgb" => F::Rgba8UnormSrgb,
        "bgra8unorm" => F::Bgra8Unorm,
        "bgra8unorm-srgb" => F::Bgra8UnormSrgb,
        "r16float" => F::R16Float,
        "rg16float" => F::Rg16Float,
        "rgba16float" => F::Rgba16Float,
        "r32float" => F::R32Float,
        "rg32float" => F::Rg32Float,
        "rgba32float" => F::Rgba32Float,
        "r32uint" => F::R32Uint,
        "r32sint" => F::R32Sint,
        "rgba8uint" => F::Rgba8Uint,
        "rgba8sint" => F::Rgba8Sint,
        "rgba32uint" => F::Rgba32Uint,
        "rgba32sint" => F::Rgba32Sint,
        "depth32float" => F::Depth32Float,
        _ => return None,
    })
}

pub(super) fn parse_format(name: &str) -> Result<wgpu::TextureFormat, OsError> {
    format(name).ok_or_else(|| {
        invalid(format!(
            "unknown texture format `{name}`; one of {}",
            FORMATS.join(", ")
        ))
    })
}

/// The OR of a non-empty list of names.
pub(super) fn flags<F: Copy + std::ops::BitOr<Output = F>>(
    value: &Value,
    map: fn(&str) -> Option<F>,
    allowed: &[&str],
    what: &str,
) -> Result<F, OsError> {
    let items = value
        .as_array()
        .ok_or_else(|| invalid(format!("{what} usage must be a list of names")))?;
    let mut out: Option<F> = None;
    for item in items {
        let name = item
            .as_str()
            .ok_or_else(|| invalid(format!("{what} usage entries must be strings")))?;
        let flag = map(name).ok_or_else(|| {
            invalid(format!(
                "unknown {what} usage `{name}`; one of {}",
                allowed.join(", ")
            ))
        })?;
        out = Some(match out {
            Some(acc) => acc | flag,
            None => flag,
        });
    }
    out.ok_or_else(|| invalid(format!("{what} usage must name at least one usage")))
}
