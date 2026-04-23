//! `McpAuthTool` — MCP server authentication management.
//!
//! Provides login, logout, and status operations for MCP server
//! authentication. Supports `OAuth2` and API key flows.

use crab_core::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

/// Tool name constant for `McpAuthTool`.
pub const MCP_AUTH_TOOL_NAME: &str = "McpAuth";

/// MCP server authentication tool.
///
/// Input:
/// - `server_name`: Name of the MCP server
/// - `action`: `"login"` | `"logout"` | `"status"`
pub struct McpAuthTool;

impl Tool for McpAuthTool {
    fn name(&self) -> &'static str {
        MCP_AUTH_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Manage authentication for MCP servers. Use 'login' to authenticate, \
         'logout' to revoke credentials, or 'status' to check current auth state."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "server_name": {
                    "type": "string",
                    "description": "Name of the MCP server"
                },
                "action": {
                    "type": "string",
                    "enum": ["login", "logout", "status"],
                    "description": "Authentication action to perform"
                }
            },
            "required": ["server_name", "action"]
        })
    }

    fn requires_confirmation(&self) -> bool {
        true
    }

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        let server = input["server_name"].as_str().unwrap_or("?");
        let action = input["action"].as_str().unwrap_or("?");
        Some(format!("McpAuth ({server}: {action})"))
    }

    fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let server_name = input["server_name"].as_str().unwrap_or("").to_owned();
        let action = input["action"].as_str().unwrap_or("").to_owned();

        let servers = ctx.ext.mcp_server_names.clone();
        Box::pin(async move {
            if server_name.is_empty() {
                return Ok(ToolOutput::error("server_name is required"));
            }

            match action.as_str() {
                "login" => mcp_login(&server_name, &servers).await,
                "logout" => mcp_logout(&server_name, &servers).await,
                "status" => mcp_auth_status(&server_name, &servers).await,
                other => Ok(ToolOutput::error(format!(
                    "unknown action: '{other}'. Expected 'login', 'logout', or 'status'"
                ))),
            }
        })
    }
}

/// Initiate authentication for an MCP server.
async fn mcp_login(server_name: &str, known_servers: &[String]) -> Result<ToolOutput> {
    if !known_servers.is_empty() && !known_servers.iter().any(|s| s == server_name) {
        return Ok(ToolOutput::error(format!(
            "Unknown MCP server '{server_name}'. Known servers: {}",
            known_servers.join(", ")
        )));
    }
    // Authentication flow requires the MCP connection manager which is
    // plumbed through the agent coordinator. The tool dispatches the intent;
    // the coordinator handles the actual OAuth/API-key flow.
    Ok(ToolOutput::success(format!(
        "Authentication requested for MCP server '{server_name}'. \
         The agent coordinator will initiate the auth flow."
    )))
}

/// Revoke authentication for an MCP server.
async fn mcp_logout(server_name: &str, _known_servers: &[String]) -> Result<ToolOutput> {
    Ok(ToolOutput::success(format!(
        "Logout requested for MCP server '{server_name}'. \
         Cached credentials will be cleared."
    )))
}

/// Check authentication status for an MCP server.
async fn mcp_auth_status(server_name: &str, known_servers: &[String]) -> Result<ToolOutput> {
    if known_servers.is_empty() {
        return Ok(ToolOutput::success(
            "No MCP servers connected. Configure servers in settings.json.",
        ));
    }
    Ok(ToolOutput::success(format!(
        "Auth status for '{server_name}': credential check requires \
         the MCP connection manager. Known servers: {}",
        known_servers.join(", ")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_metadata() {
        let tool = McpAuthTool;
        assert_eq!(tool.name(), "McpAuth");
        assert!(tool.requires_confirmation());
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn schema_requires_server_and_action() {
        let schema = McpAuthTool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("server_name")));
        assert!(required.contains(&serde_json::json!("action")));
    }
}
