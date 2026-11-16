// three r160's CubeUV atlas (cube_uv_reflection_fragment) for the PMREM of a 256 cube: a 768 x 1024 texture whose
// rows are laid out as WebGL lays them (row 0 first), so the atlas coordinates below address the same texels.
// The including shader declares `cube_uv_map: texture_2d<f32>` and `cube_uv_sampler: sampler` (linear, clamped).

const CUBE_UV_MIN_MIP_LEVEL: f32 = 4.0;
const CUBE_UV_MIN_TILE_SIZE: f32 = 16.0;
const CUBE_UV_MAX_MIP: f32 = 8.0;
const CUBE_UV_TEXEL_WIDTH: f32 = 1.0 / 768.0;
const CUBE_UV_TEXEL_HEIGHT: f32 = 1.0 / 1024.0;

fn cube_uv_face(direction: vec3<f32>) -> f32 {
  let a = abs(direction);
  var face = -1.0;
  if (a.x > a.z) {
    if (a.x > a.y) {
      face = select(3.0, 0.0, direction.x > 0.0);
    } else {
      face = select(4.0, 1.0, direction.y > 0.0);
    }
  } else {
    if (a.z > a.y) {
      face = select(5.0, 2.0, direction.z > 0.0);
    } else {
      face = select(4.0, 1.0, direction.y > 0.0);
    }
  }
  return face;
}

fn cube_uv_face_uv(direction: vec3<f32>, face: f32) -> vec2<f32> {
  var uv: vec2<f32>;
  if (face == 0.0) {
    uv = vec2<f32>(direction.z, direction.y) / abs(direction.x);
  } else if (face == 1.0) {
    uv = vec2<f32>(-direction.x, -direction.z) / abs(direction.y);
  } else if (face == 2.0) {
    uv = vec2<f32>(-direction.x, direction.y) / abs(direction.z);
  } else if (face == 3.0) {
    uv = vec2<f32>(-direction.z, direction.y) / abs(direction.x);
  } else if (face == 4.0) {
    uv = vec2<f32>(-direction.x, direction.z) / abs(direction.y);
  } else {
    uv = vec2<f32>(direction.x, direction.y) / abs(direction.z);
  }
  return 0.5 * (uv + 1.0);
}

fn cube_uv_bilinear(direction: vec3<f32>, mip_int_in: f32) -> vec3<f32> {
  var face = cube_uv_face(direction);
  let filter_int = max(CUBE_UV_MIN_MIP_LEVEL - mip_int_in, 0.0);
  let mip_int = max(mip_int_in, CUBE_UV_MIN_MIP_LEVEL);
  let face_size = exp2(mip_int);
  var uv = cube_uv_face_uv(direction, face) * (face_size - 2.0) + 1.0;
  if (face > 2.0) {
    uv.y += face_size;
    face -= 3.0;
  }
  uv.x += face * face_size;
  uv.x += filter_int * 3.0 * CUBE_UV_MIN_TILE_SIZE;
  uv.y += 4.0 * (exp2(CUBE_UV_MAX_MIP) - face_size);
  uv.x *= CUBE_UV_TEXEL_WIDTH;
  uv.y *= CUBE_UV_TEXEL_HEIGHT;
  return textureSampleLevel(cube_uv_map, cube_uv_sampler, uv, 0.0).rgb;
}

const CUBE_UV_R0: f32 = 1.0;
const CUBE_UV_M0: f32 = -2.0;
const CUBE_UV_R1: f32 = 0.8;
const CUBE_UV_M1: f32 = -1.0;
const CUBE_UV_R4: f32 = 0.4;
const CUBE_UV_M4: f32 = 2.0;
const CUBE_UV_R5: f32 = 0.305;
const CUBE_UV_M5: f32 = 3.0;
const CUBE_UV_R6: f32 = 0.21;
const CUBE_UV_M6: f32 = 4.0;

fn cube_uv_roughness_to_mip(roughness: f32) -> f32 {
  var mip = 0.0;
  if (roughness >= CUBE_UV_R1) {
    mip = (CUBE_UV_R0 - roughness) * (CUBE_UV_M1 - CUBE_UV_M0) / (CUBE_UV_R0 - CUBE_UV_R1) + CUBE_UV_M0;
  } else if (roughness >= CUBE_UV_R4) {
    mip = (CUBE_UV_R1 - roughness) * (CUBE_UV_M4 - CUBE_UV_M1) / (CUBE_UV_R1 - CUBE_UV_R4) + CUBE_UV_M1;
  } else if (roughness >= CUBE_UV_R5) {
    mip = (CUBE_UV_R4 - roughness) * (CUBE_UV_M5 - CUBE_UV_M4) / (CUBE_UV_R4 - CUBE_UV_R5) + CUBE_UV_M4;
  } else if (roughness >= CUBE_UV_R6) {
    mip = (CUBE_UV_R5 - roughness) * (CUBE_UV_M6 - CUBE_UV_M5) / (CUBE_UV_R5 - CUBE_UV_R6) + CUBE_UV_M5;
  } else {
    mip = -2.0 * log2(1.16 * roughness);
  }
  return mip;
}

fn cube_uv_texture(direction: vec3<f32>, roughness: f32) -> vec3<f32> {
  let mip = clamp(cube_uv_roughness_to_mip(roughness), CUBE_UV_M0, CUBE_UV_MAX_MIP);
  let mip_f = fract(mip);
  let mip_int = floor(mip);
  let color0 = cube_uv_bilinear(direction, mip_int);
  if (mip_f == 0.0) {
    return color0;
  }
  let color1 = cube_uv_bilinear(direction, mip_int + 1.0);
  return mix(color0, color1, mip_f);
}
