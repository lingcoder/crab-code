//! Tool result cell — truncated output, optional tool-customized styling.

use crab_core::tool::{ToolDisplayResult, ToolDisplayStyle as DS};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::history::HistoryCell;

/// Default row limit before showing the "... N more lines" pager row.
const DEFAULT_LIMIT: usize = 10;

/// A tool execution result.
#[derive(Debug, Clone)]
pub struct ToolResultCell {
    tool_name: String,
    output: String,
    is_error: bool,
    display: Option<ToolDisplayResult>,
    collapsed: bool,
}

impl ToolResultCell {
    #[must_use]
    pub fn new(
        tool_name: impl Into<String>,
        output: impl Into<String>,
        is_error: bool,
        display: Option<ToolDisplayResult>,
        collapsed: bool,
    ) -> Self {
        Self {
            tool_name: tool_name.into(),
            output: output.into(),
            is_error,
            display,
            collapsed,
        }
    }

    #[must_use]
    pub fn tool_name(&self) -> &str {
        &self.tool_name
    }

    #[must_use]
    pub fn is_error(&self) -> bool {
        self.is_error
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

impl HistoryCell for ToolResultCell {
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        let mut out: Vec<Line<'static>> = Vec::new();

        if let Some(display) = &self.display {
            let total = display.lines.len();
            let limit = if self.collapsed && display.preview_lines > 0 {
                display.preview_lines.min(total)
            } else {
                total
            };
            for dl in &display.lines[..limit] {
                let style = Self::style_for(dl.style);
                out.push(Line::from(Span::styled(format!("  {}", dl.text), style)));
            }
            if limit < total {
                out.push(Line::from(Span::styled(
                    format!("  ... ({} more lines, Enter to expand)", total - limit),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        } else {
            let style = if self.is_error {
                Style::default().fg(Color::Red)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let lines: Vec<&str> = self.output.lines().collect();
            let limit = if self.collapsed {
                lines.len().min(DEFAULT_LIMIT)
            } else {
                lines.len()
            };
            for line in &lines[..limit] {
                out.push(Line::from(Span::styled(format!("  {line}"), style)));
            }
            if lines.len() > limit {
                out.push(Line::from(Span::styled(
                    format!("  ... ({} more lines)", lines.len() - limit),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
        out.push(Line::default());
        out
    }

    /// Transcript view shows the full output with no truncation, since
    /// the overlay exists specifically to inspect long outputs.
    fn transcript_lines(&self, _width: u16) -> Vec<Line<'static>> {
        if let Some(display) = &self.display {
            let mut out: Vec<Line<'static>> = display
                .lines
                .iter()
                .map(|dl| {
                    let style = Self::style_for(dl.style);
                    Line::from(Span::styled(format!("  {}", dl.text), style))
                })
                .collect();
            out.push(Line::default());
            return out;
        }
        let style = if self.is_error {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let mut out: Vec<Line<'static>> = self
            .output
            .lines()
            .map(|line| Line::from(Span::styled(format!("  {line}"), style)))
            .collect();
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

    #[test]
    fn short_output_is_not_truncated() {
        let cell = ToolResultCell::new("read", "one\ntwo", false, None, true);
        let lines = cell.display_lines(80);
        // 2 body + 1 blank
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn long_output_is_truncated_with_pager() {
        let body = (0..20)
            .map(|i| format!("line-{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let cell = ToolResultCell::new("bash", body, false, None, true);
        let lines = cell.display_lines(80);
        // 10 body + 1 pager + 1 blank
        assert_eq!(lines.len(), 12);
        let last_visible: String = lines[10].spans.iter().map(|s| &*s.content).collect();
        assert!(last_visible.contains("more lines"));
    }

    #[test]
    fn transcript_shows_full_output() {
        let body = (0..20)
            .map(|i| format!("line-{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let cell = ToolResultCell::new("bash", body, false, None, true);
        let lines = cell.transcript_lines(80);
        // 20 body + 1 blank, no pager row
        assert_eq!(lines.len(), 21);
    }

    #[test]
    fn error_gets_red_styling() {
        let cell = ToolResultCell::new("bash", "bad", true, None, true);
        let lines = cell.display_lines(80);
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::Red));
    }

    #[test]
    fn collapsed_display_respects_preview_lines() {
        use crab_core::tool::{ToolDisplayLine, ToolDisplayResult, ToolDisplayStyle};
        let display = ToolDisplayResult {
            lines: (0..10)
                .map(|i| ToolDisplayLine::new(format!("line-{i}"), ToolDisplayStyle::Normal))
                .collect(),
            preview_lines: 3,
        };
        let cell = ToolResultCell::new("bash", "", false, Some(display), true);
        let lines = cell.display_lines(80);
        // 3 preview + 1 "... more" + 1 blank
        assert_eq!(lines.len(), 5);
        let pager: String = lines[3].spans.iter().map(|s| &*s.content).collect();
        assert!(pager.contains("7 more lines"));
    }

    #[test]
    fn expanded_display_shows_all_lines() {
        use crab_core::tool::{ToolDisplayLine, ToolDisplayResult, ToolDisplayStyle};
        let display = ToolDisplayResult {
            lines: (0..10)
                .map(|i| ToolDisplayLine::new(format!("line-{i}"), ToolDisplayStyle::Normal))
                .collect(),
            preview_lines: 3,
        };
        let cell = ToolResultCell::new("bash", "", false, Some(display), false);
        let lines = cell.display_lines(80);
        // 10 body + 1 blank
        assert_eq!(lines.len(), 11);
    }
}
