use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::accents::CLAUDE_DARK;

#[derive(Debug, Clone)]
pub struct UserMessage {
    pub text: String,
}

impl UserMessage {
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    pub fn render_lines(&self, _width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        for (i, line) in self.text.lines().enumerate() {
            let prefix = if i == 0 { "❯ " } else { "  " };
            lines.push(Line::from(vec![
                Span::styled(
                    prefix.to_string(),
                    Style::default()
                        .fg(CLAUDE_DARK)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(line.to_string(), Style::default().fg(Color::White)),
            ]));
        }
        if lines.is_empty() {
            lines.push(Line::from(Span::styled(
                "❯ ",
                Style::default()
                    .fg(CLAUDE_DARK)
                    .add_modifier(Modifier::BOLD),
            )));
        }
        lines
    }

    pub fn desired_height(&self, width: u16) -> u16 {
        self.render_lines(width).len() as u16
    }

    pub fn search_text(&self) -> String {
        self.text.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_line() {
        let msg = UserMessage::new("hello");
        let lines = msg.render_lines(80);
        assert_eq!(lines.len(), 1);
        let text: String = lines[0].spans.iter().map(|s| &*s.content).collect();
        assert!(text.starts_with("❯ "));
        assert!(text.contains("hello"));
    }

    #[test]
    fn multiline() {
        let msg = UserMessage::new("line1\nline2\nline3");
        let lines = msg.render_lines(80);
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn desired_height_matches() {
        let msg = UserMessage::new("a\nb");
        assert_eq!(msg.desired_height(80), 2);
    }

    #[test]
    fn search_text() {
        let msg = UserMessage::new("find me");
        assert!(msg.search_text().contains("find me"));
    }
}
