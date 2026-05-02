use std::collections::HashMap;
use std::process::Stdio;

use rmcp::service::RunningService;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use serde_json::{Map, Value, json};

use crate::protocol::{
    InitializeParams, InitializeResult, JsonRpcRequest, McpPrompt, McpResource, McpToolDef,
    PromptArgument, PromptGetParams, PromptGetResult, PromptMessage, PromptMessageContent,
    ResourceContent, ResourceReadParams, ResourceReadResult, ServerCapabilities, ServerInfo,
    ToolCallParams, ToolCallResult, ToolResultContent,
};
use crate::transport::Transport;

type RmcpClientService = RunningService<rmcp::RoleClient, rmcp::model::ClientInfo>;

enum ClientBackend {
    Legacy(Box<dyn Transport>),
    Rmcp(RmcpClientService),
}

/// MCP client — connects to an external MCP server, discovers tools/resources,
/// and forwards tool calls.
pub struct McpClient {
    backend: ClientBackend,
    server_name: String,
    server_info: ServerInfo,
    capabilities: ServerCapabilities,
    tools: Vec<McpToolDef>,
}

impl std::fmt::Debug for McpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpClient")
            .field("server_name", &self.server_name)
            .finish_non_exhaustive()
    }
}

impl McpClient {
    /// Connect to an MCP server using the legacy transport abstraction.
    ///
    /// This path remains for tests and transports that are still implemented
    /// locally inside `crab-mcp`.
    pub async fn connect(
        transport: Box<dyn Transport>,
        server_name: &str,
    ) -> crab_core::Result<Self> {
        let params = InitializeParams::default();
        let req = JsonRpcRequest::new(
            crate::protocol::method::INITIALIZE,
            Some(serde_json::to_value(&params).map_err(|e| {
                crab_core::Error::Other(format!("failed to serialize initialize params: {e}"))
            })?),
        );

        tracing::info!(server = server_name, "initializing MCP connection");

        let resp = transport.send(req).await?;
        let result_value = resp.into_result()?;

        let init_result: InitializeResult = serde_json::from_value(result_value).map_err(|e| {
            crab_core::Error::Other(format!("failed to parse initialize result: {e}"))
        })?;

        tracing::info!(
            server = server_name,
            server_name = init_result.server_info.name,
            server_version = init_result.server_info.version,
            protocol_version = init_result.protocol_version,
            "MCP server initialized"
        );

        transport
            .notify(
                crate::protocol::method::INITIALIZED,
                serde_json::Value::Null,
            )
            .await?;

        let tools = if init_result.capabilities.tools.is_some() {
            fetch_tools_legacy(&*transport).await?
        } else {
            Vec::new()
        };

        tracing::info!(
            server = server_name,
            tool_count = tools.len(),
            "MCP tools discovered"
        );

        Ok(Self {
            backend: ClientBackend::Legacy(transport),
            server_name: server_name.to_string(),
            server_info: init_result.server_info,
            capabilities: init_result.capabilities,
            tools,
        })
    }

    /// Connect to an MCP server over stdio via the official `rmcp` SDK.
    pub async fn connect_stdio(
        command: &str,
        args: &[String],
        env: Option<&HashMap<String, String>>,
        server_name: &str,
    ) -> crab_core::Result<Self> {
        let mut cmd = tokio::process::Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        if let Some(env) = env {
            cmd.envs(env);
        }

        let transport = TokioChildProcess::new(cmd).map_err(|e| {
            crab_core::Error::Other(format!("failed to spawn MCP child process: {e}"))
        })?;

        let service = rmcp::serve_client(rmcp::model::ClientInfo::default(), transport)
            .await
            .map_err(map_rmcp_error)?;

        Self::from_rmcp_service(service, server_name).await
    }

    /// Connect to a remote MCP HTTP endpoint via the official `rmcp` SDK.
    pub async fn connect_streamable_http(url: &str, server_name: &str) -> crab_core::Result<Self> {
        let transport = StreamableHttpClientTransport::from_uri(url.to_string());
        let service = rmcp::serve_client(rmcp::model::ClientInfo::default(), transport)
            .await
            .map_err(map_rmcp_error)?;

        Self::from_rmcp_service(service, server_name).await
    }

    async fn from_rmcp_service(
        service: RmcpClientService,
        server_name: &str,
    ) -> crab_core::Result<Self> {
        let peer_info = service.peer().peer_info().cloned().unwrap_or_default();

        tracing::info!(
            server = server_name,
            server_name = peer_info.server_info.name,
            server_version = peer_info.server_info.version,
            protocol_version = peer_info.protocol_version.to_string(),
            "MCP server initialized"
        );

        let tools = if peer_info.capabilities.tools.is_some() {
            fetch_tools_rmcp(service.peer()).await?
        } else {
            Vec::new()
        };

        tracing::info!(
            server = server_name,
            tool_count = tools.len(),
            "MCP tools discovered"
        );

        Ok(Self {
            backend: ClientBackend::Rmcp(service),
            server_name: server_name.to_string(),
            server_info: ServerInfo {
                name: peer_info.server_info.name,
                version: peer_info.server_info.version,
            },
            capabilities: convert_server_capabilities(&peer_info.capabilities),
            tools,
        })
    }

    /// Call a tool on the connected MCP server.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> crab_core::Result<ToolCallResult> {
        tracing::debug!(server = %self.server_name, tool = name, "calling MCP tool");

        match &self.backend {
            ClientBackend::Legacy(transport) => {
                call_tool_legacy(&**transport, name, arguments).await
            }
            ClientBackend::Rmcp(service) => call_tool_rmcp(service.peer(), name, arguments).await,
        }
    }

    /// List resources from the connected MCP server.
    pub async fn list_resources(&self) -> crab_core::Result<Vec<McpResource>> {
        match &self.backend {
            ClientBackend::Legacy(transport) => list_resources_legacy(&**transport).await,
            ClientBackend::Rmcp(service) => list_resources_rmcp(service.peer()).await,
        }
    }

    /// Read a resource from the connected MCP server.
    pub async fn read_resource(&self, uri: &str) -> crab_core::Result<ResourceReadResult> {
        match &self.backend {
            ClientBackend::Legacy(transport) => read_resource_legacy(&**transport, uri).await,
            ClientBackend::Rmcp(service) => read_resource_rmcp(service.peer(), uri).await,
        }
    }

    /// List prompts from the connected MCP server.
    pub async fn list_prompts(&self) -> crab_core::Result<Vec<McpPrompt>> {
        match &self.backend {
            ClientBackend::Legacy(transport) => list_prompts_legacy(&**transport).await,
            ClientBackend::Rmcp(service) => list_prompts_rmcp(service.peer()).await,
        }
    }

    /// Fetch a specific prompt's rendered messages from the connected MCP server.
    pub async fn get_prompt(
        &self,
        name: &str,
        arguments: HashMap<String, String>,
    ) -> crab_core::Result<PromptGetResult> {
        match &self.backend {
            ClientBackend::Legacy(transport) => {
                get_prompt_legacy(&**transport, name, arguments).await
            }
            ClientBackend::Rmcp(service) => get_prompt_rmcp(service.peer(), name, arguments).await,
        }
    }

    /// Send a `ping` request to the connected MCP server (liveness check).
    pub async fn ping(&self) -> crab_core::Result<()> {
        match &self.backend {
            ClientBackend::Legacy(transport) => ping_legacy(&**transport).await,
            ClientBackend::Rmcp(service) => ping_rmcp(service.peer()).await,
        }
    }

    /// Refresh the tool list from the server.
    pub async fn refresh_tools(&mut self) -> crab_core::Result<()> {
        self.tools = match &self.backend {
            ClientBackend::Legacy(transport) => fetch_tools_legacy(&**transport).await?,
            ClientBackend::Rmcp(service) => fetch_tools_rmcp(service.peer()).await?,
        };
        Ok(())
    }

    /// Close the connection to the MCP server.
    pub async fn close(&mut self) -> crab_core::Result<()> {
        match &mut self.backend {
            ClientBackend::Legacy(transport) => transport.close().await,
            ClientBackend::Rmcp(service) => {
                service.close().await.map(|_| ()).map_err(|e| {
                    crab_core::Error::Other(format!("failed to close rmcp service: {e}"))
                })
            }
        }
    }

    /// Get the server name (as configured by the user).
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// Get the server info returned during initialization.
    pub fn server_info(&self) -> &ServerInfo {
        &self.server_info
    }

    /// Get the server capabilities.
    pub fn capabilities(&self) -> &ServerCapabilities {
        &self.capabilities
    }

    /// Get the list of tools discovered from this server.
    pub fn tools(&self) -> &[McpToolDef] {
        &self.tools
    }
}

fn map_rmcp_error(error: impl std::fmt::Display) -> crab_core::Error {
    crab_core::Error::Other(format!("rmcp error: {error}"))
}

fn convert_server_capabilities(
    capabilities: &rmcp::model::ServerCapabilities,
) -> ServerCapabilities {
    let value = serde_json::to_value(capabilities).unwrap_or(Value::Null);
    ServerCapabilities {
        tools: value.get("tools").cloned().filter(|v| !v.is_null()),
        resources: value.get("resources").cloned().filter(|v| !v.is_null()),
        prompts: value.get("prompts").cloned().filter(|v| !v.is_null()),
    }
}

fn convert_tool(tool: rmcp::model::Tool) -> McpToolDef {
    McpToolDef {
        name: tool.name.into_owned(),
        description: tool
            .description
            .map(std::borrow::Cow::into_owned)
            .unwrap_or_default(),
        input_schema: Value::Object((*tool.input_schema).clone()),
    }
}

fn convert_prompt(prompt: rmcp::model::Prompt) -> McpPrompt {
    McpPrompt {
        name: prompt.name,
        description: prompt.description,
        arguments: prompt
            .arguments
            .unwrap_or_default()
            .into_iter()
            .map(|arg| PromptArgument {
                name: arg.name,
                description: arg.description,
                required: arg.required.unwrap_or(false),
            })
            .collect(),
    }
}

fn convert_resource(resource: &rmcp::model::Resource) -> McpResource {
    McpResource {
        uri: resource.uri.clone(),
        name: resource.name.clone(),
        description: resource.description.clone(),
        mime_type: resource.mime_type.clone(),
    }
}

fn convert_resource_content(content: rmcp::model::ResourceContents) -> ResourceContent {
    match content {
        rmcp::model::ResourceContents::TextResourceContents {
            uri,
            mime_type,
            text,
            ..
        } => ResourceContent {
            uri,
            mime_type,
            text: Some(text),
        },
        rmcp::model::ResourceContents::BlobResourceContents {
            uri,
            mime_type,
            blob,
            ..
        } => ResourceContent {
            uri,
            mime_type,
            text: Some(blob),
        },
    }
}

fn convert_tool_result_content(content: rmcp::model::Content) -> ToolResultContent {
    match content.raw {
        rmcp::model::RawContent::Text(text) => ToolResultContent::Text { text: text.text },
        rmcp::model::RawContent::Image(image) => ToolResultContent::Image {
            data: image.data,
            mime_type: image.mime_type,
        },
        rmcp::model::RawContent::Resource(resource) => ToolResultContent::Resource {
            resource: convert_resource_content(resource.resource),
        },
        rmcp::model::RawContent::Audio(audio) => ToolResultContent::Text {
            text: format!("[audio:{}]", audio.mime_type),
        },
        rmcp::model::RawContent::ResourceLink(resource) => ToolResultContent::Resource {
            resource: ResourceContent {
                uri: resource.uri,
                mime_type: resource.mime_type,
                text: resource.description.or(resource.title),
            },
        },
    }
}

fn value_to_json_object(value: Value) -> crab_core::Result<Map<String, Value>> {
    match value {
        Value::Object(map) => Ok(map),
        Value::Null => Ok(Map::new()),
        other => Err(crab_core::Error::Other(format!(
            "MCP tool arguments must be a JSON object, got {other}"
        ))),
    }
}

async fn fetch_tools_legacy(transport: &dyn Transport) -> crab_core::Result<Vec<McpToolDef>> {
    let mut all_tools = Vec::new();
    let mut cursor: Option<String> = None;

    loop {
        let params = cursor
            .as_ref()
            .map_or_else(|| json!({}), |c| json!({"cursor": c}));

        let req = JsonRpcRequest::new(crate::protocol::method::TOOLS_LIST, Some(params));
        let resp = transport.send(req).await?;
        let result_value = resp.into_result()?;

        if let Some(tools_arr) = result_value.get("tools") {
            let tools: Vec<McpToolDef> = serde_json::from_value(tools_arr.clone())
                .map_err(|e| crab_core::Error::Other(format!("failed to parse tools list: {e}")))?;
            all_tools.extend(tools);
        }

        cursor = result_value
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(String::from);

        if cursor.is_none() {
            break;
        }
    }

    Ok(all_tools)
}

async fn fetch_tools_rmcp(
    peer: &rmcp::Peer<rmcp::RoleClient>,
) -> crab_core::Result<Vec<McpToolDef>> {
    peer.list_all_tools()
        .await
        .map_err(map_rmcp_error)
        .map(|tools| tools.into_iter().map(convert_tool).collect())
}

async fn call_tool_legacy(
    transport: &dyn Transport,
    name: &str,
    arguments: serde_json::Value,
) -> crab_core::Result<ToolCallResult> {
    let params = ToolCallParams {
        name: name.to_string(),
        arguments,
    };

    let req = JsonRpcRequest::new(
        crate::protocol::method::TOOLS_CALL,
        Some(serde_json::to_value(&params).map_err(|e| {
            crab_core::Error::Other(format!("failed to serialize tool call params: {e}"))
        })?),
    );

    let resp = transport.send(req).await?;
    let result_value = resp.into_result()?;

    serde_json::from_value(result_value)
        .map_err(|e| crab_core::Error::Other(format!("failed to parse tool call result: {e}")))
}

async fn call_tool_rmcp(
    peer: &rmcp::Peer<rmcp::RoleClient>,
    name: &str,
    arguments: serde_json::Value,
) -> crab_core::Result<ToolCallResult> {
    let params = rmcp::model::CallToolRequestParams::new(name.to_string())
        .with_arguments(value_to_json_object(arguments)?);

    peer.call_tool(params)
        .await
        .map_err(map_rmcp_error)
        .map(|result| ToolCallResult {
            content: result
                .content
                .into_iter()
                .map(convert_tool_result_content)
                .collect(),
            is_error: result.is_error.unwrap_or(false),
        })
}

async fn list_resources_legacy(transport: &dyn Transport) -> crab_core::Result<Vec<McpResource>> {
    let req = JsonRpcRequest::new(crate::protocol::method::RESOURCES_LIST, Some(json!({})));
    let resp = transport.send(req).await?;
    let result_value = resp.into_result()?;

    Ok(result_value
        .get("resources")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default())
}

async fn list_resources_rmcp(
    peer: &rmcp::Peer<rmcp::RoleClient>,
) -> crab_core::Result<Vec<McpResource>> {
    peer.list_all_resources()
        .await
        .map_err(map_rmcp_error)
        .map(|resources| resources.iter().map(convert_resource).collect())
}

async fn read_resource_legacy(
    transport: &dyn Transport,
    uri: &str,
) -> crab_core::Result<ResourceReadResult> {
    let params = ResourceReadParams {
        uri: uri.to_string(),
    };

    let req = JsonRpcRequest::new(
        crate::protocol::method::RESOURCES_READ,
        Some(serde_json::to_value(&params).map_err(|e| {
            crab_core::Error::Other(format!("failed to serialize resource read params: {e}"))
        })?),
    );

    let resp = transport.send(req).await?;
    let result_value = resp.into_result()?;

    serde_json::from_value(result_value)
        .map_err(|e| crab_core::Error::Other(format!("failed to parse resource read result: {e}")))
}

async fn read_resource_rmcp(
    peer: &rmcp::Peer<rmcp::RoleClient>,
    uri: &str,
) -> crab_core::Result<ResourceReadResult> {
    let params = rmcp::model::ReadResourceRequestParams::new(uri.to_string());
    peer.read_resource(params)
        .await
        .map_err(map_rmcp_error)
        .map(|result| ResourceReadResult {
            contents: result
                .contents
                .into_iter()
                .map(convert_resource_content)
                .collect(),
        })
}

async fn list_prompts_legacy(transport: &dyn Transport) -> crab_core::Result<Vec<McpPrompt>> {
    let req = JsonRpcRequest::new(crate::protocol::method::PROMPTS_LIST, Some(json!({})));
    let resp = transport.send(req).await?;
    let result_value = resp.into_result()?;

    Ok(result_value
        .get("prompts")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default())
}

async fn list_prompts_rmcp(
    peer: &rmcp::Peer<rmcp::RoleClient>,
) -> crab_core::Result<Vec<McpPrompt>> {
    peer.list_all_prompts()
        .await
        .map_err(map_rmcp_error)
        .map(|prompts| prompts.into_iter().map(convert_prompt).collect())
}

async fn get_prompt_legacy(
    transport: &dyn Transport,
    name: &str,
    arguments: HashMap<String, String>,
) -> crab_core::Result<PromptGetResult> {
    let params = PromptGetParams {
        name: name.to_string(),
        arguments,
    };

    let req = JsonRpcRequest::new(
        crate::protocol::method::PROMPTS_GET,
        Some(serde_json::to_value(&params).map_err(|e| {
            crab_core::Error::Other(format!("failed to serialize prompt get params: {e}"))
        })?),
    );

    let resp = transport.send(req).await?;
    let result_value = resp.into_result()?;

    serde_json::from_value(result_value)
        .map_err(|e| crab_core::Error::Other(format!("failed to parse prompt get result: {e}")))
}

async fn get_prompt_rmcp(
    peer: &rmcp::Peer<rmcp::RoleClient>,
    name: &str,
    arguments: HashMap<String, String>,
) -> crab_core::Result<PromptGetResult> {
    let mut params = rmcp::model::GetPromptRequestParams::new(name.to_string());
    if !arguments.is_empty() {
        params = params.with_arguments(string_hashmap_to_json_object(arguments));
    }

    peer.get_prompt(params)
        .await
        .map_err(map_rmcp_error)
        .map(convert_prompt_get_result)
}

async fn ping_legacy(transport: &dyn Transport) -> crab_core::Result<()> {
    let req = JsonRpcRequest::new(crate::protocol::method::PING, Some(json!({})));
    let resp = transport.send(req).await?;
    resp.into_result().map(|_| ())
}

async fn ping_rmcp(peer: &rmcp::Peer<rmcp::RoleClient>) -> crab_core::Result<()> {
    let request = rmcp::model::ClientRequest::PingRequest(rmcp::model::PingRequest::default());
    peer.send_request(request)
        .await
        .map(|_| ())
        .map_err(map_rmcp_error)
}

fn string_hashmap_to_json_object(map: HashMap<String, String>) -> Map<String, Value> {
    map.into_iter()
        .map(|(k, v)| (k, Value::String(v)))
        .collect()
}

fn convert_prompt_get_result(result: rmcp::model::GetPromptResult) -> PromptGetResult {
    PromptGetResult {
        description: result.description,
        messages: result
            .messages
            .into_iter()
            .map(convert_prompt_message)
            .collect(),
    }
}

fn convert_prompt_message(msg: rmcp::model::PromptMessage) -> PromptMessage {
    let role = match msg.role {
        rmcp::model::PromptMessageRole::User => "user".to_string(),
        rmcp::model::PromptMessageRole::Assistant => "assistant".to_string(),
    };
    PromptMessage {
        role,
        content: convert_prompt_message_content(msg.content),
    }
}

fn convert_prompt_message_content(
    content: rmcp::model::PromptMessageContent,
) -> PromptMessageContent {
    match content {
        rmcp::model::PromptMessageContent::Text { text } => PromptMessageContent::Text { text },
        rmcp::model::PromptMessageContent::Resource { resource } => {
            PromptMessageContent::Resource {
                resource: convert_resource_content(resource.raw.resource),
            }
        }
        rmcp::model::PromptMessageContent::Image { image } => PromptMessageContent::Text {
            text: format!("[image:{}]", image.raw.mime_type),
        },
        rmcp::model::PromptMessageContent::ResourceLink { link } => {
            PromptMessageContent::Resource {
                resource: ResourceContent {
                    uri: link.raw.uri,
                    mime_type: link.raw.mime_type,
                    text: link.raw.description.or(link.raw.title),
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::JsonRpcResponse;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct MockTransport {
        call_count: AtomicUsize,
        responses: tokio::sync::Mutex<Vec<serde_json::Value>>,
    }

    impl MockTransport {
        fn new(responses: Vec<serde_json::Value>) -> Self {
            Self {
                call_count: AtomicUsize::new(0),
                responses: tokio::sync::Mutex::new(responses),
            }
        }
    }

    impl Transport for MockTransport {
        fn send(
            &self,
            req: JsonRpcRequest,
        ) -> Pin<Box<dyn Future<Output = crab_core::Result<JsonRpcResponse>> + Send + '_>> {
            Box::pin(async move {
                let idx = self.call_count.fetch_add(1, Ordering::Relaxed);
                let result = {
                    let responses = self.responses.lock().await;
                    responses
                        .get(idx)
                        .cloned()
                        .unwrap_or(serde_json::Value::Null)
                };

                Ok(JsonRpcResponse {
                    jsonrpc: "2.0".into(),
                    id: req.id,
                    result: Some(result),
                    error: None,
                })
            })
        }

        fn notify(
            &self,
            _method: &str,
            _params: serde_json::Value,
        ) -> Pin<Box<dyn Future<Output = crab_core::Result<()>> + Send + '_>> {
            Box::pin(async { Ok(()) })
        }

        fn close(&self) -> Pin<Box<dyn Future<Output = crab_core::Result<()>> + Send + '_>> {
            Box::pin(async { Ok(()) })
        }
    }

    #[tokio::test]
    async fn connect_performs_handshake() {
        let transport = MockTransport::new(vec![
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "test-server", "version": "1.0"}
            }),
            json!({
                "tools": [
                    {
                        "name": "read_file",
                        "description": "Read a file",
                        "inputSchema": {"type": "object"}
                    }
                ]
            }),
        ]);

        let client = McpClient::connect(Box::new(transport), "test")
            .await
            .unwrap();
        assert_eq!(client.server_name(), "test");
        assert_eq!(client.server_info().name, "test-server");
        assert_eq!(client.tools().len(), 1);
        assert_eq!(client.tools()[0].name, "read_file");
    }

    #[tokio::test]
    async fn connect_without_tools_capability() {
        let transport = MockTransport::new(vec![json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "serverInfo": {"name": "no-tools", "version": "1.0"}
        })]);

        let client = McpClient::connect(Box::new(transport), "test")
            .await
            .unwrap();
        assert!(client.tools().is_empty());
    }

    #[tokio::test]
    async fn get_prompt_parses_messages() {
        let transport = MockTransport::new(vec![
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {"prompts": {}},
                "serverInfo": {"name": "s", "version": "1.0"}
            }),
            json!({
                "description": "A greeting",
                "messages": [
                    {
                        "role": "user",
                        "content": {"type": "text", "text": "Hello, world"}
                    }
                ]
            }),
        ]);

        let client = McpClient::connect(Box::new(transport), "test")
            .await
            .unwrap();
        let mut args = HashMap::new();
        args.insert("name".into(), "world".into());
        let result = client.get_prompt("greet", args).await.unwrap();

        assert_eq!(result.description.as_deref(), Some("A greeting"));
        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.messages[0].role, "user");
        match &result.messages[0].content {
            PromptMessageContent::Text { text } => assert_eq!(text, "Hello, world"),
            PromptMessageContent::Resource { .. } => panic!("expected text content"),
        }
    }

    #[tokio::test]
    async fn ping_returns_ok_on_success() {
        let transport = MockTransport::new(vec![
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "serverInfo": {"name": "s", "version": "1.0"}
            }),
            json!({}),
        ]);

        let client = McpClient::connect(Box::new(transport), "test")
            .await
            .unwrap();
        client.ping().await.unwrap();
    }

    #[tokio::test]
    async fn call_tool_sends_correct_params() {
        let transport = MockTransport::new(vec![
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "serverInfo": {"name": "s", "version": "1.0"}
            }),
            json!({
                "content": [{"type": "text", "text": "hello world"}],
                "isError": false
            }),
        ]);

        let client = McpClient::connect(Box::new(transport), "test")
            .await
            .unwrap();
        let result = client
            .call_tool("echo", json!({"message": "hello"}))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(result.content.len(), 1);
    }
}
