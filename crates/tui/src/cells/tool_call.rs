use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

const OUTPUT_TRUNCATE_LIMIT: usize = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    Proposed,
    Running,
    Complete,
    Error,
}

#[derive(Debug, Clone)]
pub struct ToolCallState {
    pub name: String,
    pub summary: Option<String>,
    pub output: Option<String>,
    pub status: ToolStatus,
    pub collapsed: bool,
    pub is_error: bool,
}

impl ToolCallState {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            summary: None,
            output: None,
            status: ToolStatus::Proposed,
            collapsed: true,
            is_error: false,
        }
    }

    #[must_use]
    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    #[must_use]
    pub fn with_output(mut self, output: impl Into<String>) -> Self {
        self.output = Some(output.into());
        self
    }

    #[must_use]
    pub fn with_status(mut self, status: ToolStatus) -> Self {
        self.status = status;
        self
    }

    pub fn toggle_collapsed(&mut self) {
        self.collapsed = !self.collapsed;
    }

    fn status_glyph(&self) -> &'static str {
        match self.status {
            ToolStatus::Proposed => "○",
            ToolStatus::Running => "⏺",
            ToolStatus::Complete => {
                if self.is_error {
                    "✗"
                } else {
                    "✓"
                }
            }
            ToolStatus::Error => "✗",
        }
    }

    fn status_color(&self) -> Color {
        match self.status {
            ToolStatus::Proposed => Color::DarkGray,
            ToolStatus::Running => Color::Yellow,
            ToolStatus::Complete => {
                if self.is_error {
                    Color::Red
                } else {
                    Color::Green
                }
            }
            ToolStatus::Error => Color::Red,
        }
    }

    fn label(&self) -> String {
        self.summary.as_deref().unwrap_or(&self.name).to_string()
    }

    pub fn render_lines(&self, _width: u16) -> Vec<Line<'static>> {
        let glyph = self.status_glyph();
        let color = self.status_color();
        let label = self.label();

        let mut lines = Vec::new();

        if self.collapsed {
            let output_hint = self
                .output
                .as_ref()
                .map(|o| {
                    let n = o.lines().count();
                    format!(" ({n} lines)")
                })
                .unwrap_or_default();

            lines.push(Line::from(vec![
                Span::styled(
                    format!("[{name}] ", name = self.name),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(label, Style::default().fg(Color::White)),
                Span::styled(format!(" {glyph}{output_hint}"), Style::default().fg(color)),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("[{name}] ", name = self.name),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(label, Style::default().fg(Color::White)),
                Span::styled(format!(" {glyph}"), Style::default().fg(color)),
            ]));

            if let Some(output) = &self.output {
                let all: Vec<&str> = output.lines().collect();
                let show = all.len().min(OUTPUT_TRUNCATE_LIMIT);
                let style = if self.is_error {
                    Style::default().fg(Color::Red)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                for line in &all[..show] {
                    let line_style = if line.contains("Shell cwd was reset") {
                        Style::default().fg(Color::Yellow)
                    } else {
                        style
                    };
                    lines.push(Line::from(Span::styled(format!("  {line}"), line_style)));
                }
                if all.len() > OUTPUT_TRUNCATE_LIMIT {
                    lines.push(Line::from(Span::styled(
                        format!(
                            "  ... ({} lines hidden) [expand]",
                            all.len() - OUTPUT_TRUNCATE_LIMIT
                        ),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
        }

        lines
    }

    pub fn desired_height(&self, width: u16) -> u16 {
        self.render_lines(width).len() as u16
    }

    pub fn is_streaming(&self) -> bool {
        self.status == ToolStatus::Running
    }

    pub fn search_text(&self) -> String {
        let mut text = format!("{} {}", self.name, self.label());
        if let Some(output) = &self.output {
            text.push('\n');
            text.push_str(output);
        }
        text
    }

    pub fn tool_header(&self) -> Option<String> {
        match self.name.as_str() {
            "Grep" => {
                if let Some(output) = &self.output {
                    let file_count = output
                        .lines()
                        .filter(|l| !l.starts_with(' ') && !l.is_empty())
                        .count();
                    let match_count = output.lines().filter(|l| l.starts_with(' ')).count();
                    Some(format!(
                        "Found {match_count} matches across {file_count} files"
                    ))
                } else {
                    None
                }
            }
            "Read" => self.output.as_ref().map(|o| {
                let line_count = o.lines().count();
                format!("Read {line_count} lines")
            }),
            "WebFetch" => Some("Fetched".to_string()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapsed_single_line() {
        let tc = ToolCallState::new("Bash")
            .with_summary("ls -la")
            .with_status(ToolStatus::Complete)
            .with_output("file1\nfile2");
        let lines = tc.render_lines(80);
        assert_eq!(lines.len(), 1);
        let text: String = lines[0].spans.iter().map(|s| &*s.content).collect();
        assert!(text.contains("[Bash]"));
        assert!(text.contains("✓"));
        assert!(text.contains("2 lines"));
    }

    #[test]
    fn expanded_shows_output() {
        let mut tc = ToolCallState::new("Bash")
            .with_summary("ls")
            .with_status(ToolStatus::Complete)
            .with_output("file1\nfile2\nfile3");
        tc.collapsed = false;
        let lines = tc.render_lines(80);
        assert!(lines.len() >= 4);
    }

    #[test]
    fn running_is_streaming() {
        let tc = ToolCallState::new("Bash").with_status(ToolStatus::Running);
        assert!(tc.is_streaming());
    }

    #[test]
    fn error_status() {
        let tc = ToolCallState::new("Bash").with_status(ToolStatus::Error);
        let lines = tc.render_lines(80);
        let text: String = lines[0].spans.iter().map(|s| &*s.content).collect();
        assert!(text.contains("✗"));
    }

    #[test]
    fn truncation() {
        let output = (0..50)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut tc = ToolCallState::new("Bash")
            .with_status(ToolStatus::Complete)
            .with_output(output);
        tc.collapsed = false;
        let lines = tc.render_lines(80);
        let last: String = lines
            .last()
            .unwrap()
            .spans
            .iter()
            .map(|s| &*s.content)
            .collect();
        assert!(last.contains("lines hidden"));
    }

    #[test]
    fn toggle() {
        let mut tc = ToolCallState::new("Bash");
        assert!(tc.collapsed);
        tc.toggle_collapsed();
        assert!(!tc.collapsed);
    }

    #[test]
    fn cwd_reset_warning_yellow() {
        let mut tc = ToolCallState::new("Bash")
            .with_status(ToolStatus::Complete)
            .with_output("ok\nShell cwd was reset to /home\ndone");
        tc.collapsed = false;
        let lines = tc.render_lines(80);
        let warning_line = &lines[2];
        assert_eq!(warning_line.spans[0].style.fg, Some(Color::Yellow));
    }

    #[test]
    fn grep_header() {
        let tc =
            ToolCallState::new("Grep").with_output("file.rs\n match1\n match2\nother.rs\n match3");
        let header = tc.tool_header().unwrap();
        assert!(header.contains("3 matches"));
        assert!(header.contains("2 files"));
    }
}
