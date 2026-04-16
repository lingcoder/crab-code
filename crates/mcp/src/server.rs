//! MCP server — exposes local tools to external MCP clients.
//!
//! The server reads JSON-RPC requests (Content-Length framed, like LSP) from an
//! async reader (typically stdin) and writes responses to an async writer
//! (typically stdout). It handles the MCP handshake (`initialize`) and serves
//! `tools/list` and `tools/call` by delegating to a [`ToolHandler`].
//!
//! Also supports HTTP SSE mode via [`McpServer::run_sse`], where the server
//! listens on a TCP port and serves SSE streams to multiple concurrent clients.

use std::sync::Arc;

use crab_core::permission::{PermissionMode, PermissionPolicy};
use crab_core::tool::{Tool, ToolContext, ToolOutputContent};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::protocol::{
    InitializeResult, JsonRpcError, JsonRpcRequest, JsonRpcResponse, MCP_PROTOCOL_VERSION,
    McpPrompt, McpResource, McpToolDef, PromptGetParams, PromptGetResult, ResourceContent,
    ResourceReadParams, ResourceReadResult, ServerCapabilities, ServerInfo, ToolCallParams,
    ToolCallResult, ToolResultContent, method,
};

// ─── ToolHandler trait ───

/// Trait for handling tool calls within the MCP server.
///
/// Implementations bridge from the MCP server to the local tool system.
/// A default implementation is provided for `Vec<Arc<dyn Tool>>`.
pub trait ToolHandler: Send + Sync {
    /// List the available tools as MCP tool definitions.
    fn list_tools(&self) -> Vec<McpToolDef>;

    /// Call a tool by name with the given JSON arguments.
    ///
    /// Returns a `ToolCallResult` suitable for JSON-RPC response.
    fn call_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolCallResult> + Send + '_>>;
}

// ─── ResourceHandler trait ───

/// Trait for handling MCP resource requests.
///
/// Implementations expose data (files, database records, API responses, etc.)
/// as MCP resources that clients can list and read.
pub trait ResourceHandler: Send + Sync {
    /// List available resources.
    fn list_resources(&self) -> Vec<McpResource>;

    /// Read a resource by URI.
    ///
    /// Returns the resource contents, or an error message if the URI is unknown
    /// or the resource cannot be read.
    fn read_resource(
        &self,
        uri: &str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ResourceReadResult, String>> + Send + '_>,
    >;
}

// ─── PromptHandler trait ───

/// Trait for handling MCP prompt requests.
///
/// Implementations expose prompt templates that clients can list and invoke
/// with arguments. Each prompt returns a list of messages suitable for LLM input.
pub trait PromptHandler: Send + Sync {
    /// List available prompts.
    fn list_prompts(&self) -> Vec<McpPrompt>;

    /// Get a prompt by name, substituting the given arguments.
    fn get_prompt(
        &self,
        name: &str,
        arguments: &std::collections::HashMap<String, String>,
    ) -> Result<PromptGetResult, String>;
}

// ─── McpServer ───

/// MCP server that reads JSON-RPC from a reader and writes responses to a writer.
pub struct McpServer<H: ToolHandler> {
    server_info: ServerInfo,
    handler: Arc<H>,
    resource_handler: Option<Arc<dyn ResourceHandler>>,
    prompt_handler: Option<Arc<dyn PromptHandler>>,
}

impl<H: ToolHandler + 'static> McpServer<H> {
    /// Create a new MCP server with the given tool handler.
    pub fn new(name: impl Into<String>, version: impl Into<String>, handler: Arc<H>) -> Self {
        Self {
            server_info: ServerInfo {
                name: name.into(),
                version: version.into(),
            },
            handler,
            resource_handler: None,
            prompt_handler: None,
        }
    }

    /// Attach a resource handler to this server.
    ///
    /// When set, the server will advertise `resources` capability during
    /// initialization and handle `resources/list` and `resources/read` requests.
    #[must_use]
    pub fn with_resource_handler(mut self, handler: Arc<dyn ResourceHandler>) -> Self {
        self.resource_handler = Some(handler);
        self
    }

    /// Attach a prompt handler to this server.
    #[must_use]
    pub fn with_prompt_handler(mut self, handler: Arc<dyn PromptHandler>) -> Self {
        self.prompt_handler = Some(handler);
        self
    }

    /// Run the server, reading from `reader` and writing to `writer`.
    ///
    /// This is a long-running loop that processes requests until EOF or error.
    /// For stdio usage, pass `tokio::io::stdin()` and `tokio::io::stdout()`.
    pub async fn run<R, W>(&self, reader: R, writer: W) -> crab_common::Result<()>
    where
        R: tokio::io::AsyncRead + Unpin + Send,
        W: tokio::io::AsyncWrite + Unpin + Send,
    {
        let mut buf_reader = BufReader::new(reader);
        let writer = Arc::new(Mutex::new(writer));

        loop {
            match read_message(&mut buf_reader).await? {
                None => {
                    tracing::debug!("MCP server: stdin closed, shutting down");
                    break;
                }
                Some(data) => {
                    // Try to parse as a request (has "id" + "method").
                    if let Ok(req) = serde_json::from_str::<JsonRpcRequest>(&data) {
                        let resp = self.handle_request(req).await;
                        let json = serde_json::to_string(&resp).map_err(|e| {
                            crab_common::Error::Other(format!("failed to serialize response: {e}"))
                        })?;
                        write_message(&writer, &json).await?;
                    }
                    // Notifications (no "id") are silently accepted.
                }
            }
        }
        Ok(())
    }

    /// Public dispatch method for use by SSE server transport.
    pub async fn handle_request_public(&self, req: JsonRpcRequest) -> JsonRpcResponse {
        self.handle_request(req).await
    }

    /// Dispatch a single JSON-RPC request to the appropriate handler.
    async fn handle_request(&self, req: JsonRpcRequest) -> JsonRpcResponse {
        let id = req.id;
        match req.method.as_str() {
            method::INITIALIZE => self.handle_initialize(id, req.params),
            method::PING => ok_response(id, json!({})),
            method::TOOLS_LIST => self.handle_tools_list(id),
            method::TOOLS_CALL => self.handle_tools_call(id, req.params).await,
            method::RESOURCES_LIST => self.handle_resources_list(id),
            method::RESOURCES_READ => self.handle_resources_read(id, req.params).await,
            method::PROMPTS_LIST => self.handle_prompts_list(id),
            method::PROMPTS_GET => self.handle_prompts_get(id, req.params),
            _ => error_response(id, -32601, format!("method not found: {}", req.method)),
        }
    }

    fn handle_initialize(&self, id: u64, _params: Option<Value>) -> JsonRpcResponse {
        let resources = if self.resource_handler.is_some() {
            Some(json!({}))
        } else {
            None
        };
        let result = InitializeResult {
            protocol_version: MCP_PROTOCOL_VERSION.to_string(),
            capabilities: ServerCapabilities {
                tools: Some(json!({})),
                resources,
                prompts: if self.prompt_handler.is_some() {
                    Some(json!({}))
                } else {
                    None
                },
            },
            server_info: self.server_info.clone(),
        };
        ok_response(id, serde_json::to_value(result).unwrap_or(Value::Null))
    }

    fn handle_tools_list(&self, id: u64) -> JsonRpcResponse {
        let tools = self.handler.list_tools();
        ok_response(id, json!({ "tools": tools }))
    }

    async fn handle_tools_call(&self, id: u64, params: Option<Value>) -> JsonRpcResponse {
        let Some(params_val) = params else {
            return error_response(id, -32602, "missing params for tools/call".into());
        };

        let call_params: ToolCallParams = match serde_json::from_value(params_val) {
            Ok(p) => p,
            Err(e) => {
                return error_response(id, -32602, format!("invalid params for tools/call: {e}"));
            }
        };

        let result = self
            .handler
            .call_tool(&call_params.name, call_params.arguments)
            .await;

        ok_response(id, serde_json::to_value(result).unwrap_or(Value::Null))
    }

    fn handle_resources_list(&self, id: u64) -> JsonRpcResponse {
        let Some(rh) = &self.resource_handler else {
            return error_response(id, -32601, "resources not supported".into());
        };
        let resources = rh.list_resources();
        ok_response(id, json!({ "resources": resources }))
    }

    async fn handle_resources_read(&self, id: u64, params: Option<Value>) -> JsonRpcResponse {
        let Some(rh) = &self.resource_handler else {
            return error_response(id, -32601, "resources not supported".into());
        };

        let Some(params_val) = params else {
            return error_response(id, -32602, "missing params for resources/read".into());
        };

        let read_params: ResourceReadParams = match serde_json::from_value(params_val) {
            Ok(p) => p,
            Err(e) => {
                return error_response(
                    id,
                    -32602,
                    format!("invalid params for resources/read: {e}"),
                );
            }
        };

        match rh.read_resource(&read_params.uri).await {
            Ok(result) => ok_response(id, serde_json::to_value(result).unwrap_or(Value::Null)),
            Err(msg) => error_response(id, -32602, msg),
        }
    }
    fn handle_prompts_list(&self, id: u64) -> JsonRpcResponse {
        let Some(ph) = &self.prompt_handler else {
            return error_response(id, -32601, "prompts not supported".into());
        };
        let prompts = ph.list_prompts();
        ok_response(id, json!({ "prompts": prompts }))
    }

    fn handle_prompts_get(&self, id: u64, params: Option<Value>) -> JsonRpcResponse {
        let Some(ph) = &self.prompt_handler else {
            return error_response(id, -32601, "prompts not supported".into());
        };
        let Some(params_val) = params else {
            return error_response(id, -32602, "missing params for prompts/get".into());
        };
        let get_params: PromptGetParams = match serde_json::from_value(params_val) {
            Ok(p) => p,
            Err(e) => {
                return error_response(id, -32602, format!("invalid params for prompts/get: {e}"));
            }
        };
        match ph.get_prompt(&get_params.name, &get_params.arguments) {
            Ok(result) => ok_response(id, serde_json::to_value(result).unwrap_or(Value::Null)),
            Err(msg) => error_response(id, -32602, msg),
        }
    }
}

// ─── Message framing ───

/// Read a Content-Length framed message from an async reader.
/// Returns `Ok(None)` on EOF.
async fn read_message<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
) -> crab_common::Result<Option<String>> {
    let mut content_length: Option<usize> = None;
    let mut header_line = String::new();

    loop {
        header_line.clear();
        let bytes_read = reader
            .read_line(&mut header_line)
            .await
            .map_err(|e| crab_common::Error::Other(format!("failed to read header: {e}")))?;

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

    let length = content_length
        .ok_or_else(|| crab_common::Error::Other("missing Content-Length header".into()))?;

    let mut body = vec![0u8; length];
    tokio::io::AsyncReadExt::read_exact(reader, &mut body)
        .await
        .map_err(|e| crab_common::Error::Other(format!("failed to read body: {e}")))?;

    String::from_utf8(body)
        .map(Some)
        .map_err(|e| crab_common::Error::Other(format!("invalid UTF-8: {e}")))
}

/// Write a Content-Length framed message to a shared async writer.
async fn write_message<W: tokio::io::AsyncWrite + Unpin>(
    writer: &Arc<Mutex<W>>,
    json: &str,
) -> crab_common::Result<()> {
    let frame = format!("Content-Length: {}\r\n\r\n{json}", json.len());
    let mut w = writer.lock().await;
    w.write_all(frame.as_bytes())
        .await
        .map_err(|e| crab_common::Error::Other(format!("failed to write response: {e}")))?;
    w.flush()
        .await
        .map_err(|e| crab_common::Error::Other(format!("failed to flush: {e}")))?;
    drop(w);
    Ok(())
}

// ─── Response helpers ───

fn ok_response(id: u64, result: Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".into(),
        id,
        result: Some(result),
        error: None,
    }
}

fn error_response(id: u64, code: i32, message: String) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".into(),
        id,
        result: None,
        error: Some(JsonRpcError {
            code,
            message,
            data: None,
        }),
    }
}

// ─── Default ToolHandler for Vec<Arc<dyn Tool>> ───

/// A simple tool handler backed by a list of `Arc<dyn Tool>`.
pub struct ToolRegistryHandler {
    tools: Vec<Arc<dyn Tool>>,
    working_dir: std::path::PathBuf,
}

impl ToolRegistryHandler {
    /// Create a handler from a list of tools and a working directory.
    pub fn new(tools: Vec<Arc<dyn Tool>>, working_dir: std::path::PathBuf) -> Self {
        Self { tools, working_dir }
    }
}

impl ToolHandler for ToolRegistryHandler {
    fn list_tools(&self) -> Vec<McpToolDef> {
        self.tools
            .iter()
            .map(|t| McpToolDef {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: t.input_schema(),
            })
            .collect()
    }

    fn call_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolCallResult> + Send + '_>> {
        let name = name.to_string();
        Box::pin(async move {
            let tool = self.tools.iter().find(|t| t.name() == name);
            let Some(tool) = tool else {
                return ToolCallResult {
                    content: vec![ToolResultContent::Text {
                        text: format!("unknown tool: {name}"),
                    }],
                    is_error: true,
                };
            };

            let ctx = ToolContext {
                working_dir: self.working_dir.clone(),
                permission_mode: PermissionMode::Default,
                session_id: "mcp-server".into(),
                cancellation_token: CancellationToken::new(),
                permission_policy: PermissionPolicy::default(),
                ext: crab_core::tool::ToolContextExt::default(),
            };

            match tool.execute(arguments, &ctx).await {
                Ok(output) => {
                    let content = output
                        .content
                        .into_iter()
                        .map(|c| match c {
                            ToolOutputContent::Text { text } => ToolResultContent::Text { text },
                            ToolOutputContent::Image { media_type, data } => {
                                ToolResultContent::Image {
                                    data,
                                    mime_type: media_type,
                                }
                            }
                            ToolOutputContent::Json { value } => ToolResultContent::Text {
                                text: serde_json::to_string_pretty(&value).unwrap_or_default(),
                            },
                        })
                        .collect();
                    ToolCallResult {
                        content,
                        is_error: output.is_error,
                    }
                }
                Err(e) => ToolCallResult {
                    content: vec![ToolResultContent::Text {
                        text: format!("tool execution error: {e}"),
                    }],
                    is_error: true,
                },
            }
        })
    }
}

// ─── FileResourceHandler ───

/// A resource handler that exposes files under a root directory as MCP resources.
///
/// Files are listed with `file://` URIs relative to the root. Only regular files
/// are listed (no directories or symlinks). Reads return file contents as text.
pub struct FileResourceHandler {
    root: std::path::PathBuf,
}

impl FileResourceHandler {
    /// Create a handler that serves files under `root`.
    pub fn new(root: std::path::PathBuf) -> Self {
        Self { root }
    }

    /// Convert a `file://` URI back to an absolute path, validating it is
    /// under the root directory (prevents path traversal).
    fn uri_to_path(&self, uri: &str) -> Result<std::path::PathBuf, String> {
        let path_str = uri
            .strip_prefix("file://")
            .ok_or_else(|| format!("unsupported URI scheme: {uri}"))?;

        // On Windows, file:// URIs may have a leading slash before the drive letter
        #[cfg(windows)]
        let path_str = path_str.strip_prefix('/').unwrap_or(path_str);

        let path = std::path::PathBuf::from(path_str);

        // Canonicalize both to prevent traversal via .. or symlinks
        let canonical = path
            .canonicalize()
            .map_err(|e| format!("cannot resolve path: {e}"))?;
        let canonical_root = self
            .root
            .canonicalize()
            .map_err(|e| format!("cannot resolve root: {e}"))?;

        if !canonical.starts_with(&canonical_root) {
            return Err(format!("path outside root directory: {uri}"));
        }

        Ok(canonical)
    }

    /// Build a `file://` URI for a path.
    fn path_to_uri(path: &std::path::Path) -> String {
        let s = path.to_string_lossy().replace('\\', "/");
        if s.starts_with('/') {
            format!("file://{s}")
        } else {
            format!("file:///{s}")
        }
    }

    /// Guess MIME type from file extension.
    fn guess_mime(path: &std::path::Path) -> Option<String> {
        let ext = path.extension()?.to_str()?;
        let mime = match ext {
            "rs" => "text/x-rust",
            "toml" => "text/x-toml",
            "json" => "application/json",
            "md" => "text/markdown",
            "yaml" | "yml" => "text/x-yaml",
            "py" => "text/x-python",
            "js" => "text/javascript",
            "ts" => "text/typescript",
            "html" | "htm" => "text/html",
            "css" => "text/css",
            "sh" | "bash" => "text/x-shellscript",
            "xml" => "text/xml",
            "csv" => "text/csv",
            _ => "text/plain",
        };
        Some(mime.to_string())
    }
}

impl ResourceHandler for FileResourceHandler {
    fn list_resources(&self) -> Vec<McpResource> {
        let entries = list_files_recursive(&self.root, 500);
        let mut resources = Vec::with_capacity(entries.len());
        for path in entries {
            let uri = Self::path_to_uri(&path);
            let name = path
                .strip_prefix(&self.root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let mime_type = Self::guess_mime(&path);
            resources.push(McpResource {
                uri,
                name,
                description: None,
                mime_type,
            });
        }
        resources
    }

    fn read_resource(
        &self,
        uri: &str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ResourceReadResult, String>> + Send + '_>,
    > {
        let uri = uri.to_string();
        Box::pin(async move {
            let path = self.uri_to_path(&uri)?;

            let content = tokio::fs::read_to_string(&path)
                .await
                .map_err(|e| format!("failed to read {uri}: {e}"))?;

            let mime_type = Self::guess_mime(&path);

            Ok(ResourceReadResult {
                contents: vec![ResourceContent {
                    uri,
                    mime_type,
                    text: Some(content),
                }],
            })
        })
    }
}

/// Recursively list regular files under `dir`, up to `max_files`.
fn list_files_recursive(dir: &std::path::Path, max_files: usize) -> Vec<std::path::PathBuf> {
    let mut result = Vec::new();
    let mut stack = vec![dir.to_path_buf()];

    while let Some(current) = stack.pop() {
        if result.len() >= max_files {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries {
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|name| {
                    name.starts_with('.') || name == "target" || name == "node_modules"
                })
            {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                result.push(path);
                if result.len() >= max_files {
                    break;
                }
            }
        }
    }

    result.sort();
    result
}

// ─── SkillPromptHandler ───

/// A prompt handler that bridges `SkillRegistry` skills to MCP prompts.
///
/// Each skill becomes an MCP prompt. The skill content is returned as a
/// single user message, with any \{\{arg\}\} placeholders substituted.
pub struct SkillPromptHandler {
    prompts: Vec<(McpPrompt, String)>, // (definition, content template)
}

impl SkillPromptHandler {
    /// Create a handler from a list of (name, description, content) tuples.
    ///
    /// Arguments are detected by scanning content for  placeholders.
    pub fn new(skills: Vec<(String, String, String)>) -> Self {
        let prompts = skills
            .into_iter()
            .map(|(name, description, content)| {
                let arguments = extract_placeholders(&content);
                let prompt = McpPrompt {
                    name,
                    description: if description.is_empty() {
                        None
                    } else {
                        Some(description)
                    },
                    arguments,
                };
                (prompt, content)
            })
            .collect();
        Self { prompts }
    }
}

/// Extract  placeholders from content and return them as prompt arguments.
fn extract_placeholders(content: &str) -> Vec<crate::protocol::PromptArgument> {
    let mut seen = std::collections::HashSet::new();
    let mut args = Vec::new();
    let mut rest = content;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        if let Some(end) = after.find("}}") {
            let name = after[..end].trim().to_string();
            if !name.is_empty() && seen.insert(name.clone()) {
                args.push(crate::protocol::PromptArgument {
                    name,
                    description: None,
                    required: true,
                });
            }
            rest = &after[end + 2..];
        } else {
            break;
        }
    }
    args
}

impl PromptHandler for SkillPromptHandler {
    fn list_prompts(&self) -> Vec<McpPrompt> {
        self.prompts.iter().map(|(p, _)| p.clone()).collect()
    }

    fn get_prompt(
        &self,
        name: &str,
        arguments: &std::collections::HashMap<String, String>,
    ) -> Result<PromptGetResult, String> {
        let (prompt_def, template) = self
            .prompts
            .iter()
            .find(|(p, _)| p.name == name)
            .ok_or_else(|| format!("prompt not found: {name}"))?;

        // Validate required arguments
        for arg in &prompt_def.arguments {
            if arg.required && !arguments.contains_key(&arg.name) {
                return Err(format!("missing required argument: {}", arg.name));
            }
        }

        // Substitute placeholders
        let mut text = template.clone();
        for (key, value) in arguments {
            text = text.replace(&format!("{{{{{key}}}}}"), value);
        }

        Ok(PromptGetResult {
            description: prompt_def.description.clone(),
            messages: vec![crate::protocol::PromptMessage {
                role: "user".into(),
                content: crate::protocol::PromptMessageContent::Text { text },
            }],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A minimal test tool handler with static tools.
    struct TestHandler {
        tools: Vec<McpToolDef>,
    }

    impl TestHandler {
        fn new() -> Self {
            Self {
                tools: vec![
                    McpToolDef {
                        name: "echo".into(),
                        description: "Echo input back".into(),
                        input_schema: json!({
                            "type": "object",
                            "properties": { "message": { "type": "string" } },
                            "required": ["message"]
                        }),
                    },
                    McpToolDef {
                        name: "add".into(),
                        description: "Add two numbers".into(),
                        input_schema: json!({
                            "type": "object",
                            "properties": {
                                "a": { "type": "number" },
                                "b": { "type": "number" }
                            },
                            "required": ["a", "b"]
                        }),
                    },
                ],
            }
        }
    }

    impl ToolHandler for TestHandler {
        fn list_tools(&self) -> Vec<McpToolDef> {
            self.tools.clone()
        }

        fn call_tool(
            &self,
            name: &str,
            arguments: Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolCallResult> + Send + '_>>
        {
            let name = name.to_string();
            Box::pin(async move {
                match name.as_str() {
                    "echo" => {
                        let msg = arguments["message"]
                            .as_str()
                            .unwrap_or("no message")
                            .to_string();
                        ToolCallResult {
                            content: vec![ToolResultContent::Text { text: msg }],
                            is_error: false,
                        }
                    }
                    "add" => {
                        let a = arguments["a"].as_f64().unwrap_or(0.0);
                        let b = arguments["b"].as_f64().unwrap_or(0.0);
                        ToolCallResult {
                            content: vec![ToolResultContent::Text {
                                text: format!("{}", a + b),
                            }],
                            is_error: false,
                        }
                    }
                    _ => ToolCallResult {
                        content: vec![ToolResultContent::Text {
                            text: format!("unknown tool: {name}"),
                        }],
                        is_error: true,
                    },
                }
            })
        }
    }

    fn make_server() -> McpServer<TestHandler> {
        McpServer::new("test-server", "0.1.0", Arc::new(TestHandler::new()))
    }

    /// Encode a JSON-RPC request as a Content-Length framed message.
    fn frame_request(req: &JsonRpcRequest) -> Vec<u8> {
        let json = serde_json::to_string(req).unwrap();
        format!("Content-Length: {}\r\n\r\n{json}", json.len()).into_bytes()
    }

    #[tokio::test]
    async fn handle_initialize() {
        let server = make_server();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 1,
            method: method::INITIALIZE.into(),
            params: Some(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "test-client", "version": "1.0" }
            })),
        };
        let resp = server.handle_request(req).await;
        assert_eq!(resp.id, 1);
        assert!(resp.error.is_none());
        let result: InitializeResult = serde_json::from_value(resp.result.unwrap()).unwrap();
        assert_eq!(result.protocol_version, MCP_PROTOCOL_VERSION);
        assert_eq!(result.server_info.name, "test-server");
        assert!(result.capabilities.tools.is_some());
    }

    #[tokio::test]
    async fn handle_ping() {
        let server = make_server();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 2,
            method: method::PING.into(),
            params: None,
        };
        let resp = server.handle_request(req).await;
        assert_eq!(resp.id, 2);
        assert!(resp.error.is_none());
    }

    #[tokio::test]
    async fn handle_tools_list() {
        let server = make_server();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 3,
            method: method::TOOLS_LIST.into(),
            params: Some(json!({})),
        };
        let resp = server.handle_request(req).await;
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let tools: Vec<McpToolDef> = serde_json::from_value(result["tools"].clone()).unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "echo");
        assert_eq!(tools[1].name, "add");
    }

    #[tokio::test]
    async fn handle_tools_call_echo() {
        let server = make_server();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 4,
            method: method::TOOLS_CALL.into(),
            params: Some(json!({
                "name": "echo",
                "arguments": { "message": "hello world" }
            })),
        };
        let resp = server.handle_request(req).await;
        assert!(resp.error.is_none());
        let result: ToolCallResult = serde_json::from_value(resp.result.unwrap()).unwrap();
        assert!(!result.is_error);
        assert_eq!(result.content.len(), 1);
        match &result.content[0] {
            ToolResultContent::Text { text } => assert_eq!(text, "hello world"),
            _ => panic!("expected text content"),
        }
    }

    #[tokio::test]
    async fn handle_tools_call_add() {
        let server = make_server();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 5,
            method: method::TOOLS_CALL.into(),
            params: Some(json!({
                "name": "add",
                "arguments": { "a": 3, "b": 7 }
            })),
        };
        let resp = server.handle_request(req).await;
        assert!(resp.error.is_none());
        let result: ToolCallResult = serde_json::from_value(resp.result.unwrap()).unwrap();
        assert!(!result.is_error);
        match &result.content[0] {
            ToolResultContent::Text { text } => assert_eq!(text, "10"),
            _ => panic!("expected text content"),
        }
    }

    #[tokio::test]
    async fn handle_tools_call_unknown_tool() {
        let server = make_server();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 6,
            method: method::TOOLS_CALL.into(),
            params: Some(json!({
                "name": "nonexistent",
                "arguments": {}
            })),
        };
        let resp = server.handle_request(req).await;
        assert!(resp.error.is_none()); // tool error is in the result, not JSON-RPC error
        let result: ToolCallResult = serde_json::from_value(resp.result.unwrap()).unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn handle_tools_call_missing_params() {
        let server = make_server();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 7,
            method: method::TOOLS_CALL.into(),
            params: None,
        };
        let resp = server.handle_request(req).await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32602);
    }

    #[tokio::test]
    async fn handle_unknown_method() {
        let server = make_server();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 8,
            method: "nonexistent/method".into(),
            params: None,
        };
        let resp = server.handle_request(req).await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    #[tokio::test]
    async fn run_full_session() {
        let server = make_server();

        // Build a sequence of framed requests
        let mut input = Vec::new();

        // 1. initialize
        input.extend_from_slice(&frame_request(&JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 1,
            method: method::INITIALIZE.into(),
            params: Some(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1.0" }
            })),
        }));

        // 2. tools/list
        input.extend_from_slice(&frame_request(&JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 2,
            method: method::TOOLS_LIST.into(),
            params: Some(json!({})),
        }));

        // 3. tools/call
        input.extend_from_slice(&frame_request(&JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 3,
            method: method::TOOLS_CALL.into(),
            params: Some(json!({
                "name": "echo",
                "arguments": { "message": "test" }
            })),
        }));

        let mut output = Vec::new();

        server.run(&input[..], &mut output).await.unwrap();

        // Parse all responses from the output buffer
        let output_str = String::from_utf8(output).unwrap();
        let responses: Vec<JsonRpcResponse> = output_str
            .split("Content-Length: ")
            .filter(|s| !s.is_empty())
            .map(|chunk| {
                let body_start = chunk.find("\r\n\r\n").unwrap() + 4;
                serde_json::from_str(&chunk[body_start..]).unwrap()
            })
            .collect();

        assert_eq!(responses.len(), 3);

        // Verify initialize response
        assert_eq!(responses[0].id, 1);
        assert!(responses[0].error.is_none());

        // Verify tools/list response
        assert_eq!(responses[1].id, 2);
        let tools = responses[1].result.as_ref().unwrap()["tools"]
            .as_array()
            .unwrap();
        assert_eq!(tools.len(), 2);

        // Verify tools/call response
        assert_eq!(responses[2].id, 3);
        let call_result: ToolCallResult =
            serde_json::from_value(responses[2].result.clone().unwrap()).unwrap();
        assert!(!call_result.is_error);
        match &call_result.content[0] {
            ToolResultContent::Text { text } => assert_eq!(text, "test"),
            _ => panic!("expected text"),
        }
    }

    #[tokio::test]
    async fn run_empty_input_returns_ok() {
        let server = make_server();
        let input: &[u8] = &[];
        let mut output = Vec::new();
        let result = server.run(input, &mut output).await;
        assert!(result.is_ok());
        assert!(output.is_empty());
    }

    #[tokio::test]
    async fn read_message_eof() {
        let data: &[u8] = &[];
        let mut reader = BufReader::new(data);
        let msg = read_message(&mut reader).await.unwrap();
        assert!(msg.is_none());
    }

    #[tokio::test]
    async fn read_message_parses_frame() {
        let data = b"Content-Length: 13\r\n\r\n{\"test\":true}";
        let mut reader = BufReader::new(&data[..]);
        let msg = read_message(&mut reader).await.unwrap().unwrap();
        assert_eq!(msg, "{\"test\":true}");
    }

    #[tokio::test]
    async fn write_message_frames_correctly() {
        let writer = Arc::new(Mutex::new(Vec::<u8>::new()));
        write_message(&writer, "{\"ok\":true}").await.unwrap();
        let text = {
            let data = writer.lock().await;
            String::from_utf8(data.clone()).unwrap()
        };
        assert!(text.starts_with("Content-Length: 11\r\n\r\n"));
        assert!(text.contains("{\"ok\":true}"));
    }

    #[test]
    fn ok_response_has_no_error() {
        let resp = ok_response(1, json!({"result": true}));
        assert_eq!(resp.id, 1);
        assert!(resp.error.is_none());
        assert!(resp.result.is_some());
    }

    #[test]
    fn error_response_has_no_result() {
        let resp = error_response(2, -32600, "bad request".into());
        assert_eq!(resp.id, 2);
        assert!(resp.result.is_none());
        assert_eq!(resp.error.as_ref().unwrap().code, -32600);
        assert_eq!(resp.error.as_ref().unwrap().message, "bad request");
    }

    #[test]
    fn server_info_stored() {
        let server = make_server();
        assert_eq!(server.server_info.name, "test-server");
        assert_eq!(server.server_info.version, "0.1.0");
    }

    // ─── Resource handler tests ───

    struct TestResourceHandler {
        resources: Vec<McpResource>,
        contents: std::collections::HashMap<String, String>,
    }

    impl TestResourceHandler {
        fn new() -> Self {
            let mut contents = std::collections::HashMap::new();
            contents.insert("file:///project/README.md".into(), "# Hello".into());
            contents.insert("file:///project/src/main.rs".into(), "fn main() {}".into());
            Self {
                resources: vec![
                    McpResource {
                        uri: "file:///project/README.md".into(),
                        name: "README.md".into(),
                        description: Some("Project readme".into()),
                        mime_type: Some("text/markdown".into()),
                    },
                    McpResource {
                        uri: "file:///project/src/main.rs".into(),
                        name: "src/main.rs".into(),
                        description: None,
                        mime_type: Some("text/x-rust".into()),
                    },
                ],
                contents,
            }
        }
    }

    impl ResourceHandler for TestResourceHandler {
        fn list_resources(&self) -> Vec<McpResource> {
            self.resources.clone()
        }
        fn read_resource(
            &self,
            uri: &str,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<ResourceReadResult, String>> + Send + '_>,
        > {
            let uri = uri.to_string();
            Box::pin(async move {
                let content = self
                    .contents
                    .get(&uri)
                    .ok_or_else(|| format!("resource not found: {uri}"))?;
                Ok(ResourceReadResult {
                    contents: vec![ResourceContent {
                        uri: uri.clone(),
                        mime_type: Some("text/plain".into()),
                        text: Some(content.clone()),
                    }],
                })
            })
        }
    }

    fn make_server_with_resources() -> McpServer<TestHandler> {
        McpServer::new("test-server", "0.1.0", Arc::new(TestHandler::new()))
            .with_resource_handler(Arc::new(TestResourceHandler::new()))
    }

    #[tokio::test]
    async fn init_without_resources_no_capability() {
        let server = make_server();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 1,
            method: method::INITIALIZE.into(),
            params: Some(
                json!({"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"1"}}),
            ),
        };
        let resp = server.handle_request(req).await;
        let r: InitializeResult = serde_json::from_value(resp.result.unwrap()).unwrap();
        assert!(r.capabilities.resources.is_none());
    }

    #[tokio::test]
    async fn init_with_resources_advertises() {
        let server = make_server_with_resources();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 1,
            method: method::INITIALIZE.into(),
            params: Some(
                json!({"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"1"}}),
            ),
        };
        let resp = server.handle_request(req).await;
        let r: InitializeResult = serde_json::from_value(resp.result.unwrap()).unwrap();
        assert!(r.capabilities.resources.is_some());
    }

    #[tokio::test]
    async fn resources_list_ok() {
        let server = make_server_with_resources();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 10,
            method: method::RESOURCES_LIST.into(),
            params: Some(json!({})),
        };
        let resp = server.handle_request(req).await;
        assert!(resp.error.is_none());
        let resources: Vec<McpResource> =
            serde_json::from_value(resp.result.unwrap()["resources"].clone()).unwrap();
        assert_eq!(resources.len(), 2);
        assert_eq!(resources[0].name, "README.md");
    }

    #[tokio::test]
    async fn resources_list_no_handler() {
        let server = make_server();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 11,
            method: method::RESOURCES_LIST.into(),
            params: None,
        };
        let resp = server.handle_request(req).await;
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    #[tokio::test]
    async fn resources_read_ok() {
        let server = make_server_with_resources();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 12,
            method: method::RESOURCES_READ.into(),
            params: Some(json!({"uri":"file:///project/README.md"})),
        };
        let resp = server.handle_request(req).await;
        assert!(resp.error.is_none());
        let r: ResourceReadResult = serde_json::from_value(resp.result.unwrap()).unwrap();
        assert_eq!(r.contents[0].text.as_deref(), Some("# Hello"));
    }

    #[tokio::test]
    async fn resources_read_missing_params() {
        let server = make_server_with_resources();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 13,
            method: method::RESOURCES_READ.into(),
            params: None,
        };
        let resp = server.handle_request(req).await;
        assert_eq!(resp.error.unwrap().code, -32602);
    }

    #[tokio::test]
    async fn resources_read_unknown_uri() {
        let server = make_server_with_resources();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 14,
            method: method::RESOURCES_READ.into(),
            params: Some(json!({"uri":"file:///nonexistent"})),
        };
        let resp = server.handle_request(req).await;
        assert!(resp.error.unwrap().message.contains("not found"));
    }

    #[tokio::test]
    async fn resources_read_no_handler() {
        let server = make_server();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 15,
            method: method::RESOURCES_READ.into(),
            params: Some(json!({"uri":"file:///x"})),
        };
        let resp = server.handle_request(req).await;
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    #[test]
    fn file_handler_path_to_uri() {
        let uri = FileResourceHandler::path_to_uri(std::path::Path::new("/tmp/foo.rs"));
        assert_eq!(uri, "file:///tmp/foo.rs");
    }

    #[test]
    fn file_handler_guess_mime() {
        assert_eq!(
            FileResourceHandler::guess_mime(std::path::Path::new("foo.rs")),
            Some("text/x-rust".into())
        );
        assert_eq!(
            FileResourceHandler::guess_mime(std::path::Path::new("d.json")),
            Some("application/json".into())
        );
        assert_eq!(
            FileResourceHandler::guess_mime(std::path::Path::new("no_ext")),
            None
        );
    }

    #[tokio::test]
    async fn file_handler_list_and_read() {
        let dir = std::env::temp_dir().join("crab_mcp_res_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("hello.txt"), "Hello!").unwrap();
        std::fs::write(dir.join("data.json"), r#"{"k":"v"}"#).unwrap();

        let handler = FileResourceHandler::new(dir.clone());
        let resources = handler.list_resources();
        assert_eq!(resources.len(), 2);

        let hello = resources.iter().find(|r| r.name == "hello.txt").unwrap();
        let result = handler.read_resource(&hello.uri).await.unwrap();
        assert_eq!(result.contents[0].text.as_deref(), Some("Hello!"));

        let err = handler
            .read_resource("file:///nonexistent/p.txt")
            .await
            .unwrap_err();
        assert!(err.contains("cannot resolve path"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn file_handler_path_traversal_blocked() {
        let dir = std::env::temp_dir().join("crab_mcp_trav_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("safe.txt"), "ok").unwrap();

        let parent_file = dir.parent().unwrap().join("crab_outside.txt");
        std::fs::write(&parent_file, "secret").unwrap();

        let handler = FileResourceHandler::new(dir.clone());
        let uri = FileResourceHandler::path_to_uri(&parent_file);
        let err = handler.read_resource(&uri).await.unwrap_err();
        assert!(err.contains("path outside root directory"));

        let _ = std::fs::remove_file(&parent_file);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_files_respects_limit() {
        let dir = std::env::temp_dir().join("crab_mcp_lim_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..10 {
            std::fs::write(dir.join(format!("f{i}.txt")), "x").unwrap();
        }
        assert_eq!(list_files_recursive(&dir, 3).len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_files_skips_hidden_and_target() {
        let dir = std::env::temp_dir().join("crab_mcp_skip_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join(".git").join("config"), "git").unwrap();
        std::fs::write(dir.join("target").join("debug"), "bin").unwrap();
        std::fs::write(dir.join("src").join("main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]").unwrap();

        let files = list_files_recursive(&dir, 100);
        let names: Vec<String> = files
            .iter()
            .map(|f| {
                f.strip_prefix(&dir)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert!(names.contains(&"Cargo.toml".to_string()));
        assert!(names.contains(&"src/main.rs".to_string()));
        assert!(!names.iter().any(|n| n.starts_with(".git")));
        assert!(!names.iter().any(|n| n.starts_with("target")));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn full_session_with_resources() {
        let server = make_server_with_resources();
        let mut input = Vec::new();
        input.extend_from_slice(&frame_request(&JsonRpcRequest { jsonrpc: "2.0".into(), id: 1, method: method::INITIALIZE.into(), params: Some(json!({"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"1"}})) }));
        input.extend_from_slice(&frame_request(&JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 2,
            method: method::RESOURCES_LIST.into(),
            params: Some(json!({})),
        }));
        input.extend_from_slice(&frame_request(&JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 3,
            method: method::RESOURCES_READ.into(),
            params: Some(json!({"uri":"file:///project/src/main.rs"})),
        }));

        let mut output = Vec::new();
        server.run(&input[..], &mut output).await.unwrap();

        let output_str = String::from_utf8(output).unwrap();
        let responses: Vec<JsonRpcResponse> = output_str
            .split("Content-Length: ")
            .filter(|s| !s.is_empty())
            .map(|chunk| {
                let s = chunk.find("\r\n\r\n").unwrap() + 4;
                serde_json::from_str(&chunk[s..]).unwrap()
            })
            .collect();
        assert_eq!(responses.len(), 3);

        let init: InitializeResult =
            serde_json::from_value(responses[0].result.clone().unwrap()).unwrap();
        assert!(init.capabilities.resources.is_some());

        let resources: Vec<McpResource> =
            serde_json::from_value(responses[1].result.as_ref().unwrap()["resources"].clone())
                .unwrap();
        assert_eq!(resources.len(), 2);

        let read: ResourceReadResult =
            serde_json::from_value(responses[2].result.clone().unwrap()).unwrap();
        assert_eq!(read.contents[0].text.as_deref(), Some("fn main() {}"));
    }

    // ─── Prompt handler tests ───

    struct TestPromptHandler;

    impl PromptHandler for TestPromptHandler {
        fn list_prompts(&self) -> Vec<McpPrompt> {
            vec![
                McpPrompt {
                    name: "greet".into(),
                    description: Some("Generate a greeting".into()),
                    arguments: vec![crate::protocol::PromptArgument {
                        name: "name".into(),
                        description: Some("Person to greet".into()),
                        required: true,
                    }],
                },
                McpPrompt {
                    name: "summarize".into(),
                    description: Some("Summarize text".into()),
                    arguments: vec![],
                },
            ]
        }

        fn get_prompt(
            &self,
            name: &str,
            arguments: &std::collections::HashMap<String, String>,
        ) -> Result<PromptGetResult, String> {
            match name {
                "greet" => {
                    let person = arguments
                        .get("name")
                        .ok_or_else(|| "missing required argument: name".to_string())?;
                    Ok(PromptGetResult {
                        description: Some("Generate a greeting".into()),
                        messages: vec![crate::protocol::PromptMessage {
                            role: "user".into(),
                            content: crate::protocol::PromptMessageContent::Text {
                                text: format!("Hello, {person}!"),
                            },
                        }],
                    })
                }
                "summarize" => Ok(PromptGetResult {
                    description: Some("Summarize text".into()),
                    messages: vec![crate::protocol::PromptMessage {
                        role: "user".into(),
                        content: crate::protocol::PromptMessageContent::Text {
                            text: "Please summarize the following text.".into(),
                        },
                    }],
                }),
                _ => Err(format!("prompt not found: {name}")),
            }
        }
    }

    fn make_server_with_prompts() -> McpServer<TestHandler> {
        McpServer::new("test-server", "0.1.0", Arc::new(TestHandler::new()))
            .with_prompt_handler(Arc::new(TestPromptHandler))
    }

    #[tokio::test]
    async fn init_without_prompts_no_capability() {
        let server = make_server();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 1,
            method: method::INITIALIZE.into(),
            params: Some(
                json!({"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"1"}}),
            ),
        };
        let resp = server.handle_request(req).await;
        let r: InitializeResult = serde_json::from_value(resp.result.unwrap()).unwrap();
        assert!(r.capabilities.prompts.is_none());
    }

    #[tokio::test]
    async fn init_with_prompts_advertises() {
        let server = make_server_with_prompts();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 1,
            method: method::INITIALIZE.into(),
            params: Some(
                json!({"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"1"}}),
            ),
        };
        let resp = server.handle_request(req).await;
        let r: InitializeResult = serde_json::from_value(resp.result.unwrap()).unwrap();
        assert!(r.capabilities.prompts.is_some());
    }

    #[tokio::test]
    async fn prompts_list_ok() {
        let server = make_server_with_prompts();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 20,
            method: method::PROMPTS_LIST.into(),
            params: Some(json!({})),
        };
        let resp = server.handle_request(req).await;
        assert!(resp.error.is_none());
        let prompts: Vec<McpPrompt> =
            serde_json::from_value(resp.result.unwrap()["prompts"].clone()).unwrap();
        assert_eq!(prompts.len(), 2);
        assert_eq!(prompts[0].name, "greet");
        assert_eq!(prompts[0].arguments.len(), 1);
        assert_eq!(prompts[1].name, "summarize");
    }

    #[tokio::test]
    async fn prompts_list_no_handler() {
        let server = make_server();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 21,
            method: method::PROMPTS_LIST.into(),
            params: None,
        };
        let resp = server.handle_request(req).await;
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    #[tokio::test]
    async fn prompts_get_ok() {
        let server = make_server_with_prompts();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 22,
            method: method::PROMPTS_GET.into(),
            params: Some(json!({"name":"greet","arguments":{"name":"Alice"}})),
        };
        let resp = server.handle_request(req).await;
        assert!(resp.error.is_none());
        let result: PromptGetResult = serde_json::from_value(resp.result.unwrap()).unwrap();
        assert_eq!(result.messages.len(), 1);
        match &result.messages[0].content {
            crate::protocol::PromptMessageContent::Text { text } => {
                assert_eq!(text, "Hello, Alice!");
            }
            crate::protocol::PromptMessageContent::Resource { .. } => {
                panic!("expected text content")
            }
        }
    }

    #[tokio::test]
    async fn prompts_get_missing_params() {
        let server = make_server_with_prompts();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 23,
            method: method::PROMPTS_GET.into(),
            params: None,
        };
        let resp = server.handle_request(req).await;
        assert_eq!(resp.error.unwrap().code, -32602);
    }

    #[tokio::test]
    async fn prompts_get_unknown_name() {
        let server = make_server_with_prompts();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 24,
            method: method::PROMPTS_GET.into(),
            params: Some(json!({"name":"nonexistent","arguments":{}})),
        };
        let resp = server.handle_request(req).await;
        assert!(resp.error.unwrap().message.contains("not found"));
    }

    #[tokio::test]
    async fn prompts_get_no_handler() {
        let server = make_server();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 25,
            method: method::PROMPTS_GET.into(),
            params: Some(json!({"name":"greet","arguments":{}})),
        };
        let resp = server.handle_request(req).await;
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    #[tokio::test]
    async fn prompts_get_missing_required_arg() {
        let server = make_server_with_prompts();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 26,
            method: method::PROMPTS_GET.into(),
            params: Some(json!({"name":"greet","arguments":{}})),
        };
        let resp = server.handle_request(req).await;
        // The TestPromptHandler returns an error for missing "name" arg
        assert!(
            resp.error
                .unwrap()
                .message
                .contains("missing required argument")
        );
    }

    // ─── SkillPromptHandler tests ───

    #[test]
    fn extract_placeholders_works() {
        let args = extract_placeholders("Hello {{name}}, welcome to {{place}}!");
        assert_eq!(args.len(), 2);
        assert_eq!(args[0].name, "name");
        assert_eq!(args[1].name, "place");
        assert!(args[0].required);
    }

    #[test]
    fn extract_placeholders_deduplicates() {
        let args = extract_placeholders("{{x}} and {{x}} again");
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].name, "x");
    }

    #[test]
    fn extract_placeholders_empty() {
        let args = extract_placeholders("no placeholders here");
        assert!(args.is_empty());
    }

    #[test]
    fn skill_prompt_handler_list_and_get() {
        let handler = SkillPromptHandler::new(vec![
            (
                "code-review".into(),
                "Review code for issues".into(),
                "Please review this {{language}} code:\n{{code}}".into(),
            ),
            (
                "explain".into(),
                "Explain a concept".into(),
                "Explain {{topic}} simply.".into(),
            ),
        ]);

        let prompts = handler.list_prompts();
        assert_eq!(prompts.len(), 2);
        assert_eq!(prompts[0].name, "code-review");
        assert_eq!(prompts[0].arguments.len(), 2);
        assert_eq!(prompts[0].arguments[0].name, "language");
        assert_eq!(prompts[0].arguments[1].name, "code");
        assert_eq!(prompts[1].name, "explain");
        assert_eq!(prompts[1].arguments.len(), 1);
    }

    #[test]
    fn skill_prompt_handler_placeholder_substitution() {
        let handler = SkillPromptHandler::new(vec![(
            "greet".into(),
            "Greet someone".into(),
            "Hello {{name}}, welcome to {{place}}!".into(),
        )]);

        let mut args = std::collections::HashMap::new();
        args.insert("name".into(), "Bob".into());
        args.insert("place".into(), "Rust Land".into());

        let result = handler.get_prompt("greet", &args).unwrap();
        assert_eq!(result.messages.len(), 1);
        match &result.messages[0].content {
            crate::protocol::PromptMessageContent::Text { text } => {
                assert_eq!(text, "Hello Bob, welcome to Rust Land!");
            }
            crate::protocol::PromptMessageContent::Resource { .. } => {
                panic!("expected text")
            }
        }
    }

    #[test]
    fn skill_prompt_handler_missing_required_arg() {
        let handler = SkillPromptHandler::new(vec![(
            "greet".into(),
            String::new(),
            "Hello {{name}}!".into(),
        )]);

        let args = std::collections::HashMap::new();
        let err = handler.get_prompt("greet", &args).unwrap_err();
        assert!(err.contains("missing required argument: name"));
    }

    #[test]
    fn skill_prompt_handler_unknown_prompt() {
        let handler = SkillPromptHandler::new(vec![]);
        let args = std::collections::HashMap::new();
        let err = handler.get_prompt("nope", &args).unwrap_err();
        assert!(err.contains("prompt not found: nope"));
    }

    #[tokio::test]
    async fn full_session_with_prompts() {
        let server = make_server_with_prompts();
        let mut input = Vec::new();
        input.extend_from_slice(&frame_request(&JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 1,
            method: method::INITIALIZE.into(),
            params: Some(
                json!({"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"1"}}),
            ),
        }));
        input.extend_from_slice(&frame_request(&JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 2,
            method: method::PROMPTS_LIST.into(),
            params: Some(json!({})),
        }));
        input.extend_from_slice(&frame_request(&JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 3,
            method: method::PROMPTS_GET.into(),
            params: Some(json!({"name":"summarize","arguments":{}})),
        }));

        let mut output = Vec::new();
        server.run(&input[..], &mut output).await.unwrap();

        let output_str = String::from_utf8(output).unwrap();
        let responses: Vec<JsonRpcResponse> = output_str
            .split("Content-Length: ")
            .filter(|s| !s.is_empty())
            .map(|chunk| {
                let s = chunk.find("\r\n\r\n").unwrap() + 4;
                serde_json::from_str(&chunk[s..]).unwrap()
            })
            .collect();
        assert_eq!(responses.len(), 3);

        // init advertises prompts
        let init: InitializeResult =
            serde_json::from_value(responses[0].result.clone().unwrap()).unwrap();
        assert!(init.capabilities.prompts.is_some());

        // prompts/list returns 2
        let prompts: Vec<McpPrompt> =
            serde_json::from_value(responses[1].result.as_ref().unwrap()["prompts"].clone())
                .unwrap();
        assert_eq!(prompts.len(), 2);

        // prompts/get returns message
        let get_result: PromptGetResult =
            serde_json::from_value(responses[2].result.clone().unwrap()).unwrap();
        assert_eq!(get_result.messages.len(), 1);
        assert_eq!(get_result.messages[0].role, "user");
    }
}
