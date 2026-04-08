//! Per-source settings change detection.
//!
//! Tracks which settings layer changed and emits diff events so that
//! consumers (hot-reload, UI) can react to specific configuration changes
//! without re-reading the entire merged settings tree.
//!
//! Each settings source (global, project, env, MDM) is fingerprinted
//! and compared against the last-known state.

use std::collections::HashMap;

// ── Change event ──────────────────────────────────────────────────────

/// Describes a detected change in a single settings source.
#[derive(Debug, Clone)]
pub struct SettingsChange {
    /// The source that changed (e.g. "global", "project", "env", "mdm").
    pub source: String,
    /// Keys within that source whose values differ from the last snapshot.
    pub changed_keys: Vec<String>,
}

// ── Detector ──────────────────────────────────────────────────────────

/// Detects changes in individual settings layers by comparing content
/// fingerprints between successive checks.
///
/// # Usage
///
/// ```rust,no_run
/// use crab_config::change_detector::ChangeDetector;
///
/// let mut detector = ChangeDetector::new();
/// let changes = detector.check_for_changes();
/// for change in &changes {
///     println!("source {} changed keys: {:?}", change.source, change.changed_keys);
/// }
/// ```
#[allow(dead_code)]
pub struct ChangeDetector {
    /// Last-known fingerprint per source name.
    known_hashes: HashMap<String, u64>,
}

impl ChangeDetector {
    /// Create a new detector with no known state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            known_hashes: HashMap::new(),
        }
    }

    /// Check all settings sources and return a list of changes since the
    /// last call. Sources that have not changed are omitted.
    pub fn check_for_changes(&mut self) -> Vec<SettingsChange> {
        todo!("check_for_changes: fingerprint each source and compare with known_hashes")
    }

    /// Mark a source as known at its current state, suppressing change
    /// notifications until it actually changes again.
    pub fn mark_known(&mut self, source: &str) {
        todo!(
            "mark_known: record current fingerprint for source '{}'",
            source
        )
    }
}

impl Default for ChangeDetector {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_detector_has_empty_state() {
        let detector = ChangeDetector::new();
        assert!(detector.known_hashes.is_empty());
    }

    #[test]
    fn default_is_same_as_new() {
        let d = ChangeDetector::default();
        assert!(d.known_hashes.is_empty());
    }

    #[test]
    fn settings_change_fields() {
        let change = SettingsChange {
            source: "global".into(),
            changed_keys: vec!["theme".into(), "model".into()],
        };
        assert_eq!(change.source, "global");
        assert_eq!(change.changed_keys.len(), 2);
    }
}
