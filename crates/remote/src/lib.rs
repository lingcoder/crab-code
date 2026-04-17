//! `crab-remote` — the remote-control protocol and both endpoints.
//!
//! This crate is crab-code's own remote-control surface: a JSON-RPC-over-WebSocket
//! protocol (the `crab-proto`) together with the client and server that speak it.
//! It is the architectural hinge for every non-CLI entry point — web UI, mobile
//! app, desktop app, and headless daemon all attach via this surface.
//!
//! ## Module layout (planned; landed incrementally)
//!
//! ```text
//! crab-remote/
//! ├── protocol/     wire types (JSON-RPC envelopes, message enums)  ← this commit
//! ├── auth/         shared auth types (JWT claims, trusted devices) — Phase α
//! ├── client/       outbound client: `RemoteClient::connect(url)`   — Phase α
//! └── server/       inbound server: `RemoteServer` + `SessionHandler` trait — Phase α
//! ```
//!
//! Protocol types derive [`schemars::JsonSchema`] so TS / Swift / Kotlin client
//! stubs can be generated from the same source — critical for supporting web /
//! mobile / desktop clients that are not written in Rust.
//!
//! ## Relation to `crab-mcp`
//!
//! Same shape as `crab-mcp` (client + server + protocol in one crate), different
//! wire language: `crab-mcp` speaks MCP (external standard, tool-focused), this
//! crate speaks `crab-proto` (our own, session-focused).

pub mod auth;
pub mod client;
pub mod protocol;
pub mod server;

pub use client::{ClientConfig, ClientError, RemoteClient};
pub use protocol::PROTOCOL_VERSION;
