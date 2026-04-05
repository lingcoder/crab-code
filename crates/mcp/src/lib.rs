pub mod audit;
pub mod client;
#[cfg(test)]
mod compliance_tests;
pub mod discovery;
pub mod manager;
pub mod protocol;
pub mod resource;
pub mod server;
pub mod sse_server;
pub mod tool_chain;
pub mod tool_group;
pub mod tool_sandbox;
pub mod tool_version;
pub mod transport;

pub use client::McpClient;
pub use discovery::{McpServerConfig, McpTransportConfig, connect_server, parse_mcp_servers};
pub use manager::{DiscoveredTool, McpManager};
pub use protocol::{
    ClientCapabilities, ClientInfo, InitializeParams, InitializeResult, JsonRpcError,
    JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, McpPrompt, McpResource, McpToolDef,
    ServerCapabilities, ServerInfo, ToolCallParams, ToolCallResult,
};
pub use resource::ResourceCache;
pub use server::{
    FileResourceHandler, McpServer, PromptHandler, ResourceHandler, SkillPromptHandler,
    ToolHandler, ToolRegistryHandler,
};
pub use sse_server::run_sse;
pub use tool_chain::{ChainBuilder, ChainExecutor, ChainResult, ChainStep, ToolChain, ToolChainTemplate};
pub use tool_group::{IndexedTool, ToolGroup, ToolIndex};
pub use tool_sandbox::{McpPermissionBoundary, McpToolSandbox, PermissionLevel, SandboxPolicy, SandboxVerdict};
pub use tool_version::{ToolVersion, ToolVersionRegistry, VersionedTool};
pub use transport::Transport;
pub use audit::{AuditEntry, AuditEntryBuilder, AuditOutcome, McpAuditLog};
