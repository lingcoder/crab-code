//! ACP wire types (scaffold — full surface in Phase δ).
//!
//! ACP is JSON-RPC over stdio: the editor spawns the agent as a child
//! process and uses the child's stdin/stdout as the transport. Frames
//! are line-delimited JSON (LSP-style `Content-Length` is **not** used
//! in ACP).

pub mod agent_info;
pub mod version;

pub use agent_info::AgentInfo;
pub use version::PROTOCOL_VERSION;
