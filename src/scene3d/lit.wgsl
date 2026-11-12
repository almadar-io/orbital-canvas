// The lit 3D pass and its shadow pass (appended to prelude.wgsl).

struct Object {
  model: mat4x4<f32>,
  normal_model: mat4x4<f32>,
  color: vec4<f32>,
  emissive: vec4<f32>,
  // x: metalness, y: roughness, z: kind (0 basic, 1 standard, 2 physical, 3 toon, 4 flat), w: flags
  params: vec4<f32>,
  // x: transmission, y: ior, z: thickness, w: a shader material's `since` (seconds from its anchor)
  physical: vec4<f32>,
  // with FLAG_QUAD, the unit quad's local rect (min x, min y, width, height) and the map rect its UVs span
  quad: vec4<f32>,
  uv_rect: vec4<f32>,
  // a shader material's two parameter vectors
  custom_a: vec4<f32>,
  custom_b: vec4<f32>,
};

const FLAG_FLAT: u32 = 1u;
const FLAG_RECEIVE_SHADOW: u32 = 2u;
const FLAG_MAP: u32 = 4u;
const FLAG_CUTOUT: u32 = 8u;
const FLAG_BILLBOARD: u32 = 16u;
const FLAG_QUAD: u32 = 32u;
const ALPHA_TEST: f32 = 0.1;

@group(0) @binding(1) var shadow_map: texture_depth_2d;
@group(0) @binding(2) var shadow_sampler: sampler_comparison;
@group(1) @binding(0) var<uniform> object: Object;
@group(2) @binding(0) var color_map: texture_2d<f32>;
@group(2) @binding(1) var color_sampler: sampler;

struct VertexIn {
  @location(0) position: vec3<f32>,
  @location(1) normal: vec3<f32>,
  @location(2) uv: vec2<f32>,
};

struct VertexOut {
  @builtin(position) clip: vec4<f32>,
  @location(0) world: vec3<f32>,
  @location(1) normal: vec3<f32>,
  @location(2) uv: vec2<f32>,
  @location(3) shadow_coord: vec4<f32>,
  @location(4) local: vec3<f32>,
};

// drei's Billboard: the camera's rotation with the object's own origin and scale
fn world_position(position: vec3<f32>) -> vec4<f32> {
  if ((u32(object.params.w) & FLAG_BILLBOARD) == 0u) {
    return object.model * vec4<f32>(position, 1.0);
  }
  let origin = object.model[3].xyz;
  let sx = length(object.model[0].xyz);
  let sy = length(object.model[1].xyz);
  return vec4<f32>(origin + frame.camera_right.xyz * (position.x * sx) + frame.camera_up.xyz * (position.y * sy), 1.0);
}

fn world_normal(normal: vec3<f32>) -> vec3<f32> {
  if ((u32(object.params.w) & FLAG_BILLBOARD) == 0u) {
    return normalize((object.normal_model * vec4<f32>(normal, 0.0)).xyz);
  }
  return normalize(cross(frame.camera_right.xyz, frame.camera_up.xyz));
}

fn local_position(position: vec3<f32>) -> vec3<f32> {
  if ((u32(object.params.w) & FLAG_QUAD) == 0u) {
    return position;
  }
  return vec3<f32>(object.quad.xy + position.xy * object.quad.zw, 0.0);
}

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
  let local = local_position(in.position);
  let world = world_position(local);
  let normal = world_normal(in.normal);
  var out: VertexOut;
  out.local = local;
  out.clip = frame.view_proj * world;
  out.world = world.xyz;
  out.normal = normal;
  out.uv = select(in.uv, object.uv_rect.xy + in.uv * object.uv_rect.zw, (u32(object.params.w) & FLAG_QUAD) != 0u);
  out.shadow_coord = frame.light_view_proj * vec4<f32>(world.xyz + normal * frame.shadow.y, 1.0);
  return out;
}

@vertex
fn vs_shadow(in: VertexIn) -> @builtin(position) vec4<f32> {
  return frame.light_view_proj * world_position(in.position);
}

fn shadow_factor(coord: vec4<f32>) -> f32 {
  if (frame.shadow.w < 0.5) {
    return 1.0;
  }
  let ndc = coord.xyz / coord.w;
  let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
  let depth = ndc.z + frame.shadow.x;
  if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || depth > 1.0) {
    return 1.0;
  }
  let texel = frame.shadow.z;
  var lit = 0.0;
  for (var y = -1; y <= 1; y = y + 1) {
    for (var x = -1; x <= 1; x = x + 1) {
      lit = lit + textureSampleCompareLevel(shadow_map, shadow_sampler, uv + vec2<f32>(f32(x), f32(y)) * texel, depth);
    }
  }
  return lit / 9.0;
}

fn f_schlick(f0: vec3<f32>, dot_vh: f32) -> vec3<f32> {
  let fresnel = exp2((-5.55473 * dot_vh - 6.98316) * dot_vh);
  return f0 * (1.0 - fresnel) + vec3<f32>(fresnel);
}

fn v_ggx_smith_correlated(alpha: f32, dot_nl: f32, dot_nv: f32) -> f32 {
  let a2 = alpha * alpha;
  let gv = dot_nl * sqrt(a2 + (1.0 - a2) * dot_nv * dot_nv);
  let gl = dot_nv * sqrt(a2 + (1.0 - a2) * dot_nl * dot_nl);
  return 0.5 / max(gv + gl, 1e-6);
}

fn d_ggx(alpha: f32, dot_nh: f32) -> f32 {
  let a2 = alpha * alpha;
  let denom = dot_nh * dot_nh * (a2 - 1.0) + 1.0;
  return (1.0 / PI) * a2 / (denom * denom);
}

struct Surface {
  diffuse: vec3<f32>,
  specular: vec3<f32>,
  roughness: f32,
  toon: bool,
};

struct Reflected {
  diffuse: vec3<f32>,
  specular: vec3<f32>,
};

fn direct(s: Surface, n: vec3<f32>, v: vec3<f32>, l: vec3<f32>, radiance: vec3<f32>) -> Reflected {
  var out: Reflected;
  let dot_nl = clamp(dot(n, l), 0.0, 1.0);
  if (s.toon) {
    let coord = dot(n, l) * 0.5 + 0.5;
    let ramp = select(1.0, 0.7, coord < 0.7);
    out.diffuse = radiance * ramp * s.diffuse / PI;
    out.specular = vec3<f32>(0.0);
    return out;
  }
  let irradiance = radiance * dot_nl;
  let h = normalize(l + v);
  let dot_nv = clamp(dot(n, v), 1e-4, 1.0);
  let dot_nh = clamp(dot(n, h), 0.0, 1.0);
  let dot_vh = clamp(dot(v, h), 0.0, 1.0);
  let alpha = s.roughness * s.roughness;
  out.diffuse = irradiance * s.diffuse / PI;
  out.specular = irradiance * f_schlick(s.specular, dot_vh) * v_ggx_smith_correlated(alpha, dot_nl, dot_nv) * d_ggx(alpha, dot_nh);
  return out;
}

@fragment
fn fs_main(in: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
  let flags = u32(object.params.w);
  let flat_shaded = (flags & FLAG_FLAT) != 0u;
  let texel = textureSample(color_map, color_sampler, in.uv);
  // derivatives before the cutout's discard, which leaves the rest of the shader in non-uniform control flow
  let face = cross(dpdx(in.world), dpdy(in.world));
  let roughness = ibl_material_roughness(object.params.y, select(normalize(in.normal), normalize(face), flat_shaded));
  let base = object.color * select(vec4<f32>(1.0), texel, (flags & FLAG_MAP) != 0u);
  let kind = u32(object.params.z);
  if (kind == 0u || kind == 4u) {
    if ((flags & FLAG_CUTOUT) != 0u && base.a < ALPHA_TEST) {
      discard;
    }
    return finish(base.rgb, base.a);
  }
  var n = normalize(in.normal);
  if (flat_shaded) {
    n = normalize(face);
  }
  let v = normalize(frame.camera_pos.xyz - in.world);
  if (flat_shaded) {
    if (dot(n, v) < 0.0) { n = -n; }
  } else if (!front) {
    n = -n;
  }
  let metalness = clamp(object.params.x, 0.0, 1.0);
  let physical = kind == 2u;
  let specular = ibl_specular(base.rgb, metalness, object.physical.y, physical);
  var s: Surface;
  s.diffuse = base.rgb * (1.0 - metalness);
  s.specular = specular.rgb;
  s.roughness = roughness;
  s.toon = kind == 3u;
  if (s.toon) {
    s.diffuse = base.rgb;
  }

  var diffuse = vec3<f32>(0.0);
  var spec = vec3<f32>(0.0);
  // indirect: ambient and hemisphere irradiance over a Lambert surface
  var indirect = frame.ambient.rgb;
  if (frame.hemi_sky.w > 0.5) {
    let weight = 0.5 * n.y + 0.5;
    indirect = indirect + mix(frame.hemi_ground.rgb, frame.hemi_sky.rgb, weight);
  }
  diffuse = diffuse + indirect * s.diffuse / PI;
  // three gives scene.environment to MeshStandardMaterial and its subclass alone
  if ((kind == 1u || physical) && frame.options.w > 0.5) {
    let ibl = ibl_indirect(n, v, s.diffuse, specular.rgb, specular.w, s.roughness);
    diffuse = diffuse + ibl.diffuse;
    spec = spec + ibl.specular;
  }

  if (frame.dir_to_light.w > 0.5) {
    var visible = 1.0;
    if ((flags & FLAG_RECEIVE_SHADOW) != 0u) {
      visible = shadow_factor(in.shadow_coord);
    }
    let r = direct(s, n, v, normalize(frame.dir_to_light.xyz), frame.dir_color.rgb);
    diffuse = diffuse + visible * r.diffuse;
    spec = spec + visible * r.specular;
  }
  let count = u32(frame.options.x);
  for (var i = 0u; i < MAX_POINTS; i = i + 1u) {
    if (i >= count) { break; }
    let to_light = frame.point_pos[i].xyz - in.world;
    let distance = length(to_light);
    let cutoff = frame.point_pos[i].w;
    let decay = frame.point_color[i].w;
    var falloff = 1.0 / max(pow(distance, decay), 0.01);
    if (cutoff > 0.0) {
      let q = distance / cutoff;
      let t = clamp(1.0 - q * q * q * q, 0.0, 1.0);
      falloff = falloff * t * t;
    }
    let r = direct(s, n, v, to_light / max(distance, 1e-6), frame.point_color[i].rgb * falloff);
    diffuse = diffuse + r.diffuse;
    spec = spec + r.specular;
  }
  var alpha = base.a;
  let transmission = object.physical.x;
  if (physical && transmission > 0.0) {
    let t = transmission_volume_refraction(n, v, s.roughness, s.diffuse, specular.rgb, specular.w, in.world,
      object.model, frame.view_proj, object.physical.y, object.physical.z, vec3<f32>(1.0), 0.0, false);
    diffuse = mix(diffuse, t.rgb, transmission);
    alpha = base.a * mix(1.0, t.a, transmission);
  }
  return finish(diffuse + spec + object.emissive.rgb, alpha);
}
