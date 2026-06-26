//! Retry/backoff policy for API requests.
//!
//! Client-side preventive rate limiting is intentionally absent: providers
//! enforce limits server-side and return 429 / `retry-after` headers, which
//! the retry policy below already handles.

use std::time::Duration;

use crate::error::ApiError;

/// Exponential backoff delay for retries.
///
/// Base 500ms, multiplied by 2^attempt, capped at 30 seconds.
#[must_use]
pub fn backoff_delay(attempt: u32) -> Duration {
    let base = Duration::from_millis(500);
    let max = Duration::from_secs(30);
    let delay = base.saturating_mul(2u32.saturating_pow(attempt.min(6)));
    delay.min(max)
}

/// Maximum number of retry attempts.
pub const MAX_RETRIES: u32 = 3;

/// Configurable retry policy for API requests.
///
/// Controls how many times to retry, the backoff strategy, and which
/// HTTP status codes are considered retryable beyond the defaults.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Maximum number of retry attempts (default: 3).
    pub max_retries: u32,
    /// Base delay for exponential backoff (default: 500ms).
    pub base_delay: Duration,
    /// Maximum delay cap (default: 30s).
    pub max_delay: Duration,
    /// Additional retryable status codes beyond 429 and 529.
    pub retryable_statuses: Vec<u16>,
    /// Whether to retry on 5xx server errors (default: true).
    pub retry_server_errors: bool,
}

impl RetryPolicy {
    /// Create a default retry policy.
    #[must_use]
    pub fn new() -> Self {
        Self {
            max_retries: MAX_RETRIES,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
            retryable_statuses: Vec::new(),
            retry_server_errors: true,
        }
    }

    /// Create a policy that never retries.
    #[must_use]
    pub fn no_retry() -> Self {
        Self {
            max_retries: 0,
            ..Self::new()
        }
    }

    /// Create an aggressive retry policy (more attempts, shorter delays).
    #[must_use]
    pub fn aggressive() -> Self {
        Self {
            max_retries: 5,
            base_delay: Duration::from_millis(200),
            max_delay: Duration::from_secs(10),
            retryable_statuses: Vec::new(),
            retry_server_errors: true,
        }
    }

    /// Set maximum retry attempts.
    #[must_use]
    pub fn with_max_retries(mut self, max: u32) -> Self {
        self.max_retries = max;
        self
    }

    /// Set base delay for exponential backoff.
    #[must_use]
    pub fn with_base_delay(mut self, delay: Duration) -> Self {
        self.base_delay = delay;
        self
    }

    /// Set maximum delay cap.
    #[must_use]
    pub fn with_max_delay(mut self, delay: Duration) -> Self {
        self.max_delay = delay;
        self
    }

    /// Add an additional retryable status code.
    #[must_use]
    pub fn with_retryable_status(mut self, status: u16) -> Self {
        self.retryable_statuses.push(status);
        self
    }

    /// Calculate backoff delay for a given attempt number.
    #[must_use]
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let delay = self
            .base_delay
            .saturating_mul(2u32.saturating_pow(attempt.min(6)));
        delay.min(self.max_delay)
    }

    /// Whether an error should be retried under this policy.
    #[must_use]
    pub fn should_retry(&self, err: &ApiError, attempt: u32) -> bool {
        if attempt >= self.max_retries {
            return false;
        }
        match err {
            ApiError::RateLimited { .. } | ApiError::Timeout => true,
            ApiError::Api { status, .. } => {
                *status == 429
                    || *status == 529
                    || (self.retry_server_errors && (500..600).contains(status))
                    || self.retryable_statuses.contains(status)
            }
            ApiError::Http(e) => e.is_timeout() || e.is_connect(),
            ApiError::Json(_) | ApiError::Sse(_) | ApiError::Common(_) => false,
        }
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::new()
    }
}

/// Check whether an API error is retryable under the default policy.
///
/// Delegates to [`RetryPolicy::should_retry`] so this standalone helper can
/// never disagree with the policy on the same error (notably transient 5xx,
/// which the policy retries but a hand-rolled 429/529-only check would not).
#[must_use]
pub fn is_retryable(err: &ApiError) -> bool {
    RetryPolicy::default().should_retry(err, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_delay_increases_exponentially() {
        let d0 = backoff_delay(0);
        let d1 = backoff_delay(1);
        let d2 = backoff_delay(2);
        assert_eq!(d0, Duration::from_millis(500));
        assert_eq!(d1, Duration::from_millis(1000));
        assert_eq!(d2, Duration::from_millis(2000));
    }

    #[test]
    fn backoff_delay_capped_at_30s() {
        let d10 = backoff_delay(10);
        assert_eq!(d10, Duration::from_secs(30));
    }

    #[test]
    fn rate_limited_is_retryable() {
        let err = ApiError::RateLimited {
            retry_after_ms: 1000,
        };
        assert!(is_retryable(&err));
    }

    #[test]
    fn status_429_is_retryable() {
        let err = ApiError::Api {
            status: 429,
            message: "rate limited".into(),
        };
        assert!(is_retryable(&err));
    }

    #[test]
    fn status_529_is_retryable() {
        let err = ApiError::Api {
            status: 529,
            message: "overloaded".into(),
        };
        assert!(is_retryable(&err));
    }

    #[test]
    fn transient_5xx_is_retryable_like_the_policy() {
        // The standalone helper must agree with RetryPolicy::should_retry on
        // 5xx, which the default policy retries.
        for status in [500u16, 502, 503, 504] {
            let err = ApiError::Api {
                status,
                message: "server error".into(),
            };
            assert!(is_retryable(&err), "status {status} should be retryable");
            assert_eq!(
                is_retryable(&err),
                RetryPolicy::default().should_retry(&err, 0)
            );
        }
    }

    #[test]
    fn status_400_is_not_retryable() {
        let err = ApiError::Api {
            status: 400,
            message: "bad request".into(),
        };
        assert!(!is_retryable(&err));
    }

    #[test]
    fn json_error_is_not_retryable() {
        let err = ApiError::Sse("parse error".into());
        assert!(!is_retryable(&err));
    }

    #[test]
    fn backoff_delay_all_attempts() {
        // Verify the full sequence
        assert_eq!(backoff_delay(0), Duration::from_millis(500));
        assert_eq!(backoff_delay(1), Duration::from_millis(1000));
        assert_eq!(backoff_delay(2), Duration::from_millis(2000));
        assert_eq!(backoff_delay(3), Duration::from_millis(4000));
        assert_eq!(backoff_delay(4), Duration::from_millis(8000));
        assert_eq!(backoff_delay(5), Duration::from_millis(16000));
        assert_eq!(backoff_delay(6), Duration::from_secs(30)); // capped
    }

    #[test]
    fn backoff_delay_huge_attempt_saturates() {
        // u32::MAX should not panic; just caps at 30s
        let d = backoff_delay(u32::MAX);
        assert_eq!(d, Duration::from_secs(30));
    }

    #[test]
    fn max_retries_constant() {
        assert_eq!(MAX_RETRIES, 3);
    }

    #[test]
    fn timeout_is_retryable() {
        assert!(is_retryable(&ApiError::Timeout));
    }

    #[test]
    fn status_500_is_retryable() {
        // 5xx are transient; is_retryable must agree with the default policy
        // (which retries server errors) rather than diverging from it.
        let err = ApiError::Api {
            status: 500,
            message: "internal".into(),
        };
        assert!(is_retryable(&err));
    }

    #[test]
    fn common_error_is_not_retryable() {
        let err = ApiError::Common(crab_core::Error::Other("test".into()));
        assert!(!is_retryable(&err));
    }

    // ─── RetryPolicy ───

    #[test]
    fn retry_policy_default() {
        let policy = RetryPolicy::new();
        assert_eq!(policy.max_retries, 3);
        assert_eq!(policy.base_delay, Duration::from_millis(500));
        assert_eq!(policy.max_delay, Duration::from_secs(30));
        assert!(policy.retry_server_errors);
        assert!(policy.retryable_statuses.is_empty());
    }

    #[test]
    fn retry_policy_no_retry() {
        let policy = RetryPolicy::no_retry();
        assert_eq!(policy.max_retries, 0);
        let err = ApiError::RateLimited {
            retry_after_ms: 1000,
        };
        assert!(!policy.should_retry(&err, 0));
    }

    #[test]
    fn retry_policy_aggressive() {
        let policy = RetryPolicy::aggressive();
        assert_eq!(policy.max_retries, 5);
        assert_eq!(policy.base_delay, Duration::from_millis(200));
        assert_eq!(policy.max_delay, Duration::from_secs(10));
    }

    #[test]
    fn retry_policy_delay_for_attempt() {
        let policy = RetryPolicy::new();
        assert_eq!(policy.delay_for_attempt(0), Duration::from_millis(500));
        assert_eq!(policy.delay_for_attempt(1), Duration::from_millis(1000));
        assert_eq!(policy.delay_for_attempt(2), Duration::from_millis(2000));
    }

    #[test]
    fn retry_policy_delay_capped() {
        let policy = RetryPolicy::new();
        assert_eq!(policy.delay_for_attempt(10), Duration::from_secs(30));
    }

    #[test]
    fn retry_policy_should_retry_rate_limited() {
        let policy = RetryPolicy::new();
        let err = ApiError::RateLimited {
            retry_after_ms: 1000,
        };
        assert!(policy.should_retry(&err, 0));
        assert!(policy.should_retry(&err, 2));
        assert!(!policy.should_retry(&err, 3)); // exceeds max
    }

    #[test]
    fn retry_policy_should_retry_5xx() {
        let policy = RetryPolicy::new();
        let err = ApiError::Api {
            status: 500,
            message: "internal".into(),
        };
        assert!(policy.should_retry(&err, 0)); // retry_server_errors = true
    }

    #[test]
    fn retry_policy_no_5xx_when_disabled() {
        let policy = RetryPolicy {
            retry_server_errors: false,
            ..RetryPolicy::new()
        };
        let err = ApiError::Api {
            status: 500,
            message: "internal".into(),
        };
        assert!(!policy.should_retry(&err, 0));
    }

    #[test]
    fn retry_policy_custom_status() {
        let policy = RetryPolicy::new().with_retryable_status(418);
        let err = ApiError::Api {
            status: 418,
            message: "I'm a teapot".into(),
        };
        assert!(policy.should_retry(&err, 0));
    }

    #[test]
    fn retry_policy_timeout_is_retryable() {
        let policy = RetryPolicy::new();
        assert!(policy.should_retry(&ApiError::Timeout, 0));
    }

    #[test]
    fn retry_policy_json_not_retryable() {
        let policy = RetryPolicy::new();
        let err = ApiError::Sse("parse error".into());
        assert!(!policy.should_retry(&err, 0));
    }

    #[test]
    fn retry_policy_builder_chain() {
        let policy = RetryPolicy::new()
            .with_max_retries(5)
            .with_base_delay(Duration::from_millis(100))
            .with_max_delay(Duration::from_secs(10))
            .with_retryable_status(503);
        assert_eq!(policy.max_retries, 5);
        assert_eq!(policy.base_delay, Duration::from_millis(100));
        assert_eq!(policy.max_delay, Duration::from_secs(10));
        assert!(policy.retryable_statuses.contains(&503));
    }

    #[test]
    fn retry_policy_default_trait() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.max_retries, MAX_RETRIES);
    }

    #[test]
    fn retry_policy_aggressive_delays() {
        let policy = RetryPolicy::aggressive();
        assert_eq!(policy.delay_for_attempt(0), Duration::from_millis(200));
        assert_eq!(policy.delay_for_attempt(1), Duration::from_millis(400));
        assert_eq!(policy.delay_for_attempt(10), Duration::from_secs(10)); // capped
    }
}
