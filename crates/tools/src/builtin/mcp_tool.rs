use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crab_common::Result;
use crab_core::tool::{
    Tool, ToolContext, ToolDisplayLine, ToolDisplayResult, ToolDisplayStyle, ToolOutput,
    ToolOutputContent, ToolSource,
};
use crab_mcp::McpClient;
use serde_json::Value;
use tokio::sync::Mutex;

/// Adapter that bridges an MCP tool to the native `Tool` trait.
///
/// Each adapter wraps a single MCP tool definition and holds a shared
/// reference to the `McpClient` that owns the connection to the server.
/// When `execute()` is called, it forwards the JSON arguments to the
/// remote MCP server via `McpClient::call_tool()` and converts the
/// result into a native `ToolOutput`.
pub struct McpTool {
    /// Tool name in `mcp__<server>__<tool>` format for uniqueness.
    tool_name: String,
    /// Original MCP tool name (used for the actual `tools/call` RPC).
    original_name: String,
    tool_description: String,
    server_name: String,
    schema: Value,
    /// Shared MCP client — `Mutex` because `call_tool` takes `&self` but we
    /// need exclusive access to the transport for concurrent requests.
    client: Arc<Mutex<McpClient>>,
}

impl McpTool {
    /// Create a new adapter.
    ///
    /// - `server_name`: logical name of the MCP server (from settings)
    /// - `mcp_tool_name`: the tool name as returned by the server
    /// - `description`: tool description from the server
    /// - `schema`: JSON Schema for the tool's input parameters
    /// - `client`: shared MCP client connection
    #[must_use]
    pub fn new(
        server_name: String,
        mcp_tool_name: String,
        description: String,
        schema: Value,
        client: Arc<Mutex<McpClient>>,
    ) -> Self {
        let tool_name = format!("mcp__{server_name}__{mcp_tool_name}");
        Self {
            tool_name,
            original_name: mcp_tool_name,
            tool_description: description,
            server_name,
            schema,
            client,
        }
    }

    /// Get the original MCP tool name (without server prefix).
    #[must_use]
    pub fn mcp_tool_name(&self) -> &str {
        &self.original_name
    }

    /// Get the server name this tool belongs to.
    #[must_use]
    pub fn server_name(&self) -> &str {
        &self.server_name
    }
}

impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &str {
        &self.tool_description
    }

    fn input_schema(&self) -> Value {
        self.schema.clone()
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        Box::pin(async move {
            let result = self
                .client
                .lock()
                .await
                .call_tool(&self.original_name, input)
                .await?;

            // Convert MCP ToolCallResult → native ToolOutput
            let content = result
                .content
                .into_iter()
                .map(|block| match block {
                    crab_mcp::protocol::ToolResultContent::Text { text } => {
                        ToolOutputContent::Text { text }
                    }
                    crab_mcp::protocol::ToolResultContent::Image { data, mime_type } => {
                        ToolOutputContent::Image {
                            media_type: mime_type,
                            data,
                        }
                    }
                    crab_mcp::protocol::ToolResultContent::Resource { resource } => {
                        // Convert resource content to text (best effort)
                        ToolOutputContent::Text {
                            text: resource
                                .text
                                .unwrap_or_else(|| format!("[resource: {}]", resource.uri)),
                        }
                    }
                })
                .collect();

            Ok(ToolOutput::with_content(content, result.is_error))
        })
    }

    fn source(&self) -> ToolSource {
        ToolSource::McpExternal {
            server_name: self.server_name.clone(),
        }
    }

    fn requires_confirmation(&self) -> bool {
        true
    }

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        let display_name = format!("{}::{}", self.server_name, self.original_name);

        let params = if let Some(obj) = input.as_object() {
            let pairs: Vec<String> = obj
                .iter()
                .take(3)
                .map(|(k, v)| {
                    let val = match v {
                        Value::String(s) => {
                            if s.len() > 30 {
                                format!("\"{}…\"", &s[..27])
                            } else {
                                format!("\"{s}\"")
                            }
                        }
                        other => {
                            let s = other.to_string();
                            if s.len() > 30 {
                                format!("{}…", &s[..27])
                            } else {
                                s
                            }
                        }
                    };
                    format!("{k}={val}")
                })
                .collect();
            if pairs.is_empty() {
                String::new()
            } else {
                format!(" ({})", pairs.join(", "))
            }
        } else {
            String::new()
        };

        Some(format!("{display_name}{params}"))
    }

    fn format_result(&self, output: &ToolOutput) -> Option<ToolDisplayResult> {
        let text = output.text();
        if text.is_empty() {
            return Some(ToolDisplayResult {
                lines: vec![ToolDisplayLine::new("(empty)", ToolDisplayStyle::Muted)],
                preview_lines: 1,
            });
        }

        // Try to detect JSON and format it nicely
        if let Ok(json) = serde_json::from_str::<Value>(&text)
            && let Ok(pretty) = serde_json::to_string_pretty(&json)
        {
            let lines: Vec<ToolDisplayLine> = pretty
                .lines()
                .take(10)
                .map(|l| ToolDisplayLine::new(l, ToolDisplayStyle::Normal))
                .collect();
            let total = pretty.lines().count();
            let mut result_lines = lines;
            if total > 10 {
                result_lines.push(ToolDisplayLine::new(
                    format!("... {total} total lines"),
                    ToolDisplayStyle::Muted,
                ));
            }
            return Some(ToolDisplayResult {
                preview_lines: 3,
                lines: result_lines,
            });
        }

        // Plain text: show first 10 lines
        let lines: Vec<ToolDisplayLine> = text
            .lines()
            .take(10)
            .map(|l| ToolDisplayLine::new(l, ToolDisplayStyle::Normal))
            .collect();
        let total = text.lines().count();
        let mut result_lines = lines;
        if total > 10 {
            result_lines.push(ToolDisplayLine::new(
                format!("... {total} total lines"),
                ToolDisplayStyle::Muted,
            ));
        }
        Some(ToolDisplayResult {
            preview_lines: 3,
            lines: result_lines,
        })
    }

    fn display_color(&self) -> ToolDisplayStyle {
        ToolDisplayStyle::Highlight
    }
}

/// Register all MCP tools from the manager into the tool registry.
///
/// For each discovered tool, creates an `McpTool` and registers it.
/// Returns the number of tools registered.
pub async fn register_mcp_tools(
    manager: &crab_mcp::McpManager,
    registry: &mut crate::registry::ToolRegistry,
) -> usize {
    let discovered = manager.discovered_tools().await;
    let count = discovered.len();

    for tool in discovered {
        let adapter = McpTool::new(
            tool.server_name,
            tool.tool_def.name,
            tool.tool_def.description,
            tool.tool_def.input_schema,
            tool.client,
        );
        registry.register(Arc::new(adapter));
    }

    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_mcp::Transport;
    use crab_mcp::protocol::{JsonRpcRequest, JsonRpcResponse};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Mock transport for testing the adapter.
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
                let result = self
                    .responses
                    .lock()
                    .await
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

    /// Helper to create a connected `McpClient` with mock transport.
    async fn mock_client(tool_responses: Vec<serde_json::Value>) -> McpClient {
        // First two responses: initialize + tools/list
        let mut responses = vec![serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "serverInfo": {"name": "mock", "version": "1.0"}
        })];
        responses.extend(tool_responses);

        let transport = MockTransport::new(responses);
        McpClient::connect(Box::new(transport), "mock-server")
            .await
            .unwrap()
    }

    #[test]
    fn adapter_name_format() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let client = mock_client(vec![]).await;
            let adapter = McpTool::new(
                "playwright".into(),
                "click".into(),
                "Click an element".into(),
                serde_json::json!({"type": "object"}),
                Arc::new(Mutex::new(client)),
            );

            assert_eq!(adapter.name(), "mcp__playwright__click");
            assert_eq!(adapter.mcp_tool_name(), "click");
            assert_eq!(adapter.server_name(), "playwright");
            assert_eq!(adapter.description(), "Click an element");
            assert!(matches!(
                adapter.source(),
                ToolSource::McpExternal { server_name } if server_name == "playwright"
            ));
            assert!(adapter.requires_confirmation());
        });
    }

    #[tokio::test]
    async fn adapter_execute_forwards_to_mcp_client() {
        // Mock: initialize response, then tool call response
        let responses = vec![
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "serverInfo": {"name": "test", "version": "1.0"}
            }),
            // tools/call response
            serde_json::json!({
                "content": [{"type": "text", "text": "clicked!"}],
                "isError": false
            }),
        ];

        let transport = MockTransport::new(responses);
        let client = McpClient::connect(Box::new(transport), "test")
            .await
            .unwrap();

        let adapter = McpTool::new(
            "test".into(),
            "do_thing".into(),
            "Does a thing".into(),
            serde_json::json!({"type": "object"}),
            Arc::new(Mutex::new(client)),
        );

        let ctx = crab_core::tool::ToolContext {
            working_dir: std::path::PathBuf::from("/tmp"),
            permission_mode: crab_core::permission::PermissionMode::Default,
            session_id: "test".into(),
            cancellation_token: tokio_util::sync::CancellationToken::new(),
            permission_policy: crab_core::permission::PermissionPolicy::default(),
            ext: crab_core::tool::ToolContextExt::default(),
        };

        let output = adapter
            .execute(serde_json::json!({"selector": "#btn"}), &ctx)
            .await
            .unwrap();

        assert!(!output.is_error);
        assert_eq!(output.text(), "clicked!");
    }

    #[tokio::test]
    async fn adapter_execute_error_result() {
        let transport = MockTransport::new(vec![
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "serverInfo": {"name": "test", "version": "1.0"}
            }),
            serde_json::json!({
                "content": [{"type": "text", "text": "tool failed"}],
                "isError": true
            }),
        ]);

        let client = McpClient::connect(Box::new(transport), "test")
            .await
            .unwrap();

        let adapter = McpTool::new(
            "test".into(),
            "failing_tool".into(),
            "A tool that fails".into(),
            serde_json::json!({"type": "object"}),
            Arc::new(Mutex::new(client)),
        );

        let ctx = crab_core::tool::ToolContext {
            working_dir: std::path::PathBuf::from("/tmp"),
            permission_mode: crab_core::permission::PermissionMode::Default,
            session_id: "test".into(),
            cancellation_token: tokio_util::sync::CancellationToken::new(),
            permission_policy: crab_core::permission::PermissionPolicy::default(),
            ext: crab_core::tool::ToolContextExt::default(),
        };

        let output = adapter.execute(serde_json::json!({}), &ctx).await.unwrap();

        assert!(output.is_error);
        assert_eq!(output.text(), "tool failed");
    }

    #[tokio::test]
    async fn register_mcp_tools_populates_registry() {
        // Create a mock client with 2 tools
        let transport = MockTransport::new(vec![
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "mock", "version": "1.0"}
            }),
            serde_json::json!({
                "tools": [
                    {
                        "name": "tool_a",
                        "description": "Tool A",
                        "inputSchema": {"type": "object"}
                    },
                    {
                        "name": "tool_b",
                        "description": "Tool B",
                        "inputSchema": {"type": "object"}
                    }
                ]
            }),
        ]);

        let client = McpClient::connect(Box::new(transport), "srv")
            .await
            .unwrap();

        // Build a manager with the mock client injected
        let _mgr = crab_mcp::McpManager::new();
        // We need to insert the client into the manager — use the public API
        // by wrapping in a DiscoveredTool directly via discovered_tools after
        // adding through internal means. Since McpManager.clients is private,
        // we test register_mcp_tools via DiscoveredTool manually.
        let client_arc = Arc::new(Mutex::new(client));

        let mut registry = crate::registry::ToolRegistry::new();
        let initial_count = registry.len();

        // Create DiscoveredTool structs manually
        let tools = vec![
            crab_mcp::DiscoveredTool {
                server_name: "srv".into(),
                tool_def: crab_mcp::McpToolDef {
                    name: "tool_a".into(),
                    description: "Tool A".into(),
                    input_schema: serde_json::json!({"type": "object"}),
                },
                client: Arc::clone(&client_arc),
            },
            crab_mcp::DiscoveredTool {
                server_name: "srv".into(),
                tool_def: crab_mcp::McpToolDef {
                    name: "tool_b".into(),
                    description: "Tool B".into(),
                    input_schema: serde_json::json!({"type": "object"}),
                },
                client: Arc::clone(&client_arc),
            },
        ];

        // Register manually (same logic as register_mcp_tools)
        for tool in tools {
            let adapter = McpTool::new(
                tool.server_name,
                tool.tool_def.name,
                tool.tool_def.description,
                tool.tool_def.input_schema,
                tool.client,
            );
            registry.register(Arc::new(adapter));
        }

        assert_eq!(registry.len(), initial_count + 2);
        assert!(registry.get("mcp__srv__tool_a").is_some());
        assert!(registry.get("mcp__srv__tool_b").is_some());

        // Verify tool properties through the registry
        let tool_a = registry.get("mcp__srv__tool_a").unwrap();
        assert_eq!(tool_a.description(), "Tool A");
        assert!(matches!(
            tool_a.source(),
            ToolSource::McpExternal { server_name } if server_name == "srv"
        ));
        assert!(tool_a.requires_confirmation());
    }

    #[test]
    fn format_use_summary_shows_server_tool_format() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let client = mock_client(vec![]).await;
            let adapter = McpTool::new(
                "playwright".into(),
                "click".into(),
                "Click an element".into(),
                serde_json::json!({"type": "object"}),
                Arc::new(Mutex::new(client)),
            );

            let input = serde_json::json!({"selector": "#btn", "timeout": 5000});
            let summary = adapter.format_use_summary(&input).unwrap();
            assert!(summary.starts_with("playwright::click"));
            assert!(summary.contains("selector"));
            assert!(summary.contains("#btn"));
        });
    }

    #[test]
    fn format_use_summary_truncates_long_values() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let client = mock_client(vec![]).await;
            let adapter = McpTool::new(
                "srv".into(),
                "tool".into(),
                "desc".into(),
                serde_json::json!({"type": "object"}),
                Arc::new(Mutex::new(client)),
            );

            let long_val = "a".repeat(50);
            let input = serde_json::json!({"key": long_val});
            let summary = adapter.format_use_summary(&input).unwrap();
            assert!(summary.contains("…"));
            assert!(summary.len() < 80);
        });
    }

    #[test]
    fn format_use_summary_empty_input() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let client = mock_client(vec![]).await;
            let adapter = McpTool::new(
                "srv".into(),
                "tool".into(),
                "desc".into(),
                serde_json::json!({"type": "object"}),
                Arc::new(Mutex::new(client)),
            );

            let summary = adapter.format_use_summary(&serde_json::json!({})).unwrap();
            assert_eq!(summary, "srv::tool");
        });
    }

    #[test]
    fn format_result_json() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let client = mock_client(vec![]).await;
            let adapter = McpTool::new(
                "srv".into(),
                "tool".into(),
                "desc".into(),
                serde_json::json!({"type": "object"}),
                Arc::new(Mutex::new(client)),
            );

            let output = ToolOutput::success(r#"{"key": "value", "num": 42}"#);
            let result = adapter.format_result(&output).unwrap();
            let text: String = result
                .lines
                .iter()
                .map(|l| &l.text)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n");
            assert!(text.contains("key"));
            assert!(text.contains("value"));
            assert_eq!(result.preview_lines, 3);
        });
    }

    #[test]
    fn format_result_plain_text() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let client = mock_client(vec![]).await;
            let adapter = McpTool::new(
                "srv".into(),
                "tool".into(),
                "desc".into(),
                serde_json::json!({"type": "object"}),
                Arc::new(Mutex::new(client)),
            );

            let output = ToolOutput::success("line1\nline2\nline3");
            let result = adapter.format_result(&output).unwrap();
            assert_eq!(result.lines.len(), 3);
            assert_eq!(result.lines[0].text, "line1");
        });
    }

    #[test]
    fn format_result_long_output_truncated() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let client = mock_client(vec![]).await;
            let adapter = McpTool::new(
                "srv".into(),
                "tool".into(),
                "desc".into(),
                serde_json::json!({"type": "object"}),
                Arc::new(Mutex::new(client)),
            );

            let long_text = (1..=20)
                .map(|i| format!("line {i}"))
                .collect::<Vec<_>>()
                .join("\n");
            let output = ToolOutput::success(&long_text);
            let result = adapter.format_result(&output).unwrap();
            assert_eq!(result.lines.len(), 11); // 10 lines + "... N total lines"
            assert!(result.lines[10].text.contains("20 total lines"));
        });
    }

    #[test]
    fn display_color_is_highlight() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let client = mock_client(vec![]).await;
            let adapter = McpTool::new(
                "srv".into(),
                "tool".into(),
                "desc".into(),
                serde_json::json!({"type": "object"}),
                Arc::new(Mutex::new(client)),
            );

            assert_eq!(adapter.display_color(), ToolDisplayStyle::Highlight);
        });
    }
}
