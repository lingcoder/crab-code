use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

#[derive(Debug, Clone)]
pub struct ToolRejected {
    pub tool_name: String,
    pub reason: Option<String>,
}

impl ToolRejected {
    #[must_use]
    pub fn new(tool_name: impl Into<String>) -> Self {
        Self {
            tool_name: tool_name.into(),
            reason: None,
        }
    }

    #[must_use]
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    pub fn render_lines(&self, _width: u16) -> Vec<Line<'static>> {
        let dim_strike = Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::CROSSED_OUT);

        let reason = self.reason.as_deref().unwrap_or("denied by user");
        vec![Line::from(vec![
            Span::styled("✗ ", Style::default().fg(Color::Red)),
            Span::styled(self.tool_name.clone(), dim_strike),
            Span::styled(format!(" — {reason}"), Style::default().fg(Color::DarkGray)),
        ])]
    }

    pub fn desired_height(&self, width: u16) -> u16 {
        self.render_lines(width).len() as u16
    }

    #[must_use]
    pub fn is_streaming(&self) -> bool {
        false
    }

    pub fn search_text(&self) -> String {
        format!(
            "{} {}",
            self.tool_name,
            self.reason.as_deref().unwrap_or("")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_with_strikethrough() {
        let rej = ToolRejected::new("Bash").with_reason("not allowed");
        let lines = rej.render_lines(80);
        assert_eq!(lines.len(), 1);
        let text: String = lines[0].spans.iter().map(|s| &*s.content).collect();
        assert!(text.contains("Bash"));
        assert!(text.contains("not allowed"));
        assert!(
            lines[0].spans[1]
                .style
                .add_modifier
                .contains(Modifier::CROSSED_OUT)
        );
    }

    #[test]
    fn default_reason() {
        let rej = ToolRejected::new("Read");
        let lines = rej.render_lines(80);
        let text: String = lines[0].spans.iter().map(|s| &*s.content).collect();
        assert!(text.contains("denied by user"));
    }
}
