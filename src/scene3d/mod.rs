//! Real 3D: the retained scene as data ([`model`], [`geometry`], the camera and its input) from
//! `orbital-canvas-base`, glTF models and labels from `orbital-canvas-paint`, and the GPU renderer ([`render`]) from
//! `orbital-canvas-render`.

pub use orbital_canvas_base::scene3d::*;
pub use orbital_canvas_paint::scene3d::{gltf, label};
pub use orbital_canvas_render::scene3d::{
    environment, glow, mipmap, post, render, transmission, Render3DError, Renderer3D,
};
