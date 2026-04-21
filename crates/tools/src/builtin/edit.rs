use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolDisplayResult, ToolDisplayStyle, ToolOutput};
use serde_json::Value;
use std::fmt::Write;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use crate::str_utils::truncate_chars;

/// Diff-based file editing tool.
pub const EDIT_TOOL_NAME: &str = "Edit";

pub struct EditTool;

impl Tool for EditTool {
    fn name(&self) -> &'static str {
        EDIT_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Perform exact string replacements in files"
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the file to modify"
                },
                "old_string": {
                    "type": "string",
                    "description": "The text to replace"
                },
                "new_string": {
                    "type": "string",
                    "description": "The replacement text (must differ from old_string)"
                },
                "replace_all": {
                    "type": "boolean",
                    "default": false,
                    "description": "Replace all occurrences (default: false, replace first only)"
                },
                "fuzzy_match": {
                    "type": "boolean",
                    "default": false,
                    "description": "If true, try whitespace-insensitive matching when exact match fails"
                },
                "dry_run": {
                    "type": "boolean",
                    "default": false,
                    "description": "If true, return a unified diff preview without modifying the file"
                }
            },
            "required": ["file_path", "old_string", "new_string"]
        })
    }

    #[allow(clippy::too_many_lines)]
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
                    crab_common::Error::Other("missing required parameter: file_path".into())
                })?;

            let old_string = input
                .get("old_string")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    crab_common::Error::Other("missing required parameter: old_string".into())
                })?;

            let new_string = input
                .get("new_string")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    crab_common::Error::Other("missing required parameter: new_string".into())
                })?;

            let replace_all = input
                .get("replace_all")
                .and_then(Value::as_bool)
                .unwrap_or(false);

            let fuzzy_match = input
                .get("fuzzy_match")
                .and_then(Value::as_bool)
                .unwrap_or(false);

            let dry_run = input
                .get("dry_run")
                .and_then(Value::as_bool)
                .unwrap_or(false);

            let path = Path::new(file_path);

            // Validate absolute path
            if !path.is_absolute() {
                return Ok(ToolOutput::error(format!(
                    "file_path must be absolute, got: {file_path}"
                )));
            }

            // Check file exists
            if !path.exists() {
                return Ok(ToolOutput::error(format!("file not found: {file_path}")));
            }

            // Validate old_string != new_string
            if old_string == new_string {
                return Ok(ToolOutput::error(
                    "old_string and new_string must be different".to_string(),
                ));
            }

            // Validate old_string is not empty
            if old_string.is_empty() {
                return Ok(ToolOutput::error(
                    "old_string must not be empty".to_string(),
                ));
            }

            // Read the file
            let content = tokio::fs::read_to_string(path).await.map_err(|e| {
                crab_common::Error::Other(format!("failed to read {file_path}: {e}"))
            })?;

            // Resolve match: exact or fuzzy
            let resolved = resolve_match(&content, old_string, fuzzy_match, replace_all, file_path);
            let (effective_old, used_fuzzy, new_content) = match resolved {
                Ok((eff, fuzzy)) => {
                    let replacement = if replace_all {
                        content.replace(eff.as_str(), new_string)
                    } else {
                        content.replacen(eff.as_str(), new_string, 1)
                    };
                    (eff, fuzzy, replacement)
                }
                Err(output) => return Ok(output),
            };

            let effective_count = content.matches(effective_old.as_str()).count();

            // Dry-run mode: return diff preview without writing
            if dry_run {
                let diff =
                    crab_fs::diff::unified_diff(&content, &new_content, file_path, file_path);
                let mut out = String::from("[dry-run] Preview of changes:\n");
                if used_fuzzy {
                    let _ = writeln!(out, "(fuzzy match used)");
                }
                out.push_str(&diff);
                return Ok(ToolOutput::success(out));
            }

            // Write the file back
            tokio::fs::write(path, &new_content).await.map_err(|e| {
                crab_common::Error::Other(format!("failed to write {file_path}: {e}"))
            })?;

            let mut msg = if replace_all {
                format!("Replaced {effective_count} occurrence(s) in {file_path}")
            } else {
                format!("Replaced 1 occurrence in {file_path}")
            };
            if used_fuzzy {
                let _ = write!(msg, " (fuzzy match)");
            }

            Ok(ToolOutput::success(msg))
        })
    }

    fn requires_confirmation(&self) -> bool {
        true
    }

    // ── CCB-aligned rendering hooks ──

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        // CCB: userFacingName = "Update" (if old_string non-empty) or "Create" (if empty)
        // message = file path
        let path = input["file_path"].as_str()?;
        let filename = path.rsplit(['/', '\\']).next().unwrap_or(path);
        let verb = if input["old_string"].as_str().is_some_and(|s| !s.is_empty()) {
            "Update"
        } else {
            "Create"
        };
        Some(format!("{verb} ({filename})"))
    }

    fn format_result(&self, output: &ToolOutput) -> Option<ToolDisplayResult> {
        use crab_core::tool::{ToolDisplayLine, ToolDisplayResult, ToolDisplayStyle};
        let text = output.text();
        if text.is_empty() {
            return None;
        }

        // CCB: summary "Added N lines, Removed M lines" + colored diff below
        let mut added = 0usize;
        let mut removed = 0usize;
        let mut diff_lines: Vec<ToolDisplayLine> = Vec::new();

        for line in text.lines() {
            if line.starts_with('+') {
                added += 1;
                diff_lines.push(ToolDisplayLine::new(line, ToolDisplayStyle::DiffAdd));
            } else if line.starts_with('-') {
                removed += 1;
                diff_lines.push(ToolDisplayLine::new(line, ToolDisplayStyle::DiffRemove));
            } else if line.starts_with("@@") {
                diff_lines.push(ToolDisplayLine::new(line, ToolDisplayStyle::Highlight));
            } else {
                diff_lines.push(ToolDisplayLine::new(line, ToolDisplayStyle::DiffContext));
            }
        }

        // Summary line first (CCB style: "Added N lines, Removed M lines")
        let mut result_lines = vec![ToolDisplayLine::new(
            format!("Added {added} lines, Removed {removed} lines"),
            ToolDisplayStyle::Muted,
        )];
        result_lines.extend(diff_lines);

        Some(ToolDisplayResult {
            lines: result_lines,
            preview_lines: 1, // condensed: show summary only
        })
    }

    fn format_rejected_summary(&self, input: &Value) -> Option<String> {
        let path = input["file_path"].as_str()?;
        let filename = path.rsplit(['/', '\\']).next().unwrap_or(path);
        Some(format!("Update rejected ({filename})"))
    }

    fn format_rejected(&self, input: &Value) -> Option<ToolDisplayResult> {
        use crab_core::tool::ToolDisplayLine;
        let old = input["old_string"].as_str()?;
        let new = input["new_string"].as_str().unwrap_or("");
        let mut lines = Vec::new();
        for line in old.lines().take(3) {
            lines.push(ToolDisplayLine::new(
                format!("- {line}"),
                ToolDisplayStyle::DiffRemove,
            ));
        }
        for line in new.lines().take(3) {
            lines.push(ToolDisplayLine::new(
                format!("+ {line}"),
                ToolDisplayStyle::DiffAdd,
            ));
        }
        Some(ToolDisplayResult {
            lines,
            preview_lines: 5,
        })
    }

    fn format_error(&self, output: &ToolOutput, input: &Value) -> Option<ToolDisplayResult> {
        use crab_core::tool::{ToolDisplayLine, ToolDisplayStyle};
        let text = output.text();
        let path = input["file_path"].as_str().unwrap_or("?");
        let filename = path.rsplit(['/', '\\']).next().unwrap_or(path);

        let mut lines = vec![ToolDisplayLine::new(
            format!("Error editing {filename}"),
            ToolDisplayStyle::Error,
        )];

        if text.contains("not been read") {
            lines.push(ToolDisplayLine::new(
                "Hint: Read the file before editing",
                ToolDisplayStyle::Muted,
            ));
        } else if text.contains("not found") || text.contains("No such file") {
            lines.push(ToolDisplayLine::new(
                format!("Hint: {path} does not exist — check the path"),
                ToolDisplayStyle::Muted,
            ));
        } else if text.contains("not unique") {
            lines.push(ToolDisplayLine::new(
                "Hint: old_string matches multiple locations — add more context",
                ToolDisplayStyle::Muted,
            ));
        }

        Some(ToolDisplayResult {
            lines,
            preview_lines: 2,
        })
    }

    fn display_color(&self) -> ToolDisplayStyle {
        ToolDisplayStyle::DiffAdd
    }
}

/// Resolve the match target: try exact match first, then fuzzy if enabled.
/// Returns `Ok((effective_old_string, used_fuzzy))` or `Err(ToolOutput)` on failure.
fn resolve_match(
    content: &str,
    old_string: &str,
    fuzzy: bool,
    replace_all: bool,
    file_path: &str,
) -> std::result::Result<(String, bool), ToolOutput> {
    let match_count = content.matches(old_string).count();

    let (effective_old, used_fuzzy) = if match_count == 0 && fuzzy {
        match find_fuzzy_match(content, old_string) {
            Some(matched) => (matched, true),
            None => {
                return Err(ToolOutput::error(format!(
                    "old_string not found in {file_path} (exact and fuzzy match both failed)"
                )));
            }
        }
    } else if match_count == 0 {
        return Err(ToolOutput::error(format!(
            "old_string not found in {file_path}"
        )));
    } else {
        (old_string.to_owned(), false)
    };

    let effective_count = content.matches(effective_old.as_str()).count();

    if !replace_all && effective_count > 1 {
        let locations = find_match_locations(content, &effective_old);
        let mut msg = format!(
            "old_string appears {effective_count} times in {file_path}. \
             Use replace_all: true to replace all occurrences, \
             or provide more context to make the match unique.\n\nMatch locations:"
        );
        for (line_num, context_line) in &locations {
            let _ = write!(msg, "\n  line {line_num}: {context_line}");
        }
        return Err(ToolOutput::error(msg));
    }

    Ok((effective_old, used_fuzzy))
}

/// Normalize whitespace in a string: collapse runs of whitespace into single spaces.
fn normalize_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Try to find a fuzzy match in `content` for `needle` by normalizing whitespace.
/// Returns the actual substring from `content` that matches, if found.
fn find_fuzzy_match(content: &str, needle: &str) -> Option<String> {
    let normalized_needle = normalize_whitespace(needle);
    if normalized_needle.is_empty() {
        return None;
    }

    // Split content into lines, try matching contiguous line groups
    let needle_line_count = needle.lines().count().max(1);
    let content_lines: Vec<&str> = content.lines().collect();

    for start in 0..content_lines.len() {
        // Try windows of needle_line_count lines, plus one extra on each side
        for window_size in [needle_line_count, needle_line_count + 1] {
            let end = start + window_size;
            if end > content_lines.len() {
                continue;
            }
            let candidate = content_lines[start..end].join("\n");
            if normalize_whitespace(&candidate) == normalized_needle {
                return Some(candidate);
            }
        }
    }

    None
}

/// Find all locations (1-based line number + trimmed context) where `needle` appears.
fn find_match_locations(content: &str, needle: &str) -> Vec<(usize, String)> {
    let mut locations = Vec::new();
    let mut search_from = 0;

    while let Some(pos) = content[search_from..].find(needle) {
        let abs_pos = search_from + pos;
        let line_num = content[..abs_pos].chars().filter(|&c| c == '\n').count() + 1;
        // Get the line containing the start of the match
        let line_start = content[..abs_pos].rfind('\n').map_or(0, |p| p + 1);
        let line_end = content[abs_pos..]
            .find('\n')
            .map_or(content.len(), |p| abs_pos + p);
        let context_line = content[line_start..line_end].trim();
        // Truncate long context lines. File content may contain multi-byte
        // UTF-8 (CJK, emoji in comments/strings); truncate_chars counts
        // codepoints to avoid panics on non-ASCII input.
        let display = truncate_chars(context_line, 80, "...");
        locations.push((line_num, display));
        search_from = abs_pos + needle.len();
    }

    locations
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
        let tool = EditTool;
        assert_eq!(tool.name(), "Edit");
        assert!(!tool.is_read_only());
        assert!(tool.requires_confirmation());
    }

    #[test]
    fn schema_has_required_fields() {
        let schema = EditTool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("file_path")));
        assert!(required.contains(&json!("old_string")));
        assert!(required.contains(&json!("new_string")));
    }

    #[tokio::test]
    async fn edit_single_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.rs");
        std::fs::write(&file, "fn hello() {}\nfn world() {}\n").unwrap();
        let ctx = test_ctx();

        let input = json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "fn hello() {}",
            "new_string": "fn greeting() {}"
        });

        let output = EditTool.execute(input, &ctx).await.unwrap();
        assert!(!output.is_error, "output: {}", output.text());
        let content = std::fs::read_to_string(&file).unwrap();
        assert!(content.contains("fn greeting() {}"));
        assert!(!content.contains("fn hello() {}"));
    }

    #[tokio::test]
    async fn edit_replace_all() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "foo bar foo baz foo").unwrap();
        let ctx = test_ctx();

        let input = json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "foo",
            "new_string": "qux",
            "replace_all": true
        });

        let output = EditTool.execute(input, &ctx).await.unwrap();
        assert!(!output.is_error);
        assert!(output.text().contains("3 occurrence"));
        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, "qux bar qux baz qux");
    }

    #[tokio::test]
    async fn edit_rejects_non_unique_without_replace_all() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "aaa bbb aaa").unwrap();
        let ctx = test_ctx();

        let input = json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "aaa",
            "new_string": "ccc"
        });

        let output = EditTool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("2 times"));
    }

    #[tokio::test]
    async fn edit_old_string_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "hello world").unwrap();
        let ctx = test_ctx();

        let input = json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "nonexistent",
            "new_string": "replacement"
        });

        let output = EditTool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("not found"));
    }

    #[tokio::test]
    async fn edit_same_strings_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "content").unwrap();
        let ctx = test_ctx();

        let input = json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "same",
            "new_string": "same"
        });

        let output = EditTool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("different"));
    }

    #[tokio::test]
    async fn edit_empty_old_string_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "content").unwrap();
        let ctx = test_ctx();

        let input = json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "",
            "new_string": "something"
        });

        let output = EditTool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("empty"));
    }

    #[tokio::test]
    async fn edit_file_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("nonexistent.txt");
        let ctx = test_ctx();

        let input = json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "foo",
            "new_string": "bar"
        });

        let output = EditTool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("not found"));
    }

    #[tokio::test]
    async fn edit_rejects_relative_path() {
        let ctx = test_ctx();
        let input = json!({
            "file_path": "relative/path.txt",
            "old_string": "foo",
            "new_string": "bar"
        });

        let output = EditTool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("absolute"));
    }

    #[tokio::test]
    async fn edit_missing_parameters() {
        let ctx = test_ctx();

        // Missing file_path
        let result = EditTool
            .execute(json!({"old_string": "a", "new_string": "b"}), &ctx)
            .await;
        assert!(result.is_err());

        // Missing old_string
        let result = EditTool
            .execute(json!({"file_path": "/tmp/x", "new_string": "b"}), &ctx)
            .await;
        assert!(result.is_err());

        // Missing new_string
        let result = EditTool
            .execute(json!({"file_path": "/tmp/x", "old_string": "a"}), &ctx)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn edit_preserves_unchanged_content() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        let original = "line 1\nline 2\nline 3\n";
        std::fs::write(&file, original).unwrap();
        let ctx = test_ctx();

        let input = json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "line 2",
            "new_string": "LINE TWO"
        });

        EditTool.execute(input, &ctx).await.unwrap();
        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, "line 1\nLINE TWO\nline 3\n");
    }

    // ── Fuzzy match tests ──────────────────────────────────────────

    #[test]
    fn normalize_whitespace_collapses() {
        assert_eq!(normalize_whitespace("  hello   world  "), "hello world");
        assert_eq!(normalize_whitespace("a\t\nb"), "a b");
        assert_eq!(normalize_whitespace("single"), "single");
        assert_eq!(normalize_whitespace(""), "");
    }

    #[test]
    fn fuzzy_match_finds_whitespace_variant() {
        let content = "fn  hello()  {\n    body\n}\n";
        let needle = "fn hello() {\n    body\n}";
        let result = find_fuzzy_match(content, needle);
        assert!(result.is_some());
        // Should match the 3-line block with original whitespace
        assert_eq!(result.unwrap(), "fn  hello()  {\n    body\n}");
    }

    #[test]
    fn fuzzy_match_returns_none_when_no_match() {
        let content = "fn hello() {}\n";
        let needle = "fn totally_different() {}";
        assert!(find_fuzzy_match(content, needle).is_none());
    }

    #[tokio::test]
    async fn edit_fuzzy_match_whitespace_difference() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("fuzzy.rs");
        std::fs::write(&file, "fn  hello()  {}\nfn world() {}\n").unwrap();
        let ctx = test_ctx();

        let input = json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "fn hello() {}",
            "new_string": "fn greeting() {}",
            "fuzzy_match": true
        });

        let output = EditTool.execute(input, &ctx).await.unwrap();
        assert!(!output.is_error, "output: {}", output.text());
        assert!(output.text().contains("fuzzy match"));
        let content = std::fs::read_to_string(&file).unwrap();
        assert!(content.contains("fn greeting() {}"));
    }

    #[tokio::test]
    async fn edit_fuzzy_match_disabled_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("nofuzzy.rs");
        std::fs::write(&file, "fn  hello()  {}\n").unwrap();
        let ctx = test_ctx();

        // Without fuzzy_match, should fail because exact match doesn't exist
        let input = json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "fn hello() {}",
            "new_string": "fn greeting() {}"
        });

        let output = EditTool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("not found"));
    }

    #[tokio::test]
    async fn edit_fuzzy_match_both_fail() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("nofuzzy2.rs");
        std::fs::write(&file, "fn hello() {}\n").unwrap();
        let ctx = test_ctx();

        let input = json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "fn totally_different() {}",
            "new_string": "fn replacement() {}",
            "fuzzy_match": true
        });

        let output = EditTool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("fuzzy match both failed"));
    }

    // ── Conflict detection tests ───────────────────────────────────

    #[test]
    fn find_match_locations_basic() {
        let content = "foo bar\nbaz foo\nqux\nfoo end";
        let locs = find_match_locations(content, "foo");
        assert_eq!(locs.len(), 3);
        assert_eq!(locs[0].0, 1); // line 1
        assert_eq!(locs[1].0, 2); // line 2
        assert_eq!(locs[2].0, 4); // line 4
    }

    #[tokio::test]
    async fn edit_conflict_shows_line_numbers() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("conflict.txt");
        std::fs::write(&file, "let x = 1;\nlet y = 2;\nlet x = 3;\n").unwrap();
        let ctx = test_ctx();

        let input = json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "let x",
            "new_string": "let z"
        });

        let output = EditTool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
        let text = output.text();
        assert!(text.contains("2 times"));
        assert!(text.contains("Match locations"));
        assert!(text.contains("line 1"));
        assert!(text.contains("line 3"));
    }

    // ── Dry-run tests ──────────────────────────────────────────────

    #[tokio::test]
    async fn edit_dry_run_shows_diff() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("dryrun.txt");
        let original = "line 1\nline 2\nline 3\n";
        std::fs::write(&file, original).unwrap();
        let ctx = test_ctx();

        let input = json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "line 2",
            "new_string": "LINE TWO",
            "dry_run": true
        });

        let output = EditTool.execute(input, &ctx).await.unwrap();
        assert!(!output.is_error);
        let text = output.text();
        assert!(text.contains("[dry-run]"));
        assert!(text.contains("-line 2"));
        assert!(text.contains("+LINE TWO"));
        // File should NOT be modified
        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, original);
    }

    #[tokio::test]
    async fn edit_dry_run_with_fuzzy() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("dryfuzzy.rs");
        std::fs::write(&file, "fn  hello()  {}\nfn world() {}\n").unwrap();
        let ctx = test_ctx();

        let input = json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "fn hello() {}",
            "new_string": "fn greeting() {}",
            "fuzzy_match": true,
            "dry_run": true
        });

        let output = EditTool.execute(input, &ctx).await.unwrap();
        assert!(!output.is_error);
        let text = output.text();
        assert!(text.contains("[dry-run]"));
        assert!(text.contains("fuzzy match used"));
        // File should NOT be modified
        let content = std::fs::read_to_string(&file).unwrap();
        assert!(content.contains("fn  hello()  {}"));
    }

    #[tokio::test]
    async fn edit_dry_run_replace_all() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("dryall.txt");
        let original = "foo bar foo baz foo";
        std::fs::write(&file, original).unwrap();
        let ctx = test_ctx();

        let input = json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "foo",
            "new_string": "qux",
            "replace_all": true,
            "dry_run": true
        });

        let output = EditTool.execute(input, &ctx).await.unwrap();
        assert!(!output.is_error);
        let text = output.text();
        assert!(text.contains("[dry-run]"));
        // File should NOT be modified
        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, original);
    }

    #[test]
    fn schema_has_new_optional_fields() {
        let schema = EditTool.input_schema();
        assert!(schema["properties"]["fuzzy_match"].is_object());
        assert!(schema["properties"]["dry_run"].is_object());
    }
}
