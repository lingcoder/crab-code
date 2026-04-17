//! Layer 3 — session runtime.
//!
//! Wires one conversation + backend + executor + memory store + cost
//! accumulator together. Decides whether Coordinator Mode (Layer 2b) is
//! active based on [`SessionConfig::coordinator_mode`].
//!
//! - [`session_config`] — [`SessionConfig`] value struct for session startup
//! - [`runtime`] — [`AgentSession`] running-session state + event plumbing

pub mod runtime;
pub mod session_config;

pub use runtime::AgentSession;
pub use session_config::SessionConfig;
