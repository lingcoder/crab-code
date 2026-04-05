//! Web page fetching tool — fetches a URL and extracts text content.
//!
//! Currently returns a stub response. Real HTTP fetching with HTML-to-text
//! conversion is deferred to Phase 2 (requires adding `reqwest` dependency).

use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

/// Default timeout in seconds for HTTP requests.
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Maximum response body size in bytes (5 MB).
const MAX_BODY_SIZE: u64 = 5 * 1024 * 1024;

/// Web page fetching tool.
pub struct WebFetchTool;

impl Tool for WebFetchTool {
    fn name(&self) -> &'static str {
        "web_fetch"
    }

    fn description(&self) -> &'static str {
        "Fetch content from a URL, strip HTML to plain text, and return the \
         extracted content. Use a prompt to describe what information to extract \
         from the page. Includes timeout and size limits for safety."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "format": "uri",
                    "description": "The URL to fetch content from (must be a valid HTTP/HTTPS URL)"
                },
                "prompt": {
                    "type": "string",
                    "description": "Describe what information to extract from the page"
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Request timeout in seconds (default: 30, max: 120)"
                },
                "max_size_bytes": {
                    "type": "integer",
                    "description": "Maximum response body size in bytes (default: 5242880 = 5MB)"
                }
            },
            "required": ["url", "prompt"]
        })
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let url = input["url"].as_str().unwrap_or("").to_owned();
        let prompt = input["prompt"].as_str().unwrap_or("").to_owned();
        let timeout_secs = input["timeout_secs"]
            .as_u64()
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .min(120);
        let max_size = input["max_size_bytes"]
            .as_u64()
            .unwrap_or(MAX_BODY_SIZE)
            .min(MAX_BODY_SIZE);

        Box::pin(async move {
            // Validate inputs
            if url.is_empty() {
                return Ok(ToolOutput::error("url is required and must be non-empty"));
            }
            if prompt.is_empty() {
                return Ok(ToolOutput::error(
                    "prompt is required — describe what to extract from the page",
                ));
            }
            if let Err(reason) = validate_url(&url) {
                return Ok(ToolOutput::error(reason));
            }

            // TODO: Replace with real HTTP fetch + HTML-to-text in Phase 2.
            // Implementation plan:
            //   1. reqwest::Client with timeout_secs, redirect policy (max 5)
            //   2. Check Content-Length against max_size before downloading
            //   3. Download body with streaming size limit
            //   4. Detect Content-Type: HTML → strip tags, JSON → pretty-print,
            //      plain text → return as-is
            //   5. Truncate to ~100k chars to avoid context overflow
            //   6. Apply prompt as extraction instruction (optional LLM pass)

            let stub_content = stub_fetch(&url, &prompt, timeout_secs, max_size);
            Ok(ToolOutput::success(stub_content))
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

/// Validate that the URL is a reasonable HTTP/HTTPS URL.
fn validate_url(url: &str) -> std::result::Result<(), String> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(format!(
            "URL must start with http:// or https://, got: {url}"
        ));
    }
    // Basic check for a host component
    let after_scheme = if let Some(rest) = url.strip_prefix("https://") {
        rest
    } else if let Some(rest) = url.strip_prefix("http://") {
        rest
    } else {
        return Err("invalid URL scheme".to_string());
    };
    if after_scheme.is_empty() || after_scheme.starts_with('/') {
        return Err("URL must include a hostname".to_string());
    }
    Ok(())
}

/// Generate a stub response for development/testing.
fn stub_fetch(url: &str, prompt: &str, timeout_secs: u64, max_size: u64) -> String {
    format!(
        "# Web Fetch Result\n\n\
         **URL:** {url}\n\
         **Prompt:** {prompt}\n\
         **Timeout:** {timeout_secs}s\n\
         **Max size:** {max_size} bytes\n\n\
         ---\n\n\
         This is a placeholder response. Web fetching is not yet connected to a \
         real HTTP client. In Phase 2, this tool will:\n\
         - Fetch the page via reqwest with configurable timeout\n\
         - Convert HTML to plain text (strip tags, extract main content)\n\
         - Apply size limits to prevent context overflow\n\
         - Optionally use the prompt to guide content extraction\n\n\
         To test with real content, configure an HTTP client in the tools crate."
    )
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
        let tool = WebFetchTool;
        assert_eq!(tool.name(), "web_fetch");
        assert!(tool.is_read_only());
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn input_schema_has_required_fields() {
        let schema = WebFetchTool.input_schema();
        let required = schema["required"].as_array().unwrap();
        let required_strs: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert!(required_strs.contains(&"url"));
        assert!(required_strs.contains(&"prompt"));
    }

    #[test]
    fn input_schema_has_optional_fields() {
        let schema = WebFetchTool.input_schema();
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("timeout_secs"));
        assert!(props.contains_key("max_size_bytes"));
    }

    #[tokio::test]
    async fn execute_empty_url_returns_error() {
        let tool = WebFetchTool;
        let ctx = test_ctx();
        let result = tool
            .execute(serde_json::json!({"url": "", "prompt": "extract"}), &ctx)
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.text().contains("url is required"));
    }

    #[tokio::test]
    async fn execute_empty_prompt_returns_error() {
        let tool = WebFetchTool;
        let ctx = test_ctx();
        let result = tool
            .execute(
                serde_json::json!({"url": "https://example.com", "prompt": ""}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.text().contains("prompt is required"));
    }

    #[tokio::test]
    async fn execute_invalid_scheme_returns_error() {
        let tool = WebFetchTool;
        let ctx = test_ctx();
        let result = tool
            .execute(
                serde_json::json!({"url": "ftp://example.com", "prompt": "extract"}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.text().contains("http://"));
    }

    #[tokio::test]
    async fn execute_no_host_returns_error() {
        let tool = WebFetchTool;
        let ctx = test_ctx();
        let result = tool
            .execute(
                serde_json::json!({"url": "https://", "prompt": "extract"}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.text().contains("hostname"));
    }

    #[tokio::test]
    async fn execute_valid_returns_stub() {
        let tool = WebFetchTool;
        let ctx = test_ctx();
        let result = tool
            .execute(
                serde_json::json!({
                    "url": "https://example.com/page",
                    "prompt": "extract the main content"
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains("https://example.com/page"));
        assert!(text.contains("extract the main content"));
        assert!(text.contains("placeholder"));
    }

    #[tokio::test]
    async fn execute_with_custom_timeout() {
        let tool = WebFetchTool;
        let ctx = test_ctx();
        let result = tool
            .execute(
                serde_json::json!({
                    "url": "https://example.com",
                    "prompt": "get info",
                    "timeout_secs": 60
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.text().contains("60s"));
    }

    #[tokio::test]
    async fn execute_caps_timeout_at_120() {
        let tool = WebFetchTool;
        let ctx = test_ctx();
        let result = tool
            .execute(
                serde_json::json!({
                    "url": "https://example.com",
                    "prompt": "get info",
                    "timeout_secs": 999
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(result.text().contains("120s"));
    }

    #[test]
    fn validate_url_valid_https() {
        assert!(validate_url("https://example.com").is_ok());
        assert!(validate_url("https://example.com/path?q=1").is_ok());
    }

    #[test]
    fn validate_url_valid_http() {
        assert!(validate_url("http://localhost:8080").is_ok());
    }

    #[test]
    fn validate_url_rejects_ftp() {
        assert!(validate_url("ftp://example.com").is_err());
    }

    #[test]
    fn validate_url_rejects_no_host() {
        assert!(validate_url("https://").is_err());
        assert!(validate_url("https:///path").is_err());
    }

    #[test]
    fn validate_url_rejects_empty() {
        assert!(validate_url("").is_err());
    }

    #[test]
    fn stub_fetch_includes_url_and_prompt() {
        let result = stub_fetch("https://test.com", "get data", 30, 5_000_000);
        assert!(result.contains("https://test.com"));
        assert!(result.contains("get data"));
        assert!(result.contains("30s"));
    }
}
