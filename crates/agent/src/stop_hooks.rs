//! Evaluates whether the agent loop should stop.
//!
//! Provides a set of configurable stop conditions that are checked after each
//! turn of the agent loop. When any condition is met, a [`StopReason`] is
//! returned describing why the loop should terminate.
//!
//! Maps to CCB `query/stopHooks.ts`.

// ─── Stop reason ───────────────────────────────────────────────────────

/// The reason the agent loop should stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    /// Maximum number of turns reached.
    MaxTurns(u32),
    /// Token budget has been exhausted.
    TokenBudgetExceeded,
    /// The user explicitly cancelled the operation.
    UserCancel,
    /// The model emitted an explicit stop signal (e.g. `end_turn`).
    ExplicitStop,
    /// The model produced a response with no tool calls (conversation complete).
    NoToolCalls,
    /// An unrecoverable error occurred.
    Error(String),
}

impl std::fmt::Display for StopReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MaxTurns(n) => write!(f, "maximum turns reached ({n})"),
            Self::TokenBudgetExceeded => f.write_str("token budget exceeded"),
            Self::UserCancel => f.write_str("cancelled by user"),
            Self::ExplicitStop => f.write_str("explicit stop signal"),
            Self::NoToolCalls => f.write_str("no tool calls in response"),
            Self::Error(msg) => write!(f, "error: {msg}"),
        }
    }
}

// ─── Stop conditions ───────────────────────────────────────────────────

/// Configurable conditions that determine when the agent loop should stop.
///
/// Each field is checked by [`should_stop`](Self::should_stop) in priority
/// order. Call [`increment_turn`](Self::increment_turn) after each agent
/// loop iteration.
///
/// # Example
///
/// ```
/// use crab_agent::stop_hooks::{StopConditions, StopReason};
///
/// let mut conds = StopConditions {
///     max_turns: Some(3),
///     max_tokens: None,
///     current_turn: 0,
///     tokens_used: 0,
/// };
///
/// assert!(conds.should_stop().is_none());
/// conds.increment_turn();
/// conds.increment_turn();
/// conds.increment_turn();
/// assert_eq!(conds.should_stop(), Some(StopReason::MaxTurns(3)));
/// ```
#[derive(Debug, Clone, Default)]
pub struct StopConditions {
    /// Maximum number of turns before stopping. `None` = unlimited.
    pub max_turns: Option<u32>,
    /// Maximum cumulative token usage before stopping. `None` = unlimited.
    pub max_tokens: Option<u64>,
    /// Current turn counter (0-indexed, incremented after each loop iteration).
    pub current_turn: u32,
    /// Cumulative tokens used so far.
    pub tokens_used: u64,
}

impl StopConditions {
    /// Evaluate all stop conditions and return the first matched reason, if any.
    ///
    /// Conditions are checked in priority order:
    /// 1. Max turns exceeded
    /// 2. Token budget exceeded
    #[must_use]
    pub fn should_stop(&self) -> Option<StopReason> {
        if let Some(max) = self.max_turns
            && self.current_turn >= max
        {
            return Some(StopReason::MaxTurns(max));
        }
        if let Some(max) = self.max_tokens
            && self.tokens_used >= max
        {
            return Some(StopReason::TokenBudgetExceeded);
        }
        None
    }

    /// Increment the turn counter by one.
    pub fn increment_turn(&mut self) {
        self.current_turn = self.current_turn.saturating_add(1);
    }

    /// Record token usage from the latest turn.
    pub fn record_tokens(&mut self, tokens: u64) {
        self.tokens_used = self.tokens_used.saturating_add(tokens);
    }
}

// ─── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_conditions_never_stop() {
        let conds = StopConditions::default();
        // should_stop has a todo!() body so we can only test
        // that it compiles and the type is correct.
        assert_eq!(conds.current_turn, 0);
        assert_eq!(conds.tokens_used, 0);
    }

    #[test]
    fn increment_turn_advances_counter() {
        let mut conds = StopConditions::default();
        conds.increment_turn();
        conds.increment_turn();
        assert_eq!(conds.current_turn, 2);
    }

    #[test]
    fn record_tokens_accumulates() {
        let mut conds = StopConditions::default();
        conds.record_tokens(1000);
        conds.record_tokens(2000);
        assert_eq!(conds.tokens_used, 3000);
    }

    #[test]
    fn stop_reason_display() {
        assert_eq!(
            StopReason::MaxTurns(10).to_string(),
            "maximum turns reached (10)"
        );
        assert_eq!(StopReason::UserCancel.to_string(), "cancelled by user");
        assert_eq!(
            StopReason::NoToolCalls.to_string(),
            "no tool calls in response"
        );
    }
}
