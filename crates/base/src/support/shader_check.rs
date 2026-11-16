//! Whether a program's WGSL makes a module valid on every host, once its prelude is added. Answers are kept by the
//! (prelude kind, WGSL) pair, since a canvas maps its scene every frame; the module is assembled only on a miss. The
//! check itself is the kernel's [`orbital_core_base::gpu_wgsl::check_wgsl_portable`].

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};

static CHECKED: OnceLock<Mutex<HashMap<u64, Result<(), String>>>> = OnceLock::new();

pub fn check_module(
    kind: &'static str,
    wgsl: &str,
    module: impl FnOnce() -> String,
) -> Result<(), String> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (kind, wgsl).hash(&mut hasher);
    let key = hasher.finish();
    let checked = CHECKED.get_or_init(Default::default);
    if let Some(known) = checked.lock().unwrap_or_else(|p| p.into_inner()).get(&key) {
        return known.clone();
    }
    let answer = orbital_core_base::gpu_wgsl::check_wgsl_portable(&module());
    checked
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(key, answer.clone());
    answer
}

#[cfg(test)]
mod tests {
    use super::check_module;
    use std::cell::Cell;

    const PLAIN: &str = "@fragment fn main() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }";
    const CUBE: &str = "@group(0) @binding(0) var t: texture_cube_array<f32>;\n@group(0) @binding(1) var s: sampler;\n@fragment fn main() -> @location(0) vec4<f32> { return textureSample(t, s, vec3<f32>(0.0, 0.0, 1.0), 0); }";

    #[test]
    fn a_module_is_assembled_and_checked_once_per_kind_and_wgsl() {
        let built = Cell::new(0);
        let build = || {
            built.set(built.get() + 1);
            PLAIN.to_owned()
        };
        assert!(check_module("test-plain", PLAIN, build).is_ok());
        assert!(check_module("test-plain", PLAIN, || {
            built.set(built.get() + 1);
            PLAIN.to_owned()
        })
        .is_ok());
        assert_eq!(built.get(), 1, "the second answer is remembered");
        assert!(
            check_module("test-cube", CUBE, || CUBE.to_owned()).is_err(),
            "control: what WebGL2 lacks is refused"
        );
    }
}
