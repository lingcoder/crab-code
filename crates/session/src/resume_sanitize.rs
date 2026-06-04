//! Sanitize a resumed message history so it forms a valid request sequence.
//!
//! A session can be persisted mid-turn (crash recovery) with an assistant
//! `ToolUse` that has no matching `ToolResult`, or end on a tool result with no
//! following assistant turn. Sending such a sequence to the model errors or
//! wastes a turn, so it is repaired before the conversation is reloaded.

use std::collections::HashSet;

use crab_core::message::{ContentBlock, Message, Role};

/// Repair a resumed message list:
/// 1. Drop assistant `ToolUse` blocks whose matching `ToolResult` never arrived.
/// 2. Drop assistant messages left empty (or thinking-only) after step 1.
/// 3. If the history ends on a tool result with no following assistant turn,
///    append a synthetic user nudge so the model continues.
#[must_use]
pub fn sanitize_for_resume(messages: Vec<Message>) -> Vec<Message> {
    let result_ids: HashSet<String> = messages
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
            _ => None,
        })
        .collect();

    let mut out: Vec<Message> = Vec::with_capacity(messages.len());
    for msg in messages {
        if msg.role != Role::Assistant {
            out.push(msg);
            continue;
        }
        // Drop ToolUse blocks whose result never arrived (orphaned by a crash).
        let kept: Vec<ContentBlock> = msg
            .content
            .into_iter()
            .filter(|b| match b {
                ContentBlock::ToolUse { id, .. } => result_ids.contains(id),
                _ => true,
            })
            .collect();
        // Drop a message left with nothing but thinking (or nothing at all).
        let has_substance = kept
            .iter()
            .any(|b| !matches!(b, ContentBlock::Thinking { .. }));
        if has_substance {
            out.push(Message::new(Role::Assistant, kept));
        }
    }

    // A trailing tool result means the assistant still owes a turn; nudge it.
    if out.last().is_some_and(Message::has_tool_result) {
        out.push(Message::user("Continue from where you left off."));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool_use(id: &str) -> ContentBlock {
        ContentBlock::tool_use(id, "Bash", json!({"command": "ls"}))
    }

    #[test]
    fn drops_orphaned_tool_use() {
        // Assistant calls two tools but only one result came back.
        let messages = vec![
            Message::user("do it"),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::text("ok"), tool_use("a"), tool_use("b")],
            ),
            Message::tool_result("a", "done", false),
        ];
        let out = sanitize_for_resume(messages);
        // The orphaned ToolUse "b" is gone; "a" stays.
        let uses: Vec<&str> = out
            .iter()
            .flat_map(|m| m.content.iter())
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, .. } => Some(id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(uses, vec!["a"]);
    }

    #[test]
    fn drops_assistant_left_empty_after_orphan_removal() {
        let messages = vec![
            Message::user("go"),
            Message::new(Role::Assistant, vec![tool_use("x")]),
        ];
        let out = sanitize_for_resume(messages);
        // The assistant message had only an orphaned tool use -> removed; the
        // trailing user message remains (no tool result to nudge after).
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].role, Role::User);
    }

    #[test]
    fn appends_continue_after_trailing_tool_result() {
        let messages = vec![
            Message::user("go"),
            Message::new(Role::Assistant, vec![tool_use("a")]),
            Message::tool_result("a", "output", false),
        ];
        let out = sanitize_for_resume(messages);
        let last = out.last().unwrap();
        assert_eq!(last.role, Role::User);
        assert!(last.text().contains("Continue from where you left off"));
    }

    #[test]
    fn clean_history_is_unchanged() {
        let messages = vec![
            Message::user("hi"),
            Message::assistant("hello"),
            Message::user("bye"),
            Message::assistant("goodbye"),
        ];
        let out = sanitize_for_resume(messages.clone());
        assert_eq!(out.len(), messages.len());
        assert_eq!(out.last().unwrap().text(), "goodbye");
    }
}
