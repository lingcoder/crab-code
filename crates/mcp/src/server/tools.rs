//! `ToolRegistryHandler` — bridges a list of `Arc<dyn Tool>` from
//! `crab-tools` into the MCP server's `ToolHandler` interface so those
//! tools are listable + callable by any connected MCP client.

use std::sync::Arc;

use crab_core::permission::{PermissionMode, PermissionPolicy};
use crab_core::tool::{Tool, ToolContext, ToolOutputContent};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::ToolHandler;
use crate::protocol::{McpToolDef, ToolCallResult, ToolResultContent};

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
