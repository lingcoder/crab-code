//! `DiffTool` — compare two files and output a unified diff.
//!
//! Reads both files and delegates to [`crab_fs::diff::unified_diff`] to
//! produce a standard unified diff string.

use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

/// Tool that compares two files and returns a unified diff.
pub struct DiffTool;

impl Tool for DiffTool {
    fn name(&self) -> &'static str {
        "diff"
    }

    fn description(&self) -> &'static str {
        "Compare two files and output a unified diff. Useful for reviewing \
         changes between file versions, comparing generated output to expected \
         output, or inspecting what changed in a file."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_a": {
                    "type": "string",
                    "description": "Absolute path to the first (old) file"
                },
                "file_b": {
                    "type": "string",
                    "description": "Absolute path to the second (new) file"
                },
                "context_lines": {
                    "type": "integer",
                    "description": "Number of context lines around each change (default: 3)"
                }
            },
            "required": ["file_a", "file_b"]
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
        let file_a = input["file_a"].as_str().unwrap_or("").to_owned();
        let file_b = input["file_b"].as_str().unwrap_or("").to_owned();
        let context_lines =
            usize::try_from(input["context_lines"].as_u64().unwrap_or(3)).unwrap_or(3);

        Box::pin(async move {
            if file_a.is_empty() || file_b.is_empty() {
                return Ok(ToolOutput::error("both file_a and file_b are required"));
            }

            let content_a = tokio::fs::read_to_string(&file_a)
                .await
                .map_err(|e| crab_common::Error::Other(format!("failed to read {file_a}: {e}")))?;

            let content_b = tokio::fs::read_to_string(&file_b)
                .await
                .map_err(|e| crab_common::Error::Other(format!("failed to read {file_b}: {e}")))?;

            let diff = crab_fs::diff::unified_diff_with_context(
                &content_a,
                &content_b,
                &file_a,
                &file_b,
                context_lines,
            );

            if diff.is_empty() {
                Ok(ToolOutput::success("files are identical"))
            } else {
                Ok(ToolOutput::success(diff))
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
        }
    }

    #[tokio::test]
    async fn empty_paths_return_error() {
        let tool = DiffTool;
        let result = tool
            .execute(json!({"file_a": "", "file_b": ""}), &test_ctx())
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.text().contains("required"));
    }

    #[tokio::test]
    async fn missing_file_a_returns_error() {
        let tool = DiffTool;
        let result = tool
            .execute(json!({"file_a": "", "file_b": "/tmp/b.txt"}), &test_ctx())
            .await
            .unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn identical_files_reports_identical() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, "hello\nworld\n").unwrap();
        std::fs::write(&b, "hello\nworld\n").unwrap();

        let tool = DiffTool;
        let result = tool
            .execute(
                json!({"file_a": a.to_str().unwrap(), "file_b": b.to_str().unwrap()}),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.text().contains("identical"));
    }

    #[tokio::test]
    async fn different_files_produce_diff() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, "line1\nline2\nline3\n").unwrap();
        std::fs::write(&b, "line1\nchanged\nline3\n").unwrap();

        let tool = DiffTool;
        let result = tool
            .execute(
                json!({"file_a": a.to_str().unwrap(), "file_b": b.to_str().unwrap()}),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains("-line2"));
        assert!(text.contains("+changed"));
    }

    #[tokio::test]
    async fn nonexistent_file_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("exists.txt");
        std::fs::write(&a, "data").unwrap();

        let tool = DiffTool;
        let result = tool
            .execute(
                json!({"file_a": a.to_str().unwrap(), "file_b": "/tmp/nonexistent_diff_test_12345.txt"}),
                &test_ctx(),
            )
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn tool_metadata() {
        let tool = DiffTool;
        assert_eq!(tool.name(), "diff");
        assert!(tool.is_read_only());
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn schema_has_required_fields() {
        let tool = DiffTool;
        let schema = tool.input_schema();
        assert_eq!(schema["required"], json!(["file_a", "file_b"]));
        assert!(schema["properties"]["file_a"].is_object());
        assert!(schema["properties"]["file_b"].is_object());
        assert!(schema["properties"]["context_lines"].is_object());
    }
}
