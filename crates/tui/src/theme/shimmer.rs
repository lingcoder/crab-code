//! Shimmer variant derivation.
//!
//! A shimmer pass "sweeps" a narrow bright segment across a base color.
//! Given a base color and a phase in `[0.0, 1.0)`, [`shimmer_at`] returns
//! a derived color for each column position. Callers drive `phase` over
//! time (typically `SHIMMER_INTERVAL_MS = 80`) to animate the sweep.

use std::time::Duration;

use ratatui::style::Color;

/// Default time between shimmer frames.
pub const SHIMMER_INTERVAL_MS: u64 = 80;

/// Default shimmer frame interval as a `Duration`.
pub const SHIMMER_INTERVAL: Duration = Duration::from_millis(SHIMMER_INTERVAL_MS);

/// Width (in columns) of the bright band in the sweep.
pub const SHIMMER_BAND_WIDTH: f32 = 6.0;

/// Factor added to each channel when a column is dead-center of the band.
/// Channels are clamped to `0..=255` after.
pub const SHIMMER_PEAK_LIFT: u8 = 60;

/// Return the shimmer color for a column within a line of `width`,
/// given a phase `t` in `[0.0, 1.0)`.
///
/// When `base` is a named ANSI color, we cannot brighten it
/// compositionally; we fall back to `base` unchanged. When `base` is an
/// RGB color, we return a lifted variant whose lift magnitude follows a
/// triangular falloff away from the current peak column.
#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
#[must_use]
pub fn shimmer_at(base: Color, column: u16, width: u16, t: f32) -> Color {
    let Color::Rgb(r, g, b) = base else {
        return base;
    };
    if width == 0 {
        return base;
    }
    let peak = t.rem_euclid(1.0) * f32::from(width);
    let distance = (f32::from(column) - peak).abs();
    let band = SHIMMER_BAND_WIDTH.max(1.0);
    if distance > band {
        return base;
    }
    let intensity = 1.0 - (distance / band);
    let lift_f = (f32::from(SHIMMER_PEAK_LIFT) * intensity).clamp(0.0, 255.0);
    let lift = lift_f as u16;
    let clamp = |c: u8| -> u8 {
        let lifted = u16::from(c).saturating_add(lift);
        lifted.min(255) as u8
    };
    Color::Rgb(clamp(r), clamp(g), clamp(b))
}

/// Compute shimmer colors for an entire span at phase `t`.
///
/// Returns one `Color` per column. Callers paint each glyph with the
/// corresponding color to produce the sweep effect.
#[must_use]
pub fn shimmer_segments(base: Color, width: u16, t: f32) -> Vec<Color> {
    (0..width)
        .map(|col| shimmer_at(base, col, width, t))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_color_shimmer_is_identity() {
        assert_eq!(shimmer_at(Color::Cyan, 0, 10, 0.0), Color::Cyan);
        assert_eq!(shimmer_at(Color::Red, 5, 10, 0.5), Color::Red);
    }

    #[test]
    fn rgb_outside_band_is_identity() {
        let base = Color::Rgb(100, 100, 100);
        let far = shimmer_at(base, 0, 100, 0.9);
        assert_eq!(far, base);
    }

    #[test]
    fn rgb_at_peak_is_brighter() {
        let base = Color::Rgb(100, 100, 100);
        let peak = shimmer_at(base, 50, 100, 0.5);
        match peak {
            Color::Rgb(r, g, b) => {
                assert!(r > 100);
                assert!(g > 100);
                assert!(b > 100);
            }
            _ => panic!("peak should be RGB"),
        }
    }

    #[test]
    fn rgb_clamps_at_255() {
        let base = Color::Rgb(250, 250, 250);
        let peak = shimmer_at(base, 50, 100, 0.5);
        match peak {
            Color::Rgb(r, g, b) => {
                assert_eq!(r, 255);
                assert_eq!(g, 255);
                assert_eq!(b, 255);
            }
            _ => panic!("peak should be RGB"),
        }
    }

    #[test]
    fn segments_length_matches_width() {
        let base = Color::Rgb(80, 120, 200);
        let segments = shimmer_segments(base, 15, 0.3);
        assert_eq!(segments.len(), 15);
    }

    #[test]
    fn zero_width_safe() {
        let base = Color::Rgb(80, 80, 80);
        assert_eq!(shimmer_at(base, 0, 0, 0.0), base);
        assert!(shimmer_segments(base, 0, 0.0).is_empty());
    }
}
