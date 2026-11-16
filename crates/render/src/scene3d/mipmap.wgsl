// One mip level from the level above it: a bilinear tap at each destination texel's center, which averages the 2x2
// source texels under it (gl.generateMipmap's box filter; a format with an sRGB view averages in linear light).

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;

struct MipOut {
  @builtin(position) clip: vec4<f32>,
  @location(0) uv: vec2<f32>,
};

@vertex
fn vs_mip(@builtin(vertex_index) index: u32) -> MipOut {
  let p = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
  var out: MipOut;
  out.clip = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
  out.uv = vec2<f32>(p.x, 1.0 - p.y);
  return out;
}

@fragment
fn fs_mip(in: MipOut) -> @location(0) vec4<f32> {
  return textureSampleLevel(source, source_sampler, in.uv, 0.0);
}
