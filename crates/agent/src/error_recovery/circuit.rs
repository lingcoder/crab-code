use std::time::{Duration, Instant};

// ── Circuit breaker ───────────────────────────────────────────────────

/// State of a circuit breaker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation — requests flow through.
    Closed,
    /// Tripped — requests are blocked.
    Open,
    /// Partially open — allowing a single probe request.
    HalfOpen,
}

impl std::fmt::Display for CircuitState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => write!(f, "closed"),
            Self::Open => write!(f, "open"),
            Self::HalfOpen => write!(f, "half_open"),
        }
    }
}

/// Circuit breaker configuration.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of consecutive failures before opening the circuit.
    pub failure_threshold: u32,
    /// Duration to keep the circuit open before allowing a probe.
    pub open_duration: Duration,
    /// Number of successes in half-open state to close the circuit.
    pub success_threshold: u32,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            open_duration: Duration::from_secs(30),
            success_threshold: 2,
        }
    }
}

/// Circuit breaker: trips after consecutive failures, blocks requests
/// until a cooldown period passes, then allows probe requests.
#[derive(Debug)]
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    state: CircuitState,
    consecutive_failures: u32,
    half_open_successes: u32,
    last_failure_time: Option<Instant>,
    total_trips: u32,
}

impl CircuitBreaker {
    /// Create a new circuit breaker with default config.
    #[must_use]
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            state: CircuitState::Closed,
            consecutive_failures: 0,
            half_open_successes: 0,
            last_failure_time: None,
            total_trips: 0,
        }
    }

    /// Check if a request should be allowed.
    ///
    /// Returns `true` if the circuit allows the request.
    /// Transitions from Open to `HalfOpen` if the cooldown has elapsed.
    pub fn allow_request(&mut self) -> bool {
        match self.state {
            CircuitState::Closed | CircuitState::HalfOpen => true,
            CircuitState::Open => {
                if let Some(last_fail) = self.last_failure_time {
                    if last_fail.elapsed() >= self.config.open_duration {
                        self.state = CircuitState::HalfOpen;
                        self.half_open_successes = 0;
                        true
                    } else {
                        false
                    }
                } else {
                    // No failure recorded — shouldn't be open, reset
                    self.state = CircuitState::Closed;
                    true
                }
            }
        }
    }

    /// Record a successful request.
    pub fn record_success(&mut self) {
        match self.state {
            CircuitState::Closed => {
                self.consecutive_failures = 0;
            }
            CircuitState::HalfOpen => {
                self.half_open_successes += 1;
                if self.half_open_successes >= self.config.success_threshold {
                    self.state = CircuitState::Closed;
                    self.consecutive_failures = 0;
                }
            }
            CircuitState::Open => {
                // Shouldn't happen if allow_request is checked first
            }
        }
    }

    /// Record a failed request.
    pub fn record_failure(&mut self) {
        self.last_failure_time = Some(Instant::now());
        match self.state {
            CircuitState::Closed => {
                self.consecutive_failures += 1;
                if self.consecutive_failures >= self.config.failure_threshold {
                    self.state = CircuitState::Open;
                    self.total_trips += 1;
                }
            }
            CircuitState::HalfOpen => {
                // Probe failed — reopen
                self.state = CircuitState::Open;
                self.total_trips += 1;
            }
            CircuitState::Open => {
                // Already open
            }
        }
    }

    /// Get the current circuit state.
    #[must_use]
    pub fn state(&self) -> CircuitState {
        self.state
    }

    /// Get the number of consecutive failures.
    #[must_use]
    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }

    /// Get the total number of times the circuit has tripped.
    #[must_use]
    pub fn total_trips(&self) -> u32 {
        self.total_trips
    }

    /// Manually reset the circuit to closed state.
    pub fn reset(&mut self) {
        self.state = CircuitState::Closed;
        self.consecutive_failures = 0;
        self.half_open_successes = 0;
        self.last_failure_time = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── CircuitState ───────────────────────────────────────────────

    #[test]
    fn circuit_state_display() {
        assert_eq!(CircuitState::Closed.to_string(), "closed");
        assert_eq!(CircuitState::Open.to_string(), "open");
        assert_eq!(CircuitState::HalfOpen.to_string(), "half_open");
    }

    // ── CircuitBreaker ─────────────────────────────────────────────

    #[test]
    fn breaker_starts_closed() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig::default());
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.consecutive_failures(), 0);
        assert_eq!(cb.total_trips(), 0);
    }

    #[test]
    fn breaker_allows_when_closed() {
        let mut cb = CircuitBreaker::new(CircuitBreakerConfig::default());
        assert!(cb.allow_request());
    }

    #[test]
    fn breaker_trips_after_threshold() {
        let mut cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 3,
            ..Default::default()
        });

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        assert_eq!(cb.total_trips(), 1);
    }

    #[test]
    fn breaker_blocks_when_open() {
        let mut cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 1,
            open_duration: Duration::from_secs(60),
            ..Default::default()
        });

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(!cb.allow_request());
    }

    #[test]
    fn breaker_success_resets_count() {
        let mut cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 3,
            ..Default::default()
        });

        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.consecutive_failures(), 2);

        cb.record_success();
        assert_eq!(cb.consecutive_failures(), 0);
    }

    #[test]
    fn breaker_half_open_success_closes() {
        let mut cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 1,
            open_duration: Duration::from_millis(0),
            success_threshold: 2,
        });

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Wait for open_duration (0ms) and allow request -> half_open
        assert!(cb.allow_request());
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        cb.record_success();
        assert_eq!(cb.state(), CircuitState::HalfOpen); // need 2 successes
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn breaker_half_open_failure_reopens() {
        let mut cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 1,
            open_duration: Duration::from_millis(0),
            success_threshold: 2,
        });

        cb.record_failure();
        assert!(cb.allow_request()); // transitions to HalfOpen
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        cb.record_failure(); // probe failed
        assert_eq!(cb.state(), CircuitState::Open);
        assert_eq!(cb.total_trips(), 2);
    }

    #[test]
    fn breaker_reset() {
        let mut cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 1,
            ..Default::default()
        });

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        cb.reset();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.consecutive_failures(), 0);
    }
}
