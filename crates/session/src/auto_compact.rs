//! Reactive auto-compaction based on context usage.
//!
//! Monitors token usage relative to the model's context window and triggers
//! compaction when a configurable threshold is exceeded. After compaction,
//! performs cleanup to re-attach file/skill attachments and reset caches.
//!
//! Maps to CCB `compact/autoCompact.ts` + `reactiveCompact.ts`.

// ─── Constants ────────────────────────────────────────────────────────

/// Buffer tokens reserved for auto-compact trigger headroom.
/// Auto-compact fires at `effective_window - AUTOCOMPACT_BUFFER_TOKENS`.
const AUTOCOMPACT_BUFFER_TOKENS: usize = 13_000;

/// Tokens reserved for the compaction summary output.
const SUMMARY_OUTPUT_RESERVE: usize = 20_000;

/// Maximum consecutive auto-compact failures before the circuit breaker
/// disables further attempts for the session.
const MAX_CONSECUTIVE_FAILURES: u32 = 3;

// ─── Configuration ─────────────────────────────────────────────────────

/// Configuration for automatic compaction triggers.
#[derive(Debug, Clone)]
pub struct AutoCompactConfig {
    /// Whether auto-compaction is enabled.
    pub enabled: bool,
    /// Token usage fraction at which to trigger compaction (e.g. 0.85 = 85%).
    pub threshold_percent: f64,
    /// Minimum number of messages in the conversation before compaction is
    /// allowed. Prevents compacting very short conversations.
    pub min_messages_before_compact: usize,
}

impl Default for AutoCompactConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold_percent: 0.85,
            min_messages_before_compact: 6,
        }
    }
}

// ─── Trigger ───────────────────────────────────────────────────────────

/// Describes why compaction was triggered.
#[derive(Debug, Clone, PartialEq)]
pub enum CompactTrigger {
    /// Token usage exceeded the configured threshold.
    ThresholdExceeded {
        /// Actual usage percentage at the time of trigger.
        usage_percent: f64,
    },
    /// The user explicitly requested compaction.
    ManualRequest,
    /// Compaction is needed as part of session restoration.
    SessionRestore,
}

// ─── Circuit breaker ──────────────────────────────────────────────────

/// Tracks auto-compact failures to implement a circuit breaker pattern.
///
/// After [`MAX_CONSECUTIVE_FAILURES`] consecutive failures, auto-compaction
/// is disabled for the remainder of the session.
#[derive(Debug, Default)]
pub struct AutoCompactState {
    /// Number of consecutive auto-compact failures.
    pub consecutive_failures: u32,
}

impl AutoCompactState {
    /// Record a successful compaction (resets failure counter).
    pub fn record_success(&mut self) {
        self.consecutive_failures = 0;
    }

    /// Record a failed compaction attempt.
    pub fn record_failure(&mut self) {
        self.consecutive_failures += 1;
    }

    /// Whether the circuit breaker has tripped (too many failures).
    pub fn is_circuit_broken(&self) -> bool {
        self.consecutive_failures >= MAX_CONSECUTIVE_FAILURES
    }
}

// ─── Decision function ─────────────────────────────────────────────────

/// Compute the effective auto-compact threshold in tokens.
///
/// Formula: `context_window - SUMMARY_OUTPUT_RESERVE - AUTOCOMPACT_BUFFER_TOKENS`
pub fn auto_compact_threshold(context_window: usize) -> usize {
    context_window
        .saturating_sub(SUMMARY_OUTPUT_RESERVE)
        .saturating_sub(AUTOCOMPACT_BUFFER_TOKENS)
}

/// Determine whether auto-compaction should be triggered.
///
/// Returns `Some(CompactTrigger)` if compaction should happen, `None` otherwise.
pub fn should_auto_compact(
    total_tokens: usize,
    context_window: usize,
    config: &AutoCompactConfig,
    message_count: usize,
) -> Option<CompactTrigger> {
    // Disabled
    if !config.enabled {
        return None;
    }

    // Not enough messages to justify compaction
    if message_count < config.min_messages_before_compact {
        return None;
    }

    // Context window too small to be useful
    if context_window == 0 {
        return None;
    }

    // Check percentage-based threshold
    let usage_percent = total_tokens as f64 / context_window as f64;
    if usage_percent >= config.threshold_percent {
        return Some(CompactTrigger::ThresholdExceeded { usage_percent });
    }

    // Also check absolute token threshold (CCB-style buffer approach)
    let threshold = auto_compact_threshold(context_window);
    if total_tokens >= threshold {
        let usage_pct = total_tokens as f64 / context_window as f64;
        return Some(CompactTrigger::ThresholdExceeded {
            usage_percent: usage_pct,
        });
    }

    None
}

/// Post-compact cleanup: re-attach file/skill attachments, reset caches.
///
/// After a compaction pass removes or summarizes old messages, certain
/// context attachments (open files, active skills) need to be re-injected
/// into the conversation so the model retains awareness of them.
///
/// This is currently a no-op placeholder. The actual re-injection will
/// be implemented when the file-tracking and skill systems are built
/// (Phases 7-8). The function signature is stable for callers.
pub fn post_compact_cleanup() {
    // Phase 7 (tools) will track open files for re-injection.
    // Phase 8 (skills) will track active skills for re-injection.
    // For now, compaction.rs already handles the core message rewriting.
}

// ─── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let config = AutoCompactConfig::default();
        assert!(config.enabled);
        assert!((config.threshold_percent - 0.85).abs() < f64::EPSILON);
        assert_eq!(config.min_messages_before_compact, 6);
    }

    #[test]
    fn compact_trigger_equality() {
        let a = CompactTrigger::ManualRequest;
        let b = CompactTrigger::ManualRequest;
        assert_eq!(a, b);

        let c = CompactTrigger::ThresholdExceeded {
            usage_percent: 0.90,
        };
        let d = CompactTrigger::ThresholdExceeded {
            usage_percent: 0.90,
        };
        assert_eq!(c, d);
    }

    #[test]
    fn compact_trigger_debug() {
        let trigger = CompactTrigger::SessionRestore;
        let debug = format!("{trigger:?}");
        assert!(debug.contains("SessionRestore"));
    }

    #[test]
    fn should_compact_when_above_threshold() {
        let config = AutoCompactConfig {
            enabled: true,
            threshold_percent: 0.80,
            min_messages_before_compact: 4,
        };
        // 90K out of 100K = 90%, above 80% threshold
        let result = should_auto_compact(90_000, 100_000, &config, 10);
        assert!(result.is_some());
        if let Some(CompactTrigger::ThresholdExceeded { usage_percent }) = result {
            assert!(usage_percent >= 0.9);
        }
    }

    #[test]
    fn should_not_compact_below_threshold() {
        let config = AutoCompactConfig {
            enabled: true,
            threshold_percent: 0.85,
            min_messages_before_compact: 4,
        };
        // 50K out of 100K = 50%, below 85%
        let result = should_auto_compact(50_000, 100_000, &config, 10);
        assert!(result.is_none());
    }

    #[test]
    fn should_not_compact_when_disabled() {
        let config = AutoCompactConfig {
            enabled: false,
            threshold_percent: 0.50,
            min_messages_before_compact: 1,
        };
        let result = should_auto_compact(90_000, 100_000, &config, 10);
        assert!(result.is_none());
    }

    #[test]
    fn should_not_compact_too_few_messages() {
        let config = AutoCompactConfig {
            enabled: true,
            threshold_percent: 0.80,
            min_messages_before_compact: 10,
        };
        // Above threshold but only 5 messages
        let result = should_auto_compact(90_000, 100_000, &config, 5);
        assert!(result.is_none());
    }

    #[test]
    fn auto_compact_threshold_calculation() {
        // 100K - 20K reserve - 13K buffer = 67K
        let threshold = auto_compact_threshold(100_000);
        assert_eq!(threshold, 67_000);
    }

    #[test]
    fn auto_compact_threshold_small_window() {
        // Window smaller than reserves → saturates to 0
        let threshold = auto_compact_threshold(10_000);
        assert_eq!(threshold, 0);
    }

    #[test]
    fn circuit_breaker_trips_after_max_failures() {
        let mut state = AutoCompactState::default();
        assert!(!state.is_circuit_broken());

        for _ in 0..3 {
            state.record_failure();
        }
        assert!(state.is_circuit_broken());
    }

    #[test]
    fn circuit_breaker_resets_on_success() {
        let mut state = AutoCompactState::default();
        state.record_failure();
        state.record_failure();
        assert!(!state.is_circuit_broken());

        state.record_success();
        assert_eq!(state.consecutive_failures, 0);
        assert!(!state.is_circuit_broken());
    }

    #[test]
    fn post_compact_cleanup_is_noop() {
        // Should not panic
        post_compact_cleanup();
    }
}
