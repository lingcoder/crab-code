use std::fmt::Write as _;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use serde_json::Value;

/// File reading tool.
pub struct ReadTool;

/// Extensions treated as binary/non-text — return type info instead of content.
const BINARY_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "webp", "ico", "svg", "pdf", "zip", "tar", "gz", "bz2",
    "xz", "7z", "rar", "exe", "dll", "so", "dylib", "mp3", "mp4", "wav", "ogg", "avi", "mov",
    "mkv",
];

/// Notebook extensions — return type info (full notebook reading handled separately).
const NOTEBOOK_EXTENSIONS: &[&str] = &["ipynb"];

fn extension_of(path: &Path) -> &str {
    path.extension().and_then(|e| e.to_str()).unwrap_or("")
}

impl Tool for ReadTool {
    fn name(&self) -> &'static str {
        "read"
    }

    fn description(&self) -> &'static str {
        "Read a file from the local filesystem. Supports text files with optional \
         line range (offset/limit). Returns content in cat -n format (line numbers). \
         Binary files (images, PDF, archives) return file type information only."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the file to read"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start reading from (1-based, default: 1)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Number of lines to read (default: 2000)"
                }
            },
            "required": ["file_path"]
        })
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let file_path = input["file_path"].as_str().unwrap_or("").to_owned();
        // offset is 1-based line number; default 1
        #[allow(clippy::cast_possible_truncation)]
        let offset = input["offset"].as_u64().map_or(1, |v| v as usize);
        #[allow(clippy::cast_possible_truncation)]
        let limit = input["limit"].as_u64().map_or(2000, |v| v as usize);

        Box::pin(async move {
            if file_path.is_empty() {
                return Ok(ToolOutput::error("file_path is required"));
            }

            let path = std::path::PathBuf::from(&file_path);
            let ext = extension_of(&path).to_ascii_lowercase();

            if NOTEBOOK_EXTENSIONS.contains(&ext.as_str()) {
                return Ok(ToolOutput::success(format!(
                    "Jupyter notebook file: {file_path}\n\
                     Use the notebook_edit tool to read and modify notebook cells."
                )));
            }

            if BINARY_EXTENSIONS.contains(&ext.as_str()) {
                return Ok(ToolOutput::success(format!(
                    "Binary file ({ext}): {file_path}\n\
                     This file type cannot be displayed as text."
                )));
            }

            let content = match tokio::fs::read_to_string(&path).await {
                Ok(c) => c,
                Err(e) => {
                    return Ok(ToolOutput::error(format!(
                        "Failed to read {file_path}: {e}"
                    )));
                }
            };

            // offset is 1-based; clamp to at least 1
            let start = offset.saturating_sub(1); // convert to 0-based index
            let lines: Vec<&str> = content.lines().collect();
            let total = lines.len();

            let end = (start + limit).min(total);
            let selected = if start >= total {
                &[][..]
            } else {
                &lines[start..end]
            };

            // Format as cat -n: "     N\tline"
            let mut output = String::new();
            for (i, line) in selected.iter().enumerate() {
                let line_num = start + i + 1; // 1-based
                let _ = writeln!(output, "{line_num:6}\t{line}");
            }

            Ok(ToolOutput::success(output))
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::permission::{PermissionMode, PermissionPolicy};
    use tokio_util::sync::CancellationToken;

    fn make_ctx() -> ToolContext {
        ToolContext {
            working_dir: std::env::temp_dir(),
            permission_mode: PermissionMode::Default,
            session_id: "test".into(),
            cancellation_token: CancellationToken::new(),
            permission_policy: PermissionPolicy::default(),
        }
    }

    async fn write_temp(name: &str, content: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(name);
        tokio::fs::write(&path, content).await.unwrap();
        path
    }

    #[tokio::test]
    async fn read_simple_file() {
        let path = write_temp("read_test_simple.txt", "line one\nline two\nline three\n").await;
        let tool = ReadTool;
        let input = serde_json::json!({ "file_path": path.to_str().unwrap() });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(!out.is_error);
        let text = out.text();
        assert!(text.contains("     1\tline one"));
        assert!(text.contains("     2\tline two"));
        assert!(text.contains("     3\tline three"));
    }

    #[tokio::test]
    async fn read_with_offset_and_limit() {
        let content = "a\nb\nc\nd\ne\n";
        let path = write_temp("read_test_offset.txt", content).await;
        let tool = ReadTool;
        let input = serde_json::json!({
            "file_path": path.to_str().unwrap(),
            "offset": 2,
            "limit": 2
        });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(!out.is_error);
        let text = out.text();
        // offset=2, limit=2 → lines 2 and 3
        assert!(text.contains("     2\tb"));
        assert!(text.contains("     3\tc"));
        assert!(!text.contains("     1\t"));
        assert!(!text.contains("     4\t"));
    }

    #[tokio::test]
    async fn read_nonexistent_file_is_error() {
        let tool = ReadTool;
        let input = serde_json::json!({ "file_path": "/nonexistent/path/file.txt" });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn read_empty_path_is_error() {
        let tool = ReadTool;
        let input = serde_json::json!({ "file_path": "" });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn read_binary_extension_returns_info() {
        let tool = ReadTool;
        let input = serde_json::json!({ "file_path": "/some/image.png" });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(!out.is_error);
        assert!(out.text().contains("Binary file"));
    }

    #[tokio::test]
    async fn read_notebook_extension_returns_info() {
        let tool = ReadTool;
        let input = serde_json::json!({ "file_path": "/some/notebook.ipynb" });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(!out.is_error);
        assert!(out.text().contains("Jupyter notebook"));
    }

    #[test]
    fn read_is_read_only() {
        assert!(ReadTool.is_read_only());
    }

    #[test]
    fn read_schema_requires_file_path() {
        let schema = ReadTool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "file_path"));
    }
}
