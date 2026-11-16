//! WGSL a canvas assembles from parts: the SDF material prelude every 2D material is appended to, and the lit 3D
//! pass (with its image-based lighting and transmission) every 3D surface shader is appended to. The mappers check
//! a program's WGSL against these modules before a scene is accepted; the GPU passes compile the same text.

/// The SDF pass's prelude: the `Pixel` a material's `shade` reads.
pub const SDF_PRELUDE: &str = include_str!("sdf.wgsl");
/// The 3D prelude every 3D pass starts with.
pub const PRELUDE_3D: &str = include_str!("prelude.wgsl");
/// The cube-map helpers the environment and its image-based lighting share.
pub const CUBE_UV: &str = include_str!("cube_uv.wgsl");
/// Three's image-based lighting, appended to the lit pass.
pub const IBL: &str = concat!(
    include_str!("cube_uv.wgsl"),
    include_str!("environment_ibl.wgsl")
);
/// Three's transmission, appended after the image-based lighting.
pub const TRANSMISSION: &str = include_str!("transmission.wgsl");

const LIT: &str = include_str!("lit.wgsl");
const SURFACE: &str = include_str!("surface.wgsl");
const PLAIN_SHADE: &str = "\nfn shade(px: Pixel) -> vec4<f32> { return px.color; }\n";

/// A 2D material's whole module: the prelude and the program's WGSL that declares `shade`.
pub fn material_source(wgsl: &str) -> String {
    [SDF_PRELUDE, wgsl].concat()
}

/// The module of an SDF shape without a material.
pub fn plain_material_source() -> String {
    [SDF_PRELUDE, PLAIN_SHADE].concat()
}

/// The lit shader's source: the shared prelude, the lit pass, three's IBL and its transmission.
pub fn lit_source() -> String {
    [PRELUDE_3D, LIT, IBL, TRANSMISSION].concat()
}

/// A 3D shader material's whole module: the lit source (its vertex stage and bindings), the fragment stage that
/// calls `shade`, and the program's WGSL that declares it.
pub fn surface_source(wgsl: &str) -> String {
    [lit_source().as_str(), SURFACE, wgsl].concat()
}

/// Whether a 2D material's WGSL makes a module every host runs.
pub fn check_material(wgsl: &str) -> Result<(), String> {
    crate::shader_check::check_module("material", wgsl, || material_source(wgsl))
}

/// Whether a 3D surface shader's WGSL makes a module every host runs: the mapper refuses a scene whose shader does
/// not, so the device never sees it.
pub fn check_surface(wgsl: &str) -> Result<(), String> {
    crate::shader_check::check_module("surface", wgsl, || surface_source(wgsl))
}
