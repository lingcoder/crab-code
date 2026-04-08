//! JSON-RPC message types for the bridge protocol.
//!
//! Defines the request/response/notification message format used for
//! communication between IDE extensions and Crab Code sessions.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC version string.
pub const JSONRPC_VERSION: &str = "2.0";

/// A JSON-RPC request from client to server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeRequest {
    /// JSON-RPC version (always "2.0").
    pub jsonrpc: String,
    /// Request ID for correlating responses.
    pub id: RequestId,
    /// Method name.
    pub method: String,
    /// Optional parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// A JSON-RPC response from server to client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeResponse {
    /// JSON-RPC version.
    pub jsonrpc: String,
    /// Request ID this response correlates to.
    pub id: RequestId,
    /// Successful result (mutually exclusive with error).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error result (mutually exclusive with result).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

/// A JSON-RPC notification (no response expected).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeNotification {
    /// JSON-RPC version.
    pub jsonrpc: String,
    /// Notification method.
    pub method: String,
    /// Optional parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// JSON-RPC request ID (integer or string).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    /// Numeric ID.
    Number(i64),
    /// String ID.
    String(String),
}

/// JSON-RPC error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    /// Error code.
    pub code: i32,
    /// Human-readable error message.
    pub message: String,
    /// Optional structured error data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Standard JSON-RPC error codes.
pub mod error_codes {
    /// Invalid JSON was received.
    pub const PARSE_ERROR: i32 = -32700;
    /// The JSON sent is not a valid Request object.
    pub const INVALID_REQUEST: i32 = -32600;
    /// The method does not exist or is not available.
    pub const METHOD_NOT_FOUND: i32 = -32601;
    /// Invalid method parameters.
    pub const INVALID_PARAMS: i32 = -32602;
    /// Internal JSON-RPC error.
    pub const INTERNAL_ERROR: i32 = -32603;
}

// ── Known bridge methods ──────────────────────────────────────────────

/// Well-known bridge protocol methods.
pub mod methods {
    /// Initialize the bridge connection.
    pub const INITIALIZE: &str = "bridge/initialize";
    /// Execute a tool via the bridge.
    pub const EXECUTE_TOOL: &str = "bridge/executeTool";
    /// Send a message to the session.
    pub const SEND_MESSAGE: &str = "bridge/sendMessage";
    /// Get current session state.
    pub const GET_STATE: &str = "bridge/getState";
    /// Notification: session state changed.
    pub const STATE_CHANGED: &str = "bridge/stateChanged";
    /// Notification: tool execution started.
    pub const TOOL_STARTED: &str = "bridge/toolStarted";
    /// Notification: tool execution completed.
    pub const TOOL_COMPLETED: &str = "bridge/toolCompleted";
}

impl BridgeRequest {
    /// Create a new request.
    pub fn new(id: RequestId, method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            id,
            method: method.into(),
            params,
        }
    }
}

impl BridgeResponse {
    /// Create a successful response.
    pub fn success(id: RequestId, result: Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// Create an error response.
    pub fn error(id: RequestId, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

impl BridgeNotification {
    /// Create a new notification.
    pub fn new(method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            method: method.into(),
            params,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_serde_roundtrip() {
        let req = BridgeRequest::new(
            RequestId::Number(1),
            "bridge/initialize",
            Some(json!({"version": "1.0"})),
        );
        let json = serde_json::to_string(&req).unwrap();
        let parsed: BridgeRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, RequestId::Number(1));
        assert_eq!(parsed.method, "bridge/initialize");
    }

    #[test]
    fn response_success() {
        let resp = BridgeResponse::success(RequestId::Number(1), json!({"ok": true}));
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn response_error() {
        let resp = BridgeResponse::error(
            RequestId::String("req_1".into()),
            error_codes::METHOD_NOT_FOUND,
            "Method not found",
        );
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, error_codes::METHOD_NOT_FOUND);
    }

    #[test]
    fn notification_serde() {
        let notif = BridgeNotification::new("bridge/stateChanged", Some(json!({"state": "idle"})));
        let json = serde_json::to_string(&notif).unwrap();
        let parsed: BridgeNotification = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.method, "bridge/stateChanged");
    }

    #[test]
    fn request_id_variants() {
        let num_id = RequestId::Number(42);
        let str_id = RequestId::String("req_abc".into());
        assert_ne!(num_id, str_id);

        let json_num = serde_json::to_string(&num_id).unwrap();
        assert_eq!(json_num, "42");

        let json_str = serde_json::to_string(&str_id).unwrap();
        assert_eq!(json_str, "\"req_abc\"");
    }
}
