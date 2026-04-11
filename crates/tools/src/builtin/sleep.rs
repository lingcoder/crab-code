//! `SleepTool` — async wait for a specified duration.
//!
//! Pauses execution for the requested number of milliseconds. Respects
//! the cancellation token so the sleep can be interrupted.

use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

/// Tool name constant for `SleepTool`.
pub const SLEEP_TOOL_NAME: &str = "Sleep";

/// Maximum allowed sleep duration (5 minutes).
const MAX_DURATION_MS: u64 = 300_000;

/// Async wait tool.
///
/// Input:
/// - `duration_ms`: Duration to sleep in milliseconds
pub struct SleepTool;

impl Tool for SleepTool {
    fn name(&self) -> &'static str {
        SLEEP_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Wait for a specified duration in milliseconds. The maximum duration is \
         300000 ms (5 minutes). The wait can be cancelled via the session cancellation token."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "duration_ms": {
                    "type": "integer",
                    "description": "Duration to sleep in milliseconds (max 300000)"
                }
            },
            "required": ["duration_ms"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        input["duration_ms"]
            .as_u64()
            .map(|d| format!("Sleep ({d}ms)"))
    }

    fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let duration_ms = input["duration_ms"].as_u64().unwrap_or(0);
        let cancel = ctx.cancellation_token.clone();

        Box::pin(async move {
            if duration_ms == 0 {
                return Ok(ToolOutput::error("duration_ms must be a positive integer"));
            }

            let clamped = duration_ms.min(MAX_DURATION_MS);
            let duration = std::time::Duration::from_millis(clamped);

            tokio::select! {
                () = tokio::time::sleep(duration) => {
                    Ok(ToolOutput::success(format!("Slept for {clamped} ms")))
                }
                () = cancel.cancelled() => {
                    Ok(ToolOutput::error("Sleep cancelled"))
                }
            }
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
            ext: crab_core::tool::ToolContextExt::default(),
        }
    }

    #[test]
    fn tool_metadata() {
        let tool = SleepTool;
        assert_eq!(tool.name(), "Sleep");
        assert!(tool.is_read_only());
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn schema_has_required_fields() {
        let schema = SleepTool.input_schema();
        assert_eq!(schema["required"], json!(["duration_ms"]));
        assert!(schema["properties"]["duration_ms"].is_object());
    }

    #[tokio::test]
    async fn zero_duration_returns_error() {
        let tool = SleepTool;
        let result = tool
            .execute(json!({"duration_ms": 0}), &test_ctx())
            .await
            .unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn short_sleep_succeeds() {
        let tool = SleepTool;
        let result = tool
            .execute(json!({"duration_ms": 10}), &test_ctx())
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.text().contains("10 ms"));
    }
}
