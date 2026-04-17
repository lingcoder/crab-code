//! Wire types for `crab-proto`.
//!
//! Messages are JSON-RPC 2.0 envelopes carried over WebSocket text frames.
//! All public types derive [`schemars::JsonSchema`] so the wire schema can
//! be dumped (see [`dump_schema`]) and used to generate TS / Swift / Kotlin
//! client stubs — supporting the web / mobile / desktop entry points
//! without Rust bindings on those clients.
//!
//! ## Sub-modules
//!
//! - [`envelope`]  — JSON-RPC Request / Response / Notification / Error
//! - [`method`]    — canonical method-name constants
//! - [`error`]     — [`error::ErrorCode`] enum (standard + vendor codes)
//! - [`session`]   — session lifecycle params + events
//! - [`handshake`] — initialize request / result + client / server info
//! - [`meta`]      — protocol version, id type, JSON-Schema dump helper

pub mod envelope;
pub mod error;
pub mod handshake;
pub mod meta;
pub mod method;
pub mod session;

pub use envelope::{
    JsonRpcError, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, next_request_id,
};
pub use error::ErrorCode;
pub use handshake::{ClientInfo, InitializeParams, InitializeResult, ServerInfo};
pub use meta::{MessageId, PROTOCOL_VERSION, dump_schema};
pub use session::{
    SessionAttachParams, SessionAttachResult, SessionCancelParams, SessionCreateParams,
    SessionCreateResult, SessionEventParams, SessionSendInputParams,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_constants_are_distinct() {
        let all = [
            method::INITIALIZE,
            method::INITIALIZED,
            method::SESSION_ATTACH,
            method::SESSION_CREATE,
            method::SESSION_SEND_INPUT,
            method::SESSION_CANCEL,
            method::SESSION_EVENT,
        ];
        let mut sorted: Vec<_> = all.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), all.len(), "method names must be distinct");
    }
}
