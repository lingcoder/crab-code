//! Welcome cell — compact ambient startup panel.
//!
//! Shown at the top of the transcript when any of these is true:
//! - the current binary version differs from `global_state.last_welcome_version`
//! - the current project has no `CRAB.md` (new-project hint)
//! - the `CRAB_FORCE_FULL_LOGO` env var is truthy
//!
//! Single-column layout capped at 6 content lines (+ 1 trailing blank) so
//! it always fits inside the message viewport and never gets clipped by
//! the bottom-anchored scroller. Recent-activity lives in the session
//! sidebar, not here — duplicating it here made earlier three-column
//! layouts overflow on short terminals.
//!
//! Wide (width ≥ 40):
//! ```text
//! ✦ Crab Code v0.1.0
//! What's new
//!   • bullet 1
//!   • bullet 2
//!   • bullet 3
//! First time? Press /help · No CRAB.md — consider /init
//! ```
//!
//! Narrow (width < 40):
//! ```text
//! ✦ Crab Code v0.1.0
//! • bullet 1 · bullet 2 · bullet 3
//! /help · /init
//! ```
//!
//! Tiny (width < 24): banner only.
//!
//! Not included in the transcript overlay — ambient context is not
//! conversation content.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::history::HistoryCell;
use crate::theme::accents::CLAUDE_DARK as CRAB_COLOR;

/// Maximum number of bullets rendered, regardless of how many the caller
/// supplied. Keeps the cell within its line budget.
const MAX_BULLETS: usize = 3;

/// Thresholds for layout degradation.
const WIDE_MIN: u16 = 40;
const NARROW_MIN: u16 = 24;

/// Compact ambient welcome panel.
#[derive(Debug, Clone)]
pub struct WelcomeCell {
    version: String,
    whats_new: Vec<String>,
    show_project_hint: bool,
}

impl WelcomeCell {
    #[must_use]
    pub fn new(version: String, whats_new: Vec<String>, show_project_hint: bool) -> Self {
        Self {
            version,
            whats_new,
            show_project_hint,
        }
    }
}

impl HistoryCell for WelcomeCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        if width >= WIDE_MIN {
            render_wide(self)
        } else if width >= NARROW_MIN {
            render_narrow(self)
        } else {
            render_tiny(self)
        }
    }

    /// Ambient context, not conversation — skip in transcript overlay.
    fn transcript_lines(&self, _width: u16) -> Vec<Line<'static>> {
        Vec::new()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

// ─── Layouts ──────────────────────────────────────────────────────────────

fn banner_line(version: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            "✦ ",
            Style::default().fg(CRAB_COLOR).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("Crab Code {version}"),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

fn render_wide(cell: &WelcomeCell) -> Vec<Line<'static>> {
    let mut out = Vec::with_capacity(7);
    out.push(banner_line(&cell.version));
    if !cell.whats_new.is_empty() {
        out.push(Line::from(Span::styled(
            "What's new",
            Style::default()
                .fg(CRAB_COLOR)
                .add_modifier(Modifier::BOLD),
        )));
        for bullet in cell.whats_new.iter().take(MAX_BULLETS) {
            out.push(Line::from(vec![
                Span::styled("  • ", Style::default().fg(CRAB_COLOR)),
                Span::styled(bullet.clone(), Style::default().fg(Color::Gray)),
            ]));
        }
    }
    out.push(hint_line(cell.show_project_hint));
    out.push(Line::default());
    out
}

fn render_narrow(cell: &WelcomeCell) -> Vec<Line<'static>> {
    let mut out = Vec::with_capacity(4);
    out.push(banner_line(&cell.version));
    if !cell.whats_new.is_empty() {
        let joined: String = cell
            .whats_new
            .iter()
            .take(MAX_BULLETS)
            .cloned()
            .collect::<Vec<_>>()
            .join(" · ");
        out.push(Line::from(vec![
            Span::styled("• ", Style::default().fg(CRAB_COLOR)),
            Span::styled(joined, Style::default().fg(Color::Gray)),
        ]));
    }
    out.push(short_hint_line(cell.show_project_hint));
    out.push(Line::default());
    out
}

fn render_tiny(cell: &WelcomeCell) -> Vec<Line<'static>> {
    vec![banner_line(&cell.version), Line::default()]
}

// ─── Hint rows ────────────────────────────────────────────────────────────

fn hint_line(show_project_hint: bool) -> Line<'static> {
    let dim = Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::ITALIC);
    let mut spans = vec![Span::styled("First time? Press /help", dim)];
    if show_project_hint {
        spans.push(Span::styled(
            "  ·  No CRAB.md — consider /init",
            dim,
        ));
    }
    Line::from(spans)
}

fn short_hint_line(show_project_hint: bool) -> Line<'static> {
    let dim = Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::ITALIC);
    let text = if show_project_hint {
        "/help · /init"
    } else {
        "/help"
    };
    Line::from(Span::styled(text, dim))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> WelcomeCell {
        WelcomeCell::new(
            "0.1.0".into(),
            vec![
                "dropped cmd fallback".into(),
                "collapse parallel reads".into(),
                "reasoning_content for R1".into(),
            ],
            true,
        )
    }

    fn flatten(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn wide_layout_fits_budget() {
        let cell = sample();
        let lines = cell.display_lines(120);
        // banner + "What's new" + 3 bullets + hint + trailing blank = 7
        assert!(lines.len() <= 7, "got {} lines: {:?}", lines.len(), lines);
    }

    #[test]
    fn wide_includes_all_sections() {
        let cell = sample();
        let text = flatten(&cell.display_lines(120));
        assert!(text.contains("Crab Code 0.1.0"));
        assert!(text.contains("What's new"));
        assert!(text.contains("dropped cmd fallback"));
        assert!(text.contains("First time?"));
        assert!(text.contains("CRAB.md"));
    }

    #[test]
    fn narrow_layout_collapses_to_four_lines() {
        let cell = sample();
        let lines = cell.display_lines(30);
        assert!(lines.len() <= 4, "got {} lines", lines.len());
    }

    #[test]
    fn tiny_layout_banner_only() {
        let cell = sample();
        let lines = cell.display_lines(10);
        assert_eq!(lines.len(), 2);
        let text = flatten(&lines);
        assert!(text.contains("Crab Code"));
        assert!(!text.contains("What's new"));
    }

    #[test]
    fn caps_bullets_at_three() {
        let cell = WelcomeCell::new(
            "0.1.0".into(),
            vec![
                "a".into(),
                "b".into(),
                "c".into(),
                "d".into(),
                "e".into(),
            ],
            false,
        );
        let text = flatten(&cell.display_lines(120));
        assert!(text.contains("• a"));
        assert!(text.contains("• b"));
        assert!(text.contains("• c"));
        assert!(!text.contains("• d"));
    }

    #[test]
    fn project_hint_omitted_when_disabled() {
        let cell = WelcomeCell::new("0.1.0".into(), vec!["n".into()], false);
        let text = flatten(&cell.display_lines(120));
        assert!(!text.contains("CRAB.md"));
        assert!(text.contains("First time?"));
    }

    #[test]
    fn empty_whats_new_drops_section() {
        let cell = WelcomeCell::new("0.1.0".into(), Vec::new(), false);
        let lines = cell.display_lines(120);
        // banner + hint + blank = 3
        assert_eq!(lines.len(), 3);
        let text = flatten(&lines);
        assert!(!text.contains("What's new"));
    }

    #[test]
    fn transcript_lines_are_empty() {
        let cell = sample();
        assert!(cell.transcript_lines(120).is_empty());
    }
}
