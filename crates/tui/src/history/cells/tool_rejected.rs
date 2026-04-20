//! Tool rejection cell — shows what was rejected with optional rich preview.

use crab_core::tool::{ToolDisplayResult, ToolDisplayStyle as DS};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::history::HistoryCell;

/// A tool invocation rejected by the user.
#[derive(Debug, Clone)]
pub struct ToolRejectedCell {
    summary: String,
    display: Option<ToolDisplayResult>,
}

impl ToolRejectedCell {
    #[must_use]
    pub fn new(summary: impl Into<String>, display: Option<ToolDisplayResult>) -> Self {
        Self {
            summary: summary.into(),
            display,
        }
    }

    fn style_for(ds: Option<DS>) -> Style {
        match ds {
            Some(DS::Error | DS::DiffRemove) => Style::default().fg(Color::Red),
            Some(DS::DiffAdd) => Style::default().fg(Color::Green),
            Some(DS::DiffContext | DS::Muted) => Style::default().fg(Color::DarkGray),
            Some(DS::Highlight) => Style::default().fg(Color::Cyan),
            _ => Style::default().fg(Color::Gray),
        }
    }
}

impl HistoryCell for ToolRejectedCell {
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        let mut out = vec![Line::from(vec![
            Span::styled(
                "  \u{2298} ",
                Style::default()
                    .fg(Color::Red)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                self.summary.clone(),
                Style::default().fg(Color::Red),
            ),
        ])];

        if let Some(display) = &self.display {
            for dl in &display.lines {
                let style = Self::style_for(dl.style);
                out.push(Line::from(Span::styled(format!("    {}", dl.text), style)));
            }
        }

        out.push(Line::default());
        out
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::tool::{ToolDisplayLine, ToolDisplayResult, ToolDisplayStyle};

    #[test]
    fn plain_rejection_renders_summary() {
        let cell = ToolRejectedCell::new("Run rejected (ls)", None);
        let lines = cell.display_lines(80);
        assert!(lines.len() >= 2); // header + blank
        let text: String = lines[0].spans.iter().map(|s| &*s.content).collect();
        assert!(text.contains("Run rejected"));
    }

    #[test]
    fn rich_rejection_renders_preview_lines() {
        let display = ToolDisplayResult {
            lines: vec![
                ToolDisplayLine::new("echo hello", ToolDisplayStyle::Highlight),
                ToolDisplayLine::new("echo world", ToolDisplayStyle::Highlight),
            ],
            preview_lines: 2,
        };
        let cell = ToolRejectedCell::new("Run rejected", Some(display));
        let lines = cell.display_lines(80);
        // header + 2 preview + blank
        assert_eq!(lines.len(), 4);
    }
}
