use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};

/// Global request ID counter for JSON-RPC messages.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Generate a unique JSON-RPC request ID.
pub fn next_request_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

// ─── JSON-RPC 2.0 base types ───

/// JSON-RPC 2.0 request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    /// Create a new JSON-RPC 2.0 request with an auto-generated ID.
    pub fn new(method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: next_request_id(),
            method: method.into(),
            params,
        }
    }
}

/// JSON-RPC 2.0 response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    /// Check whether this response indicates an error.
    pub fn is_error(&self) -> bool {
        self.error.is_some()
    }

    /// Extract the result value, returning an error if the response is an error.
    pub fn into_result(self) -> crab_core::Result<Value> {
        if let Some(err) = self.error {
            Err(crab_core::Error::Other(format!(
                "MCP error: code={}, message={}",
                err.code, err.message
            )))
        } else {
            Ok(self.result.unwrap_or(Value::Null))
        }
    }
}

/// JSON-RPC 2.0 notification (no id, no response expected).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcNotification {
    /// Create a new notification.
    pub fn new(method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: method.into(),
            params,
        }
    }
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

// ─── MCP protocol constants ───

/// MCP protocol version.
pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

/// MCP method names.
pub mod method {
    pub const INITIALIZE: &str = "initialize";
    pub const INITIALIZED: &str = "notifications/initialized";
    pub const TOOLS_LIST: &str = "tools/list";
    pub const TOOLS_CALL: &str = "tools/call";
    pub const RESOURCES_LIST: &str = "resources/list";
    pub const RESOURCES_READ: &str = "resources/read";
    pub const PROMPTS_LIST: &str = "prompts/list";
    pub const PROMPTS_GET: &str = "prompts/get";
    pub const PING: &str = "ping";
}

// ─── MCP capability negotiation ───

/// Client capabilities sent during initialize.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClientCapabilities {
    /// Whether the client supports tool calling.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Value>,
    /// Whether the client supports resource access.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<Value>,
    /// Whether the client supports prompts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompts: Option<Value>,
}

/// Server capabilities returned during initialize.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompts: Option<Value>,
}

/// Client info sent during initialize.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

impl Default for ClientInfo {
    fn default() -> Self {
        Self {
            name: "crab-code".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

/// Server info returned during initialize.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
}

/// The `initialize` request params.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub protocol_version: String,
    pub capabilities: ClientCapabilities,
    pub client_info: ClientInfo,
}

impl Default for InitializeParams {
    fn default() -> Self {
        Self {
            protocol_version: MCP_PROTOCOL_VERSION.to_string(),
            capabilities: ClientCapabilities::default(),
            client_info: ClientInfo::default(),
        }
    }
}

/// The `initialize` response result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    pub server_info: ServerInfo,
}

// ─── MCP entity types ───

/// MCP tool definition returned by `tools/list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolDef {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub input_schema: Value,
}

/// Parameters for `tools/call`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallParams {
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}

/// Result of `tools/call`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallResult {
    pub content: Vec<ToolResultContent>,
    #[serde(default)]
    pub is_error: bool,
}

/// Content block in a tool call result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolResultContent {
    Text { text: String },
    Image { data: String, mime_type: String },
    Resource { resource: ResourceContent },
}

/// Embedded resource content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceContent {
    pub uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// MCP resource definition returned by `resources/list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpResource {
    pub uri: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// Parameters for `resources/read`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceReadParams {
    pub uri: String,
}

/// Result of `resources/read`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceReadResult {
    pub contents: Vec<ResourceContent>,
}

/// MCP prompt definition returned by `prompts/list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPrompt {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<PromptArgument>,
}

/// A prompt argument definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptArgument {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
}

/// Parameters for `prompts/get`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptGetParams {
    pub name: String,
    #[serde(default)]
    pub arguments: std::collections::HashMap<String, String>,
}

/// A message in a prompt result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptMessage {
    pub role: String,
    pub content: PromptMessageContent,
}

/// Content of a prompt message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PromptMessageContent {
    Text { text: String },
    Resource { resource: ResourceContent },
}

/// Result of `prompts/get`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptGetResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub messages: Vec<PromptMessage>,
}

// ─── Pagination ───

/// Paginated list result wrapper.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListResult<T> {
    #[serde(flatten)]
    pub items: ListItems<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// The items field varies by list type.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ListItems<T> {
    Tools { tools: Vec<T> },
    Resources { resources: Vec<T> },
    Prompts { prompts: Vec<T> },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_auto_id() {
        let r1 = JsonRpcRequest::new("test", None);
        let r2 = JsonRpcRequest::new("test", None);
        assert_ne!(r1.id, r2.id);
        assert_eq!(r1.jsonrpc, "2.0");
    }

    #[test]
    fn request_serde_roundtrip() {
        let req = JsonRpcRequest::new("tools/list", Some(json!({})));
        let json = serde_json::to_string(&req).unwrap();
        let parsed: JsonRpcRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.method, "tools/list");
        assert_eq!(parsed.jsonrpc, "2.0");
    }

    #[test]
    fn response_into_result_success() {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: 1,
            result: Some(json!({"ok": true})),
            error: None,
        };
        let val = resp.into_result().unwrap();
        assert_eq!(val, json!({"ok": true}));
    }

    #[test]
    fn response_into_result_error() {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: 1,
            result: None,
            error: Some(JsonRpcError {
                code: -32600,
                message: "Invalid Request".into(),
                data: None,
            }),
        };
        let err = resp.into_result().unwrap_err();
        assert!(err.to_string().contains("Invalid Request"));
    }

    #[test]
    fn notification_serde() {
        let notif = JsonRpcNotification::new("notifications/initialized", None);
        let json = serde_json::to_string(&notif).unwrap();
        assert!(json.contains("notifications/initialized"));
        assert!(!json.contains("\"id\""));
    }

    #[test]
    fn initialize_params_default() {
        let params = InitializeParams::default();
        assert_eq!(params.protocol_version, MCP_PROTOCOL_VERSION);
        assert_eq!(params.client_info.name, "crab-code");
    }

    #[test]
    fn initialize_result_deser() {
        let json = json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "test-server", "version": "1.0"}
        });
        let result: InitializeResult = serde_json::from_value(json).unwrap();
        assert_eq!(result.server_info.name, "test-server");
        assert!(result.capabilities.tools.is_some());
    }

    #[test]
    fn tool_def_deser() {
        let json = json!({
            "name": "read_file",
            "description": "Read a file",
            "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}}}
        });
        let tool: McpToolDef = serde_json::from_value(json).unwrap();
        assert_eq!(tool.name, "read_file");
    }

    #[test]
    fn tool_call_params_ser() {
        let params = ToolCallParams {
            name: "read_file".into(),
            arguments: json!({"path": "/tmp/foo.txt"}),
        };
        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["name"], "read_file");
        assert_eq!(json["arguments"]["path"], "/tmp/foo.txt");
    }

    #[test]
    fn tool_call_result_deser() {
        let json = json!({
            "content": [{"type": "text", "text": "file contents here"}],
            "isError": false
        });
        let result: ToolCallResult = serde_json::from_value(json).unwrap();
        assert!(!result.is_error);
        assert_eq!(result.content.len(), 1);
    }

    #[test]
    fn resource_deser() {
        let json = json!({
            "uri": "file:///tmp/data.json",
            "name": "data.json",
            "mimeType": "application/json"
        });
        let res: McpResource = serde_json::from_value(json).unwrap();
        assert_eq!(res.uri, "file:///tmp/data.json");
        assert_eq!(res.mime_type, Some("application/json".into()));
    }
}
