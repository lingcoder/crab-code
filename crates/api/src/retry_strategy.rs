//! Request retry strategies with exponential/linear backoff and circuit breaker.
//!
//! `RetryStrategy` is a trait for pluggable retry logic. Built-in strategies:
//! `ExponentialBackoff`, `LinearBackoff`, `CircuitBreakerRetry`.
//! `CompositeRetryStrategy` combines multiple strategies.

use std::time::{Duration, Instant};

use crate::error_classifier::ErrorCategory;

// ---------------------------------------------------------------------------
// RetryDecision
// ---------------------------------------------------------------------------

/// Decision from a retry strategy.
#[derive(Debug, Clone)]
pub struct RetryDecision {
    /// Whether to retry the request.
    pub retry: bool,
    /// How long to wait before retrying.
    pub delay: Duration,
    /// Human-readable reason for the decision.
    pub reason: String,
}

impl RetryDecision {
    /// Create a "do retry" decision.
    #[must_use]
    pub fn retry_after(delay: Duration, reason: impl Into<String>) -> Self {
        Self {
            retry: true,
            delay,
            reason: reason.into(),
        }
    }

    /// Create a "do not retry" decision.
    #[must_use]
    pub fn stop(reason: impl Into<String>) -> Self {
        Self {
            retry: false,
            delay: Duration::ZERO,
            reason: reason.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// RetryConfig
// ---------------------------------------------------------------------------

/// Configuration for retry behavior.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts.
    pub max_attempts: u32,
    /// Base delay for the first retry.
    pub base_delay: Duration,
    /// Maximum delay cap.
    pub max_delay: Duration,
    /// Whether to add random jitter to delays.
    pub jitter: bool,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
            jitter: true,
        }
    }
}

// ---------------------------------------------------------------------------
// RetryStrategy trait
// ---------------------------------------------------------------------------

/// Trait for retry decision logic.
pub trait RetryStrategy: Send + Sync {
    /// Decide whether to retry given the error category and attempt number.
    fn should_retry(&self, category: ErrorCategory, attempt: u32) -> RetryDecision;

    /// Strategy name (for logging).
    fn name(&self) -> &'static str;
}

// ---------------------------------------------------------------------------
// ExponentialBackoff
// ---------------------------------------------------------------------------

/// Exponential backoff with optional jitter.
///
/// Delay = min(base * 2^`attempt`, `max_delay`), optionally with ±25% jitter.
#[derive(Debug, Clone)]
pub struct ExponentialBackoff {
    config: RetryConfig,
}

impl ExponentialBackoff {
    #[must_use]
    pub fn new(config: RetryConfig) -> Self {
        Self { config }
    }

    /// Compute delay for the given attempt.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let base_ms = self.config.base_delay.as_millis() as u64;
        let exp_ms = base_ms.saturating_mul(1u64 << attempt.min(20));
        let capped_ms = exp_ms.min(self.config.max_delay.as_millis() as u64);

        if self.config.jitter {
            // Deterministic "jitter" based on attempt number (no rand dependency).
            let jitter_factor = match attempt % 4 {
                1 => 85,
                2 => 115,
                3 => 95,
                _ => 100,
            };
            Duration::from_millis(capped_ms * jitter_factor / 100)
        } else {
            Duration::from_millis(capped_ms)
        }
    }
}

impl Default for ExponentialBackoff {
    fn default() -> Self {
        Self::new(RetryConfig::default())
    }
}

impl RetryStrategy for ExponentialBackoff {
    fn should_retry(&self, category: ErrorCategory, attempt: u32) -> RetryDecision {
        if attempt >= self.config.max_attempts {
            return RetryDecision::stop(format!(
                "max attempts ({}) reached",
                self.config.max_attempts
            ));
        }
        if !crate::error_classifier::is_retryable(category) {
            return RetryDecision::stop(format!("error category {category} is not retryable"));
        }
        let delay = self.delay_for_attempt(attempt);
        RetryDecision::retry_after(delay, format!("exponential backoff attempt {attempt}"))
    }

    fn name(&self) -> &'static str {
        "exponential_backoff"
    }
}

// ---------------------------------------------------------------------------
// LinearBackoff
// ---------------------------------------------------------------------------

/// Linear backoff: delay = base * (attempt + 1), capped at `max_delay`.
#[derive(Debug, Clone)]
pub struct LinearBackoff {
    config: RetryConfig,
}

impl LinearBackoff {
    #[must_use]
    pub fn new(config: RetryConfig) -> Self {
        Self { config }
    }
}

impl RetryStrategy for LinearBackoff {
    fn should_retry(&self, category: ErrorCategory, attempt: u32) -> RetryDecision {
        if attempt >= self.config.max_attempts {
            return RetryDecision::stop(format!(
                "max attempts ({}) reached",
                self.config.max_attempts
            ));
        }
        if !crate::error_classifier::is_retryable(category) {
            return RetryDecision::stop(format!("error category {category} is not retryable"));
        }
        #[allow(clippy::cast_possible_truncation)]
        let base_ms = self.config.base_delay.as_millis() as u64;
        let delay_ms = base_ms.saturating_mul(u64::from(attempt) + 1);
        #[allow(clippy::cast_possible_truncation)]
        let capped = delay_ms.min(self.config.max_delay.as_millis() as u64);
        RetryDecision::retry_after(
            Duration::from_millis(capped),
            format!("linear backoff attempt {attempt}"),
        )
    }

    fn name(&self) -> &'static str {
        "linear_backoff"
    }
}

// ---------------------------------------------------------------------------
// CircuitBreakerRetry
// ---------------------------------------------------------------------------

/// Circuit breaker state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation — requests pass through.
    Closed,
    /// Testing recovery — one request allowed through.
    HalfOpen,
    /// Failing — all requests rejected immediately.
    Open,
}

/// Circuit breaker retry strategy.
///
/// After `failure_threshold` consecutive failures, the circuit opens for
/// `open_duration`. After that, one test request is allowed (half-open).
/// If it succeeds, the circuit closes; if it fails, it reopens.
#[derive(Debug)]
pub struct CircuitBreakerRetry {
    config: RetryConfig,
    failure_threshold: u32,
    open_duration: Duration,
    // Mutable state — interior mutability not needed since we take &self
    // and the caller is expected to track state externally or use this
    // as a stateless policy evaluator.
    state: std::sync::Mutex<CircuitBreakerState>,
}

#[derive(Debug)]
struct CircuitBreakerState {
    current: CircuitState,
    consecutive_failures: u32,
    opened_at: Option<Instant>,
}

impl CircuitBreakerRetry {
    /// Create a circuit breaker with the given thresholds.
    #[must_use]
    pub fn new(config: RetryConfig, failure_threshold: u32, open_duration: Duration) -> Self {
        Self {
            config,
            failure_threshold,
            open_duration,
            state: std::sync::Mutex::new(CircuitBreakerState {
                current: CircuitState::Closed,
                consecutive_failures: 0,
                opened_at: None,
            }),
        }
    }

    /// Get the current circuit state.
    #[must_use]
    pub fn state(&self) -> CircuitState {
        let state = self.state.lock().unwrap();
        state.current
    }

    /// Record a successful request (resets the circuit).
    pub fn record_success(&self) {
        let mut state = self.state.lock().unwrap();
        state.current = CircuitState::Closed;
        state.consecutive_failures = 0;
        state.opened_at = None;
    }

    /// Record a failed request (may trip the circuit).
    pub fn record_failure(&self) {
        let mut state = self.state.lock().unwrap();
        state.consecutive_failures += 1;
        if state.consecutive_failures >= self.failure_threshold
            && state.current == CircuitState::Closed
        {
            state.current = CircuitState::Open;
            state.opened_at = Some(Instant::now());
        } else if state.current == CircuitState::HalfOpen {
            // Test request failed — reopen.
            state.current = CircuitState::Open;
            state.opened_at = Some(Instant::now());
        }
    }

    fn check_transition(&self) {
        let mut state = self.state.lock().unwrap();
        if state.current == CircuitState::Open
            && let Some(opened_at) = state.opened_at
            && opened_at.elapsed() >= self.open_duration
        {
            state.current = CircuitState::HalfOpen;
        }
    }
}

impl RetryStrategy for CircuitBreakerRetry {
    fn should_retry(&self, category: ErrorCategory, attempt: u32) -> RetryDecision {
        self.check_transition();

        let state = self.state();
        match state {
            CircuitState::Open => {
                RetryDecision::stop("circuit breaker is open — request rejected".to_string())
            }
            CircuitState::HalfOpen => {
                // Allow one test request.
                if attempt >= 1 {
                    RetryDecision::stop(
                        "circuit breaker half-open — only one test request allowed".to_string(),
                    )
                } else {
                    RetryDecision::retry_after(
                        Duration::ZERO,
                        "circuit breaker half-open — test request",
                    )
                }
            }
            CircuitState::Closed => {
                if attempt >= self.config.max_attempts {
                    return RetryDecision::stop(format!(
                        "max attempts ({}) reached",
                        self.config.max_attempts
                    ));
                }
                if !crate::error_classifier::is_retryable(category) {
                    return RetryDecision::stop(format!("error {category} not retryable"));
                }
                RetryDecision::retry_after(
                    self.config.base_delay,
                    format!("circuit closed, attempt {attempt}"),
                )
            }
        }
    }

    fn name(&self) -> &'static str {
        "circuit_breaker"
    }
}

// ---------------------------------------------------------------------------
// CompositeRetryStrategy
// ---------------------------------------------------------------------------

/// Combines multiple retry strategies. All must agree to retry.
pub struct CompositeRetryStrategy {
    strategies: Vec<Box<dyn RetryStrategy>>,
}

impl CompositeRetryStrategy {
    #[must_use]
    pub fn new(strategies: Vec<Box<dyn RetryStrategy>>) -> Self {
        Self { strategies }
    }
}

impl RetryStrategy for CompositeRetryStrategy {
    fn should_retry(&self, category: ErrorCategory, attempt: u32) -> RetryDecision {
        let mut max_delay = Duration::ZERO;
        for strategy in &self.strategies {
            let decision = strategy.should_retry(category, attempt);
            if !decision.retry {
                return decision;
            }
            if decision.delay > max_delay {
                max_delay = decision.delay;
            }
        }
        RetryDecision::retry_after(max_delay, "all strategies agree to retry")
    }

    fn name(&self) -> &'static str {
        "composite"
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- RetryDecision --

    #[test]
    fn decision_retry() {
        let d = RetryDecision::retry_after(Duration::from_secs(1), "test");
        assert!(d.retry);
        assert_eq!(d.delay, Duration::from_secs(1));
    }

    #[test]
    fn decision_stop() {
        let d = RetryDecision::stop("done");
        assert!(!d.retry);
        assert_eq!(d.delay, Duration::ZERO);
    }

    // -- RetryConfig --

    #[test]
    fn config_defaults() {
        let cfg = RetryConfig::default();
        assert_eq!(cfg.max_attempts, 3);
        assert!(cfg.jitter);
    }

    // -- ExponentialBackoff --

    #[test]
    fn exponential_retries_transient() {
        let eb = ExponentialBackoff::default();
        let d = eb.should_retry(ErrorCategory::Transient, 0);
        assert!(d.retry);
        assert!(d.delay.as_millis() > 0);
    }

    #[test]
    fn exponential_stops_after_max_attempts() {
        let eb = ExponentialBackoff::new(RetryConfig {
            max_attempts: 2,
            ..RetryConfig::default()
        });
        let d = eb.should_retry(ErrorCategory::Transient, 2);
        assert!(!d.retry);
    }

    #[test]
    fn exponential_stops_for_non_retryable() {
        let eb = ExponentialBackoff::default();
        let d = eb.should_retry(ErrorCategory::Auth, 0);
        assert!(!d.retry);
    }

    #[test]
    fn exponential_delay_increases() {
        let eb = ExponentialBackoff::new(RetryConfig {
            jitter: false,
            ..RetryConfig::default()
        });
        let d0 = eb.delay_for_attempt(0);
        let d1 = eb.delay_for_attempt(1);
        let d2 = eb.delay_for_attempt(2);
        assert!(d1 > d0);
        assert!(d2 > d1);
    }

    #[test]
    fn exponential_delay_capped() {
        let eb = ExponentialBackoff::new(RetryConfig {
            max_delay: Duration::from_secs(5),
            jitter: false,
            ..RetryConfig::default()
        });
        let d = eb.delay_for_attempt(20);
        assert!(d <= Duration::from_secs(5));
    }

    #[test]
    fn exponential_name() {
        let eb = ExponentialBackoff::default();
        assert_eq!(eb.name(), "exponential_backoff");
    }

    // -- LinearBackoff --

    #[test]
    fn linear_retries_transient() {
        let lb = LinearBackoff::new(RetryConfig::default());
        let d = lb.should_retry(ErrorCategory::Transient, 0);
        assert!(d.retry);
    }

    #[test]
    fn linear_stops_after_max() {
        let lb = LinearBackoff::new(RetryConfig {
            max_attempts: 2,
            ..RetryConfig::default()
        });
        let d = lb.should_retry(ErrorCategory::Transient, 2);
        assert!(!d.retry);
    }

    #[test]
    fn linear_delay_linear() {
        let cfg = RetryConfig {
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(10),
            max_attempts: 5,
            jitter: false,
        };
        let lb = LinearBackoff::new(cfg);
        let d0 = lb.should_retry(ErrorCategory::Transient, 0);
        let d1 = lb.should_retry(ErrorCategory::Transient, 1);
        // attempt 0: 100 * 1 = 100ms, attempt 1: 100 * 2 = 200ms
        assert_eq!(d0.delay, Duration::from_millis(100));
        assert_eq!(d1.delay, Duration::from_millis(200));
    }

    // -- CircuitBreakerRetry --

    #[test]
    fn circuit_breaker_starts_closed() {
        let cb = CircuitBreakerRetry::new(RetryConfig::default(), 3, Duration::from_secs(30));
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn circuit_breaker_opens_after_threshold() {
        let cb = CircuitBreakerRetry::new(RetryConfig::default(), 3, Duration::from_secs(30));
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn circuit_breaker_rejects_when_open() {
        let cb = CircuitBreakerRetry::new(RetryConfig::default(), 1, Duration::from_secs(300));
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        let d = cb.should_retry(ErrorCategory::Transient, 0);
        assert!(!d.retry);
    }

    #[test]
    fn circuit_breaker_resets_on_success() {
        let cb = CircuitBreakerRetry::new(RetryConfig::default(), 2, Duration::from_secs(30));
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn circuit_breaker_half_open_after_timeout() {
        let cb = CircuitBreakerRetry::new(RetryConfig::default(), 1, Duration::from_millis(0));
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        // Open duration is 0ms, so it should immediately transition.
        cb.check_transition();
        assert_eq!(cb.state(), CircuitState::HalfOpen);
    }

    // -- CompositeRetryStrategy --

    #[test]
    fn composite_all_agree() {
        let strategies: Vec<Box<dyn RetryStrategy>> = vec![
            Box::new(ExponentialBackoff::default()),
            Box::new(LinearBackoff::new(RetryConfig::default())),
        ];
        let comp = CompositeRetryStrategy::new(strategies);
        let d = comp.should_retry(ErrorCategory::Transient, 0);
        assert!(d.retry);
    }

    #[test]
    fn composite_one_rejects() {
        let strategies: Vec<Box<dyn RetryStrategy>> = vec![
            Box::new(ExponentialBackoff::new(RetryConfig {
                max_attempts: 0, // Will always reject.
                ..RetryConfig::default()
            })),
            Box::new(LinearBackoff::new(RetryConfig::default())),
        ];
        let comp = CompositeRetryStrategy::new(strategies);
        let d = comp.should_retry(ErrorCategory::Transient, 0);
        assert!(!d.retry);
    }

    #[test]
    fn composite_name() {
        let comp = CompositeRetryStrategy::new(vec![]);
        assert_eq!(comp.name(), "composite");
    }
}
