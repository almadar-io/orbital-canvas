// The SDF pass prelude: one quad per instance, the shape's signed distance in the fragment stage, and the program's
// `shade` for the fill. Instances live in one uniform array (no vertex attributes, no storage buffers), so the module
// runs on WebGL2. Instance layout, in vec4 slots:
//   0 rect (x y w h)            1 corner radii (tl tr br bl)     2 (shape, corner kind, rotation, opacity)
//   3 (arc start, sweep, thickness, polygon sides)               4 (star ratio, stroke width, dash on, dash off)
//   5 (dash offset, shadow sigma, shadow dx, shadow dy)          6 fill   7 stroke colour   8 shadow colour
//   9 material params 0..4      10 material params 4..8

struct Pixel {
  local: vec2<f32>,
  size: vec2<f32>,
  uv: vec2<f32>,
  sdf: f32,
  time: f32,
  pointer: vec2<f32>,
  color: vec4<f32>,
  a: vec4<f32>,
  b: vec4<f32>,
}

struct SdfParams {
  frame: vec4<f32>,
  pointer: vec4<f32>,
  data: array<vec4<f32>, 704>,
}

@group(0) @binding(0) var<uniform> sdf_params: SdfParams;

const SDF_FIELDS: u32 = 11u;
const SDF_PI: f32 = 3.14159265358979;
const SDF_TAU: f32 = 6.28318530717959;

struct SdfShape {
  d: f32,
  s: f32,
}

struct SdfVsOut {
  @builtin(position) clip: vec4<f32>,
  @location(0) local: vec2<f32>,
  @location(1) @interpolate(flat) index: u32,
}

fn sdf_inst(i: u32, k: u32) -> vec4<f32> {
  return sdf_params.data[i * SDF_FIELDS + k];
}

fn sdf_erf(x: f32) -> f32 {
  let ax = abs(x);
  let t = 1.0 + ax * (0.278393 + ax * (0.230389 + ax * (0.000972 + ax * 0.078108)));
  let t2 = t * t;
  let r = 1.0 - 1.0 / (t2 * t2);
  return select(-r, r, x >= 0.0);
}

fn sdf_corner_radius(p: vec2<f32>, r: vec4<f32>) -> f32 {
  if (p.x < 0.0) {
    if (p.y < 0.0) {
      return r.x;
    }
    return r.w;
  }
  if (p.y < 0.0) {
    return r.y;
  }
  return r.z;
}

fn sdf_rect(p: vec2<f32>, hs: vec2<f32>, radii: vec4<f32>, kind: u32) -> f32 {
  let r = clamp(sdf_corner_radius(p, radii), 0.0, min(hs.x, hs.y));
  let ap = abs(p);
  let q0 = ap - hs;
  let box_d = min(max(q0.x, q0.y), 0.0) + length(max(q0, vec2<f32>(0.0)));
  if (r <= 0.0) {
    return box_d;
  }
  if (kind == 0u) {
    let q = q0 + vec2<f32>(r);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - r;
  }
  if (kind == 1u) {
    return max(max(q0.x, q0.y), (q0.x + q0.y + r) * 0.70710678);
  }
  if (kind == 2u) {
    let half_r = r * 0.5;
    let sq = abs(ap - (hs - vec2<f32>(half_r))) - vec2<f32>(half_r);
    let sd = min(max(sq.x, sq.y), 0.0) + length(max(sq, vec2<f32>(0.0)));
    return max(box_d, -sd);
  }
  return max(box_d, r - length(ap - hs));
}

// Distance along the rectangle's outline, clockwise from the top-left corner.
fn sdf_rect_s(p: vec2<f32>, hs: vec2<f32>) -> f32 {
  let w = hs.x * 2.0;
  let h = hs.y * 2.0;
  if (abs(p.x) * hs.y > abs(p.y) * hs.x) {
    if (p.x > 0.0) {
      return w + clamp(p.y + hs.y, 0.0, h);
    }
    return 2.0 * w + h + clamp(hs.y - p.y, 0.0, h);
  }
  if (p.y < 0.0) {
    return clamp(p.x + hs.x, 0.0, w);
  }
  return w + h + clamp(hs.x - p.x, 0.0, w);
}

fn sdf_capsule(p: vec2<f32>, hs: vec2<f32>) -> f32 {
  let r = min(hs.x, hs.y);
  let q = abs(p) - (hs - vec2<f32>(r));
  return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - r;
}

fn sdf_ellipse(p: vec2<f32>, hs: vec2<f32>) -> SdfShape {
  let h = max(hs, vec2<f32>(0.0001));
  let k0 = length(p / h);
  let k1 = length(p / (h * h));
  var d = -min(h.x, h.y);
  if (k1 > 0.000000001) {
    d = k0 * (k0 - 1.0) / k1;
  }
  let a = h.x;
  let b = h.y;
  let perimeter = SDF_PI * (3.0 * (a + b) - sqrt((3.0 * a + b) * (a + 3.0 * b)));
  let s = (atan2(p.y * a, p.x * b) + SDF_PI) / SDF_TAU * perimeter;
  return SdfShape(d, s);
}

fn sdf_arc(p: vec2<f32>, hs: vec2<f32>, arc: vec3<f32>) -> SdfShape {
  let big_r = min(hs.x, hs.y);
  let t = clamp(arc.z, 0.0, big_r);
  let rc = big_r - t * 0.5;
  let rb = t * 0.5;
  let sweep = arc.y;
  let ap = clamp(abs(sweep) * 0.5, 0.0, SDF_PI);
  let mid = arc.x + sweep * 0.5;
  let turn = SDF_PI * 0.5 - mid;
  let ca = cos(turn);
  let sa = sin(turn);
  var q = vec2<f32>(p.x * ca - p.y * sa, p.x * sa + p.y * ca);
  q.x = abs(q.x);
  let sc = vec2<f32>(sin(ap), cos(ap));
  var d = abs(length(q) - rc) - rb;
  if (sc.y * q.x > sc.x * q.y) {
    d = length(q - sc * rc) - rb;
  }
  var rel = atan2(p.y, p.x) - arc.x;
  if (sweep < 0.0) {
    rel = -rel;
  }
  rel = rel - floor(rel / SDF_TAU) * SDF_TAU;
  return SdfShape(d, rel * rc);
}

fn sdf_poly_vertex(k: u32, m: u32, r: f32, ratio: f32, star: bool) -> vec2<f32> {
  let a = -SDF_PI * 0.5 + f32(k) * SDF_TAU / f32(m);
  var rad = r;
  if (star && (k % 2u) == 1u) {
    rad = r * ratio;
  }
  return vec2<f32>(cos(a), sin(a)) * rad;
}

fn sdf_polygon(p: vec2<f32>, hs: vec2<f32>, poly: vec2<f32>) -> SdfShape {
  let r = min(hs.x, hs.y);
  let n = u32(clamp(floor(poly.x), 3.0, 64.0));
  let star = poly.y < 1.0;
  var m = n;
  if (star) {
    m = n * 2u;
  }
  let v0 = sdf_poly_vertex(0u, m, r, poly.y, star);
  let edge_len = length(sdf_poly_vertex(1u, m, r, poly.y, star) - v0);
  var d = dot(p - v0, p - v0);
  var s = 1.0;
  var best_j = m - 1u;
  var best_t = 0.0;
  var j = m - 1u;
  for (var i = 0u; i < m; i = i + 1u) {
    let vi = sdf_poly_vertex(i, m, r, poly.y, star);
    let vj = sdf_poly_vertex(j, m, r, poly.y, star);
    let e = vj - vi;
    let w = p - vi;
    let t = clamp(dot(w, e) / dot(e, e), 0.0, 1.0);
    let b = w - e * t;
    let dd = dot(b, b);
    if (dd < d) {
      d = dd;
      best_j = j;
      best_t = t;
    }
    // each comparison parenthesised: Tint reads `a < b, c > d` as a template list
    let c = vec3<bool>((p.y >= vi.y), (p.y < vj.y), (e.x * w.y > e.y * w.x));
    if (all(c) || all(!c)) {
      s = -s;
    }
    j = i;
  }
  return SdfShape(s * sqrt(d), (f32(best_j) + 1.0 - best_t) * edge_len);
}

fn sdf_shape(kind: u32, p: vec2<f32>, hs: vec2<f32>, radii: vec4<f32>, corner_kind: u32, arc: vec3<f32>, poly: vec2<f32>) -> SdfShape {
  if (kind == 1u) {
    return SdfShape(sdf_capsule(p, hs), sdf_rect_s(p, hs));
  }
  if (kind == 2u) {
    return sdf_arc(p, hs, arc);
  }
  if (kind == 3u) {
    return sdf_polygon(p, hs, poly);
  }
  if (kind == 4u) {
    return sdf_ellipse(p, hs);
  }
  return SdfShape(sdf_rect(p, hs, radii, corner_kind), sdf_rect_s(p, hs));
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> SdfVsOut {
  let rect = sdf_inst(ii, 0u);
  let misc = sdf_inst(ii, 2u);
  let p4 = sdf_inst(ii, 4u);
  let p5 = sdf_inst(ii, 5u);
  let size = max(rect.zw, vec2<f32>(0.001));
  var margin = 2.0 + p4.y * 0.5;
  if (sdf_inst(ii, 8u).w > 0.0) {
    margin = margin + 3.0 * p5.y + length(p5.zw);
  }
  var corners = array<vec2<f32>, 6>(
    vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
    vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
  );
  let c = corners[vi];
  let local = mix(vec2<f32>(-margin), size + vec2<f32>(margin), c);
  let cs = cos(misc.z);
  let sn = sin(misc.z);
  let d = local - size * 0.5;
  let world = rect.xy + size * 0.5 + vec2<f32>(d.x * cs - d.y * sn, d.x * sn + d.y * cs);
  var out: SdfVsOut;
  out.clip = vec4<f32>(world.x / sdf_params.frame.x * 2.0 - 1.0, 1.0 - world.y / sdf_params.frame.y * 2.0, 0.0, 1.0);
  out.local = local;
  out.index = ii;
  return out;
}

@fragment
fn fs_main(in: SdfVsOut) -> @location(0) vec4<f32> {
  let aa = max(max(length(dpdx(in.local)), length(dpdy(in.local))), 0.0001);
  let i = in.index;
  let rect = sdf_inst(i, 0u);
  let radii = sdf_inst(i, 1u);
  let misc = sdf_inst(i, 2u);
  let arcv = sdf_inst(i, 3u);
  let p4 = sdf_inst(i, 4u);
  let p5 = sdf_inst(i, 5u);
  let fill = sdf_inst(i, 6u);
  let stroke_c = sdf_inst(i, 7u);
  let shadow_c = sdf_inst(i, 8u);
  let size = max(rect.zw, vec2<f32>(0.001));
  let hs = size * 0.5;
  let p = in.local - hs;
  let kind = u32(misc.x + 0.5);
  let corner_kind = u32(misc.y + 0.5);
  let opacity = misc.w;
  let poly = vec2<f32>(arcv.w, p4.x);
  let shape = sdf_shape(kind, p, hs, radii, corner_kind, arcv.xyz, poly);

  var pointer = vec2<f32>(-100000.0);
  if (sdf_params.frame.w > 0.5) {
    let dd = sdf_params.pointer.xy - (rect.xy + hs);
    let cs = cos(misc.z);
    let sn = sin(misc.z);
    pointer = vec2<f32>(dd.x * cs + dd.y * sn, -dd.x * sn + dd.y * cs) + hs;
  }
  let px = Pixel(in.local, size, in.local / size, shape.d, sdf_params.frame.z, pointer, fill, sdf_inst(i, 9u), sdf_inst(i, 10u));
  var base = shade(px);
  let fill_cov = clamp(0.5 - shape.d / aa, 0.0, 1.0);
  let base_a = base.a * fill_cov * opacity;

  var acc = vec4<f32>(0.0);
  if (shadow_c.a > 0.0) {
    let sd = sdf_shape(kind, p - p5.zw, hs, radii, corner_kind, arcv.xyz, poly).d;
    let sigma = max(p5.y, aa * 0.5);
    let cov = 0.5 * (1.0 - sdf_erf(sd / (sigma * 1.41421356)));
    let a = shadow_c.a * cov * opacity;
    acc = vec4<f32>(shadow_c.rgb * a, a);
  }
  acc = vec4<f32>(base.rgb * base_a, base_a) + acc * (1.0 - base_a);

  if (p4.y > 0.0 && stroke_c.a > 0.0) {
    let ds = abs(shape.d) - p4.y * 0.5;
    var cov = clamp(0.5 - ds / aa, 0.0, 1.0);
    let period = p4.z + p4.w;
    if (period > 0.0) {
      let t = shape.s + p5.x;
      let m = t - floor(t / period) * period;
      var along = -min(m - p4.z, period - m);
      if (m <= p4.z) {
        along = min(m, p4.z - m);
      }
      cov = cov * clamp(0.5 + along / aa, 0.0, 1.0);
    }
    let a = stroke_c.a * cov * opacity;
    acc = vec4<f32>(stroke_c.rgb * a, a) + acc * (1.0 - a);
  }

  if (acc.a <= 0.0) {
    return vec4<f32>(0.0);
  }
  return vec4<f32>(acc.rgb / acc.a, acc.a);
}
