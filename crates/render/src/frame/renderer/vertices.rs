//! Vertices for quads, polygon fans, sprites and textured triangles, in normalized device coordinates.

use super::*;

/// Build the 6 vertices (two triangles) for one sprite command: `src` in
/// pixels → UVs over the sheet; `dst` in pixels → NDC, rotated about its
/// center; `opacity` rides the vertex alpha; flips swap the UV extents.
pub(super) fn sprite_vertices(
    src: &Rect,
    dst: &Rect,
    (sheet_w, sheet_h): (u32, u32),
    rotation: f32,
    opacity: f32,
    (flip_x, flip_y): (bool, bool),
    (target_w, target_h): (u32, u32),
) -> [SpriteVertex; 6] {
    let tw = sheet_w.max(1) as f32;
    let th = sheet_h.max(1) as f32;
    let (mut u0, mut v0) = (src.x / tw, src.y / th);
    let (mut u1, mut v1) = ((src.x + src.w) / tw, (src.y + src.h) / th);
    if flip_x {
        std::mem::swap(&mut u0, &mut u1);
    }
    if flip_y {
        std::mem::swap(&mut v0, &mut v1);
    }
    let uvs = [[u0, v0], [u0, v1], [u1, v0], [u1, v1]];

    let cx = dst.x + dst.w / 2.0;
    let cy = dst.y + dst.h / 2.0;
    let (sin, cos) = rotation.sin_cos();
    let corner = |dx: f32, dy: f32| -> [f32; 2] {
        let px = dst.x + dx - cx;
        let py = dst.y + dy - cy;
        let rx = cx + px * cos - py * sin;
        let ry = cy + px * sin + py * cos;
        to_ndc(rx, ry, target_w, target_h)
    };
    let corners = [
        corner(0.0, 0.0),     // tl
        corner(0.0, dst.h),   // bl
        corner(dst.w, 0.0),   // tr
        corner(dst.w, dst.h), // br
    ];

    let color = [1.0, 1.0, 1.0, opacity.clamp(0.0, 1.0)];
    let v = |pos: [f32; 2], uv: [f32; 2]| SpriteVertex {
        position: pos,
        uv,
        color,
    };
    [
        v(corners[0], uvs[0]),
        v(corners[1], uvs[1]),
        v(corners[2], uvs[2]),
        v(corners[2], uvs[2]),
        v(corners[1], uvs[1]),
        v(corners[3], uvs[3]),
    ]
}

pub(super) fn textured_triangle_vertices(
    points: [Vec2; 3],
    uv: [[f32; 2]; 3],
    uv_in_pixels: bool,
    (sheet_w, sheet_h): (u32, u32),
    opacity: f32,
    (target_w, target_h): (u32, u32),
) -> [SpriteVertex; 3] {
    let scale_u = if uv_in_pixels {
        sheet_w.max(1) as f32
    } else {
        1.0
    };
    let scale_v = if uv_in_pixels {
        sheet_h.max(1) as f32
    } else {
        1.0
    };
    std::array::from_fn(|i| SpriteVertex {
        position: to_ndc(points[i].x, points[i].y, target_w, target_h),
        uv: [uv[i][0] / scale_u, uv[i][1] / scale_v],
        color: [1.0, 1.0, 1.0, opacity.clamp(0.0, 1.0)],
    })
}

/// Convert pixel coordinates to Normalized Device Coordinates (-1..1).
pub(super) fn to_ndc(x: f32, y: f32, width: u32, height: u32) -> [f32; 2] {
    [
        (x / width as f32) * 2.0 - 1.0,
        1.0 - (y / height as f32) * 2.0,
    ]
}

/// Push two triangles covering `rect` into the vertex buffer.
pub(super) fn push_quad(
    vertices: &mut Vec<Vertex>,
    rect: &Rect,
    color: &[f32; 4],
    width: u32,
    height: u32,
) {
    let tl = to_ndc(rect.x, rect.y, width, height);
    let tr = to_ndc(rect.x + rect.w, rect.y, width, height);
    let bl = to_ndc(rect.x, rect.y + rect.h, width, height);
    let br = to_ndc(rect.x + rect.w, rect.y + rect.h, width, height);

    let c = *color;

    // First triangle: tl, bl, tr
    vertices.push(Vertex {
        position: tl,
        color: c,
    });
    vertices.push(Vertex {
        position: bl,
        color: c,
    });
    vertices.push(Vertex {
        position: tr,
        color: c,
    });

    // Second triangle: tr, bl, br
    vertices.push(Vertex {
        position: tr,
        color: c,
    });
    vertices.push(Vertex {
        position: bl,
        color: c,
    });
    vertices.push(Vertex {
        position: br,
        color: c,
    });
}

/// Push a triangle fan (p0, pi, pi+1) for a convex polygon into the vertex buffer.
pub(super) fn push_poly_fan(
    vertices: &mut Vec<Vertex>,
    points: &[Vec2],
    color: &[f32; 4],
    width: u32,
    height: u32,
) {
    if points.len() < 3 {
        return;
    }
    let c = *color;
    let p0 = to_ndc(points[0].x, points[0].y, width, height);
    for i in 1..points.len() - 1 {
        vertices.push(Vertex {
            position: p0,
            color: c,
        });
        vertices.push(Vertex {
            position: to_ndc(points[i].x, points[i].y, width, height),
            color: c,
        });
        vertices.push(Vertex {
            position: to_ndc(points[i + 1].x, points[i + 1].y, width, height),
            color: c,
        });
    }
}
