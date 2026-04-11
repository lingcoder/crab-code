use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput, ToolOutputContent, ToolSource};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

use crate::str_utils::truncate_chars;

/// Sub-agent spawning tool.
///
/// This tool does not directly spawn a sub-agent (it lives in `crab-tools`,
/// which cannot depend on `crab-agent`). Instead, it validates the input
/// and returns a structured JSON output with `"action": "spawn_agent"` that
/// the agent layer's query loop intercepts to spawn an `AgentWorker`.
pub const AGENT_TOOL_NAME: &str = "Agent";

pub struct AgentTool;

impl Tool for AgentTool {
    fn name(&self) -> &'static str {
        AGENT_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Spawn a sub-agent to perform a task independently"
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "Description of the task for the sub-agent to perform"
                },
                "model": {
                    "type": "string",
                    "description": "Optional model ID override for the sub-agent (e.g. 'claude-sonnet-4-20250514')"
                },
                "working_dir": {
                    "type": "string",
                    "description": "Optional working directory override for the sub-agent (absolute path)"
                },
                "max_turns": {
                    "type": "integer",
                    "description": "Maximum number of turns before the sub-agent is stopped (default: 20)"
                }
            },
            "required": ["task"]
        })
    }

    fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        // Clone ctx fields into owned values so the async block holds no borrows on `ctx`.
        let default_working_dir = ctx.working_dir.display().to_string();
        let session_id = ctx.session_id.clone();

        Box::pin(async move {
            let task = input.get("task").and_then(|v| v.as_str()).ok_or_else(|| {
                crab_common::Error::Other("missing required parameter: task".into())
            })?;

            if task.trim().is_empty() {
                return Ok(ToolOutput::error("task description must not be empty"));
            }

            let model = input
                .get("model")
                .and_then(|v| v.as_str())
                .map(String::from);

            let working_dir = input
                .get("working_dir")
                .and_then(|v| v.as_str())
                .map(String::from);

            // Validate working_dir is absolute if provided
            if let Some(ref wd) = working_dir
                && !std::path::Path::new(wd).is_absolute()
            {
                return Ok(ToolOutput::error(format!(
                    "working_dir must be an absolute path, got: {wd}"
                )));
            }

            let max_turns = input
                .get("max_turns")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(20);

            // Build the spawn request as structured JSON for the agent layer
            let spawn_request = serde_json::json!({
                "action": "spawn_agent",
                "task": task,
                "model": model,
                "working_dir": working_dir.unwrap_or(default_working_dir),
                "max_turns": max_turns,
                "session_id": session_id,
            });

            Ok(ToolOutput::with_content(
                vec![ToolOutputContent::Json {
                    value: spawn_request,
                }],
                false,
            ))
        })
    }

    fn source(&self) -> ToolSource {
        ToolSource::AgentSpawn
    }

    fn requires_confirmation(&self) -> bool {
        true
    }

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        let desc = input["description"]
            .as_str()
            .or_else(|| input["task"].as_str())
            .unwrap_or("agent task");
        // LLM-generated descriptions can contain any Unicode — byte slicing is
        // unsafe. truncate_chars handles multi-byte input correctly.
        let truncated = truncate_chars(desc, 77, "…");
        Some(format!("Agent ({truncated})"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::permission::PermissionPolicy;
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    fn test_ctx() -> ToolContext {
        ToolContext {
            working_dir: std::path::PathBuf::from("/tmp/project"),
            permission_mode: crab_core::permission::PermissionMode::Dangerously,
            session_id: "test_session".into(),
            cancellation_token: CancellationToken::new(),
            permission_policy: PermissionPolicy::default(),
            ext: crab_core::tool::ToolContextExt::default(),
        }
    }

    #[test]
    fn tool_metadata() {
        let tool = AgentTool;
        assert_eq!(tool.name(), "Agent");
        assert!(tool.requires_confirmation());
        assert!(!tool.is_read_only());
        assert!(matches!(tool.source(), ToolSource::AgentSpawn));
    }

    #[test]
    fn schema_has_required_fields() {
        let schema = AgentTool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("task")));
        assert_eq!(required.len(), 1);
    }

    #[test]
    fn schema_has_optional_fields() {
        let schema = AgentTool.input_schema();
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("task"));
        assert!(props.contains_key("model"));
        assert!(props.contains_key("working_dir"));
        assert!(props.contains_key("max_turns"));
    }

    #[tokio::test]
    async fn execute_returns_spawn_request() {
        let ctx = test_ctx();
        let input = json!({
            "task": "Fix the bug in auth module"
        });

        let output = AgentTool.execute(input, &ctx).await.unwrap();
        assert!(!output.is_error);

        // Should contain structured JSON output
        let ToolOutputContent::Json {
            value: json_content,
        } = &output.content[0]
        else {
            panic!("expected JSON output");
        };
        assert_eq!(json_content["action"], "spawn_agent");
        assert_eq!(json_content["task"], "Fix the bug in auth module");
        assert_eq!(json_content["max_turns"], 20);
        assert_eq!(json_content["session_id"], "test_session");
    }

    #[tokio::test]
    async fn execute_with_optional_params() {
        let ctx = test_ctx();
        let abs_dir = std::env::temp_dir().join("agent_test_project");
        let abs_dir_str = abs_dir.to_string_lossy().to_string();
        let input = json!({
            "task": "Review code",
            "model": "claude-sonnet-4-20250514",
            "working_dir": abs_dir_str,
            "max_turns": 10
        });

        let output = AgentTool.execute(input, &ctx).await.unwrap();
        assert!(!output.is_error);

        let ToolOutputContent::Json {
            value: json_content,
        } = &output.content[0]
        else {
            panic!("expected JSON output");
        };
        assert_eq!(json_content["model"], "claude-sonnet-4-20250514");
        assert_eq!(json_content["working_dir"], abs_dir_str);
        assert_eq!(json_content["max_turns"], 10);
    }

    #[tokio::test]
    async fn execute_uses_ctx_working_dir_as_default() {
        let ctx = test_ctx();
        let input = json!({"task": "do something"});

        let output = AgentTool.execute(input, &ctx).await.unwrap();
        let ToolOutputContent::Json {
            value: json_content,
        } = &output.content[0]
        else {
            panic!("expected JSON output");
        };
        assert_eq!(json_content["working_dir"], "/tmp/project");
    }

    #[tokio::test]
    async fn execute_rejects_empty_task() {
        let ctx = test_ctx();
        let input = json!({"task": "  "});

        let output = AgentTool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("empty"));
    }

    #[tokio::test]
    async fn execute_rejects_relative_working_dir() {
        let ctx = test_ctx();
        let input = json!({
            "task": "do something",
            "working_dir": "relative/path"
        });

        let output = AgentTool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("absolute"));
    }

    #[tokio::test]
    async fn execute_missing_task() {
        let ctx = test_ctx();
        let input = json!({});
        let result = AgentTool.execute(input, &ctx).await;
        assert!(result.is_err());
    }
}
