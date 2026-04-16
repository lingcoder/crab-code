//! `AutoDream` — periodic background "dream" cycles for the agent.
//!
//! The dream system is designed to let the agent periodically consolidate
//! context, prune stale information, or run background analysis.
//! The current implementation logs that the cycle was skipped because the
//! feature is not yet fully integrated.

use std::time::{Duration, Instant};

/// Default interval between dream cycles (10 minutes).
const DEFAULT_INTERVAL: Duration = Duration::from_secs(600);

/// Configuration for the auto-dream subsystem.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct AutoDreamConfig {
    /// How often a dream cycle should trigger.
    pub interval: Duration,
    /// Whether auto-dreaming is enabled.
    pub enabled: bool,
}

impl AutoDreamConfig {
    /// Create a new configuration with the given interval.
    #[allow(dead_code)]
    pub fn new(interval: Duration, enabled: bool) -> Self {
        Self { interval, enabled }
    }
}

impl Default for AutoDreamConfig {
    fn default() -> Self {
        Self {
            interval: DEFAULT_INTERVAL,
            enabled: false,
        }
    }
}

/// Runtime state for the auto-dream subsystem.
#[allow(dead_code)]
#[derive(Debug)]
pub struct AutoDream {
    /// Configuration.
    config: AutoDreamConfig,
    /// Timestamp of the last completed dream cycle.
    last_dream: Option<Instant>,
    /// Total number of dream cycles executed.
    cycle_count: u64,
}

#[allow(dead_code)]
impl AutoDream {
    /// Create a new `AutoDream` instance with the given configuration.
    pub fn new(config: AutoDreamConfig) -> Self {
        Self {
            config,
            last_dream: None,
            cycle_count: 0,
        }
    }

    /// Check whether enough time has passed since the last dream cycle.
    ///
    /// Returns `true` if auto-dream is enabled and the interval has elapsed
    /// (or no dream has ever been run).
    pub fn should_dream(&self) -> bool {
        if !self.config.enabled {
            return false;
        }
        match self.last_dream {
            Some(last) => last.elapsed() >= self.config.interval,
            None => true,
        }
    }

    /// Run a dream cycle.
    ///
    /// Currently logs that the cycle was skipped because the feature is
    /// not yet integrated. Updates internal bookkeeping regardless.
    pub fn run_dream_cycle(&mut self) {
        tracing::info!(
            cycle = self.cycle_count + 1,
            "dream cycle skipped (not yet integrated)"
        );
        self.last_dream = Some(Instant::now());
        self.cycle_count += 1;
    }

    /// The number of dream cycles that have been executed.
    pub fn cycle_count(&self) -> u64 {
        self.cycle_count
    }

    /// Whether auto-dream is enabled.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Access the current configuration.
    pub fn config(&self) -> &AutoDreamConfig {
        &self.config
    }

    /// Update the configuration at runtime.
    pub fn set_config(&mut self, config: AutoDreamConfig) {
        self.config = config;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_disabled() {
        let config = AutoDreamConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.interval, DEFAULT_INTERVAL);
    }

    #[test]
    fn should_dream_when_enabled_and_never_run() {
        let config = AutoDreamConfig::new(Duration::from_secs(1), true);
        let dream = AutoDream::new(config);
        assert!(dream.should_dream());
    }

    #[test]
    fn should_not_dream_when_disabled() {
        let config = AutoDreamConfig::new(Duration::from_secs(0), false);
        let dream = AutoDream::new(config);
        assert!(!dream.should_dream());
    }

    #[test]
    fn should_not_dream_immediately_after_cycle() {
        let config = AutoDreamConfig::new(Duration::from_secs(3600), true);
        let mut dream = AutoDream::new(config);
        dream.run_dream_cycle();
        assert!(!dream.should_dream());
    }

    #[test]
    fn should_dream_after_zero_interval_cycle() {
        let config = AutoDreamConfig::new(Duration::ZERO, true);
        let mut dream = AutoDream::new(config);
        dream.run_dream_cycle();
        // With zero interval, should_dream should be true immediately.
        assert!(dream.should_dream());
    }

    #[test]
    fn cycle_count_increments() {
        let config = AutoDreamConfig::new(Duration::from_secs(1), true);
        let mut dream = AutoDream::new(config);
        assert_eq!(dream.cycle_count(), 0);
        dream.run_dream_cycle();
        assert_eq!(dream.cycle_count(), 1);
        dream.run_dream_cycle();
        assert_eq!(dream.cycle_count(), 2);
    }

    #[test]
    fn set_config_updates_state() {
        let mut dream = AutoDream::new(AutoDreamConfig::default());
        assert!(!dream.is_enabled());

        dream.set_config(AutoDreamConfig::new(Duration::from_secs(30), true));
        assert!(dream.is_enabled());
        assert_eq!(dream.config().interval, Duration::from_secs(30));
    }
}
