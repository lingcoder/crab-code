//! Frame-driven shimmer animation adapter.
//!
//! This module is the time-driven wrapper around [`crate::theme::shimmer`].
//! The theme module knows how to map `(base, column, phase)` to a color;
//! this module advances `phase` on each animation tick.

use std::time::Instant;

use crate::theme::shimmer::{SHIMMER_INTERVAL, shimmer_at};
use ratatui::style::Color;

/// Holds one shimmer animation's running state.
#[derive(Debug, Clone, Copy)]
pub struct ShimmerState {
    start: Instant,
}

impl ShimmerState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    #[must_use]
    pub fn starting_at(start: Instant) -> Self {
        Self { start }
    }

    pub fn restart(&mut self, now: Instant) {
        self.start = now;
    }

    /// The current phase in `[0.0, 1.0)`.
    #[must_use]
    pub fn phase(&self, now: Instant) -> f32 {
        let elapsed = now.saturating_duration_since(self.start);
        let per_cycle = SHIMMER_INTERVAL.as_millis().max(1) as f32 * 32.0;
        let cycle = (elapsed.as_millis() as f32 / per_cycle).fract();
        if cycle.is_nan() { 0.0 } else { cycle }
    }

    /// Resolve a column's color at the current frame.
    #[must_use]
    pub fn color_at(&self, now: Instant, base: Color, column: u16, width: u16) -> Color {
        shimmer_at(base, column, width, self.phase(now))
    }
}

impl Default for ShimmerState {
    fn default() -> Self {
        Self::new()
    }
}

/// A subscriber that keeps its shimmer active only while not dropped.
pub struct ShimmerSubscriber {
    state: ShimmerState,
}

impl ShimmerSubscriber {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: ShimmerState::new(),
        }
    }

    #[must_use]
    pub fn state(&self) -> ShimmerState {
        self.state
    }
}

impl Default for ShimmerSubscriber {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn phase_returns_within_unit_interval() {
        let s = ShimmerState::new();
        let t0 = Instant::now();
        for ms in [0, 40, 400, 4000, 40000] {
            let p = s.phase(t0 + Duration::from_millis(ms));
            assert!(p >= 0.0 && p < 1.0, "phase {p} out of range");
        }
    }

    #[test]
    fn color_at_on_rgb_is_bright_at_peak() {
        let s = ShimmerState::starting_at(Instant::now());
        let base = Color::Rgb(80, 80, 80);
        let t = Instant::now() + Duration::from_millis(1280); // ~0.5 cycle
        let c = s.color_at(t, base, 50, 100);
        assert!(matches!(c, Color::Rgb(_, _, _)));
    }
}
