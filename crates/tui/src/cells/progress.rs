use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

#[derive(Debug, Clone)]
pub struct ProgressInfo {
    pub message: String,
}

impl ProgressInfo {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn render_lines(&self, _width: u16) -> Vec<Line<'static>> {
        vec![Line::from(Span::styled(
            format!("⏺ {}", self.message),
            Style::default().fg(Color::DarkGray),
        ))]
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
    fn renders_with_glyph() {
        let info = ProgressInfo::new("loading...");
        let lines = info.render_lines(80);
        assert_eq!(lines.len(), 1);
        let text: String = lines[0].spans.iter().map(|s| &*s.content).collect();
        assert!(text.starts_with("⏺ "));
        assert!(text.contains("loading..."));
    }

    #[test]
    fn dimmed_style() {
        let info = ProgressInfo::new("test");
        let lines = info.render_lines(80);
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::DarkGray));
    }

    #[test]
    fn not_streaming() {
        let info = ProgressInfo::new("x");
        assert!(!info.is_streaming());
    }
}
