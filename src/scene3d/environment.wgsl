// three r160's PMREMGenerator.fromScene(new RoomEnvironment(), 0.04), appended to cube_uv.wgsl: the six cube faces
// rendered into the atlas (MeshStandardMaterial lit by the room's one point light, MeshBasicMaterial emitters), then
// SphericalGaussianBlur's latitudinal and longitudinal half blurs. Clip y is negated so each viewport fills the same
// atlas rows WebGL fills.

const PI: f32 = 3.141592653589793;

struct Face {
  view_proj: mat4x4<f32>,
  // xyz: position, w: cutoff distance
  light_position: vec4<f32>,
  // rgb: color times intensity, w: decay
  light_color: vec4<f32>,
};

struct RoomObject {
  model: mat4x4<f32>,
  normal_model: mat4x4<f32>,
  // x: 0 basic, 1 standard; y: the basic color (a gray scalar); z: normal sign (-1 for BackSide)
  params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> face: Face;
@group(1) @binding(0) var<uniform> room_object: RoomObject;

struct SceneIn {
  @location(0) position: vec3<f32>,
  @location(1) normal: vec3<f32>,
};

struct SceneOut {
  @builtin(position) clip: vec4<f32>,
  @location(0) world: vec3<f32>,
  @location(1) normal: vec3<f32>,
};

@vertex
fn vs_scene(in: SceneIn) -> SceneOut {
  let world = room_object.model * vec4<f32>(in.position, 1.0);
  var out: SceneOut;
  out.clip = face.view_proj * world;
  out.clip.y = -out.clip.y;
  out.world = world.xyz;
  out.normal = (room_object.normal_model * vec4<f32>(in.normal, 0.0)).xyz * room_object.params.z;
  return out;
}

fn pow2(x: f32) -> f32 {
  return x * x;
}

fn f_schlick(f0: vec3<f32>, f90: f32, dot_vh: f32) -> vec3<f32> {
  let fresnel = exp2((-5.55473 * dot_vh - 6.98316) * dot_vh);
  return f0 * (1.0 - fresnel) + (f90 * fresnel);
}

fn v_ggx_smith_correlated(alpha: f32, dot_nl: f32, dot_nv: f32) -> f32 {
  let a2 = pow2(alpha);
  let gv = dot_nl * sqrt(a2 + (1.0 - a2) * pow2(dot_nv));
  let gl = dot_nv * sqrt(a2 + (1.0 - a2) * pow2(dot_nl));
  return 0.5 / max(gv + gl, 1e-6);
}

fn d_ggx(alpha: f32, dot_nh: f32) -> f32 {
  let a2 = pow2(alpha);
  let denom = pow2(dot_nh) * (a2 - 1.0) + 1.0;
  return (1.0 / PI) * a2 / pow2(denom);
}

fn brdf_ggx(l: vec3<f32>, v: vec3<f32>, n: vec3<f32>, f0: vec3<f32>, f90: f32, roughness: f32) -> vec3<f32> {
  let alpha = pow2(roughness);
  let h = normalize(l + v);
  let dot_nl = clamp(dot(n, l), 0.0, 1.0);
  let dot_nv = clamp(dot(n, v), 0.0, 1.0);
  let dot_nh = clamp(dot(n, h), 0.0, 1.0);
  let dot_vh = clamp(dot(v, h), 0.0, 1.0);
  return f_schlick(f0, f90, dot_vh) * (v_ggx_smith_correlated(alpha, dot_nl, dot_nv) * d_ggx(alpha, dot_nh));
}

@fragment
fn fs_scene(in: SceneOut) -> @location(0) vec4<f32> {
  if (room_object.params.x < 0.5) {
    return vec4<f32>(vec3<f32>(room_object.params.y), 1.0);
  }
  let n = normalize(in.normal);
  let v = normalize(-in.world);
  let to_light = face.light_position.xyz - in.world;
  let distance = length(to_light);
  let l = normalize(to_light);
  var falloff = 1.0 / max(pow(distance, face.light_color.w), 0.01);
  let cutoff = face.light_position.w;
  if (cutoff > 0.0) {
    falloff = falloff * pow2(clamp(1.0 - pow2(pow2(distance / cutoff)), 0.0, 1.0));
  }
  let irradiance = clamp(dot(n, l), 0.0, 1.0) * face.light_color.rgb * falloff;
  // white, metalness 0, roughness 1: MeshStandardMaterial's defaults
  let diffuse = irradiance * (vec3<f32>(1.0) / PI);
  let specular = irradiance * brdf_ggx(l, v, n, vec3<f32>(0.04), 1.0, 1.0);
  return vec4<f32>(diffuse + specular, 1.0);
}

struct Blur {
  weights: array<vec4<f32>, 5>,
  pole_axis: vec4<f32>,
  // x: samples, y: 1 for latitudinal, z: dTheta, w: mipInt
  params: vec4<f32>,
  // x: the output lod's face size
  plane: vec4<f32>,
};

@group(0) @binding(1) var cube_uv_map: texture_2d<f32>;
@group(0) @binding(2) var cube_uv_sampler: sampler;
@group(0) @binding(3) var<uniform> blur: Blur;

struct BlurOut {
  @builtin(position) clip: vec4<f32>,
  @location(0) direction: vec3<f32>,
};

fn output_direction(uv_in: vec2<f32>, face_index: f32) -> vec3<f32> {
  let uv = 2.0 * uv_in - 1.0;
  var d = vec3<f32>(uv, 1.0);
  if (face_index == 0.0) {
    d = d.zyx;
  } else if (face_index == 1.0) {
    d = d.xzy;
    d = vec3<f32>(-d.x, d.y, -d.z);
  } else if (face_index == 2.0) {
    d.x = -d.x;
  } else if (face_index == 3.0) {
    d = d.zyx;
    d = vec3<f32>(-d.x, d.y, -d.z);
  } else if (face_index == 4.0) {
    d = d.xzy;
    d = vec3<f32>(-d.x, -d.y, d.z);
  } else if (face_index == 5.0) {
    d.z = -d.z;
  }
  return d;
}

// PMREMGenerator's lod plane for the output lod: six quads in the 3 x 2 face layout, UVs reaching one texel past
// each face edge (#25071).
@vertex
fn vs_blur(@builtin(vertex_index) index: u32) -> BlurOut {
  let face_index = index / 6u;
  let corner = index % 6u;
  let cx = select(0.0, 1.0, corner == 1u || corner == 2u || corner == 4u);
  let cy = select(0.0, 1.0, corner == 2u || corner == 4u || corner == 5u);
  let x = f32(face_index % 3u) * 2.0 / 3.0 - 1.0;
  let y = select(-1.0, 0.0, face_index > 2u);
  let texel = 1.0 / (blur.plane.x - 2.0);
  let uv = mix(vec2<f32>(-texel), vec2<f32>(1.0 + texel), vec2<f32>(cx, cy));
  var out: BlurOut;
  out.clip = vec4<f32>(x + cx * 2.0 / 3.0, -(y + cy), 0.0, 1.0);
  out.direction = output_direction(uv, f32(face_index));
  return out;
}

fn blur_sample(theta: f32, axis: vec3<f32>, direction: vec3<f32>) -> vec3<f32> {
  let cos_theta = cos(theta);
  let sample_direction = direction * cos_theta
    + cross(axis, direction) * sin(theta)
    + axis * dot(axis, direction) * (1.0 - cos_theta);
  return cube_uv_bilinear(sample_direction, blur.params.w);
}

fn blur_weight(i: i32) -> f32 {
  return blur.weights[i / 4][i % 4];
}

@fragment
fn fs_blur(in: BlurOut) -> @location(0) vec4<f32> {
  let direction = in.direction;
  var axis = select(cross(blur.pole_axis.xyz, direction), blur.pole_axis.xyz, blur.params.y > 0.5);
  if (all(axis == vec3<f32>(0.0))) {
    axis = vec3<f32>(direction.z, 0.0, -direction.x);
  }
  axis = normalize(axis);
  var color = vec3<f32>(0.0);
  color += blur_weight(0) * blur_sample(0.0, axis, direction);
  let samples = i32(blur.params.x);
  for (var i = 1; i < 20; i = i + 1) {
    if (i >= samples) {
      break;
    }
    let theta = blur.params.z * f32(i);
    color += blur_weight(i) * blur_sample(-1.0 * theta, axis, direction);
    color += blur_weight(i) * blur_sample(theta, axis, direction);
  }
  return vec4<f32>(color, 1.0);
}
