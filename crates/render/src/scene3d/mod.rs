//! The 3D renderer: the scene's transforms, lighting, shadows, image-based lighting, transmission, glow and
//! post-processing run in shaders on the shared device.

pub mod environment;
pub mod glow;
pub mod mipmap;
pub mod post;
pub mod render;
pub mod transmission;

pub use render::{Render3DError, Renderer3D};

#[cfg(test)]
mod tests;
