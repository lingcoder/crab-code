use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolDisplayResult, ToolDisplayStyle, ToolOutput};
use crab_fs::grep::{GrepMatch, GrepOptions, search};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;

/// Content search tool (regex-powered).
pub const GREP_TOOL_NAME: &str = "Grep";

pub struct GrepTool;

impl Tool for GrepTool {
    fn name(&self) -> &'static str {
        GREP_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Search file contents using regex patterns. Supports content, files_with_matches, and count output modes."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The regular expression pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "File or directory to search in. Defaults to the working directory."
                },
                "glob": {
                    "type": "string",
                    "description": "Glob pattern to filter files (e.g. \"*.rs\", \"*.{ts,tsx}\")"
                },
                "output_mode": {
                    "type": "string",
                    "description": "Output mode: \"content\" (matching lines), \"files_with_matches\" (file paths only), or \"count\" (match counts per file). Defaults to \"files_with_matches\".",
                    "enum": ["content", "files_with_matches", "count"]
                },
                "context": {
                    "type": "integer",
                    "description": "Number of context lines before and after each match (for content mode)"
                },
                "head_limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return"
                }
            },
            "required": ["pattern"]
        })
    }

    #[allow(clippy::cast_possible_truncation)]
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

            let output_mode = input["output_mode"]
                .as_str()
                .unwrap_or("files_with_matches");
            let context_lines = input["context"].as_u64().unwrap_or(0) as usize;
            let head_limit = input["head_limit"].as_u64().unwrap_or(0) as usize;

            let opts = GrepOptions {
                pattern: pattern.to_string(),
                path: search_path,
                case_insensitive: false,
                file_glob: input["glob"].as_str().map(String::from),
                max_results: if output_mode == "content" {
                    head_limit
                } else {
                    0
                },
                context_lines: if output_mode == "content" {
                    context_lines
                } else {
                    0
                },
                respect_gitignore: true,
            };

            let matches = search(&opts)?;

            if matches.is_empty() {
                return Ok(ToolOutput::success("No matches found."));
            }

            let output = match output_mode {
                "content" => format_content(&matches),
                "count" => format_count(&matches, head_limit),
                // "files_with_matches" and default
                _ => format_files_with_matches(&matches, head_limit),
            };

            Ok(ToolOutput::success(output))
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    // ── CCB-aligned rendering hooks ──

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        // CCB: message = pattern: "X", path: "Y"
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
                    "Found 0 results",
                    ToolDisplayStyle::Muted,
                )],
                preview_lines: 1,
            });
        }

        // CCB: "Found N files" / "Found N lines" depending on output_mode
        // We detect by looking at content format
        let total_lines = text.lines().count();

        // Check if output is file-list mode (one file per line, no colons with line numbers)
        let is_file_list = text
            .lines()
            .all(|l| !l.contains(':') || l.starts_with('/') || l.starts_with("C:"));
        let summary = if is_file_list {
            format!("Found {total_lines} files")
        } else {
            format!("Found {total_lines} results")
        };

        let mut lines = vec![ToolDisplayLine::new(&summary, ToolDisplayStyle::Muted)];

        // Show results with ⎿ connector (CCB verbose tree style)
        for line in text.lines().take(20) {
            let style = if line.contains(':') {
                ToolDisplayStyle::Highlight
            } else {
                ToolDisplayStyle::Normal
            };
            lines.push(ToolDisplayLine::new(format!("  ⎿ {line}"), style));
        }
        if total_lines > 20 {
            lines.push(ToolDisplayLine::new(
                format!("  … +{} results", total_lines - 20),
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

/// Format output as matching lines with file path and line number.
fn format_content(matches: &[GrepMatch]) -> String {
    let mut output = String::new();
    let mut last_path: Option<&Path> = None;

    for m in matches {
        // Print file separator when path changes
        if last_path != Some(&m.path) {
            if last_path.is_some() {
                output.push('\n');
            }
            last_path = Some(&m.path);
        }

        // Context before
        for (i, line) in m.context_before.iter().enumerate() {
            let ctx_line_num = m.line_number - m.context_before.len() + i;
            let _ = writeln!(output, "{}:{ctx_line_num}-{line}", m.path.display());
        }

        // Matched line
        let _ = writeln!(
            output,
            "{}:{}:{}",
            m.path.display(),
            m.line_number,
            m.line_content
        );

        // Context after
        for (i, line) in m.context_after.iter().enumerate() {
            let after_num = m.line_number + 1 + i;
            let _ = writeln!(output, "{}:{after_num}-{line}", m.path.display());
        }
    }

    output
}

/// Format output as file paths only (deduplicated).
fn format_files_with_matches(matches: &[GrepMatch], limit: usize) -> String {
    let mut seen = Vec::new();
    for m in matches {
        if !seen.contains(&m.path) {
            seen.push(m.path.clone());
        }
    }

    let effective_limit = if limit > 0 { limit } else { seen.len() };
    let truncated = seen.len() > effective_limit;
    let display: Vec<_> = seen.iter().take(effective_limit).collect();

    let mut output = String::new();
    for path in &display {
        output.push_str(&path.to_string_lossy());
        output.push('\n');
    }
    if truncated {
        let _ = write!(
            output,
            "\n({effective_limit} files shown, more available.)\n"
        );
    }
    output
}

/// Format output as match count per file.
fn format_count(matches: &[GrepMatch], limit: usize) -> String {
    let mut counts: BTreeMap<&Path, usize> = BTreeMap::new();
    for m in matches {
        *counts.entry(&m.path).or_insert(0) += 1;
    }

    let effective_limit = if limit > 0 { limit } else { counts.len() };
    let mut output = String::new();
    for (i, (path, count)) in counts.iter().enumerate() {
        if i >= effective_limit {
            break;
        }
        let _ = writeln!(output, "{}:{count}", path.display());
    }
    output
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
    fn grep_tool_metadata() {
        let tool = GrepTool;
        assert_eq!(tool.name(), "Grep");
        assert!(tool.is_read_only());
        assert!(!tool.requires_confirmation());
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["pattern"].is_object());
        assert!(schema["properties"]["path"].is_object());
        assert!(schema["properties"]["glob"].is_object());
        assert!(schema["properties"]["output_mode"].is_object());
        assert!(schema["properties"]["context"].is_object());
        assert!(schema["properties"]["head_limit"].is_object());
    }

    #[tokio::test]
    async fn grep_finds_content() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("test.rs"),
            "fn main() {}\nfn helper() {}\nlet x = 5;\n",
        )
        .unwrap();

        let ctx = make_ctx(tmp.path());
        let input = serde_json::json!({"pattern": "fn\\s+\\w+", "output_mode": "content"});
        let result = GrepTool.execute(input, &ctx).await.unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains("fn main()"));
        assert!(text.contains("fn helper()"));
        assert!(!text.contains("let x"));
    }

    #[tokio::test]
    async fn grep_files_with_matches_mode() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a.rs"), "hello world\n").unwrap();
        fs::write(tmp.path().join("b.rs"), "goodbye world\n").unwrap();
        fs::write(tmp.path().join("c.txt"), "no match here\n").unwrap();

        let ctx = make_ctx(tmp.path());
        let input = serde_json::json!({"pattern": "world"});
        let result = GrepTool.execute(input, &ctx).await.unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains("a.rs"));
        assert!(text.contains("b.rs"));
        assert!(!text.contains("c.txt"));
    }

    #[tokio::test]
    async fn grep_count_mode() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("multi.txt"), "match1\nmatch2\nmatch3\n").unwrap();

        let ctx = make_ctx(tmp.path());
        let input = serde_json::json!({"pattern": "match", "output_mode": "count"});
        let result = GrepTool.execute(input, &ctx).await.unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains(":3"));
    }

    #[tokio::test]
    async fn grep_no_matches() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("test.txt"), "nothing here\n").unwrap();

        let ctx = make_ctx(tmp.path());
        let input = serde_json::json!({"pattern": "zzz_nonexistent"});
        let result = GrepTool.execute(input, &ctx).await.unwrap();
        assert!(!result.is_error);
        assert!(result.text().contains("No matches found"));
    }

    #[tokio::test]
    async fn grep_with_glob_filter() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("code.rs"), "hello\n").unwrap();
        fs::write(tmp.path().join("doc.md"), "hello\n").unwrap();

        let ctx = make_ctx(tmp.path());
        let input = serde_json::json!({"pattern": "hello", "glob": "*.rs"});
        let result = GrepTool.execute(input, &ctx).await.unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains("code.rs"));
        assert!(!text.contains("doc.md"));
    }

    #[tokio::test]
    async fn grep_with_context_lines() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("ctx.txt"),
            "line1\nline2\nTARGET\nline4\nline5\n",
        )
        .unwrap();

        let ctx = make_ctx(tmp.path());
        let input =
            serde_json::json!({"pattern": "TARGET", "output_mode": "content", "context": 1});
        let result = GrepTool.execute(input, &ctx).await.unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains("TARGET"));
        assert!(text.contains("line2"));
        assert!(text.contains("line4"));
    }

    #[tokio::test]
    async fn grep_missing_pattern_is_error() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = make_ctx(tmp.path());
        let input = serde_json::json!({});
        let result = GrepTool.execute(input, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn grep_invalid_regex_is_error() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = make_ctx(tmp.path());
        let input = serde_json::json!({"pattern": "[invalid"});
        let result = GrepTool.execute(input, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn grep_with_explicit_path() {
        let tmp = tempfile::tempdir().unwrap();
        let sub = tmp.path().join("subdir");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("inner.rs"), "target line\n").unwrap();
        fs::write(tmp.path().join("outer.rs"), "target line\n").unwrap();

        let ctx = make_ctx(tmp.path());
        let input = serde_json::json!({"pattern": "target", "path": "subdir"});
        let result = GrepTool.execute(input, &ctx).await.unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains("inner.rs"));
        assert!(!text.contains("outer.rs"));
    }
}
