//! `SendMessage` tool — inter-agent messaging within the session's team.
//!
//! Every session has one implicit team: teammates come into existence when
//! the `Agent` tool is called with a `name`, and are addressed here by that
//! name (or `"*"` for broadcast). There is no separate team lifecycle to
//! manage.
//!
//! The tool itself is transport-free: it validates input and emits a
//! `message_sent` JSON action that the agent layer intercepts to route the
//! message to the target teammate(s).

// ─── Protocol documentation ─────────────────────────────────────────────
//
// The SendMessage tool supports two message shapes:
//
// 1. **Plain text message** — a string `message` with a `summary` preview:
//    ```json
//    {
//      "to": "researcher",
//      "message": "Please review the PR",
//      "summary": "PR review request"
//    }
//    ```
//
// 2. **Structured protocol message** — a JSON object in `message` for
//    system-level coordination (shutdown requests, plan approvals):
//    ```json
//    {
//      "to": "team-lead",
//      "message": {
//        "type": "shutdown_response",
//        "request_id": "abc123",
//        "approve": true
//      }
//    }
//    ```
//
// The tool itself does not interpret structured messages — it passes them
// through as JSON actions for the agent layer to handle.

use crab_core::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput, ToolOutputContent};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

use crate::str_utils::truncate_chars;

/// Canonical tool name.
pub const SEND_MESSAGE_TOOL_NAME: &str = "SendMessage";

/// Maximum allowed message length (characters).
const MAX_MESSAGE_LENGTH: usize = 100_000;

/// Maximum allowed summary length (characters).
const MAX_SUMMARY_LENGTH: usize = 200;

/// Send a message to another agent by name, or broadcast to all teammates.
///
/// Returns a structured JSON action that the agent/orchestrator layer
/// intercepts to route the message to the target agent(s).
///
/// # Input Schema
///
/// | Field     | Type   | Required | Description                                     |
/// |-----------|--------|----------|-------------------------------------------------|
/// | `to`      | string | yes      | Recipient teammate name, or `"*"` for broadcast |
/// | `message` | string | yes      | Message content (plain text or JSON)            |
/// | `summary` | string | no       | Short 5-10 word preview shown in the UI         |
///
/// # Validation
///
/// - `to` and `message` must be non-empty.
/// - `to` must be a bare teammate name or `"*"` — there is only one team
///   per session, so qualified `name@team` addresses are rejected.
/// - `message` length is capped at 100,000 characters.
/// - `summary` is truncated to 200 characters.
pub struct SendMessageTool;

impl Tool for SendMessageTool {
    fn name(&self) -> &'static str {
        SEND_MESSAGE_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Send a message to a named teammate spawned via the Agent tool, or \
         broadcast to all teammates with \"*\". Use a summary for a short \
         preview shown in the UI. Messages are delivered asynchronously."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "to": {
                    "type": "string",
                    "description": "Recipient teammate name, or \"*\" to broadcast to all teammates"
                },
                "message": {
                    "type": "string",
                    "description": "The message content to send"
                },
                "summary": {
                    "type": "string",
                    "description": "A short 5-10 word summary shown as a preview in the UI"
                }
            },
            "required": ["to", "message"]
        })
    }

    fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let session_id = ctx.session_id.clone();

        Box::pin(async move {
            let to = input
                .get("to")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crab_core::Error::Other("missing required parameter: to".into()))?;

            let message = input
                .get("message")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    crab_core::Error::Other("missing required parameter: message".into())
                })?;

            if to.trim().is_empty() {
                return Ok(ToolOutput::error("'to' must not be empty"));
            }

            if to.contains('@') {
                return Ok(ToolOutput::error(
                    "to must be a bare teammate name or \"*\" — there is only one team per session",
                ));
            }

            if message.trim().is_empty() {
                return Ok(ToolOutput::error("message must not be empty"));
            }

            if message.len() > MAX_MESSAGE_LENGTH {
                return Ok(ToolOutput::error(format!(
                    "message exceeds maximum length of {MAX_MESSAGE_LENGTH} characters"
                )));
            }

            let summary = input
                .get("summary")
                .and_then(|v| v.as_str())
                .map(|s| truncate_chars(s, MAX_SUMMARY_LENGTH, "…"));

            let is_broadcast = to == "*";

            let action = serde_json::json!({
                "action": "message_sent",
                "to": to,
                "message": message,
                "summary": summary,
                "is_broadcast": is_broadcast,
                "from_session": session_id,
            });

            // Emit both a Json block (structured consumers) and a Text block
            // carrying the same payload verbatim — the agent layer scans
            // conversation tool-result text for the `message_sent` marker,
            // and ToolOutput::text() drops Json blocks.
            let text = serde_json::to_string(&action).unwrap_or_default();

            Ok(ToolOutput::with_content(
                vec![
                    ToolOutputContent::Json { value: action },
                    ToolOutputContent::Text { text },
                ],
                false,
            ))
        })
    }

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        let to = input["to"].as_str().unwrap_or("?");
        Some(format!("SendMessage (to: {to})"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::permission::{PermissionMode, PermissionPolicy};
    use serde_json::json;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    fn test_ctx() -> ToolContext {
        ToolContext {
            working_dir: std::path::PathBuf::from("/tmp/project"),
            permission_mode: PermissionMode::Dangerously,
            session_id: "test_session".into(),
            cancellation_token: CancellationToken::new(),
            permission_policy: PermissionPolicy::default(),
            ext: crab_core::tool::ToolContextExt::default(),
            job_registry: None,
            nested_memory_triggers: Arc::new(tokio::sync::Mutex::new(
                std::collections::HashSet::new(),
            )),
        }
    }

    #[test]
    fn metadata() {
        let tool = SendMessageTool;
        assert_eq!(tool.name(), "SendMessage");
        assert!(!tool.requires_confirmation());
        assert!(!tool.is_read_only());
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn schema_has_required_fields() {
        let schema = SendMessageTool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("to")));
        assert!(required.contains(&json!("message")));
        assert_eq!(required.len(), 2);
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("to"));
        assert!(props.contains_key("message"));
        assert!(props.contains_key("summary"));
    }

    #[tokio::test]
    async fn send_basic_message() {
        let ctx = test_ctx();
        let input = json!({"to": "alice", "message": "Please review the PR"});
        let output = SendMessageTool.execute(input, &ctx).await.unwrap();
        assert!(!output.is_error);

        match &output.content[0] {
            ToolOutputContent::Json { value } => {
                assert_eq!(value["action"], "message_sent");
                assert_eq!(value["to"], "alice");
                assert_eq!(value["message"], "Please review the PR");
                assert!(value["summary"].is_null());
                assert_eq!(value["is_broadcast"], false);
                assert_eq!(value["from_session"], "test_session");
            }
            _ => panic!("expected JSON output"),
        }
    }

    #[tokio::test]
    async fn send_message_with_summary() {
        let ctx = test_ctx();
        let input = json!({
            "to": "bob",
            "message": "The auth module refactoring is complete",
            "summary": "auth refactor done"
        });
        let output = SendMessageTool.execute(input, &ctx).await.unwrap();
        assert!(!output.is_error);

        match &output.content[0] {
            ToolOutputContent::Json { value } => {
                assert_eq!(value["summary"], "auth refactor done");
            }
            _ => panic!("expected JSON output"),
        }
    }

    #[tokio::test]
    async fn summary_truncated_char_safe() {
        let ctx = test_ctx();
        let long_summary = "预".repeat(MAX_SUMMARY_LENGTH + 50);
        let input = json!({"to": "bob", "message": "hi", "summary": long_summary});
        let output = SendMessageTool.execute(input, &ctx).await.unwrap();
        assert!(!output.is_error);

        match &output.content[0] {
            ToolOutputContent::Json { value } => {
                let stored = value["summary"].as_str().unwrap();
                assert!(stored.chars().count() <= MAX_SUMMARY_LENGTH + 1);
                assert!(stored.ends_with('…'));
            }
            _ => panic!("expected JSON output"),
        }
    }

    #[tokio::test]
    async fn broadcast_message() {
        let ctx = test_ctx();
        let input = json!({"to": "*", "message": "Build is green, merging now"});
        let output = SendMessageTool.execute(input, &ctx).await.unwrap();
        assert!(!output.is_error);

        match &output.content[0] {
            ToolOutputContent::Json { value } => {
                assert_eq!(value["to"], "*");
                assert_eq!(value["is_broadcast"], true);
            }
            _ => panic!("expected JSON output"),
        }
    }

    #[tokio::test]
    async fn emits_text_marker_for_scanning() {
        let ctx = test_ctx();
        let output = SendMessageTool
            .execute(json!({"to": "alice", "message": "hi"}), &ctx)
            .await
            .unwrap();
        // The Text block must carry the message_sent marker so the agent
        // layer's tool-result scan finds it (Json blocks are dropped by text()).
        let text = output.text();
        assert!(
            text.contains("\"action\":\"message_sent\""),
            "marker missing from text output: {text}"
        );
    }

    #[tokio::test]
    async fn rejects_qualified_address() {
        let ctx = test_ctx();
        let input = json!({"to": "alice@other-team", "message": "hi"});
        let output = SendMessageTool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("one team per session"));
    }

    #[tokio::test]
    async fn rejects_empty_to() {
        let ctx = test_ctx();
        let input = json!({"to": "  ", "message": "hello"});
        let output = SendMessageTool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("empty"));
    }

    #[tokio::test]
    async fn rejects_empty_message() {
        let ctx = test_ctx();
        let input = json!({"to": "alice", "message": "  "});
        let output = SendMessageTool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("empty"));
    }

    #[tokio::test]
    async fn rejects_oversized_message() {
        let ctx = test_ctx();
        let input = json!({"to": "alice", "message": "x".repeat(MAX_MESSAGE_LENGTH + 1)});
        let output = SendMessageTool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("maximum length"));
    }

    #[tokio::test]
    async fn missing_to_errors() {
        let ctx = test_ctx();
        let input = json!({"message": "hello"});
        let result = SendMessageTool.execute(input, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn missing_message_errors() {
        let ctx = test_ctx();
        let input = json!({"to": "alice"});
        let result = SendMessageTool.execute(input, &ctx).await;
        assert!(result.is_err());
    }
}
