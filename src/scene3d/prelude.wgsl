// Shared by every scene3d shader: the frame uniform, three.js's ACES fit and the sRGB encode. The lit pass follows three.js r160's physically based model (Lambert diffuse over
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
  // x: point count, y: 1 for ACES tone mapping, z: 1 to encode sRGB in the shader, w: 1 when an environment is bound
  options: vec4<f32>,
  // the camera's world-space right and up axes, for billboards
  camera_right: vec4<f32>,
  camera_up: vec4<f32>,
  // x: seconds on the canvas clock
  clock: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;

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

