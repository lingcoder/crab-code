//! Shared sub-agent spawn helpers used by both runtime facades.
//!
//! The Agent tool emits a `spawn_agent` marker into its tool-result text; these
//! helpers scan a turn's tool results for that marker, spawn each requested
//! worker into a [`WorkerPool`], and fold the worker's output back into the
//! conversation as an `<agent-result>` user message.

use std::sync::Arc;

use crab_api::LlmBackend;
use crab_core::event::Event;
use crab_core::message::{ContentBlock, Message};
use crab_core::tool::ToolContext;
use crab_engine::QueryConfig;
use crab_session::Conversation;
use crab_tools::executor::ToolExecutor;
use tokio::sync::mpsc;

use super::worker::WorkerResult;
use super::worker_pool::WorkerPool;

/// The marker `action` value the Agent tool emits for a spawn request.
pub const SPAWN_AGENT_ACTION: &str = "spawn_agent";

/// Scan `conversation[starting_len..]` tool-result text for `spawn_agent`
/// markers emitted by the Agent tool.
#[must_use]
pub fn scan_spawn_markers(
    conversation: &Conversation,
    starting_len: usize,
) -> Vec<serde_json::Value> {
    conversation
        .messages()
        .iter()
        .skip(starting_len)
        .flat_map(|m| m.content.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolResult { content, .. } => {
                serde_json::from_str::<serde_json::Value>(content).ok()
            }
            _ => None,
        })
        .filter(|v| v.get("action").and_then(serde_json::Value::as_str) == Some(SPAWN_AGENT_ACTION))
        .collect()
}

/// Spawn one worker from a parsed `spawn_agent` marker into `pool`, using the
/// caller-supplied worker executor (its tool registry) and base system prompt.
///
/// Returns the worker id, or `None` when the marker is not a spawn request. The
/// worker never persists into the parent session's crash log (its
/// `session_persister` is cleared) since it runs an independent conversation.
#[allow(clippy::too_many_arguments)]
pub fn spawn_worker_from_marker(
    pool: &mut WorkerPool,
    marker: &serde_json::Value,
    backend: &Arc<LlmBackend>,
    worker_executor: Arc<ToolExecutor>,
    parent_prompt: &str,
    tool_ctx: &ToolContext,
    loop_config: &QueryConfig,
    event_tx: &mpsc::Sender<Event>,
) -> Option<String> {
    if marker.get("action")?.as_str()? != SPAWN_AGENT_ACTION {
        return None;
    }
    let task = marker.get("task")?.as_str()?.to_string();
    let max_turns = marker
        .get("max_turns")
        .and_then(serde_json::Value::as_u64)
        .map(|v| usize::try_from(v).unwrap_or(usize::MAX));

    let system_prompt =
        format!("You are a sub-agent worker. Complete the assigned task.\n\n{parent_prompt}");

    let mut worker_ctx = tool_ctx.clone();
    if let Some(parent_mode) = marker
        .get("parent_permission_mode")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<crab_core::permission::PermissionMode>().ok())
    {
        worker_ctx.permission_mode = worker_ctx.permission_mode.restrict_to(parent_mode);
    }

    // The worker runs an independent conversation, so it must not append to the
    // parent session's JSONL crash log.
    let mut worker_config = loop_config.clone();
    worker_config.session_persister = None;

    Some(pool.spawn_worker(
        task,
        system_prompt,
        backend.clone(),
        worker_executor,
        worker_ctx,
        worker_config,
        event_tx.clone(),
        max_turns,
    ))
}

/// Format a finished worker's output as an `<agent-result>` user message the
/// parent model sees on its next turn.
#[must_use]
pub fn agent_result_message(result: &WorkerResult) -> Message {
    let status = if result.success {
        "completed"
    } else {
        "failed"
    };
    let output = result.output.as_deref().unwrap_or("(no output)");
    Message::user(format!(
        "<agent-result worker-id=\"{}\" status=\"{status}\">\n{output}\n</agent-result>",
        result.worker_id
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn convo_with_marker(marker: &str) -> Conversation {
        let mut c = Conversation::new("t".into(), String::new(), 1000);
        c.push(Message::tool_result("tu_1", marker, false));
        c
    }

    #[test]
    fn scan_finds_spawn_marker() {
        let marker = r#"{"action":"spawn_agent","task":"do x"}"#;
        let c = convo_with_marker(marker);
        let found = scan_spawn_markers(&c, 0);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0]["task"], "do x");
    }

    #[test]
    fn scan_ignores_other_markers_and_respects_start() {
        let c = convo_with_marker(r#"{"action":"team_created","team_name":"x"}"#);
        assert!(scan_spawn_markers(&c, 0).is_empty());
        // starting_len past the message skips it.
        let c2 = convo_with_marker(r#"{"action":"spawn_agent","task":"y"}"#);
        assert!(scan_spawn_markers(&c2, 1).is_empty());
    }

    #[test]
    fn agent_result_message_formats_status_and_output() {
        let ok = WorkerResult {
            worker_id: "worker_1".into(),
            output: Some("the answer".into()),
            success: true,
            usage: crab_core::model::TokenUsage::default(),
            conversation: Conversation::new("w".into(), String::new(), 0),
        };
        let msg = ok.success.then(|| agent_result_message(&ok)).unwrap();
        let text = msg.text();
        assert!(text.contains("worker-id=\"worker_1\""));
        assert!(text.contains("status=\"completed\""));
        assert!(text.contains("the answer"));

        let failed = WorkerResult {
            worker_id: "worker_2".into(),
            output: None,
            success: false,
            usage: crab_core::model::TokenUsage::default(),
            conversation: Conversation::new("w2".into(), String::new(), 0),
        };
        let text = agent_result_message(&failed).text();
        assert!(text.contains("status=\"failed\""));
        assert!(text.contains("(no output)"));
    }
}
