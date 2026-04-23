use crab_core::Result;
use crab_core::tool::{Tool, ToolContext, ToolDisplayResult, ToolDisplayStyle, ToolOutput};
use serde_json::Value;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;

/// Sensitive file patterns that should be flagged before writing.
const SENSITIVE_PATTERNS: &[&str] = &[
    ".env",
    ".env.",
    "credentials",
    "secret",
    ".pem",
    ".key",
    ".p12",
    ".pfx",
    "id_rsa",
    "id_ed25519",
    ".npmrc",
    ".pypirc",
    "token",
];

/// File creation/overwrite tool.
pub const WRITE_TOOL_NAME: &str = "Write";

pub struct WriteTool;

impl Tool for WriteTool {
    fn name(&self) -> &'static str {
        WRITE_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Write content to a file, creating or overwriting it"
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the file to write"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write"
                }
            },
            "required": ["file_path", "content"]
        })
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        Box::pin(async move {
            let file_path = input
                .get("file_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    crab_core::Error::Other("missing required parameter: file_path".into())
                })?;

            let content = input
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    crab_core::Error::Other("missing required parameter: content".into())
                })?;

            let path = Path::new(file_path);

            // Validate absolute path
            if !path.is_absolute() {
                return Ok(ToolOutput::error(format!(
                    "file_path must be absolute, got: {file_path}"
                )));
            }

            // Check for sensitive file patterns
            if let Some(warning) = check_sensitive_file(file_path) {
                return Ok(ToolOutput::error(warning));
            }

            // Create parent directories if needed
            if let Some(parent) = path.parent()
                && !parent.exists()
            {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    crab_core::Error::Other(format!(
                        "failed to create directory {}: {e}",
                        parent.display()
                    ))
                })?;
            }

            // Write the file
            tokio::fs::write(path, content).await.map_err(|e| {
                crab_core::Error::Other(format!("failed to write {file_path}: {e}"))
            })?;

            let bytes = content.len();
            Ok(ToolOutput::success(format!(
                "Wrote {bytes} bytes to {file_path}"
            )))
        })
    }

    fn requires_confirmation(&self) -> bool {
        true
    }

    // ── CCB-aligned rendering hooks ──

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        // CCB: userFacingName="Write", message = file path
        let path = input["file_path"].as_str()?;
        let filename = path.rsplit(['/', '\\']).next().unwrap_or(path);
        Some(format!("Write ({filename})"))
    }

    fn format_result(&self, output: &ToolOutput) -> Option<ToolDisplayResult> {
        use crab_core::tool::{ToolDisplayLine, ToolDisplayResult, ToolDisplayStyle};
        let text = output.text();
        // CCB: "Wrote N lines to filename" (N and filename bold)
        let line_count = text.lines().count();
        // Show first 10 lines of content as preview (CCB: MAX_LINES_TO_RENDER=10)
        let preview_lines: Vec<&str> = text.lines().take(10).collect();
        let mut lines = vec![ToolDisplayLine::new(
            format!("Wrote {line_count} lines"),
            ToolDisplayStyle::Muted,
        )];
        for pl in &preview_lines {
            lines.push(ToolDisplayLine::new(*pl, ToolDisplayStyle::Normal));
        }
        let total = text.lines().count();
        if total > 10 {
            lines.push(ToolDisplayLine::new(
                format!("… +{} lines", total - 10),
                ToolDisplayStyle::Muted,
            ));
        }
        Some(ToolDisplayResult {
            lines,
            preview_lines: 1, // condensed: just the summary
        })
    }

    fn format_rejected_summary(&self, input: &Value) -> Option<String> {
        let path = input["file_path"].as_str()?;
        let filename = path.rsplit(['/', '\\']).next().unwrap_or(path);
        Some(format!("Write rejected ({filename})"))
    }

    fn format_rejected(&self, input: &Value) -> Option<ToolDisplayResult> {
        use crab_core::tool::ToolDisplayLine;
        let content = input["content"].as_str()?;
        let preview: Vec<&str> = content.lines().take(3).collect();
        let mut lines = Vec::new();
        for line in &preview {
            lines.push(ToolDisplayLine::new(*line, ToolDisplayStyle::Muted));
        }
        if content.lines().count() > 3 {
            lines.push(ToolDisplayLine::new(
                format!("... ({} more lines)", content.lines().count() - 3),
                ToolDisplayStyle::Muted,
            ));
        }
        Some(ToolDisplayResult {
            lines,
            preview_lines: 3,
        })
    }

    fn display_color(&self) -> ToolDisplayStyle {
        ToolDisplayStyle::DiffAdd
    }
}

/// Check if a file path matches any sensitive file patterns.
/// Returns a warning message if sensitive, None otherwise.
fn check_sensitive_file(path: &str) -> Option<String> {
    let lower = path.to_lowercase();
    let file_name = Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();

    for pattern in SENSITIVE_PATTERNS {
        if file_name.starts_with(pattern) || file_name.ends_with(pattern) || lower.contains(pattern)
        {
            return Some(format!(
                "Refusing to write potentially sensitive file matching pattern '{pattern}': {path}"
            ));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::permission::PermissionPolicy;
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    fn test_ctx() -> ToolContext {
        ToolContext {
            working_dir: std::env::temp_dir(),
            permission_mode: crab_core::permission::PermissionMode::Dangerously,
            session_id: "test".into(),
            cancellation_token: CancellationToken::new(),
            permission_policy: PermissionPolicy::default(),
            ext: crab_core::tool::ToolContextExt::default(),
        }
    }

    #[test]
    fn tool_metadata() {
        let tool = WriteTool;
        assert_eq!(tool.name(), "Write");
        assert!(!tool.is_read_only());
        assert!(tool.requires_confirmation());
    }

    #[test]
    fn schema_has_required_fields() {
        let schema = WriteTool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("file_path")));
        assert!(required.contains(&json!("content")));
    }

    #[tokio::test]
    async fn write_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        let ctx = test_ctx();

        let input = json!({
            "file_path": file.to_str().unwrap(),
            "content": "hello world"
        });

        let output = WriteTool.execute(input, &ctx).await.unwrap();
        assert!(!output.is_error, "output: {}", output.text());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello world");
    }

    #[tokio::test]
    async fn write_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a").join("b").join("c.txt");
        let ctx = test_ctx();

        let input = json!({
            "file_path": file.to_str().unwrap(),
            "content": "nested"
        });

        let output = WriteTool.execute(input, &ctx).await.unwrap();
        assert!(!output.is_error);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "nested");
    }

    #[tokio::test]
    async fn write_overwrites_existing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("existing.txt");
        std::fs::write(&file, "old content").unwrap();
        let ctx = test_ctx();

        let input = json!({
            "file_path": file.to_str().unwrap(),
            "content": "new content"
        });

        let output = WriteTool.execute(input, &ctx).await.unwrap();
        assert!(!output.is_error);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "new content");
    }

    #[tokio::test]
    async fn write_rejects_relative_path() {
        let ctx = test_ctx();
        let input = json!({
            "file_path": "relative/path.txt",
            "content": "data"
        });

        let output = WriteTool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("absolute"));
    }

    #[tokio::test]
    async fn write_rejects_sensitive_env_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(".env");
        let ctx = test_ctx();

        let input = json!({
            "file_path": file.to_str().unwrap(),
            "content": "SECRET=123"
        });

        let output = WriteTool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("sensitive"));
    }

    #[tokio::test]
    async fn write_rejects_sensitive_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("credentials.json");
        let ctx = test_ctx();

        let input = json!({
            "file_path": file.to_str().unwrap(),
            "content": "{}"
        });

        let output = WriteTool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("sensitive"));
    }

    #[tokio::test]
    async fn write_missing_file_path() {
        let ctx = test_ctx();
        let input = json!({"content": "data"});
        let result = WriteTool.execute(input, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn write_missing_content() {
        let ctx = test_ctx();
        let input = json!({"file_path": "/tmp/test.txt"});
        let result = WriteTool.execute(input, &ctx).await;
        assert!(result.is_err());
    }

    #[test]
    fn check_sensitive_detects_patterns() {
        assert!(check_sensitive_file("/home/user/.env").is_some());
        assert!(check_sensitive_file("/home/user/.env.local").is_some());
        assert!(check_sensitive_file("/home/user/credentials.json").is_some());
        assert!(check_sensitive_file("/home/user/server.key").is_some());
        assert!(check_sensitive_file("/home/user/id_rsa").is_some());
        assert!(check_sensitive_file("/home/user/.npmrc").is_some());
    }

    #[test]
    fn check_sensitive_allows_normal_files() {
        assert!(check_sensitive_file("/home/user/main.rs").is_none());
        assert!(check_sensitive_file("/home/user/README.md").is_none());
        assert!(check_sensitive_file("/home/user/Cargo.toml").is_none());
    }
}
