//! Mesh builders in local Y-up space, centered on the origin unless stated: positions, normals and UVs per vertex,
//! counter-clockwise front faces. Segment counts are clamped to 3..=128.

use super::model::MeshData;
use std::f32::consts::{PI, TAU};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeometryError(pub String);

fn finite_positive(values: &[(f32, &str)]) -> Result<(), GeometryError> {
    for (value, name) in values {
        if !value.is_finite() || *value <= 0.0 {
            return Err(GeometryError(format!(
                "{name} must be a positive number, got {value}"
            )));
        }
    }
    Ok(())
}

fn segments(count: usize) -> usize {
    count.clamp(3, 128)
}

/// An axis-aligned box, 24 vertices so each face keeps its own normal and UVs.
pub fn cuboid(width: f32, height: f32, depth: f32) -> Result<MeshData, GeometryError> {
    finite_positive(&[(width, "width"), (height, "height"), (depth, "depth")])?;
    let (x, y, z) = (width * 0.5, height * 0.5, depth * 0.5);
    let mut mesh = MeshData::default();
    // normal, then u and v axes of the face, each scaled to the half extents
    let faces: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
        ([1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),
        ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
        ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([0.0, 0.0, -1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    ];
    let half = [x, y, z];
    for (normal, u, v) in faces {
        let base = mesh.positions.len() as u32;
        for (su, sv, uv) in [
            (-1.0, -1.0, [0.0, 1.0]),
            (1.0, -1.0, [1.0, 1.0]),
            (1.0, 1.0, [1.0, 0.0]),
            (-1.0, 1.0, [0.0, 0.0]),
        ] {
            let p = [0, 1, 2].map(|i| (normal[i] + su * u[i] + sv * v[i]) * half[i]);
            mesh.positions.push(p);
            mesh.normals.push(normal);
            mesh.uvs.push(uv);
        }
        mesh.indices
            .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    Ok(mesh)
}

/// A surface of revolution around +Y from a profile of (radius, y) points, bottom to top, with smooth normals taken
/// from the profile's slope and optional flat caps at either end.
fn lathe(profile: &[(f32, f32)], count: usize, cap_bottom: bool, cap_top: bool) -> MeshData {
    let count = segments(count);
    let mut mesh = MeshData::default();
    let rows = profile.len();
    for (row, &(radius, y)) in profile.iter().enumerate() {
        let prev = profile[row.saturating_sub(1)];
        let next = profile[(row + 1).min(rows - 1)];
        let (dr, dy) = (next.0 - prev.0, next.1 - prev.1);
        let (nr, ny) = {
            let len = (dr * dr + dy * dy).sqrt().max(f32::EPSILON);
            (dy / len, -dr / len)
        };
        for s in 0..=count {
            let angle = s as f32 / count as f32 * TAU;
            let (sin, cos) = angle.sin_cos();
            mesh.positions.push([radius * sin, y, radius * cos]);
            mesh.normals.push([nr * sin, ny, nr * cos]);
            mesh.uvs.push([
                s as f32 / count as f32,
                1.0 - row as f32 / (rows - 1) as f32,
            ]);
        }
    }
    let stride = count as u32 + 1;
    for row in 0..rows as u32 - 1 {
        for s in 0..count as u32 {
            let a = row * stride + s;
            let b = a + 1;
            let c = a + stride;
            let d = c + 1;
            mesh.indices.extend([a, b, d, a, d, c]);
        }
    }
    let mut cap = |radius: f32, y: f32, up: bool| {
        if radius <= 0.0 {
            return;
        }
        let normal = [0.0, if up { 1.0 } else { -1.0 }, 0.0];
        let center = mesh.positions.len() as u32;
        mesh.positions.push([0.0, y, 0.0]);
        mesh.normals.push(normal);
        mesh.uvs.push([0.5, 0.5]);
        for s in 0..=count {
            let angle = s as f32 / count as f32 * TAU;
            let (sin, cos) = angle.sin_cos();
            mesh.positions.push([radius * sin, y, radius * cos]);
            mesh.normals.push(normal);
            mesh.uvs.push([0.5 + 0.5 * sin, 0.5 - 0.5 * cos]);
        }
        for s in 0..count as u32 {
            let (a, b) = (center + 1 + s, center + 2 + s);
            if up {
                mesh.indices.extend([center, a, b]);
            } else {
                mesh.indices.extend([center, b, a]);
            }
        }
    };
    if cap_bottom {
        cap(profile[0].0, profile[0].1, false);
    }
    if cap_top {
        cap(profile[rows - 1].0, profile[rows - 1].1, true);
    }
    mesh
}

pub fn sphere(radius: f32, count: usize) -> Result<MeshData, GeometryError> {
    finite_positive(&[(radius, "radius")])?;
    let rings = segments(count);
    let profile: Vec<(f32, f32)> = (0..=rings)
        .map(|r| {
            let phi = -PI / 2.0 + r as f32 / rings as f32 * PI;
            (radius * phi.cos(), radius * phi.sin())
        })
        .collect();
    let mut mesh = lathe(&profile, count, false, false);
    for (p, n) in mesh.positions.iter().zip(mesh.normals.iter_mut()) {
        let len = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2])
            .sqrt()
            .max(f32::EPSILON);
        *n = [p[0] / len, p[1] / len, p[2] / len];
    }
    Ok(mesh)
}

/// A capped cylinder or cone (a zero radius at one end) of total `height`, centered on the origin.
pub fn cylinder(
    radius_top: f32,
    radius_bottom: f32,
    height: f32,
    count: usize,
) -> Result<MeshData, GeometryError> {
    finite_positive(&[(height, "height")])?;
    for (value, name) in [(radius_top, "radiusTop"), (radius_bottom, "radiusBottom")] {
        if !value.is_finite() || value < 0.0 {
            return Err(GeometryError(format!(
                "{name} must be zero or more, got {value}"
            )));
        }
    }
    if radius_top == 0.0 && radius_bottom == 0.0 {
        return Err(GeometryError(
            "a cylinder needs a radius above zero at one end".into(),
        ));
    }
    let h = height * 0.5;
    Ok(lathe(
        &[(radius_bottom, -h), (radius_top, h)],
        count,
        true,
        true,
    ))
}

/// A cylinder of `length` between two hemispheres of `radius`, centered on the origin.
pub fn capsule(radius: f32, length: f32, count: usize) -> Result<MeshData, GeometryError> {
    finite_positive(&[(radius, "radius")])?;
    if !length.is_finite() || length < 0.0 {
        return Err(GeometryError(format!(
            "length must be zero or more, got {length}"
        )));
    }
    let cap_rows = (segments(count) / 2).max(2);
    let half = length * 0.5;
    let mut profile = Vec::new();
    for r in 0..=cap_rows {
        let phi = -PI / 2.0 + r as f32 / cap_rows as f32 * (PI / 2.0);
        profile.push((radius * phi.cos(), -half + radius * phi.sin()));
    }
    for r in 0..=cap_rows {
        let phi = r as f32 / cap_rows as f32 * (PI / 2.0);
        profile.push((radius * phi.cos(), half + radius * phi.sin()));
    }
    Ok(lathe(&profile, count, false, false))
}

/// A torus around +Y: `radius` to the tube's center, `tube` the tube's radius.
pub fn torus(radius: f32, tube: f32, count: usize) -> Result<MeshData, GeometryError> {
    finite_positive(&[(radius, "radius"), (tube, "tube")])?;
    let radial = segments(count);
    let tubular = segments(count * 2);
    let mut mesh = MeshData::default();
    for j in 0..=radial {
        let v = j as f32 / radial as f32 * TAU;
        for i in 0..=tubular {
            let u = i as f32 / tubular as f32 * TAU;
            let center = [radius * u.cos(), 0.0, radius * u.sin()];
            let p = [
                (radius + tube * v.cos()) * u.cos(),
                tube * v.sin(),
                (radius + tube * v.cos()) * u.sin(),
            ];
            let n = [
                (p[0] - center[0]) / tube,
                (p[1] - center[1]) / tube,
                (p[2] - center[2]) / tube,
            ];
            mesh.positions.push(p);
            mesh.normals.push(n);
            mesh.uvs
                .push([i as f32 / tubular as f32, j as f32 / radial as f32]);
        }
    }
    let stride = tubular as u32 + 1;
    for j in 0..radial as u32 {
        for i in 0..tubular as u32 {
            let a = j * stride + i;
            let b = a + 1;
            let c = a + stride;
            let d = c + 1;
            mesh.indices.extend([a, c, d, a, d, b]);
        }
    }
    Ok(mesh)
}

/// A plane in XZ facing +Y.
pub fn plane(width: f32, depth: f32) -> Result<MeshData, GeometryError> {
    finite_positive(&[(width, "width"), (depth, "depth")])?;
    let (x, z) = (width * 0.5, depth * 0.5);
    Ok(MeshData {
        positions: vec![[-x, 0.0, z], [x, 0.0, z], [x, 0.0, -z], [-x, 0.0, -z]],
        normals: vec![[0.0, 1.0, 0.0]; 4],
        uvs: vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]],
        indices: vec![0, 1, 2, 0, 2, 3],
    })
}

/// A disc in XZ facing +Y.
pub fn circle(radius: f32, count: usize) -> Result<MeshData, GeometryError> {
    finite_positive(&[(radius, "radius")])?;
    let count = segments(count);
    let mut mesh = MeshData::default();
    mesh.positions.push([0.0, 0.0, 0.0]);
    mesh.normals.push([0.0, 1.0, 0.0]);
    mesh.uvs.push([0.5, 0.5]);
    for s in 0..=count {
        let angle = s as f32 / count as f32 * TAU;
        let (sin, cos) = angle.sin_cos();
        mesh.positions.push([radius * cos, 0.0, -radius * sin]);
        mesh.normals.push([0.0, 1.0, 0.0]);
        mesh.uvs.push([0.5 + 0.5 * cos, 0.5 + 0.5 * sin]);
    }
    for s in 0..count as u32 {
        mesh.indices.extend([0, s + 1, s + 2]);
    }
    Ok(mesh)
}

/// Authored triangles with normals averaged per vertex from the faces' area-weighted normals (smooth shading).
/// Faces with an out-of-range index or zero area are refused.
pub fn polyhedron(
    positions: Vec<[f32; 3]>,
    faces: &[[u32; 3]],
    uvs: Option<Vec<[f32; 2]>>,
) -> Result<MeshData, GeometryError> {
    if positions.iter().flatten().any(|v| !v.is_finite()) {
        return Err(GeometryError("a vertex holds a non-finite number".into()));
    }
    if let Some(uvs) = &uvs {
        if uvs.len() != positions.len() {
            return Err(GeometryError(format!(
                "{} uvs for {} vertices",
                uvs.len(),
                positions.len()
            )));
        }
    }
    if faces.is_empty() {
        return Err(GeometryError("a polyhedron needs at least one face".into()));
    }
    let mut normals = vec![[0.0f32; 3]; positions.len()];
    for (i, face) in faces.iter().enumerate() {
        let [a, b, c] = face.map(|v| v as usize);
        if a >= positions.len() || b >= positions.len() || c >= positions.len() {
            return Err(GeometryError(format!(
                "face {i} names a vertex past the {} given",
                positions.len()
            )));
        }
        let (pa, pb, pc) = (positions[a], positions[b], positions[c]);
        let e1 = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
        let e2 = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
        let n = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        if n[0] * n[0] + n[1] * n[1] + n[2] * n[2] <= f32::EPSILON * f32::EPSILON {
            return Err(GeometryError(format!("face {i} has zero area")));
        }
        for v in [a, b, c] {
            for k in 0..3 {
                normals[v][k] += n[k];
            }
        }
    }
    for n in &mut normals {
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        *n = if len > 0.0 {
            [n[0] / len, n[1] / len, n[2] / len]
        } else {
            [0.0, 1.0, 0.0]
        };
    }
    let count = positions.len();
    Ok(MeshData {
        positions,
        normals,
        uvs: uvs.unwrap_or_else(|| vec![[0.0, 0.0]; count]),
        indices: faces.iter().flatten().copied().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outward(mesh: &MeshData) -> bool {
        mesh.indices.chunks(3).all(|t| {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| mesh.positions[i as usize]);
            let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let n = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ];
            let avg = mesh.normals[t[0] as usize];
            n[0] * avg[0] + n[1] * avg[1] + n[2] * avg[2] >= -1e-6
        })
    }

    #[test]
    fn every_primitive_winds_counter_clockwise_toward_its_normals() {
        for (name, mesh) in [
            ("box", cuboid(1.0, 2.0, 3.0).unwrap()),
            ("sphere", sphere(1.0, 16).unwrap()),
            ("cylinder", cylinder(0.5, 1.0, 2.0, 16).unwrap()),
            ("cone", cylinder(0.0, 1.0, 2.0, 16).unwrap()),
            ("capsule", capsule(0.5, 1.0, 16).unwrap()),
            ("torus", torus(1.0, 0.25, 16).unwrap()),
            ("plane", plane(2.0, 2.0).unwrap()),
            ("circle", circle(1.0, 16).unwrap()),
        ] {
            assert!(outward(&mesh), "{name} has a face wound against its normal");
            assert_eq!(mesh.positions.len(), mesh.normals.len(), "{name}");
            assert_eq!(mesh.positions.len(), mesh.uvs.len(), "{name}");
            assert!(
                mesh.indices
                    .iter()
                    .all(|&i| (i as usize) < mesh.positions.len()),
                "{name}"
            );
        }
    }

    #[test]
    fn control_a_non_positive_size_is_refused() {
        assert!(cuboid(0.0, 1.0, 1.0).is_err());
        assert!(sphere(-1.0, 8).is_err());
        assert!(cylinder(0.0, 0.0, 1.0, 8).is_err());
    }

    #[test]
    fn a_polyhedron_face_past_its_vertices_or_of_zero_area_is_refused() {
        let tri = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        assert!(polyhedron(tri.clone(), &[[0, 1, 2]], None).is_ok());
        assert!(polyhedron(tri.clone(), &[[0, 1, 3]], None).is_err());
        assert!(polyhedron(tri, &[[0, 1, 1]], None).is_err());
    }
}
