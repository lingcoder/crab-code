//! IDE bridge and IPC layer for Crab Code.
//!
//! Provides communication protocols and transports for IDE extensions
//! (VS Code, JetBrains, etc.) to interact with running Crab Code sessions.
//!
//! # Architecture
//!
//! - [`protocol`] — JSON-RPC message types for the bridge protocol
//! - [`repl_bridge`] — REPL relay: IDE <-> running session
//! - [`remote_bridge`] — Remote connection to daemon sessions
//! - [`ws_server`] — WebSocket server for persistent connections
//! - [`session_token`] — JWT session token generation and validation
//! - [`trusted_device`] — Trusted device registration and verification
//! - [`types`] — Shared types used across bridge modules

pub mod protocol;
pub mod remote_bridge;
pub mod repl_bridge;
pub mod session_token;
pub mod trusted_device;
pub mod types;
pub mod ws_server;
