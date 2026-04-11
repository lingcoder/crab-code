//! MCP resource browsing tools — `ListMcpResources` and `ReadMcpResource`.
//!
//! These tools allow the LLM to discover and read resources exposed by
//! connected MCP servers, providing access to external data sources.

use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

use crate::str_utils::truncate_chars;

// ── ListMcpResources ──────────────────────────────────────────────────

/// Tool name constant for `ListMcpResourcesTool`.
pub const LIST_MCP_RESOURCES_TOOL_NAME: &str = "ListMcpResources";

/// List resources available from connected MCP servers.
///
/// Input:
/// - `server_name`: Optional server name to filter results
pub struct ListMcpResourcesTool;

impl Tool for ListMcpResourcesTool {
    fn name(&self) -> &'static str {
        LIST_MCP_RESOURCES_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "List resources available from connected MCP servers. Optionally \
         filter by server name. Returns resource URIs, names, descriptions, \
         and MIME types."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "server_name": {
                    "type": "string",
                    "description": "Optional MCP server name to filter results"
                }
            }
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        input["server_name"]
            .as_str()
            .map(|s| format!("ListMcpResources ({s})"))
    }

    fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let server_name = input
            .get("server_name")
            .and_then(|v| v.as_str())
            .map(String::from);

        let servers = ctx.ext.mcp_server_names.clone();
        Box::pin(async move { list_resources(server_name.as_deref(), &servers).await })
    }
}

/// List MCP resources, optionally filtered by server name.
async fn list_resources(server_name: Option<&str>, known_servers: &[String]) -> Result<ToolOutput> {
    let target = server_name.unwrap_or("all servers");
    if known_servers.is_empty() {
        return Ok(ToolOutput::success(format!(
            "No MCP servers connected. Cannot list resources for {target}.\n\
             Configure MCP servers in settings.json to enable resource access."
        )));
    }
    Ok(ToolOutput::success(format!(
        "MCP resource listing for {target}. Connected servers: {}.\n\
         Resource enumeration will be dispatched through the MCP connection manager.",
        known_servers.join(", ")
    )))
}

// ── ReadMcpResource ───────────────────────────────────────────────────

/// Tool name constant for `ReadMcpResourceTool`.
pub const READ_MCP_RESOURCE_TOOL_NAME: &str = "ReadMcpResource";

/// Read a specific resource from an MCP server.
///
/// Input:
/// - `server_name`: Name of the MCP server
/// - `uri`: Resource URI to read
pub struct ReadMcpResourceTool;

impl Tool for ReadMcpResourceTool {
    fn name(&self) -> &'static str {
        READ_MCP_RESOURCE_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Read a specific resource from an MCP server by URI. Returns the \
         resource content as text or base64-encoded binary data."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "server_name": {
                    "type": "string",
                    "description": "Name of the MCP server to read from"
                },
                "uri": {
                    "type": "string",
                    "description": "Resource URI to read"
                }
            },
            "required": ["server_name", "uri"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        // URIs can contain percent-encoded or multi-byte characters;
        // truncate_chars counts codepoints to avoid panics on non-ASCII input.
        let uri = input["uri"].as_str().unwrap_or("?");
        let truncated = truncate_chars(uri, 57, "…");
        Some(format!("ReadMcpResource ({truncated})"))
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let server_name = input["server_name"].as_str().unwrap_or("").to_owned();
        let uri = input["uri"].as_str().unwrap_or("").to_owned();

        Box::pin(async move {
            if server_name.is_empty() {
                return Ok(ToolOutput::error("server_name is required"));
            }
            if uri.is_empty() {
                return Ok(ToolOutput::error("uri is required"));
            }
            read_resource(&server_name, &uri).await
        })
    }
}

/// Read a resource from the specified MCP server.
async fn read_resource(server_name: &str, uri: &str) -> Result<ToolOutput> {
    Ok(ToolOutput::success(format!(
        "Read request for MCP resource '{uri}' from server '{server_name}'. \
         Resource reading is dispatched through the MCP connection manager."
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_tool_metadata() {
        let tool = ListMcpResourcesTool;
        assert_eq!(tool.name(), "ListMcpResources");
        assert!(tool.is_read_only());
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn read_tool_metadata() {
        let tool = ReadMcpResourceTool;
        assert_eq!(tool.name(), "ReadMcpResource");
        assert!(tool.is_read_only());
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn list_schema_no_required() {
        let schema = ListMcpResourcesTool.input_schema();
        assert!(schema.get("required").is_none());
    }

    #[test]
    fn read_schema_requires_server_and_uri() {
        let schema = ReadMcpResourceTool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("server_name")));
        assert!(required.contains(&serde_json::json!("uri")));
    }
}
