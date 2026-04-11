//! Jupyter notebook tools (read + edit).

use std::fmt::Write as _;
use std::future::Future;
use std::pin::Pin;

use crab_common::Result;
use crab_core::tool::{
    Tool, ToolContext, ToolDisplayLine, ToolDisplayResult, ToolDisplayStyle, ToolOutput,
};
use serde_json::Value;

pub const NOTEBOOK_READ_TOOL_NAME: &str = "NotebookRead";
pub const NOTEBOOK_EDIT_TOOL_NAME: &str = "NotebookEdit";

// ─── NotebookReadTool ───────────────────────────────────────────────

/// Reads a Jupyter notebook and returns cell contents in a readable format.
pub struct NotebookReadTool;

impl Tool for NotebookReadTool {
    fn name(&self) -> &'static str {
        NOTEBOOK_READ_TOOL_NAME
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

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        let path = input["notebook_path"].as_str()?;
        let filename = path.rsplit(['/', '\\']).next().unwrap_or(path);
        Some(format!("Read ({filename})"))
    }

    fn format_result(&self, output: &ToolOutput) -> Option<ToolDisplayResult> {
        let text = output.text();
        let cell_count = text.matches("Cell ").count().max(1);
        Some(ToolDisplayResult {
            lines: vec![ToolDisplayLine::new(
                format!("Read {cell_count} cells"),
                ToolDisplayStyle::Muted,
            )],
            preview_lines: 1,
        })
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

// ─── NotebookTool (edit) ─────────────────────────────────────────────

/// Jupyter notebook editing tool — supports replace, insert, and delete modes.
pub struct NotebookTool;

impl Tool for NotebookTool {
    fn name(&self) -> &'static str {
        NOTEBOOK_EDIT_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Edit a cell in a Jupyter notebook (.ipynb). Supports three edit modes: \
         'replace' (default) replaces a cell's content, 'insert' adds a new cell \
         at the given index, 'delete' removes a cell. The cell_number is 0-indexed."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "notebook_path": {
                    "type": "string",
                    "description": "Absolute path to the .ipynb file"
                },
                "cell_number": {
                    "type": "integer",
                    "description": "0-indexed cell number to edit/insert-at/delete"
                },
                "new_source": {
                    "type": "string",
                    "description": "New source content for the cell"
                },
                "cell_type": {
                    "type": "string",
                    "enum": ["code", "markdown"],
                    "description": "Cell type (required for insert, optional for replace)"
                },
                "edit_mode": {
                    "type": "string",
                    "enum": ["replace", "insert", "delete"],
                    "description": "Edit mode: replace (default), insert, or delete"
                }
            },
            "required": ["notebook_path", "new_source"]
        })
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let notebook_path = input["notebook_path"].as_str().unwrap_or("").to_owned();
        let new_source = input["new_source"].as_str().unwrap_or("").to_owned();
        let cell_type = input["cell_type"].as_str().map(ToOwned::to_owned);
        let edit_mode = input["edit_mode"].as_str().unwrap_or("replace").to_owned();
        #[allow(clippy::cast_possible_truncation)]
        let cell_number = input["cell_number"].as_u64().map(|v| v as usize);

        Box::pin(async move {
            if notebook_path.is_empty() {
                return Ok(ToolOutput::error("notebook_path is required"));
            }

            // Read and parse
            let content = match tokio::fs::read_to_string(&notebook_path).await {
                Ok(c) => c,
                Err(e) => {
                    return Ok(ToolOutput::error(format!(
                        "Failed to read {notebook_path}: {e}"
                    )));
                }
            };

            let mut notebook: Value = match serde_json::from_str(&content) {
                Ok(v) => v,
                Err(e) => {
                    return Ok(ToolOutput::error(format!(
                        "Failed to parse notebook JSON: {e}"
                    )));
                }
            };

            let Some(cells) = notebook.get_mut("cells").and_then(Value::as_array_mut) else {
                return Ok(ToolOutput::error(
                    "Invalid notebook format: missing 'cells' array",
                ));
            };

            match edit_mode.as_str() {
                "replace" => {
                    let idx = cell_number.unwrap_or(0);
                    if idx >= cells.len() {
                        return Ok(ToolOutput::error(format!(
                            "cell_number {idx} out of range (notebook has {} cells)",
                            cells.len()
                        )));
                    }
                    // Update source
                    cells[idx]["source"] = Value::String(new_source);
                    // Update cell_type if provided
                    if let Some(ct) = &cell_type {
                        cells[idx]["cell_type"] = Value::String(ct.clone());
                    }
                }
                "insert" => {
                    let idx = cell_number.unwrap_or(cells.len());
                    if idx > cells.len() {
                        return Ok(ToolOutput::error(format!(
                            "cell_number {idx} out of range for insert (max {})",
                            cells.len()
                        )));
                    }
                    let ct = cell_type.as_deref().unwrap_or("code");
                    let new_cell = serde_json::json!({
                        "cell_type": ct,
                        "source": new_source,
                        "metadata": {},
                        "outputs": if ct == "code" { Value::Array(vec![]) } else { Value::Null },
                    });
                    cells.insert(idx, new_cell);
                }
                "delete" => {
                    let idx = cell_number.unwrap_or(0);
                    if idx >= cells.len() {
                        return Ok(ToolOutput::error(format!(
                            "cell_number {idx} out of range (notebook has {} cells)",
                            cells.len()
                        )));
                    }
                    cells.remove(idx);
                }
                other => {
                    return Ok(ToolOutput::error(format!(
                        "unknown edit_mode: '{other}' (expected replace, insert, or delete)"
                    )));
                }
            }

            // Write back
            let updated =
                serde_json::to_string_pretty(&notebook).unwrap_or_else(|_| notebook.to_string());

            if let Err(e) = tokio::fs::write(&notebook_path, &updated).await {
                return Ok(ToolOutput::error(format!(
                    "Failed to write {notebook_path}: {e}"
                )));
            }

            let cell_count = notebook["cells"].as_array().map_or(0, std::vec::Vec::len);
            Ok(ToolOutput::success(format!(
                "Notebook updated ({edit_mode}). Total cells: {cell_count}"
            )))
        })
    }

    fn requires_confirmation(&self) -> bool {
        true
    }

    // ── CCB-aligned rendering hooks ──

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        // CCB: "NotebookEdit (path)"
        let path = input["notebook_path"].as_str()?;
        let filename = path.rsplit(['/', '\\']).next().unwrap_or(path);
        Some(format!("NotebookEdit ({filename})"))
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
            ext: crab_core::tool::ToolContextExt::default(),
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
        assert_eq!(tool.name(), "NotebookRead");
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
        assert!(text.contains('3'));
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
        assert_eq!(NotebookTool.name(), "NotebookEdit");
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

    // ─── NotebookTool (edit) tests ──────────────────────────────────

    #[test]
    fn notebook_edit_schema_has_edit_mode() {
        let schema = NotebookTool.input_schema();
        assert!(schema["properties"]["edit_mode"].is_object());
        assert!(schema["properties"]["cell_number"].is_object());
        assert!(schema["properties"]["cell_type"].is_object());
    }

    #[test]
    fn notebook_edit_description_mentions_modes() {
        let desc = NotebookTool.description();
        assert!(desc.contains("replace"));
        assert!(desc.contains("insert"));
        assert!(desc.contains("delete"));
    }

    #[tokio::test]
    async fn notebook_edit_replace_cell() {
        let path = write_temp_notebook("crab_test_nb_edit_replace.ipynb").await;
        let tool = NotebookTool;
        let input = serde_json::json!({
            "notebook_path": path.to_str().unwrap(),
            "cell_number": 1,
            "new_source": "print('updated')",
            "edit_mode": "replace"
        });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(!out.is_error);
        assert!(out.text().contains("replace"));
        assert!(out.text().contains("Total cells: 4"));

        // Verify the cell was updated
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        let nb: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(nb["cells"][1]["source"], "print('updated')");
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn notebook_edit_replace_with_cell_type() {
        let path = write_temp_notebook("crab_test_nb_edit_type.ipynb").await;
        let tool = NotebookTool;
        let input = serde_json::json!({
            "notebook_path": path.to_str().unwrap(),
            "cell_number": 1,
            "new_source": "# Now markdown",
            "cell_type": "markdown",
            "edit_mode": "replace"
        });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(!out.is_error);

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        let nb: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(nb["cells"][1]["cell_type"], "markdown");
        assert_eq!(nb["cells"][1]["source"], "# Now markdown");
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn notebook_edit_insert_cell() {
        let path = write_temp_notebook("crab_test_nb_edit_insert.ipynb").await;
        let tool = NotebookTool;
        let input = serde_json::json!({
            "notebook_path": path.to_str().unwrap(),
            "cell_number": 1,
            "new_source": "# Inserted cell",
            "cell_type": "markdown",
            "edit_mode": "insert"
        });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(!out.is_error);
        assert!(out.text().contains("insert"));
        assert!(out.text().contains("Total cells: 5"));

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        let nb: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(nb["cells"][1]["source"], "# Inserted cell");
        assert_eq!(nb["cells"][1]["cell_type"], "markdown");
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn notebook_edit_insert_defaults_to_code() {
        let path = write_temp_notebook("crab_test_nb_edit_insert_code.ipynb").await;
        let tool = NotebookTool;
        let input = serde_json::json!({
            "notebook_path": path.to_str().unwrap(),
            "cell_number": 0,
            "new_source": "x = 1",
            "edit_mode": "insert"
        });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(!out.is_error);

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        let nb: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(nb["cells"][0]["cell_type"], "code");
        assert_eq!(nb["cells"][0]["source"], "x = 1");
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn notebook_edit_delete_cell() {
        let path = write_temp_notebook("crab_test_nb_edit_delete.ipynb").await;
        let tool = NotebookTool;
        let input = serde_json::json!({
            "notebook_path": path.to_str().unwrap(),
            "cell_number": 0,
            "new_source": "",
            "edit_mode": "delete"
        });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(!out.is_error);
        assert!(out.text().contains("delete"));
        assert!(out.text().contains("Total cells: 3"));
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn notebook_edit_out_of_range() {
        let path = write_temp_notebook("crab_test_nb_edit_oor.ipynb").await;
        let tool = NotebookTool;
        let input = serde_json::json!({
            "notebook_path": path.to_str().unwrap(),
            "cell_number": 99,
            "new_source": "x",
            "edit_mode": "replace"
        });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(out.is_error);
        assert!(out.text().contains("out of range"));
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn notebook_edit_unknown_mode() {
        let path = write_temp_notebook("crab_test_nb_edit_bad_mode.ipynb").await;
        let tool = NotebookTool;
        let input = serde_json::json!({
            "notebook_path": path.to_str().unwrap(),
            "new_source": "x",
            "edit_mode": "merge"
        });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(out.is_error);
        assert!(out.text().contains("unknown edit_mode"));
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn notebook_edit_empty_path() {
        let tool = NotebookTool;
        let input = serde_json::json!({ "notebook_path": "", "new_source": "x" });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn notebook_edit_default_mode_is_replace() {
        let path = write_temp_notebook("crab_test_nb_edit_default.ipynb").await;
        let tool = NotebookTool;
        // No edit_mode — should default to replace
        let input = serde_json::json!({
            "notebook_path": path.to_str().unwrap(),
            "cell_number": 0,
            "new_source": "# Replaced default"
        });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(!out.is_error);
        assert!(out.text().contains("replace"));

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        let nb: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(nb["cells"][0]["source"], "# Replaced default");
        let _ = tokio::fs::remove_file(&path).await;
    }
}
