//! Frame-driven spinner.
//!
//! A `Spinner` wraps a static frame set plus a rotation period. Calling
//! [`Spinner::frame_at`] with the wall clock returns the glyph to render
//! at that moment. The scheduler decides when to redraw; the spinner
//! itself is stateless aside from its configuration.

use std::time::{Duration, Instant};

/// Classic braille rotor. 10 frames, evenly spaced.
pub const BRAILLE_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Small dots alternating around a glyph center.
pub const DOTS_FRAMES: &[&str] = &["·", "•", "●", "•"];

/// Plain ASCII spinner for terminals that lack unicode support.
pub const LINE_FRAMES: &[&str] = &["|", "/", "-", "\\"];

/// The set of built-in spinner styles.
#[derive(Debug, Clone, Copy)]
pub enum SpinnerStyle {
    Braille,
    Dots,
    Line,
    Custom,
}

/// A single spinner instance.
#[derive(Debug, Clone)]
pub struct Spinner {
    frames: &'static [&'static str],
    interval: Duration,
    style: SpinnerStyle,
    start: Instant,
}

impl Spinner {
    #[must_use]
    pub fn braille() -> Self {
        Self::new(
            BRAILLE_FRAMES,
            Duration::from_millis(80),
            SpinnerStyle::Braille,
        )
    }

    #[must_use]
    pub fn dots() -> Self {
        Self::new(DOTS_FRAMES, Duration::from_millis(150), SpinnerStyle::Dots)
    }

    #[must_use]
    pub fn line() -> Self {
        Self::new(LINE_FRAMES, Duration::from_millis(120), SpinnerStyle::Line)
    }

    /// Build a spinner from a custom frame set and period.
    ///
    /// `frames` must contain at least one entry.
    #[must_use]
    pub fn custom(frames: &'static [&'static str], interval: Duration) -> Self {
        assert!(!frames.is_empty(), "Spinner frames must not be empty");
        Self::new(frames, interval, SpinnerStyle::Custom)
    }

    fn new(frames: &'static [&'static str], interval: Duration, style: SpinnerStyle) -> Self {
        Self {
            frames,
            interval,
            style,
            start: Instant::now(),
        }
    }

    /// Force the animation's phase zero point to `now`. Call this when a
    /// spinner should appear to start on a fresh cycle.
    pub fn restart(&mut self, now: Instant) {
        self.start = now;
    }

    #[must_use]
    pub fn style(&self) -> SpinnerStyle {
        self.style
    }

    #[must_use]
    pub fn frames(&self) -> &'static [&'static str] {
        self.frames
    }

    #[must_use]
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// Current frame for `now`. Returns the glyph as `&'static str`.
    #[must_use]
    pub fn frame_at(&self, now: Instant) -> &'static str {
        let elapsed = now.saturating_duration_since(self.start);
        let elapsed_ticks = if self.interval.is_zero() {
            0
        } else {
            (elapsed.as_nanos() / self.interval.as_nanos()) as usize
        };
        self.frames[elapsed_ticks % self.frames.len()]
    }
}

impl Default for Spinner {
    fn default() -> Self {
        Self::braille()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn braille_frame_cycles() {
        let s = Spinner::braille();
        let t0 = Instant::now();
        assert_eq!(s.frame_at(t0), BRAILLE_FRAMES[0]);

        let next = t0 + Duration::from_millis(80);
        assert_eq!(s.frame_at(next), BRAILLE_FRAMES[1]);

        let wrapped = t0 + Duration::from_millis(80 * BRAILLE_FRAMES.len() as u64);
        assert_eq!(s.frame_at(wrapped), BRAILLE_FRAMES[0]);
    }

    #[test]
    fn custom_frames_work() {
        static F: &[&str] = &["a", "b", "c"];
        let s = Spinner::custom(F, Duration::from_millis(10));
        let t0 = Instant::now();
        assert_eq!(s.frame_at(t0), "a");
        assert_eq!(s.frame_at(t0 + Duration::from_millis(10)), "b");
        assert_eq!(s.frame_at(t0 + Duration::from_millis(30)), "a");
    }

    #[test]
    fn restart_resets_phase() {
        let mut s = Spinner::line();
        let t0 = Instant::now();
        // Move time forward to "advance" a frame or two.
        s.restart(t0 + Duration::from_millis(500));
        assert_eq!(s.frame_at(t0 + Duration::from_millis(500)), LINE_FRAMES[0]);
    }

    #[test]
    fn zero_interval_is_safe() {
        static F: &[&str] = &["x", "y"];
        let s = Spinner::custom(F, Duration::from_millis(0));
        assert_eq!(s.frame_at(Instant::now()), "x");
    }

    #[test]
    #[should_panic]
    fn empty_custom_panics() {
        static E: &[&str] = &[];
        let _ = Spinner::custom(E, Duration::from_millis(50));
    }
}
