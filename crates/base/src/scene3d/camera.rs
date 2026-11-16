//! The camera's matrices.

use super::model::{Projection, Scene3D};
use glam::{Mat4, Vec3};

/// The camera's view-projection for an `aspect` (width over height).
pub fn view_projection(scene: &Scene3D, aspect: f32) -> Mat4 {
    let c = &scene.camera;
    let view = Mat4::look_at_rh(Vec3::from(c.eye), Vec3::from(c.target), Vec3::from(c.up));
    let projection = match c.projection {
        Projection::Perspective { fov_y_degrees } => {
            Mat4::perspective_rh(fov_y_degrees.to_radians(), aspect, c.near, c.far)
        }
        Projection::Orthographic { height } => {
            let (h, w) = (height * 0.5, height * 0.5 * aspect);
            Mat4::orthographic_rh(-w, w, -h, h, c.near, c.far)
        }
    };
    projection * view
}
