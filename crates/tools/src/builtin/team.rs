use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput, ToolOutputContent};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

// ─── TeamCreateTool ───

/// Create a new team for multi-agent collaboration.
///
/// Returns a structured JSON action that the agent layer intercepts
/// to set up the team configuration directory and task store.
pub struct TeamCreateTool;

impl Tool for TeamCreateTool {
    fn name(&self) -> &'static str {
        "team_create"
    }

    fn description(&self) -> &'static str {
        "Create a new team for multi-agent collaboration"
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "team_name": {
                    "type": "string",
                    "description": "Name of the team to create"
                },
                "description": {
                    "type": "string",
                    "description": "Optional description of the team's purpose"
                }
            },
            "required": ["team_name"]
        })
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        Box::pin(async move {
            let team_name = input
                .get("team_name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    crab_common::Error::Other("missing required parameter: team_name".into())
                })?;

            if team_name.trim().is_empty() {
                return Ok(ToolOutput::error("team_name must not be empty"));
            }

            // Validate team name: only alphanumeric, hyphens, underscores
            if !team_name
                .chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
            {
                return Ok(ToolOutput::error(
                    "team_name may only contain alphanumeric characters, hyphens, and underscores",
                ));
            }

            let description = input
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let action = serde_json::json!({
                "action": "team_created",
                "team_name": team_name,
                "description": description,
            });

            Ok(ToolOutput::with_content(
                vec![ToolOutputContent::Json { value: action }],
                false,
            ))
        })
    }

    fn requires_confirmation(&self) -> bool {
        true
    }
}

// ─── TeamDeleteTool ───

/// Delete the current team and clean up its configuration.
///
/// Returns a structured JSON action that the agent layer intercepts
/// to remove the team directory and associated task store.
pub struct TeamDeleteTool;

impl Tool for TeamDeleteTool {
    fn name(&self) -> &'static str {
        "team_delete"
    }

    fn description(&self) -> &'static str {
        "Delete the current team and clean up its configuration"
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "team_name": {
                    "type": "string",
                    "description": "Name of the team to delete. If omitted, deletes the current team."
                }
            }
        })
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        Box::pin(async move {
            let team_name = input
                .get("team_name")
                .and_then(|v| v.as_str())
                .unwrap_or("current");

            let action = serde_json::json!({
                "action": "team_deleted",
                "team_name": team_name,
            });

            Ok(ToolOutput::with_content(
                vec![ToolOutputContent::Json { value: action }],
                false,
            ))
        })
    }

    fn requires_confirmation(&self) -> bool {
        true
    }
}

// ─── SendMessageTool ───

/// Send a message to another agent or broadcast to all team members.
///
/// Returns a structured JSON action that the agent layer intercepts
/// to route the message to the target agent(s).
pub struct SendMessageTool;

impl Tool for SendMessageTool {
    fn name(&self) -> &'static str {
        "send_message"
    }

    fn description(&self) -> &'static str {
        "Send a message to another agent by name, or broadcast to all with \"*\""
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
                    "description": "A short 5-10 word summary shown as a preview"
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
                .ok_or_else(|| {
                    crab_common::Error::Other("missing required parameter: to".into())
                })?;

            let message = input
                .get("message")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    crab_common::Error::Other("missing required parameter: message".into())
                })?;

            if to.trim().is_empty() {
                return Ok(ToolOutput::error("'to' must not be empty"));
            }

            if message.trim().is_empty() {
                return Ok(ToolOutput::error("message must not be empty"));
            }

            let summary = input
                .get("summary")
                .and_then(|v| v.as_str())
                .map(String::from);

            let is_broadcast = to == "*";

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
}

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
        }
    }

    // ─── TeamCreateTool ───

    #[test]
    fn team_create_metadata() {
        let tool = TeamCreateTool;
        assert_eq!(tool.name(), "team_create");
        assert!(tool.requires_confirmation());
        assert!(!tool.is_read_only());
    }

    #[test]
    fn team_create_schema() {
        let schema = TeamCreateTool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("team_name")));
        assert_eq!(required.len(), 1);
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("team_name"));
        assert!(props.contains_key("description"));
    }

    #[tokio::test]
    async fn team_create_basic() {
        let ctx = test_ctx();
        let input = json!({"team_name": "dev-team"});
        let output = TeamCreateTool.execute(input, &ctx).await.unwrap();
        assert!(!output.is_error);

        match &output.content[0] {
            ToolOutputContent::Json { value } => {
                assert_eq!(value["action"], "team_created");
                assert_eq!(value["team_name"], "dev-team");
                assert_eq!(value["description"], "");
            }
            _ => panic!("expected JSON output"),
        }
    }

    #[tokio::test]
    async fn team_create_with_description() {
        let ctx = test_ctx();
        let input = json!({
            "team_name": "phase2_team",
            "description": "Phase 2 feature development"
        });
        let output = TeamCreateTool.execute(input, &ctx).await.unwrap();
        assert!(!output.is_error);

        match &output.content[0] {
            ToolOutputContent::Json { value } => {
                assert_eq!(value["team_name"], "phase2_team");
                assert_eq!(value["description"], "Phase 2 feature development");
            }
            _ => panic!("expected JSON output"),
        }
    }

    #[tokio::test]
    async fn team_create_rejects_empty_name() {
        let ctx = test_ctx();
        let input = json!({"team_name": "  "});
        let output = TeamCreateTool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("empty"));
    }

    #[tokio::test]
    async fn team_create_rejects_invalid_name() {
        let ctx = test_ctx();
        let input = json!({"team_name": "my team!"});
        let output = TeamCreateTool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("alphanumeric"));
    }

    #[tokio::test]
    async fn team_create_missing_name() {
        let ctx = test_ctx();
        let input = json!({});
        let result = TeamCreateTool.execute(input, &ctx).await;
        assert!(result.is_err());
    }

    // ─── TeamDeleteTool ───

    #[test]
    fn team_delete_metadata() {
        let tool = TeamDeleteTool;
        assert_eq!(tool.name(), "team_delete");
        assert!(tool.requires_confirmation());
        assert!(!tool.is_read_only());
    }

    #[test]
    fn team_delete_schema() {
        let schema = TeamDeleteTool.input_schema();
        // No required fields
        assert!(schema.get("required").is_none());
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("team_name"));
    }

    #[tokio::test]
    async fn team_delete_with_name() {
        let ctx = test_ctx();
        let input = json!({"team_name": "old-team"});
        let output = TeamDeleteTool.execute(input, &ctx).await.unwrap();
        assert!(!output.is_error);

        match &output.content[0] {
            ToolOutputContent::Json { value } => {
                assert_eq!(value["action"], "team_deleted");
                assert_eq!(value["team_name"], "old-team");
            }
            _ => panic!("expected JSON output"),
        }
    }

    #[tokio::test]
    async fn team_delete_defaults_to_current() {
        let ctx = test_ctx();
        let input = json!({});
        let output = TeamDeleteTool.execute(input, &ctx).await.unwrap();
        assert!(!output.is_error);

        match &output.content[0] {
            ToolOutputContent::Json { value } => {
                assert_eq!(value["action"], "team_deleted");
                assert_eq!(value["team_name"], "current");
            }
            _ => panic!("expected JSON output"),
        }
    }

    // ─── SendMessageTool ───

    #[test]
    fn send_message_metadata() {
        let tool = SendMessageTool;
        assert_eq!(tool.name(), "send_message");
        assert!(!tool.requires_confirmation());
        assert!(!tool.is_read_only());
    }

    #[test]
    fn send_message_schema() {
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
    async fn send_message_basic() {
        let ctx = test_ctx();
        let input = json!({
            "to": "alice",
            "message": "Please review the PR"
        });
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
    async fn send_message_broadcast() {
        let ctx = test_ctx();
        let input = json!({
            "to": "*",
            "message": "Build is green, merging now"
        });
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
    async fn send_message_rejects_empty_to() {
        let ctx = test_ctx();
        let input = json!({"to": "  ", "message": "hello"});
        let output = SendMessageTool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("empty"));
    }

    #[tokio::test]
    async fn send_message_rejects_empty_message() {
        let ctx = test_ctx();
        let input = json!({"to": "alice", "message": "  "});
        let output = SendMessageTool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("empty"));
    }

    #[tokio::test]
    async fn send_message_missing_to() {
        let ctx = test_ctx();
        let input = json!({"message": "hello"});
        let result = SendMessageTool.execute(input, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn send_message_missing_message() {
        let ctx = test_ctx();
        let input = json!({"to": "alice"});
        let result = SendMessageTool.execute(input, &ctx).await;
        assert!(result.is_err());
    }

    // ─── Registration verification ───

    #[test]
    fn all_team_tools_have_valid_schemas() {
        let tools: Vec<Box<dyn Tool>> = vec![
            Box::new(TeamCreateTool),
            Box::new(TeamDeleteTool),
            Box::new(SendMessageTool),
        ];
        for tool in &tools {
            let schema = tool.input_schema();
            assert_eq!(schema["type"], "object");
            assert!(schema["properties"].is_object());
        }
    }
}
