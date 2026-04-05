//! `EnterPlanMode` tool — switches the agent into planning mode.
//!
//! When invoked, this tool signals that the agent should enter a structured
//! planning phase (e.g., outlining steps before executing). The actual mode
//! transition is handled by the agent loop; this tool returns a confirmation.

use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use serde_json::Value;
use std::fmt::Write;
use std::future::Future;
use std::pin::Pin;

/// Tool that triggers a transition to plan mode in the agent session.
pub struct EnterPlanModeTool;

impl Tool for EnterPlanModeTool {
    fn name(&self) -> &'static str {
        "enter_plan_mode"
    }

    fn description(&self) -> &'static str {
        "Switch the agent into planning mode. In plan mode, the agent outlines \
         a structured plan before executing any changes. Use this when facing a \
         complex task that benefits from upfront planning. Optionally provide an \
         initial plan description."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "description": {
                    "type": "string",
                    "description": "Optional description of what the plan should cover"
                },
                "steps": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional initial list of planned steps"
                }
            },
            "required": []
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let description = input["description"].as_str().unwrap_or("").to_owned();
        let steps = parse_steps(&input["steps"]);

        Box::pin(async move {
            let mut output = String::from("[Plan Mode Activated]");

            if !description.is_empty() {
                let _ = write!(output, "\n\nObjective: {description}");
            }

            if !steps.is_empty() {
                output.push_str("\n\nPlanned steps:");
                for (i, step) in steps.iter().enumerate() {
                    let _ = write!(output, "\n  {}. {step}", i + 1);
                }
            }

            // TODO: In Phase 2, set a flag on the agent session state
            // (e.g., `session.mode = AgentMode::Planning`) so the agent loop
            // can adjust its behavior (e.g., only produce plan text, no tool
            // calls until the user approves the plan).
            Ok(ToolOutput::success(output))
        })
    }
}

/// Parse a JSON array of strings into step descriptions.
fn parse_steps(value: &Value) -> Vec<String> {
    value.as_array().map_or_else(Vec::new, |arr| {
        arr.iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect()
    })
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
        }
    }

    #[tokio::test]
    async fn basic_plan_mode() {
        let tool = EnterPlanModeTool;
        let result = tool.execute(json!({}), &test_ctx()).await.unwrap();
        assert!(!result.is_error);
        assert!(result.text().contains("Plan Mode Activated"));
    }

    #[tokio::test]
    async fn plan_with_description() {
        let tool = EnterPlanModeTool;
        let result = tool
            .execute(
                json!({"description": "Refactor the auth module"}),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains("Plan Mode Activated"));
        assert!(text.contains("Objective: Refactor the auth module"));
    }

    #[tokio::test]
    async fn plan_with_steps() {
        let tool = EnterPlanModeTool;
        let result = tool
            .execute(
                json!({
                    "description": "Add caching",
                    "steps": ["Audit current queries", "Add Redis layer", "Write tests"]
                }),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains("1. Audit current queries"));
        assert!(text.contains("2. Add Redis layer"));
        assert!(text.contains("3. Write tests"));
    }

    #[tokio::test]
    async fn plan_empty_description_and_steps() {
        let tool = EnterPlanModeTool;
        let result = tool
            .execute(json!({"description": "", "steps": []}), &test_ctx())
            .await
            .unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert_eq!(text, "[Plan Mode Activated]");
    }

    #[tokio::test]
    async fn schema_has_no_required_fields() {
        let tool = EnterPlanModeTool;
        let schema = tool.input_schema();
        assert_eq!(schema["required"], json!([]));
        assert!(schema["properties"]["description"].is_object());
        assert!(schema["properties"]["steps"].is_object());
    }

    #[test]
    fn tool_metadata() {
        let tool = EnterPlanModeTool;
        assert_eq!(tool.name(), "enter_plan_mode");
        assert!(tool.is_read_only());
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn parse_steps_empty() {
        assert!(parse_steps(&json!(null)).is_empty());
        assert!(parse_steps(&json!([])).is_empty());
    }

    #[test]
    fn parse_steps_filters_non_strings() {
        let steps = parse_steps(&json!(["a", 1, "b", null, "c"]));
        assert_eq!(steps, vec!["a", "b", "c"]);
    }
}
