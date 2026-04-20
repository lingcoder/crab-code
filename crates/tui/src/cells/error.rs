use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

#[derive(Debug, Clone)]
pub struct ErrorInfo {
    pub message: String,
    pub recoverable: bool,
}

impl ErrorInfo {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            recoverable: false,
        }
    }

    #[must_use]
    pub fn recoverable(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            recoverable: true,
        }
    }

    pub fn render_lines(&self, _width: u16) -> Vec<Line<'static>> {
        let error_style = Style::default().fg(Color::Red);
        let mut lines = vec![Line::from(vec![
            Span::styled("✗ ", error_style.add_modifier(Modifier::BOLD)),
            Span::styled(self.message.clone(), error_style),
        ])];

        if self.recoverable {
            lines.push(Line::from(Span::styled(
                "  [R] Retry  [N] New session",
                Style::default().fg(Color::DarkGray),
            )));
        }

        lines
    }

    pub fn desired_height(&self, width: u16) -> u16 {
        self.render_lines(width).len() as u16
    }

    #[must_use]
    pub fn is_streaming(&self) -> bool {
        false
    }

    pub fn search_text(&self) -> String {
        self.message.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_error_prefix() {
        let err = ErrorInfo::new("connection failed");
        let lines = err.render_lines(80);
        assert_eq!(lines.len(), 1);
        let text: String = lines[0].spans.iter().map(|s| &*s.content).collect();
        assert!(text.starts_with("✗ "));
        assert!(text.contains("connection failed"));
    }

    #[test]
    fn red_style() {
        let err = ErrorInfo::new("bad");
        let lines = err.render_lines(80);
        assert_eq!(lines[0].spans[1].style.fg, Some(Color::Red));
    }

    #[test]
    fn recoverable_shows_actions() {
        let err = ErrorInfo::recoverable("rate limited");
        let lines = err.render_lines(80);
        assert_eq!(lines.len(), 2);
        let action: String = lines[1].spans.iter().map(|s| &*s.content).collect();
        assert!(action.contains("[R] Retry"));
    }

    #[test]
    fn non_recoverable_no_actions() {
        let err = ErrorInfo::new("fatal");
        let lines = err.render_lines(80);
        assert_eq!(lines.len(), 1);
    }
}
