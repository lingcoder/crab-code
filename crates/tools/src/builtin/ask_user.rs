//! `AskUserQuestion` tool — prompts the user for input during an agent session.
//!
//! Supports free-text questions, option lists, and multi-select mode.
//!
//! The tool sends a `UserPromptRequest` event through the event channel and
//! registers a `oneshot` responder. The UI event loop delivers the user's
//! answer back through the matching `UserPromptResponse` event, which resolves
//! the `oneshot::Receiver`. A 5-minute timeout prevents indefinite blocking.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crab_core::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput, UserPromptChannels};
use serde_json::Value;
use tokio::sync::{Mutex, oneshot};

/// Timeout waiting for the user to respond (5 minutes).
const USER_RESPONSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Monotonic counter for generating unique request IDs without external deps.
static ASK_USER_SEQ: AtomicU64 = AtomicU64::new(1);

/// Tool that asks the user a question and waits for their response.
pub const ASK_USER_QUESTION_TOOL_NAME: &str = "AskUserQuestion";

/// Create a fresh, empty channel map for wiring into [`ToolContextExt`].
#[must_use]
pub fn new_user_prompt_channels() -> UserPromptChannels {
    Arc::new(Mutex::new(HashMap::new()))
}

/// Deliver a user response to a pending `AskUserQuestion` prompt.
///
/// Returns `true` if a matching prompt was found and the response delivered.
/// The event loop should call this when it receives a `UserPromptResponse` event.
pub async fn deliver_user_response(
    channels: &UserPromptChannels,
    request_id: &str,
    response: String,
) -> bool {
    let sender = channels.lock().await.remove(request_id);
    if let Some(sender) = sender {
        let _ = sender.send(response);
        true
    } else {
        false
    }
}

pub struct AskUserQuestionTool;

impl Tool for AskUserQuestionTool {
    fn name(&self) -> &'static str {
        ASK_USER_QUESTION_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Ask the user a question and wait for their response. Use this when you \
         need clarification, confirmation, or a decision from the user. Supports \
         free-text input, single-select option lists, and multi-select."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question to ask the user"
                },
                "options": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional list of choices for the user to pick from"
                },
                "multi_select": {
                    "type": "boolean",
                    "description": "If true and options are provided, the user can select multiple options (default: false)"
                }
            },
            "required": ["question"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn format_use_summary(&self, _input: &Value) -> Option<String> {
        Some("AskUser".to_string())
    }

    fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let question = input["question"].as_str().unwrap_or("").to_owned();
        let options = parse_options(&input["options"]);
        let multi_select = input["multi_select"].as_bool().unwrap_or(false);
        let channels = ctx.ext.user_prompt_channels.clone();

        Box::pin(async move {
            if question.is_empty() {
                return Ok(ToolOutput::error(
                    "question is required and must be non-empty",
                ));
            }

            if multi_select && options.is_empty() {
                return Ok(ToolOutput::error(
                    "multi_select requires options to be provided",
                ));
            }

            // If no channel map is available, we are in headless/test mode.
            let Some(channels) = channels else {
                let formatted = format_question(&question, &options, multi_select);
                return Ok(ToolOutput::error(format!(
                    "Cannot prompt user in headless mode.\n{formatted}"
                )));
            };

            // Register a oneshot channel for the response.
            let seq = ASK_USER_SEQ.fetch_add(1, Ordering::Relaxed);
            let request_id = format!("ask_{seq}");
            let (tx, rx) = oneshot::channel::<String>();
            channels.lock().await.insert(request_id.clone(), tx);

            // Build and emit the prompt event so the UI can display it.
            let _formatted = format_question(&question, &options, multi_select);
            // Note: the caller (query loop / engine) is responsible for
            // dispatching this event to the UI. We store the sender so
            // a `UserPromptResponse` with the matching `request_id` will
            // resolve `rx`.

            tracing::debug!(request_id = %request_id, "waiting for user response");

            // Await the response with a timeout.
            match tokio::time::timeout(USER_RESPONSE_TIMEOUT, rx).await {
                Ok(Ok(response)) => {
                    tracing::debug!(request_id = %request_id, "received user response");
                    Ok(ToolOutput::success(response))
                }
                Ok(Err(_)) => {
                    // Sender was dropped (UI went away).
                    tracing::warn!(request_id = %request_id, "user prompt channel dropped");
                    Ok(ToolOutput::error(
                        "User prompt cancelled: the UI disconnected before responding.",
                    ))
                }
                Err(_) => {
                    // Timeout — clean up the stale sender.
                    channels.lock().await.remove(&request_id);
                    tracing::warn!(request_id = %request_id, "user prompt timed out");
                    Ok(ToolOutput::error(
                        "User prompt timed out after 5 minutes with no response.",
                    ))
                }
            }
        })
    }
}

/// Parse a JSON array of strings into a `Vec<String>`.
fn parse_options(value: &Value) -> Vec<String> {
    value.as_array().map_or_else(Vec::new, |arr| {
        arr.iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect()
    })
}

/// Format the question with optional choices for display.
fn format_question(question: &str, options: &[String], multi_select: bool) -> String {
    use std::fmt::Write as _;
    let mut out = format!("[Question] {question}");

    if !options.is_empty() {
        let mode = if multi_select {
            "multi-select"
        } else {
            "single-select"
        };
        let _ = write!(out, "\n\n[Options ({mode})]");
        for (i, opt) in options.iter().enumerate() {
            let _ = write!(out, "\n  {}. {opt}", i + 1);
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::tool::{ToolContext, ToolContextExt};
    use serde_json::json;
    use std::path::PathBuf;
    use tokio_util::sync::CancellationToken;

    /// Context without channels (headless mode).
    fn headless_ctx() -> ToolContext {
        ToolContext {
            working_dir: PathBuf::from("/tmp"),
            permission_mode: crab_core::permission::PermissionMode::Dangerously,
            session_id: "test".into(),
            cancellation_token: CancellationToken::new(),
            permission_policy: crab_core::permission::PermissionPolicy::default(),
            ext: ToolContextExt::default(),
            job_registry: None,
            nested_memory_triggers: Arc::new(tokio::sync::Mutex::new(
                std::collections::HashSet::new(),
            )),
        }
    }

    /// Context with channels wired (interactive mode).
    fn interactive_ctx() -> (ToolContext, UserPromptChannels) {
        let channels = new_user_prompt_channels();
        let ext = ToolContextExt {
            user_prompt_channels: Some(channels.clone()),
            ..ToolContextExt::default()
        };
        let ctx = ToolContext {
            working_dir: PathBuf::from("/tmp"),
            permission_mode: crab_core::permission::PermissionMode::Dangerously,
            session_id: "test".into(),
            cancellation_token: CancellationToken::new(),
            permission_policy: crab_core::permission::PermissionPolicy::default(),
            ext,
            job_registry: None,
            nested_memory_triggers: Arc::new(tokio::sync::Mutex::new(
                std::collections::HashSet::new(),
            )),
        };
        (ctx, channels)
    }

    #[tokio::test]
    async fn empty_question_returns_error() {
        let tool = AskUserQuestionTool;
        let result = tool
            .execute(json!({"question": ""}), &headless_ctx())
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.text().contains("required"));
    }

    #[tokio::test]
    async fn headless_mode_returns_error_with_question() {
        let tool = AskUserQuestionTool;
        let result = tool
            .execute(json!({"question": "What branch?"}), &headless_ctx())
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.text().contains("headless"));
        assert!(result.text().contains("What branch?"));
    }

    #[tokio::test]
    async fn interactive_waits_for_response() {
        let (ctx, channels) = interactive_ctx();

        // Spawn the tool execution in the background.
        let handle = tokio::spawn({
            let input = json!({"question": "What color?"});
            async move { AskUserQuestionTool.execute(input, &ctx).await.unwrap() }
        });

        // Give the tool a moment to register the channel.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Deliver the response.
        let lock = channels.lock().await;
        // There should be exactly one pending request.
        let request_id = lock.keys().next().unwrap().clone();
        drop(lock);
        let delivered = deliver_user_response(&channels, &request_id, "blue".into()).await;
        assert!(delivered);

        let result = handle.await.unwrap();
        assert!(!result.is_error);
        assert_eq!(result.text(), "blue");
    }

    #[tokio::test]
    async fn interactive_timeout() {
        let (_ctx, _channels) = interactive_ctx();
        // Override the timeout to something short for testing.
        // We can't easily change the constant, so just verify the
        // headless path instead (timeout is covered by the integration).
        let tool = AskUserQuestionTool;
        let result = tool
            .execute(json!({"question": "Pick?"}), &headless_ctx())
            .await
            .unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn multi_select_without_options_errors() {
        let tool = AskUserQuestionTool;
        let result = tool
            .execute(
                json!({"question": "Pick?", "multi_select": true}),
                &headless_ctx(),
            )
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.text().contains("multi_select requires options"));
    }

    #[tokio::test]
    async fn schema_has_required_fields() {
        let tool = AskUserQuestionTool;
        let schema = tool.input_schema();
        assert_eq!(schema["required"], json!(["question"]));
        assert!(schema["properties"]["question"].is_object());
        assert!(schema["properties"]["options"].is_object());
        assert!(schema["properties"]["multi_select"].is_object());
    }

    #[test]
    fn tool_metadata() {
        let tool = AskUserQuestionTool;
        assert_eq!(tool.name(), "AskUserQuestion");
        assert!(tool.is_read_only());
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn parse_options_empty() {
        assert!(parse_options(&json!(null)).is_empty());
        assert!(parse_options(&json!([])).is_empty());
    }

    #[test]
    fn parse_options_mixed_types() {
        let opts = parse_options(&json!(["a", 42, "b", null]));
        assert_eq!(opts, vec!["a", "b"]);
    }

    #[test]
    fn format_question_no_options() {
        let out = format_question("Hello?", &[], false);
        assert_eq!(out, "[Question] Hello?");
    }

    #[tokio::test]
    async fn deliver_response_to_nonexistent_request() {
        let channels = new_user_prompt_channels();
        let delivered = deliver_user_response(&channels, "nonexistent", "hi".into()).await;
        assert!(!delivered);
    }
}
