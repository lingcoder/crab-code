//! `SymlinkCheckTool` — check whether a path is safe from symlink escapes.
//!
//! Delegates to [`crab_fs::symlink::check_symlink_safety`] to verify that
//! a target path resolves within a given boundary directory.

use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use serde_json::Value;
use std::fmt::Write;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;

/// Tool that checks whether a path is safe (no symlink escape).
pub struct SymlinkCheckTool;

impl Tool for SymlinkCheckTool {
    fn name(&self) -> &'static str {
        "symlink_check"
    }

    fn description(&self) -> &'static str {
        "Check whether a file path is safe from symlink escapes. Resolves all \
         symlinks and verifies the resolved path stays within the given boundary \
         directory (defaults to the working directory). Also reports whether the \
         path is a symlink and its resolved target."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to check"
                },
                "boundary": {
                    "type": "string",
                    "description": "Boundary directory the path must resolve within (defaults to working directory)"
                }
            },
            "required": ["path"]
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
        let target = input["path"].as_str().unwrap_or("").to_owned();
        let boundary = input["boundary"].as_str().map_or_else(
            || ctx.working_dir.to_string_lossy().into_owned(),
            String::from,
        );

        Box::pin(async move {
            if target.is_empty() {
                return Ok(ToolOutput::error("path is required"));
            }

            let target_path = Path::new(&target);
            let boundary_path = Path::new(&boundary);

            let is_symlink = crab_fs::symlink::is_symlink(target_path);

            let mut out = String::new();
            let _ = write!(out, "path: {target}");
            let _ = write!(out, "\nis_symlink: {is_symlink}");

            match crab_fs::symlink::check_symlink_safety(target_path, boundary_path) {
                Ok(resolved) => {
                    let _ = write!(out, "\nresolved: {}", resolved.display());
                    let _ = write!(out, "\nsafe: true");
                    let _ = write!(out, "\nboundary: {boundary}");
                    Ok(ToolOutput::success(out))
                }
                Err(e) => {
                    let _ = write!(out, "\nsafe: false");
                    let _ = write!(out, "\nreason: {e}");
                    Ok(ToolOutput::success(out))
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
        }
    }

    #[tokio::test]
    async fn empty_path_returns_error() {
        let tool = SymlinkCheckTool;
        let result = tool
            .execute(json!({"path": ""}), &test_ctx())
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.text().contains("required"));
    }

    #[tokio::test]
    async fn safe_path_within_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("safe.txt");
        std::fs::write(&file, "ok").unwrap();

        let tool = SymlinkCheckTool;
        let result = tool
            .execute(
                json!({
                    "path": file.to_str().unwrap(),
                    "boundary": dir.path().to_str().unwrap()
                }),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains("safe: true"));
        assert!(text.contains("is_symlink: false"));
    }

    #[tokio::test]
    async fn nonexistent_path_reports_unsafe() {
        let dir = tempfile::tempdir().unwrap();

        let tool = SymlinkCheckTool;
        let result = tool
            .execute(
                json!({
                    "path": dir.path().join("nope.txt").to_str().unwrap(),
                    "boundary": dir.path().to_str().unwrap()
                }),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.text().contains("safe: false"));
    }

    #[tokio::test]
    async fn uses_working_dir_as_default_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "data").unwrap();

        let ctx = ToolContext {
            working_dir: dir.path().to_path_buf(),
            permission_mode: crab_core::permission::PermissionMode::Dangerously,
            session_id: "test".into(),
            cancellation_token: CancellationToken::new(),
            permission_policy: crab_core::permission::PermissionPolicy::default(),
        };

        let tool = SymlinkCheckTool;
        let result = tool
            .execute(json!({"path": file.to_str().unwrap()}), &ctx)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.text().contains("safe: true"));
    }

    #[test]
    fn tool_metadata() {
        let tool = SymlinkCheckTool;
        assert_eq!(tool.name(), "symlink_check");
        assert!(tool.is_read_only());
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn schema_has_required_fields() {
        let tool = SymlinkCheckTool;
        let schema = tool.input_schema();
        assert_eq!(schema["required"], json!(["path"]));
        assert!(schema["properties"]["path"].is_object());
        assert!(schema["properties"]["boundary"].is_object());
    }
}
