//! Adapter: build a boxed `HistoryCell` from a legacy `ChatMessage` enum value.
//!
//! Lets the existing `App::messages: Vec<ChatMessage>` feed the cell-based
//! renderer without refactoring every call site at once.

use super::HistoryCell;
use super::cells::{self, AssistantCell, SystemCell, ToolCallCell, ToolResultCell, UserCell};

#[must_use]
pub fn cell_from_chat_message(msg: &crate::app::ChatMessage) -> Box<dyn HistoryCell> {
    use crate::app::ChatMessage;

    match msg {
        ChatMessage::User { text } => Box::new(UserCell::new(text.clone())),
        ChatMessage::Assistant {
            text,
            committed_lines,
            streaming,
        } => {
            Box::new(AssistantCell::with_skip(text.clone(), *committed_lines).streaming(*streaming))
        }
        ChatMessage::ToolUse {
            name,
            summary,
            color,
            status,
            ..
        } => Box::new(ToolCallCell::new(
            name.clone(),
            summary.clone(),
            *color,
            *status,
        )),
        ChatMessage::ToolResult {
            tool_name,
            output,
            is_error,
            display,
            collapsed,
            is_read_only: _,
        } => Box::new(ToolResultCell::new(
            tool_name.clone(),
            output.clone(),
            *is_error,
            display.clone(),
            *collapsed,
        )),
        ChatMessage::ToolProgress {
            tool_name,
            tail_output,
            total_lines,
            elapsed_secs,
            tool_use_id: _,
        } => Box::new(cells::ToolProgressCell::new(
            tool_name.clone(),
            tail_output.clone(),
            *total_lines,
            *elapsed_secs,
        )),
        ChatMessage::System { text, kind } => {
            Box::new(SystemCell::new(text.clone()).with_kind(*kind))
        }
        ChatMessage::CompactBoundary {
            strategy,
            after_tokens,
            removed_messages,
        } => Box::new(cells::CompactBoundaryCell::new(
            strategy.clone(),
            *after_tokens,
            *removed_messages,
        )),
        ChatMessage::PlanStep {
            title,
            steps,
            awaiting_approval,
        } => Box::new(cells::PlanStepCell::new(
            title.clone(),
            steps.clone(),
            *awaiting_approval,
        )),
        ChatMessage::ToolRejected {
            summary, display, ..
        } => Box::new(cells::ToolRejectedCell::new(
            summary.clone(),
            display.clone(),
        )),
        ChatMessage::Thinking {
            text,
            collapsed,
            duration,
        } => Box::new(cells::ThinkingCell::new(
            text.clone(),
            *collapsed,
            *duration,
        )),
        ChatMessage::Welcome {
            version,
            whats_new,
            show_project_hint,
            model,
            working_dir,
        } => Box::new(cells::WelcomeCell::new(
            version.clone(),
            whats_new.clone(),
            *show_project_hint,
            model.clone(),
            working_dir.clone(),
        )),
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
            ChatMessage::Assistant {
                streaming: false,
                text: "a".into(),
                committed_lines: 0,
            },
            ChatMessage::ToolUse {
                id: "tu_1".into(),
                name: "read".into(),
                summary: None,
                color: None,
                is_read_only: true,
                status: crate::app::ToolCallStatus::Running,
                collapsed_label: None,
            },
            ChatMessage::ToolResult {
                tool_name: "read".into(),
                output: "ok".into(),
                is_error: false,
                display: None,
                collapsed: false,
                is_read_only: true,
            },
            ChatMessage::System {
                text: "s".into(),
                kind: super::cells::SystemKind::Info,
            },
            ChatMessage::CompactBoundary {
                strategy: "summary".into(),
                after_tokens: 50000,
                removed_messages: 5,
            },
            ChatMessage::PlanStep {
                title: "Plan".into(),
                steps: vec![(
                    "Step 1".into(),
                    crate::components::plan_card::PlanStepStatus::Done,
                )],
                awaiting_approval: false,
            },
            ChatMessage::ToolRejected {
                tool_name: "bash".into(),
                summary: "Run rejected (ls)".into(),
                display: None,
            },
        ];
        for msg in &cases {
            let cell = cell_from_chat_message(msg);
            let lines = cell.display_lines(80);
            assert!(!lines.is_empty(), "{msg:?} produced no lines");
        }
    }

    #[test]
    fn transcript_lines_defaults_to_display() {
        let cell = cell_from_chat_message(&ChatMessage::System {
            text: "hi".into(),
            kind: super::cells::SystemKind::Info,
        });
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
