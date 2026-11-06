//! Mesh builders that reproduce three.js r160's geometries vertex for vertex (positions, normals, UVs, index order and
//! each one's local frame), so a scene moved off three.js tessellates and textures the same. Front faces wind
//! counter-clockwise.

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

/// three.js `BoxGeometry(width, height, depth)` with one segment per side: six faces in three's order (+x, -x, +y,
/// -y, +z, -z), each with its own normal and UVs.
pub fn cuboid(width: f32, height: f32, depth: f32) -> Result<MeshData, GeometryError> {
    finite_positive(&[(width, "width"), (height, "height"), (depth, "depth")])?;
    let mut mesh = MeshData::default();
    // (u axis, v axis, w axis, udir, vdir, plane width, plane height, plane depth)
    let sides: [(usize, usize, usize, f32, f32, f32, f32, f32); 6] = [
        (2, 1, 0, -1.0, -1.0, depth, height, width),
        (2, 1, 0, 1.0, -1.0, depth, height, -width),
        (0, 2, 1, 1.0, 1.0, width, depth, height),
        (0, 2, 1, 1.0, -1.0, width, depth, -height),
        (0, 1, 2, 1.0, -1.0, width, height, depth),
        (0, 1, 2, -1.0, -1.0, width, height, -depth),
    ];
    for (u, v, w, udir, vdir, pw, ph, pd) in sides {
        let base = mesh.positions.len() as u32;
        for iy in 0..=1 {
            let y = iy as f32 * ph - ph * 0.5;
            for ix in 0..=1 {
                let x = ix as f32 * pw - pw * 0.5;
                let mut vertex = [0.0f32; 3];
                vertex[u] = x * udir;
                vertex[v] = y * vdir;
                vertex[w] = pd * 0.5;
                let mut normal = [0.0f32; 3];
                normal[w] = if pd > 0.0 { 1.0 } else { -1.0 };
                mesh.positions.push(vertex);
                mesh.normals.push(normal);
                mesh.uvs.push([ix as f32, 1.0 - iy as f32]);
            }
        }
        let (a, b, c, d) = (base, base + 2, base + 3, base + 1);
        mesh.indices.extend([a, b, d, b, c, d]);
    }
    Ok(mesh)
}

/// three.js `SphereGeometry(radius, widthSegments, heightSegments)`.
pub fn sphere(
    radius: f32,
    width_segments: usize,
    height_segments: usize,
) -> Result<MeshData, GeometryError> {
    finite_positive(&[(radius, "radius")])?;
    let (ws, hs) = (width_segments.max(3), height_segments.max(2));
    let mut mesh = MeshData::default();
    let mut grid = Vec::with_capacity(hs + 1);
    for iy in 0..=hs {
        let v = iy as f32 / hs as f32;
        let u_offset = if iy == 0 {
            0.5 / ws as f32
        } else if iy == hs {
            -0.5 / ws as f32
        } else {
            0.0
        };
        let mut row = Vec::with_capacity(ws + 1);
        for ix in 0..=ws {
            let u = ix as f32 / ws as f32;
            let (phi, theta) = (u * TAU, v * PI);
            let p = [
                -radius * phi.cos() * theta.sin(),
                radius * theta.cos(),
                radius * phi.sin() * theta.sin(),
            ];
            let len = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
            row.push(mesh.positions.len() as u32);
            mesh.positions.push(p);
            mesh.normals.push(if len > 0.0 {
                [p[0] / len, p[1] / len, p[2] / len]
            } else {
                [0.0, 0.0, 0.0]
            });
            mesh.uvs.push([u + u_offset, 1.0 - v]);
        }
        grid.push(row);
    }
    for iy in 0..hs {
        for ix in 0..ws {
            let (a, b, c, d) = (
                grid[iy][ix + 1],
                grid[iy][ix],
                grid[iy + 1][ix],
                grid[iy + 1][ix + 1],
            );
            if iy != 0 {
                mesh.indices.extend([a, b, d]);
            }
            if iy != hs - 1 {
                mesh.indices.extend([b, c, d]);
            }
        }
    }
    Ok(mesh)
}

/// three.js `CylinderGeometry(radiusTop, radiusBottom, height, radialSegments)` with one height segment and both caps
/// (a cap of zero radius is skipped, so a zero top radius is three's `ConeGeometry`).
pub fn cylinder(
    radius_top: f32,
    radius_bottom: f32,
    height: f32,
    radial_segments: usize,
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
    let rs = radial_segments.max(3);
    let half = height * 0.5;
    let slope = (radius_bottom - radius_top) / height;
    let mut mesh = MeshData::default();
    let mut rows = Vec::new();
    for y in 0..=1 {
        let v = y as f32;
        let radius = v * (radius_bottom - radius_top) + radius_top;
        let mut row = Vec::new();
        for x in 0..=rs {
            let u = x as f32 / rs as f32;
            let (sin, cos) = (u * TAU).sin_cos();
            let n = [sin, slope, cos];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            row.push(mesh.positions.len() as u32);
            mesh.positions
                .push([radius * sin, -v * height + half, radius * cos]);
            mesh.normals.push([n[0] / len, n[1] / len, n[2] / len]);
            mesh.uvs.push([u, 1.0 - v]);
        }
        rows.push(row);
    }
    for x in 0..rs {
        let (a, b, c, d) = (rows[0][x], rows[1][x], rows[1][x + 1], rows[0][x + 1]);
        mesh.indices.extend([a, b, d, b, c, d]);
    }
    for top in [true, false] {
        let radius = if top { radius_top } else { radius_bottom };
        if radius <= 0.0 {
            continue;
        }
        let sign = if top { 1.0 } else { -1.0 };
        let center_start = mesh.positions.len() as u32;
        for _ in 1..=rs {
            mesh.positions.push([0.0, half * sign, 0.0]);
            mesh.normals.push([0.0, sign, 0.0]);
            mesh.uvs.push([0.5, 0.5]);
        }
        let center_end = mesh.positions.len() as u32;
        for x in 0..=rs {
            let (sin, cos) = (x as f32 / rs as f32 * TAU).sin_cos();
            mesh.positions
                .push([radius * sin, half * sign, radius * cos]);
            mesh.normals.push([0.0, sign, 0.0]);
            mesh.uvs.push([cos * 0.5 + 0.5, sin * 0.5 * sign + 0.5]);
        }
        for x in 0..rs as u32 {
            let (c, i) = (center_start + x, center_end + x);
            if top {
                mesh.indices.extend([i, i + 1, c]);
            } else {
                mesh.indices.extend([i + 1, i, c]);
            }
        }
    }
    Ok(mesh)
}

/// three.js `LatheGeometry(points, segments)`: `points` are (x, y) in the XY plane, revolved around +Y.
fn lathe(points: &[[f32; 2]], segments: usize) -> MeshData {
    let mut normals0 = Vec::with_capacity(points.len());
    let mut prev = [0.0f32; 2];
    for j in 0..points.len() {
        if j == points.len() - 1 {
            normals0.push(prev);
            continue;
        }
        let (dx, dy) = (
            points[j + 1][0] - points[j][0],
            points[j + 1][1] - points[j][1],
        );
        let current = [dy, -dx];
        let mut n = current;
        if j > 0 {
            n = [n[0] + prev[0], n[1] + prev[1]];
        }
        let len = (n[0] * n[0] + n[1] * n[1]).sqrt();
        normals0.push(if len > 0.0 {
            [n[0] / len, n[1] / len]
        } else {
            n
        });
        prev = current;
    }
    let mut mesh = MeshData::default();
    for i in 0..=segments {
        let (sin, cos) = (i as f32 / segments as f32 * TAU).sin_cos();
        for (j, p) in points.iter().enumerate() {
            mesh.positions.push([p[0] * sin, p[1], p[0] * cos]);
            mesh.uvs.push([
                i as f32 / segments as f32,
                j as f32 / (points.len() - 1) as f32,
            ]);
            let n = normals0[j];
            mesh.normals.push([n[0] * sin, n[1], n[0] * cos]);
        }
    }
    let count = points.len() as u32;
    for i in 0..segments as u32 {
        for j in 0..count - 1 {
            let base = j + i * count;
            let (a, b, c, d) = (base, base + count, base + count + 1, base + 1);
            mesh.indices.extend([a, b, d, c, d, b]);
        }
    }
    mesh
}

/// The points of a three.js `EllipseCurve` arc around `center` from `start` to `end` (counter-clockwise), `divisions`
/// segments.
fn arc_points(
    center: [f32; 2],
    radius: f32,
    start: f32,
    end: f32,
    divisions: usize,
) -> Vec<[f32; 2]> {
    let mut delta = end - start;
    while delta < 0.0 {
        delta += TAU;
    }
    while delta > TAU {
        delta -= TAU;
    }
    if delta < f32::EPSILON {
        delta = TAU;
    }
    (0..=divisions)
        .map(|d| {
            let angle = start + d as f32 / divisions as f32 * delta;
            [
                center[0] + radius * angle.cos(),
                center[1] + radius * angle.sin(),
            ]
        })
        .collect()
}

/// three.js r160 `CapsuleGeometry(radius, length, capSegments, radialSegments)`: a lathe of two quarter arcs joined by
/// a line, with consecutive equal points dropped as `Path.getPoints` does.
pub fn capsule(
    radius: f32,
    length: f32,
    cap_segments: usize,
    radial_segments: usize,
) -> Result<MeshData, GeometryError> {
    finite_positive(&[(radius, "radius")])?;
    if !length.is_finite() || length < 0.0 {
        return Err(GeometryError(format!(
            "length must be zero or more, got {length}"
        )));
    }
    let divisions = cap_segments.max(1) * 2;
    let half = length * 0.5;
    let mut points: Vec<[f32; 2]> = Vec::new();
    let mut push = |p: [f32; 2]| {
        if points.last() != Some(&p) {
            points.push(p);
        }
    };
    for p in arc_points([0.0, -half], radius, PI * 1.5, 0.0, divisions) {
        push(p);
    }
    push([radius, half]);
    for p in arc_points([0.0, half], radius, 0.0, PI * 0.5, divisions) {
        push(p);
    }
    Ok(lathe(&points, radial_segments.max(3)))
}

/// three.js `TorusGeometry(radius, tube, radialSegments, tubularSegments)`, around +Z.
pub fn torus(
    radius: f32,
    tube: f32,
    radial_segments: usize,
    tubular_segments: usize,
) -> Result<MeshData, GeometryError> {
    finite_positive(&[(radius, "radius"), (tube, "tube")])?;
    let (radial, tubular) = (radial_segments.max(3), tubular_segments.max(3));
    let mut mesh = MeshData::default();
    for j in 0..=radial {
        for i in 0..=tubular {
            let u = i as f32 / tubular as f32 * TAU;
            let v = j as f32 / radial as f32 * TAU;
            let p = [
                (radius + tube * v.cos()) * u.cos(),
                (radius + tube * v.cos()) * u.sin(),
                tube * v.sin(),
            ];
            let c = [radius * u.cos(), radius * u.sin(), 0.0];
            let n = [p[0] - c[0], p[1] - c[1], p[2] - c[2]];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            mesh.positions.push(p);
            mesh.normals.push([n[0] / len, n[1] / len, n[2] / len]);
            mesh.uvs
                .push([i as f32 / tubular as f32, j as f32 / radial as f32]);
        }
    }
    let row = tubular as u32 + 1;
    for j in 1..=radial as u32 {
        for i in 1..=tubular as u32 {
            let a = row * j + i - 1;
            let b = row * (j - 1) + i - 1;
            let c = row * (j - 1) + i;
            let d = row * j + i;
            mesh.indices.extend([a, b, d, b, c, d]);
        }
    }
    Ok(mesh)
}

/// three.js `PlaneGeometry(width, height)`: in XY, facing +Z.
pub fn plane(width: f32, height: f32) -> Result<MeshData, GeometryError> {
    finite_positive(&[(width, "width"), (height, "height")])?;
    let (x, y) = (width * 0.5, height * 0.5);
    Ok(MeshData {
        positions: vec![[-x, y, 0.0], [x, y, 0.0], [-x, -y, 0.0], [x, -y, 0.0]],
        normals: vec![[0.0, 0.0, 1.0]; 4],
        uvs: vec![[0.0, 1.0], [1.0, 1.0], [0.0, 0.0], [1.0, 0.0]],
        indices: vec![0, 2, 1, 2, 3, 1],
    })
}

/// three.js `CircleGeometry(radius, segments)`: in XY, facing +Z.
pub fn circle(radius: f32, segments: usize) -> Result<MeshData, GeometryError> {
    finite_positive(&[(radius, "radius")])?;
    let count = segments.max(3);
    let mut mesh = MeshData::default();
    mesh.positions.push([0.0, 0.0, 0.0]);
    mesh.normals.push([0.0, 0.0, 1.0]);
    mesh.uvs.push([0.5, 0.5]);
    for s in 0..=count {
        let (sin, cos) = (s as f32 / count as f32 * TAU).sin_cos();
        mesh.positions.push([radius * cos, radius * sin, 0.0]);
        mesh.normals.push([0.0, 0.0, 1.0]);
        mesh.uvs.push([(cos + 1.0) / 2.0, (sin + 1.0) / 2.0]);
    }
    for i in 1..=count as u32 {
        mesh.indices.extend([i, i + 1, 0]);
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

    #[test]
    fn counts_match_three() {
        // three r160: BoxGeometry 24/36, SphereGeometry(1,16,16) 289 vertices and 16*16*6-2*16*3 indices,
        // CylinderGeometry(.5,1,2,16) 34 + 2*(16+17) vertices, CapsuleGeometry(.5,1,8,16) 17*(2*16+1+... ) per lathe rule
        assert_eq!(
            (
                cuboid(1.0, 1.0, 1.0).unwrap().positions.len(),
                cuboid(1.0, 1.0, 1.0).unwrap().indices.len()
            ),
            (24, 36)
        );
        let s = sphere(1.0, 16, 16).unwrap();
        assert_eq!(
            (s.positions.len(), s.indices.len()),
            (17 * 17, 16 * 16 * 6 - 2 * 16 * 3)
        );
        let c = cylinder(0.5, 1.0, 2.0, 16).unwrap();
        assert_eq!(c.positions.len(), 2 * 17 + 2 * (16 + 17));
        let cone = cylinder(0.0, 1.0, 2.0, 16).unwrap();
        assert_eq!(cone.positions.len(), 2 * 17 + (16 + 17));
        let cap = capsule(0.5, 1.0, 4, 8).unwrap();
        // two arcs of 8 divisions (9 points each) and the joining line, with the shared ends dropped: 9 + 9 points
        assert_eq!(cap.positions.len(), 9 * 18);
        let t = torus(1.0, 0.25, 8, 16).unwrap();
        assert_eq!((t.positions.len(), t.indices.len()), (9 * 17, 8 * 16 * 6));
        assert_eq!(circle(1.0, 16).unwrap().positions.len(), 18);
    }

    #[test]
    fn a_sphere_vertex_sits_where_three_puts_it() {
        let s = sphere(2.0, 4, 2).unwrap();
        // iy = 1 (equator), ix = 1: phi = pi/2 -> x = -2 cos(pi/2) = 0, z = 2 sin(pi/2) = 2
        let p = s.positions[5 + 1];
        assert!(
            p[0].abs() < 1e-5 && p[1].abs() < 1e-5 && (p[2] - 2.0).abs() < 1e-5,
            "{p:?}"
        );
        assert_eq!(
            s.uvs[0],
            [0.125, 1.0],
            "the pole's u is offset by half a segment"
        );
    }

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
            ("sphere", sphere(1.0, 16, 16).unwrap()),
            ("cylinder", cylinder(0.5, 1.0, 2.0, 16).unwrap()),
            ("cone", cylinder(0.0, 1.0, 2.0, 16).unwrap()),
            ("capsule", capsule(0.5, 1.0, 8, 16).unwrap()),
            ("torus", torus(1.0, 0.25, 8, 16).unwrap()),
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
        assert!(sphere(-1.0, 8, 8).is_err());
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
