//! `crab-acp` — [Agent Client Protocol](https://agentclientprotocol.com) server glue.
//!
//! ACP lets editors (Zed, Neovim, Helix, …) drive external AI coding
//! agents the way LSP lets them drive language servers. This crate
//! wires the upstream [`agent_client_protocol`] SDK to stdio so that a
//! user's editor can spawn `crab` as an ACP-speaking child process.
//!
//! ## Architecture
//!
//! The wire types and the [`Agent`] trait come from the upstream SDK
//! (`agent-client-protocol = 0.10.4`, Zed's official Rust crate,
//! Apache-2.0). This crate only provides the stdio entry point and
//! re-exports the upstream surface that composition roots need, so
//! `cli` / `daemon` don't have to add the SDK as a direct dep.
//!
//! Composition roots (`crates/cli/` or `crates/daemon/`) implement the
//! [`Agent`] trait against the crab engine and hand the implementation
//! to [`server::AcpServer::serve_stdio`]; this crate never embeds any
//! engine logic.
//!
//! [`Agent`]: agent_client_protocol::Agent

pub mod server;

pub use server::{AcpServeError, AcpServer};

/// Re-export of the upstream [`agent_client_protocol`] crate so
/// consumers can implement the [`Agent`] trait without adding the SDK
/// as a direct workspace dependency.
///
/// [`Agent`]: agent_client_protocol::Agent
pub use agent_client_protocol as sdk;
