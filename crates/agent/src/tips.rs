//! Contextual tips based on tool usage patterns and first-time feature use.
//!
//! The tip system shows non-intrusive hints in the TUI when users encounter
//! new features or common pitfalls. Tips are shown at most once per session
//! and can be permanently dismissed.
//!
//! Maps to CCB `tips/tips.ts`.

use std::collections::HashSet;

// ── Types ─────────────────────────────────────────────────────────────

/// A single contextual tip.
#[derive(Debug, Clone)]
pub struct Tip {
    /// Stable identifier for deduplication and persistence.
    pub id: &'static str,
    /// Human-readable tip message (may contain markdown).
    pub message: String,
    /// Whether this tip has been shown in the current session.
    pub shown: bool,
}

/// Registry that tracks available tips and which have been shown.
///
/// # Example
///
/// ```rust,no_run
/// use crab_agent::tips::TipRegistry;
///
/// let mut registry = TipRegistry::new();
/// if let Some(tip) = registry.get_tip_for_context("Bash", None) {
///     println!("Tip: {}", tip.message);
///     registry.mark_shown(tip.id);
/// }
/// ```
#[allow(dead_code)]
pub struct TipRegistry {
    /// All registered tips.
    tips: Vec<Tip>,
    /// IDs of tips that have been shown (either this session or persisted).
    shown_ids: HashSet<String>,
}

impl TipRegistry {
    /// Create a new registry pre-populated with the built-in tips.
    #[must_use]
    pub fn new() -> Self {
        Self {
            tips: Vec::new(),
            shown_ids: HashSet::new(),
        }
    }

    /// Find the most relevant tip for the current context.
    ///
    /// # Arguments
    ///
    /// * `tool_name` — The tool that was just used or is about to be used.
    /// * `error` — If the tool returned an error, the error message.
    ///
    /// Returns `None` if no un-shown tip matches the context.
    #[must_use]
    pub fn get_tip_for_context(&self, _tool_name: &str, _error: Option<&str>) -> Option<&Tip> {
        todo!("get_tip_for_context: match tool_name/error against tip triggers, skip shown tips")
    }

    /// Mark a tip as shown so it will not be suggested again.
    pub fn mark_shown(&mut self, id: &str) {
        todo!("mark_shown: add id '{}' to shown_ids", id)
    }
}

impl Default for TipRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_registry_is_empty() {
        let registry = TipRegistry::new();
        assert!(registry.tips.is_empty());
        assert!(registry.shown_ids.is_empty());
    }

    #[test]
    fn default_is_same_as_new() {
        let r = TipRegistry::default();
        assert!(r.tips.is_empty());
    }

    #[test]
    fn tip_fields() {
        let tip = Tip {
            id: "bash_timeout",
            message: "Use the Sleep tool for long waits".into(),
            shown: false,
        };
        assert_eq!(tip.id, "bash_timeout");
        assert!(!tip.shown);
    }
}
