// The glow pass: a mesh's coverage drawn into its atlas tile, then blurred there along one axis per pass.

struct Tile {
  // the tile's local rect: min x, min y, width, height
  rect: vec4<f32>,
  // the tile's atlas texels: x, y, width, height
  texels: vec4<f32>,
  // x: the Gaussian sigma in texels, y: 0 blurs along x, 1 along y
  blur: vec4<f32>,
};

@group(0) @binding(0) var<uniform> tile: Tile;
@group(1) @binding(0) var source: texture_2d<f32>;

@vertex
fn vs_coverage(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
  let t = (position.xy - tile.rect.xy) / tile.rect.zw;
  return vec4<f32>(t * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_coverage() -> @location(0) vec4<f32> {
  return vec4<f32>(1.0);
}

@vertex
fn vs_tile(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
  let corner = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
  return vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_blur(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
  let sigma = tile.blur.x;
  let step = select(vec2<i32>(1, 0), vec2<i32>(0, 1), tile.blur.y > 0.5);
  let lo = vec2<i32>(tile.texels.xy);
  let hi = lo + vec2<i32>(tile.texels.zw) - vec2<i32>(1, 1);
  let p = vec2<i32>(at.xy);
  let reach = i32(ceil(3.0 * sigma));
  var sum = 0.0;
  var total = 0.0;
  for (var k = -reach; k <= reach; k = k + 1) {
    let w = exp(-f32(k * k) / (2.0 * sigma * sigma));
    sum = sum + w * textureLoad(source, clamp(p + step * k, lo, hi), 0).a;
    total = total + w;
  }
  return vec4<f32>(1.0, 1.0, 1.0, sum / total);
}
