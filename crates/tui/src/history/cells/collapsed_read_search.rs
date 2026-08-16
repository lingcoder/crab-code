//! Collapsed cell for runs of read-only tool calls.
//!
//! When the model fires multiple read-only tools in parallel, the default
//! transcript would show every `tool_use` on its own line followed by every
//! `tool_result` on its own line, leaving calls and results visually orphaned.
//! This cell replaces such runs with a single summary like:
//!
//! ```text
//! ● Read 3 files, searched 2 patterns
//! ```
//!
//! The cell is fully tool-agnostic: it groups calls by the
//! `CollapsedGroupLabel` each tool provides via `Tool::collapsed_group_label()`.
//! Adding a new read-only tool requires zero changes here.

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crab_core::tool::CollapsedGroupLabel;

use crate::app::ChatMessage;
use crate::history::HistoryCell;
use crate::history::cell_from_chat_message;

/// Per-group count accumulated during construction.
#[derive(Debug, Clone)]
struct GroupCount {
    label: CollapsedGroupLabel,
    count: usize,
}

/// Aggregate counts for a collapsed run of read-only tool calls.
#[derive(Debug, Clone)]
pub struct CollapsedReadSearchCell {
    groups: Vec<GroupCount>,
    other_count: usize,
    pending_count: usize,
    any_error: bool,
    messages: Vec<ChatMessage>,
    /// Latest file or pattern being processed, shown while active.
    pub latest_hint: Option<String>,
}

impl CollapsedReadSearchCell {
    /// Build from a slice of consecutive read-only `ToolUse` / `ToolResult` messages.
    #[must_use]
    pub fn from_messages(messages: &[ChatMessage]) -> Self {
        let mut groups: Vec<GroupCount> = Vec::new();
        let mut other_count = 0usize;
        let mut call_count = 0usize;
        let mut result_count = 0usize;
        let mut any_error = false;

        for msg in messages {
            match msg {
                ChatMessage::ToolUse {
                    collapsed_label, ..
                } => {
                    call_count += 1;
                    if let Some(label) = collapsed_label {
                        if let Some(g) = groups.iter_mut().find(|g| g.label == *label) {
                            g.count += 1;
                        } else {
                            groups.push(GroupCount {
                                label: label.clone(),
                                count: 1,
                            });
                        }
                    } else {
                        other_count += 1;
                    }
                }
                ChatMessage::ToolResult { is_error, .. } => {
                    result_count += 1;
                    if *is_error {
                        any_error = true;
                    }
                }
                _ => {}
            }
        }

        Self {
            groups,
            other_count,
            pending_count: call_count.saturating_sub(result_count),
            any_error,
            messages: messages.to_vec(),
            latest_hint: None,
        }
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.pending_count > 0
    }

    #[must_use]
    pub fn read_count(&self) -> usize {
        self.groups.iter().map(|g| g.count).sum::<usize>() + self.other_count
    }

    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.pending_count
    }

    fn summary(&self) -> String {
        let active = self.is_active();
        let mut parts: Vec<String> = Vec::new();

        for g in &self.groups {
            let base = if active {
                g.label.active_verb
            } else {
                g.label.past_verb
            };
            let verb = if parts.is_empty() {
                base.to_string()
            } else {
                to_lowercase_start(base)
            };
            let noun = if g.count == 1 {
                g.label.noun_singular
            } else {
                g.label.noun_plural
            };
            parts.push(format!("{verb} {} {noun}", g.count));
        }

        if self.other_count > 0 {
            let verb: &str = if parts.is_empty() {
                if active { "Running" } else { "Ran" }
            } else if active {
                "running"
            } else {
                "ran"
            };
            let noun = if self.other_count == 1 {
                "call"
            } else {
                "calls"
            };
            parts.push(format!("{verb} {} other {noun}", self.other_count));
        }

        parts.join(", ")
    }
}

fn to_lowercase_start(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_lowercase().to_string() + chars.as_str(),
        None => String::new(),
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
        let dim_style = Style::default().fg(Color::DarkGray);

        let mut spans = vec![
            Span::styled("● ", Style::default().fg(icon_color)),
            Span::styled(self.summary(), text_style),
        ];
        if self.is_active() {
            spans.push(Span::styled("…", dim_style));
            spans.push(Span::styled("  Ctrl+O", dim_style));
        }

        let mut lines = vec![Line::from(spans)];

        if self.is_active()
            && let Some(hint) = &self.latest_hint
        {
            lines.push(Line::from(vec![
                Span::styled("  \u{23bf}  ", dim_style),
                Span::styled(hint.clone(), dim_style),
            ]));
        }

        lines.push(Line::default());
        lines
    }

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
            id: format!("tu_{path}"),
            name: "Read".into(),
            summary: Some(format!("Read ({path})")),
            color: None,
            is_read_only: true,
            status: crate::app::ToolCallStatus::Running,
            collapsed_label: Some(crab_core::tool::CollapsedGroupLabel {
                active_verb: "Reading",
                past_verb: "Read",
                noun_singular: "file",
                noun_plural: "files",
            }),
        }
    }

    fn read_result(lines: &str) -> ChatMessage {
        ChatMessage::ToolResult {
            tool_name: "Read".into(),
            output: lines.into(),
            is_error: false,
            display: None,
            collapsed: false,
            is_read_only: true,
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
        assert_eq!(cell.read_count(), 3);
        assert_eq!(cell.pending_count(), 0);
        assert!(!cell.is_active());
    }

    #[test]
    fn pending_counted_before_results_arrive() {
        let msgs = vec![read_use("a.rs"), read_use("b.rs")];
        let cell = CollapsedReadSearchCell::from_messages(&msgs);
        assert_eq!(cell.read_count(), 2);
        assert_eq!(cell.pending_count(), 2);
        assert!(cell.is_active());
    }

    #[test]
    fn summary_past_tense_when_complete() {
        let msgs = vec![
            read_use("a.rs"),
            read_use("b.rs"),
            read_result(""),
            read_result(""),
        ];
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
                id: "grep_tu".into(),
                name: "Grep".into(),
                summary: None,
                color: None,
                is_read_only: true,
                status: crate::app::ToolCallStatus::Running,
                collapsed_label: Some(crab_core::tool::CollapsedGroupLabel {
                    active_verb: "Searching for",
                    past_verb: "Searched for",
                    noun_singular: "pattern",
                    noun_plural: "patterns",
                }),
            },
            read_result(""),
            ChatMessage::ToolResult {
                tool_name: "Grep".into(),
                output: String::new(),
                is_error: false,
                display: None,
                collapsed: false,
                is_read_only: true,
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
                is_read_only: true,
            },
        ];
        let cell = CollapsedReadSearchCell::from_messages(&msgs);
        assert!(cell.any_error);
    }

    #[test]
    fn display_produces_summary_line() {
        let msgs = vec![
            read_use("a.rs"),
            read_use("b.rs"),
            read_result(""),
            read_result(""),
        ];
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
