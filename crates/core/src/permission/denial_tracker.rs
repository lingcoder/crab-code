//! Denial tracking for permission decisions.
//!
//! Tracks consecutive and total permission denials so the system can warn the
//! user or the agent when it appears stuck in a denial loop (e.g., repeatedly
//! trying a denied tool).

use std::time::Instant;

// Constants

/// Maximum consecutive denials before triggering a fallback warning.
const MAX_CONSECUTIVE_DENIALS: u32 = 3;

/// Maximum total denials before triggering a fallback warning.
const MAX_TOTAL_DENIALS: usize = 20;

// Types

/// A single denial event record.
#[derive(Debug, Clone)]
pub struct DenialRecord {
    /// Name of the tool that was denied.
    pub tool_name: String,
    /// When the denial occurred.
    pub timestamp: Instant,
    /// Human-readable reason for the denial.
    pub reason: String,
}

/// Tracks permission denials and detects denial loops.
///
/// Maintains a log of recent denials and counts consecutive denials so the
/// agent loop or UI can surface warnings like "You have been denied 3 times
/// in a row -- consider a different approach."
#[derive(Debug)]
pub struct DenialTracker {
    /// All recorded denials (most recent last).
    denials: Vec<DenialRecord>,
    /// Number of consecutive denials without an intervening allow.
    consecutive_count: u32,
}

impl Default for DenialTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl DenialTracker {
    /// Create a new, empty denial tracker.
    pub fn new() -> Self {
        Self {
            denials: Vec::new(),
            consecutive_count: 0,
        }
    }

    /// Record a new denial event.
    pub fn record_denial(&mut self, tool_name: &str, reason: &str) {
        self.consecutive_count += 1;
        self.denials.push(DenialRecord {
            tool_name: tool_name.to_string(),
            timestamp: Instant::now(),
            reason: reason.to_string(),
        });
    }

    /// Return the number of consecutive denials since the last allow/reset.
    pub fn consecutive_denials(&self) -> u32 {
        self.consecutive_count
    }

    /// Check whether the denial count has reached a threshold that warrants
    /// a warning to the user or agent.
    ///
    /// Returns `true` if consecutive denials >= 3 or total denials >= 20.
    pub fn should_warn(&self) -> bool {
        self.consecutive_count >= MAX_CONSECUTIVE_DENIALS || self.denials.len() >= MAX_TOTAL_DENIALS
    }

    /// Reset the consecutive denial counter (called after a successful allow).
    pub fn reset(&mut self) {
        self.consecutive_count = 0;
    }

    /// Return a slice of all recorded denial records.
    pub fn history(&self) -> &[DenialRecord] {
        &self.denials
    }

    /// Return the total number of denials ever recorded.
    pub fn total_denials(&self) -> usize {
        self.denials.len()
    }
}

// Tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_tracker_has_zero_denials() {
        let tracker = DenialTracker::new();
        assert_eq!(tracker.consecutive_denials(), 0);
        assert_eq!(tracker.total_denials(), 0);
        assert!(tracker.history().is_empty());
    }

    #[test]
    fn record_denial_increments_counters() {
        let mut tracker = DenialTracker::new();
        tracker.record_denial("Bash", "denied by policy");
        assert_eq!(tracker.consecutive_denials(), 1);
        assert_eq!(tracker.total_denials(), 1);
        assert_eq!(tracker.history()[0].tool_name, "Bash");
        assert_eq!(tracker.history()[0].reason, "denied by policy");
    }

    #[test]
    fn should_warn_false_below_threshold() {
        let mut tracker = DenialTracker::new();
        tracker.record_denial("Bash", "denied");
        tracker.record_denial("Bash", "denied");
        assert!(!tracker.should_warn());
    }

    #[test]
    fn should_warn_true_at_consecutive_threshold() {
        let mut tracker = DenialTracker::new();
        for _ in 0..3 {
            tracker.record_denial("Bash", "denied");
        }
        assert!(tracker.should_warn());
    }

    #[test]
    fn should_warn_true_at_total_threshold() {
        let mut tracker = DenialTracker::new();
        for i in 0..20 {
            tracker.record_denial("Bash", "denied");
            // Reset consecutive every 2 to avoid hitting consecutive threshold
            if i % 2 == 1 {
                tracker.reset();
            }
        }
        assert!(tracker.should_warn());
    }

    #[test]
    fn reset_clears_consecutive_but_preserves_history() {
        let mut tracker = DenialTracker::new();
        tracker.record_denial("Bash", "denied");
        tracker.record_denial("Bash", "denied");
        assert_eq!(tracker.consecutive_denials(), 2);
        assert_eq!(tracker.total_denials(), 2);

        tracker.reset();
        assert_eq!(tracker.consecutive_denials(), 0);
        assert_eq!(tracker.total_denials(), 2); // history preserved
    }

    #[test]
    fn reset_after_warn_clears_warning() {
        let mut tracker = DenialTracker::new();
        for _ in 0..3 {
            tracker.record_denial("Bash", "denied");
        }
        assert!(tracker.should_warn());

        tracker.reset();
        assert!(!tracker.should_warn());
    }

    #[test]
    fn multiple_tools_tracked() {
        let mut tracker = DenialTracker::new();
        tracker.record_denial("Bash", "policy");
        tracker.record_denial("Edit", "read-only mode");
        assert_eq!(tracker.total_denials(), 2);
        assert_eq!(tracker.consecutive_denials(), 2);
        assert_eq!(tracker.history()[0].tool_name, "Bash");
        assert_eq!(tracker.history()[1].tool_name, "Edit");
    }
}
