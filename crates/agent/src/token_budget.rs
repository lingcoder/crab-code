//! Token budget tracking and enforcement.
//!
//! Manages token allocation across input and output during a query loop iteration.
//! The budget tracks cumulative usage and provides decisions on whether to continue,
//! compact, or abort based on remaining capacity.
//!
//! Maps to CCB `query/tokenBudget.ts`.

// ─── Budget decision ───────────────────────────────────────────────────

/// Decision returned by [`TokenBudget::check`] after evaluating remaining capacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetDecision {
    /// Enough budget remains to continue normally.
    Continue,
    /// Budget is running low — trigger compaction to free context space.
    CompactNeeded,
    /// Budget is fully exhausted — the loop must stop.
    BudgetExceeded,
}

// ─── Token budget ──────────────────────────────────────────────────────

/// Compact threshold: trigger compaction when input usage exceeds this fraction.
const COMPACT_THRESHOLD: f64 = 0.85;

/// Tracks token allocation and remaining budget during a query.
///
/// Created at the start of each query loop iteration with the model's
/// context-window limits. As the LLM streams responses and tools produce
/// output, [`record_usage`](Self::record_usage) accumulates consumption.
///
/// # Example
///
/// ```
/// use crab_agent::token_budget::{TokenBudget, BudgetDecision};
///
/// let mut budget = TokenBudget::new(200_000, 16_000);
/// budget.record_usage(50_000, 2_000);
/// assert_eq!(budget.remaining_input(), 150_000);
/// assert_eq!(budget.check(), BudgetDecision::Continue);
/// ```
#[derive(Debug, Clone)]
pub struct TokenBudget {
    /// Maximum input tokens allowed (model context window minus output reservation).
    max_input_tokens: usize,
    /// Maximum output tokens allowed per response.
    max_output_tokens: usize,
    /// Cumulative input tokens consumed so far.
    used_input: usize,
    /// Cumulative output tokens consumed so far.
    used_output: usize,
}

impl TokenBudget {
    /// Create a new budget with the given limits.
    ///
    /// `max_input` is typically `context_window - max_output`.
    /// `max_output` is the model's maximum output token count.
    #[must_use]
    pub fn new(max_input: usize, max_output: usize) -> Self {
        Self {
            max_input_tokens: max_input,
            max_output_tokens: max_output,
            used_input: 0,
            used_output: 0,
        }
    }

    /// Record token consumption from a single LLM round-trip.
    pub fn record_usage(&mut self, input: usize, output: usize) {
        self.used_input = self.used_input.saturating_add(input);
        self.used_output = self.used_output.saturating_add(output);
    }

    /// Remaining input tokens before hitting the limit.
    #[must_use]
    pub fn remaining_input(&self) -> usize {
        self.max_input_tokens.saturating_sub(self.used_input)
    }

    /// Remaining output tokens before hitting the limit.
    #[must_use]
    pub fn remaining_output(&self) -> usize {
        self.max_output_tokens.saturating_sub(self.used_output)
    }

    /// Evaluate current budget state and return an actionable decision.
    ///
    /// - Returns [`BudgetDecision::BudgetExceeded`] if input usage >= max.
    /// - Returns [`BudgetDecision::CompactNeeded`] if input usage >= 85% of max.
    /// - Otherwise returns [`BudgetDecision::Continue`].
    #[must_use]
    pub fn check(&self) -> BudgetDecision {
        if self.used_input >= self.max_input_tokens {
            return BudgetDecision::BudgetExceeded;
        }
        let utilization = self.used_input as f64 / self.max_input_tokens.max(1) as f64;
        if utilization >= COMPACT_THRESHOLD {
            return BudgetDecision::CompactNeeded;
        }
        BudgetDecision::Continue
    }

    /// Current input utilization as a percentage (0.0 – 100.0).
    #[must_use]
    pub fn utilization_percent(&self) -> f64 {
        if self.max_input_tokens == 0 {
            return 100.0;
        }
        (self.used_input as f64 / self.max_input_tokens as f64) * 100.0
    }

    /// Suggest a maximum token count for a tool result based on remaining budget.
    ///
    /// Returns a conservative allocation that leaves room for the LLM to
    /// produce a response after incorporating the tool result.
    #[must_use]
    pub fn tool_result_budget(&self) -> usize {
        // Allocate 80% of remaining input budget for tool results,
        // reserving the rest for the LLM to produce a response.
        self.remaining_input() * 4 / 5
    }
}

// ─── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_budget_starts_empty() {
        let budget = TokenBudget::new(200_000, 16_000);
        assert_eq!(budget.remaining_input(), 200_000);
        assert_eq!(budget.remaining_output(), 16_000);
        assert_eq!(budget.check(), BudgetDecision::Continue);
    }

    #[test]
    fn record_usage_accumulates() {
        let mut budget = TokenBudget::new(200_000, 16_000);
        budget.record_usage(50_000, 2_000);
        budget.record_usage(30_000, 1_000);
        assert_eq!(budget.remaining_input(), 120_000);
        assert_eq!(budget.remaining_output(), 13_000);
    }

    #[test]
    fn check_returns_compact_needed_at_threshold() {
        let mut budget = TokenBudget::new(100_000, 16_000);
        budget.record_usage(86_000, 0);
        assert_eq!(budget.check(), BudgetDecision::CompactNeeded);
    }

    #[test]
    fn check_returns_exceeded_at_max() {
        let mut budget = TokenBudget::new(100_000, 16_000);
        budget.record_usage(100_000, 0);
        assert_eq!(budget.check(), BudgetDecision::BudgetExceeded);
    }

    #[test]
    fn check_returns_continue_below_threshold() {
        let mut budget = TokenBudget::new(100_000, 16_000);
        budget.record_usage(80_000, 0);
        assert_eq!(budget.check(), BudgetDecision::Continue);
    }

    #[test]
    fn utilization_percent_basic() {
        let mut budget = TokenBudget::new(200_000, 16_000);
        budget.record_usage(100_000, 0);
        assert!((budget.utilization_percent() - 50.0).abs() < f64::EPSILON);
    }

    #[test]
    fn utilization_percent_zero_max() {
        let budget = TokenBudget::new(0, 0);
        assert!((budget.utilization_percent() - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn remaining_saturates_at_zero() {
        let mut budget = TokenBudget::new(100, 100);
        budget.record_usage(200, 200);
        assert_eq!(budget.remaining_input(), 0);
        assert_eq!(budget.remaining_output(), 0);
    }
}
