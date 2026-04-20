//! Tool-invocation cell — one-line "● {summary}" header.
//!
//! This is intentionally a lightweight header; the matching output is
//! rendered separately by [`super::ToolResultCell`]. Together they
//! model the same two events that `Event::ToolUseStart` +
//! `Event::ToolResult` emit from the agent.

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::history::HistoryCell;

/// A tool invocation start marker.
#[derive(Debug, Clone)]
pub struct ToolCallCell {
    name: String,
    summary: Option<String>,
}

impl ToolCallCell {
    #[must_use]
    pub fn new(name: impl Into<String>, summary: Option<String>) -> Self {
        Self {
            name: name.into(),
            summary,
        }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn label(&self) -> &str {
        self.summary.as_deref().unwrap_or(&self.name)
    }
}

impl HistoryCell for ToolCallCell {
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        vec![Line::from(Span::styled(
            format!("● {}", self.label()),
            Style::default().fg(Color::DarkGray),
        ))]
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_summary_when_present() {
        let cell = ToolCallCell::new("read", Some("src/lib.rs".into()));
        let text: String = cell.display_lines(80)[0]
            .spans
            .iter()
            .map(|s| &*s.content)
            .collect();
        assert!(text.contains("src/lib.rs"));
    }

    #[test]
    fn falls_back_to_name() {
        let cell = ToolCallCell::new("bash", None);
        let text: String = cell.display_lines(80)[0]
            .spans
            .iter()
            .map(|s| &*s.content)
            .collect();
        assert!(text.contains("bash"));
    }
}
