//! Automatic retry strategies for failed agent tasks.
//!
//! When a worker fails, the retry policy decides whether to re-assign
//! the task and how long to wait before retrying.

use std::time::Duration;

/// Outcome of a retry decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryDecision {
    /// Retry the task after the specified delay.
    Retry { delay: Duration, attempt: u32 },
    /// Do not retry — max attempts exhausted.
    GiveUp { attempts_made: u32 },
}

/// Configuration for retry behavior.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Maximum number of retry attempts (0 = no retries).
    pub max_retries: u32,
    /// Base delay between retries.
    pub base_delay: Duration,
    /// Backoff strategy.
    pub backoff: BackoffStrategy,
    /// Maximum delay cap (prevents exponential from growing unbounded).
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 2,
            base_delay: Duration::from_secs(1),
            backoff: BackoffStrategy::Exponential { factor: 2.0 },
            max_delay: Duration::from_secs(60),
        }
    }
}

/// Backoff strategy for computing delay between retries.
#[derive(Debug, Clone)]
pub enum BackoffStrategy {
    /// Fixed delay — same wait every time.
    Fixed,
    /// Linear — delay = `base_delay` * attempt.
    Linear,
    /// Exponential — delay = `base_delay` * factor^(attempt-1).
    Exponential { factor: f64 },
}

impl RetryPolicy {
    /// Create a policy that never retries.
    #[must_use]
    pub fn no_retry() -> Self {
        Self {
            max_retries: 0,
            ..Default::default()
        }
    }

    /// Create a policy with a fixed number of retries and constant delay.
    #[must_use]
    pub fn fixed(max_retries: u32, delay: Duration) -> Self {
        Self {
            max_retries,
            base_delay: delay,
            backoff: BackoffStrategy::Fixed,
            max_delay: delay,
        }
    }

    /// Create a policy with exponential backoff.
    #[must_use]
    pub fn exponential(max_retries: u32, base_delay: Duration, factor: f64) -> Self {
        Self {
            max_retries,
            base_delay,
            backoff: BackoffStrategy::Exponential { factor },
            max_delay: Duration::from_secs(300),
        }
    }

    /// Decide whether to retry given the current attempt number (1-based).
    ///
    /// `attempts_so_far` is how many times the task has already been attempted
    /// (including the initial try). So after the first failure, `attempts_so_far = 1`.
    #[must_use]
    pub fn should_retry(&self, attempts_so_far: u32) -> RetryDecision {
        if attempts_so_far > self.max_retries {
            return RetryDecision::GiveUp {
                attempts_made: attempts_so_far,
            };
        }

        let delay = self.compute_delay(attempts_so_far);
        RetryDecision::Retry {
            delay,
            attempt: attempts_so_far + 1,
        }
    }

    /// Compute the delay for the given attempt number.
    fn compute_delay(&self, attempt: u32) -> Duration {
        let raw = match &self.backoff {
            BackoffStrategy::Fixed => self.base_delay,
            BackoffStrategy::Linear => self.base_delay * attempt,
            BackoffStrategy::Exponential { factor } => {
                let multiplier =
                    factor.powi(i32::try_from(attempt.saturating_sub(1)).unwrap_or(i32::MAX));
                self.base_delay.mul_f64(multiplier)
            }
        };
        raw.min(self.max_delay)
    }
}

/// Tracks retry state for a specific task.
#[derive(Debug, Clone)]
pub struct RetryTracker {
    policy: RetryPolicy,
    /// Map of `task_id` → attempts made so far.
    attempts: std::collections::HashMap<String, u32>,
}

impl RetryTracker {
    /// Create a tracker with the given policy.
    #[must_use]
    pub fn new(policy: RetryPolicy) -> Self {
        Self {
            policy,
            attempts: std::collections::HashMap::new(),
        }
    }

    /// Record a task failure and decide whether to retry.
    pub fn on_failure(&mut self, task_id: &str) -> RetryDecision {
        let attempts = self.attempts.entry(task_id.to_string()).or_insert(0);
        *attempts += 1;
        self.policy.should_retry(*attempts)
    }

    /// Record a task success (clears retry state for that task).
    pub fn on_success(&mut self, task_id: &str) {
        self.attempts.remove(task_id);
    }

    /// Get the number of attempts made for a task.
    #[must_use]
    pub fn attempts_for(&self, task_id: &str) -> u32 {
        self.attempts.get(task_id).copied().unwrap_or(0)
    }

    /// Check if a task has exhausted all retries.
    #[must_use]
    pub fn is_exhausted(&self, task_id: &str) -> bool {
        self.attempts
            .get(task_id)
            .is_some_and(|a| *a > self.policy.max_retries)
    }

    /// Clear retry state for a specific task.
    pub fn clear(&mut self, task_id: &str) {
        self.attempts.remove(task_id);
    }

    /// Clear all retry state.
    pub fn clear_all(&mut self) {
        self.attempts.clear();
    }

    /// Get the retry policy.
    #[must_use]
    pub fn policy(&self) -> &RetryPolicy {
        &self.policy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── RetryPolicy ───

    #[test]
    fn default_policy() {
        let p = RetryPolicy::default();
        assert_eq!(p.max_retries, 2);
        assert_eq!(p.base_delay, Duration::from_secs(1));
    }

    #[test]
    fn no_retry_policy() {
        let p = RetryPolicy::no_retry();
        assert_eq!(p.max_retries, 0);
        let d = p.should_retry(1);
        assert_eq!(d, RetryDecision::GiveUp { attempts_made: 1 });
    }

    #[test]
    fn fixed_policy() {
        let p = RetryPolicy::fixed(3, Duration::from_secs(5));
        assert_eq!(p.max_retries, 3);

        // Attempt 1: retry with 5s delay
        let d = p.should_retry(1);
        assert_eq!(
            d,
            RetryDecision::Retry {
                delay: Duration::from_secs(5),
                attempt: 2,
            }
        );

        // Attempt 3: still retry
        let d = p.should_retry(3);
        assert_eq!(
            d,
            RetryDecision::Retry {
                delay: Duration::from_secs(5),
                attempt: 4,
            }
        );

        // Attempt 4: give up (> max_retries)
        let d = p.should_retry(4);
        assert_eq!(d, RetryDecision::GiveUp { attempts_made: 4 });
    }

    #[test]
    fn linear_backoff() {
        let p = RetryPolicy {
            max_retries: 5,
            base_delay: Duration::from_secs(2),
            backoff: BackoffStrategy::Linear,
            max_delay: Duration::from_secs(60),
        };

        // Attempt 1: 2s * 1 = 2s
        let d = p.should_retry(1);
        assert_eq!(
            d,
            RetryDecision::Retry {
                delay: Duration::from_secs(2),
                attempt: 2,
            }
        );

        // Attempt 3: 2s * 3 = 6s
        let d = p.should_retry(3);
        assert_eq!(
            d,
            RetryDecision::Retry {
                delay: Duration::from_secs(6),
                attempt: 4,
            }
        );
    }

    #[test]
    fn exponential_backoff() {
        let p = RetryPolicy::exponential(5, Duration::from_secs(1), 2.0);

        // Attempt 1: 1s * 2^0 = 1s
        let d = p.should_retry(1);
        assert_eq!(
            d,
            RetryDecision::Retry {
                delay: Duration::from_secs(1),
                attempt: 2,
            }
        );

        // Attempt 2: 1s * 2^1 = 2s
        let d = p.should_retry(2);
        assert_eq!(
            d,
            RetryDecision::Retry {
                delay: Duration::from_secs(2),
                attempt: 3,
            }
        );

        // Attempt 3: 1s * 2^2 = 4s
        let d = p.should_retry(3);
        assert_eq!(
            d,
            RetryDecision::Retry {
                delay: Duration::from_secs(4),
                attempt: 4,
            }
        );
    }

    #[test]
    fn max_delay_cap() {
        let p = RetryPolicy {
            max_retries: 20,
            base_delay: Duration::from_secs(1),
            backoff: BackoffStrategy::Exponential { factor: 10.0 },
            max_delay: Duration::from_secs(30),
        };

        // Attempt 5: 1s * 10^4 = 10000s → capped at 30s
        let d = p.should_retry(5);
        assert_eq!(
            d,
            RetryDecision::Retry {
                delay: Duration::from_secs(30),
                attempt: 6,
            }
        );
    }

    #[test]
    fn give_up_after_max() {
        let p = RetryPolicy::default(); // max_retries = 2

        assert!(matches!(p.should_retry(1), RetryDecision::Retry { .. }));
        assert!(matches!(p.should_retry(2), RetryDecision::Retry { .. }));
        assert!(matches!(p.should_retry(3), RetryDecision::GiveUp { .. }));
    }

    // ─── RetryTracker ───

    #[test]
    fn tracker_new() {
        let tracker = RetryTracker::new(RetryPolicy::default());
        assert_eq!(tracker.attempts_for("t1"), 0);
        assert!(!tracker.is_exhausted("t1"));
    }

    #[test]
    fn tracker_failure_and_retry() {
        let mut tracker = RetryTracker::new(RetryPolicy::fixed(2, Duration::from_secs(1)));

        let d = tracker.on_failure("t1");
        assert!(matches!(d, RetryDecision::Retry { attempt: 2, .. }));
        assert_eq!(tracker.attempts_for("t1"), 1);

        let d = tracker.on_failure("t1");
        assert!(matches!(d, RetryDecision::Retry { attempt: 3, .. }));
        assert_eq!(tracker.attempts_for("t1"), 2);

        let d = tracker.on_failure("t1");
        assert_eq!(d, RetryDecision::GiveUp { attempts_made: 3 });
        assert!(tracker.is_exhausted("t1"));
    }

    #[test]
    fn tracker_success_clears_state() {
        let mut tracker = RetryTracker::new(RetryPolicy::default());
        tracker.on_failure("t1");
        assert_eq!(tracker.attempts_for("t1"), 1);

        tracker.on_success("t1");
        assert_eq!(tracker.attempts_for("t1"), 0);
        assert!(!tracker.is_exhausted("t1"));
    }

    #[test]
    fn tracker_independent_tasks() {
        let mut tracker = RetryTracker::new(RetryPolicy::fixed(2, Duration::from_secs(1)));

        tracker.on_failure("t1");
        tracker.on_failure("t1");
        tracker.on_failure("t2");

        assert_eq!(tracker.attempts_for("t1"), 2);
        assert_eq!(tracker.attempts_for("t2"), 1);
    }

    #[test]
    fn tracker_clear() {
        let mut tracker = RetryTracker::new(RetryPolicy::default());
        tracker.on_failure("t1");
        tracker.on_failure("t2");

        tracker.clear("t1");
        assert_eq!(tracker.attempts_for("t1"), 0);
        assert_eq!(tracker.attempts_for("t2"), 1);
    }

    #[test]
    fn tracker_clear_all() {
        let mut tracker = RetryTracker::new(RetryPolicy::default());
        tracker.on_failure("t1");
        tracker.on_failure("t2");

        tracker.clear_all();
        assert_eq!(tracker.attempts_for("t1"), 0);
        assert_eq!(tracker.attempts_for("t2"), 0);
    }

    #[test]
    fn tracker_policy_accessor() {
        let policy = RetryPolicy::fixed(5, Duration::from_secs(10));
        let tracker = RetryTracker::new(policy);
        assert_eq!(tracker.policy().max_retries, 5);
    }

    #[test]
    fn retry_decision_equality() {
        let a = RetryDecision::Retry {
            delay: Duration::from_secs(1),
            attempt: 2,
        };
        let b = RetryDecision::Retry {
            delay: Duration::from_secs(1),
            attempt: 2,
        };
        assert_eq!(a, b);

        let c = RetryDecision::GiveUp { attempts_made: 3 };
        let d = RetryDecision::GiveUp { attempts_made: 3 };
        assert_eq!(c, d);
        assert_ne!(a, c);
    }
}
