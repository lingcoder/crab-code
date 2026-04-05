use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use serde_json::Value;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;

/// Diff-based file editing tool.
pub struct EditTool;

impl Tool for EditTool {
    fn name(&self) -> &'static str {
        "edit"
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
                }
            },
            "required": ["file_path", "old_string", "new_string"]
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

            // Count occurrences
            let match_count = content.matches(old_string).count();

            if match_count == 0 {
                return Ok(ToolOutput::error(format!(
                    "old_string not found in {file_path}"
                )));
            }

            // Uniqueness check: when not using replace_all, old_string must appear exactly once
            if !replace_all && match_count > 1 {
                return Ok(ToolOutput::error(format!(
                    "old_string appears {match_count} times in {file_path}. \
                     Use replace_all: true to replace all occurrences, \
                     or provide more context to make the match unique."
                )));
            }

            // Perform replacement
            let new_content = if replace_all {
                content.replace(old_string, new_string)
            } else {
                content.replacen(old_string, new_string, 1)
            };

            // Write the file back
            tokio::fs::write(path, &new_content).await.map_err(|e| {
                crab_common::Error::Other(format!("failed to write {file_path}: {e}"))
            })?;

            let msg = if replace_all {
                format!("Replaced {match_count} occurrence(s) in {file_path}")
            } else {
                format!("Replaced 1 occurrence in {file_path}")
            };

            Ok(ToolOutput::success(msg))
        })
    }

    fn requires_confirmation(&self) -> bool {
        true
    }
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
        }
    }

    #[test]
    fn tool_metadata() {
        let tool = EditTool;
        assert_eq!(tool.name(), "edit");
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
}
