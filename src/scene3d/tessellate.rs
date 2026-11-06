//! Flat geometry for the 2D drawables a 3D scene lays out: path flattening with the 2D painter's segment semantics,
//! nonzero fills, butt-capped mitred strokes, three's `RingGeometry`, textured quads, and the normal-offset shell an
//! outline draws around authored triangles. Everything is built in the XY plane facing +Z, as three builds its shape
//! geometries before the caller lays them on the ground.

use super::model::MeshData;
use crate::packet::PathSegment;
use std::f32::consts::{FRAC_PI_2, TAU};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TessError(pub String);

impl std::fmt::Display for TessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

type P = [f32; 2];

/// One flattened subpath; `closed` when the path closed it (a fill closes every subpath regardless).
#[derive(Debug, Clone, PartialEq)]
pub struct Subpath {
    pub points: Vec<P>,
    pub closed: bool,
}

/// The canvas miter limit, which the 2D painter's strokes keep.
const MITER_LIMIT: f32 = 10.0;

fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1]]
}

fn cross(a: P, b: P) -> f32 {
    a[0] * b[1] - a[1] * b[0]
}

fn length(a: P) -> f32 {
    a[0].hypot(a[1])
}

/// Flatten `segments` as `vector::build_path` builds them (an arc joins the current point by a line and runs as
/// quarter-turn cubics; a cubic with no current point starts at its first control point; after a close the next
/// segment starts a new subpath at the closed one's start), each cubic split until it deviates under `tolerance`.
pub fn flatten(segments: &[PathSegment], tolerance: f32) -> Result<Vec<Subpath>, TessError> {
    if !(tolerance.is_finite() && tolerance > 0.0) {
        return Err(TessError(format!(
            "flattening tolerance {tolerance} is not above zero"
        )));
    }
    let mut out: Vec<Subpath> = Vec::new();
    let mut current: Option<Subpath> = None;
    let mut start: Option<P> = None;
    let finite = |p: P| -> Result<P, TessError> {
        if p[0].is_finite() && p[1].is_finite() {
            Ok(p)
        } else {
            Err(TessError("the path holds a non-finite coordinate".into()))
        }
    };
    let begin = |current: &mut Option<Subpath>, out: &mut Vec<Subpath>, p: P| {
        if let Some(done) = current.take() {
            out.push(done);
        }
        *current = Some(Subpath {
            points: vec![p],
            closed: false,
        });
    };
    // the current subpath, reopened at the last close's start when a segment follows a close
    let open = |current: &mut Option<Subpath>, out: &mut Vec<Subpath>, start: Option<P>| {
        if current.as_ref().is_some_and(|s| s.closed) {
            let from = start.unwrap_or([0.0, 0.0]);
            if let Some(done) = current.take() {
                out.push(done);
            }
            *current = Some(Subpath {
                points: vec![from],
                closed: false,
            });
        }
    };
    for segment in segments {
        match *segment {
            PathSegment::MoveTo { x, y } => {
                let p = finite([x, y])?;
                begin(&mut current, &mut out, p);
                start = Some(p);
            }
            PathSegment::LineTo { x, y } => {
                let p = finite([x, y])?;
                open(&mut current, &mut out, start);
                match current.as_mut() {
                    Some(s) => s.points.push(p),
                    None => {
                        begin(&mut current, &mut out, p);
                        start = Some(p);
                    }
                }
            }
            PathSegment::CubicTo {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => {
                let (c1, c2, to) = (finite([x1, y1])?, finite([x2, y2])?, finite([x, y])?);
                open(&mut current, &mut out, start);
                if current.is_none() {
                    begin(&mut current, &mut out, c1);
                    start = Some(c1);
                }
                if let Some(s) = current.as_mut() {
                    let from = *s.points.last().unwrap_or(&c1);
                    cubic(&mut s.points, from, c1, c2, to, tolerance);
                }
            }
            PathSegment::Arc {
                cx,
                cy,
                rx,
                ry,
                start: a0,
                end: a1,
            } => {
                for v in [cx, cy, rx, ry, a0, a1] {
                    if !v.is_finite() {
                        return Err(TessError("an arc holds a non-finite number".into()));
                    }
                }
                let mut sweep = a1 - a0;
                if sweep.abs() >= TAU - 1e-6 {
                    sweep = TAU.copysign(sweep);
                }
                let first = [cx + rx * a0.cos(), cy + ry * a0.sin()];
                open(&mut current, &mut out, start);
                match current.as_mut() {
                    Some(s) => s.points.push(first),
                    None => {
                        begin(&mut current, &mut out, first);
                        start = Some(first);
                    }
                }
                let pieces = (sweep.abs() / FRAC_PI_2).ceil().max(1.0) as usize;
                let da = sweep / pieces as f32;
                let k = 4.0 / 3.0 * (da / 4.0).tan();
                let mut a = a0;
                if let Some(s) = current.as_mut() {
                    for _ in 0..pieces {
                        let b = a + da;
                        let (sa, ca) = a.sin_cos();
                        let (sb, cb) = b.sin_cos();
                        let from = *s.points.last().unwrap_or(&first);
                        cubic(
                            &mut s.points,
                            from,
                            [cx + rx * (ca - k * sa), cy + ry * (sa + k * ca)],
                            [cx + rx * (cb + k * sb), cy + ry * (sb - k * cb)],
                            [cx + rx * cb, cy + ry * sb],
                            tolerance,
                        );
                        a = b;
                    }
                }
            }
            PathSegment::Close => {
                if let Some(s) = current.as_mut() {
                    s.closed = true;
                }
            }
        }
    }
    if let Some(done) = current.take() {
        out.push(done);
    }
    Ok(out)
}

/// Append the cubic from `p0` as line points, in as many equal steps as keep it within `tolerance`.
fn cubic(points: &mut Vec<P>, p0: P, p1: P, p2: P, p3: P, tolerance: f32) {
    let dd = |a: P, b: P, c: P| length([a[0] - 2.0 * b[0] + c[0], a[1] - 2.0 * b[1] + c[1]]);
    let bend = dd(p0, p1, p2).max(dd(p1, p2, p3));
    let steps = ((0.75 * bend / tolerance).sqrt().ceil() as usize).max(1);
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        let u = 1.0 - t;
        let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        points.push([
            a * p0[0] + b * p1[0] + c * p2[0] + d * p3[0],
            a * p0[1] + b * p1[1] + c * p2[1] + d * p3[1],
        ]);
    }
}

/// `points` without consecutive repeats or a closing repeat of the first point.
fn dedup(points: &[P]) -> Vec<P> {
    // points closer than this are one point: a flattened arc ends a float-hair off where it began
    let same = |a: P, b: P| {
        let scale = 1.0 + a[0].abs().max(a[1].abs()).max(b[0].abs()).max(b[1].abs());
        (a[0] - b[0]).abs() <= 1e-5 * scale && (a[1] - b[1]).abs() <= 1e-5 * scale
    };
    let mut out: Vec<P> = Vec::with_capacity(points.len());
    for &p in points {
        if out.last().is_none_or(|&last| !same(last, p)) {
            out.push(p);
        }
    }
    while out.len() > 1 && same(out[0], out[out.len() - 1]) {
        out.pop();
    }
    out
}

fn signed_area(poly: &[P]) -> f32 {
    let mut sum = 0.0;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        sum += a[0] * b[1] - b[0] * a[1];
    }
    sum * 0.5
}

/// Whether segments `ab` and `cd` share any point.
fn segments_meet(a: P, b: P, c: P, d: P) -> bool {
    let o = |p: P, q: P, r: P| {
        let v = cross(sub(q, p), sub(r, p));
        if v > 0.0 {
            1
        } else if v < 0.0 {
            -1
        } else {
            0
        }
    };
    let on = |p: P, q: P, r: P| {
        r[0] >= p[0].min(q[0])
            && r[0] <= p[0].max(q[0])
            && r[1] >= p[1].min(q[1])
            && r[1] <= p[1].max(q[1])
    };
    let (o1, o2, o3, o4) = (o(a, b, c), o(a, b, d), o(c, d, a), o(c, d, b));
    if o1 != o2 && o3 != o4 {
        return true;
    }
    (o1 == 0 && on(a, b, c))
        || (o2 == 0 && on(a, b, d))
        || (o3 == 0 && on(c, d, a))
        || (o4 == 0 && on(c, d, b))
}

fn inside(p: P, poly: &[P]) -> bool {
    let mut hit = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            hit = !hit;
        }
        j = i;
    }
    hit
}

/// Fill `subpaths` (each implicitly closed) by the nonzero rule into triangles in the XY plane facing +Z, UVs equal to
/// the positions as three's `ShapeGeometry` gives them. Subpaths that cross themselves or each other are refused: the
/// nesting of non-crossing subpaths is what decides each region's winding here.
pub fn fill(subpaths: &[Vec<P>]) -> Result<MeshData, TessError> {
    // crossings are checked before degenerate outlines drop, so a bow-tie (zero net area) is refused, not lost
    let polys: Vec<Vec<P>> = subpaths
        .iter()
        .map(|s| dedup(s.as_slice()))
        .filter(|p| p.len() >= 3)
        .collect();
    let edges: Vec<(usize, usize, P, P)> = polys
        .iter()
        .enumerate()
        .flat_map(|(k, p)| (0..p.len()).map(move |i| (k, i, p[i], p[(i + 1) % p.len()])))
        .collect();
    for (x, &(pa, ia, a, b)) in edges.iter().enumerate() {
        for &(pb, ib, c, d) in &edges[x + 1..] {
            if pa == pb {
                let n = polys[pa].len();
                if ib == (ia + 1) % n || ia == (ib + 1) % n {
                    continue;
                }
            }
            if segments_meet(a, b, c, d) {
                return Err(TessError(if pa == pb {
                    format!("subpath {pa} crosses itself")
                } else {
                    format!("subpaths {pa} and {pb} cross")
                }));
            }
        }
    }
    let polys: Vec<Vec<P>> = polys
        .into_iter()
        .filter(|p| signed_area(p).abs() > f32::EPSILON)
        .collect();
    let areas: Vec<f32> = polys.iter().map(|p| signed_area(p)).collect();
    let mut order: Vec<usize> = (0..polys.len()).collect();
    order.sort_by(|&a, &b| areas[b].abs().total_cmp(&areas[a].abs()));
    let mut parent: Vec<Option<usize>> = vec![None; polys.len()];
    let mut winding = vec![0i32; polys.len()];
    for (rank, &j) in order.iter().enumerate() {
        // the smallest earlier (larger) subpath that holds this one
        parent[j] = order[..rank]
            .iter()
            .rev()
            .copied()
            .find(|&i| inside(polys[j][0], &polys[i]));
        let own = if areas[j] > 0.0 { 1 } else { -1 };
        winding[j] = own + parent[j].map_or(0, |p| winding[p]);
    }
    let mut mesh = MeshData::default();
    for (s, outer) in polys.iter().enumerate() {
        if winding[s] == 0 {
            continue;
        }
        let holes: Vec<&Vec<P>> = (0..polys.len())
            .filter(|&c| parent[c] == Some(s))
            .map(|c| &polys[c])
            .collect();
        for tri in triangulate(outer, &holes)? {
            for p in tri {
                mesh.indices.push(mesh.positions.len() as u32);
                mesh.positions.push([p[0], p[1], 0.0]);
                mesh.normals.push([0.0, 0.0, 1.0]);
                mesh.uvs.push(p);
            }
        }
    }
    Ok(mesh)
}

/// Ear-clip `outer` with `holes` bridged in: outer counter-clockwise, each hole clockwise, joined to the nearest
/// outer vertex its rightmost vertex sees.
fn triangulate(outer: &[P], holes: &[&Vec<P>]) -> Result<Vec<[P; 3]>, TessError> {
    let mut ring: Vec<P> = outer.to_vec();
    if signed_area(&ring) < 0.0 {
        ring.reverse();
    }
    let mut pending: Vec<Vec<P>> = holes
        .iter()
        .map(|h| {
            let mut h = h.to_vec();
            if signed_area(&h) > 0.0 {
                h.reverse();
            }
            h
        })
        .collect();
    let right = |h: &Vec<P>| h.iter().map(|p| p[0]).fold(f32::NEG_INFINITY, f32::max);
    pending.sort_by(|a, b| right(b).total_cmp(&right(a)));
    for k in 0..pending.len() {
        let hole = pending[k].clone();
        let h = (0..hole.len())
            .max_by(|&a, &b| hole[a][0].total_cmp(&hole[b][0]))
            .unwrap_or(0);
        let hp = hole[h];
        let blocked = |m: P, ring: &[P]| {
            let crosses = |poly: &[P]| {
                (0..poly.len()).any(|i| {
                    let (c, d) = (poly[i], poly[(i + 1) % poly.len()]);
                    c != m && d != m && c != hp && d != hp && segments_meet(hp, m, c, d)
                })
            };
            crosses(ring) || pending[k..].iter().any(|other| crosses(other.as_slice()))
        };
        let m = (0..ring.len())
            .filter(|&i| !blocked(ring[i], ring.as_slice()))
            .min_by(|&a, &b| length(sub(ring[a], hp)).total_cmp(&length(sub(ring[b], hp))))
            .ok_or_else(|| TessError("a hole sees no vertex of its outline".into()))?;
        let mut merged = Vec::with_capacity(ring.len() + hole.len() + 2);
        merged.extend_from_slice(&ring[..=m]);
        merged.extend(hole[h..].iter().chain(&hole[..=h]));
        merged.extend_from_slice(&ring[m..]);
        ring = merged;
    }
    ear_clip(ring)
}

fn ear_clip(mut ring: Vec<P>) -> Result<Vec<[P; 3]>, TessError> {
    let mut out = Vec::with_capacity(ring.len().saturating_sub(2));
    let eps = f32::EPSILON * 16.0;
    while ring.len() > 3 {
        let n = ring.len();
        let mut clipped = false;
        for i in 0..n {
            let (a, b, c) = (ring[(i + n - 1) % n], ring[i], ring[(i + 1) % n]);
            let turn = cross(sub(b, a), sub(c, b));
            if turn.abs() <= eps * length(sub(b, a)).max(1.0) * length(sub(c, b)).max(1.0) {
                // a straight or doubled-back vertex holds no area
                ring.remove(i);
                clipped = true;
                break;
            }
            if turn < 0.0 {
                continue;
            }
            let blocked = ring.iter().any(|&p| {
                p != a && p != b && p != c && {
                    let (d1, d2, d3) = (
                        cross(sub(b, a), sub(p, a)),
                        cross(sub(c, b), sub(p, b)),
                        cross(sub(a, c), sub(p, c)),
                    );
                    d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0
                }
            });
            if !blocked {
                out.push([a, b, c]);
                ring.remove(i);
                clipped = true;
                break;
            }
        }
        if !clipped {
            return Err(TessError("the outline has no ear left to clip".into()));
        }
    }
    if ring.len() == 3 && cross(sub(ring[1], ring[0]), sub(ring[2], ring[1])).abs() > eps {
        out.push([ring[0], ring[1], ring[2]]);
    }
    Ok(out)
}

/// Stroke `subpaths` `width` wide, centered on the line, with butt caps and mitred joins (bevelled past the canvas
/// miter limit), as the 2D painter strokes its shapes.
pub fn stroke(subpaths: &[Subpath], width: f32) -> Result<MeshData, TessError> {
    if !(width.is_finite() && width > 0.0) {
        return Err(TessError(format!("stroke width {width} is not above zero")));
    }
    let half = width * 0.5;
    let mut mesh = MeshData::default();
    let tri = |mesh: &mut MeshData, ps: [P; 3]| {
        for p in ps {
            mesh.indices.push(mesh.positions.len() as u32);
            mesh.positions.push([p[0], p[1], 0.0]);
            mesh.normals.push([0.0, 0.0, 1.0]);
            mesh.uvs.push(p);
        }
    };
    for s in subpaths {
        let pts = if s.closed {
            dedup(&s.points)
        } else {
            let mut out: Vec<P> = Vec::new();
            for &p in &s.points {
                if out.last() != Some(&p) {
                    out.push(p);
                }
            }
            out
        };
        if pts.len() < 2 {
            continue;
        }
        let n = pts.len();
        let edges = if s.closed { n } else { n - 1 };
        let dir = |i: usize| {
            let (a, b) = (pts[i % n], pts[(i + 1) % n]);
            let d = sub(b, a);
            let l = length(d);
            [d[0] / l, d[1] / l]
        };
        let left = |d: P| [-d[1] * half, d[0] * half];
        for i in 0..edges {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            let o = left(dir(i));
            let (a1, a2) = ([a[0] + o[0], a[1] + o[1]], [a[0] - o[0], a[1] - o[1]]);
            let (b1, b2) = ([b[0] + o[0], b[1] + o[1]], [b[0] - o[0], b[1] - o[1]]);
            tri(&mut mesh, [a1, a2, b2]);
            tri(&mut mesh, [a1, b2, b1]);
        }
        let joins: Vec<usize> = if s.closed {
            (0..n).collect()
        } else {
            (1..n - 1).collect()
        };
        for v in joins {
            let (d0, d1) = (dir((v + n - 1) % n), dir(v));
            let turn = cross(d0, d1);
            if turn.abs() <= f32::EPSILON {
                continue;
            }
            // the outer side of the turn
            let side = if turn > 0.0 { -1.0 } else { 1.0 };
            let (n0, n1) = (left(d0), left(d1));
            let p = pts[v];
            let a = [p[0] + n0[0] * side, p[1] + n0[1] * side];
            let b = [p[0] + n1[0] * side, p[1] + n1[1] * side];
            let sum = [(n0[0] + n1[0]) / half, (n0[1] + n1[1]) / half];
            let sum_len = length(sum);
            if sum_len > f32::EPSILON && 2.0 / sum_len <= MITER_LIMIT {
                let scale = 2.0 * half / (sum_len * sum_len) * side;
                let m = [p[0] + sum[0] * scale, p[1] + sum[1] * scale];
                tri(&mut mesh, [p, a, m]);
                tri(&mut mesh, [p, m, b]);
            } else {
                tri(&mut mesh, [p, a, b]);
            }
        }
    }
    Ok(mesh)
}

/// three's `RingGeometry(inner, outer, segments)` (one ring of faces) with separate X and Y radii, so an elliptical
/// band keeps the construction; a circle passes equal radii.
pub fn ring(inner: P, outer: P, segments: usize) -> Result<MeshData, TessError> {
    for v in [inner[0], inner[1], outer[0], outer[1]] {
        if !(v.is_finite() && v >= 0.0) {
            return Err(TessError(format!("ring radius {v} is negative")));
        }
    }
    if outer[0] <= 0.0 || outer[1] <= 0.0 {
        return Err(TessError(
            "the ring's outer radius is not above zero".into(),
        ));
    }
    let count = segments.max(3);
    let mut mesh = MeshData::default();
    for r in [inner, outer] {
        for i in 0..=count {
            let (sin, cos) = (i as f32 / count as f32 * TAU).sin_cos();
            let p = [r[0] * cos, r[1] * sin];
            mesh.positions.push([p[0], p[1], 0.0]);
            mesh.normals.push([0.0, 0.0, 1.0]);
            mesh.uvs
                .push([(p[0] / outer[0] + 1.0) / 2.0, (p[1] / outer[1] + 1.0) / 2.0]);
        }
    }
    let row = count as u32 + 1;
    for i in 0..count as u32 {
        let (a, b, c, d) = (i, i + row, i + row + 1, i + 1);
        mesh.indices.extend([a, b, d, b, c, d]);
    }
    Ok(mesh)
}

/// three's `CircleGeometry(rx, segments)` stretched to `ry` along Y.
pub fn ellipse(rx: f32, ry: f32, segments: usize) -> Result<MeshData, TessError> {
    let mut mesh = super::geometry::circle(rx, segments).map_err(|e| TessError(e.0))?;
    if !(ry.is_finite() && ry > 0.0) {
        return Err(TessError(format!("radius {ry} is not above zero")));
    }
    let k = ry / rx;
    for p in &mut mesh.positions {
        p[1] *= k;
    }
    Ok(mesh)
}

/// A `width` × `height` quad centered on the origin, facing +Z; `uv` is the image rect it shows, `[u0, v0, u1, v1]`
/// with v down (the image's top row at `v0`), drawn at its top edge.
pub fn quad(width: f32, height: f32, uv: [f32; 4]) -> Result<MeshData, TessError> {
    if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
        return Err(TessError(format!(
            "a {width} x {height} quad is not a positive size"
        )));
    }
    let (x, y) = (width * 0.5, height * 0.5);
    let [u0, v0, u1, v1] = uv;
    Ok(MeshData {
        positions: vec![[-x, y, 0.0], [x, y, 0.0], [-x, -y, 0.0], [x, -y, 0.0]],
        normals: vec![[0.0, 0.0, 1.0]; 4],
        uvs: vec![[u0, v0], [u1, v0], [u0, v1], [u1, v1]],
        indices: vec![0, 2, 1, 2, 3, 1],
    })
}

/// Move every position by `offset` in the mesh's local plane.
pub fn translate(mesh: &mut MeshData, offset: P) {
    for p in &mut mesh.positions {
        p[0] += offset[0];
        p[1] += offset[1];
    }
}

/// Turn every position by `radians` counter-clockwise in the mesh's local plane.
pub fn rotate(mesh: &mut MeshData, radians: f32) {
    let (s, c) = radians.sin_cos();
    for p in &mut mesh.positions {
        let [x, y, z] = *p;
        *p = [x * c - y * s, x * s + y * c, z];
    }
}

/// The outline shell of authored triangles: each vertex pushed `distance` along its (normalized) normal, the
/// triangles unchanged.
pub fn inflate(mesh: &MeshData, distance: f32) -> Result<MeshData, TessError> {
    if mesh.normals.len() != mesh.positions.len() {
        return Err(TessError(format!(
            "{} normals for {} positions",
            mesh.normals.len(),
            mesh.positions.len()
        )));
    }
    let mut out = mesh.clone();
    for (i, (p, n)) in out.positions.iter_mut().zip(&mesh.normals).enumerate() {
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if !(len.is_finite() && len > 0.0) {
            return Err(TessError(format!("vertex {i} has no normal to push along")));
        }
        for k in 0..3 {
            p[k] += n[k] / len * distance;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(mesh: &MeshData) -> f32 {
        mesh.indices
            .chunks(3)
            .map(|t| {
                let p = |i: u32| {
                    let v = mesh.positions[i as usize];
                    [v[0], v[1]]
                };
                (cross(sub(p(t[1]), p(t[0])), sub(p(t[2]), p(t[0]))) * 0.5).abs()
            })
            .sum()
    }

    fn square(x: f32, y: f32, s: f32, ccw: bool) -> Vec<P> {
        let mut p = vec![[x, y], [x + s, y], [x + s, y + s], [x, y + s]];
        if !ccw {
            p.reverse();
        }
        p
    }

    #[test]
    fn a_concave_outline_fills_its_exact_area() {
        let l = vec![
            [0.0, 0.0],
            [2.0, 0.0],
            [2.0, 1.0],
            [1.0, 1.0],
            [1.0, 2.0],
            [0.0, 2.0],
        ];
        let mesh = fill(&[l]).unwrap();
        assert!((area(&mesh) - 3.0).abs() < 1e-5);
        assert_eq!(mesh.indices.len(), 12);
    }

    #[test]
    fn nonzero_cuts_an_opposite_hole_and_control_keeps_a_same_direction_one() {
        let outer = square(0.0, 0.0, 4.0, true);
        let hole = fill(&[outer.clone(), square(1.0, 1.0, 2.0, false)]).unwrap();
        assert!((area(&hole) - 12.0).abs() < 1e-4, "{}", area(&hole));
        let same = fill(&[outer, square(1.0, 1.0, 2.0, true)]).unwrap();
        assert!((area(&same) - 16.0).abs() < 1e-4, "{}", area(&same));
    }

    #[test]
    fn a_crossing_outline_is_refused_and_control_a_simple_one_fills() {
        let bow = vec![[0.0, 0.0], [1.0, 1.0], [1.0, 0.0], [0.0, 1.0]];
        assert!(fill(&[bow]).is_err());
        assert!(fill(&[square(0.0, 0.0, 1.0, false)]).is_ok());
    }

    #[test]
    fn a_closed_arc_flattens_to_a_circle_within_tolerance() {
        let segs = [
            PathSegment::Arc {
                cx: 0.0,
                cy: 0.0,
                rx: 1.0,
                ry: 1.0,
                start: 0.0,
                end: TAU,
            },
            PathSegment::Close,
        ];
        let subs = flatten(&segs, 0.001).unwrap();
        assert_eq!(subs.len(), 1);
        assert!(subs[0].closed);
        for p in &subs[0].points {
            assert!((length(*p) - 1.0).abs() < 0.002, "{p:?}");
        }
        let mesh = fill(&[subs[0].points.clone()]).unwrap();
        assert!((area(&mesh) - std::f32::consts::PI).abs() < 0.01);
    }

    #[test]
    fn a_segment_after_close_starts_at_the_closed_subpaths_start() {
        let segs = [
            PathSegment::MoveTo { x: 0.0, y: 0.0 },
            PathSegment::LineTo { x: 1.0, y: 0.0 },
            PathSegment::Close,
            PathSegment::LineTo { x: 0.0, y: 1.0 },
        ];
        let subs = flatten(&segs, 0.01).unwrap();
        assert_eq!(subs.len(), 2);
        assert_eq!(subs[1].points, vec![[0.0, 0.0], [0.0, 1.0]]);
        assert!(!subs[1].closed);
    }

    #[test]
    fn a_stroke_covers_its_width_along_each_edge_and_mitres_the_corner() {
        let open = Subpath {
            points: vec![[0.0, 0.0], [2.0, 0.0], [2.0, 2.0]],
            closed: false,
        };
        let mesh = stroke(&[open], 0.2).unwrap();
        // two 2 x 0.2 bands (their 0.01 overlap counted in each) and the corner's mitred outer square
        assert!((area(&mesh) - 0.81).abs() < 1e-4, "{}", area(&mesh));
        assert!(
            stroke(&[], 0.0).is_err(),
            "control: a zero width is refused"
        );
    }

    #[test]
    fn inflate_moves_vertices_along_their_normals() {
        let mesh = MeshData {
            positions: vec![[0.0, 0.0, 0.0]],
            normals: vec![[0.0, 1.0, 0.0]],
            uvs: vec![[0.0, 0.0]],
            indices: vec![],
        };
        assert_eq!(
            inflate(&mesh, 0.5).unwrap().positions,
            vec![[0.0, 0.5, 0.0]]
        );
    }
}
