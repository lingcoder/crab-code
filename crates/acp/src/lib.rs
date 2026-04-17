//! `crab-acp` — Agent Client Protocol server side.
//!
//! [ACP](https://agentclientprotocol.com) is an open JSON-RPC standard (introduced
//! by Zed in 2025) that lets editors drive external AI coding agents the way LSP
//! lets editors drive language servers. This crate lets crab **be** such an
//! external agent: a Zed / Neovim / Helix user picks crab from their editor's
//! "external agents" menu, the editor spawns crab as a child process, and
//! messages flow over stdio framed as ACP JSON-RPC.
//!
//! ## Architectural role
//!
//! ```text
//! Editor (ACP client)            ◄── ACP over stdio ──►   crab-acp (this crate)
//!                                                                │
//!                                                                ▼
//!                                                           AgentHandler trait
//!                                                                │
//!                                                                ▼
//!                                                       crab-engine / crab-agent
//! ```
//!
//! `AgentHandler` is this crate's external boundary — consumers (cli / daemon)
//! plug in a real implementation wired to `crab-engine`. This mirrors how
//! `crab-mcp::McpServer` takes a `ToolHandler` trait without embedding any
//! specific tool backend.
//!
//! ## Module layout (scaffold; implementation lands in Phase δ)
//!
//! ```text
//! crab-acp/
//! ├── protocol/      ACP wire types (initialize, prompt, tool_use, cancel, …)  ← this commit
//! └── server.rs      AcpServer + AgentHandler trait — Phase δ
//! ```

pub mod protocol;

pub use protocol::PROTOCOL_VERSION;
