//! Mapper gaps: a drawable or prop the native mappers cannot paint is logged once per key, process-wide.

use std::collections::HashSet;
use std::sync::{OnceLock, RwLock};

const NS: &str = "almadar::game_renderer::mapper";

static GAPS: OnceLock<RwLock<HashSet<String>>> = OnceLock::new();

/// Log a mapper gap the first time `key` is seen.
pub fn gap_once(key: &str, detail: &str) {
    if first_sighting(key) {
        tracing::warn!(target: NS, gap = key, detail = %detail, "mapper gap");
    }
}

fn first_sighting(key: &str) -> bool {
    GAPS.get_or_init(Default::default)
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(key.to_owned())
}

#[cfg(test)]
mod tests {
    use super::first_sighting;

    #[test]
    fn a_gap_is_logged_once_per_key() {
        assert!(first_sighting("diagnostics-test-a"));
        assert!(
            !first_sighting("diagnostics-test-a"),
            "the same key logs once"
        );
        assert!(
            first_sighting("diagnostics-test-b"),
            "control: another key logs"
        );
    }
}
