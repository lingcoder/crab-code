//! `RemoteTriggerTool` — trigger tasks via HTTP webhook (stub).
//!
//! Provides a stub implementation for triggering remote tasks via
//! HTTP webhooks. In production this would make real HTTP calls;
//! currently returns a simulated response.

use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use serde_json::Value;
use std::fmt::Write;
use std::future::Future;
use std::pin::Pin;

/// Tool that triggers a remote task via HTTP webhook (stub).
pub struct RemoteTriggerTool;

impl Tool for RemoteTriggerTool {
    fn name(&self) -> &'static str {
        "remote_trigger"
    }

    fn description(&self) -> &'static str {
        "Trigger a remote task by sending an HTTP POST to a webhook URL. \
         Sends a JSON payload with the specified event name and optional data. \
         Currently a stub implementation that simulates the webhook call."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The webhook URL to POST to (must be https)"
                },
                "event": {
                    "type": "string",
                    "description": "The event name to include in the payload"
                },
                "data": {
                    "type": "object",
                    "description": "Optional additional data to include in the webhook payload"
                },
                "headers": {
                    "type": "object",
                    "description": "Optional HTTP headers to include in the request (e.g. authorization tokens)"
                }
            },
            "required": ["url", "event"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let url = input["url"].as_str().unwrap_or("").to_owned();
        let event = input["event"].as_str().unwrap_or("").to_owned();
        let data = input.get("data").cloned();
        let headers = input.get("headers").cloned();

        Box::pin(async move {
            if url.is_empty() {
                return Ok(ToolOutput::error("url is required"));
            }
            if event.is_empty() {
                return Ok(ToolOutput::error("event is required"));
            }

            // Validate URL scheme
            if !url.starts_with("https://") && !url.starts_with("http://localhost") {
                return Ok(ToolOutput::error(
                    "url must use https (except http://localhost for development)",
                ));
            }

            // Build the payload that would be sent
            let mut payload = serde_json::json!({
                "event": event,
                "timestamp": "2026-04-05T10:58:00Z",
            });
            if let Some(d) = &data {
                payload["data"] = d.clone();
            }

            // Count custom headers
            let header_count = headers
                .as_ref()
                .and_then(|h| h.as_object())
                .map_or(0, serde_json::Map::len);

            // Stub: simulate successful webhook delivery
            let mut out = String::new();
            let _ = write!(out, "Webhook triggered (stub)");
            let _ = write!(out, "\nurl: {url}");
            let _ = write!(out, "\nevent: {event}");
            let _ = write!(out, "\nstatus: 200 OK (simulated)");
            if header_count > 0 {
                let _ = write!(out, "\ncustom_headers: {header_count}");
            }
            if data.is_some() {
                let _ = write!(out, "\npayload_has_data: true");
            }
            let _ = write!(out, "\npayload: {payload}");

            Ok(ToolOutput::success(out))
        })
    }
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
    async fn empty_url_returns_error() {
        let tool = RemoteTriggerTool;
        let result = tool
            .execute(json!({"url": "", "event": "deploy"}), &test_ctx())
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.text().contains("url is required"));
    }

    #[tokio::test]
    async fn empty_event_returns_error() {
        let tool = RemoteTriggerTool;
        let result = tool
            .execute(
                json!({"url": "https://example.com/hook", "event": ""}),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.text().contains("event is required"));
    }

    #[tokio::test]
    async fn http_non_localhost_rejected() {
        let tool = RemoteTriggerTool;
        let result = tool
            .execute(
                json!({"url": "http://example.com/hook", "event": "test"}),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.text().contains("https"));
    }

    #[tokio::test]
    async fn http_localhost_allowed() {
        let tool = RemoteTriggerTool;
        let result = tool
            .execute(
                json!({"url": "http://localhost:8080/hook", "event": "test"}),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.text().contains("200 OK"));
    }

    #[tokio::test]
    async fn https_url_succeeds() {
        let tool = RemoteTriggerTool;
        let result = tool
            .execute(
                json!({"url": "https://example.com/webhook", "event": "deploy"}),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains("Webhook triggered (stub)"));
        assert!(text.contains("https://example.com/webhook"));
        assert!(text.contains("event: deploy"));
        assert!(text.contains("200 OK (simulated)"));
    }

    #[tokio::test]
    async fn with_custom_data() {
        let tool = RemoteTriggerTool;
        let result = tool
            .execute(
                json!({
                    "url": "https://example.com/hook",
                    "event": "build",
                    "data": {"branch": "main", "commit": "abc123"}
                }),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains("payload_has_data: true"));
        assert!(text.contains("abc123"));
    }

    #[tokio::test]
    async fn with_custom_headers() {
        let tool = RemoteTriggerTool;
        let result = tool
            .execute(
                json!({
                    "url": "https://example.com/hook",
                    "event": "notify",
                    "headers": {"Authorization": "Bearer token123"}
                }),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.text().contains("custom_headers: 1"));
    }

    #[test]
    fn tool_metadata() {
        let tool = RemoteTriggerTool;
        assert_eq!(tool.name(), "remote_trigger");
        assert!(!tool.is_read_only());
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn schema_has_required_fields() {
        let tool = RemoteTriggerTool;
        let schema = tool.input_schema();
        assert_eq!(schema["required"], json!(["url", "event"]));
        assert!(schema["properties"]["url"].is_object());
        assert!(schema["properties"]["event"].is_object());
        assert!(schema["properties"]["data"].is_object());
        assert!(schema["properties"]["headers"].is_object());
    }
}
