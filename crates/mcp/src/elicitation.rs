//! Elicitation handler for MCP server requests for user input.
//!
//! When an MCP server needs additional information from the user (e.g.,
//! selecting a resource, providing credentials, confirming an action),
//! it sends an elicitation request. This module defines the request/response
//! types and the async handler interface.

use serde::{Deserialize, Serialize};

/// A request from an MCP server for user input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitationRequest {
    /// Name of the MCP server making the request.
    pub server_name: String,
    /// Human-readable message to display to the user.
    pub message: String,
    /// Optional JSON Schema describing the expected response data shape.
    pub schema: Option<serde_json::Value>,
    /// Optional request ID for correlating request/response pairs.
    pub request_id: Option<String>,
    /// Whether this is a simple yes/no confirmation or requires data.
    #[serde(default)]
    pub is_confirmation: bool,
}

/// The user's response to an elicitation request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitationResponse {
    /// Whether the user approved / provided the requested data.
    pub approved: bool,
    /// The user's input data (conforming to the request schema), if any.
    pub data: Option<serde_json::Value>,
    /// Optional reason for denial, if `approved` is false.
    pub reason: Option<String>,
}

impl ElicitationResponse {
    /// Create an approval response with data.
    pub fn approve(data: serde_json::Value) -> Self {
        Self {
            approved: true,
            data: Some(data),
            reason: None,
        }
    }

    /// Create a simple approval with no data (confirmation).
    pub fn confirm() -> Self {
        Self {
            approved: true,
            data: None,
            reason: None,
        }
    }

    /// Create a denial response.
    pub fn deny(reason: Option<String>) -> Self {
        Self {
            approved: false,
            data: None,
            reason,
        }
    }
}

/// Handle an elicitation request.
///
/// In interactive mode, this would display a prompt to the user and wait
/// for their response. In non-interactive mode, it auto-denies.
///
/// # Current implementation
///
/// Returns `todo!()` — will be wired to the TUI prompt system.
pub async fn handle_elicitation(_req: ElicitationRequest) -> ElicitationResponse {
    todo!()
}

/// Validate that a response conforms to the request's schema, if one was provided.
///
/// # Errors
///
/// Returns an error message if validation fails.
pub fn validate_response(
    _req: &ElicitationRequest,
    _resp: &ElicitationResponse,
) -> Result<(), String> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approve_response() {
        let resp = ElicitationResponse::approve(serde_json::json!({"key": "value"}));
        assert!(resp.approved);
        assert!(resp.data.is_some());
        assert!(resp.reason.is_none());
    }

    #[test]
    fn confirm_response() {
        let resp = ElicitationResponse::confirm();
        assert!(resp.approved);
        assert!(resp.data.is_none());
    }

    #[test]
    fn deny_response() {
        let resp = ElicitationResponse::deny(Some("user cancelled".into()));
        assert!(!resp.approved);
        assert!(resp.data.is_none());
        assert_eq!(resp.reason.as_deref(), Some("user cancelled"));
    }

    #[test]
    fn request_serde_roundtrip() {
        let req = ElicitationRequest {
            server_name: "my-server".into(),
            message: "Please select a file".into(),
            schema: Some(serde_json::json!({"type": "string"})),
            request_id: Some("req-1".into()),
            is_confirmation: false,
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: ElicitationRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.server_name, "my-server");
        assert_eq!(parsed.request_id.as_deref(), Some("req-1"));
    }
}
