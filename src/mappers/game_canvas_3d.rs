//! Perspective projection of authored scene-frame mesh triangles into the
//! shared depth-tested packet. The canvas host still needs a full 3D scene
//! mapper before `draw-mesh` can be registered.

use crate::packet::{DrawCommand, MeshVertex};

#[derive(Debug, Clone, Copy)]
pub struct Camera3D {
    pub eye: [f32; 3],
    pub target: [f32; 3],
    pub up: [f32; 3],
    pub fov_y_degrees: f32,
    pub near: f32,
    pub far: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshProjectionError {
    InvalidCamera,
    InvalidVertex,
    InvalidFace,
    InvalidGeometry,
    ClippedTriangle,
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn normalize(v: [f32; 3]) -> Option<[f32; 3]> {
    let length = dot(v, v).sqrt();
    (length.is_finite() && length > f32::EPSILON)
        .then(|| [v[0] / length, v[1] / length, v[2] / length])
}

impl Camera3D {
    pub fn project(
        self,
        scene_position: [f32; 3],
        width: f32,
        height: f32,
        color: [f32; 4],
    ) -> Result<MeshVertex, MeshProjectionError> {
        if !width.is_finite()
            || !height.is_finite()
            || width <= 0.0
            || height <= 0.0
            || !self.fov_y_degrees.is_finite()
            || !(0.0..180.0).contains(&self.fov_y_degrees)
            || !self.near.is_finite()
            || !self.far.is_finite()
            || self.near <= 0.0
            || self.far <= self.near
        {
            return Err(MeshProjectionError::InvalidCamera);
        }
        if scene_position.iter().any(|v| !v.is_finite()) || color.iter().any(|v| !v.is_finite()) {
            return Err(MeshProjectionError::InvalidVertex);
        }
        let forward =
            normalize(sub(self.target, self.eye)).ok_or(MeshProjectionError::InvalidCamera)?;
        let right = normalize(cross(forward, self.up)).ok_or(MeshProjectionError::InvalidCamera)?;
        let up = cross(right, forward);
        let world = [scene_position[0], scene_position[2], scene_position[1]];
        let relative = sub(world, self.eye);
        let depth = dot(relative, forward);
        if depth < self.near || depth > self.far {
            return Err(MeshProjectionError::ClippedTriangle);
        }
        let half_height = (self.fov_y_degrees.to_radians() * 0.5).tan() * depth;
        let half_width = half_height * width / height;
        let ndc_x = dot(relative, right) / half_width;
        let ndc_y = dot(relative, up) / half_height;
        let vertex = MeshVertex {
            x: (ndc_x + 1.0) * 0.5 * width,
            y: (1.0 - ndc_y) * 0.5 * height,
            z: (depth - self.near) / (self.far - self.near),
            color,
        };
        if !vertex.x.is_finite() || !vertex.y.is_finite() {
            return Err(MeshProjectionError::InvalidVertex);
        }
        Ok(vertex)
    }
}

/// Project one authored polyhedron. Faces are triples into `vertices` in scene
/// axes (z up); a triangle intersecting the near/far plane is rejected until
/// the scene mapper supplies exact clipping.
pub fn project_polyhedron(
    vertices: &[[f32; 3]],
    faces: &[[usize; 3]],
    camera: Camera3D,
    width: f32,
    height: f32,
    color: [f32; 4],
) -> Result<DrawCommand, MeshProjectionError> {
    if faces.is_empty() {
        return Err(MeshProjectionError::InvalidFace);
    }
    let mut projected = Vec::with_capacity(faces.len() * 3);
    for face in faces {
        if face[0] == face[1] || face[1] == face[2] || face[0] == face[2] {
            return Err(MeshProjectionError::InvalidFace);
        }
        let positions = [
            *vertices
                .get(face[0])
                .ok_or(MeshProjectionError::InvalidFace)?,
            *vertices
                .get(face[1])
                .ok_or(MeshProjectionError::InvalidFace)?,
            *vertices
                .get(face[2])
                .ok_or(MeshProjectionError::InvalidFace)?,
        ];
        let normal = cross(
            sub(positions[1], positions[0]),
            sub(positions[2], positions[0]),
        );
        let area = dot(normal, normal);
        if !area.is_finite() || area <= f32::EPSILON {
            return Err(MeshProjectionError::InvalidFace);
        }
        for position in positions {
            projected.push(camera.project(position, width, height, color)?);
        }
    }
    Ok(DrawCommand::Mesh3D {
        vertices: projected,
    })
}

/// Box geometry in authored scene axes (x across, y ground depth, z up).
/// `bottom_pivot` rests the box on z=0; a center pivot spans both sides.
pub fn box_geometry(
    width: f32,
    depth: f32,
    height: f32,
    bottom_pivot: bool,
) -> Result<([[f32; 3]; 8], [[usize; 3]; 12]), MeshProjectionError> {
    if [width, depth, height]
        .iter()
        .any(|v| !v.is_finite() || *v <= 0.0)
    {
        return Err(MeshProjectionError::InvalidGeometry);
    }
    let (x, y) = (width * 0.5, depth * 0.5);
    let (z0, z1) = if bottom_pivot {
        (0.0, height)
    } else {
        (-height * 0.5, height * 0.5)
    };
    let vertices = [
        [-x, -y, z0],
        [x, -y, z0],
        [x, y, z0],
        [-x, y, z0],
        [-x, -y, z1],
        [x, -y, z1],
        [x, y, z1],
        [-x, y, z1],
    ];
    let faces = [
        [0, 2, 1],
        [0, 3, 2], // bottom
        [4, 5, 6],
        [4, 6, 7], // top
        [0, 1, 5],
        [0, 5, 4], // front
        [1, 2, 6],
        [1, 6, 5], // right
        [2, 3, 7],
        [2, 7, 6], // back
        [3, 0, 4],
        [3, 4, 7], // left
    ];
    Ok((vertices, faces))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perspective_projection_preserves_depth_and_scene_up_axis() {
        let camera = Camera3D {
            eye: [0.0, 0.0, 5.0],
            target: [0.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            fov_y_degrees: 60.0,
            near: 0.1,
            far: 20.0,
        };
        let center = camera
            .project([0.0, 0.0, 0.0], 100.0, 100.0, [1.0; 4])
            .unwrap();
        let raised = camera
            .project([0.0, 0.0, 1.0], 100.0, 100.0, [1.0; 4])
            .unwrap();
        let nearer = camera
            .project([0.0, 1.0, 0.0], 100.0, 100.0, [1.0; 4])
            .unwrap();
        assert!((center.x - 50.0).abs() < 0.001);
        assert!((center.y - 50.0).abs() < 0.001);
        assert!(raised.y < center.y);
        assert!(nearer.z < center.z);
    }

    #[test]
    fn malformed_polyhedron_is_rejected_before_packet_paint() {
        let camera = Camera3D {
            eye: [0.0, 0.0, 5.0],
            target: [0.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            fov_y_degrees: 60.0,
            near: 0.1,
            far: 20.0,
        };
        assert_eq!(
            project_polyhedron(
                &[[0.0, 0.0, 0.0]],
                &[[0, 1, 2]],
                camera,
                100.0,
                100.0,
                [1.0; 4]
            )
            .unwrap_err(),
            MeshProjectionError::InvalidFace
        );
        assert_eq!(
            project_polyhedron(&[[0.0, 0.0, 0.0]], &[], camera, 100.0, 100.0, [1.0; 4])
                .unwrap_err(),
            MeshProjectionError::InvalidFace
        );
    }

    #[test]
    fn polyhedron_faces_project_to_depth_tested_triangles() {
        let camera = Camera3D {
            eye: [0.0, 0.0, 5.0],
            target: [0.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            fov_y_degrees: 60.0,
            near: 0.1,
            far: 20.0,
        };
        let command = project_polyhedron(
            &[[-1.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            &[[0, 1, 2]],
            camera,
            100.0,
            100.0,
            [0.1, 0.2, 0.3, 1.0],
        )
        .unwrap();
        let DrawCommand::Mesh3D { vertices } = command else {
            panic!("polyhedron must produce a mesh command")
        };
        assert_eq!(vertices.len(), 3);
        assert_eq!(vertices[0].color, [0.1, 0.2, 0.3, 1.0]);
        assert!(vertices[2].y < vertices[0].y);
    }

    #[test]
    fn box_geometry_respects_ground_pivot_and_all_six_faces() {
        let (vertices, faces) = box_geometry(2.0, 4.0, 6.0, true).unwrap();
        assert_eq!(vertices[0], [-1.0, -2.0, 0.0]);
        assert_eq!(vertices[6], [1.0, 2.0, 6.0]);
        assert_eq!(faces.len(), 12);
        assert_eq!(
            box_geometry(0.0, 4.0, 6.0, true).unwrap_err(),
            MeshProjectionError::InvalidGeometry
        );
    }
}
