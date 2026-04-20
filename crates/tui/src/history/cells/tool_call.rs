//! Tool-invocation cell — colored `● {name} {detail}` header.
//!
//! Each tool category gets a distinct icon color via `Tool::display_color()`.
//! The summary is split into a bold tool name and a cyan detail portion.

use crab_core::tool::ToolDisplayStyle as DS;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::history::HistoryCell;

/// A tool invocation start marker.
#[derive(Debug, Clone)]
pub struct ToolCallCell {
    name: String,
    summary: Option<String>,
    color: Option<DS>,
}

impl ToolCallCell {
    #[must_use]
    pub fn new(name: impl Into<String>, summary: Option<String>, color: Option<DS>) -> Self {
        Self {
            name: name.into(),
            summary,
            color,
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

    fn icon_color(&self) -> Color {
        match self.color {
            Some(DS::Highlight) => Color::Cyan,
            Some(DS::DiffAdd) => Color::Green,
            Some(DS::DiffRemove | DS::Error) => Color::Red,
            Some(DS::Muted) => Color::DarkGray,
            _ => Color::White,
        }
    }

    fn parse_summary(&self) -> (&str, Option<&str>) {
        let label = self.label();
        if let Some(paren_start) = label.find('(')
            && label.ends_with(')')
        {
            let tool_part = label[..paren_start].trim();
            let detail = &label[paren_start + 1..label.len() - 1];
            return (tool_part, Some(detail));
        }
        (label, None)
    }
}

impl HistoryCell for ToolCallCell {
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        let icon_color = self.icon_color();
        let (tool_part, detail) = self.parse_summary();

        let mut spans = vec![
            Span::styled("● ", Style::default().fg(icon_color)),
            Span::styled(
                tool_part.to_string(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ];

        if let Some(detail) = detail {
            spans.push(Span::styled(
                format!(" {detail}"),
                Style::default().fg(Color::Cyan),
            ));
        }

        vec![Line::from(spans)]
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
        let cell = ToolCallCell::new("read", Some("Read (src/lib.rs)".into()), Some(DS::Muted));
        let lines = cell.display_lines(80);
        let text: String = lines[0].spans.iter().map(|s| &*s.content).collect();
        assert!(text.contains("Read"));
        assert!(text.contains("src/lib.rs"));
    }

    #[test]
    fn falls_back_to_name() {
        let cell = ToolCallCell::new("bash", None, None);
        let text: String = cell.display_lines(80)[0]
            .spans
            .iter()
            .map(|s| &*s.content)
            .collect();
        assert!(text.contains("bash"));
    }

    #[test]
    fn icon_color_matches_display_style() {
        let cell = ToolCallCell::new("bash", None, Some(DS::Highlight));
        assert_eq!(cell.icon_color(), Color::Cyan);

        let cell = ToolCallCell::new("edit", None, Some(DS::DiffAdd));
        assert_eq!(cell.icon_color(), Color::Green);

        let cell = ToolCallCell::new("read", None, Some(DS::Muted));
        assert_eq!(cell.icon_color(), Color::DarkGray);
    }

    #[test]
    fn summary_parsed_into_name_and_detail() {
        let cell = ToolCallCell::new("bash", Some("Run (ls -la)".into()), None);
        let (name, detail) = cell.parse_summary();
        assert_eq!(name, "Run");
        assert_eq!(detail, Some("ls -la"));
    }

    #[test]
    fn summary_without_parens_stays_whole() {
        let cell = ToolCallCell::new("bash", Some("Run command".into()), None);
        let (name, detail) = cell.parse_summary();
        assert_eq!(name, "Run command");
        assert_eq!(detail, None);
    }

    #[test]
    fn multi_span_rendering() {
        let cell = ToolCallCell::new(
            "edit",
            Some("Update (src/main.rs)".into()),
            Some(DS::DiffAdd),
        );
        let lines = cell.display_lines(80);
        assert_eq!(lines.len(), 1);
        // icon + name + detail = 3 spans
        assert_eq!(lines[0].spans.len(), 3);
        // icon is green
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::Green));
        // name is bold white
        assert_eq!(lines[0].spans[1].style.fg, Some(Color::White));
        // detail is cyan
        assert_eq!(lines[0].spans[2].style.fg, Some(Color::Cyan));
    }
}
