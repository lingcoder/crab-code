use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::client::McpClient;
use crate::discovery::{McpServerConfig, connect_server};
use crate::protocol::McpToolDef;

/// Manages the lifecycle of multiple MCP server connections.
///
/// The session layer creates one `McpManager` at startup, passing in the
/// `mcpServers` config from settings. The manager connects to each server,
/// discovers tools, and exposes them for registration into the `ToolRegistry`.
///
/// On shutdown, `close_all()` terminates every server connection gracefully.
pub struct McpManager {
    /// Connected MCP clients keyed by server name.
    clients: HashMap<String, Arc<Mutex<McpClient>>>,
}

/// A tool discovered from an MCP server, carrying enough info for registration.
#[derive(Clone)]
pub struct DiscoveredTool {
    /// The MCP server name (from config).
    pub server_name: String,
    /// The tool definition from the server.
    pub tool_def: McpToolDef,
    /// Shared client handle for forwarding calls.
    pub client: Arc<Mutex<McpClient>>,
}

impl McpManager {
    /// Create an empty manager (no connections yet).
    #[must_use]
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
        }
    }

    /// Connect to all configured MCP servers **concurrently**.
    ///
    /// Servers that fail to connect are logged and skipped — a single broken
    /// server should not prevent the rest from working.
    pub async fn connect_all(&mut self, configs: &[McpServerConfig]) -> Vec<String> {
        use futures::future::join_all;

        let results = join_all(configs.iter().map(connect_server)).await;
        let mut failed = Vec::new();

        for (config, result) in configs.iter().zip(results) {
            match result {
                Ok(client) => {
                    tracing::info!(
                        server = config.name.as_str(),
                        tools = client.tools().len(),
                        "MCP server connected"
                    );
                    self.clients
                        .insert(config.name.clone(), Arc::new(Mutex::new(client)));
                }
                Err(e) => {
                    tracing::warn!(
                        server = config.name.as_str(),
                        error = %e,
                        "failed to connect to MCP server"
                    );
                    failed.push(config.name.clone());
                }
            }
        }

        failed
    }

    /// Connect to a single MCP server and add it to the manager.
    pub async fn connect_one(&mut self, config: &McpServerConfig) -> crab_core::Result<()> {
        let client = connect_server(config).await?;
        self.clients
            .insert(config.name.clone(), Arc::new(Mutex::new(client)));
        Ok(())
    }

    /// Get all discovered tools from all connected servers.
    ///
    /// Returns `DiscoveredTool` structs that carry enough context to create
    /// `McpToolAdapter` instances for the `ToolRegistry`.
    pub async fn discovered_tools(&self) -> Vec<DiscoveredTool> {
        let mut tools = Vec::new();

        for (server_name, client_arc) in &self.clients {
            let client = client_arc.lock().await;
            for tool_def in client.tools() {
                tools.push(DiscoveredTool {
                    server_name: server_name.clone(),
                    tool_def: tool_def.clone(),
                    client: Arc::clone(client_arc),
                });
            }
        }

        tools
    }

    /// Get the shared client handle for a specific server.
    #[must_use]
    pub fn get_client(&self, server_name: &str) -> Option<&Arc<Mutex<McpClient>>> {
        self.clients.get(server_name)
    }

    /// Get the names of all connected servers.
    #[must_use]
    pub fn server_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.clients.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    }

    /// Number of connected servers.
    #[must_use]
    pub fn server_count(&self) -> usize {
        self.clients.len()
    }

    /// Refresh the tool list for a specific server.
    pub async fn refresh_tools(&self, server_name: &str) -> crab_core::Result<()> {
        let client_arc = self.clients.get(server_name).ok_or_else(|| {
            crab_core::Error::Other(format!("MCP server '{server_name}' not connected"))
        })?;
        let mut client = client_arc.lock().await;
        client.refresh_tools().await
    }

    /// Disconnect a specific server.
    pub async fn disconnect(&mut self, server_name: &str) -> crab_core::Result<()> {
        if let Some(client_arc) = self.clients.remove(server_name) {
            // Tool adapters may still hold a client reference, so shutdown
            // must go through the shared mutex instead of reaching into a
            // backend-specific transport.
            client_arc.lock().await.close().await?;
            tracing::info!(server = server_name, "MCP server disconnected");
        }
        Ok(())
    }

    /// Start all MCP servers from a `mcpServers` settings value.
    ///
    /// Convenience method that parses the config and connects to all servers
    /// in one call. Returns the names of servers that failed to connect.
    pub async fn start_all(
        &mut self,
        mcp_servers_value: &serde_json::Value,
    ) -> crab_core::Result<Vec<String>> {
        let configs = crate::discovery::parse_mcp_servers(mcp_servers_value)?;
        let failed = self.connect_all(&configs).await;
        Ok(failed)
    }

    /// Restart a specific server by disconnecting and reconnecting.
    pub async fn restart_server(&mut self, config: &McpServerConfig) -> crab_core::Result<()> {
        self.disconnect(&config.name).await?;
        self.connect_one(config).await
    }

    /// Close all MCP server connections.
    pub async fn close_all(&mut self) {
        let names: Vec<String> = self.clients.keys().cloned().collect();
        for name in names {
            if let Err(e) = self.disconnect(&name).await {
                tracing::warn!(
                    server = name.as_str(),
                    error = %e,
                    "error closing MCP server"
                );
            }
        }
    }
}

impl Default for McpManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_manager_is_empty() {
        let mgr = McpManager::new();
        assert_eq!(mgr.server_count(), 0);
        assert!(mgr.server_names().is_empty());
        assert!(mgr.get_client("anything").is_none());
    }

    #[test]
    fn default_is_new() {
        let mgr = McpManager::default();
        assert_eq!(mgr.server_count(), 0);
    }

    #[tokio::test]
    async fn discovered_tools_empty_when_no_servers() {
        let mgr = McpManager::new();
        let tools = mgr.discovered_tools().await;
        assert!(tools.is_empty());
    }

    #[tokio::test]
    async fn disconnect_nonexistent_is_ok() {
        let mut mgr = McpManager::new();
        let result = mgr.disconnect("nonexistent").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn refresh_tools_nonexistent_is_error() {
        let mgr = McpManager::new();
        let result = mgr.refresh_tools("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn close_all_empty_is_noop() {
        let mut mgr = McpManager::new();
        mgr.close_all().await;
        assert_eq!(mgr.server_count(), 0);
    }

    #[tokio::test]
    async fn start_all_with_empty_object() {
        let mut mgr = McpManager::new();
        let failed = mgr.start_all(&serde_json::json!({})).await.unwrap();
        assert!(failed.is_empty());
        assert_eq!(mgr.server_count(), 0);
    }

    #[tokio::test]
    async fn start_all_with_invalid_value() {
        let mut mgr = McpManager::new();
        let result = mgr.start_all(&serde_json::json!("not an object")).await;
        assert!(result.is_err());
    }

    /// Helper to create a manager with a mock client injected directly.
    async fn manager_with_mock_client() -> McpManager {
        use crate::protocol::{JsonRpcRequest, JsonRpcResponse};
        use crate::transport::Transport;
        use std::future::Future;
        use std::pin::Pin;
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct MockTransport {
            call_count: AtomicUsize,
            responses: tokio::sync::Mutex<Vec<serde_json::Value>>,
        }

        impl Transport for MockTransport {
            fn send(
                &self,
                req: JsonRpcRequest,
            ) -> Pin<Box<dyn Future<Output = crab_core::Result<JsonRpcResponse>> + Send + '_>>
            {
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

        let transport = MockTransport {
            call_count: AtomicUsize::new(0),
            responses: tokio::sync::Mutex::new(vec![
                // initialize response
                serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "mock-server", "version": "1.0"}
                }),
                // tools/list response
                serde_json::json!({
                    "tools": [
                        {
                            "name": "read_file",
                            "description": "Read a file",
                            "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}}}
                        },
                        {
                            "name": "write_file",
                            "description": "Write a file",
                            "inputSchema": {"type": "object"}
                        }
                    ]
                }),
            ]),
        };

        let client = McpClient::connect(Box::new(transport), "test-server")
            .await
            .unwrap();

        let mut mgr = McpManager::new();
        mgr.clients
            .insert("test-server".into(), Arc::new(Mutex::new(client)));
        mgr
    }

    #[tokio::test]
    async fn manager_with_mock_discovers_tools() {
        let mgr = manager_with_mock_client().await;

        assert_eq!(mgr.server_count(), 1);
        assert_eq!(mgr.server_names(), vec!["test-server"]);
        assert!(mgr.get_client("test-server").is_some());

        let tools = mgr.discovered_tools().await;
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].server_name, "test-server");
        // Tools are in the order the server returned them
        let names: Vec<&str> = tools.iter().map(|t| t.tool_def.name.as_str()).collect();
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"write_file"));
    }

    #[tokio::test]
    async fn manager_disconnect_removes_server() {
        let mut mgr = manager_with_mock_client().await;
        assert_eq!(mgr.server_count(), 1);

        mgr.disconnect("test-server").await.unwrap();
        assert_eq!(mgr.server_count(), 0);
        assert!(mgr.get_client("test-server").is_none());
    }

    #[tokio::test]
    async fn manager_close_all_clears_everything() {
        let mut mgr = manager_with_mock_client().await;
        assert_eq!(mgr.server_count(), 1);

        mgr.close_all().await;
        assert_eq!(mgr.server_count(), 0);
    }
}
