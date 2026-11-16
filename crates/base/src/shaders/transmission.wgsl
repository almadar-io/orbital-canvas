// three r160's MeshPhysicalMaterial transmission (transmission_pars_fragment, getIBLVolumeRefraction), appended to
// the lit shader after environment.rs's IBL_WGSL (it uses ibl_environment_brdf). The opaque-scene target binds at
// group 0, bindings 5 (a mipmapped half-float texture) and 6 (a trilinear clamped sampler).

@group(0) @binding(5) var transmission_map: texture_2d<f32>;
@group(0) @binding(6) var transmission_sampler: sampler;

fn transmission_w0(a: f32) -> f32 {
  return (1.0 / 6.0) * (a * (a * (-a + 3.0) - 3.0) + 1.0);
}

fn transmission_w1(a: f32) -> f32 {
  return (1.0 / 6.0) * (a * a * (3.0 * a - 6.0) + 4.0);
}

fn transmission_w2(a: f32) -> f32 {
  return (1.0 / 6.0) * (a * (a * (-3.0 * a + 3.0) + 3.0) + 1.0);
}

fn transmission_w3(a: f32) -> f32 {
  return (1.0 / 6.0) * (a * a * a);
}

fn transmission_g0(a: f32) -> f32 {
  return transmission_w0(a) + transmission_w1(a);
}

fn transmission_g1(a: f32) -> f32 {
  return transmission_w2(a) + transmission_w3(a);
}

fn transmission_h0(a: f32) -> f32 {
  return -1.0 + transmission_w1(a) / (transmission_w0(a) + transmission_w1(a));
}

fn transmission_h1(a: f32) -> f32 {
  return 1.0 + transmission_w3(a) / (transmission_w2(a) + transmission_w3(a));
}

fn transmission_tap(uv: vec2<f32>, lod: f32) -> vec4<f32> {
  return textureSampleLevel(transmission_map, transmission_sampler, uv, lod);
}

// Mipped bicubic filtering by N8, as three samples it.
fn transmission_bicubic(uv_in: vec2<f32>, texel_size: vec4<f32>, lod: f32) -> vec4<f32> {
  let uv = uv_in * texel_size.zw + 0.5;
  let iuv = floor(uv);
  let fuv = fract(uv);
  let g0x = transmission_g0(fuv.x);
  let g1x = transmission_g1(fuv.x);
  let h0x = transmission_h0(fuv.x);
  let h1x = transmission_h1(fuv.x);
  let h0y = transmission_h0(fuv.y);
  let h1y = transmission_h1(fuv.y);
  let p0 = (vec2<f32>(iuv.x + h0x, iuv.y + h0y) - 0.5) * texel_size.xy;
  let p1 = (vec2<f32>(iuv.x + h1x, iuv.y + h0y) - 0.5) * texel_size.xy;
  let p2 = (vec2<f32>(iuv.x + h0x, iuv.y + h1y) - 0.5) * texel_size.xy;
  let p3 = (vec2<f32>(iuv.x + h1x, iuv.y + h1y) - 0.5) * texel_size.xy;
  return transmission_g0(fuv.y) * (g0x * transmission_tap(p0, lod) + g1x * transmission_tap(p1, lod))
    + transmission_g1(fuv.y) * (g0x * transmission_tap(p2, lod) + g1x * transmission_tap(p3, lod));
}

// textureSize(sampler, level): level 0 halved per level, at least 1.
fn transmission_level_size(level: f32) -> vec2<f32> {
  let base = vec2<f32>(textureDimensions(transmission_map, 0));
  return max(floor(base / exp2(level)), vec2<f32>(1.0));
}

fn transmission_texture_bicubic(uv: vec2<f32>, lod: f32) -> vec4<f32> {
  let f_lod_size = transmission_level_size(floor(lod));
  let c_lod_size = transmission_level_size(floor(lod + 1.0));
  let f_sample = transmission_bicubic(uv, vec4<f32>(1.0 / f_lod_size, f_lod_size), floor(lod));
  let c_sample = transmission_bicubic(uv, vec4<f32>(1.0 / c_lod_size, c_lod_size), ceil(lod));
  return mix(f_sample, c_sample, fract(lod));
}

fn transmission_volume_ray(n: vec3<f32>, v: vec3<f32>, thickness: f32, ior: f32, model: mat4x4<f32>) -> vec3<f32> {
  let refraction_vector = refract(-v, normalize(n), 1.0 / ior);
  let model_scale = vec3<f32>(length(model[0].xyz), length(model[1].xyz), length(model[2].xyz));
  return normalize(refraction_vector) * thickness * model_scale;
}

fn transmission_ior_roughness(roughness: f32, ior: f32) -> f32 {
  return roughness * clamp(ior * 2.0 - 2.0, 0.0, 1.0);
}

fn transmission_sample(frag_coord: vec2<f32>, roughness: f32, ior: f32) -> vec4<f32> {
  let size = vec2<f32>(textureDimensions(transmission_map, 0));
  let lod = log2(size.x) * transmission_ior_roughness(roughness, ior);
  return transmission_texture_bicubic(frag_coord, lod);
}

// Beer's law; `attenuated` false is three's infinite attenuationDistance (its default).
fn transmission_attenuation(distance: f32, attenuation_color: vec3<f32>, attenuation_distance: f32, attenuated: bool) -> vec3<f32> {
  if (!attenuated) {
    return vec3<f32>(1.0);
  }
  let coefficient = -log(attenuation_color) / attenuation_distance;
  return exp(-coefficient * distance);
}

// getIBLVolumeRefraction, in world space: rgb replaces totalDiffuse by `transmission`, a is the transmitted alpha.
fn transmission_volume_refraction(
  n: vec3<f32>, v: vec3<f32>, roughness: f32, diffuse_color: vec3<f32>, specular_color: vec3<f32>,
  specular_f90: f32, position: vec3<f32>, model: mat4x4<f32>, view_proj: mat4x4<f32>, ior: f32, thickness: f32,
  attenuation_color: vec3<f32>, attenuation_distance: f32, attenuated: bool) -> vec4<f32> {
  let ray = transmission_volume_ray(n, v, thickness, ior, model);
  let exit_point = position + ray;
  let ndc = view_proj * vec4<f32>(exit_point, 1.0);
  let coords = ndc.xy / ndc.w;
  // WebGL's target rows run bottom-up, this one's top-down
  let uv = vec2<f32>(coords.x * 0.5 + 0.5, 0.5 - coords.y * 0.5);
  let transmitted = transmission_sample(uv, roughness, ior);
  let transmittance = diffuse_color * transmission_attenuation(length(ray), attenuation_color, attenuation_distance, attenuated);
  let attenuated_color = transmittance * transmitted.rgb;
  let f = ibl_environment_brdf(n, v, specular_color, specular_f90, roughness);
  let factor = (transmittance.r + transmittance.g + transmittance.b) / 3.0;
  return vec4<f32>((1.0 - f) * attenuated_color, 1.0 - (1.0 - transmitted.a) * factor);
}
