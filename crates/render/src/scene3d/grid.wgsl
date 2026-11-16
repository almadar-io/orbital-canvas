// drei's Grid, ported line for line (appended to prelude.wgsl).
struct GridParams {
  center: vec4<f32>,
  // x, y: size; z: cell size; w: section size
  size: vec4<f32>,
  // x: cell thickness, y: section thickness, z: fade distance, w: fade strength
  lines: vec4<f32>,
  cell_color: vec4<f32>,
  section_color: vec4<f32>,
  // xyz: the camera projected onto the grid plane, w: fade from
  camera_on_plane: vec4<f32>,
};

@group(1) @binding(0) var<uniform> grid: GridParams;

struct GridOut {
  @builtin(position) clip: vec4<f32>,
  @location(0) local: vec3<f32>,
  @location(1) world: vec3<f32>,
};

@vertex
fn vs_grid(@builtin(vertex_index) index: u32) -> GridOut {
  var corners = array<vec2<f32>, 6>(
    vec2<f32>(-0.5, -0.5), vec2<f32>(-0.5, 0.5), vec2<f32>(0.5, 0.5),
    vec2<f32>(-0.5, -0.5), vec2<f32>(0.5, 0.5), vec2<f32>(0.5, -0.5));
  let c = corners[index] * grid.size.xy;
  let local = vec3<f32>(c.x, 0.0, c.y);
  let world = grid.center.xyz + local;
  var out: GridOut;
  out.clip = frame.view_proj * vec4<f32>(world, 1.0);
  out.local = local;
  out.world = world;
  return out;
}

fn grid_lines(local: vec3<f32>, size: f32, thickness: f32) -> f32 {
  let r = local.xz / size;
  let g = abs(fract(r - 0.5) - 0.5) / fwidth(r);
  let line = min(g.x, g.y) + 1.0 - thickness;
  return 1.0 - min(line, 1.0);
}

@fragment
fn fs_grid(in: GridOut) -> @location(0) vec4<f32> {
  let g1 = grid_lines(in.local, grid.size.z, grid.lines.x);
  let g2 = grid_lines(in.local, grid.size.w, grid.lines.y);
  let fade_origin = grid.camera_on_plane.xyz * grid.camera_on_plane.w;
  let dist = distance(fade_origin, in.world);
  let d = 1.0 - min(dist / grid.lines.z, 1.0);
  let color = mix(grid.cell_color.rgb, grid.section_color.rgb, min(1.0, grid.lines.y * g2));
  var alpha = (g1 + g2) * pow(d, grid.lines.w);
  alpha = mix(0.75 * alpha, alpha, g2);
  if (alpha <= 0.0) {
    discard;
  }
  return finish(color, alpha);
}
