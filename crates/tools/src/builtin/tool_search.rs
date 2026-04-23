//! `ToolSearchTool` — search available tools by name or description.
//!
//! Helps the LLM discover tools when the full tool list is too large
//! to include in the system prompt. Supports fuzzy name matching and
//! keyword search in descriptions.

use std::fmt::Write;
use std::future::Future;

use crab_core::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use serde_json::Value;
use std::pin::Pin;

/// Tool name constant for `ToolSearchTool`.
pub const TOOL_SEARCH_TOOL_NAME: &str = "ToolSearch";

/// Tool discovery via search.
///
/// Input:
/// - `query`: Search query to match against tool names and descriptions
pub struct ToolSearchTool;

impl Tool for ToolSearchTool {
    fn name(&self) -> &'static str {
        TOOL_SEARCH_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Search available tools by name or description. Returns a list of \
         matching tools with their names, descriptions, and input schemas. \
         Useful when the full tool list is too large to browse."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query to match against tool names and descriptions"
                }
            },
            "required": ["query"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let query = input["query"].as_str().unwrap_or("").to_owned();
        let tool_descs = ctx.ext.tool_descriptions.clone();

        Box::pin(async move {
            if query.is_empty() {
                return Ok(ToolOutput::error("query must be non-empty"));
            }
            search_tools(&query, &tool_descs)
        })
    }

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        input["query"].as_str().map(|q| format!("ToolSearch ({q})"))
    }
}

/// Search the tool registry for tools matching the query.
fn search_tools(query: &str, tool_descriptions: &[String]) -> Result<ToolOutput> {
    if tool_descriptions.is_empty() {
        return Ok(ToolOutput::success(format!(
            "Tool search for '{query}': no tool descriptions available in current context. \
             The agent loop populates tool_descriptions when creating the ToolContext."
        )));
    }

    let q = query.to_lowercase();
    let mut matches: Vec<(&str, u32)> = tool_descriptions
        .iter()
        .filter_map(|desc| {
            let lower = desc.to_lowercase();
            let mut score = 0u32;
            for word in q.split_whitespace() {
                if lower.contains(word) {
                    score += 10;
                }
            }
            if score > 0 {
                Some((desc.as_str(), score))
            } else {
                None
            }
        })
        .collect();

    matches.sort_by_key(|m| std::cmp::Reverse(m.1));

    if matches.is_empty() {
        return Ok(ToolOutput::success(format!(
            "No tools found matching '{query}'. Try a different search term."
        )));
    }

    let mut result = format!("Found {} tools matching '{query}':\n\n", matches.len());
    for (desc, _) in matches.iter().take(10) {
        let _ = writeln!(result, "- {desc}");
    }
    Ok(ToolOutput::success(result))
}

/// Score how well a tool matches a search query.
///
/// Returns a score >= 0. Higher is better. 0 means no match.
#[must_use]
pub fn match_score(query: &str, tool_name: &str, tool_description: &str) -> u32 {
    let q = query.to_lowercase();
    let name = tool_name.to_lowercase();
    let desc = tool_description.to_lowercase();

    let mut score = 0u32;

    // Exact name match
    if name == q {
        score += 100;
    } else if name.contains(&q) {
        score += 50;
    }

    // Description keyword match
    for word in q.split_whitespace() {
        if desc.contains(word) {
            score += 10;
        }
    }

    score
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_metadata() {
        let tool = ToolSearchTool;
        assert_eq!(tool.name(), "ToolSearch");
        assert!(tool.is_read_only());
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn schema_requires_query() {
        let schema = ToolSearchTool.input_schema();
        assert_eq!(schema["required"], serde_json::json!(["query"]));
    }

    #[test]
    fn match_score_exact_name() {
        let score = match_score("bash", "Bash", "Execute shell commands");
        assert!(score >= 100);
    }

    #[test]
    fn match_score_partial_name() {
        let score = match_score("bas", "Bash", "Execute shell commands");
        assert!(score >= 50);
    }

    #[test]
    fn match_score_description_keyword() {
        let score = match_score("shell", "Bash", "Execute shell commands");
        assert!(score > 0);
    }

    #[test]
    fn match_score_no_match() {
        let score = match_score("xyz", "Bash", "Execute shell commands");
        assert_eq!(score, 0);
    }
}
