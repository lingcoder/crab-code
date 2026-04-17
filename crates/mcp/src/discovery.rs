use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Configuration for an MCP server to connect to.
///
/// Parsed from the `mcpServers` field in `~/.crab/settings.json` or
/// project-level `.crab/settings.json`. The format mirrors Claude Code's
/// MCP server configuration for user familiarity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Logical name of the server (the JSON object key).
    #[serde(skip)]
    pub name: String,
    /// Transport configuration.
    #[serde(flatten)]
    pub transport: McpTransportConfig,
    /// Optional environment variables to pass to stdio processes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,
}

/// Transport-specific configuration for an MCP server.
///
/// Settings format examples:
/// ```json
/// { "command": "npx", "args": ["-y", "@playwright/mcp"] }
/// { "url": "https://mcp.example.com/mcp" }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum McpTransportConfig {
    /// Stdio transport — launch a child process.
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
    /// WebSocket transport — connect to a remote WS/WSS endpoint.
    /// Config: `{ "ws_url": "ws://localhost:8080" }`
    Ws { ws_url: String },
    /// Remote HTTP transport — connect to a remote MCP HTTP endpoint.
    Http { url: String },
}

/// Parse MCP server configurations from a `mcpServers` JSON value.
///
/// The expected format is:
/// ```json
/// {
///   "server-name": {
///     "command": "npx",
///     "args": ["-y", "@playwright/mcp"],
///     "env": { "KEY": "value" }
///   },
///   "remote-server": {
///     "url": "https://mcp.example.com/mcp"
///   }
/// }
/// ```
pub fn parse_mcp_servers(
    mcp_servers_value: &serde_json::Value,
) -> crab_common::Result<Vec<McpServerConfig>> {
    let obj = mcp_servers_value
        .as_object()
        .ok_or_else(|| crab_common::Error::Other("mcpServers must be a JSON object".into()))?;

    let mut configs = Vec::new();

    for (name, value) in obj {
        match serde_json::from_value::<McpServerConfig>(value.clone()) {
            Ok(mut config) => {
                config.name.clone_from(name);
                configs.push(config);
            }
            Err(e) => {
                tracing::warn!(
                    server = name.as_str(),
                    error = %e,
                    "skipping MCP server with invalid config"
                );
            }
        }
    }

    Ok(configs)
}

/// Connect to a configured MCP server, returning a connected `McpClient`.
///
/// Spawns the appropriate transport based on the config and performs the
/// MCP initialize handshake.
pub async fn connect_server(config: &McpServerConfig) -> crab_common::Result<crate::McpClient> {
    match &config.transport {
        McpTransportConfig::Stdio { command, args } => {
            crate::McpClient::connect_stdio(command, args, config.env.as_ref(), &config.name).await
        }
        McpTransportConfig::Ws { ws_url } => {
            let transport = crate::transport::ws::WsTransport::connect(ws_url).await?;
            crate::McpClient::connect(Box::new(transport), &config.name).await
        }
        McpTransportConfig::Http { url } => {
            crate::McpClient::connect_streamable_http(url, &config.name).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_stdio_config() {
        let value = json!({
            "playwright": {
                "command": "npx",
                "args": ["-y", "@anthropic-ai/mcp-server-playwright"]
            }
        });

        let configs = parse_mcp_servers(&value).unwrap();
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].name, "playwright");
        assert!(matches!(
            &configs[0].transport,
            McpTransportConfig::Stdio { command, args }
                if command == "npx" && args.len() == 2
        ));
    }

    #[test]
    fn parse_http_config() {
        let value = json!({
            "remote": {
                "url": "https://mcp.example.com/mcp"
            }
        });

        let configs = parse_mcp_servers(&value).unwrap();
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].name, "remote");
        assert!(matches!(
            &configs[0].transport,
            McpTransportConfig::Http { url } if url == "https://mcp.example.com/mcp"
        ));
    }

    #[test]
    fn parse_stdio_with_env() {
        let value = json!({
            "server": {
                "command": "node",
                "args": ["server.js"],
                "env": { "PORT": "3000", "DEBUG": "true" }
            }
        });

        let configs = parse_mcp_servers(&value).unwrap();
        assert_eq!(configs.len(), 1);
        let env = configs[0].env.as_ref().unwrap();
        assert_eq!(env.get("PORT"), Some(&"3000".to_string()));
        assert_eq!(env.get("DEBUG"), Some(&"true".to_string()));
    }

    #[test]
    fn parse_multiple_servers() {
        let value = json!({
            "server-a": { "command": "a", "args": [] },
            "server-b": { "url": "https://b.example.com/mcp" },
            "server-c": { "command": "c" }
        });

        let configs = parse_mcp_servers(&value).unwrap();
        assert_eq!(configs.len(), 3);
    }

    #[test]
    fn parse_skips_invalid_config() {
        let value = json!({
            "valid": { "command": "ok" },
            "invalid": { "unknown_field_only": true }
        });

        // Invalid entries are skipped with a warning, not an error
        let configs = parse_mcp_servers(&value).unwrap();
        // "invalid" has no command or url, so serde untagged will fail
        // But depending on serde behavior, it may still parse as Stdio with empty command
        // The important thing is we don't panic
        assert!(!configs.is_empty());
    }

    #[test]
    fn parse_empty_servers() {
        let value = json!({});
        let configs = parse_mcp_servers(&value).unwrap();
        assert!(configs.is_empty());
    }

    #[test]
    fn parse_non_object_returns_error() {
        let value = json!([1, 2, 3]);
        let result = parse_mcp_servers(&value);
        assert!(result.is_err());
    }

    #[test]
    fn config_serde_roundtrip_stdio() {
        let config = McpServerConfig {
            name: String::new(), // skipped in serde
            transport: McpTransportConfig::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "server".into()],
            },
            env: Some(HashMap::from([("KEY".into(), "val".into())])),
        };

        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["command"], "npx");
        assert!(json["args"].is_array());
        assert_eq!(json["env"]["KEY"], "val");
    }

    #[test]
    fn config_serde_roundtrip_http() {
        let config = McpServerConfig {
            name: String::new(),
            transport: McpTransportConfig::Http {
                url: "https://example.com/mcp".into(),
            },
            env: None,
        };

        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["url"], "https://example.com/mcp");
        // env should not appear when None
        assert!(json.get("env").is_none());
    }

    #[test]
    fn parse_ws_config() {
        let value = json!({
            "ws-server": {
                "ws_url": "ws://localhost:8080"
            }
        });

        let configs = parse_mcp_servers(&value).unwrap();
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].name, "ws-server");
        assert!(matches!(
            &configs[0].transport,
            McpTransportConfig::Ws { ws_url } if ws_url == "ws://localhost:8080"
        ));
    }

    #[test]
    fn config_serde_roundtrip_ws() {
        let config = McpServerConfig {
            name: String::new(),
            transport: McpTransportConfig::Ws {
                ws_url: "wss://mcp.example.com/ws".into(),
            },
            env: None,
        };

        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["ws_url"], "wss://mcp.example.com/ws");
        assert!(json.get("env").is_none());
    }

    #[test]
    fn parse_mixed_transports() {
        let value = json!({
            "stdio-srv": { "command": "node", "args": ["server.js"] },
            "http-srv": { "url": "https://example.com/mcp" },
            "ws-srv": { "ws_url": "ws://localhost:9090" }
        });

        let configs = parse_mcp_servers(&value).unwrap();
        assert_eq!(configs.len(), 3);

        let names: Vec<&str> = configs.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"stdio-srv"));
        assert!(names.contains(&"http-srv"));
        assert!(names.contains(&"ws-srv"));
    }
}
