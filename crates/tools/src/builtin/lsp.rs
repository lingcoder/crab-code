//! LSP (Language Server Protocol) tool stub.
//!
//! Phase 1: returns stub responses. Real LSP client integration is TODO.

use std::future::Future;
use std::pin::Pin;

use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolDisplayResult, ToolOutput};
use serde_json::Value;

/// LSP operations tool — stub for Phase 1.
pub const LSP_TOOL_NAME: &str = "LSP";

pub struct LspTool;

impl Tool for LspTool {
    fn name(&self) -> &'static str {
        LSP_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Perform LSP operations on source files (go to definition, find references, \
         hover, diagnostics). Phase 1 stub — returns placeholder responses."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "description": "LSP operation to perform",
                    "enum": ["goToDefinition", "findReferences", "hover", "diagnostics", "rename", "codeAction"]
                },
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the source file"
                },
                "line": {
                    "type": "integer",
                    "description": "1-based line number"
                },
                "column": {
                    "type": "integer",
                    "description": "1-based column number"
                },
                "new_name": {
                    "type": "string",
                    "description": "New name for rename operation (only for rename)"
                }
            },
            "required": ["operation", "file_path", "line", "column"]
        })
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let operation = input["operation"].as_str().unwrap_or("").to_owned();
        let file_path = input["file_path"].as_str().unwrap_or("").to_owned();
        #[allow(clippy::cast_possible_truncation)]
        let line = input["line"].as_u64().unwrap_or(0) as u32;
        #[allow(clippy::cast_possible_truncation)]
        let column = input["column"].as_u64().unwrap_or(0) as u32;

        Box::pin(async move {
            if file_path.is_empty() {
                return Ok(ToolOutput::error("file_path is required"));
            }
            if operation.is_empty() {
                return Ok(ToolOutput::error("operation is required"));
            }
            if line == 0 || column == 0 {
                return Ok(ToolOutput::error(
                    "line and column must be positive integers (1-based)",
                ));
            }

            let msg = match operation.as_str() {
                "goToDefinition" => format!(
                    "LSP goToDefinition stub: {file_path}:{line}:{column}\n\
                     TODO: Connect to language server and resolve definition location."
                ),
                "findReferences" => format!(
                    "LSP findReferences stub: {file_path}:{line}:{column}\n\
                     TODO: Connect to language server and find all references."
                ),
                "hover" => format!(
                    "LSP hover stub: {file_path}:{line}:{column}\n\
                     TODO: Connect to language server and return hover information."
                ),
                "diagnostics" => format!(
                    "LSP diagnostics stub: {file_path}\n\
                     TODO: Connect to language server and return file diagnostics."
                ),
                "rename" => format!(
                    "LSP rename stub: {file_path}:{line}:{column}\n\
                     TODO: Connect to language server and perform rename refactoring."
                ),
                "codeAction" => format!(
                    "LSP codeAction stub: {file_path}:{line}:{column}\n\
                     TODO: Connect to language server and return available code actions."
                ),
                other => {
                    return Ok(ToolOutput::error(format!(
                        "Unknown LSP operation: {other}. \
                         Supported: goToDefinition, findReferences, hover, diagnostics, rename, codeAction"
                    )));
                }
            };

            Ok(ToolOutput::success(msg))
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    // ── CCB-aligned rendering hooks ──

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        // CCB: "LSP (operation: "X", symbol: "Y", in: "Z")"
        let op = input["operation"].as_str()?;
        let file = input["file_path"].as_str().unwrap_or("?");
        let filename = file.rsplit(['/', '\\']).next().unwrap_or(file);
        Some(format!("LSP (operation: \"{op}\", in: \"{filename}\")"))
    }

    fn format_result(&self, output: &ToolOutput) -> Option<ToolDisplayResult> {
        use crab_core::tool::{ToolDisplayLine, ToolDisplayResult, ToolDisplayStyle};
        let text = output.text();
        if text.is_empty() {
            return None;
        }
        // CCB: "Found N references" / "Found N symbols" etc.
        let line_count = text.lines().count();
        Some(ToolDisplayResult {
            lines: vec![ToolDisplayLine::new(
                format!("Found {line_count} results"),
                ToolDisplayStyle::Muted,
            )],
            preview_lines: 1,
        })
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

    #[test]
    fn lsp_name_and_schema() {
        let tool = LspTool;
        assert_eq!(tool.name(), "LSP");
        let schema = tool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "operation"));
        assert!(required.iter().any(|v| v == "file_path"));
        assert!(required.iter().any(|v| v == "line"));
        assert!(required.iter().any(|v| v == "column"));
    }

    #[test]
    fn lsp_is_read_only() {
        assert!(LspTool.is_read_only());
    }

    #[tokio::test]
    async fn lsp_go_to_definition_stub() {
        let tool = LspTool;
        let input = serde_json::json!({
            "operation": "goToDefinition",
            "file_path": "/src/main.rs",
            "line": 10,
            "column": 5
        });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(!out.is_error);
        assert!(out.text().contains("goToDefinition stub"));
        assert!(out.text().contains("/src/main.rs:10:5"));
    }

    #[tokio::test]
    async fn lsp_find_references_stub() {
        let tool = LspTool;
        let input = serde_json::json!({
            "operation": "findReferences",
            "file_path": "/src/lib.rs",
            "line": 20,
            "column": 3
        });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(!out.is_error);
        assert!(out.text().contains("findReferences stub"));
    }

    #[tokio::test]
    async fn lsp_hover_stub() {
        let tool = LspTool;
        let input = serde_json::json!({
            "operation": "hover",
            "file_path": "/src/lib.rs",
            "line": 1,
            "column": 1
        });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(!out.is_error);
        assert!(out.text().contains("hover stub"));
    }

    #[tokio::test]
    async fn lsp_unknown_operation_is_error() {
        let tool = LspTool;
        let input = serde_json::json!({
            "operation": "unknownOp",
            "file_path": "/src/lib.rs",
            "line": 1,
            "column": 1
        });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(out.is_error);
        assert!(out.text().contains("Unknown LSP operation"));
    }

    #[tokio::test]
    async fn lsp_missing_file_path() {
        let tool = LspTool;
        let input = serde_json::json!({
            "operation": "hover",
            "file_path": "",
            "line": 1,
            "column": 1
        });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn lsp_zero_line_is_error() {
        let tool = LspTool;
        let input = serde_json::json!({
            "operation": "hover",
            "file_path": "/src/main.rs",
            "line": 0,
            "column": 1
        });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(out.is_error);
        assert!(out.text().contains("positive integers"));
    }
}
