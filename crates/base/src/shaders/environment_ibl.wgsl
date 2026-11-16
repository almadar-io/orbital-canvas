// three r160's image-based lighting for MeshStandardMaterial and MeshPhysicalMaterial when scene.environment is a
// PMREM (envmap_physical_pars_fragment, lights_fragment_maps, RE_IndirectSpecular_Physical), appended with
// cube_uv.wgsl to the lit shader. The atlas binds at group 0, bindings 3 and 4.

@group(0) @binding(3) var cube_uv_map: texture_2d<f32>;
@group(0) @binding(4) var cube_uv_sampler: sampler;

const IBL_ENV_MAP_INTENSITY: f32 = 1.0;

// getIBLIrradiance: world-space normal.
fn ibl_irradiance(normal: vec3<f32>) -> vec3<f32> {
  return PI * cube_uv_texture(normal, 1.0) * IBL_ENV_MAP_INTENSITY;
}

// getIBLRadiance: world-space view direction (surface to eye) and normal.
fn ibl_radiance(view_dir: vec3<f32>, normal: vec3<f32>, roughness: f32) -> vec3<f32> {
  var reflect_vec = reflect(-view_dir, normal);
  reflect_vec = normalize(mix(reflect_vec, normal, roughness * roughness));
  return cube_uv_texture(reflect_vec, roughness) * IBL_ENV_MAP_INTENSITY;
}

fn ibl_dfg_approx(normal: vec3<f32>, view_dir: vec3<f32>, roughness: f32) -> vec2<f32> {
  let dot_nv = clamp(dot(normal, view_dir), 0.0, 1.0);
  let c0 = vec4<f32>(-1.0, -0.0275, -0.572, 0.022);
  let c1 = vec4<f32>(1.0, 0.0425, 1.04, -0.04);
  let r = roughness * c0 + c1;
  let a004 = min(r.x * r.x, exp2(-9.28 * dot_nv)) * r.x + r.y;
  return vec2<f32>(-1.04, 1.04) * a004 + r.zw;
}

// EnvironmentBRDF
fn ibl_environment_brdf(normal: vec3<f32>, view_dir: vec3<f32>, specular_color: vec3<f32>, specular_f90: f32, roughness: f32) -> vec3<f32> {
  let fab = ibl_dfg_approx(normal, view_dir, roughness);
  return specular_color * fab.x + specular_f90 * fab.y;
}

struct IblScattering {
  single: vec3<f32>,
  multi: vec3<f32>,
};

// computeMultiscattering (Fdez-Aguera)
fn ibl_multiscattering(normal: vec3<f32>, view_dir: vec3<f32>, specular_color: vec3<f32>, specular_f90: f32, roughness: f32) -> IblScattering {
  let fab = ibl_dfg_approx(normal, view_dir, roughness);
  let fss_ess = specular_color * fab.x + specular_f90 * fab.y;
  let ess = fab.x + fab.y;
  let ems = 1.0 - ess;
  let f_avg = specular_color + (1.0 - specular_color) * 0.047619;
  let f_ms = fss_ess * f_avg / (1.0 - ems * f_avg);
  return IblScattering(fss_ess, f_ms * ems);
}

struct IblLight {
  diffuse: vec3<f32>,
  specular: vec3<f32>,
};

// What the environment adds to reflectedLight.indirectDiffuse and .indirectSpecular. Ambient and hemisphere
// irradiance stay on RE_IndirectDiffuse (irradiance * diffuse / PI) as before; without an environment three adds 0.
fn ibl_indirect(normal: vec3<f32>, view_dir: vec3<f32>, diffuse_color: vec3<f32>, specular_color: vec3<f32>, specular_f90: f32, roughness: f32) -> IblLight {
  let irradiance = ibl_irradiance(normal);
  let radiance = ibl_radiance(view_dir, normal, roughness);
  let scattering = ibl_multiscattering(normal, view_dir, specular_color, specular_f90, roughness);
  let total = scattering.single + scattering.multi;
  let diffuse = diffuse_color * (1.0 - max(max(total.r, total.g), total.b));
  let cosine_weighted = irradiance * (1.0 / PI);
  var out: IblLight;
  out.specular = radiance * scattering.single + scattering.multi * cosine_weighted;
  out.diffuse = diffuse * cosine_weighted;
  return out;
}

// lights_physical_fragment's specularColor (rgb) and specularF90 (w): MeshStandardMaterial when `physical` is false;
// MeshPhysicalMaterial with its default specularIntensity 1 and specularColor white otherwise (F0 follows the ior).
fn ibl_specular(base: vec3<f32>, metalness: f32, ior: f32, physical: bool) -> vec4<f32> {
  if (physical) {
    let r = (ior - 1.0) / (ior + 1.0);
    let f0 = min(vec3<f32>(r * r), vec3<f32>(1.0));
    return vec4<f32>(mix(f0, base, metalness), 1.0);
  }
  return vec4<f32>(mix(vec3<f32>(0.04), base, metalness), 1.0);
}

// lights_physical_fragment's material.roughness: the factor floored at 0.0525 plus the normal's screen-space change.
// Call it in uniform control flow (it takes derivatives) with the interpolated, unperturbed normal.
fn ibl_material_roughness(roughness_factor: f32, geometry_normal: vec3<f32>) -> f32 {
  let dxy = max(abs(dpdx(geometry_normal)), abs(dpdy(geometry_normal)));
  let geometry_roughness = max(max(dxy.x, dxy.y), dxy.z);
  return min(max(roughness_factor, 0.0525) + geometry_roughness, 1.0);
}
