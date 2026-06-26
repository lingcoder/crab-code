//! MCP server — exposes local tools to external MCP clients.
//!
//! The server reads newline-delimited JSON-RPC requests (one message per line,
//! per the MCP stdio spec) from an async reader (typically stdin) and writes
//! responses to an async writer (typically stdout). It handles the MCP
//! handshake (`initialize`) and serves
//! `tools/list` and `tools/call` by delegating to a [`ToolHandler`].
//!
//! Also supports HTTP SSE mode via [`McpServer::run_sse`], where the server
//! listens on a TCP port and serves SSE streams to multiple concurrent clients.

mod handler_traits;
pub mod prompts;
pub mod resources;
#[allow(clippy::module_inception)]
mod server;
pub mod tools;

pub use handler_traits::{PromptHandler, ResourceHandler, ToolHandler};
pub use prompts::SkillPromptHandler;
pub use resources::FileResourceHandler;
pub use server::McpServer;
pub use tools::ToolRegistryHandler;
