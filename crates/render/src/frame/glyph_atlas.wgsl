// Glyph quads: one instance per glyph, corners from the vertex index, coverage from an R8 atlas page tinted by the
// text colour. No uniforms, no storage buffers: the module runs on WebGL2.
//   rect  = (left, top, right, bottom) in NDC      uv = (u0, v0, u1, v1)

struct Instance {
  @location(0) rect: vec4<f32>,
  @location(1) uv: vec4<f32>,
  @location(2) color: vec4<f32>,
}

struct VsOut {
  @builtin(position) clip: vec4<f32>,
  @location(0) uv: vec2<f32>,
  @location(1) color: vec4<f32>,
}

@group(0) @binding(0) var atlas: texture_2d<f32>;
@group(0) @binding(1) var atlas_sampler: sampler;

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32, inst: Instance) -> VsOut {
  var corners = array<vec2<f32>, 6>(
    vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
    vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
  );
  let c = corners[vertex];
  var out: VsOut;
  out.clip = vec4<f32>(mix(inst.rect.x, inst.rect.z, c.x), mix(inst.rect.y, inst.rect.w, c.y), 0.0, 1.0);
  out.uv = vec2<f32>(mix(inst.uv.x, inst.uv.z, c.x), mix(inst.uv.y, inst.uv.w, c.y));
  out.color = inst.color;
  return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
  let coverage = textureSample(atlas, atlas_sampler, in.uv).r;
  return vec4<f32>(in.color.rgb, in.color.a * coverage);
}
