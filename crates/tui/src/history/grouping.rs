//! Grouping pass that collapses consecutive read-only tool runs.
//!
//! Applied just before rendering the transcript. When the model fires
//! multiple read-only tools (`Read` / `Grep` / `Glob` / `NotebookRead`) in a
//! single turn, the raw message list interleaves `ToolUse` blocks with
//! `ToolResult` blocks and produces a large visually unpaired block. This
//! helper walks the messages once and replaces such runs with a single
//! [`CollapsedReadSearchCell`].
//!
//! Mirrors Claude Code's `applyGrouping` + `CollapsedReadSearchContent`
//! flow for a terminal-native immediate-mode renderer.

use crate::app::ChatMessage;
use crate::history::HistoryCell;
use crate::history::cell_from_chat_message;
use crate::history::cells::CollapsedReadSearchCell;

/// Minimum number of tool calls in a run before we collapse it.
const COLLAPSE_THRESHOLD: usize = 2;

/// Convert a message slice into renderable cells, collapsing consecutive
/// read-only tool runs of 2+ calls into a single summary cell.
#[must_use]
pub fn group_messages(messages: &[ChatMessage]) -> Vec<Box<dyn HistoryCell>> {
    let mut out: Vec<Box<dyn HistoryCell>> = Vec::with_capacity(messages.len());
    let mut i = 0;
    while i < messages.len() {
        let run_end = scan_read_only_run(messages, i);
        let run_len = run_end - i;
        if run_len > 0 && tool_call_count(&messages[i..run_end]) >= COLLAPSE_THRESHOLD {
            out.push(Box::new(CollapsedReadSearchCell::from_messages(
                &messages[i..run_end],
            )));
            i = run_end;
        } else {
            out.push(cell_from_chat_message(&messages[i]));
            i += 1;
        }
    }
    out
}

/// Return the index of the first message after `start` that is NOT a
/// read-only `ToolUse` / `ToolResult`. Each tool carries its own
/// `is_read_only` flag cached at push time from `Tool::is_read_only()`,
/// so this layer never needs a hardcoded tool-name list.
fn scan_read_only_run(messages: &[ChatMessage], start: usize) -> usize {
    let mut end = start;
    while end < messages.len() {
        let read_only = match &messages[end] {
            ChatMessage::ToolUse { is_read_only, .. }
            | ChatMessage::ToolResult { is_read_only, .. } => *is_read_only,
            _ => break,
        };
        if !read_only {
            break;
        }
        end += 1;
    }
    end
}

/// Count how many `ToolUse` messages a slice contains. Results are not
/// counted so the threshold applies to *call* count, not total messages.
fn tool_call_count(messages: &[ChatMessage]) -> usize {
    messages
        .iter()
        .filter(|m| matches!(m, ChatMessage::ToolUse { .. }))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_use() -> ChatMessage {
        ChatMessage::ToolUse {
            name: "Read".into(),
            summary: Some("Read (a.rs)".into()),
            color: None,
            is_read_only: true,
        }
    }

    fn read_result() -> ChatMessage {
        ChatMessage::ToolResult {
            tool_name: "Read".into(),
            output: "ok".into(),
            is_error: false,
            display: None,
            collapsed: false,
            is_read_only: true,
        }
    }

    fn bash_use() -> ChatMessage {
        ChatMessage::ToolUse {
            name: "Bash".into(),
            summary: Some("Run (ls)".into()),
            color: None,
            is_read_only: false,
        }
    }

    fn bash_result() -> ChatMessage {
        ChatMessage::ToolResult {
            tool_name: "Bash".into(),
            output: String::new(),
            is_error: false,
            display: None,
            collapsed: false,
            is_read_only: false,
        }
    }

    fn user_msg() -> ChatMessage {
        ChatMessage::User { text: "hi".into() }
    }

    fn downcast<T: 'static>(cell: &dyn HistoryCell) -> Option<&T> {
        cell.as_any().downcast_ref::<T>()
    }

    #[test]
    fn single_read_is_not_collapsed() {
        let msgs = vec![read_use(), read_result()];
        let cells = group_messages(&msgs);
        assert_eq!(cells.len(), 2);
        assert!(downcast::<CollapsedReadSearchCell>(cells[0].as_ref()).is_none());
    }

    #[test]
    fn two_reads_collapse_to_one_cell() {
        let msgs = vec![read_use(), read_use(), read_result(), read_result()];
        let cells = group_messages(&msgs);
        assert_eq!(cells.len(), 1);
        assert!(downcast::<CollapsedReadSearchCell>(cells[0].as_ref()).is_some());
    }

    #[test]
    fn bash_breaks_the_run() {
        // When a non-read-only tool splits a parallel read batch, the two
        // read tool_uses still collapse but their results — separated from
        // the uses by the bash — fall back to individual rendering since
        // the results-only slice has zero tool calls (below threshold).
        let msgs = vec![
            read_use(),
            read_use(),
            bash_use(),
            read_result(),
            read_result(),
            bash_result(),
        ];
        let cells = group_messages(&msgs);
        // Run 1: [read_use, read_use]      → collapsed (2 calls)
        // Run 2: [bash_use]                 → individual
        // Run 3: [read_result, read_result] → 2 individuals (0 calls)
        // Run 4: [bash_result]              → individual
        assert_eq!(cells.len(), 5);
        assert!(downcast::<CollapsedReadSearchCell>(cells[0].as_ref()).is_some());
    }

    #[test]
    fn user_message_ends_the_run() {
        let msgs = vec![read_use(), read_use(), user_msg(), read_use()];
        let cells = group_messages(&msgs);
        // collapsed(read×2) + user + read_use
        assert_eq!(cells.len(), 3);
        assert!(downcast::<CollapsedReadSearchCell>(cells[0].as_ref()).is_some());
        assert!(downcast::<CollapsedReadSearchCell>(cells[2].as_ref()).is_none());
    }

    #[test]
    fn empty_input_produces_no_cells() {
        let cells = group_messages(&[]);
        assert!(cells.is_empty());
    }
}
