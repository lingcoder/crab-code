//! Jupyter notebook tools (read + edit).

use std::fmt::Write as _;
use std::future::Future;
use std::pin::Pin;

use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use serde_json::Value;

// ─── NotebookReadTool ───────────────────────────────────────────────

/// Reads a Jupyter notebook and returns cell contents in a readable format.
pub struct NotebookReadTool;

impl Tool for NotebookReadTool {
    fn name(&self) -> &'static str {
        "notebook_read"
    }

    fn description(&self) -> &'static str {
        "Read a Jupyter notebook (.ipynb) file and return all cells with their \
         content (code, markdown, outputs) in a readable format."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "notebook_path": {
                    "type": "string",
                    "description": "Absolute path to the .ipynb file"
                }
            },
            "required": ["notebook_path"]
        })
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let notebook_path = input["notebook_path"].as_str().unwrap_or("").to_owned();

        Box::pin(async move {
            if notebook_path.is_empty() {
                return Ok(ToolOutput::error("notebook_path is required"));
            }

            let content = match tokio::fs::read_to_string(&notebook_path).await {
                Ok(c) => c,
                Err(e) => {
                    return Ok(ToolOutput::error(format!(
                        "Failed to read {notebook_path}: {e}"
                    )));
                }
            };

            let notebook: Value = match serde_json::from_str(&content) {
                Ok(v) => v,
                Err(e) => {
                    return Ok(ToolOutput::error(format!(
                        "Failed to parse notebook JSON: {e}"
                    )));
                }
            };

            let Some(cells) = notebook.get("cells").and_then(Value::as_array) else {
                return Ok(ToolOutput::error(
                    "Invalid notebook format: missing 'cells' array",
                ));
            };

            let mut output = String::new();

            // Notebook metadata summary
            if let Some(metadata) = notebook.get("metadata") {
                if let Some(kernel) = metadata
                    .get("kernelspec")
                    .and_then(|k| k.get("display_name"))
                    .and_then(Value::as_str)
                {
                    let _ = writeln!(output, "Kernel: {kernel}");
                }
                if let Some(lang) = metadata
                    .get("language_info")
                    .and_then(|l| l.get("name"))
                    .and_then(Value::as_str)
                {
                    let _ = writeln!(output, "Language: {lang}");
                }
                if !output.is_empty() {
                    let _ = writeln!(output, "---");
                }
            }

            let _ = writeln!(output, "Total cells: {}\n", cells.len());

            for (i, cell) in cells.iter().enumerate() {
                let cell_type = cell
                    .get("cell_type")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");

                let _ = writeln!(output, "--- Cell {i} [{cell_type}] ---");

                // Source content
                let source = extract_source(cell);
                if !source.is_empty() {
                    let _ = writeln!(output, "{source}");
                }

                // Outputs (for code cells)
                if cell_type == "code" {
                    if let Some(outputs) = cell.get("outputs").and_then(Value::as_array) {
                        for out in outputs {
                            format_output(&mut output, out);
                        }
                    }

                    // Execution count
                    if let Some(count) = cell.get("execution_count").and_then(Value::as_u64) {
                        let _ = writeln!(output, "[execution_count: {count}]");
                    }
                }

                output.push('\n');
            }

            Ok(ToolOutput::success(output.trim_end().to_string()))
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

/// Extract the source text from a cell.
/// Source can be a string or an array of strings.
fn extract_source(cell: &Value) -> String {
    match cell.get("source") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// Format a single cell output block into the output string.
fn format_output(buf: &mut String, output: &Value) {
    let output_type = output
        .get("output_type")
        .and_then(Value::as_str)
        .unwrap_or("");

    match output_type {
        "stream" => {
            let name = output
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("stdout");
            let text = extract_text(output);
            if !text.is_empty() {
                let _ = writeln!(buf, "[{name}]\n{text}");
            }
        }
        "execute_result" | "display_data" => {
            if let Some(data) = output.get("data") {
                if let Some(text) = data.get("text/plain") {
                    let t = value_to_text(text);
                    if !t.is_empty() {
                        let _ = writeln!(buf, "[output]\n{t}");
                    }
                }
                if data.get("image/png").is_some() || data.get("image/jpeg").is_some() {
                    let _ = writeln!(buf, "[image output]");
                }
                if let Some(html) = data.get("text/html") {
                    let _ = writeln!(buf, "[html output: {} chars]", value_to_text(html).len());
                }
            }
        }
        "error" => {
            let ename = output
                .get("ename")
                .and_then(Value::as_str)
                .unwrap_or("Error");
            let evalue = output.get("evalue").and_then(Value::as_str).unwrap_or("");
            let _ = writeln!(buf, "[error: {ename}: {evalue}]");
        }
        _ => {}
    }
}

/// Extract text from an output's "text" field (string or array of strings).
fn extract_text(output: &Value) -> String {
    match output.get("text") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// Convert a Value that may be a string or array of strings into a single string.
fn value_to_text(val: &Value) -> String {
    match val {
        Value::String(s) => s.clone(),
        Value::Array(arr) => arr
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

// ─── NotebookTool (edit — existing stub) ─────────────────────────────

/// Jupyter notebook editing tool.
pub struct NotebookTool;

impl Tool for NotebookTool {
    fn name(&self) -> &'static str {
        "notebook_edit"
    }

    fn description(&self) -> &'static str {
        "Edit a cell in a Jupyter notebook"
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "notebook_path": { "type": "string", "description": "Absolute path to the notebook" },
                "cell_number": { "type": "integer", "description": "0-indexed cell number" },
                "new_source": { "type": "string", "description": "New source for the cell" },
                "cell_type": { "type": "string", "enum": ["code", "markdown"] }
            },
            "required": ["notebook_path", "new_source"]
        })
    }

    fn execute(
        &self,
        _input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        Box::pin(async move {
            // TODO: implement notebook editing
            Ok(ToolOutput::error("not implemented"))
        })
    }

    fn requires_confirmation(&self) -> bool {
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

    fn sample_notebook() -> String {
        serde_json::json!({
            "metadata": {
                "kernelspec": { "display_name": "Python 3", "language": "python" },
                "language_info": { "name": "python" }
            },
            "cells": [
                {
                    "cell_type": "markdown",
                    "source": ["# Hello Notebook\n", "This is a test."],
                    "metadata": {}
                },
                {
                    "cell_type": "code",
                    "source": "print('hello')\n",
                    "metadata": {},
                    "execution_count": 1,
                    "outputs": [
                        {
                            "output_type": "stream",
                            "name": "stdout",
                            "text": ["hello\n"]
                        }
                    ]
                },
                {
                    "cell_type": "code",
                    "source": ["1 + 2"],
                    "metadata": {},
                    "execution_count": 2,
                    "outputs": [
                        {
                            "output_type": "execute_result",
                            "data": { "text/plain": "3" },
                            "metadata": {},
                            "execution_count": 2
                        }
                    ]
                },
                {
                    "cell_type": "code",
                    "source": "raise ValueError('oops')",
                    "metadata": {},
                    "execution_count": 3,
                    "outputs": [
                        {
                            "output_type": "error",
                            "ename": "ValueError",
                            "evalue": "oops",
                            "traceback": []
                        }
                    ]
                }
            ],
            "nbformat": 4,
            "nbformat_minor": 5
        })
        .to_string()
    }

    async fn write_temp_notebook(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(name);
        tokio::fs::write(&path, sample_notebook()).await.unwrap();
        path
    }

    #[test]
    fn notebook_read_name_and_schema() {
        let tool = NotebookReadTool;
        assert_eq!(tool.name(), "notebook_read");
        assert!(tool.is_read_only());
        let schema = tool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "notebook_path"));
    }

    #[tokio::test]
    async fn read_notebook_basic() {
        let path = write_temp_notebook("crab_test_nb_read.ipynb").await;
        let tool = NotebookReadTool;
        let input = serde_json::json!({ "notebook_path": path.to_str().unwrap() });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(!out.is_error);
        let text = out.text();
        assert!(text.contains("Kernel: Python 3"));
        assert!(text.contains("Language: python"));
        assert!(text.contains("Total cells: 4"));
        assert!(text.contains("[markdown]"));
        assert!(text.contains("# Hello Notebook"));
        assert!(text.contains("[code]"));
        assert!(text.contains("print('hello')"));
        assert!(text.contains("[stdout]"));
        assert!(text.contains("hello"));
        assert!(text.contains("[output]"));
        assert!(text.contains("3"));
        assert!(text.contains("[error: ValueError: oops]"));
        assert!(text.contains("[execution_count: 1]"));
        // Cleanup
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn read_notebook_missing_path() {
        let tool = NotebookReadTool;
        let input = serde_json::json!({ "notebook_path": "" });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn read_notebook_nonexistent_file() {
        let tool = NotebookReadTool;
        let input = serde_json::json!({ "notebook_path": "/nonexistent/nb.ipynb" });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(out.is_error);
        assert!(out.text().contains("Failed to read"));
    }

    #[tokio::test]
    async fn read_notebook_invalid_json() {
        let path = std::env::temp_dir().join("crab_test_nb_bad.ipynb");
        tokio::fs::write(&path, "not json").await.unwrap();
        let tool = NotebookReadTool;
        let input = serde_json::json!({ "notebook_path": path.to_str().unwrap() });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(out.is_error);
        assert!(out.text().contains("Failed to parse"));
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn read_notebook_missing_cells() {
        let path = std::env::temp_dir().join("crab_test_nb_nocells.ipynb");
        tokio::fs::write(&path, "{}").await.unwrap();
        let tool = NotebookReadTool;
        let input = serde_json::json!({ "notebook_path": path.to_str().unwrap() });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(out.is_error);
        assert!(out.text().contains("missing 'cells'"));
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[test]
    fn notebook_edit_name() {
        assert_eq!(NotebookTool.name(), "notebook_edit");
        assert!(NotebookTool.requires_confirmation());
    }

    #[test]
    fn extract_source_string() {
        let cell = serde_json::json!({ "source": "hello" });
        assert_eq!(extract_source(&cell), "hello");
    }

    #[test]
    fn extract_source_array() {
        let cell = serde_json::json!({ "source": ["a", "b", "c"] });
        assert_eq!(extract_source(&cell), "abc");
    }

    #[test]
    fn extract_source_missing() {
        let cell = serde_json::json!({});
        assert_eq!(extract_source(&cell), "");
    }
}
