//! Real 3D: a retained scene ([`model`]), mesh builders ([`geometry`]) and a GPU renderer ([`render`]) whose transforms,
//! lighting and shadows run in shaders.

pub mod geometry;
pub mod model;
pub mod render;

pub use model::*;
pub use render::{view_projection, Render3DError, Renderer3D};

#[cfg(test)]
mod tests;
