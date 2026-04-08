//! `Skill` tool — invokes a skill by name from the plugin system.
//!
//! Skills are prompt templates activated by slash commands, pattern matching,
//! or explicit invocation. This tool provides a programmatic way for the LLM
//! to trigger a skill by name with optional arguments.
//!
//! Maps to Claude Code's `SkillTool`.

use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput, ToolOutputContent};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

/// Canonical tool name for the Skill tool.
pub const SKILL_TOOL_NAME: &str = "Skill";

// ─── SkillTool ───────────────────────────────────────────────────────────

/// Tool that invokes a skill by name, optionally passing arguments.
///
/// When executed, the tool looks up the named skill in the plugin/skill
/// registry and returns a structured JSON action that the agent layer
/// intercepts to inject the skill's prompt content into the conversation.
///
/// # Input Schema
///
/// | Field    | Type   | Required | Description                             |
/// |----------|--------|----------|-----------------------------------------|
/// | `skill`  | string | yes      | Skill name or fully-qualified name      |
/// | `args`   | string | no       | Optional arguments passed to the skill  |
///
/// # Output
///
/// Returns a JSON action `{ "action": "invoke_skill", "skill": "...", "args": "..." }`
/// that the agent loop intercepts to load and inject the skill content.
///
/// # Examples
///
/// ```json
/// { "skill": "commit" }
/// { "skill": "review-pr", "args": "123" }
/// { "skill": "ms-office-suite:pdf" }
/// ```
pub struct SkillTool;

impl Tool for SkillTool {
    fn name(&self) -> &'static str {
        SKILL_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Execute a skill within the current conversation. Skills provide \
         specialized capabilities and domain knowledge. Use the skill name \
         (e.g. \"commit\", \"review-pr\") or a fully qualified name \
         (e.g. \"ms-office-suite:pdf\"). Optionally pass arguments."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "skill": {
                    "type": "string",
                    "description": "The skill name (e.g. \"commit\", \"review-pr\") or fully qualified name (e.g. \"ms-office-suite:pdf\")"
                },
                "args": {
                    "type": "string",
                    "description": "Optional arguments for the skill"
                }
            },
            "required": ["skill"]
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
        Box::pin(async move {
            let skill_name = input.get("skill").and_then(|v| v.as_str()).ok_or_else(|| {
                crab_common::Error::Other("missing required parameter: skill".into())
            })?;

            if skill_name.trim().is_empty() {
                return Ok(ToolOutput::error("skill name must not be empty"));
            }

            // Validate skill name: allow alphanumeric, hyphens, underscores, colons, dots
            if !is_valid_skill_name(skill_name) {
                return Ok(ToolOutput::error(
                    "skill name may only contain alphanumeric characters, \
                     hyphens, underscores, colons, and dots",
                ));
            }

            let args = input.get("args").and_then(|v| v.as_str()).map(String::from);

            // Parse fully-qualified skill names (e.g. "plugin-name:skill-name").
            let (plugin_prefix, resolved_name) = parse_qualified_name(skill_name);

            let action = serde_json::json!({
                "action": "invoke_skill",
                "skill": resolved_name,
                "plugin": plugin_prefix,
                "args": args,
                "original_name": skill_name,
            });

            Ok(ToolOutput::with_content(
                vec![ToolOutputContent::Json { value: action }],
                false,
            ))
        })
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────

/// Validate a skill name contains only allowed characters.
///
/// Allowed: alphanumeric, `-`, `_`, `:`, `.`
fn is_valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == ':' || c == '.')
}

/// Parse a fully-qualified skill name into (`plugin_prefix`, `skill_name`).
///
/// Format: `"plugin-name:skill-name"` or just `"skill-name"`.
///
/// Returns `(Some("plugin-name"), "skill-name")` for qualified names,
/// or `(None, "skill-name")` for simple names.
fn parse_qualified_name(name: &str) -> (Option<&str>, &str) {
    if let Some(colon_pos) = name.find(':') {
        let prefix = &name[..colon_pos];
        let skill = &name[colon_pos + 1..];
        if !prefix.is_empty() && !skill.is_empty() {
            return (Some(prefix), skill);
        }
    }
    (None, name)
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::permission::{PermissionMode, PermissionPolicy};
    use crab_core::tool::ToolContext;
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

    // ─── Metadata ───

    #[test]
    fn skill_tool_metadata() {
        let tool = SkillTool;
        assert_eq!(tool.name(), SKILL_TOOL_NAME);
        assert!(tool.is_read_only());
        assert!(!tool.requires_confirmation());
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn skill_tool_schema() {
        let schema = SkillTool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("skill")));
        assert_eq!(required.len(), 1);
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("skill"));
        assert!(props.contains_key("args"));
    }

    // ─── Execution ───

    #[tokio::test]
    async fn invoke_simple_skill() {
        let ctx = test_ctx();
        let input = json!({"skill": "commit"});
        let output = SkillTool.execute(input, &ctx).await.unwrap();
        assert!(!output.is_error);

        match &output.content[0] {
            ToolOutputContent::Json { value } => {
                assert_eq!(value["action"], "invoke_skill");
                assert_eq!(value["skill"], "commit");
                assert!(value["plugin"].is_null());
                assert!(value["args"].is_null());
            }
            _ => panic!("expected JSON output"),
        }
    }

    #[tokio::test]
    async fn invoke_skill_with_args() {
        let ctx = test_ctx();
        let input = json!({"skill": "review-pr", "args": "123"});
        let output = SkillTool.execute(input, &ctx).await.unwrap();
        assert!(!output.is_error);

        match &output.content[0] {
            ToolOutputContent::Json { value } => {
                assert_eq!(value["skill"], "review-pr");
                assert_eq!(value["args"], "123");
            }
            _ => panic!("expected JSON output"),
        }
    }

    #[tokio::test]
    async fn invoke_qualified_skill() {
        let ctx = test_ctx();
        let input = json!({"skill": "ms-office-suite:pdf"});
        let output = SkillTool.execute(input, &ctx).await.unwrap();
        assert!(!output.is_error);

        match &output.content[0] {
            ToolOutputContent::Json { value } => {
                assert_eq!(value["skill"], "pdf");
                assert_eq!(value["plugin"], "ms-office-suite");
                assert_eq!(value["original_name"], "ms-office-suite:pdf");
            }
            _ => panic!("expected JSON output"),
        }
    }

    #[tokio::test]
    async fn rejects_empty_skill_name() {
        let ctx = test_ctx();
        let input = json!({"skill": "  "});
        let output = SkillTool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("empty"));
    }

    #[tokio::test]
    async fn rejects_invalid_skill_name() {
        let ctx = test_ctx();
        let input = json!({"skill": "my skill!"});
        let output = SkillTool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("alphanumeric"));
    }

    #[tokio::test]
    async fn missing_skill_returns_error() {
        let ctx = test_ctx();
        let input = json!({});
        let result = SkillTool.execute(input, &ctx).await;
        assert!(result.is_err());
    }

    // ─── Helpers ───

    #[test]
    fn valid_skill_names() {
        assert!(is_valid_skill_name("commit"));
        assert!(is_valid_skill_name("review-pr"));
        assert!(is_valid_skill_name("ms-office-suite:pdf"));
        assert!(is_valid_skill_name("my_plugin.v2:skill-name"));
        assert!(!is_valid_skill_name("has space"));
        assert!(!is_valid_skill_name("special!char"));
        assert!(!is_valid_skill_name(""));
    }

    #[test]
    fn parse_qualified_simple() {
        let (prefix, name) = parse_qualified_name("commit");
        assert!(prefix.is_none());
        assert_eq!(name, "commit");
    }

    #[test]
    fn parse_qualified_with_plugin() {
        let (prefix, name) = parse_qualified_name("my-plugin:my-skill");
        assert_eq!(prefix, Some("my-plugin"));
        assert_eq!(name, "my-skill");
    }

    #[test]
    fn parse_qualified_leading_colon() {
        // ":skill" — empty prefix, treated as simple name
        let (prefix, name) = parse_qualified_name(":skill");
        assert!(prefix.is_none());
        assert_eq!(name, ":skill");
    }

    #[test]
    fn parse_qualified_trailing_colon() {
        // "plugin:" — empty skill, treated as simple name
        let (prefix, name) = parse_qualified_name("plugin:");
        assert!(prefix.is_none());
        assert_eq!(name, "plugin:");
    }
}
