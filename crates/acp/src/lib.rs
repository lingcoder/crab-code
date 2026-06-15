//! `crab-acp` — [Agent Client Protocol](https://agentclientprotocol.com) server.
//!
//! ACP lets editors (Zed, Neovim, Helix, …) drive external AI coding
//! agents the way LSP lets them drive language servers. This crate
//! implements the agent side of the protocol on top of the upstream
//! [`agent_client_protocol`] SDK (Zed's official Rust crate) so that a
//! user's editor can spawn `crab` as an ACP-speaking child process.
//!
//! ## Architecture
//!
//! [`AcpServer`] owns the wire protocol: stdio transport, session
//! registry, prompt dispatch, cancellation, and the bridge from crab
//! engine events to ACP session notifications. [`AgentHandler`] is the
//! crate's external boundary — the composition root (cli) plugs in
//! an implementation wired to the engine and calls
//! [`AcpServer::serve_stdio`]. Mirrors how `crab-mcp`'s server takes a
//! `ToolHandler` without embedding a tool backend.

mod handler;
mod notify;
mod server;
mod sessions;

pub use handler::{AgentHandler, HandlerError, PromptTurn};
pub use server::{AcpServeError, AcpServer};
