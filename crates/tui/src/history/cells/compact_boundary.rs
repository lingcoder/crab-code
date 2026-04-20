//! Compact boundary cell — visual separator for context compaction events.

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::history::HistoryCell;

/// Renders a full-width horizontal line with a centered compaction summary label.
#[derive(Debug, Clone)]
pub struct CompactBoundaryCell {
    strategy: String,
    after_tokens: u64,
    removed_messages: usize,
}

impl CompactBoundaryCell {
    #[must_use]
    pub fn new(strategy: String, after_tokens: u64, removed_messages: usize) -> Self {
        Self {
            strategy,
            after_tokens,
            removed_messages,
        }
    }
}

impl HistoryCell for CompactBoundaryCell {
    #[allow(clippy::cast_possible_truncation)]
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let w = width as usize;
        let tokens_k = self.after_tokens / 1000;
        let label = format!(
            " compacted ({}) \u{2014} removed {} msgs, {}k tokens remaining ",
            self.strategy, self.removed_messages, tokens_k
        );

        let dim = Style::default().fg(Color::DarkGray);
        let label_style = Style::default().fg(Color::DarkGray);

        let label_len = label.len();
        let remaining = w.saturating_sub(label_len);
        let left = remaining / 2;
        let right = remaining.saturating_sub(left);

        let left_dashes = "\u{2500}".repeat(left);
        let right_dashes = "\u{2500}".repeat(right);

        vec![
            Line::default(),
            Line::from(vec![
                Span::styled(left_dashes, dim),
                Span::styled(label, label_style),
                Span::styled(right_dashes, dim),
            ]),
            Line::default(),
        ]
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundary_renders_three_lines() {
        let cell = CompactBoundaryCell::new("summary".into(), 50000, 12);
        let lines = cell.display_lines(80);
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn boundary_contains_label() {
        let cell = CompactBoundaryCell::new("summary".into(), 50000, 12);
        let lines = cell.display_lines(80);
        let text: String = lines[1]
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert!(text.contains("compacted"));
        assert!(text.contains("12 msgs"));
        assert!(text.contains("50k tokens"));
    }

    #[test]
    fn boundary_contains_dashes() {
        let cell = CompactBoundaryCell::new("trim".into(), 100_000, 5);
        let lines = cell.display_lines(80);
        let text: String = lines[1]
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert!(text.contains('\u{2500}'));
    }

    #[test]
    fn boundary_narrow_width() {
        let cell = CompactBoundaryCell::new("trim".into(), 1000, 1);
        let lines = cell.display_lines(20);
        assert_eq!(lines.len(), 3);
    }
}
