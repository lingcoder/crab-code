//! `AskUserQuestion` tool — prompts the user for input during an agent session.
//!
//! Supports free-text questions, option lists, and multi-select mode.

use crab_core::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

/// Tool that asks the user a question and returns their response.
///
/// In the current stub implementation, the tool formats the question and
/// options for display but returns a placeholder response. A real
/// implementation would hook into the TUI/CLI input system.
pub const ASK_USER_QUESTION_TOOL_NAME: &str = "AskUserQuestion";

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
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let question = input["question"].as_str().unwrap_or("").to_owned();
        let options = parse_options(&input["options"]);
        let multi_select = input["multi_select"].as_bool().unwrap_or(false);

        Box::pin(async move {
            if question.is_empty() {
                return Ok(ToolOutput::error(
                    "question is required and must be non-empty",
                ));
            }

            // Validate options
            if multi_select && options.is_empty() {
                return Ok(ToolOutput::error(
                    "multi_select requires options to be provided",
                ));
            }

            // Build the formatted question for display.
            // In a real implementation, this would be forwarded to the TUI/CLI
            // input handler and block until the user responds.
            let formatted = format_question(&question, &options, multi_select);

            // TODO: In Phase 2, integrate with the TUI event loop to actually
            // prompt the user and capture their response. For now, return the
            // formatted question as the output.
            Ok(ToolOutput::success(formatted))
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
    use crab_core::tool::ToolContext;
    use serde_json::json;
    use std::path::PathBuf;
    use tokio_util::sync::CancellationToken;

    fn test_ctx() -> ToolContext {
        ToolContext {
            working_dir: PathBuf::from("/tmp"),
            permission_mode: crab_core::permission::PermissionMode::Dangerously,
            session_id: "test".into(),
            cancellation_token: CancellationToken::new(),
            permission_policy: crab_core::permission::PermissionPolicy::default(),
            ext: crab_core::tool::ToolContextExt::default(),
        }
    }

    #[tokio::test]
    async fn empty_question_returns_error() {
        let tool = AskUserQuestionTool;
        let result = tool
            .execute(json!({"question": ""}), &test_ctx())
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.text().contains("required"));
    }

    #[tokio::test]
    async fn simple_question() {
        let tool = AskUserQuestionTool;
        let result = tool
            .execute(json!({"question": "What branch?"}), &test_ctx())
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.text().contains("What branch?"));
    }

    #[tokio::test]
    async fn question_with_options() {
        let tool = AskUserQuestionTool;
        let result = tool
            .execute(
                json!({
                    "question": "Pick a color",
                    "options": ["red", "green", "blue"]
                }),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains("Pick a color"));
        assert!(text.contains("single-select"));
        assert!(text.contains("1. red"));
        assert!(text.contains("2. green"));
        assert!(text.contains("3. blue"));
    }

    #[tokio::test]
    async fn multi_select_with_options() {
        let tool = AskUserQuestionTool;
        let result = tool
            .execute(
                json!({
                    "question": "Select features",
                    "options": ["auth", "logging"],
                    "multi_select": true
                }),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.text().contains("multi-select"));
    }

    #[tokio::test]
    async fn multi_select_without_options_errors() {
        let tool = AskUserQuestionTool;
        let result = tool
            .execute(
                json!({"question": "Pick?", "multi_select": true}),
                &test_ctx(),
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
}
