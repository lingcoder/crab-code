//! Thinking cell — collapsible extended-thinking block.
//!
//! Renders with a `∴` glyph and dim purple styling. Default collapsed,
//! showing only a summary line. When expanded, shows the full reasoning.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::history::HistoryCell;

/// An extended-thinking content block.
#[derive(Debug, Clone)]
pub struct ThinkingCell {
    text: String,
    collapsed: bool,
    duration: Option<std::time::Duration>,
}

impl ThinkingCell {
    #[must_use]
    pub fn new(text: String, collapsed: bool, duration: Option<std::time::Duration>) -> Self {
        Self {
            text,
            collapsed,
            duration,
        }
    }

    fn summary_line(&self) -> Line<'static> {
        let label = if let Some(dur) = self.duration {
            format!("∴ Thinking ({}s)", dur.as_secs())
        } else {
            "∴ Thinking…".to_string()
        };
        let arrow = if self.collapsed { " ▸" } else { " ▾" };
        Line::from(vec![
            Span::styled(
                label,
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::DIM),
            ),
            Span::styled(arrow, Style::default().fg(Color::DarkGray)),
        ])
    }
}

impl HistoryCell for ThinkingCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = vec![self.summary_line()];
        if !self.collapsed && !self.text.is_empty() {
            let w = width.saturating_sub(2) as usize;
            for raw_line in self.text.lines() {
                if raw_line.len() <= w || w == 0 {
                    lines.push(Line::from(Span::styled(
                        format!("  {raw_line}"),
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    )));
                } else {
                    for chunk in raw_line.as_bytes().chunks(w) {
                        let s = String::from_utf8_lossy(chunk);
                        lines.push(Line::from(Span::styled(
                            format!("  {s}"),
                            Style::default()
                                .fg(Color::DarkGray)
                                .add_modifier(Modifier::ITALIC),
                        )));
                    }
                }
            }
        }
        lines
    }

    fn search_text(&self) -> String {
        self.text.clone()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapsed_shows_summary_only() {
        let cell = ThinkingCell::new("deep thought".into(), true, None);
        let lines = cell.display_lines(80);
        assert_eq!(lines.len(), 1);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("Thinking"));
        assert!(text.contains('▸'));
    }

    #[test]
    fn expanded_shows_content() {
        let cell = ThinkingCell::new("line one\nline two".into(), false, None);
        let lines = cell.display_lines(80);
        assert!(lines.len() >= 3);
        let text: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("line one"));
    }

    #[test]
    fn duration_in_summary() {
        let cell = ThinkingCell::new(
            "reasoning".into(),
            true,
            Some(std::time::Duration::from_secs(5)),
        );
        let lines = cell.display_lines(80);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("5s"));
    }

    #[test]
    fn search_text_includes_content() {
        let cell = ThinkingCell::new("secret reasoning".into(), true, None);
        assert!(cell.search_text().contains("secret reasoning"));
    }
}
