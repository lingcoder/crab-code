use serde_json::json;

use crate::protocol::{
    InitializeParams, InitializeResult, JsonRpcRequest, McpPrompt, McpResource, McpToolDef,
    ResourceReadParams, ResourceReadResult, ServerCapabilities, ServerInfo, ToolCallParams,
    ToolCallResult,
};
use crate::transport::Transport;

/// MCP client — connects to an external MCP server, discovers tools/resources,
/// and forwards tool calls.
pub struct McpClient {
    transport: Box<dyn Transport>,
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
    /// Connect to an MCP server: perform the initialize handshake and discover tools.
    ///
    /// 1. Send `initialize` request with client capabilities.
    /// 2. Receive server capabilities.
    /// 3. Send `notifications/initialized` notification.
    /// 4. Fetch `tools/list` if server supports tools.
    pub async fn connect(
        transport: Box<dyn Transport>,
        server_name: &str,
    ) -> crab_common::Result<Self> {
        let params = InitializeParams::default();
        let req = JsonRpcRequest::new(
            crate::protocol::method::INITIALIZE,
            Some(serde_json::to_value(&params).map_err(|e| {
                crab_common::Error::Other(format!("failed to serialize initialize params: {e}"))
            })?),
        );

        tracing::info!(server = server_name, "initializing MCP connection");

        // Step 1: Send initialize request.
        let resp = transport.send(req).await?;
        let result_value = resp.into_result()?;

        let init_result: InitializeResult = serde_json::from_value(result_value).map_err(|e| {
            crab_common::Error::Other(format!("failed to parse initialize result: {e}"))
        })?;

        tracing::info!(
            server = server_name,
            server_name = init_result.server_info.name,
            server_version = init_result.server_info.version,
            protocol_version = init_result.protocol_version,
            "MCP server initialized"
        );

        // Step 2: Send initialized notification.
        transport
            .notify(
                crate::protocol::method::INITIALIZED,
                serde_json::Value::Null,
            )
            .await?;

        // Step 3: Fetch tools if server supports them.
        let tools = if init_result.capabilities.tools.is_some() {
            fetch_tools(&*transport).await?
        } else {
            Vec::new()
        };

        tracing::info!(
            server = server_name,
            tool_count = tools.len(),
            "MCP tools discovered"
        );

        Ok(Self {
            transport,
            server_name: server_name.to_string(),
            server_info: init_result.server_info,
            capabilities: init_result.capabilities,
            tools,
        })
    }

    /// Call a tool on the connected MCP server.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> crab_common::Result<ToolCallResult> {
        let params = ToolCallParams {
            name: name.to_string(),
            arguments,
        };

        let req = JsonRpcRequest::new(
            crate::protocol::method::TOOLS_CALL,
            Some(serde_json::to_value(&params).map_err(|e| {
                crab_common::Error::Other(format!("failed to serialize tool call params: {e}"))
            })?),
        );

        tracing::debug!(server = %self.server_name, tool = name, "calling MCP tool");
        let resp = self.transport.send(req).await?;
        let result_value = resp.into_result()?;

        serde_json::from_value(result_value).map_err(|e| {
            crab_common::Error::Other(format!("failed to parse tool call result: {e}"))
        })
    }

    /// List resources from the connected MCP server.
    pub async fn list_resources(&self) -> crab_common::Result<Vec<McpResource>> {
        let req = JsonRpcRequest::new(crate::protocol::method::RESOURCES_LIST, Some(json!({})));

        let resp = self.transport.send(req).await?;
        let result_value = resp.into_result()?;

        let resources: Vec<McpResource> = result_value
            .get("resources")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        Ok(resources)
    }

    /// Read a resource from the connected MCP server.
    pub async fn read_resource(&self, uri: &str) -> crab_common::Result<ResourceReadResult> {
        let params = ResourceReadParams {
            uri: uri.to_string(),
        };

        let req = JsonRpcRequest::new(
            crate::protocol::method::RESOURCES_READ,
            Some(serde_json::to_value(&params).map_err(|e| {
                crab_common::Error::Other(format!("failed to serialize resource read params: {e}"))
            })?),
        );

        let resp = self.transport.send(req).await?;
        let result_value = resp.into_result()?;

        serde_json::from_value(result_value).map_err(|e| {
            crab_common::Error::Other(format!("failed to parse resource read result: {e}"))
        })
    }

    /// List prompts from the connected MCP server.
    pub async fn list_prompts(&self) -> crab_common::Result<Vec<McpPrompt>> {
        let req = JsonRpcRequest::new(crate::protocol::method::PROMPTS_LIST, Some(json!({})));

        let resp = self.transport.send(req).await?;
        let result_value = resp.into_result()?;

        let prompts: Vec<McpPrompt> = result_value
            .get("prompts")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        Ok(prompts)
    }

    /// Refresh the tool list from the server.
    pub async fn refresh_tools(&mut self) -> crab_common::Result<()> {
        self.tools = fetch_tools(&*self.transport).await?;
        Ok(())
    }

    /// Close the connection to the MCP server.
    pub async fn close(self) -> crab_common::Result<()> {
        self.transport.close().await
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

    /// Get a reference to the underlying transport.
    pub fn transport(&self) -> &dyn Transport {
        &*self.transport
    }
}

/// Fetch all tools from an MCP server, handling pagination.
async fn fetch_tools(transport: &dyn Transport) -> crab_common::Result<Vec<McpToolDef>> {
    let mut all_tools = Vec::new();
    let mut cursor: Option<String> = None;

    loop {
        let params = cursor
            .as_ref()
            .map_or_else(|| json!({}), |c| json!({"cursor": c}));

        let req = JsonRpcRequest::new(crate::protocol::method::TOOLS_LIST, Some(params));
        let resp = transport.send(req).await?;
        let result_value = resp.into_result()?;

        // Parse tools from the response.
        if let Some(tools_arr) = result_value.get("tools") {
            let tools: Vec<McpToolDef> =
                serde_json::from_value(tools_arr.clone()).map_err(|e| {
                    crab_common::Error::Other(format!("failed to parse tools list: {e}"))
                })?;
            all_tools.extend(tools);
        }

        // Check for pagination cursor.
        cursor = result_value
            .get("nextCursor")
            .and_then(|v| v.as_str())
            .map(String::from);

        if cursor.is_none() {
            break;
        }
    }

    Ok(all_tools)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::JsonRpcResponse;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A mock transport for testing the client.
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
        ) -> Pin<Box<dyn Future<Output = crab_common::Result<JsonRpcResponse>> + Send + '_>>
        {
            Box::pin(async move {
                let idx = self.call_count.fetch_add(1, Ordering::Relaxed);
                let responses = self.responses.lock().await;
                let result = responses
                    .get(idx)
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);

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
        ) -> Pin<Box<dyn Future<Output = crab_common::Result<()>> + Send + '_>> {
            Box::pin(async { Ok(()) })
        }

        fn close(&self) -> Pin<Box<dyn Future<Output = crab_common::Result<()>> + Send + '_>> {
            Box::pin(async { Ok(()) })
        }
    }

    #[tokio::test]
    async fn connect_performs_handshake() {
        let transport = MockTransport::new(vec![
            // Response to initialize
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "test-server", "version": "1.0"}
            }),
            // Response to tools/list
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
    async fn call_tool_sends_correct_params() {
        let transport = MockTransport::new(vec![
            // initialize
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "serverInfo": {"name": "s", "version": "1.0"}
            }),
            // call_tool response
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
