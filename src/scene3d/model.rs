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
        }
    }
}

impl Material {
    pub fn is_transparent(&self) -> bool {
        self.opacity < 1.0
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
