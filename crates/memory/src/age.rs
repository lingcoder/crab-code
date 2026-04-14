//! Memory aging: decay relevance over time.
//!
//! Memories lose relevance as they age. This module exposes small building
//! blocks:
//!
//! - [`age_days`] — whole-day age from a [`SystemTime`]
//! - [`decay_score`] — exponential decay with a 30-day half-life
//! - [`age_text`] — human-readable age ("today", "yesterday", "N days ago")
//! - [`freshness_caveat`] — staleness hint for memories older than one day

use std::time::SystemTime;

/// Half-life in days for exponential decay. A memory is scored 0.5 after
/// this many days, ~0.25 after double, etc.
const HALF_LIFE_DAYS: f64 = 30.0;

/// Seconds in a day.
const SECONDS_PER_DAY: u64 = 86_400;

/// Compute whole-day age from `mtime` to now.
///
/// Returns `0` when `mtime` is in the future or equal to now.
#[must_use]
pub fn age_days(mtime: SystemTime) -> u64 {
    match SystemTime::now().duration_since(mtime) {
        Ok(d) => d.as_secs() / SECONDS_PER_DAY,
        Err(_) => 0,
    }
}

/// Exponential-decay score for a given age: `0.5 ^ (days / 30)`.
///
/// Returns `1.0` at day 0 and halves every 30 days.
#[must_use]
pub fn decay_score(days: u64) -> f64 {
    0.5_f64.powf(days as f64 / HALF_LIFE_DAYS)
}

/// Human-readable age string.
///
/// - `0` → `"today"`
/// - `1` → `"yesterday"`
/// - `n` → `"N days ago"`
#[must_use]
pub fn age_text(days: u64) -> String {
    match days {
        0 => "today".to_string(),
        1 => "yesterday".to_string(),
        n => format!("{n} days ago"),
    }
}

/// Staleness caveat for memories older than one day.
///
/// Returns `None` when `days <= 1`. Otherwise returns a short English
/// sentence reminding the reader that the memory is a point-in-time
/// observation and should be verified against current code.
#[must_use]
pub fn freshness_caveat(days: u64) -> Option<String> {
    if days <= 1 {
        None
    } else {
        Some(format!(
            "This memory is {}. Memories are point-in-time observations. \
             Verify against current code before asserting as fact.",
            age_text(days)
        ))
    }
}

// ─── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn age_days_now_is_zero() {
        let now = SystemTime::now();
        assert_eq!(age_days(now), 0);
    }

    #[test]
    fn age_days_30_days_ago() {
        let past = SystemTime::now() - Duration::from_secs(30 * SECONDS_PER_DAY);
        // Allow ±1 day slack for rounding across the test execution boundary.
        let days = age_days(past);
        assert!((29..=30).contains(&days), "expected ~30 days, got {days}");
    }

    #[test]
    fn age_days_future_clamps_to_zero() {
        let future = SystemTime::now() + Duration::from_secs(10 * SECONDS_PER_DAY);
        assert_eq!(age_days(future), 0);
    }

    #[test]
    fn decay_score_day_zero_is_one() {
        assert!((decay_score(0) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn decay_score_30_days_is_half() {
        assert!((decay_score(30) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn decay_score_60_days_is_quarter() {
        assert!((decay_score(60) - 0.25).abs() < 1e-9);
    }

    #[test]
    fn age_text_variants() {
        assert_eq!(age_text(0), "today");
        assert_eq!(age_text(1), "yesterday");
        assert_eq!(age_text(47), "47 days ago");
    }

    #[test]
    fn freshness_caveat_fresh() {
        assert!(freshness_caveat(0).is_none());
        assert!(freshness_caveat(1).is_none());
    }

    #[test]
    fn freshness_caveat_stale() {
        let text = freshness_caveat(5).expect("stale memory should produce a caveat");
        assert!(text.contains("5 days ago"));
        assert!(text.contains("Verify"));
    }
}
