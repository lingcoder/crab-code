//! User input cell — the `❯ {text}` row.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::history::HistoryCell;
use crate::theme::accents::CLAUDE_DARK;

/// A user input cell.
#[derive(Debug, Clone)]
pub struct UserCell {
    text: String,
}

impl UserCell {
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

impl HistoryCell for UserCell {
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        vec![
            Line::from(vec![
                Span::styled(
                    "❯ ",
                    Style::default()
                        .fg(CLAUDE_DARK)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(self.text.clone(), Style::default().fg(Color::White)),
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
    fn renders_prompt_glyph_and_text() {
        let cell = UserCell::new("hi");
        let lines = cell.display_lines(80);
        assert_eq!(lines.len(), 2); // content + blank
        let rendered: String = lines[0].spans.iter().map(|s| &*s.content).collect();
        assert!(rendered.starts_with("❯ "));
        assert!(rendered.contains("hi"));
    }

    #[test]
    fn desired_height_matches_line_count() {
        let cell = UserCell::new("some text");
        assert_eq!(cell.desired_height(80), 2);
    }

    #[test]
    fn search_text_includes_body() {
        let cell = UserCell::new("searchable text");
        assert!(cell.search_text().contains("searchable text"));
    }
}
