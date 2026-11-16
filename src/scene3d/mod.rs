//! Real 3D: the retained scene as data ([`model`], [`geometry`], the camera and its input, from
//! `orbital-canvas-base`) and the GPU renderer ([`render`]) whose transforms, lighting and shadows run in shaders.

pub use orbital_canvas_base::scene3d::*;
pub use orbital_canvas_paint::scene3d::{gltf, label};

pub mod environment;
pub mod glow;
pub mod mipmap;
pub mod post;
pub mod render;
pub mod transmission;

pub use render::{Render3DError, Renderer3D};

#[cfg(test)]
mod tests;
