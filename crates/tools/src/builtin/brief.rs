//! `BriefTool` — conversation summary and context handoff.
//!
//! Generates a concise summary of the current conversation or a specific
//! scope within it, useful for context compression and agent handoff.

use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

/// Tool name constant for `BriefTool`.
pub const BRIEF_TOOL_NAME: &str = "Brief";

/// Conversation summary / handoff tool.
///
/// Input:
/// - `scope`: Optional scope to summarize (e.g. `"recent"`, `"tools"`, `"all"`)
pub struct BriefTool;

impl Tool for BriefTool {
    fn name(&self) -> &'static str {
        BRIEF_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Generate a concise summary of the current conversation or a specific \
         scope within it. Useful for context compression, handoff between agents, \
         or reviewing what has been accomplished."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "scope": {
                    "type": "string",
                    "description": "Scope to summarize: 'recent' (last few turns), 'tools' (tool usage), or 'all' (entire conversation). Defaults to 'all'."
                }
            }
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
        let scope = input["scope"].as_str().unwrap_or("all").to_owned();

        Box::pin(async move { generate_brief(&scope).await })
    }
}

/// Generate a brief summary for the given scope.
async fn generate_brief(scope: &str) -> Result<ToolOutput> {
    let _ = scope;
    todo!("BriefTool::generate_brief — extract conversation history and produce summary")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_metadata() {
        let tool = BriefTool;
        assert_eq!(tool.name(), "Brief");
        assert!(tool.is_read_only());
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn schema_no_required_fields() {
        let schema = BriefTool.input_schema();
        assert!(schema.get("required").is_none());
        assert!(schema["properties"]["scope"].is_object());
    }
}
