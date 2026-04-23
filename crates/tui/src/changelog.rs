//! Parser for `docs/CHANGELOG.md` → "What's new" bullets.
//!
//! The full changelog is embedded at compile time via `include_str!` so the
//! welcome cell can render release notes without any filesystem I/O at
//! startup. We only surface the most recent version's bullets; older entries
//! are left in the file for historical reference.
//!
//! Supported format (Keep a Changelog subset):
//!
//! ```markdown
//! ## [Unreleased]
//! ### Added
//! - Welcome panel shown at startup
//! - Another entry that wraps
//!   onto the next line
//! ### Changed
//! - Bash tool now requires POSIX shell
//!
//! ## [0.1.0] - 2026-04-01
//! ### Added
//! - Initial release
//! ```
//!
//! We produce a flat `Vec<String>` of bullets in document order, limited to
//! the top `max` items (typically 3–5 for the welcome cell).

/// Full changelog embedded at compile time.
const CHANGELOG: &str = include_str!("../../../docs/CHANGELOG.md");

/// Maximum bullet length shown in the welcome panel.
const MAX_BULLET_LEN: usize = 80;

/// Return up to `max` bullets from the most recent changelog entry.
///
/// "Most recent entry" = the first `## [...]` heading in the document;
/// bullets keep appearing until the next `## [...]` heading (i.e. we ignore
/// `### Added / ### Changed` subsection dividers and collect them all).
#[must_use]
pub fn whats_new(max: usize) -> Vec<String> {
    parse(CHANGELOG, max)
}

#[derive(Copy, Clone)]
enum State {
    /// Before the first `## [version]` heading — skip everything.
    Prelude,
    /// Inside the most-recent version block — collect bullets until the
    /// next `## [version]` heading or end of file.
    InFirstVersion,
}

fn parse(source: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut state = State::Prelude;
    let mut current: Option<String> = None;

    for raw in source.lines() {
        let line = raw.trim_end();

        if line.starts_with("## ") {
            match state {
                State::Prelude => state = State::InFirstVersion,
                State::InFirstVersion => {
                    if let Some(bullet) = current.take() {
                        push_bullet(&mut out, &bullet, max);
                    }
                    return out;
                }
            }
            continue;
        }

        if !matches!(state, State::InFirstVersion) {
            continue;
        }

        // Skip subsection headers (### Added / ### Changed / etc.)
        if line.starts_with("### ") {
            if let Some(bullet) = current.take() {
                push_bullet(&mut out, &bullet, max);
                if out.len() >= max {
                    return out;
                }
            }
            continue;
        }

        if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            if let Some(bullet) = current.take() {
                push_bullet(&mut out, &bullet, max);
                if out.len() >= max {
                    return out;
                }
            }
            current = Some(rest.trim().to_owned());
        } else if !line.trim().is_empty()
            && let Some(ref mut acc) = current
        {
            // Continuation of a bullet that wrapped onto the next line
            // (Keep a Changelog convention: continuation is indented).
            acc.push(' ');
            acc.push_str(line.trim());
        }
    }

    if let Some(bullet) = current.take() {
        push_bullet(&mut out, &bullet, max);
    }
    out
}

fn push_bullet(out: &mut Vec<String>, bullet: &str, max: usize) {
    if out.len() >= max {
        return;
    }
    let trimmed = truncate(bullet.trim());
    if !trimmed.is_empty() {
        out.push(trimmed);
    }
}

fn truncate(s: &str) -> String {
    if s.chars().count() <= MAX_BULLET_LEN {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(MAX_BULLET_LEN.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# Changelog\n\nPreamble text.\n\n## [Unreleased]\n\n### Added\n- First bullet\n- Second bullet that wraps\n  onto the next line\n### Changed\n- Third bullet\n\n## [0.1.0] - 2026-04-01\n\n### Added\n- Older release\n- Should not appear\n";

    #[test]
    fn picks_first_version_bullets() {
        let bullets = parse(SAMPLE, 10);
        assert_eq!(bullets.len(), 3);
        assert_eq!(bullets[0], "First bullet");
        assert!(bullets[1].starts_with("Second bullet"));
        assert_eq!(bullets[2], "Third bullet");
    }

    #[test]
    fn ignores_older_versions() {
        let bullets = parse(SAMPLE, 10);
        assert!(!bullets.iter().any(|b| b.contains("Older release")));
    }

    #[test]
    fn respects_max_limit() {
        let bullets = parse(SAMPLE, 2);
        assert_eq!(bullets.len(), 2);
    }

    #[test]
    fn wraps_continuations_into_single_bullet() {
        let bullets = parse(SAMPLE, 10);
        assert!(
            bullets[1].contains("onto the next line"),
            "got: {:?}",
            bullets[1]
        );
    }

    #[test]
    fn truncates_overlong_bullets_with_ellipsis() {
        let long = "- ".to_owned() + &"x".repeat(200);
        let source = format!("## [U]\n### Added\n{long}\n");
        let bullets = parse(&source, 1);
        assert_eq!(bullets.len(), 1);
        assert!(bullets[0].ends_with('…'));
        assert!(bullets[0].chars().count() <= MAX_BULLET_LEN);
    }

    #[test]
    fn empty_changelog_returns_empty() {
        assert!(parse("", 3).is_empty());
        assert!(parse("# Changelog\n", 3).is_empty());
    }

    #[test]
    fn zero_max_returns_empty() {
        let bullets = parse(SAMPLE, 0);
        assert!(bullets.is_empty());
    }

    #[test]
    fn accepts_asterisk_bullets() {
        let src = "## [U]\n### Added\n* Star bullet\n* Another\n";
        let bullets = parse(src, 10);
        assert_eq!(bullets, vec!["Star bullet", "Another"]);
    }

    #[test]
    fn real_changelog_returns_some_items() {
        // Sanity check: the embedded CHANGELOG should yield at least one
        // bullet so the welcome panel never silently goes empty on a
        // tracked release.
        let bullets = whats_new(3);
        assert!(!bullets.is_empty(), "expected some bullets from CHANGELOG");
    }
}
