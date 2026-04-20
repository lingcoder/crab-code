use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolDisplayResult, ToolDisplayStyle, ToolOutput};
use crab_fs::glob::{GlobOptions, find_files};
use serde_json::Value;
use std::fmt::Write as _;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;

/// File pattern matching tool.
pub const GLOB_TOOL_NAME: &str = "Glob";

pub struct GlobTool;

impl Tool for GlobTool {
    fn name(&self) -> &'static str {
        GLOB_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Fast file pattern matching using glob patterns. Returns matching file paths sorted by modification time."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The glob pattern to match files against (e.g. \"**/*.rs\", \"src/**/*.ts\")"
                },
                "path": {
                    "type": "string",
                    "description": "The directory to search in. Defaults to the working directory."
                }
            },
            "required": ["pattern"]
        })
    }

    fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let working_dir = ctx.working_dir.clone();
        Box::pin(async move {
            let pattern = input["pattern"].as_str().ok_or_else(|| {
                crab_common::Error::Other("missing required field: pattern".into())
            })?;

            let search_path = match input["path"].as_str() {
                Some(p) if !p.is_empty() => {
                    let p = Path::new(p);
                    if p.is_absolute() {
                        p.to_path_buf()
                    } else {
                        working_dir.join(p)
                    }
                }
                _ => working_dir,
            };

            let opts = GlobOptions::new(&search_path, pattern);
            let result = find_files(&opts)?;

            if result.matches.is_empty() {
                return Ok(ToolOutput::success("No files matched the pattern."));
            }

            let mut output = String::new();
            for path in &result.matches {
                output.push_str(&path.to_string_lossy());
                output.push('\n');
            }

            if result.truncated {
                let count = result.matches.len();
                let _ = write!(output, "\n(Results truncated. {count} files shown.)\n");
            }

            Ok(ToolOutput::success(output))
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    // ── CCB-aligned rendering hooks ──

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        // CCB: userFacingName="Search", message = pattern: "X", path: "Y"
        let pattern = input["pattern"].as_str()?;
        let path = input["path"].as_str();
        let msg = match path {
            Some(p) => format!("pattern: \"{pattern}\", path: \"{p}\""),
            None => format!("pattern: \"{pattern}\""),
        };
        Some(format!("Search ({msg})"))
    }

    fn format_result(&self, output: &ToolOutput) -> Option<ToolDisplayResult> {
        use crab_core::tool::{ToolDisplayLine, ToolDisplayResult, ToolDisplayStyle};
        let text = output.text();
        if text.is_empty() {
            return Some(ToolDisplayResult {
                lines: vec![ToolDisplayLine::new(
                    "Found 0 files",
                    ToolDisplayStyle::Muted,
                )],
                preview_lines: 1,
            });
        }
        // CCB: "Found N files" (N bold) as summary, files listed below in verbose
        let file_count = text.lines().count();
        let mut lines = vec![ToolDisplayLine::new(
            format!("Found {file_count} files"),
            ToolDisplayStyle::Muted,
        )];
        // Show file list indented with ⎿ connector (CCB verbose style)
        for f in text.lines().take(20) {
            lines.push(ToolDisplayLine::new(
                format!("  ⎿ {f}"),
                ToolDisplayStyle::Normal,
            ));
        }
        if file_count > 20 {
            lines.push(ToolDisplayLine::new(
                format!("  … +{} files", file_count - 20),
                ToolDisplayStyle::Muted,
            ));
        }
        Some(ToolDisplayResult {
            lines,
            preview_lines: 1,
        })
    }

    fn display_color(&self) -> ToolDisplayStyle {
        ToolDisplayStyle::Highlight
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::permission::{PermissionMode, PermissionPolicy};
    use crab_core::tool::ToolContext;
    use std::fs;
    use tokio_util::sync::CancellationToken;

    fn make_ctx(dir: &Path) -> ToolContext {
        ToolContext {
            working_dir: dir.to_path_buf(),
            permission_mode: PermissionMode::Default,
            session_id: "test".into(),
            cancellation_token: CancellationToken::new(),
            permission_policy: PermissionPolicy::default(),
            ext: crab_core::tool::ToolContextExt::default(),
        }
    }

    #[test]
    fn glob_tool_metadata() {
        let tool = GlobTool;
        assert_eq!(tool.name(), "Glob");
        assert!(tool.is_read_only());
        assert!(!tool.requires_confirmation());
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["pattern"].is_object());
        assert!(schema["properties"]["path"].is_object());
        let required = schema["required"].as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0], "pattern");
    }

    #[tokio::test]
    async fn glob_finds_rs_files() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("main.rs"), "fn main() {}").unwrap();
        fs::write(tmp.path().join("lib.rs"), "pub mod lib;").unwrap();
        fs::write(tmp.path().join("readme.md"), "# README").unwrap();

        let ctx = make_ctx(tmp.path());
        let input = serde_json::json!({"pattern": "*.rs"});
        let result = GlobTool.execute(input, &ctx).await.unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains("main.rs"));
        assert!(text.contains("lib.rs"));
        assert!(!text.contains("readme.md"));
    }

    #[tokio::test]
    async fn glob_no_matches() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("readme.md"), "# hi").unwrap();

        let ctx = make_ctx(tmp.path());
        let input = serde_json::json!({"pattern": "*.xyz"});
        let result = GlobTool.execute(input, &ctx).await.unwrap();
        assert!(!result.is_error);
        assert!(result.text().contains("No files matched"));
    }

    #[tokio::test]
    async fn glob_with_explicit_path() {
        let tmp = tempfile::tempdir().unwrap();
        let sub = tmp.path().join("subdir");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("file.rs"), "// code").unwrap();
        fs::write(tmp.path().join("root.rs"), "// root").unwrap();

        let ctx = make_ctx(tmp.path());
        let input = serde_json::json!({"pattern": "*.rs", "path": "subdir"});
        let result = GlobTool.execute(input, &ctx).await.unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains("file.rs"));
        assert!(!text.contains("root.rs"));
    }

    #[tokio::test]
    async fn glob_missing_pattern_is_error() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = make_ctx(tmp.path());
        let input = serde_json::json!({});
        let result = GlobTool.execute(input, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn glob_invalid_pattern_is_error() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = make_ctx(tmp.path());
        let input = serde_json::json!({"pattern": "[invalid"});
        let result = GlobTool.execute(input, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn glob_recursive_pattern() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("top.rs"), "// top").unwrap();
        let nested = tmp.path().join("src").join("sub");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("deep.rs"), "// deep").unwrap();

        let ctx = make_ctx(tmp.path());
        let input = serde_json::json!({"pattern": "**/*.rs"});
        let result = GlobTool.execute(input, &ctx).await.unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains("top.rs"));
        assert!(text.contains("deep.rs"));
    }
}
