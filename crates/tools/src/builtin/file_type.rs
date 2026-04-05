//! `FileTypeTool` — detect file type by extension and magic bytes.
//!
//! Delegates to [`crab_fs::filetype`] for detection. Returns the file
//! category (text/binary/image/document), MIME type, and extension.

use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use serde_json::Value;
use std::fmt::Write;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;

/// Tool that detects a file's type using extension and magic bytes.
pub struct FileTypeTool;

impl Tool for FileTypeTool {
    fn name(&self) -> &'static str {
        "file_type"
    }

    fn description(&self) -> &'static str {
        "Detect the type of a file using its extension and magic bytes (file \
         header). Returns the category (text, binary, image, document), MIME \
         type, and detected extension. Use this to determine whether a file is \
         text or binary before reading it."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the file to detect"
                },
                "use_magic": {
                    "type": "boolean",
                    "description": "If true (default), read file header for magic byte detection. If false, only use extension."
                }
            },
            "required": ["file_path"]
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
        let file_path = input["file_path"].as_str().unwrap_or("").to_owned();
        let use_magic = input["use_magic"].as_bool().unwrap_or(true);

        Box::pin(async move {
            if file_path.is_empty() {
                return Ok(ToolOutput::error("file_path is required"));
            }

            let path = Path::new(&file_path);

            let ft = if use_magic {
                crab_fs::filetype::detect_by_magic(path)
            } else {
                crab_fs::filetype::detect_by_extension(path)
            };

            let mut out = String::new();
            let _ = write!(out, "file: {file_path}");
            let _ = write!(out, "\ncategory: {}", ft.category);
            let _ = write!(out, "\nmime: {}", ft.mime);
            if let Some(ext) = &ft.extension {
                let _ = write!(out, "\nextension: {ext}");
            }

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
    async fn empty_path_returns_error() {
        let tool = FileTypeTool;
        let result = tool
            .execute(json!({"file_path": ""}), &test_ctx())
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.text().contains("required"));
    }

    #[tokio::test]
    async fn detect_rust_file_by_extension() {
        let tool = FileTypeTool;
        let result = tool
            .execute(
                json!({"file_path": "/tmp/main.rs", "use_magic": false}),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains("category: text"));
        assert!(text.contains("mime: text/x-rust"));
        assert!(text.contains("extension: rs"));
    }

    #[tokio::test]
    async fn detect_png_by_extension() {
        let tool = FileTypeTool;
        let result = tool
            .execute(
                json!({"file_path": "/tmp/logo.png", "use_magic": false}),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains("category: image"));
        assert!(text.contains("mime: image/png"));
    }

    #[tokio::test]
    async fn detect_text_file_by_magic() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "Hello, world!").unwrap();

        let tool = FileTypeTool;
        let result = tool
            .execute(json!({"file_path": file.to_str().unwrap()}), &test_ctx())
            .await
            .unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains("category: text"));
    }

    #[tokio::test]
    async fn detect_png_by_magic() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("image.bin");
        // Write PNG magic bytes
        std::fs::write(&file, [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]).unwrap();

        let tool = FileTypeTool;
        let result = tool
            .execute(json!({"file_path": file.to_str().unwrap()}), &test_ctx())
            .await
            .unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains("category: image"));
        assert!(text.contains("mime: image/png"));
    }

    #[tokio::test]
    async fn nonexistent_file_falls_back_to_extension() {
        let tool = FileTypeTool;
        let result = tool
            .execute(
                json!({"file_path": "/tmp/nonexistent_filetype_test_12345.json"}),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains("category: text"));
        assert!(text.contains("mime: application/json"));
    }

    #[tokio::test]
    async fn default_use_magic_is_true() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data.txt");
        std::fs::write(&file, "plain text content").unwrap();

        let tool = FileTypeTool;
        // No use_magic param — should default to true
        let result = tool
            .execute(json!({"file_path": file.to_str().unwrap()}), &test_ctx())
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.text().contains("category: text"));
    }

    #[test]
    fn tool_metadata() {
        let tool = FileTypeTool;
        assert_eq!(tool.name(), "file_type");
        assert!(tool.is_read_only());
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn schema_has_required_fields() {
        let tool = FileTypeTool;
        let schema = tool.input_schema();
        assert_eq!(schema["required"], json!(["file_path"]));
        assert!(schema["properties"]["file_path"].is_object());
        assert!(schema["properties"]["use_magic"].is_object());
    }
}
