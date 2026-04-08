//! `McpAuthTool` — MCP server authentication management.
//!
//! Provides login, logout, and status operations for MCP server
//! authentication. Supports OAuth2 and API key flows.

use crab_common::Result;
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

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let server_name = input["server_name"].as_str().unwrap_or("").to_owned();
        let action = input["action"].as_str().unwrap_or("").to_owned();

        Box::pin(async move {
            if server_name.is_empty() {
                return Ok(ToolOutput::error("server_name is required"));
            }

            match action.as_str() {
                "login" => mcp_login(&server_name).await,
                "logout" => mcp_logout(&server_name).await,
                "status" => mcp_auth_status(&server_name).await,
                other => Ok(ToolOutput::error(format!(
                    "unknown action: '{other}'. Expected 'login', 'logout', or 'status'"
                ))),
            }
        })
    }
}

/// Initiate authentication for an MCP server.
async fn mcp_login(server_name: &str) -> Result<ToolOutput> {
    let _ = server_name;
    todo!("McpAuthTool::mcp_login — initiate OAuth2/API key auth flow")
}

/// Revoke authentication for an MCP server.
async fn mcp_logout(server_name: &str) -> Result<ToolOutput> {
    let _ = server_name;
    todo!("McpAuthTool::mcp_logout — revoke stored credentials")
}

/// Check authentication status for an MCP server.
async fn mcp_auth_status(server_name: &str) -> Result<ToolOutput> {
    let _ = server_name;
    todo!("McpAuthTool::mcp_auth_status — query credential store and token validity")
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
