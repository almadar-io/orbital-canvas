//! Perspective projection of authored scene-frame mesh triangles into the
//! shared depth-tested packet. The complete `draw-mesh` contract remains
//! unregistered while material, animation, and event support is incomplete.

use crate::packet::{DrawCommand, MeshVertex, TexturedMeshVertex};
use crate::{mappers::game_canvas_2d::CanvasSceneError, packet::GameRenderPacket};
use serde_json::Value;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshSide {
    Front,
    Back,
    Double,
}

#[derive(Debug, Clone)]
pub struct MeshNormals {
    pub faces: Vec<[f32; 3]>,
    pub vertices: Vec<[f32; 3]>,
}

#[derive(Clone, Copy)]
struct CameraBasis {
    forward: [f32; 3],
    right: [f32; 3],
    up: [f32; 3],
}

#[derive(Clone, Copy)]
struct ViewPoint {
    x: f32,
    y: f32,
    depth: f32,
    uv: [f32; 2],
}

impl ViewPoint {
    fn interpolate(self, other: Self, t: f32, depth: f32) -> Self {
        Self {
            x: self.x + (other.x - self.x) * t,
            y: self.y + (other.y - self.y) * t,
            depth,
            uv: [
                self.uv[0] + (other.uv[0] - self.uv[0]) * t,
                self.uv[1] + (other.uv[1] - self.uv[1]) * t,
            ],
        }
    }
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

fn face_area_normal(positions: [[f32; 3]; 3]) -> Result<[f32; 3], MeshProjectionError> {
    let normal = cross(
        sub(positions[1], positions[0]),
        sub(positions[2], positions[0]),
    );
    let area = dot(normal, normal);
    if !area.is_finite() || area == 0.0 {
        return Err(MeshProjectionError::InvalidFace);
    }
    Ok(normal)
}

/// Derive outward face normals and area-weighted smooth vertex normals in
/// authored scene axes. Flat materials use `faces`; smooth materials use
/// `vertices`, matching indexed polyhedron normal accumulation in the web host.
pub fn mesh_normals(
    vertices: &[[f32; 3]],
    faces: &[[usize; 3]],
) -> Result<MeshNormals, MeshProjectionError> {
    if faces.is_empty() || vertices.iter().flatten().any(|value| !value.is_finite()) {
        return Err(MeshProjectionError::InvalidFace);
    }
    let mut result = MeshNormals {
        faces: Vec::with_capacity(faces.len()),
        vertices: vec![[0.0; 3]; vertices.len()],
    };
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
        let area_normal = face_area_normal(positions)?;
        result
            .faces
            .push(normalize(area_normal).ok_or(MeshProjectionError::InvalidFace)?);
        for index in face {
            for (axis, component) in area_normal.iter().enumerate() {
                result.vertices[*index][axis] += component;
            }
        }
    }
    for normal in &mut result.vertices {
        *normal = normalize(*normal).unwrap_or([0.0; 3]);
    }
    Ok(result)
}

impl Camera3D {
    fn basis(self, width: f32, height: f32) -> Result<CameraBasis, MeshProjectionError> {
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
        let forward =
            normalize(sub(self.target, self.eye)).ok_or(MeshProjectionError::InvalidCamera)?;
        let right = normalize(cross(forward, self.up)).ok_or(MeshProjectionError::InvalidCamera)?;
        let up = cross(right, forward);
        Ok(CameraBasis { forward, right, up })
    }

    fn view(
        self,
        scene_position: [f32; 3],
        basis: CameraBasis,
    ) -> Result<ViewPoint, MeshProjectionError> {
        if scene_position.iter().any(|v| !v.is_finite()) {
            return Err(MeshProjectionError::InvalidVertex);
        }
        let world = [scene_position[0], scene_position[2], scene_position[1]];
        let relative = sub(world, self.eye);
        Ok(ViewPoint {
            x: dot(relative, basis.right),
            y: dot(relative, basis.up),
            depth: dot(relative, basis.forward),
            uv: [0.0; 2],
        })
    }

    fn project_view(
        self,
        point: ViewPoint,
        width: f32,
        height: f32,
        color: [f32; 4],
    ) -> Result<MeshVertex, MeshProjectionError> {
        if color.iter().any(|v| !v.is_finite()) {
            return Err(MeshProjectionError::InvalidVertex);
        }
        if point.depth < self.near || point.depth > self.far {
            return Err(MeshProjectionError::ClippedTriangle);
        }
        let half_height = (self.fov_y_degrees.to_radians() * 0.5).tan() * point.depth;
        let half_width = half_height * width / height;
        let ndc_x = point.x / half_width;
        let ndc_y = point.y / half_height;
        let vertex = MeshVertex {
            x: (ndc_x + 1.0) * 0.5 * width,
            y: (1.0 - ndc_y) * 0.5 * height,
            z: (point.depth - self.near) / (self.far - self.near),
            color,
        };
        if !vertex.x.is_finite() || !vertex.y.is_finite() {
            return Err(MeshProjectionError::InvalidVertex);
        }
        Ok(vertex)
    }

    pub fn project(
        self,
        scene_position: [f32; 3],
        width: f32,
        height: f32,
        color: [f32; 4],
    ) -> Result<MeshVertex, MeshProjectionError> {
        let basis = self.basis(width, height)?;
        let point = self.view(scene_position, basis)?;
        self.project_view(point, width, height, color)
    }
}

fn clip_depth(points: &[ViewPoint], plane: f32, keep_nearer: bool) -> Vec<ViewPoint> {
    let mut clipped = Vec::new();
    let Some(mut previous) = points.last().copied() else {
        return clipped;
    };
    let mut previous_inside = if keep_nearer {
        previous.depth >= plane
    } else {
        previous.depth <= plane
    };
    for &current in points {
        let current_inside = if keep_nearer {
            current.depth >= plane
        } else {
            current.depth <= plane
        };
        if current_inside != previous_inside {
            let t = (plane - previous.depth) / (current.depth - previous.depth);
            clipped.push(previous.interpolate(current, t, plane));
        }
        if current_inside {
            clipped.push(current);
        }
        previous = current;
        previous_inside = current_inside;
    }
    clipped
}

/// Project one authored polyhedron. Faces are triples into `vertices` in scene
/// axes (z up); triangles crossing near/far planes are clipped before painting.
pub fn project_polyhedron(
    vertices: &[[f32; 3]],
    faces: &[[usize; 3]],
    camera: Camera3D,
    width: f32,
    height: f32,
    color: [f32; 4],
) -> Result<DrawCommand, MeshProjectionError> {
    project_polyhedron_with_side(
        vertices,
        faces,
        camera,
        width,
        height,
        color,
        MeshSide::Double,
    )
}

pub fn project_polyhedron_with_side(
    vertices: &[[f32; 3]],
    faces: &[[usize; 3]],
    camera: Camera3D,
    width: f32,
    height: f32,
    color: [f32; 4],
    side: MeshSide,
) -> Result<DrawCommand, MeshProjectionError> {
    project_polyhedron_material(vertices, faces, camera, width, height, color, side, None)
}

pub fn project_polyhedron_textured(
    vertices: &[[f32; 3]],
    faces: &[[usize; 3]],
    uvs: &[[f32; 2]],
    texture: &str,
    camera: Camera3D,
    width: f32,
    height: f32,
    color: [f32; 4],
    side: MeshSide,
) -> Result<DrawCommand, MeshProjectionError> {
    if uvs.len() != vertices.len() || uvs.iter().flatten().any(|value| !value.is_finite()) {
        return Err(MeshProjectionError::InvalidVertex);
    }
    let face_uvs = faces
        .iter()
        .map(|face| {
            Ok([
                *uvs.get(face[0]).ok_or(MeshProjectionError::InvalidFace)?,
                *uvs.get(face[1]).ok_or(MeshProjectionError::InvalidFace)?,
                *uvs.get(face[2]).ok_or(MeshProjectionError::InvalidFace)?,
            ])
        })
        .collect::<Result<Vec<_>, MeshProjectionError>>()?;
    project_polyhedron_textured_faces(
        vertices, faces, &face_uvs, texture, camera, width, height, color, side,
    )
}

fn project_polyhedron_textured_faces(
    vertices: &[[f32; 3]],
    faces: &[[usize; 3]],
    face_uvs: &[[[f32; 2]; 3]],
    texture: &str,
    camera: Camera3D,
    width: f32,
    height: f32,
    color: [f32; 4],
    side: MeshSide,
) -> Result<DrawCommand, MeshProjectionError> {
    if face_uvs.len() != faces.len()
        || face_uvs
            .iter()
            .flatten()
            .flatten()
            .any(|value| !value.is_finite())
    {
        return Err(MeshProjectionError::InvalidVertex);
    }
    project_polyhedron_material(
        vertices,
        faces,
        camera,
        width,
        height,
        color,
        side,
        Some((texture, face_uvs)),
    )
}

fn project_polyhedron_material(
    vertices: &[[f32; 3]],
    faces: &[[usize; 3]],
    camera: Camera3D,
    width: f32,
    height: f32,
    color: [f32; 4],
    side: MeshSide,
    texture: Option<(&str, &[[[f32; 2]; 3]])>,
) -> Result<DrawCommand, MeshProjectionError> {
    if faces.is_empty() {
        return Err(MeshProjectionError::InvalidFace);
    }
    let basis = camera.basis(width, height)?;
    let mut projected = Vec::with_capacity(faces.len() * 3);
    let mut textured = Vec::with_capacity(faces.len() * 3);
    let mut culled = false;
    let eye = [camera.eye[0], camera.eye[2], camera.eye[1]];
    for (face_index, face) in faces.iter().enumerate() {
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
        let normal = face_area_normal(positions)?;
        let facing = dot(normal, sub(eye, positions[0]));
        if !facing.is_finite() {
            return Err(MeshProjectionError::InvalidFace);
        }
        if (side == MeshSide::Front && facing <= 0.0) || (side == MeshSide::Back && facing >= 0.0) {
            culled = true;
            continue;
        }
        let points = face
            .iter()
            .enumerate()
            .map(|(index, _vertex_index)| {
                let mut point = camera.view(positions[index], basis)?;
                if let Some((_, uvs)) = texture {
                    point.uv = uvs[face_index][index];
                }
                Ok(point)
            })
            .collect::<Result<Vec<_>, MeshProjectionError>>()?;
        let near_clipped = clip_depth(&points, camera.near, true);
        let clipped = clip_depth(&near_clipped, camera.far, false);
        if clipped.len() < 3 {
            continue;
        }
        let first = clipped[0];
        for pair in clipped[1..].windows(2) {
            for point in [first, pair[0], pair[1]] {
                let vertex = camera.project_view(point, width, height, color)?;
                if texture.is_some() {
                    textured.push(TexturedMeshVertex {
                        x: vertex.x,
                        y: vertex.y,
                        z: vertex.z,
                        uv: point.uv,
                        inverse_depth: 1.0 / point.depth,
                        color,
                    });
                } else {
                    projected.push(vertex);
                }
            }
        }
    }
    if projected.is_empty() && textured.is_empty() && !culled {
        return Err(MeshProjectionError::ClippedTriangle);
    }
    if let Some((texture, _)) = texture {
        Ok(DrawCommand::TexturedMesh3D {
            texture: texture.to_owned(),
            vertices: textured,
        })
    } else {
        Ok(DrawCommand::Mesh3D {
            vertices: projected,
        })
    }
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

fn radial_geometry(
    profile: &[(f32, f32)],
    segments: usize,
) -> Result<(Vec<[f32; 3]>, Vec<[usize; 3]>), MeshProjectionError> {
    if profile.len() < 2
        || profile
            .iter()
            .any(|(radius, height)| !radius.is_finite() || *radius < 0.0 || !height.is_finite())
    {
        return Err(MeshProjectionError::InvalidGeometry);
    }
    let segments = segments.clamp(3, 64);
    let mut vertices = Vec::with_capacity(profile.len() * segments);
    for &(radius, height) in profile {
        for segment in 0..segments {
            let angle = segment as f32 * std::f32::consts::TAU / segments as f32;
            vertices.push([radius * angle.cos(), radius * angle.sin(), height]);
        }
    }
    let mut faces = Vec::new();
    for ring in 0..profile.len() - 1 {
        for segment in 0..segments {
            let a = ring * segments + segment;
            let b = ring * segments + (segment + 1) % segments;
            let c = (ring + 1) * segments + segment;
            let d = (ring + 1) * segments + (segment + 1) % segments;
            for face in [[a, b, c], [b, d, c]] {
                let normal = cross(
                    sub(vertices[face[1]], vertices[face[0]]),
                    sub(vertices[face[2]], vertices[face[0]]),
                );
                let area = dot(normal, normal);
                if area.is_finite() && area > 0.0 {
                    faces.push(face);
                }
            }
        }
    }
    if faces.is_empty() {
        return Err(MeshProjectionError::InvalidGeometry);
    }
    Ok((vertices, faces))
}

pub fn sphere_geometry(
    radius: f32,
    segments: usize,
) -> Result<(Vec<[f32; 3]>, Vec<[usize; 3]>), MeshProjectionError> {
    if !radius.is_finite() || radius <= 0.0 {
        return Err(MeshProjectionError::InvalidGeometry);
    }
    let segments = segments.clamp(3, 64);
    let profile = (0..=segments)
        .map(|band| {
            let angle =
                -std::f32::consts::FRAC_PI_2 + band as f32 * std::f32::consts::PI / segments as f32;
            (radius * angle.cos().abs(), radius * angle.sin())
        })
        .collect::<Vec<_>>();
    radial_geometry(&profile, segments)
}

pub fn cylinder_geometry(
    radius_top: f32,
    radius_bottom: f32,
    height: f32,
    segments: usize,
) -> Result<(Vec<[f32; 3]>, Vec<[usize; 3]>), MeshProjectionError> {
    if [radius_top, radius_bottom, height]
        .iter()
        .any(|value| !value.is_finite() || *value < 0.0)
        || height <= 0.0
        || radius_top + radius_bottom <= 0.0
    {
        return Err(MeshProjectionError::InvalidGeometry);
    }
    radial_geometry(
        &[
            (0.0, -height * 0.5),
            (radius_bottom, -height * 0.5),
            (radius_top, height * 0.5),
            (0.0, height * 0.5),
        ],
        segments,
    )
}

pub fn capsule_geometry(
    radius: f32,
    height: f32,
    segments: usize,
) -> Result<(Vec<[f32; 3]>, Vec<[usize; 3]>), MeshProjectionError> {
    if !radius.is_finite() || radius <= 0.0 || !height.is_finite() || height <= 0.0 {
        return Err(MeshProjectionError::InvalidGeometry);
    }
    let bands = (segments.clamp(3, 64) as f32 * 0.5).round().max(2.0) as usize;
    let mut profile = Vec::with_capacity(2 * (bands + 1));
    for band in 0..=bands {
        let angle =
            -std::f32::consts::FRAC_PI_2 + band as f32 * std::f32::consts::FRAC_PI_2 / bands as f32;
        profile.push((
            radius * angle.cos().abs(),
            -height * 0.5 + radius * angle.sin(),
        ));
    }
    for band in 0..=bands {
        let angle = band as f32 * std::f32::consts::FRAC_PI_2 / bands as f32;
        profile.push((
            radius * angle.cos().abs(),
            height * 0.5 + radius * angle.sin(),
        ));
    }
    radial_geometry(&profile, segments)
}

pub fn torus_geometry(
    radius: f32,
    tube: f32,
    segments: usize,
) -> Result<(Vec<[f32; 3]>, Vec<[usize; 3]>), MeshProjectionError> {
    if !radius.is_finite() || radius <= 0.0 || !tube.is_finite() || tube <= 0.0 {
        return Err(MeshProjectionError::InvalidGeometry);
    }
    let segments = segments.clamp(3, 64);
    let tube_segments = (segments as f32 * 0.5).round().max(3.0) as usize;
    let mut vertices = Vec::with_capacity(segments * tube_segments);
    let mut faces = Vec::with_capacity(segments * tube_segments * 2);
    for major in 0..segments {
        let around = major as f32 * std::f32::consts::TAU / segments as f32;
        for minor in 0..tube_segments {
            let cross_section = minor as f32 * std::f32::consts::TAU / tube_segments as f32;
            let radial = radius + tube * cross_section.cos();
            vertices.push([
                radial * around.cos(),
                tube * cross_section.sin(),
                radial * around.sin(),
            ]);
        }
    }
    for major in 0..segments {
        for minor in 0..tube_segments {
            let a = major * tube_segments + minor;
            let b = major * tube_segments + (minor + 1) % tube_segments;
            let c = ((major + 1) % segments) * tube_segments + minor;
            let d = ((major + 1) % segments) * tube_segments + (minor + 1) % tube_segments;
            faces.extend([[a, b, c], [b, d, c]]);
        }
    }
    Ok((vertices, faces))
}

pub fn plane_geometry(
    width: f32,
    depth: f32,
) -> Result<(Vec<[f32; 3]>, Vec<[usize; 3]>), MeshProjectionError> {
    if [width, depth]
        .iter()
        .any(|value| !value.is_finite() || *value <= 0.0)
    {
        return Err(MeshProjectionError::InvalidGeometry);
    }
    let (x, y) = (width * 0.5, depth * 0.5);
    Ok((
        vec![[-x, -y, 0.0], [x, -y, 0.0], [x, y, 0.0], [-x, y, 0.0]],
        vec![[0, 1, 2], [0, 2, 3]],
    ))
}

pub fn circle_geometry(
    radius: f32,
    segments: usize,
) -> Result<(Vec<[f32; 3]>, Vec<[usize; 3]>), MeshProjectionError> {
    if !radius.is_finite() || radius <= 0.0 {
        return Err(MeshProjectionError::InvalidGeometry);
    }
    let segments = segments.clamp(3, 64);
    let mut vertices = Vec::with_capacity(segments + 1);
    vertices.push([0.0, 0.0, 0.0]);
    let mut faces = Vec::with_capacity(segments);
    for segment in 0..segments {
        let angle = segment as f32 * std::f32::consts::TAU / segments as f32;
        vertices.push([radius * angle.cos(), radius * angle.sin(), 0.0]);
        faces.push([0, segment + 1, (segment + 1) % segments + 1]);
    }
    Ok((vertices, faces))
}

fn parametric_face_uvs(
    shape: &str,
    vertices: &[[f32; 3]],
    faces: &[[usize; 3]],
) -> Result<Vec<[[f32; 2]; 3]>, MeshProjectionError> {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for vertex in vertices {
        for axis in 0..3 {
            min[axis] = min[axis].min(vertex[axis]);
            max[axis] = max[axis].max(vertex[axis]);
        }
    }
    let unit = |value: f32, axis: usize| {
        if max[axis] > min[axis] {
            (value - min[axis]) / (max[axis] - min[axis])
        } else {
            0.5
        }
    };
    let angle =
        |y: f32, x: f32| y.atan2(x).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
    let unwrap = |uvs: &mut [[f32; 2]; 3], axis: usize| {
        let low = uvs.iter().map(|uv| uv[axis]).fold(f32::INFINITY, f32::min);
        let high = uvs
            .iter()
            .map(|uv| uv[axis])
            .fold(f32::NEG_INFINITY, f32::max);
        if high - low > 0.5 {
            for uv in uvs {
                if uv[axis] < 0.5 {
                    uv[axis] += 1.0;
                }
            }
        }
    };
    let major_radius = if shape == "torus" {
        vertices.iter().map(|p| p[0].hypot(p[2])).sum::<f32>() / vertices.len() as f32
    } else {
        0.0
    };
    let mut result = Vec::with_capacity(faces.len());
    for face in faces {
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
        let mut uvs = [[0.0; 2]; 3];
        match shape {
            "box" => {
                let normal = cross(
                    sub(positions[1], positions[0]),
                    sub(positions[2], positions[0]),
                );
                let axis = (0..3)
                    .max_by(|a, b| normal[*a].abs().total_cmp(&normal[*b].abs()))
                    .ok_or(MeshProjectionError::InvalidFace)?;
                let (u_axis, v_axis) = match axis {
                    0 => (1, 2),
                    1 => (0, 2),
                    _ => (0, 1),
                };
                for (uv, position) in uvs.iter_mut().zip(positions) {
                    *uv = [
                        unit(position[u_axis], u_axis),
                        1.0 - unit(position[v_axis], v_axis),
                    ];
                }
            }
            "plane" | "circle" => {
                for (uv, position) in uvs.iter_mut().zip(positions) {
                    *uv = [unit(position[0], 0), 1.0 - unit(position[1], 1)];
                }
            }
            "sphere" | "capsule" | "cylinder" | "cone" => {
                let same_height = positions
                    .iter()
                    .all(|p| (p[2] - positions[0][2]).abs() < 1e-6);
                let cap = matches!(shape, "cylinder" | "cone") && same_height;
                if cap {
                    let radius = max[0]
                        .abs()
                        .max(min[0].abs())
                        .max(max[1].abs())
                        .max(min[1].abs());
                    for (uv, position) in uvs.iter_mut().zip(positions) {
                        *uv = [
                            0.5 + position[0] / (2.0 * radius),
                            0.5 - position[1] / (2.0 * radius),
                        ];
                    }
                } else {
                    for (uv, position) in uvs.iter_mut().zip(positions) {
                        *uv = [angle(position[1], position[0]), 1.0 - unit(position[2], 2)];
                    }
                    unwrap(&mut uvs, 0);
                    for index in 0..3 {
                        if positions[index][0].hypot(positions[index][1]) < 1e-6 {
                            uvs[index][0] =
                                (uvs[(index + 1) % 3][0] + uvs[(index + 2) % 3][0]) * 0.5;
                        }
                    }
                }
            }
            "torus" => {
                for (uv, position) in uvs.iter_mut().zip(positions) {
                    let r = position[0].hypot(position[2]);
                    *uv = [
                        angle(position[2], position[0]),
                        angle(position[1], r - major_radius),
                    ];
                }
                unwrap(&mut uvs, 0);
                unwrap(&mut uvs, 1);
            }
            _ => return Err(MeshProjectionError::InvalidGeometry),
        }
        result.push(uvs);
    }
    Ok(result)
}

fn rotate_scene_vertex(vertex: [f32; 3], rotation: [f32; 3]) -> [f32; 3] {
    let (sin_x, cos_x) = rotation[0].sin_cos();
    let (sin_y, cos_y) = rotation[1].sin_cos();
    let (sin_z, cos_z) = rotation[2].sin_cos();
    let world = [vertex[0], vertex[2], vertex[1]];
    let z = [
        cos_z * world[0] - sin_z * world[1],
        sin_z * world[0] + cos_z * world[1],
        world[2],
    ];
    let y = [
        cos_y * z[0] + sin_y * z[2],
        z[1],
        -sin_y * z[0] + cos_y * z[2],
    ];
    let x = [
        y[0],
        cos_x * y[1] - sin_x * y[2],
        sin_x * y[1] + cos_x * y[2],
    ];
    [x[0], x[2], x[1]]
}

fn unsupported(detail: impl Into<String>) -> CanvasSceneError {
    CanvasSceneError::Unsupported3DContract(detail.into())
}

fn number(value: &Value, name: &str) -> Result<f32, CanvasSceneError> {
    let value = value
        .as_f64()
        .ok_or_else(|| unsupported(format!("{name} must be a number")))?;
    let value = value as f32;
    value
        .is_finite()
        .then_some(value)
        .ok_or_else(|| unsupported(format!("{name} must be finite")))
}

fn scene_position(value: &Value, name: &str) -> Result<[f32; 3], CanvasSceneError> {
    let object = value
        .as_object()
        .ok_or_else(|| unsupported(format!("{name} must be an object")))?;
    for key in object.keys() {
        if !matches!(key.as_str(), "x" | "y" | "z") {
            return Err(unsupported(format!("{name}.{key}")));
        }
    }
    Ok([
        number(
            object
                .get("x")
                .ok_or_else(|| unsupported(format!("{name}.x is required")))?,
            &format!("{name}.x"),
        )?,
        number(
            object
                .get("y")
                .ok_or_else(|| unsupported(format!("{name}.y is required")))?,
            &format!("{name}.y"),
        )?,
        object
            .get("z")
            .map(|value| number(value, &format!("{name}.z")))
            .transpose()?
            .unwrap_or(0.0),
    ])
}

fn geometry_array<const N: usize>(
    value: &Value,
    name: &str,
) -> Result<Vec<[f32; N]>, CanvasSceneError> {
    value
        .as_array()
        .ok_or_else(|| unsupported(format!("{name} must be an array")))?
        .iter()
        .map(|row| {
            let values = row
                .as_array()
                .filter(|values| values.len() == N)
                .ok_or_else(|| unsupported(format!("{name} entries must have {N} numbers")))?;
            values
                .iter()
                .map(|value| number(value, name))
                .collect::<Result<Vec<_>, _>>()?
                .try_into()
                .map_err(|_| unsupported(format!("{name} entries must have {N} numbers")))
        })
        .collect()
}

fn faces(value: &Value) -> Result<Vec<[usize; 3]>, CanvasSceneError> {
    value
        .as_array()
        .ok_or_else(|| unsupported("draw-mesh.faces must be an array"))?
        .iter()
        .map(|face| {
            let values = face
                .as_array()
                .filter(|values| values.len() == 3)
                .ok_or_else(|| unsupported("draw-mesh.faces entries must have 3 indices"))?;
            values
                .iter()
                .map(|value| {
                    value
                        .as_u64()
                        .and_then(|value| usize::try_from(value).ok())
                        .ok_or_else(|| {
                            unsupported("draw-mesh.faces indices must be nonnegative integers")
                        })
                })
                .collect::<Result<Vec<_>, _>>()?
                .try_into()
                .map_err(|_| unsupported("draw-mesh.faces entries must have 3 indices"))
        })
        .collect()
}

/// An explicit, unlit, double-sided 3D scene subset. Every unsupported authored
/// field is rejected while the complete `draw-mesh` contract remains unregistered.
pub fn from_canvas_node_in_viewport(
    node: &Value,
    width: f32,
    height: f32,
) -> Result<GameRenderPacket, CanvasSceneError> {
    let object = node.as_object().ok_or(CanvasSceneError::NotAnObject)?;
    let fields = object
        .get("props")
        .and_then(Value::as_object)
        .unwrap_or(object);
    if fields.get("mode").and_then(Value::as_str) != Some("3d") {
        return Err(CanvasSceneError::UnsupportedMode(
            fields
                .get("mode")
                .and_then(Value::as_str)
                .unwrap_or("2d")
                .to_owned(),
        ));
    }
    for key in fields.keys() {
        if !matches!(
            key.as_str(),
            "type" | "mode" | "drawables" | "children" | "camera" | "backgroundColor"
        ) {
            return Err(unsupported(format!("canvas.{key}")));
        }
    }
    let camera = fields
        .get("camera")
        .and_then(Value::as_object)
        .ok_or_else(|| unsupported("canvas.camera with pos, target, and fov is required"))?;
    for key in camera.keys() {
        if !matches!(key.as_str(), "pos" | "target" | "fov") {
            return Err(unsupported(format!("canvas.camera.{key}")));
        }
    }
    let eye = scene_position(
        camera
            .get("pos")
            .ok_or_else(|| unsupported("canvas.camera.pos is required"))?,
        "canvas.camera.pos",
    )?;
    let target = scene_position(
        camera
            .get("target")
            .ok_or_else(|| unsupported("canvas.camera.target is required"))?,
        "canvas.camera.target",
    )?;
    let fov_y_degrees = number(
        camera
            .get("fov")
            .ok_or_else(|| unsupported("canvas.camera.fov is required"))?,
        "canvas.camera.fov",
    )?;
    let camera = Camera3D {
        eye: [eye[0], eye[2], eye[1]],
        target: [target[0], target[2], target[1]],
        up: [0.0, 1.0, 0.0],
        fov_y_degrees,
        near: 0.1,
        far: 1000.0,
    };
    camera
        .basis(width, height)
        .map_err(|error| unsupported(format!("canvas.camera: {error:?}")))?;
    let background = fields
        .get("backgroundColor")
        .and_then(Value::as_str)
        .and_then(crate::mappers::game_canvas_2d::parse_css_color)
        .ok_or_else(|| unsupported("canvas.backgroundColor must be an authored CSS color"))?;
    let mut drawables = fields
        .get("drawables")
        .map(|value| {
            value
                .as_array()
                .cloned()
                .ok_or_else(|| unsupported("canvas.drawables must be an array"))
        })
        .transpose()?
        .unwrap_or_default();
    if let Some(children) = fields.get("children") {
        drawables.extend(crate::mappers::game_canvas_2d::to_drawable_nodes(children));
    }
    if drawables.is_empty() {
        return Err(CanvasSceneError::NothingToDraw("canvas mode 3d".into()));
    }
    let mut draw_commands = vec![DrawCommand::Clear { color: background }];
    for (index, node) in drawables.iter().enumerate() {
        let node = node
            .as_object()
            .ok_or_else(|| unsupported(format!("drawables[{index}] must be an object")))?;
        if node.get("type").and_then(Value::as_str) != Some("draw-mesh") {
            return Err(unsupported(format!("drawables[{index}].type")));
        }
        let shape = node
            .get("shape")
            .and_then(Value::as_str)
            .ok_or_else(|| unsupported(format!("drawables[{index}].shape is required")))?;
        let dimensions: &[&str] = match shape {
            "box" => &["width", "height", "depth"],
            "sphere" | "circle" => &["radius", "segments"],
            "capsule" | "cone" => &["radius", "height", "segments"],
            "cylinder" => &["radius", "radiusTop", "radiusBottom", "height", "segments"],
            "torus" => &["radius", "tube", "segments"],
            "plane" => &["width", "depth"],
            "polyhedron" => &["vertices", "faces", "uvs"],
            _ => return Err(unsupported(format!("drawables[{index}].shape"))),
        };
        for key in node.keys() {
            if !matches!(
                key.as_str(),
                "type" | "shape" | "position" | "pivot" | "material" | "rotation" | "opacity"
            ) && !dimensions.contains(&key.as_str())
            {
                return Err(unsupported(format!("drawables[{index}].{key}")));
            }
        }
        let material = node
            .get("material")
            .and_then(Value::as_object)
            .ok_or_else(|| unsupported(format!("drawables[{index}].material is required")))?;
        if material.get("kind").and_then(Value::as_str) != Some("basic") {
            return Err(unsupported(format!("drawables[{index}].material.kind")));
        }
        for key in material.keys() {
            if !matches!(key.as_str(), "kind" | "side" | "color" | "opacity" | "map") {
                return Err(unsupported(format!("drawables[{index}].material.{key}")));
            }
        }
        let side = match material.get("side").and_then(Value::as_str) {
            None | Some("front") => MeshSide::Front,
            Some("back") => MeshSide::Back,
            Some("double") => MeshSide::Double,
            _ => return Err(unsupported(format!("drawables[{index}].material.side"))),
        };
        if let Some(opacity) = material.get("opacity") {
            if number(opacity, "draw-mesh.material.opacity")? != 1.0 {
                return Err(unsupported(format!("drawables[{index}].material.opacity")));
            }
        }
        if let Some(opacity) = node.get("opacity") {
            if number(opacity, "draw-mesh.opacity")? != 1.0 {
                return Err(unsupported(format!("drawables[{index}].opacity")));
            }
        }
        let color = material
            .get("color")
            .map(Value::as_str)
            .unwrap_or(Some("#ffffff"))
            .and_then(crate::mappers::game_canvas_2d::parse_css_color)
            .ok_or_else(|| unsupported(format!("drawables[{index}].material.color")))?;
        if color[3] != 1.0 {
            return Err(unsupported(format!(
                "drawables[{index}].material.color alpha"
            )));
        }
        let texture = material
            .get("map")
            .map(|value| {
                value
                    .as_str()
                    .filter(|url| !url.is_empty())
                    .ok_or_else(|| unsupported(format!("drawables[{index}].material.map")))
            })
            .transpose()?;
        let uvs = if let Some(texture) = texture {
            let uvs = if shape == "polyhedron" {
                Some(geometry_array::<2>(
                    node.get("uvs").ok_or_else(|| {
                        unsupported(format!("drawables[{index}].uvs is required"))
                    })?,
                    "draw-mesh.uvs",
                )?)
            } else {
                None
            };
            crate::asset::ensure_asset(texture);
            uvs
        } else {
            if node.contains_key("uvs") {
                return Err(unsupported(format!(
                    "drawables[{index}].uvs require material.map"
                )));
            }
            None
        };
        let position = scene_position(
            node.get("position")
                .ok_or_else(|| unsupported(format!("drawables[{index}].position is required")))?,
            &format!("drawables[{index}].position"),
        )?;
        let pivot_bottom = match node.get("pivot").and_then(Value::as_str) {
            None | Some("bottom") => true,
            Some("center") => false,
            _ => return Err(unsupported(format!("drawables[{index}].pivot"))),
        };
        let rotation = node
            .get("rotation")
            .map(|value| {
                let values = value
                    .as_array()
                    .filter(|values| values.len() == 3)
                    .ok_or_else(|| {
                        unsupported(format!("drawables[{index}].rotation must have 3 angles"))
                    })?;
                Ok::<_, CanvasSceneError>([
                    number(&values[0], "draw-mesh.rotation.x")?,
                    number(&values[1], "draw-mesh.rotation.y")?,
                    number(&values[2], "draw-mesh.rotation.z")?,
                ])
            })
            .transpose()?
            .unwrap_or([0.0; 3]);
        if matches!(shape, "plane" | "circle") && node.contains_key("rotation") {
            return Err(unsupported(format!(
                "drawables[{index}].rotation on {shape} requires intrinsic ground-plane composition"
            )));
        }
        let dimension = |key: &str, default: f32| {
            node.get(key)
                .map(|value| number(value, key))
                .transpose()
                .map(|value| value.unwrap_or(default))
        };
        let segments = || -> Result<usize, CanvasSceneError> {
            Ok(dimension("segments", 24.0)?.round().clamp(3.0, 64.0) as usize)
        };
        let (mut vertices, faces) = match shape {
            "box" => {
                let width = dimension("width", 1.0)?;
                let height = dimension("height", 1.0)?;
                let depth = dimension("depth", width)?;
                let (vertices, faces) =
                    box_geometry(width, depth, height, false).map_err(|error| {
                        unsupported(format!("drawables[{index}] geometry: {error:?}"))
                    })?;
                (vertices.to_vec(), faces.to_vec())
            }
            "sphere" => sphere_geometry(dimension("radius", 0.4)?, segments()?)
                .map_err(|error| unsupported(format!("drawables[{index}] geometry: {error:?}")))?,
            "capsule" => capsule_geometry(
                dimension("radius", 0.4)?,
                dimension("height", 1.0)?,
                segments()?,
            )
            .map_err(|error| unsupported(format!("drawables[{index}] geometry: {error:?}")))?,
            "cylinder" => {
                let radius = dimension("radius", 0.4)?;
                cylinder_geometry(
                    dimension("radiusTop", radius)?,
                    dimension("radiusBottom", radius)?,
                    dimension("height", 1.0)?,
                    segments()?,
                )
                .map_err(|error| unsupported(format!("drawables[{index}] geometry: {error:?}")))?
            }
            "cone" => cylinder_geometry(
                0.0,
                dimension("radius", 0.4)?,
                dimension("height", 1.0)?,
                segments()?,
            )
            .map_err(|error| unsupported(format!("drawables[{index}] geometry: {error:?}")))?,
            "torus" => {
                let radius = dimension("radius", 0.4)?;
                torus_geometry(radius, dimension("tube", radius / 3.0)?, segments()?).map_err(
                    |error| unsupported(format!("drawables[{index}] geometry: {error:?}")),
                )?
            }
            "plane" => plane_geometry(
                dimension("width", 1.0)?,
                dimension("depth", dimension("width", 1.0)?)?,
            )
            .map_err(|error| unsupported(format!("drawables[{index}] geometry: {error:?}")))?,
            "circle" => circle_geometry(dimension("radius", 0.4)?, segments()?)
                .map_err(|error| unsupported(format!("drawables[{index}] geometry: {error:?}")))?,
            "polyhedron" => {
                let vertices = geometry_array::<3>(
                    node.get("vertices").ok_or_else(|| {
                        unsupported(format!("drawables[{index}].vertices is required"))
                    })?,
                    "draw-mesh.vertices",
                )?;
                let faces = faces(node.get("faces").ok_or_else(|| {
                    unsupported(format!("drawables[{index}].faces is required"))
                })?)?;
                (vertices, faces)
            }
            _ => return Err(unsupported(format!("drawables[{index}].shape"))),
        };
        let face_uvs = if texture.is_some() && shape != "polyhedron" {
            Some(
                parametric_face_uvs(shape, &vertices, &faces).map_err(|error| {
                    unsupported(format!("drawables[{index}] UV geometry: {error:?}"))
                })?,
            )
        } else {
            None
        };
        let bottom_lift = if pivot_bottom {
            -vertices
                .iter()
                .map(|vertex| vertex[2])
                .fold(f32::INFINITY, f32::min)
        } else {
            0.0
        };
        for vertex in &mut vertices {
            *vertex = rotate_scene_vertex(*vertex, rotation);
            vertex[0] += position[0];
            vertex[1] += position[1];
            vertex[2] += position[2] + bottom_lift;
        }
        let command = match (texture, uvs.as_deref(), face_uvs.as_deref()) {
            (Some(texture), Some(uvs), _) => project_polyhedron_textured(
                &vertices, &faces, uvs, texture, camera, width, height, color, side,
            ),
            (Some(texture), None, Some(face_uvs)) => project_polyhedron_textured_faces(
                &vertices, &faces, face_uvs, texture, camera, width, height, color, side,
            ),
            (None, None, None) => {
                project_polyhedron_with_side(&vertices, &faces, camera, width, height, color, side)
            }
            _ => {
                return Err(unsupported(format!(
                    "drawables[{index}].material.map UV contract"
                )))
            }
        }
        .map_err(|error| unsupported(format!("drawables[{index}] projection: {error:?}")))?;
        draw_commands.push(command);
    }
    Ok(GameRenderPacket {
        pattern: "canvas".into(),
        width,
        height,
        background_image: None,
        asset_base_url: None,
        camera: None,
        draw_commands,
        tick_event: None,
        draw_event: None,
        fps: None,
        animations: Vec::new(),
    })
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
    fn polyhedron_faces_crossing_near_and_far_planes_are_clipped() {
        let camera = Camera3D {
            eye: [0.0, 0.0, 5.0],
            target: [0.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            fov_y_degrees: 60.0,
            near: 0.1,
            far: 2.0,
        };
        for positions in [
            [[-0.1, 4.95, 0.0], [0.1, 4.0, 0.0], [0.0, 4.0, 0.1]],
            [[-0.1, 2.0, 0.0], [0.1, 4.0, 0.0], [0.0, 4.0, 0.1]],
        ] {
            let DrawCommand::Mesh3D { vertices } =
                project_polyhedron(&positions, &[[0, 1, 2]], camera, 100.0, 100.0, [0.5; 4])
                    .unwrap()
            else {
                panic!("clipped polyhedron must produce a mesh command")
            };
            assert_eq!(vertices.len(), 6);
            assert!(vertices
                .iter()
                .all(|vertex| vertex.z >= 0.0 && vertex.z <= 1.0));
            assert!(vertices
                .iter()
                .all(|vertex| vertex.x.is_finite() && vertex.y.is_finite()));
        }
    }

    #[test]
    fn fully_clipped_polyhedron_has_no_drawable_triangles() {
        let camera = Camera3D {
            eye: [0.0, 0.0, 5.0],
            target: [0.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            fov_y_degrees: 60.0,
            near: 0.1,
            far: 2.0,
        };
        assert_eq!(
            project_polyhedron(
                &[[-0.1, 2.0, 0.0], [0.1, 2.0, 0.0], [0.0, 2.0, 0.1]],
                &[[0, 1, 2]],
                camera,
                100.0,
                100.0,
                [0.5; 4],
            )
            .unwrap_err(),
            MeshProjectionError::ClippedTriangle
        );
    }

    #[test]
    fn material_side_culls_scene_winding_from_the_camera() {
        let camera = Camera3D {
            eye: [0.0, 4.0, 0.0],
            target: [0.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov_y_degrees: 60.0,
            near: 0.1,
            far: 20.0,
        };
        let (vertices, faces) = plane_geometry(2.0, 2.0).unwrap();
        let count = |side| {
            let DrawCommand::Mesh3D { vertices } = project_polyhedron_with_side(
                &vertices, &faces, camera, 100.0, 100.0, [1.0; 4], side,
            )
            .unwrap() else {
                panic!("plane must produce a mesh command")
            };
            vertices.len()
        };
        assert_eq!(count(MeshSide::Front), 6);
        assert_eq!(count(MeshSide::Back), 0);
        assert_eq!(count(MeshSide::Double), 6);

        let camera_below = Camera3D {
            eye: [0.0, -4.0, 0.0],
            up: [0.0, 0.0, -1.0],
            ..camera
        };
        let DrawCommand::Mesh3D { vertices: back } = project_polyhedron_with_side(
            &vertices,
            &faces,
            camera_below,
            100.0,
            100.0,
            [1.0; 4],
            MeshSide::Back,
        )
        .unwrap() else {
            panic!("back-facing plane must produce a mesh command")
        };
        assert_eq!(back.len(), 6);
    }

    #[test]
    fn basic_material_defaults_to_front_white_and_rejects_transparency() {
        let mut node = serde_json::json!({
            "type": "canvas", "mode": "3d", "backgroundColor": "#000000",
            "camera": {"pos": {"x": 0, "y": 1, "z": 4},
                "target": {"x": 0, "y": 0, "z": 0}, "fov": 60},
            "drawables": [{"type": "draw-mesh", "shape": "plane",
                "position": {"x": 0, "y": 0, "z": 0},
                "material": {"kind": "basic"}}]
        });
        let packet = from_canvas_node_in_viewport(&node, 100.0, 100.0).unwrap();
        let DrawCommand::Mesh3D { vertices } = &packet.draw_commands[1] else {
            panic!("default front side must produce a mesh command")
        };
        assert_eq!(vertices.len(), 6);
        assert!(vertices.iter().all(|vertex| vertex.color == [1.0; 4]));

        node["drawables"][0]["material"]["opacity"] = serde_json::json!(0.5);
        assert!(matches!(
            from_canvas_node_in_viewport(&node, 100.0, 100.0),
            Err(CanvasSceneError::Unsupported3DContract(detail)) if detail.ends_with("material.opacity")
        ));
        node["drawables"][0]["material"]["opacity"] = serde_json::json!(1.0);
        node["drawables"][0]["opacity"] = serde_json::json!(0.5);
        assert!(matches!(
            from_canvas_node_in_viewport(&node, 100.0, 100.0),
            Err(CanvasSceneError::Unsupported3DContract(detail)) if detail.ends_with("].opacity")
        ));
    }

    #[test]
    fn textured_polyhedron_preserves_uvs_through_depth_clipping() {
        let camera = Camera3D {
            eye: [0.0, 0.0, 5.0],
            target: [0.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            fov_y_degrees: 60.0,
            near: 0.1,
            far: 20.0,
        };
        let source_vertices = [[-0.2, 4.95, 0.0], [0.2, 4.0, 0.0], [0.0, 4.0, 0.2]];
        let uvs = [[0.0, 0.0], [1.0, 0.0], [0.5, 1.0]];
        let DrawCommand::TexturedMesh3D { texture, vertices } = project_polyhedron_textured(
            &source_vertices,
            &[[0, 1, 2]],
            &uvs,
            "data:image/png;base64,AA==",
            camera,
            100.0,
            100.0,
            [1.0; 4],
            MeshSide::Double,
        )
        .unwrap() else {
            panic!("textured polyhedron must retain a texture command")
        };
        assert_eq!(texture, "data:image/png;base64,AA==");
        assert_eq!(vertices.len(), 6);
        assert!(vertices
            .iter()
            .all(|vertex| vertex.inverse_depth.is_finite()
                && vertex.inverse_depth > 0.0
                && vertex.uv.iter().all(|uv| uv.is_finite())));
        assert!(vertices
            .iter()
            .any(|vertex| vertex.uv[0] > 0.0 && vertex.uv[0] < 1.0));
        assert_eq!(
            project_polyhedron_textured(
                &source_vertices,
                &[[0, 1, 2]],
                &uvs[..2],
                "texture",
                camera,
                100.0,
                100.0,
                [1.0; 4],
                MeshSide::Double,
            )
            .unwrap_err(),
            MeshProjectionError::InvalidVertex
        );
    }

    #[test]
    fn authored_polyhedron_uv_map_reaches_depth_packet() {
        let node = serde_json::json!({
            "type": "canvas", "mode": "3d", "backgroundColor": "#000000",
            "camera": {"pos": {"x": 0, "y": 5, "z": 2},
                "target": {"x": 0, "y": 0, "z": 0}, "fov": 60},
            "drawables": [{"type": "draw-mesh", "shape": "polyhedron",
                "position": {"x": 0, "y": 0, "z": 0},
                "vertices": [[-1, 0, 0], [1, 0, 0], [0, 0, 1]],
                "faces": [[0, 1, 2]], "uvs": [[0, 0], [1, 0], [0.5, 1]],
                "material": {"kind": "basic", "side": "double",
                    "map": "data:image/png;base64,AA=="}}]
        });
        let packet = from_canvas_node_in_viewport(&node, 100.0, 100.0).unwrap();
        let DrawCommand::TexturedMesh3D { vertices, .. } = &packet.draw_commands[1] else {
            panic!("authored UV map must reach depth packet")
        };
        assert_eq!(vertices.len(), 3);

        let mut missing_uvs = node;
        missing_uvs["drawables"][0]
            .as_object_mut()
            .unwrap()
            .remove("uvs");
        assert!(matches!(
            from_canvas_node_in_viewport(&missing_uvs, 100.0, 100.0),
            Err(CanvasSceneError::Unsupported3DContract(detail)) if detail.ends_with("].uvs is required")
        ));
    }

    #[test]
    fn every_parametric_mesh_shape_generates_textured_depth_triangles() {
        for shape in [
            "box", "sphere", "capsule", "cylinder", "cone", "torus", "plane", "circle",
        ] {
            let node = serde_json::json!({
                "type": "canvas", "mode": "3d", "backgroundColor": "#000000",
                "camera": {"pos": {"x": 0, "y": 5, "z": 2},
                    "target": {"x": 0, "y": 0, "z": 0}, "fov": 60},
                "drawables": [{"type": "draw-mesh", "shape": shape,
                    "position": {"x": 0, "y": 0, "z": 0},
                    "material": {"kind": "basic", "side": "double", "map": "data:image/png;base64,AA=="}}]
            });
            let packet = from_canvas_node_in_viewport(&node, 320.0, 240.0).unwrap();
            let DrawCommand::TexturedMesh3D { vertices, .. } = &packet.draw_commands[1] else {
                panic!("{shape} map must produce textured depth triangles")
            };
            assert!(!vertices.is_empty(), "{shape}");
            assert_eq!(vertices.len() % 3, 0, "{shape}");
            assert!(
                vertices
                    .iter()
                    .all(|vertex| vertex.uv.iter().all(|uv| uv.is_finite())),
                "{shape}"
            );
        }
    }

    #[test]
    fn generated_uvs_keep_box_faces_and_curved_seams_distinct() {
        let (box_vertices, box_faces) = box_geometry(2.0, 2.0, 2.0, false).unwrap();
        let box_uvs = parametric_face_uvs("box", &box_vertices, &box_faces).unwrap();
        assert_eq!(box_uvs.len(), 12);
        assert!(box_uvs
            .iter()
            .all(|face| face.iter().any(|uv| uv[0] == 0.0) && face.iter().any(|uv| uv[0] == 1.0)));

        let (sphere_vertices, sphere_faces) = sphere_geometry(1.0, 8).unwrap();
        let sphere_uvs = parametric_face_uvs("sphere", &sphere_vertices, &sphere_faces).unwrap();
        assert!(sphere_uvs.iter().all(|face| {
            let u = face.map(|uv| uv[0]);
            u.into_iter().fold(f32::NEG_INFINITY, f32::max)
                - u.into_iter().fold(f32::INFINITY, f32::min)
                <= 0.5 + 1e-6
        }));
        assert!(sphere_uvs.iter().flatten().any(|uv| uv[0] >= 1.0));
    }

    #[test]
    fn normals_follow_authored_winding_and_area_weighted_shared_vertices() {
        let vertices = [
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
        ];
        let normals = mesh_normals(&vertices, &[[0, 1, 2], [0, 3, 1]]).unwrap();
        assert_eq!(normals.faces[0], [0.0, 0.0, 1.0]);
        assert_eq!(normals.faces[1], [0.0, 1.0, 0.0]);
        assert_eq!(normals.vertices[2], [0.0, 0.0, 1.0]);
        assert_eq!(normals.vertices[3], [0.0, 1.0, 0.0]);
        assert!((normals.vertices[0][1] - 2.0_f32.sqrt().recip()).abs() < 1e-6);
        assert!((normals.vertices[0][2] - 2.0_f32.sqrt().recip()).abs() < 1e-6);
        assert_eq!(
            mesh_normals(&vertices, &[[0, 0, 2]]).unwrap_err(),
            MeshProjectionError::InvalidFace
        );
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

    #[test]
    fn explicit_basic_box_scene_projects_to_depth_triangles() {
        let node = serde_json::json!({
            "type": "canvas",
            "props": {
                "mode": "3d",
                "backgroundColor": "#000000",
                "camera": {
                    "pos": {"x": 0, "y": 5, "z": 2},
                    "target": {"x": 0, "y": 0, "z": 0},
                    "fov": 60
                },
                "drawables": [{
                    "type": "draw-mesh", "shape": "box",
                    "position": {"x": 0, "y": 0, "z": 0},
                    "material": {"kind": "basic", "side": "double", "color": "#ff0000"}
                }]
            }
        });
        let packet = from_canvas_node_in_viewport(&node, 320.0, 240.0).unwrap();
        assert_eq!(packet.width, 320.0);
        assert_eq!(packet.height, 240.0);
        assert_eq!(packet.draw_commands.len(), 2);
        let DrawCommand::Mesh3D { vertices } = &packet.draw_commands[1] else {
            panic!("the explicit 3D scene must paint a mesh")
        };
        assert!(!vertices.is_empty());
        assert!(vertices
            .iter()
            .all(|vertex| vertex.color == [1.0, 0.0, 0.0, 1.0]));
    }

    #[test]
    fn unsupported_3d_material_is_rejected_before_packet_paint() {
        let node = serde_json::json!({
            "type": "canvas", "mode": "3d", "backgroundColor": "#000000",
            "camera": {"pos": {"x": 0, "y": 5, "z": 2},
                "target": {"x": 0, "y": 0, "z": 0}, "fov": 60},
            "drawables": [{"type": "draw-mesh", "shape": "box",
                "position": {"x": 0, "y": 0},
                "material": {"kind": "standard", "side": "double", "color": "#ff0000"}}]
        });
        assert!(matches!(
            from_canvas_node_in_viewport(&node, 320.0, 240.0),
            Err(CanvasSceneError::Unsupported3DContract(detail))
                if detail.contains("material")
        ));
    }

    #[test]
    fn all_parametric_mesh_shapes_reach_the_depth_packet() {
        for shape in [
            "box", "sphere", "capsule", "cylinder", "cone", "torus", "plane", "circle",
        ] {
            let node = serde_json::json!({
                "type": "canvas", "mode": "3d", "backgroundColor": "#000000",
                "camera": {"pos": {"x": 0, "y": 5, "z": 2},
                    "target": {"x": 0, "y": 0, "z": 0}, "fov": 60},
                "drawables": [{"type": "draw-mesh", "shape": shape,
                    "position": {"x": 0, "y": 0, "z": 0},
                    "material": {"kind": "basic", "side": "double", "color": "#ff0000"}}]
            });
            let packet = from_canvas_node_in_viewport(&node, 320.0, 240.0).unwrap();
            let DrawCommand::Mesh3D { vertices } = &packet.draw_commands[1] else {
                panic!("{shape} must produce a mesh command")
            };
            assert!(!vertices.is_empty(), "{shape}");
            assert_eq!(vertices.len() % 3, 0, "{shape}");
            assert!(
                vertices
                    .iter()
                    .all(|vertex| vertex.z >= 0.0 && vertex.z <= 1.0),
                "{shape}"
            );
        }
    }

    #[test]
    fn mesh_rotation_uses_world_xyz_axes_before_pivot_lift() {
        let rotated = rotate_scene_vertex([0.0, 0.0, 1.0], [0.0, 0.0, std::f32::consts::FRAC_PI_2]);
        assert!((rotated[0] + 1.0).abs() < 0.0001);
        assert!(rotated[1].abs() < 0.0001);
        assert!(rotated[2].abs() < 0.0001);

        let node = serde_json::json!({
            "type": "canvas", "mode": "3d", "backgroundColor": "#000000",
            "camera": {"pos": {"x": 0, "y": 5, "z": 2},
                "target": {"x": 0, "y": 0, "z": 0}, "fov": 60},
            "drawables": [{"type": "draw-mesh", "shape": "box",
                "position": {"x": 0, "y": 0, "z": 0},
                "rotation": [0, 0, 1.5707963267948966],
                "material": {"kind": "basic", "side": "double", "color": "#ff0000"}}]
        });
        let packet = from_canvas_node_in_viewport(&node, 320.0, 240.0).unwrap();
        let DrawCommand::Mesh3D { vertices } = &packet.draw_commands[1] else {
            panic!("rotated box must paint a mesh")
        };
        assert_eq!(vertices.len(), 36);
    }

    #[test]
    fn small_authored_sphere_keeps_its_nonzero_triangles() {
        let (vertices, faces) = sphere_geometry(0.01, 12).unwrap();
        let camera = Camera3D {
            eye: [0.0, 0.0, 5.0],
            target: [0.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            fov_y_degrees: 60.0,
            near: 0.1,
            far: 20.0,
        };
        let DrawCommand::Mesh3D { vertices } =
            project_polyhedron(&vertices, &faces, camera, 100.0, 100.0, [1.0; 4]).unwrap()
        else {
            panic!("small sphere must produce mesh triangles")
        };
        assert!(!vertices.is_empty());
    }
}
