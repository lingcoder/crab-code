//! LSP (Language Server Protocol) tool.
//!
//! Spawns language server processes (rust-analyzer, typescript-language-server,
//! pyright) and communicates over stdin/stdout JSON-RPC to perform IDE-like
//! operations: go-to-definition, find-references, hover, diagnostics, rename,
//! and code actions.

use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex as TokioMutex, oneshot};

use crab_core::Result;
use crab_core::tool::{CollapsedGroupLabel, Tool, ToolContext, ToolDisplayResult, ToolOutput};
use serde_json::Value;

/// LSP operations tool.
pub const LSP_TOOL_NAME: &str = "LSP";

/// Language server specification derived from file extension.
#[derive(Debug)]
struct LangServerSpec {
    command: &'static str,
    args: &'static [&'static str],
    language_id: &'static str,
    #[allow(dead_code)]
    label: &'static str,
}

/// Map file extension to a language server specification.
fn lang_server_for_file(file_path: &str) -> Result<LangServerSpec> {
    let ext = Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    match ext {
        "rs" => Ok(LangServerSpec {
            command: "rust-analyzer",
            args: &[],
            language_id: "rust",
            label: "rust-analyzer",
        }),
        "ts" | "tsx" | "mts" | "cts" => Ok(LangServerSpec {
            command: "typescript-language-server",
            args: &["--stdio"],
            language_id: "typescript",
            label: "typescript-language-server",
        }),
        "js" | "jsx" | "mjs" | "cjs" => Ok(LangServerSpec {
            command: "typescript-language-server",
            args: &["--stdio"],
            language_id: "javascript",
            label: "typescript-language-server",
        }),
        "py" | "pyi" => Ok(LangServerSpec {
            command: "pyright-langserver",
            args: &["--stdio"],
            language_id: "python",
            label: "pyright",
        }),
        other => Err(crab_core::Error::Tool(format!(
            "No language server available for file extension '.{other}'. \
             Supported: .rs, .ts, .tsx, .js, .jsx, .py"
        ))),
    }
}

/// A connected language server process with stdin/stdout communication.
struct LspClient {
    writer: Arc<TokioMutex<tokio::process::ChildStdin>>,
    pending: Arc<TokioMutex<HashMap<u64, oneshot::Sender<Value>>>>,
    diagnostics: Arc<TokioMutex<HashMap<String, Vec<Value>>>>,
    reader_handle: tokio::task::JoinHandle<()>,
    child: Arc<TokioMutex<Child>>,
    next_id: AtomicU64,
}

impl LspClient {
    /// Spawn a language server process and connect to it.
    async fn spawn(command: &str, args: &[&str]) -> Result<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());

        let mut child = cmd.spawn().map_err(|e| {
            crab_core::Error::Tool(format!(
                "Failed to spawn language server '{command}': {e}. \
                 Make sure it is installed and available on PATH."
            ))
        })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            crab_core::Error::Tool("failed to capture language server stdin".into())
        })?;

        let stdout = child.stdout.take().ok_or_else(|| {
            crab_core::Error::Tool("failed to capture language server stdout".into())
        })?;

        let pending: Arc<TokioMutex<HashMap<u64, oneshot::Sender<Value>>>> =
            Arc::new(TokioMutex::new(HashMap::new()));
        let diagnostics: Arc<TokioMutex<HashMap<String, Vec<Value>>>> =
            Arc::new(TokioMutex::new(HashMap::new()));

        let pending_clone = Arc::clone(&pending);
        let diagnostics_clone = Arc::clone(&diagnostics);

        let reader_handle = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_lsp_message(&mut reader).await {
                    Ok(Some(body)) => {
                        if let Ok(val) = serde_json::from_str::<Value>(&body) {
                            // Response: has "id" field — send it to the waiting caller.
                            if let Some(id) = val.get("id").and_then(Value::as_u64) {
                                let Some(tx) = pending_clone.lock().await.remove(&id) else {
                                    continue;
                                };
                                let _ = tx.send(val);
                                continue;
                            }
                            // Notification: has "method" field (no "id").
                            if val.get("method").and_then(Value::as_str)
                                == Some("textDocument/publishDiagnostics")
                                && let Some(params) = val.get("params")
                            {
                                let uri = params
                                    .get("uri")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_owned();
                                let diags = params
                                    .get("diagnostics")
                                    .and_then(|v| v.as_array().cloned())
                                    .unwrap_or_default();
                                diagnostics_clone.lock().await.insert(uri, diags);
                            }
                        }
                    }
                    Ok(None) => {
                        tracing::debug!("language server stdout closed");
                        break;
                    }
                    Err(e) => {
                        tracing::warn!("error reading from language server: {e}");
                        break;
                    }
                }
            }
        });

        Ok(Self {
            writer: Arc::new(TokioMutex::new(stdin)),
            pending,
            diagnostics,
            reader_handle,
            child: Arc::new(TokioMutex::new(child)),
            next_id: AtomicU64::new(1),
        })
    }

    /// Write a `Content-Length` framed JSON-RPC message to stdin.
    async fn write_message(&self, json: &str) -> Result<()> {
        let frame = format!("Content-Length: {}\r\n\r\n{}", json.len(), json);
        {
            let mut writer = self.writer.lock().await;
            writer.write_all(frame.as_bytes()).await.map_err(|e| {
                crab_core::Error::Tool(format!("failed to write to language server: {e}"))
            })?;
            writer.flush().await.map_err(|e| {
                crab_core::Error::Tool(format!("failed to flush language server stdin: {e}"))
            })?;
            drop(writer);
        }
        Ok(())
    }

    /// Send a JSON-RPC request and wait for the response.
    async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending.lock().await;
            map.insert(id, tx);
        }

        let json = serde_json::to_string(&msg)
            .map_err(|e| crab_core::Error::Tool(format!("failed to serialize request: {e}")))?;

        tracing::debug!(method, id, "sending LSP request");
        self.write_message(&json).await?;

        // Wait with a timeout to avoid hanging forever.
        match tokio::time::timeout(std::time::Duration::from_secs(30), rx).await {
            Ok(Ok(val)) => Ok(val),
            Ok(Err(_)) => Err(crab_core::Error::Tool(
                "language server closed connection before responding".into(),
            )),
            Err(_) => {
                // Clean up the pending entry.
                self.pending.lock().await.remove(&id);
                Err(crab_core::Error::Tool(
                    "language server did not respond within 30 seconds".into(),
                ))
            }
        }
    }

    /// Send a JSON-RPC notification (no response expected).
    async fn notify(&self, method: &str, params: Value) -> Result<()> {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let json = serde_json::to_string(&msg).map_err(|e| {
            crab_core::Error::Tool(format!("failed to serialize notification: {e}"))
        })?;
        tracing::debug!(method, "sending LSP notification");
        self.write_message(&json).await
    }

    /// Send `initialize` request and wait for the server capabilities.
    async fn initialize(&self) -> Result<Value> {
        let params = serde_json::json!({
            "processId": std::process::id(),
            "capabilities": {
                "textDocument": {
                    "definition": { "dynamicRegistration": false },
                    "references": { "dynamicRegistration": false },
                    "hover": { "dynamicRegistration": false, "contentFormat": ["markdown", "plaintext"] },
                    "rename": { "dynamicRegistration": false, "prepareSupport": false },
                    "codeAction": { "dynamicRegistration": false, "codeActionLiteralSupport": {
                        "codeActionKind": { "valueSet": [] }
                    }},
                    "synchronization": { "dynamicRegistration": false, "didSave": false }
                }
            },
            "rootUri": null
        });

        let resp = self.request("initialize", params).await?;

        // Send initialized notification.
        self.notify("initialized", serde_json::json!({})).await?;

        Ok(resp)
    }

    /// Open a file in the language server.
    async fn did_open(&self, file_path: &str, language_id: &str) -> Result<()> {
        let content = std::fs::read_to_string(file_path).map_err(|e| {
            crab_core::Error::Tool(format!("failed to read file '{file_path}': {e}"))
        })?;

        let uri = path_to_uri(file_path);
        self.notify(
            "textDocument/didOpen",
            serde_json::json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": 1,
                    "text": content
                }
            }),
        )
        .await
    }

    /// Notify the server about file content changes (full sync).
    async fn did_change(&self, file_path: &str) -> Result<()> {
        let content = std::fs::read_to_string(file_path).map_err(|e| {
            crab_core::Error::Tool(format!("failed to read file '{file_path}': {e}"))
        })?;

        let uri = path_to_uri(file_path);
        self.notify(
            "textDocument/didChange",
            serde_json::json!({
                "textDocument": { "uri": uri, "version": 2 },
                "contentChanges": [{ "text": content }]
            }),
        )
        .await
    }

    /// Shut down the language server gracefully.
    async fn shutdown(&self) {
        // Send shutdown request (best effort).
        let _ = self.request("shutdown", Value::Null).await;
        // Send exit notification.
        let _ = self.notify("exit", Value::Null).await;
        // Kill the process if still alive.
        let _ = self.child.lock().await.kill().await;
        self.reader_handle.abort();
    }
}

/// Global cache of language server clients, keyed by server command name.
static LSP_CLIENTS: LazyLock<TokioMutex<HashMap<String, Arc<LspClient>>>> =
    LazyLock::new(|| TokioMutex::new(HashMap::new()));

/// Get or spawn a language server client for the given file.
async fn get_or_spawn_client(file_path: &str) -> Result<(Arc<LspClient>, &'static str)> {
    let spec = lang_server_for_file(file_path)?;
    let key = spec.command.to_owned();

    // Fast path: check if client already exists.
    {
        let map = LSP_CLIENTS.lock().await;
        if let Some(client) = map.get(&key) {
            return Ok((Arc::clone(client), spec.language_id));
        }
    }

    // Slow path: spawn a new client.
    let client = Arc::new(LspClient::spawn(spec.command, spec.args).await?);
    client.initialize().await?;

    {
        let mut map = LSP_CLIENTS.lock().await;

        // Double-check in case another task inserted while we were spawning.
        if let Some(existing) = map.get(&key) {
            let dup = Arc::clone(existing);
            // Drop the lock before shutting down the duplicate client.
            drop(map);
            client.shutdown().await;
            return Ok((dup, spec.language_id));
        }

        map.insert(key, Arc::clone(&client));
    }
    Ok((client, spec.language_id))
}

/// Convert a filesystem path to a `file://` URI.
fn path_to_uri(path: &str) -> String {
    // Normalize Windows backslashes and handle drive letters.
    let normalized = path.replace('\\', "/");
    if cfg!(windows) && normalized.len() >= 2 && normalized.as_bytes()[1] == b':' {
        // Drive letter path: "C:/foo" -> "file:///C:/foo"
        format!("file:///{normalized}")
    } else if normalized.starts_with('/') {
        format!("file://{normalized}")
    } else {
        // Relative path — make it absolute-ish for the URI.
        format!("file:///{normalized}")
    }
}

/// Strip a `file://` URI scheme and return the filesystem path.
fn uri_to_path(uri: &str) -> &str {
    // file:///absolute/path -> /absolute/path (strip "file://", keep the leading /)
    // file://host/path      -> /path          (strip "file://host")
    // file://C:/Windows     -> C:/Windows     (strip "file://")
    if let Some(rest) = uri.strip_prefix("file://") {
        // On Windows: file:///C:/foo -> rest is "/C:/foo", strip leading /
        // On Unix: file:///home     -> rest is "/home", keep it
        if cfg!(windows) {
            // Windows URIs: file:///C:/path — rest is "/C:/path", strip the leading /
            if rest.len() >= 3 && rest.as_bytes()[0] == b'/' && rest.as_bytes()[2] == b':' {
                return &rest[1..];
            }
        }
        rest
    } else {
        uri
    }
}

/// Extract a human-readable location string from an LSP Location object.
fn format_location(loc: &Value) -> Option<String> {
    let uri = loc.get("uri")?.as_str()?;
    let range = loc.get("range")?;
    let start = range.get("start")?;
    let line = start.get("line")?.as_u64()? + 1; // LSP is 0-based
    let col = start.get("character")?.as_u64()? + 1;

    let path = uri_to_path(uri);
    let path = urlencoding::decode(path).unwrap_or_else(|_| path.into());
    let path = path.as_ref();

    Some(format!("{path}:{line}:{col}"))
}

/// Extract markdown or plaintext content from an LSP `MarkedString` / `MarkupContent`.
fn extract_hover_content(content: &Value) -> String {
    // MarkedString: { language: "rust", value: "..." } — check before MarkupContent
    // because both have a "value" field but MarkedString also has "language".
    if let (Some(lang), Some(val)) = (
        content.get("language").and_then(|v| v.as_str()),
        content.get("value").and_then(|v| v.as_str()),
    ) {
        return format!("```{lang}\n{val}\n```");
    }
    // MarkupContent: { kind: "markdown"|"plaintext", value: "..." }
    if let Some(value) = content.get("value").and_then(|v| v.as_str()) {
        return value.to_owned();
    }
    // Plain string
    if let Some(s) = content.as_str() {
        return s.to_owned();
    }
    serde_json::to_string_pretty(content).unwrap_or_default()
}

pub struct LspTool;

impl Tool for LspTool {
    fn name(&self) -> &'static str {
        LSP_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Perform LSP operations on source files: go-to-definition, find-references, \
         hover, diagnostics, rename, and code actions. Spawns a language server \
         automatically based on file extension (.rs -> rust-analyzer, .ts/.js -> \
         typescript-language-server, .py -> pyright)."
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
        let new_name = input["new_name"].as_str().unwrap_or("").to_owned();
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
            // Validate rename-specific input before spawning a client.
            if operation == "rename" && new_name.is_empty() {
                return Ok(ToolOutput::error(
                    "new_name is required for rename operation",
                ));
            }

            // Convert 1-based input to 0-based LSP coordinates.
            let lsp_line = line - 1;
            let lsp_char = column - 1;

            let (client, language_id) = match get_or_spawn_client(&file_path).await {
                Ok(pair) => pair,
                Err(e) => return Ok(ToolOutput::error(format!("{e}"))),
            };

            // Open the file (idempotent — didOpen again is fine for freshness).
            if let Err(e) = client.did_open(&file_path, language_id).await {
                return Ok(ToolOutput::error(format!("Failed to open file: {e}")));
            }
            // Sync latest content.
            if let Err(e) = client.did_change(&file_path).await {
                return Ok(ToolOutput::error(format!("Failed to sync file: {e}")));
            }

            let uri = path_to_uri(&file_path);

            let result = match operation.as_str() {
                "goToDefinition" => {
                    let params = serde_json::json!({
                        "textDocument": { "uri": uri },
                        "position": { "line": lsp_line, "character": lsp_char }
                    });
                    let resp = client.request("textDocument/definition", params).await?;
                    format_definition_response(&resp)
                }
                "findReferences" => {
                    let params = serde_json::json!({
                        "textDocument": { "uri": uri },
                        "position": { "line": lsp_line, "character": lsp_char },
                        "context": { "includeDeclaration": true }
                    });
                    let resp = client.request("textDocument/references", params).await?;
                    format_references_response(&resp)
                }
                "hover" => {
                    let params = serde_json::json!({
                        "textDocument": { "uri": uri },
                        "position": { "line": lsp_line, "character": lsp_char }
                    });
                    let resp = client.request("textDocument/hover", params).await?;
                    format_hover_response(&resp)
                }
                "diagnostics" => {
                    // Wait briefly for diagnostics to arrive after the didChange.
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    let diags = client.diagnostics.lock().await;
                    let file_uri = path_to_uri(&file_path);
                    format_diagnostics(&diags, &file_uri)
                }
                "rename" => {
                    let params = serde_json::json!({
                        "textDocument": { "uri": uri },
                        "position": { "line": lsp_line, "character": lsp_char },
                        "newName": new_name
                    });
                    let resp = client.request("textDocument/rename", params).await?;
                    format_rename_response(&resp)
                }
                "codeAction" => {
                    let params = serde_json::json!({
                        "textDocument": { "uri": uri },
                        "range": {
                            "start": { "line": lsp_line, "character": lsp_char },
                            "end": { "line": lsp_line, "character": lsp_char }
                        },
                        "context": { "diagnostics": [] }
                    });
                    let resp = client.request("textDocument/codeAction", params).await?;
                    format_code_action_response(&resp)
                }
                other => {
                    return Ok(ToolOutput::error(format!(
                        "Unknown LSP operation: {other}. \
                         Supported: goToDefinition, findReferences, hover, diagnostics, rename, codeAction"
                    )));
                }
            };

            Ok(result)
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        let op = input["operation"].as_str()?;
        let file = input["file_path"].as_str().unwrap_or("?");
        let filename = file.rsplit(['/', '\\']).next().unwrap_or(file);
        Some(format!("LSP (operation: \"{op}\", in: \"{filename}\")"))
    }

    fn format_result(&self, output: &ToolOutput) -> Option<ToolDisplayResult> {
        use crab_core::tool::{ToolDisplayLine, ToolDisplayStyle};
        let text = output.text();
        if text.is_empty() {
            return None;
        }
        let line_count = text.lines().count();
        Some(ToolDisplayResult {
            lines: vec![ToolDisplayLine::new(
                format!("Found {line_count} results"),
                ToolDisplayStyle::Muted,
            )],
            preview_lines: 1,
        })
    }

    fn collapsed_group_label(&self) -> Option<CollapsedGroupLabel> {
        Some(CollapsedGroupLabel {
            active_verb: "Querying",
            past_verb: "Queried",
            noun_singular: "symbol",
            noun_plural: "symbols",
        })
    }
}

// ─── Response Formatting ─────────────────────────────────────────────────────

/// Format a `textDocument/definition` response.
fn format_definition_response(resp: &Value) -> ToolOutput {
    let result = match resp.get("result") {
        Some(r) if !r.is_null() => r,
        _ => return ToolOutput::success("No definition found."),
    };

    // Result can be a single Location, an array of Locations, or an array of LocationLinks.
    let locations: Vec<String> = if result.is_array() {
        result
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|loc| {
                // Try Location format first, then LocationLink.
                loc.get("uri")
                    .and_then(|_| format_location(loc))
                    .or_else(|| loc.get("targetUri").and_then(|_| format_location_link(loc)))
            })
            .collect()
    } else if result.is_object() {
        result
            .get("uri")
            .and_then(|_| format_location(result))
            .into_iter()
            .collect()
    } else {
        vec![]
    };

    if locations.is_empty() {
        ToolOutput::success("No definition found.")
    } else {
        ToolOutput::success(locations.join("\n"))
    }
}

/// Format a `textDocument/references` response.
fn format_references_response(resp: &Value) -> ToolOutput {
    let result = match resp.get("result") {
        Some(r) if r.is_array() => r.as_array().unwrap(),
        _ => return ToolOutput::success("No references found."),
    };

    let locations: Vec<String> = result.iter().filter_map(format_location).collect();

    if locations.is_empty() {
        ToolOutput::success("No references found.")
    } else {
        ToolOutput::success(locations.join("\n"))
    }
}

/// Format a `textDocument/hover` response.
fn format_hover_response(resp: &Value) -> ToolOutput {
    let result = match resp.get("result") {
        Some(r) if !r.is_null() => r,
        _ => return ToolOutput::success("No hover information available."),
    };

    let content = result.get("contents");
    let text = match content {
        Some(c) if c.is_array() => {
            // Array of MarkedStrings
            let parts: Vec<String> = c
                .as_array()
                .unwrap()
                .iter()
                .map(extract_hover_content)
                .collect();
            parts.join("\n\n")
        }
        Some(c) => extract_hover_content(c),
        None => extract_hover_content(result),
    };

    if text.is_empty() {
        ToolOutput::success("No hover information available.")
    } else {
        ToolOutput::success(text)
    }
}

/// Format collected diagnostics for a file.
fn format_diagnostics(diagnostics: &HashMap<String, Vec<Value>>, file_uri: &str) -> ToolOutput {
    let diags = match diagnostics.get(file_uri) {
        Some(d) if !d.is_empty() => d,
        _ => return ToolOutput::success("No diagnostics for this file."),
    };

    let mut lines = Vec::new();
    for diag in diags {
        let severity =
            diag.get("severity")
                .and_then(Value::as_u64)
                .map_or("unknown", |s| match s {
                    1 => "error",
                    2 => "warning",
                    3 => "info",
                    4 => "hint",
                    _ => "unknown",
                });

        let msg = diag
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("<no message>");

        let range = diag.get("range").and_then(|r| r.get("start"));
        let loc = range
            .map(|start| {
                let l = start.get("line").and_then(Value::as_u64).unwrap_or(0) + 1;
                let c = start.get("character").and_then(Value::as_u64).unwrap_or(0) + 1;
                format!(":{l}:{c}")
            })
            .unwrap_or_default();

        lines.push(format!("[{severity}{loc}] {msg}"));
    }

    if lines.is_empty() {
        ToolOutput::success("No diagnostics for this file.")
    } else {
        ToolOutput::success(lines.join("\n"))
    }
}

/// Format a `textDocument/rename` response (`WorkspaceEdit`).
fn format_rename_response(resp: &Value) -> ToolOutput {
    let result = match resp.get("result") {
        Some(r) if !r.is_null() => r,
        _ => return ToolOutput::success("Rename returned no changes."),
    };

    // WorkspaceEdit with changes: { "changes": { "uri": [edits] } }
    if let Some(changes) = result.get("changes").and_then(|c| c.as_object()) {
        let mut lines = Vec::new();
        for (uri, edits) in changes {
            let path = uri_to_path(uri);
            let path = urlencoding::decode(path).unwrap_or_else(|_| path.into());
            let edit_count = edits.as_array().map_or(0, Vec::len);
            lines.push(format!("  {path}: {edit_count} edit(s)"));
        }
        if lines.is_empty() {
            ToolOutput::success("Rename returned no changes.")
        } else {
            let total = lines.len();
            let mut output = vec![format!("Rename affects {total} file(s):")];
            output.extend(lines);
            ToolOutput::success(output.join("\n"))
        }
    } else {
        ToolOutput::success(format!(
            "Rename response:\n{}",
            serde_json::to_string_pretty(result).unwrap_or_default()
        ))
    }
}

/// Format a `textDocument/codeAction` response.
fn format_code_action_response(resp: &Value) -> ToolOutput {
    let result = match resp.get("result") {
        Some(r) if r.is_array() => r.as_array().unwrap(),
        _ => return ToolOutput::success("No code actions available."),
    };

    let mut lines = Vec::new();
    for (i, action) in result.iter().enumerate() {
        let title = action
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("<untitled>");
        let kind = action
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        lines.push(format!("{}. [{kind}] {title}", i + 1));
    }

    if lines.is_empty() {
        ToolOutput::success("No code actions available.")
    } else {
        ToolOutput::success(lines.join("\n"))
    }
}

/// Format a `LocationLink` (used by some servers instead of `Location`).
fn format_location_link(link: &Value) -> Option<String> {
    let uri = link.get("targetUri")?.as_str()?;
    let range = link
        .get("targetSelectionRange")
        .or_else(|| link.get("targetRange"))?;
    let start = range.get("start")?;
    let ln = start.get("line")?.as_u64()? + 1;
    let col = start.get("character")?.as_u64()? + 1;

    let path = uri_to_path(uri);
    let path = urlencoding::decode(path).unwrap_or_else(|_| path.into());
    Some(format!("{}:{ln}:{col}", path.as_ref()))
}

/// Read a single `Content-Length` framed LSP message from an async reader.
async fn read_lsp_message<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
) -> Result<Option<String>> {
    let mut content_length: Option<usize> = None;
    let mut header_line = String::new();

    loop {
        header_line.clear();
        let bytes_read = reader
            .read_line(&mut header_line)
            .await
            .map_err(|e| crab_core::Error::Tool(format!("failed to read LSP header: {e}")))?;

        if bytes_read == 0 {
            return Ok(None);
        }

        let trimmed = header_line.trim();
        if trimmed.is_empty() {
            break;
        }

        if let Some(value) = trimmed.strip_prefix("Content-Length:") {
            content_length = value.trim().parse().ok();
        }
    }

    let length = content_length.ok_or_else(|| {
        crab_core::Error::Tool("missing Content-Length header in LSP message".into())
    })?;

    let mut body = vec![0u8; length];
    reader
        .read_exact(&mut body)
        .await
        .map_err(|e| crab_core::Error::Tool(format!("failed to read LSP message body: {e}")))?;

    String::from_utf8(body)
        .map(Some)
        .map_err(|e| crab_core::Error::Tool(format!("invalid UTF-8 in LSP message: {e}")))
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
            task_registry: None,
            nested_memory_triggers: Arc::new(tokio::sync::Mutex::new(
                std::collections::HashSet::new(),
            )),
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

    #[test]
    fn lang_server_for_rust() {
        let spec = lang_server_for_file("src/main.rs").unwrap();
        assert_eq!(spec.command, "rust-analyzer");
        assert_eq!(spec.language_id, "rust");
    }

    #[test]
    fn lang_server_for_typescript() {
        let spec = lang_server_for_file("app.ts").unwrap();
        assert_eq!(spec.command, "typescript-language-server");
        assert_eq!(spec.language_id, "typescript");
    }

    #[test]
    fn lang_server_for_javascript() {
        let spec = lang_server_for_file("index.js").unwrap();
        assert_eq!(spec.command, "typescript-language-server");
        assert_eq!(spec.language_id, "javascript");
    }

    #[test]
    fn lang_server_for_python() {
        let spec = lang_server_for_file("main.py").unwrap();
        assert_eq!(spec.command, "pyright-langserver");
        assert_eq!(spec.language_id, "python");
    }

    #[test]
    fn lang_server_for_unsupported() {
        let err = lang_server_for_file("readme.md").unwrap_err();
        assert!(format!("{err}").contains("No language server"));
    }

    #[test]
    fn path_to_uri_unix_style() {
        let uri = path_to_uri("/home/user/project/main.rs");
        assert_eq!(uri, "file:///home/user/project/main.rs");
    }

    #[test]
    fn path_to_uri_windows_style() {
        let uri = path_to_uri("C:\\Users\\test\\main.rs");
        assert_eq!(uri, "file:///C:/Users/test/main.rs");
    }

    #[test]
    fn format_location_basic() {
        let loc = serde_json::json!({
            "uri": "file:///src/main.rs",
            "range": {
                "start": { "line": 9, "character": 4 },
                "end": { "line": 9, "character": 10 }
            }
        });
        assert_eq!(format_location(&loc).unwrap(), "/src/main.rs:10:5");
    }

    #[test]
    fn format_location_link_basic() {
        let link = serde_json::json!({
            "targetUri": "file:///src/lib.rs",
            "targetSelectionRange": {
                "start": { "line": 0, "character": 0 },
                "end": { "line": 0, "character": 5 }
            }
        });
        assert_eq!(format_location_link(&link).unwrap(), "/src/lib.rs:1:1");
    }

    #[test]
    fn extract_hover_markdown() {
        let content = serde_json::json!({
            "kind": "markdown",
            "value": "fn main() -> ()"
        });
        assert_eq!(extract_hover_content(&content), "fn main() -> ()");
    }

    #[test]
    fn extract_hover_marked_string() {
        let content = serde_json::json!({
            "language": "rust",
            "value": "fn main()"
        });
        assert_eq!(extract_hover_content(&content), "```rust\nfn main()\n```");
    }

    #[test]
    fn extract_hover_plain_string() {
        let content = serde_json::json!("just text");
        assert_eq!(extract_hover_content(&content), "just text");
    }

    #[test]
    fn format_definition_no_result() {
        let resp = serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": null});
        let out = format_definition_response(&resp);
        assert_eq!(out.text(), "No definition found.");
    }

    #[test]
    fn format_definition_single_location() {
        let resp = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "uri": "file:///src/lib.rs",
                "range": { "start": { "line": 5, "character": 0 }, "end": { "line": 5, "character": 10 } }
            }
        });
        let out = format_definition_response(&resp);
        assert!(out.text().contains("/src/lib.rs:6:1"));
    }

    #[test]
    fn format_definition_multiple_locations() {
        let resp = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": [
                { "uri": "file:///a.rs", "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 5 } } },
                { "uri": "file:///b.rs", "range": { "start": { "line": 10, "character": 4 }, "end": { "line": 10, "character": 9 } } }
            ]
        });
        let out = format_definition_response(&resp);
        assert!(out.text().contains("/a.rs:1:1"));
        assert!(out.text().contains("/b.rs:11:5"));
    }

    #[test]
    fn format_references_empty() {
        let resp = serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": []});
        let out = format_references_response(&resp);
        assert_eq!(out.text(), "No references found.");
    }

    #[test]
    fn format_hover_no_result() {
        let resp = serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": null});
        let out = format_hover_response(&resp);
        assert_eq!(out.text(), "No hover information available.");
    }

    #[test]
    fn format_diagnostics_empty() {
        let diags = HashMap::new();
        let out = format_diagnostics(&diags, "file:///test.rs");
        assert_eq!(out.text(), "No diagnostics for this file.");
    }

    #[test]
    fn format_diagnostics_with_errors() {
        let mut diags = HashMap::new();
        diags.insert(
            "file:///test.rs".to_owned(),
            vec![serde_json::json!({
                "range": { "start": { "line": 4, "character": 0 }, "end": { "line": 4, "character": 10 } },
                "severity": 1,
                "message": "cannot find value `x`"
            })],
        );
        let out = format_diagnostics(&diags, "file:///test.rs");
        assert!(out.text().contains("error"));
        assert!(out.text().contains("cannot find value `x`"));
        assert!(out.text().contains(":5:1"));
    }

    #[test]
    fn format_rename_no_changes() {
        let resp = serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": null});
        let out = format_rename_response(&resp);
        assert_eq!(out.text(), "Rename returned no changes.");
    }

    #[test]
    fn format_rename_with_changes() {
        let resp = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "changes": {
                    "file:///src/main.rs": [
                        { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 5 } }, "newText": "world" }
                    ],
                    "file:///src/lib.rs": [
                        { "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 5 } }, "newText": "world" }
                    ]
                }
            }
        });
        let out = format_rename_response(&resp);
        assert!(out.text().contains("2 file(s)"));
    }

    #[test]
    fn format_code_action_empty() {
        let resp = serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": []});
        let out = format_code_action_response(&resp);
        assert_eq!(out.text(), "No code actions available.");
    }

    #[test]
    fn format_code_action_with_actions() {
        let resp = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": [
                { "title": "Extract variable", "kind": "refactor.extract" },
                { "title": "Inline variable", "kind": "refactor.inline" }
            ]
        });
        let out = format_code_action_response(&resp);
        assert!(out.text().contains("Extract variable"));
        assert!(out.text().contains("refactor.extract"));
        assert!(out.text().contains("Inline variable"));
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

    #[tokio::test]
    async fn lsp_unknown_operation_is_error() {
        let tool = LspTool;
        let input = serde_json::json!({
            "operation": "unknownOp",
            "file_path": "/src/lib.txt",
            "line": 1,
            "column": 1
        });
        // Unsupported extension returns error before reaching the operation check.
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn lsp_unsupported_extension_is_error() {
        let tool = LspTool;
        let input = serde_json::json!({
            "operation": "hover",
            "file_path": "/src/readme.md",
            "line": 1,
            "column": 1
        });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(out.is_error);
        assert!(out.text().contains("No language server"));
    }

    #[test]
    fn format_use_summary_works() {
        let tool = LspTool;
        let input = serde_json::json!({
            "operation": "hover",
            "file_path": "/home/user/project/src/main.rs",
            "line": 1,
            "column": 1
        });
        let summary = tool.format_use_summary(&input).unwrap();
        assert!(summary.contains("hover"));
        assert!(summary.contains("main.rs"));
    }

    #[tokio::test]
    async fn lsp_rename_requires_new_name() {
        let tool = LspTool;
        let input = serde_json::json!({
            "operation": "rename",
            "file_path": "/src/main.rs",
            "line": 1,
            "column": 1,
            "new_name": ""
        });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(out.is_error);
        assert!(out.text().contains("new_name"));
    }

    #[tokio::test]
    async fn read_lsp_message_parses_content_length() {
        let data = b"Content-Length: 17\r\n\r\n{\"jsonrpc\":\"2.0\"}";
        let mut reader = BufReader::new(&data[..]);
        let msg = read_lsp_message(&mut reader).await.unwrap().unwrap();
        assert_eq!(msg, "{\"jsonrpc\":\"2.0\"}");
    }

    #[tokio::test]
    async fn read_lsp_message_returns_none_on_eof() {
        let data = b"";
        let mut reader = BufReader::new(&data[..]);
        let msg = read_lsp_message(&mut reader).await.unwrap();
        assert!(msg.is_none());
    }
}
