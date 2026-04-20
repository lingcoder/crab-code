use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

#[derive(Debug, Clone)]
pub struct ThinkingBlock {
    pub text: String,
    pub streaming: bool,
}

impl ThinkingBlock {
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            streaming: false,
        }
    }

    #[must_use]
    pub fn streaming(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            streaming: true,
        }
    }

    pub fn push_delta(&mut self, delta: &str) {
        self.text.push_str(delta);
    }

    pub fn finish_streaming(&mut self) {
        self.streaming = false;
    }

    pub fn render_lines(&self, _width: u16) -> Vec<Line<'static>> {
        let dimmed = Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC);

        if self.text.is_empty() && self.streaming {
            return vec![Line::from(Span::styled("Thinking...", dimmed))];
        }

        let mut lines: Vec<Line<'static>> = Vec::new();
        for (i, line) in self.text.lines().enumerate() {
            let prefix = if i == 0 { "💭 " } else { "   " };
            lines.push(Line::from(Span::styled(format!("{prefix}{line}"), dimmed)));
        }

        if self.streaming {
            lines.push(Line::from(Span::styled("   ▌", dimmed)));
        }

        lines
    }

    pub fn desired_height(&self, width: u16) -> u16 {
        self.render_lines(width).len() as u16
    }

    pub fn is_streaming(&self) -> bool {
        self.streaming
    }

    pub fn search_text(&self) -> String {
        self.text.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_streaming_shows_placeholder() {
        let block = ThinkingBlock::streaming("");
        let lines = block.render_lines(80);
        assert_eq!(lines.len(), 1);
        let text: String = lines[0].spans.iter().map(|s| &*s.content).collect();
        assert!(text.contains("Thinking..."));
    }

    #[test]
    fn text_has_thought_prefix() {
        let block = ThinkingBlock::new("considering options");
        let lines = block.render_lines(80);
        let text: String = lines[0].spans.iter().map(|s| &*s.content).collect();
        assert!(text.contains("💭"));
    }

    #[test]
    fn streaming_has_cursor() {
        let block = ThinkingBlock::streaming("partial thought");
        let lines = block.render_lines(80);
        let last: String = lines
            .last()
            .unwrap()
            .spans
            .iter()
            .map(|s| &*s.content)
            .collect();
        assert!(last.contains('▌'));
    }

    #[test]
    fn dimmed_style() {
        let block = ThinkingBlock::new("test");
        let lines = block.render_lines(80);
        let style = lines[0].spans[0].style;
        assert_eq!(style.fg, Some(Color::DarkGray));
    }
}
