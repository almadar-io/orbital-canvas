//! The 3D scene as data: the model a canvas maps to, its meshes, skins and animation, the camera and its input.

pub mod animation;
pub mod camera;
pub mod geometry;
pub mod model;
pub mod skin;
pub mod tessellate;
pub mod view;

pub use camera::view_projection;
pub use model::*;
pub use view::{Button, Pick, Scene3DView};
