//! Real 3D: a retained scene ([`model`]), mesh builders ([`geometry`]) and a GPU renderer ([`render`]) whose transforms,
//! lighting and shadows run in shaders.

pub mod animation;
pub mod environment;
pub mod geometry;
pub mod gltf;
pub mod label;
pub mod mipmap;
pub mod model;
pub mod post;
pub mod render;
pub mod skin;
pub mod tessellate;
pub mod transmission;
pub mod view;

pub use model::*;
pub use render::{view_projection, Render3DError, Renderer3D};
pub use view::{Button, Pick, Scene3DView};

#[cfg(test)]
mod tests;
