// postprocessing 6.39 as @react-three/postprocessing 3.0.4 mounts it for Effects3D: BloomEffect's LuminancePass
// (threshold and smoothing, color output) and MipmapBlurPass (8 down and 7 up passes, radius 0.85), then one
// EffectPass that adds the bloom (BlendFunction.ADD), applies VignetteEffect (NORMAL) and encodes sRGB.
// UVs run top-down here and bottom-up in WebGL; every kernel below is symmetric, so rows map to the same texels.

struct Params {
  a: vec4<f32>,
  b: vec4<f32>,
};

@group(0) @binding(0) var input_a: texture_2d<f32>;
@group(0) @binding(1) var input_b: texture_2d<f32>;
@group(0) @binding(2) var linear_clamp: sampler;
@group(0) @binding(3) var<uniform> params: Params;

struct PostOut {
  @builtin(position) clip: vec4<f32>,
  @location(0) uv: vec2<f32>,
};

@vertex
fn vs_post(@builtin(vertex_index) index: u32) -> PostOut {
  let p = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
  var out: PostOut;
  out.clip = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
  out.uv = vec2<f32>(p.x, 1.0 - p.y);
  return out;
}

fn tap_a(uv: vec2<f32>) -> vec4<f32> {
  return textureSampleLevel(input_a, linear_clamp, uv, 0.0);
}

fn tap_b(uv: vec2<f32>) -> vec4<f32> {
  return textureSampleLevel(input_b, linear_clamp, uv, 0.0);
}

// GLSL smoothstep as GPUs evaluate it, defined for any edge order (VignetteEffect passes them high to low).
fn hermite(low: f32, high: f32, x: f32) -> f32 {
  let t = clamp((x - low) / (high - low), 0.0, 1.0);
  return t * t * (3.0 - 2.0 * t);
}

fn luminance(rgb: vec3<f32>) -> f32 {
  return dot(vec3<f32>(0.2126729, 0.7151522, 0.0721750), rgb);
}

// a.x threshold, a.y smoothing
@fragment
fn fs_luminance(in: PostOut) -> @location(0) vec4<f32> {
  let texel = tap_a(in.uv);
  let mask = hermite(params.a.x, params.a.x + params.a.y, luminance(texel.rgb));
  return texel * mask;
}

fn inside(uv: vec2<f32>) -> f32 {
  return select(0.0, 1.0, uv.x >= 0.0 && uv.x <= 1.0 && uv.y >= 0.0 && uv.y <= 1.0);
}

// a.xy: the input's texel size
@fragment
fn fs_downsample(in: PostOut) -> @location(0) vec4<f32> {
  let t = params.a.xy;
  let uv = in.uv;
  let uv00 = uv + t * vec2<f32>(-1.0, 1.0);
  let uv01 = uv + t * vec2<f32>(1.0, 1.0);
  let uv02 = uv + t * vec2<f32>(-1.0, -1.0);
  let uv03 = uv + t * vec2<f32>(1.0, -1.0);
  let uv04 = uv + t * vec2<f32>(-2.0, 2.0);
  let uv05 = uv + t * vec2<f32>(0.0, 2.0);
  let uv06 = uv + t * vec2<f32>(2.0, 2.0);
  let uv07 = uv + t * vec2<f32>(-2.0, 0.0);
  let uv08 = uv + t * vec2<f32>(2.0, 0.0);
  let uv09 = uv + t * vec2<f32>(-2.0, -2.0);
  let uv10 = uv + t * vec2<f32>(0.0, -2.0);
  let uv11 = uv + t * vec2<f32>(2.0, -2.0);
  let inner = 0.125;
  let outer = 0.05556;
  var c = vec4<f32>(0.0);
  c += inner * inside(uv00) * tap_a(uv00);
  c += inner * inside(uv01) * tap_a(uv01);
  c += inner * inside(uv02) * tap_a(uv02);
  c += inner * inside(uv03) * tap_a(uv03);
  c += outer * inside(uv04) * tap_a(uv04);
  c += outer * inside(uv05) * tap_a(uv05);
  c += outer * inside(uv06) * tap_a(uv06);
  c += outer * inside(uv07) * tap_a(uv07);
  c += outer * inside(uv08) * tap_a(uv08);
  c += outer * inside(uv09) * tap_a(uv09);
  c += outer * inside(uv10) * tap_a(uv10);
  c += outer * inside(uv11) * tap_a(uv11);
  c += outer * tap_a(uv);
  return c;
}

// a.xy: the input's texel size, a.z: radius; input_b is the support (the downsampled level of this size)
@fragment
fn fs_upsample(in: PostOut) -> @location(0) vec4<f32> {
  let t = params.a.xy;
  let uv = in.uv;
  var c = vec4<f32>(0.0);
  c += tap_a(uv + t * vec2<f32>(-1.0, 1.0)) * 0.0625;
  c += tap_a(uv + t * vec2<f32>(0.0, 1.0)) * 0.125;
  c += tap_a(uv + t * vec2<f32>(1.0, 1.0)) * 0.0625;
  c += tap_a(uv + t * vec2<f32>(-1.0, 0.0)) * 0.125;
  c += tap_a(uv) * 0.25;
  c += tap_a(uv + t * vec2<f32>(1.0, 0.0)) * 0.125;
  c += tap_a(uv + t * vec2<f32>(-1.0, -1.0)) * 0.0625;
  c += tap_a(uv + t * vec2<f32>(0.0, -1.0)) * 0.125;
  c += tap_a(uv + t * vec2<f32>(1.0, -1.0)) * 0.0625;
  let base = tap_b(uv);
  return mix(base, c, params.a.z);
}

// three's sRGBTransferOETF (r160), unclamped like the shader postprocessing includes.
fn srgb_oetf(rgb: vec3<f32>) -> vec3<f32> {
  let hi = pow(rgb, vec3<f32>(0.41666)) * 1.055 - vec3<f32>(0.055);
  let lo = rgb * 12.92;
  return select(hi, lo, rgb <= vec3<f32>(0.0031308));
}

// a: bloom intensity, bloom on, vignette offset, vignette darkness; b.x: vignette on, b.y: encode sRGB in the shader
@fragment
fn fs_composite(in: PostOut) -> @location(0) vec4<f32> {
  var color0 = tap_a(in.uv);
  if (params.a.y > 0.5) {
    let color1 = tap_b(in.uv) * params.a.x;
    color0 = vec4<f32>(color0.rgb + color1.rgb, max(color0.a, color1.a));
  }
  if (params.b.x > 0.5) {
    let d = distance(in.uv, vec2<f32>(0.5));
    let offset = params.a.z;
    let darkness = params.a.w;
    color0 = vec4<f32>(color0.rgb * hermite(0.8, offset * 0.799, d * (darkness + offset)), color0.a);
  }
  color0.a = clamp(color0.a, 0.0, 1.0);
  if (params.b.y > 0.5) {
    color0 = vec4<f32>(srgb_oetf(color0.rgb), color0.a);
  }
  return color0;
}
