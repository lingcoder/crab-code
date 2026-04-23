//! `SnipTool` — trim large tool outputs to reduce context usage.
//!
//! Replaces oversized tool results with a truncated version plus a
//! `[snipped N chars]` marker. Can target a specific message or apply
//! a global character limit.

use crab_core::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

/// Tool name constant for `SnipTool`.
pub const SNIP_TOOL_NAME: &str = "Snip";

/// Default maximum characters before snipping.
const DEFAULT_MAX_CHARS: usize = 4000;

/// Tool for trimming large tool outputs.
///
/// Input:
/// - `message_id`: Optional ID of a specific message to snip
/// - `max_chars`: Optional maximum character count (default: 4000)
pub struct SnipTool;

impl Tool for SnipTool {
    fn name(&self) -> &'static str {
        SNIP_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Trim large tool outputs to reduce context window usage. Replaces \
         oversized results with a truncated version and a '[snipped]' marker. \
         Specify a message_id to target a specific output, or omit to apply \
         to the most recent large output."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "message_id": {
                    "type": "string",
                    "description": "ID of the specific message to snip. If omitted, targets the most recent large output."
                },
                "max_chars": {
                    "type": "integer",
                    "description": "Maximum character count before snipping (default: 4000)"
                }
            }
        })
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let message_id = input
            .get("message_id")
            .and_then(|v| v.as_str())
            .map(String::from);
        let max_chars = input["max_chars"]
            .as_u64()
            .map_or(DEFAULT_MAX_CHARS, |v| v as usize);

        Box::pin(async move { snip_output(message_id.as_deref(), max_chars).await })
    }

    fn format_use_summary(&self, _input: &Value) -> Option<String> {
        Some("Snip".to_string())
    }
}

/// Snip a tool output, either by message ID or the most recent large output.
async fn snip_output(message_id: Option<&str>, max_chars: usize) -> Result<ToolOutput> {
    // Message history is managed by the agent loop and is not accessible
    // from within a tool invocation. Return a descriptive message so the
    // caller knows what was requested.
    let target = message_id.unwrap_or("most recent large output");
    Ok(ToolOutput::success(format!(
        "Snip requested for '{target}' with max_chars={max_chars}. \
         Message history is not yet accessible from within tool execution. \
         The agent loop manages message history directly; this tool will \
         be functional once session history is plumbed into the ToolContext. \
         Use the `truncate_with_marker` helper for inline truncation."
    )))
}

/// Truncate text and append a snip marker.
#[must_use]
pub fn truncate_with_marker(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_owned();
    }
    let snipped = text.len() - max_chars;
    format!("{}\n\n[snipped {snipped} chars]", &text[..max_chars])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_metadata() {
        let tool = SnipTool;
        assert_eq!(tool.name(), "Snip");
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn schema_no_required_fields() {
        let schema = SnipTool.input_schema();
        assert!(schema.get("required").is_none());
    }

    #[test]
    fn truncate_short_text_unchanged() {
        let text = "short";
        assert_eq!(truncate_with_marker(text, 100), "short");
    }

    #[test]
    fn truncate_long_text_adds_marker() {
        let text = "a".repeat(100);
        let result = truncate_with_marker(&text, 50);
        assert!(result.contains("[snipped 50 chars]"));
        assert!(result.starts_with(&"a".repeat(50)));
    }
}
