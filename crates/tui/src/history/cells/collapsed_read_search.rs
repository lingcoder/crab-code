//! Collapsed cell for runs of read-only tool calls.
//!
//! When the model fires multiple `Read` / `Grep` / `Glob` / `NotebookRead`
//! calls in parallel, the default transcript would show every `tool_use` on
//! its own line followed by every `tool_result` on its own line, leaving
//! calls and results visually orphaned. This cell replaces such runs with a
//! single summary like:
//!
//! ```text
//! ● Read 3 files, searched 2 patterns
//! ```
//!
//! Mirrors Claude Code's non-verbose `CollapsedReadSearchContent`. Only
//! read-only tools are collapsed; any mutating tool (`Bash`, `Edit`, etc.)
//! breaks the run and is rendered normally.

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::app::ChatMessage;
use crate::history::HistoryCell;
use crate::history::cell_from_chat_message;

/// Read-only tool names that participate in the collapsed run.
pub const READ_ONLY_TOOLS: &[&str] = &["Read", "Grep", "Glob", "NotebookRead"];

/// Aggregate counts for a collapsed run.
#[derive(Debug, Clone, Default)]
pub struct CollapsedReadSearchCell {
    read_count: usize,
    grep_count: usize,
    glob_count: usize,
    notebook_read_count: usize,
    /// Tool uses whose matching result has not arrived yet.
    pending_count: usize,
    /// Any tool in the run reported an error.
    any_error: bool,
    /// Original messages, retained for transcript-mode expansion.
    messages: Vec<ChatMessage>,
}

impl CollapsedReadSearchCell {
    /// Build from a slice of consecutive read-only `ToolUse` / `ToolResult` messages.
    #[must_use]
    pub fn from_messages(messages: &[ChatMessage]) -> Self {
        let mut this = Self {
            messages: messages.to_vec(),
            ..Self::default()
        };

        let mut call_count = 0usize;
        let mut result_count = 0usize;
        for msg in messages {
            match msg {
                ChatMessage::ToolUse { name, .. } => {
                    call_count += 1;
                    match name.as_str() {
                        "Read" => this.read_count += 1,
                        "Grep" => this.grep_count += 1,
                        "Glob" => this.glob_count += 1,
                        "NotebookRead" => this.notebook_read_count += 1,
                        _ => {}
                    }
                }
                ChatMessage::ToolResult { is_error, .. } => {
                    result_count += 1;
                    if *is_error {
                        this.any_error = true;
                    }
                }
                _ => {}
            }
        }
        this.pending_count = call_count.saturating_sub(result_count);
        this
    }

    /// Whether any call is still waiting for its result.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.pending_count > 0
    }

    /// Plain-english one-line summary. Uses present-tense verbs while active,
    /// past-tense once all results have arrived.
    fn summary(&self) -> String {
        let active = self.is_active();
        let mut parts: Vec<String> = Vec::new();

        if self.read_count > 0 {
            let verb = if parts.is_empty() {
                if active { "Reading" } else { "Read" }
            } else if active {
                "reading"
            } else {
                "read"
            };
            let noun = if self.read_count == 1 { "file" } else { "files" };
            parts.push(format!("{verb} {} {noun}", self.read_count));
        }
        if self.grep_count > 0 {
            let verb = if parts.is_empty() {
                if active { "Searching for" } else { "Searched for" }
            } else if active {
                "searching for"
            } else {
                "searched for"
            };
            let noun = if self.grep_count == 1 {
                "pattern"
            } else {
                "patterns"
            };
            parts.push(format!("{verb} {} {noun}", self.grep_count));
        }
        if self.glob_count > 0 {
            let verb = if parts.is_empty() {
                if active { "Globbing" } else { "Globbed" }
            } else if active {
                "globbing"
            } else {
                "globbed"
            };
            let noun = if self.glob_count == 1 {
                "pattern"
            } else {
                "patterns"
            };
            parts.push(format!("{verb} {} {noun}", self.glob_count));
        }
        if self.notebook_read_count > 0 {
            let verb = if parts.is_empty() {
                if active { "Reading" } else { "Read" }
            } else if active {
                "reading"
            } else {
                "read"
            };
            let noun = if self.notebook_read_count == 1 {
                "notebook"
            } else {
                "notebooks"
            };
            parts.push(format!("{verb} {} {noun}", self.notebook_read_count));
        }

        parts.join(", ")
    }
}

impl HistoryCell for CollapsedReadSearchCell {
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        let icon_color = if self.any_error {
            Color::Red
        } else if self.is_active() {
            Color::DarkGray
        } else {
            Color::Green
        };
        let text_style = Style::default().fg(Color::Gray);

        let mut spans = vec![
            Span::styled("● ", Style::default().fg(icon_color)),
            Span::styled(self.summary(), text_style),
        ];
        if self.is_active() {
            spans.push(Span::styled("…", Style::default().fg(Color::DarkGray)));
        }

        vec![Line::from(spans), Line::default()]
    }

    /// Transcript mode expands the group: render every original message
    /// through its normal cell so users can inspect individual calls.
    fn transcript_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut out = Vec::new();
        for msg in &self.messages {
            let cell = cell_from_chat_message(msg);
            out.extend(cell.transcript_lines(width));
        }
        out
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_use(path: &str) -> ChatMessage {
        ChatMessage::ToolUse {
            name: "Read".into(),
            summary: Some(format!("Read ({path})")),
            color: None,
        }
    }

    fn read_result(lines: &str) -> ChatMessage {
        ChatMessage::ToolResult {
            tool_name: "Read".into(),
            output: lines.into(),
            is_error: false,
            display: None,
            collapsed: false,
        }
    }

    #[test]
    fn counts_three_reads_pairs_with_results() {
        let msgs = vec![
            read_use("a.rs"),
            read_use("b.rs"),
            read_use("c.rs"),
            read_result("1\n2"),
            read_result("1"),
            read_result("1\n2\n3"),
        ];
        let cell = CollapsedReadSearchCell::from_messages(&msgs);
        assert_eq!(cell.read_count, 3);
        assert_eq!(cell.pending_count, 0);
        assert!(!cell.is_active());
    }

    #[test]
    fn pending_counted_before_results_arrive() {
        let msgs = vec![read_use("a.rs"), read_use("b.rs")];
        let cell = CollapsedReadSearchCell::from_messages(&msgs);
        assert_eq!(cell.read_count, 2);
        assert_eq!(cell.pending_count, 2);
        assert!(cell.is_active());
    }

    #[test]
    fn summary_past_tense_when_complete() {
        let msgs = vec![read_use("a.rs"), read_use("b.rs"), read_result(""), read_result("")];
        let cell = CollapsedReadSearchCell::from_messages(&msgs);
        assert_eq!(cell.summary(), "Read 2 files");
    }

    #[test]
    fn summary_present_tense_when_active() {
        let msgs = vec![read_use("a.rs"), read_use("b.rs")];
        let cell = CollapsedReadSearchCell::from_messages(&msgs);
        assert_eq!(cell.summary(), "Reading 2 files");
    }

    #[test]
    fn summary_mixes_read_and_grep() {
        let msgs = vec![
            read_use("a.rs"),
            ChatMessage::ToolUse {
                name: "Grep".into(),
                summary: None,
                color: None,
            },
            read_result(""),
            ChatMessage::ToolResult {
                tool_name: "Grep".into(),
                output: String::new(),
                is_error: false,
                display: None,
                collapsed: false,
            },
        ];
        let cell = CollapsedReadSearchCell::from_messages(&msgs);
        assert_eq!(cell.summary(), "Read 1 file, searched for 1 pattern");
    }

    #[test]
    fn error_flag_propagates() {
        let msgs = vec![
            read_use("a.rs"),
            ChatMessage::ToolResult {
                tool_name: "Read".into(),
                output: "boom".into(),
                is_error: true,
                display: None,
                collapsed: false,
            },
        ];
        let cell = CollapsedReadSearchCell::from_messages(&msgs);
        assert!(cell.any_error);
    }

    #[test]
    fn display_produces_summary_line() {
        let msgs = vec![read_use("a.rs"), read_use("b.rs"), read_result(""), read_result("")];
        let cell = CollapsedReadSearchCell::from_messages(&msgs);
        let lines = cell.display_lines(80);
        // summary line + trailing blank
        assert_eq!(lines.len(), 2);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("Read 2 files"));
    }

    #[test]
    fn transcript_lines_expand_to_originals() {
        let msgs = vec![read_use("a.rs"), read_result("one\ntwo")];
        let cell = CollapsedReadSearchCell::from_messages(&msgs);
        let transcript = cell.transcript_lines(80);
        // Should include at least both the call header and the result body.
        let text: String = transcript
            .iter()
            .flat_map(|line| line.spans.iter().map(|s| s.content.as_ref().to_string()))
            .collect::<Vec<_>>()
            .join("|");
        assert!(text.contains("Read"), "transcript text was: {text}");
    }
}
