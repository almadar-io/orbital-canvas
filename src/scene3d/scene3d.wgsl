// The lit 3D pass and its shadow pass. Lighting follows three.js r160's physically based model (Lambert diffuse over
// pi, GGX specular with Schlick Fresnel and correlated Smith visibility, its toon ramp and its ACES fit) so scenes
// keep their look when they move from three.js to this renderer.

const MAX_POINTS: u32 = 8u;
const PI: f32 = 3.141592653589793;

struct Frame {
  view_proj: mat4x4<f32>,
  light_view_proj: mat4x4<f32>,
  camera_pos: vec4<f32>,
  ambient: vec4<f32>,
  hemi_sky: vec4<f32>,
  hemi_ground: vec4<f32>,
  dir_to_light: vec4<f32>,
  dir_color: vec4<f32>,
  shadow: vec4<f32>,
  point_pos: array<vec4<f32>, 8>,
  point_color: array<vec4<f32>, 8>,
  // x: point count, y: 1 for ACES tone mapping, z: 1 to encode sRGB in the shader, w: unused
  options: vec4<f32>,
};

struct Object {
  model: mat4x4<f32>,
  normal_model: mat4x4<f32>,
  color: vec4<f32>,
  emissive: vec4<f32>,
  // x: metalness, y: roughness, z: kind (0 basic, 1 standard, 2 physical, 3 toon), w: flags
  params: vec4<f32>,
};

const FLAG_FLAT: u32 = 1u;
const FLAG_RECEIVE_SHADOW: u32 = 2u;
const FLAG_MAP: u32 = 4u;
const FLAG_BACK_ONLY: u32 = 8u;

@group(0) @binding(0) var<uniform> frame: Frame;
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
};

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
  let world = object.model * vec4<f32>(in.position, 1.0);
  let normal = normalize((object.normal_model * vec4<f32>(in.normal, 0.0)).xyz);
  var out: VertexOut;
  out.clip = frame.view_proj * world;
  out.world = world.xyz;
  out.normal = normal;
  out.uv = in.uv;
  out.shadow_coord = frame.light_view_proj * vec4<f32>(world.xyz + normal * frame.shadow.y, 1.0);
  return out;
}

@vertex
fn vs_shadow(in: VertexIn) -> @builtin(position) vec4<f32> {
  return frame.light_view_proj * (object.model * vec4<f32>(in.position, 1.0));
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

fn direct(s: Surface, n: vec3<f32>, v: vec3<f32>, l: vec3<f32>, radiance: vec3<f32>) -> vec3<f32> {
  let dot_nl = clamp(dot(n, l), 0.0, 1.0);
  if (s.toon) {
    let coord = dot(n, l) * 0.5 + 0.5;
    let ramp = select(1.0, 0.7, coord < 0.7);
    return radiance * ramp * s.diffuse / PI;
  }
  let irradiance = radiance * dot_nl;
  let h = normalize(l + v);
  let dot_nv = clamp(dot(n, v), 1e-4, 1.0);
  let dot_nh = clamp(dot(n, h), 0.0, 1.0);
  let dot_vh = clamp(dot(v, h), 0.0, 1.0);
  let alpha = s.roughness * s.roughness;
  let spec = f_schlick(s.specular, dot_vh) * v_ggx_smith_correlated(alpha, dot_nl, dot_nv) * d_ggx(alpha, dot_nh);
  return irradiance * (s.diffuse / PI + spec);
}

fn aces(color_in: vec3<f32>) -> vec3<f32> {
  let input_mat = mat3x3<f32>(
    vec3<f32>(0.59719, 0.07600, 0.02840),
    vec3<f32>(0.35458, 0.90834, 0.13383),
    vec3<f32>(0.04823, 0.01566, 0.83777));
  let output_mat = mat3x3<f32>(
    vec3<f32>(1.60475, -0.10208, -0.00327),
    vec3<f32>(-0.53108, 1.10813, -0.07276),
    vec3<f32>(-0.07367, -0.00605, 1.07602));
  var color = input_mat * (color_in / 0.6);
  let a = color * (color + 0.0245786) - 0.000090537;
  let b = color * (0.983729 * color + 0.4329510) + 0.238081;
  color = output_mat * (a / b);
  return clamp(color, vec3<f32>(0.0), vec3<f32>(1.0));
}

fn encode_srgb(c: vec3<f32>) -> vec3<f32> {
  let lo = c * 12.92;
  let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
  return select(hi, lo, c <= vec3<f32>(0.0031308));
}

fn finish(color: vec3<f32>, alpha: f32) -> vec4<f32> {
  var c = color;
  if (frame.options.y > 0.5) {
    c = aces(c);
  }
  if (frame.options.z > 0.5) {
    c = encode_srgb(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)));
  }
  return vec4<f32>(c, alpha);
}

@fragment
fn fs_main(in: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
  let flags = u32(object.params.w);
  let texel = textureSample(color_map, color_sampler, in.uv);
  let base = object.color * select(vec4<f32>(1.0), texel, (flags & FLAG_MAP) != 0u);
  let kind = u32(object.params.z);
  if (kind == 0u) {
    return finish(base.rgb, base.a);
  }
  var n = normalize(in.normal);
  if ((flags & FLAG_FLAT) != 0u) {
    n = normalize(cross(dpdx(in.world), dpdy(in.world)));
  }
  let v = normalize(frame.camera_pos.xyz - in.world);
  if ((flags & FLAG_FLAT) != 0u) {
    if (dot(n, v) < 0.0) { n = -n; }
  } else if (!front) {
    n = -n;
  }
  let metalness = clamp(object.params.x, 0.0, 1.0);
  var s: Surface;
  s.diffuse = base.rgb * (1.0 - metalness);
  s.specular = mix(vec3<f32>(0.04), base.rgb, metalness);
  s.roughness = clamp(object.params.y, 0.0525, 1.0);
  s.toon = kind == 3u;
  if (s.toon) {
    s.diffuse = base.rgb;
  }

  var light = vec3<f32>(0.0);
  // indirect: ambient and hemisphere irradiance over a Lambert surface
  var indirect = frame.ambient.rgb;
  if (frame.hemi_sky.w > 0.5) {
    let weight = 0.5 * n.y + 0.5;
    indirect = indirect + mix(frame.hemi_ground.rgb, frame.hemi_sky.rgb, weight);
  }
  light = light + indirect * s.diffuse / PI;

  if (frame.dir_to_light.w > 0.5) {
    var visible = 1.0;
    if ((flags & FLAG_RECEIVE_SHADOW) != 0u) {
      visible = shadow_factor(in.shadow_coord);
    }
    light = light + visible * direct(s, n, v, normalize(frame.dir_to_light.xyz), frame.dir_color.rgb);
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
      let r = distance / cutoff;
      let t = clamp(1.0 - r * r * r * r, 0.0, 1.0);
      falloff = falloff * t * t;
    }
    light = light + direct(s, n, v, to_light / max(distance, 1e-6), frame.point_color[i].rgb * falloff);
  }
  light = light + object.emissive.rgb;
  return finish(light, base.a);
}
