//! OSC 10 / OSC 11 system-color detection.
//!
//! Most modern terminals reply to an Operating System Control query
//! with their default foreground (OSC 10) and background (OSC 11)
//! color. We use the reply to pick between a dark and a light theme at
//! startup without forcing the user to configure one.
//!
//! The detection is strictly best-effort:
//!
//! - If the terminal does not reply within `DEFAULT_TIMEOUT`, we return
//!   [`Detection::Unknown`] and the caller falls back to the configured
//!   theme.
//! - If the reply is present but unparseable, we return
//!   [`Detection::Unknown`] as well.
//!
//! We deliberately do NOT sniff `COLORFGBG`, `TERM_PROGRAM`, or
//! `$COLORTERM` here — those are caller concerns and should layer on top
//! of the OSC reply.

use std::io::Read;
use std::time::{Duration, Instant};

/// Default wait window for the terminal reply.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(80);

/// Result of a detection attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detection {
    Dark,
    Light,
    Unknown,
}

/// RGB triple parsed from an OSC reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    /// Perceived luminance in `0.0..=1.0` using the Rec. 601 weights.
    #[must_use]
    pub fn luminance(&self) -> f32 {
        let (r, g, b) = (
            f32::from(self.r) / 255.0,
            f32::from(self.g) / 255.0,
            f32::from(self.b) / 255.0,
        );
        0.299f32.mul_add(r, 0.587f32.mul_add(g, 0.114 * b))
    }
}

/// Parse an OSC reply payload such as `rgb:RRRR/GGGG/BBBB` or
/// `rgb:RR/GG/BB`.
#[must_use]
pub fn parse_osc_rgb(payload: &str) -> Option<Rgb> {
    let s = payload.trim();
    let body = s.strip_prefix("rgb:")?;
    let parts: Vec<&str> = body.split('/').collect();
    if parts.len() != 3 {
        return None;
    }
    let parse_channel = |c: &str| -> Option<u8> {
        let hex = c.trim();
        if hex.is_empty() || hex.len() > 4 {
            return None;
        }
        let value = u32::from_str_radix(hex, 16).ok()?;
        // Normalize arbitrary-width hex down to u8. `ffff` => 255, `ff` => 255,
        // `f0` => 240. This matches xterm-style scaling.
        let shift = (hex.len() * 4).saturating_sub(8);
        Some((value >> shift) as u8)
    };
    Some(Rgb {
        r: parse_channel(parts[0])?,
        g: parse_channel(parts[1])?,
        b: parse_channel(parts[2])?,
    })
}

/// Decide dark/light from a background RGB.
#[must_use]
pub fn classify(bg: Rgb) -> Detection {
    if bg.luminance() < 0.5 {
        Detection::Dark
    } else {
        Detection::Light
    }
}

/// Query the terminal's OSC 11 background color.
///
/// This takes control of stdin for the duration of the read. It assumes
/// the terminal is in raw mode; in cooked mode the reply will be echoed
/// and this function will still work but may produce visible noise.
///
/// On any I/O error or timeout, [`Detection::Unknown`] is returned.
///
/// On Windows, raw stdin read is blocking and the terminal may not
/// respond to OSC 11 (causing a hang until the user presses a key).
/// We skip detection entirely on Windows and return `Unknown`.
pub fn detect_background(timeout: Duration) -> Detection {
    if cfg!(windows) {
        return Detection::Unknown;
    }
    detect_with_io(timeout, std::io::stdout(), std::io::stdin())
}

/// Inner entry point split for testability.
pub fn detect_with_io<W: std::io::Write, R: Read>(
    timeout: Duration,
    mut out: W,
    mut input: R,
) -> Detection {
    // Send OSC 11 query.
    if out.write_all(b"\x1b]11;?\x07").is_err() {
        return Detection::Unknown;
    }
    if out.flush().is_err() {
        return Detection::Unknown;
    }

    let deadline = Instant::now() + timeout;
    let mut buf = Vec::with_capacity(64);
    let mut chunk = [0u8; 32];
    while Instant::now() < deadline {
        match input.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.contains(&0x07) || buf.windows(2).any(|w| w == [0x1b, 0x5c]) {
                    break;
                }
            }
            Err(_) => return Detection::Unknown,
        }
    }

    let Some(start) = find_subsequence(&buf, b"\x1b]11;") else {
        return Detection::Unknown;
    };
    let tail = &buf[start + 5..];
    let end = tail
        .iter()
        .position(|&b| b == 0x07)
        .or_else(|| tail.windows(2).position(|w| w == [0x1b, 0x5c]))
        .unwrap_or(tail.len());
    let Ok(payload) = std::str::from_utf8(&tail[..end]) else {
        return Detection::Unknown;
    };
    match parse_osc_rgb(payload) {
        Some(rgb) => classify(rgb),
        None => Detection::Unknown,
    }
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn parses_short_rgb() {
        let rgb = parse_osc_rgb("rgb:00/00/00").unwrap();
        assert_eq!(rgb, Rgb { r: 0, g: 0, b: 0 });
    }

    #[test]
    fn parses_full_rgb_ffff() {
        let rgb = parse_osc_rgb("rgb:ffff/ffff/ffff").unwrap();
        assert_eq!(
            rgb,
            Rgb {
                r: 255,
                g: 255,
                b: 255
            }
        );
    }

    #[test]
    fn parses_uneven_rgb() {
        let rgb = parse_osc_rgb("rgb:f0/a0/20").unwrap();
        assert_eq!(
            rgb,
            Rgb {
                r: 240,
                g: 160,
                b: 32
            }
        );
    }

    #[test]
    fn rejects_malformed() {
        assert!(parse_osc_rgb("rgb:xxxx/yyyy/zzzz").is_none());
        assert!(parse_osc_rgb("not_rgb").is_none());
        assert!(parse_osc_rgb("rgb:ff/00").is_none());
    }

    #[test]
    fn classify_dark_and_light() {
        assert_eq!(classify(Rgb { r: 0, g: 0, b: 0 }), Detection::Dark);
        assert_eq!(
            classify(Rgb {
                r: 255,
                g: 255,
                b: 255
            }),
            Detection::Light
        );
        assert_eq!(
            classify(Rgb {
                r: 40,
                g: 40,
                b: 40
            }),
            Detection::Dark
        );
    }

    #[test]
    fn detect_with_io_dark_reply() {
        let reply = b"\x1b]11;rgb:20/20/30\x07";
        let mut out = Vec::new();
        let detection = detect_with_io(Duration::from_millis(100), &mut out, Cursor::new(reply));
        assert_eq!(detection, Detection::Dark);
        assert_eq!(out, b"\x1b]11;?\x07");
    }

    #[test]
    fn detect_with_io_light_reply() {
        let reply = b"\x1b]11;rgb:f0/f0/e0\x07";
        let detection = detect_with_io(Duration::from_millis(100), Vec::new(), Cursor::new(reply));
        assert_eq!(detection, Detection::Light);
    }

    #[test]
    fn detect_with_io_unknown_on_timeout() {
        let detection = detect_with_io(Duration::from_millis(10), Vec::new(), Cursor::new(&[][..]));
        assert_eq!(detection, Detection::Unknown);
    }

    #[test]
    fn detect_with_io_unknown_on_gibberish() {
        let detection = detect_with_io(
            Duration::from_millis(100),
            Vec::new(),
            Cursor::new(b"random stuff"),
        );
        assert_eq!(detection, Detection::Unknown);
    }
}
