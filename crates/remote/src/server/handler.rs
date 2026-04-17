//! Trait exposed by the crab-proto server to the composition root
//! (`cli` / `daemon`) — "this is how you create and drive a real
//! session". The server calls this on behalf of connected clients.
//!
//! Design: rather than a method-per-request-kind trait, we hand back a
//! [`SessionHandle`] bundling a pair of channels. Each remote connection
//! gets its own handle; the server forwards inbound protocol messages
//! into `inbound_tx` and drains `outbound_rx` into WebSocket frames.
//! The composition root is free to route both ends however it likes
//! (single-session daemon, multi-session manager, fan-out to multiple
//! clients, etc.) without touching this crate.

use serde_json::Value;
use tokio::sync::mpsc;

use crate::protocol::{
    SessionAttachParams, SessionAttachResult, SessionCancelParams, SessionCreateParams,
    SessionCreateResult, SessionSendInputParams,
};

/// Errors the session backend can surface to the server.
///
/// The server turns these into JSON-RPC error responses; see
/// [`crate::protocol::ErrorCode`] for the wire values.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("session not found: {0}")]
    NotFound(String),
    #[error("session backend error: {0}")]
    Backend(String),
}

/// Inbound command routed from a connected client to the session
/// backend. The server emits one variant per validated client request.
///
/// Variants mirror the wire methods in [`crate::protocol::method`]. The
/// backend holds an `mpsc::Receiver<InboundCmd>` and processes them
/// in order.
#[derive(Debug)]
pub enum InboundCmd {
    SendInput(SessionSendInputParams),
    Cancel(SessionCancelParams),
}

/// Outbound event the backend pushes toward the connected client.
///
/// The server wraps these in JSON-RPC notifications with method
/// [`crate::protocol::method::SESSION_EVENT`].
#[derive(Debug, Clone)]
pub enum OutboundEvent {
    /// An opaque `core::Event` payload to forward. Serialised once at
    /// the backend boundary so this crate doesn't re-depend on
    /// `crab-core::Event` schema.
    Event(Value),
}

/// Paired channels for a single remote connection ↔ session.
///
/// Dropping the [`SessionHandle`] on the server side signals the backend
/// that the client disconnected; the backend's `outbound_rx` senders
/// being dropped signals the server that the session is gone.
pub struct SessionHandle {
    pub session_id: String,
    pub inbound_tx: mpsc::Sender<InboundCmd>,
    pub outbound_rx: mpsc::Receiver<OutboundEvent>,
    /// Whether the session currently has in-flight work. Echoed back in
    /// [`SessionAttachResult::busy`] so the client UI can render "cancel"
    /// vs "input prompt".
    pub busy: bool,
}

/// Backend trait the composition root implements.
///
/// Kept to two methods — create and attach. `send_input` / `cancel` are
/// not trait methods because they route through `inbound_tx` once a
/// handle exists; this keeps the trait tiny and lets the backend reply
/// asynchronously without blocking the server's dispatch loop.
pub trait SessionHandler: Send + Sync {
    /// Spin up a new session and return its handle.
    fn create(
        &self,
        params: SessionCreateParams,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<SessionHandle, SessionError>> + Send + '_>,
    >;

    /// Reattach to an existing session by id. Returns an error if the
    /// id is unknown to the backend.
    fn attach(
        &self,
        params: SessionAttachParams,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<SessionHandle, SessionError>> + Send + '_>,
    >;
}

/// Convenience: build a [`SessionCreateResult`] from a created handle.
#[must_use]
pub fn create_result(handle: &SessionHandle) -> SessionCreateResult {
    SessionCreateResult {
        session_id: handle.session_id.clone(),
    }
}

/// Convenience: build a [`SessionAttachResult`] from an attached handle.
#[must_use]
pub fn attach_result(handle: &SessionHandle) -> SessionAttachResult {
    SessionAttachResult {
        session_id: handle.session_id.clone(),
        busy: handle.busy,
    }
}
