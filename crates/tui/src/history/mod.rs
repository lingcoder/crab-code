//! History cells — polymorphic units of the conversation transcript.
//!
//! Each conversational artifact (user input, assistant reply, tool call,
//! tool result, system note) is a cell that owns its own rendering.
//! Cells compose into a `VecDeque<Box<dyn HistoryCell>>` in `App`; the
//! renderer iterates the queue and lays out cell by cell.
//!
//! The trait signature is aligned with Codex's `HistoryCell` so the two
//! crates' learnings can cross-pollinate, with one deliberate Crab
//! addition: [`HistoryCell::search_text`] exposes copy-searchable text
//! (for Ctrl+R history search and the in-buffer find bar) without
//! forcing every caller to re-flatten `display_lines`.

pub mod cells;

use std::any::Any;

use ratatui::text::Line;

pub use cells::{AssistantCell, SystemCell, ToolCallCell, ToolResultCell, UserCell};

/// A single unit of the transcript.
pub trait HistoryCell: Send + Sync + std::fmt::Debug {
    /// The lines painted into the main chat viewport at `width`.
    fn display_lines(&self, width: u16) -> Vec<Line<'static>>;

    /// Height this cell occupies at `width`. Implementations typically
    /// return `display_lines(width).len() as u16`; override only when a
    /// cheaper calculation is available.
    fn desired_height(&self, width: u16) -> u16 {
        let lines = self.display_lines(width).len();
        u16::try_from(lines).unwrap_or(u16::MAX)
    }

    /// The lines painted into the transcript overlay (Ctrl+O). Defaults
    /// to the main display, so most cells do not need to override.
    fn transcript_lines(&self, width: u16) -> Vec<Line<'static>> {
        self.display_lines(width)
    }

    /// Return a monotonic counter the cache can use to invalidate. For
    /// static cells return `None`; for animated cells (spinner, shimmer)
    /// return the current frame.
    fn transcript_animation_tick(&self) -> Option<u64> {
        None
    }

    /// Plain-text representation for search / copy / context-collapse.
    /// Defaults to joining `display_lines` at a large width so long
    /// text is not prematurely wrapped.
    fn search_text(&self) -> String {
        self.display_lines(u16::MAX)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|s| s.content.into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn as_any(&self) -> &dyn Any;
}

/// Adapter: build a boxed cell from a legacy [`crate::app::ChatMessage`].
///
/// This lets the existing `App::messages: Vec<ChatMessage>` feed the
/// new cell-based renderer without refactoring every call site at once.
#[must_use]
pub fn cell_from_chat_message(msg: &crate::app::ChatMessage) -> Box<dyn HistoryCell> {
    use crate::app::ChatMessage;

    match msg {
        ChatMessage::User { text } => Box::new(UserCell::new(text.clone())),
        ChatMessage::Assistant { text } => Box::new(AssistantCell::new(text.clone())),
        ChatMessage::ToolUse { name, summary } => {
            Box::new(ToolCallCell::new(name.clone(), summary.clone()))
        }
        ChatMessage::ToolResult {
            tool_name,
            output,
            is_error,
            display,
        } => Box::new(ToolResultCell::new(
            tool_name.clone(),
            output.clone(),
            *is_error,
            display.clone(),
        )),
        ChatMessage::System { text } => Box::new(SystemCell::new(text.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ChatMessage;

    #[test]
    fn adapter_covers_all_variants() {
        let cases = [
            ChatMessage::User { text: "u".into() },
            ChatMessage::Assistant { text: "a".into() },
            ChatMessage::ToolUse {
                name: "read".into(),
                summary: None,
            },
            ChatMessage::ToolResult {
                tool_name: "read".into(),
                output: "ok".into(),
                is_error: false,
                display: None,
            },
            ChatMessage::System { text: "s".into() },
        ];
        for msg in &cases {
            let cell = cell_from_chat_message(msg);
            let lines = cell.display_lines(80);
            assert!(!lines.is_empty(), "{msg:?} produced no lines");
        }
    }

    #[test]
    fn transcript_lines_defaults_to_display() {
        let cell = cell_from_chat_message(&ChatMessage::System { text: "hi".into() });
        let display = cell.display_lines(40);
        let transcript = cell.transcript_lines(40);
        assert_eq!(display.len(), transcript.len());
    }

    #[test]
    fn search_text_flattens_lines() {
        let cell = cell_from_chat_message(&ChatMessage::User {
            text: "hello world".into(),
        });
        assert!(cell.search_text().contains("hello world"));
    }
}
