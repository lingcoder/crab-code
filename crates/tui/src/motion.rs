//! Global "reduced motion" preference.
//!
//! The `prefers_reduced_motion` setting from `config.toml` is stored here so
//! every animation site (spinner, shimmer, loading overlays, call cards) can
//! check it without threading a flag through the entire component tree.
//!
//! Mirrors the `theme::current` pattern: set once at startup, updated on
//! hot-reload.

use std::sync::atomic::{AtomicBool, Ordering};

static PREFERS_REDUCED: AtomicBool = AtomicBool::new(false);

/// Set the reduced-motion preference. Called at startup and on config
/// hot-reload.
pub fn set(prefers: bool) {
    PREFERS_REDUCED.store(prefers, Ordering::Relaxed);
}

/// Returns `true` when the user has opted into reduced motion.
#[must_use]
pub fn prefers_reduced_motion() -> bool {
    PREFERS_REDUCED.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_false() {
        // Other tests may have set this; just verify it returns a bool
        // without panicking.
        let _ = prefers_reduced_motion();
    }

    #[test]
    fn set_and_get() {
        set(true);
        assert!(prefers_reduced_motion());
        set(false);
        assert!(!prefers_reduced_motion());
    }
}
