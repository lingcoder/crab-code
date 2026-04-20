use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::accents::CLAUDE_DARK;

const FOLD_THRESHOLD: usize = 50;
const FOLD_PREVIEW_LINES: usize = 5;

#[derive(Debug, Clone)]
pub struct AssistantMessage {
    pub text: String,
    pub streaming: bool,
    pub collapsed: bool,
    fold_threshold: usize,
}

impl AssistantMessage {
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            streaming: false,
            collapsed: false,
            fold_threshold: FOLD_THRESHOLD,
        }
    }

    #[must_use]
    pub fn streaming(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            streaming: true,
            collapsed: false,
            fold_threshold: FOLD_THRESHOLD,
        }
    }

    pub fn push_delta(&mut self, delta: &str) {
        self.text.push_str(delta);
    }

    pub fn finish_streaming(&mut self) {
        self.streaming = false;
        let line_count = self.text.lines().count();
        if line_count > self.fold_threshold {
            self.collapsed = true;
        }
    }

    pub fn toggle_collapsed(&mut self) {
        self.collapsed = !self.collapsed;
    }

    pub fn render_lines(&self, _width: u16) -> Vec<Line<'static>> {
        if self.text.is_empty() {
            return Vec::new();
        }

        let all_lines: Vec<&str> = self.text.lines().collect();
        let mut out: Vec<Line<'static>> = Vec::new();

        let display_lines = if self.collapsed && all_lines.len() > self.fold_threshold {
            let preview = &all_lines[..FOLD_PREVIEW_LINES.min(all_lines.len())];
            let hidden = all_lines.len() - preview.len();
            let mut lines: Vec<Line<'static>> = preview
                .iter()
                .map(|l| Line::from(Span::raw(l.to_string())))
                .collect();
            lines.push(Line::from(Span::styled(
                format!("... {hidden} more lines [Enter to expand]"),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            )));
            lines
        } else {
            all_lines
                .iter()
                .map(|l| Line::from(Span::raw(l.to_string())))
                .collect()
        };

        for (i, line) in display_lines.into_iter().enumerate() {
            if i == 0 {
                let mut spans = vec![Span::styled("● ", Style::default().fg(CLAUDE_DARK))];
                spans.extend(line.spans);
                out.push(Line::from(spans));
            } else {
                out.push(line);
            }
        }

        if self.streaming {
            out.push(Line::from(Span::styled(
                "▌",
                Style::default().fg(CLAUDE_DARK),
            )));
        }

        out
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
    fn empty_text_no_lines() {
        let msg = AssistantMessage::new("");
        assert!(msg.render_lines(80).is_empty());
    }

    #[test]
    fn bullet_prefix() {
        let msg = AssistantMessage::new("hello");
        let lines = msg.render_lines(80);
        assert!(!lines.is_empty());
        let first: String = lines[0].spans.iter().map(|s| &*s.content).collect();
        assert!(first.starts_with("● "));
    }

    #[test]
    fn streaming_cursor() {
        let msg = AssistantMessage::streaming("partial");
        let lines = msg.render_lines(80);
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
    fn folding_long_message() {
        let text = (0..60)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut msg = AssistantMessage::new(text);
        msg.finish_streaming();
        assert!(msg.collapsed);
        let lines = msg.render_lines(80);
        assert!(lines.len() < 60);
        let fold_line: String = lines[5].spans.iter().map(|s| &*s.content).collect();
        assert!(fold_line.contains("more lines"));
    }

    #[test]
    fn toggle_expand() {
        let text = (0..60)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut msg = AssistantMessage::new(text);
        msg.collapsed = true;
        msg.toggle_collapsed();
        assert!(!msg.collapsed);
        let lines = msg.render_lines(80);
        assert!(lines.len() >= 60);
    }

    #[test]
    fn push_delta() {
        let mut msg = AssistantMessage::streaming("hel");
        msg.push_delta("lo");
        assert_eq!(msg.search_text(), "hello");
    }
}
