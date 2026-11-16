// A shader material's fragment stage: appended to the lit source and to the program's own WGSL, which declares
// `fn shade(px: Pixel) -> vec4<f32>` (linear colour, alpha). The program's code may also read `frame` (the camera
// and the lights) from the prelude.

struct Pixel {
  // where the pixel is in the world, and in its mesh's own space
  world: vec3<f32>,
  local: vec3<f32>,
  // the surface normal in the world, turned to the viewer on a back face
  normal: vec3<f32>,
  // unit vector from the pixel to the camera
  view: vec3<f32>,
  uv: vec2<f32>,
  // seconds on the canvas clock
  time: f32,
  // seconds from the material's `anchor` (epoch ms on the page wall clock) to this frame, 0 without an anchor
  since: f32,
  // the material's colour and opacity, its emissive times its intensity, and its two parameter vectors
  color: vec4<f32>,
  emissive: vec3<f32>,
  a: vec4<f32>,
  b: vec4<f32>,
};

@fragment
fn fs_surface(in: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
  var px: Pixel;
  px.world = in.world;
  px.local = in.local;
  let n = normalize(in.normal);
  px.normal = select(-n, n, front);
  px.view = normalize(frame.camera_pos.xyz - in.world);
  px.uv = in.uv;
  px.time = frame.clock.x;
  px.since = object.physical.w;
  px.color = object.color;
  px.emissive = object.emissive.rgb;
  px.a = object.custom_a;
  px.b = object.custom_b;
  let c = shade(px);
  return finish(c.rgb, c.a);
}
