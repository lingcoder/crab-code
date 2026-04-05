//! Web search tool — searches the web and returns results.
//!
//! Currently returns stub results. Real API integration (e.g., Brave Search,
//! `SearXNG`, Google Custom Search) is deferred to Phase 2.

use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

/// Maximum number of results to return.
const DEFAULT_MAX_RESULTS: u64 = 10;

/// Web search tool.
pub struct WebSearchTool;

impl Tool for WebSearchTool {
    fn name(&self) -> &'static str {
        "web_search"
    }

    fn description(&self) -> &'static str {
        "Search the web for up-to-date information. Returns search results with \
         titles, URLs, and snippets. Use this for questions about recent events, \
         documentation lookups, or anything beyond your training data."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query to use"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 10, max: 20)"
                },
                "allowed_domains": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Only include results from these domains"
                },
                "blocked_domains": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Exclude results from these domains"
                }
            },
            "required": ["query"]
        })
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let query = input["query"].as_str().unwrap_or("").to_owned();
        #[allow(clippy::cast_possible_truncation)]
        let max_results = input["max_results"]
            .as_u64()
            .unwrap_or(DEFAULT_MAX_RESULTS)
            .min(20) as usize;
        let allowed_domains = parse_string_array(&input["allowed_domains"]);
        let blocked_domains = parse_string_array(&input["blocked_domains"]);

        Box::pin(async move {
            if query.is_empty() {
                return Ok(ToolOutput::error("query is required and must be non-empty"));
            }

            // TODO: Replace with real search API call in Phase 2.
            // The implementation should support configurable backends
            // (Brave Search, SearXNG, Google Custom Search) via settings.
            let results = stub_search(&query, max_results, &allowed_domains, &blocked_domains);
            let json = serde_json::to_string_pretty(&results).unwrap_or_else(|_| "[]".to_string());

            Ok(ToolOutput::success(format!(
                "Search results for \"{query}\":\n\n{json}\n\n\
                 Note: Web search is not yet connected to a real search API. \
                 These are placeholder results."
            )))
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

/// Parse a JSON array of strings into a `Vec<String>`.
fn parse_string_array(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Generate stub search results for development/testing.
fn stub_search(
    query: &str,
    max_results: usize,
    _allowed_domains: &[String],
    _blocked_domains: &[String],
) -> Value {
    let stubs: Vec<Value> = (1..=max_results)
        .map(|i| {
            serde_json::json!({
                "title": format!("Result {i} for \"{query}\""),
                "url": format!("https://example.com/search?q={}&page={i}", urlencoded(query)),
                "snippet": format!(
                    "This is a placeholder snippet for result {i} matching the query \"{query}\". \
                     Real results will be available when a search API is configured."
                )
            })
        })
        .collect();
    Value::Array(stubs)
}

/// Minimal URL encoding for query strings.
fn urlencoded(s: &str) -> String {
    s.replace(' ', "+")
        .replace('&', "%26")
        .replace('=', "%3D")
        .replace('?', "%3F")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::tool::ToolContext;

    fn test_ctx() -> ToolContext {
        ToolContext {
            working_dir: std::path::PathBuf::from("/tmp"),
            permission_mode: crab_core::permission::PermissionMode::Default,
            session_id: "test".into(),
            cancellation_token: tokio_util::sync::CancellationToken::new(),
            permission_policy: crab_core::permission::PermissionPolicy::default(),
        }
    }

    #[test]
    fn tool_metadata() {
        let tool = WebSearchTool;
        assert_eq!(tool.name(), "web_search");
        assert!(tool.is_read_only());
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn input_schema_has_required_query() {
        let schema = WebSearchTool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("query")));
    }

    #[test]
    fn input_schema_has_optional_fields() {
        let schema = WebSearchTool.input_schema();
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("max_results"));
        assert!(props.contains_key("allowed_domains"));
        assert!(props.contains_key("blocked_domains"));
    }

    #[tokio::test]
    async fn execute_empty_query_returns_error() {
        let tool = WebSearchTool;
        let ctx = test_ctx();
        let result = tool
            .execute(serde_json::json!({"query": ""}), &ctx)
            .await
            .unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn execute_valid_query_returns_results() {
        let tool = WebSearchTool;
        let ctx = test_ctx();
        let result = tool
            .execute(serde_json::json!({"query": "rust programming"}), &ctx)
            .await
            .unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains("rust programming"));
        assert!(text.contains("Result 1"));
        assert!(text.contains("placeholder"));
    }

    #[tokio::test]
    async fn execute_respects_max_results() {
        let tool = WebSearchTool;
        let ctx = test_ctx();
        let result = tool
            .execute(serde_json::json!({"query": "test", "max_results": 3}), &ctx)
            .await
            .unwrap();
        let text = result.text();
        assert!(text.contains("Result 3"));
        assert!(!text.contains("Result 4"));
    }

    #[tokio::test]
    async fn execute_caps_max_results_at_20() {
        let tool = WebSearchTool;
        let ctx = test_ctx();
        let result = tool
            .execute(
                serde_json::json!({"query": "test", "max_results": 100}),
                &ctx,
            )
            .await
            .unwrap();
        let text = result.text();
        assert!(text.contains("Result 20"));
        assert!(!text.contains("Result 21"));
    }

    #[test]
    fn parse_string_array_valid() {
        let val = serde_json::json!(["a.com", "b.com"]);
        let result = parse_string_array(&val);
        assert_eq!(result, vec!["a.com", "b.com"]);
    }

    #[test]
    fn parse_string_array_null() {
        let result = parse_string_array(&Value::Null);
        assert!(result.is_empty());
    }

    #[test]
    fn parse_string_array_mixed() {
        let val = serde_json::json!(["valid.com", 42, "also.com"]);
        let result = parse_string_array(&val);
        assert_eq!(result, vec!["valid.com", "also.com"]);
    }

    #[test]
    fn urlencoded_basic() {
        assert_eq!(urlencoded("hello world"), "hello+world");
        assert_eq!(urlencoded("a&b=c"), "a%26b%3Dc");
    }

    #[test]
    fn stub_search_returns_correct_count() {
        let results = stub_search("test", 5, &[], &[]);
        assert_eq!(results.as_array().unwrap().len(), 5);
    }

    #[test]
    fn stub_search_results_have_fields() {
        let results = stub_search("rust", 1, &[], &[]);
        let first = &results[0];
        assert!(first["title"].as_str().unwrap().contains("rust"));
        assert!(first["url"].as_str().unwrap().starts_with("https://"));
        assert!(!first["snippet"].as_str().unwrap().is_empty());
    }
}
