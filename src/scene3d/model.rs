//! The retained 3D scene the mappers build and the 3D renderer draws: world space is Y-up and right-handed, colors are
//! linear RGB, and every mesh carries its own world transform (groups are flattened by the mapper).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Scene3D {
    pub camera: Camera,
    /// Linear RGBA the frame clears to.
    pub background: [f32; 4],
    pub lights: Lights,
    /// Whether lights that cast shadows render a shadow map.
    pub shadows: bool,
    pub tone_mapping: ToneMapping,
    pub meshes: Vec<MeshInstance>,
    /// A ground grid drawn as drei's `Grid` draws it.
    pub grid: Option<Grid>,
    #[serde(default)]
    pub interaction: Interaction,
    /// Whether any drawable declares a keyframe animation, so the host keeps redrawing on its canvas clock.
    #[serde(default)]
    pub animated: bool,
    /// Image-based lighting for standard and physical materials (`lighting.environment`).
    #[serde(default)]
    pub environment: Option<EnvironmentKind>,
    /// The post-processing stack (`post`): bloom and vignette as @react-three/postprocessing draws them.
    #[serde(default)]
    pub post: Option<PostSettings>,
    /// Seconds on the canvas clock; a shader material reads it as `px.time`.
    #[serde(default)]
    pub time: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum EnvironmentKind {
    /// three's `RoomEnvironment` through `PMREMGenerator`.
    Room,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct PostSettings {
    pub bloom: Option<BloomSettings>,
    pub vignette: Option<VignetteSettings>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct BloomSettings {
    pub intensity: f32,
    pub threshold: f32,
    pub smoothing: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct VignetteSettings {
    pub offset: f32,
    pub darkness: f32,
}

/// What a host needs to turn pointer input into the scene's own events: whether the user may orbit the camera,
/// and how world points map back to scene cells (`scene = world / cell + min`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Interaction {
    pub orbit: bool,
    pub cell: f32,
    pub min_x: f32,
    pub min_z: f32,
    /// The scene cell of each top-level drawable that carries an id.
    pub cells: Vec<CellId>,
    /// The far edge of the drawn bounds, in scene units (with `min_x`/`min_z`, the board the ground spans).
    #[serde(default)]
    pub max_x: f32,
    #[serde(default)]
    pub max_z: f32,
    /// A following camera's starting pose, which it eases from toward its mapped pose (three's first frame).
    #[serde(default)]
    pub follow_from: Option<[f32; 3]>,
    /// Every top-level drawn position, in scene units (overlays: coordinates, minimap).
    #[serde(default)]
    pub drawn: Vec<[f64; 2]>,
    /// Edit mode: drawables are selected and dragged instead of the camera orbiting.
    #[serde(default)]
    pub editable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CellId {
    pub x: i64,
    pub y: i64,
    pub id: String,
}

/// drei's `Grid`: cell and section lines on the y = `center[1]` plane, faded with distance from the camera's
/// projection onto that plane, visible from above only.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Grid {
    pub center: [f32; 3],
    /// Extent along X and Z.
    pub size: [f32; 2],
    pub cell_size: f32,
    pub cell_thickness: f32,
    /// Linear RGB.
    pub cell_color: [f32; 3],
    pub section_size: f32,
    pub section_thickness: f32,
    pub section_color: [f32; 3],
    pub fade_distance: f32,
    pub fade_strength: f32,
    pub fade_from: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ToneMapping {
    None,
    Aces,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Projection {
    Perspective {
        fov_y_degrees: f32,
    },
    /// `height` is the world-space height the view spans.
    Orthographic {
        height: f32,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Camera {
    pub eye: [f32; 3],
    pub target: [f32; 3],
    pub up: [f32; 3],
    pub projection: Projection,
    pub near: f32,
    pub far: f32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Lights {
    pub ambient: Option<AmbientLight>,
    pub hemisphere: Option<HemisphereLight>,
    pub directional: Option<DirectionalLight>,
    pub points: Vec<PointLight>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct AmbientLight {
    pub color: [f32; 3],
    pub intensity: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct HemisphereLight {
    pub sky: [f32; 3],
    pub ground: [f32; 3],
    pub intensity: f32,
}

/// A light from `position` toward `target`; a shadow caster when `shadow` is set.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct DirectionalLight {
    pub position: [f32; 3],
    pub target: [f32; 3],
    pub color: [f32; 3],
    pub intensity: f32,
    pub shadow: Option<ShadowSettings>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct ShadowSettings {
    pub map_size: u32,
    pub bias: f32,
    pub normal_bias: f32,
    /// Half the extent of the light's orthographic shadow camera.
    pub extent: f32,
    pub near: f32,
    pub far: f32,
}

/// `distance` 0 means unbounded; `decay` follows the physically based inverse-square falloff at 2.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct PointLight {
    pub position: [f32; 3],
    pub color: [f32; 3],
    pub intensity: f32,
    pub distance: f32,
    pub decay: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MeshInstance {
    pub mesh: MeshData,
    /// Column-major world transform.
    pub transform: [f32; 16],
    pub material: Material,
    pub cast_shadow: bool,
    pub receive_shadow: bool,
    /// The id of the innermost drawable this mesh belongs to, for picking.
    #[serde(default)]
    pub id: Option<String>,
    /// Draw only the mesh's coverage, Gaussian-blurred by this sigma in its local units within its local XY plane, in
    /// the material's color: canvas `shadowBlur` (twice the sigma) under a flat shape.
    #[serde(default)]
    pub glow: Option<f32>,
}

/// Indexed triangles in the mesh's local space, counter-clockwise front faces.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MeshData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum MaterialKind {
    Basic,
    Standard,
    Physical,
    Toon,
    Flat(Flat),
    /// The surface is coloured by the program's own WGSL ([`Material::shader`]).
    Shader,
}

/// How a shader material's colour meets what is behind it.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum ShaderBlend {
    #[default]
    Opaque,
    Alpha,
    Additive,
}

/// A surface coloured by a program's own WGSL (`material.kind: "shader"`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SurfaceShader {
    /// WGSL that declares `fn shade(px: Pixel) -> vec4<f32>`: the pixel's linear colour and alpha.
    pub wgsl: String,
    /// `material.uniforms.a` and `.b`, read as `px.a` and `px.b`.
    pub a: [f32; 4],
    pub b: [f32; 4],
    pub blend: ShaderBlend,
}

/// An unlit, always-blended surface as three draws its 2D drawables in a 3D scene (`MeshBasicMaterial` with
/// `transparent`): sprites, labels and ground shapes. It lives on the material kind because these switches change how
/// the renderer samples, blends and places the surface, and every other kind leaves them off.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Flat {
    /// Sample the map with nearest filtering (three's `NearestFilter` on sprite textures), else linear.
    pub nearest: bool,
    /// Discard fragments whose alpha (map × opacity) is below 0.1, three's sprite `alphaTest`.
    pub cutout: bool,
    /// Add to what is behind (three's `AdditiveBlending`, the canvas `lighter` blend mode) instead of blending over it.
    pub additive: bool,
    /// Turn to face the camera as drei's `Billboard` does: the mesh keeps its world origin and scale, and its local X/Y
    /// axes follow the camera's right and up vectors.
    pub billboard: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum Side {
    Front,
    Back,
    Double,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Material {
    pub kind: MaterialKind,
    /// Linear RGB.
    pub color: [f32; 3],
    pub opacity: f32,
    pub metalness: f32,
    pub roughness: f32,
    /// Linear RGB, scaled by `emissive_intensity`.
    pub emissive: [f32; 3],
    pub emissive_intensity: f32,
    pub transmission: f32,
    pub ior: f32,
    pub flat_shading: bool,
    pub side: Side,
    /// An image the renderer resolves like a sprite texture; sampled with repeat wrapping.
    pub map: Option<String>,
    /// The WGSL of a [`MaterialKind::Shader`] surface.
    #[serde(default)]
    pub shader: Option<SurfaceShader>,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            kind: MaterialKind::Standard,
            color: [1.0, 1.0, 1.0],
            opacity: 1.0,
            metalness: 0.0,
            roughness: 1.0,
            emissive: [0.0, 0.0, 0.0],
            emissive_intensity: 1.0,
            transmission: 0.0,
            ior: 1.5,
            flat_shading: false,
            side: Side::Front,
            map: None,
            shader: None,
        }
    }
}

impl Material {
    pub fn is_transparent(&self) -> bool {
        self.opacity < 1.0
            || matches!(self.kind, MaterialKind::Flat(_))
            || self
                .shader
                .as_ref()
                .is_some_and(|s| s.blend != ShaderBlend::Opaque)
    }
}

/// sRGB-encoded channel (0..1) to linear.
pub fn srgb_to_linear(channel: f32) -> f32 {
    if channel <= 0.04045 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

pub fn srgb_to_linear_rgb(rgb: [f32; 3]) -> [f32; 3] {
    rgb.map(srgb_to_linear)
}
