//! Standalone `SendMessage` tool module — cross-agent message sending.
//!
//! This module provides the `SendMessageTool` as a standalone implementation
//! separate from the `team` module. The `team` module already contains a
//! `SendMessageTool`; this module exists as an alternative entry point that
//! can be registered independently when team features are not needed but
//! inter-agent messaging is.
//!
//! Maps to Claude Code's `SendMessageTool`.
//!
//! # Difference from `team::SendMessageTool`
//!
//! This implementation is identical in behavior but lives in its own module
//! for organizational clarity. The canonical registration in `mod.rs` uses
//! `team::SendMessageTool`. This module re-exports a type alias for
//! discoverability and documents the message protocol.

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

/// Canonical tool name.
pub const SEND_MESSAGE_TOOL_NAME: &str = "SendMessage";

/// Maximum allowed message length (characters).
const MAX_MESSAGE_LENGTH: usize = 100_000;

/// Maximum allowed summary length (characters).
const MAX_SUMMARY_LENGTH: usize = 200;

// ─── StandaloneSendMessageTool ──────────────────────────────────────────

/// Send a message to another agent by name, or broadcast to all teammates.
///
/// Returns a structured JSON action that the agent/orchestrator layer
/// intercepts to route the message to the target agent(s).
///
/// # Input Schema
///
/// | Field     | Type   | Required | Description                                     |
/// |-----------|--------|----------|-------------------------------------------------|
/// | `to`      | string | yes      | Recipient agent name, or `"*"` for broadcast    |
/// | `message` | string | yes      | Message content (plain text or JSON)             |
/// | `summary` | string | no       | Short 5-10 word preview shown in the UI          |
///
/// # Validation
///
/// - `to` and `message` must be non-empty.
/// - `message` length is capped at 100,000 characters.
/// - `summary` length is capped at 200 characters.
pub struct StandaloneSendMessageTool;

impl Tool for StandaloneSendMessageTool {
    fn name(&self) -> &'static str {
        SEND_MESSAGE_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Send a message to another agent by name, or broadcast to all \
         teammates with \"*\". Use a summary for a short preview shown in \
         the UI. Messages are delivered asynchronously."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "to": {
                    "type": "string",
                    "description": "Recipient agent name, or \"*\" to broadcast to all teammates"
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
            // ── Extract and validate parameters ──

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
                .map(|s| {
                    if s.len() > MAX_SUMMARY_LENGTH {
                        &s[..MAX_SUMMARY_LENGTH]
                    } else {
                        s
                    }
                })
                .map(String::from);

            let is_broadcast = to == "*";

            // ── Build action payload ──

            let action = serde_json::json!({
                "action": "message_sent",
                "to": to,
                "message": message,
                "summary": summary,
                "is_broadcast": is_broadcast,
                "from_session": session_id,
            });

            Ok(ToolOutput::with_content(
                vec![ToolOutputContent::Json { value: action }],
                false,
            ))
        })
    }

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        let to = input["to"].as_str().unwrap_or("?");
        Some(format!("SendMessage (to: {to})"))
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::permission::{PermissionMode, PermissionPolicy};
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    fn test_ctx() -> ToolContext {
        ToolContext {
            working_dir: std::path::PathBuf::from("/tmp/project"),
            permission_mode: PermissionMode::Dangerously,
            session_id: "test_session".into(),
            cancellation_token: CancellationToken::new(),
            permission_policy: PermissionPolicy::default(),
            ext: crab_core::tool::ToolContextExt::default(),
        }
    }

    #[test]
    fn metadata() {
        let tool = StandaloneSendMessageTool;
        assert_eq!(tool.name(), "SendMessage");
        assert!(!tool.requires_confirmation());
        assert!(!tool.is_read_only());
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn schema_has_required_fields() {
        let schema = StandaloneSendMessageTool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("to")));
        assert!(required.contains(&json!("message")));
        assert_eq!(required.len(), 2);
    }

    #[tokio::test]
    async fn send_basic_message() {
        let ctx = test_ctx();
        let input = json!({"to": "alice", "message": "hello"});
        let output = StandaloneSendMessageTool
            .execute(input, &ctx)
            .await
            .unwrap();
        assert!(!output.is_error);

        match &output.content[0] {
            ToolOutputContent::Json { value } => {
                assert_eq!(value["action"], "message_sent");
                assert_eq!(value["to"], "alice");
                assert_eq!(value["message"], "hello");
                assert_eq!(value["is_broadcast"], false);
            }
            _ => panic!("expected JSON output"),
        }
    }

    #[tokio::test]
    async fn broadcast_message() {
        let ctx = test_ctx();
        let input = json!({"to": "*", "message": "team update"});
        let output = StandaloneSendMessageTool
            .execute(input, &ctx)
            .await
            .unwrap();
        assert!(!output.is_error);

        match &output.content[0] {
            ToolOutputContent::Json { value } => {
                assert_eq!(value["is_broadcast"], true);
            }
            _ => panic!("expected JSON output"),
        }
    }

    #[tokio::test]
    async fn rejects_empty_to() {
        let ctx = test_ctx();
        let input = json!({"to": "  ", "message": "hi"});
        let output = StandaloneSendMessageTool
            .execute(input, &ctx)
            .await
            .unwrap();
        assert!(output.is_error);
    }

    #[tokio::test]
    async fn rejects_empty_message() {
        let ctx = test_ctx();
        let input = json!({"to": "bob", "message": "  "});
        let output = StandaloneSendMessageTool
            .execute(input, &ctx)
            .await
            .unwrap();
        assert!(output.is_error);
    }
}
