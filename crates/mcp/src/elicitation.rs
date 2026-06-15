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
/// A full implementation will wire this to the TUI prompt dialog and
/// channel system. For now, non-interactive auto-denial is the default.
pub async fn handle_elicitation(req: ElicitationRequest) -> ElicitationResponse {
    tracing::info!(
        server = %req.server_name,
        message = %req.message,
        "elicitation request auto-denied (non-interactive mode)"
    );
    ElicitationResponse::deny(Some(
        "non-interactive mode: elicitation requests are auto-denied".into(),
    ))
}

/// Validate that a response conforms to the request's JSON Schema.
///
/// When a schema is provided and the response is approved with data,
/// this performs full `jsonschema` validation. Returns `Ok(())` on
/// success, or a descriptive error string with validation details.
pub fn validate_response(
    req: &ElicitationRequest,
    resp: &ElicitationResponse,
) -> Result<(), String> {
    let Some(ref schema) = req.schema else {
        return Ok(());
    };

    if !resp.approved {
        return Ok(());
    }

    if resp.data.is_none() && !req.is_confirmation {
        return Err("approved response requires data when schema is provided".into());
    }

    let Some(ref data) = resp.data else {
        return Ok(());
    };

    // Perform JSON Schema validation.
    let compiled = jsonschema::validator_for(schema)
        .map_err(|e| format!("invalid JSON Schema in elicitation request: {e}"))?;

    if let Err(validation_err) = compiled.validate(data) {
        return Err(format!(
            "response data does not conform to schema: {validation_err}"
        ));
    }

    Ok(())
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

    #[test]
    fn validate_no_schema_always_ok() {
        let req = ElicitationRequest {
            server_name: "s".into(),
            message: "m".into(),
            schema: None,
            request_id: None,
            is_confirmation: false,
        };
        let resp = ElicitationResponse::approve(serde_json::json!(42));
        assert!(validate_response(&req, &resp).is_ok());
    }

    #[test]
    fn validate_denied_always_ok() {
        let req = ElicitationRequest {
            server_name: "s".into(),
            message: "m".into(),
            schema: Some(serde_json::json!({"type": "string"})),
            request_id: None,
            is_confirmation: false,
        };
        let resp = ElicitationResponse::deny(None);
        assert!(validate_response(&req, &resp).is_ok());
    }

    #[test]
    fn validate_approved_without_data_fails() {
        let req = ElicitationRequest {
            server_name: "s".into(),
            message: "m".into(),
            schema: Some(serde_json::json!({"type": "object"})),
            request_id: None,
            is_confirmation: false,
        };
        // `approve(Null)` still carries `Some(data)`, so it passes the
        // presence check but fails schema validation against an object type.
        let resp = ElicitationResponse::approve(serde_json::Value::Null);
        let err = validate_response(&req, &resp).unwrap_err();
        assert!(err.contains("does not conform"));

        let resp2 = ElicitationResponse {
            approved: true,
            data: None,
            reason: None,
        };
        let err = validate_response(&req, &resp2).unwrap_err();
        assert!(err.contains("requires data"));
    }

    #[test]
    fn validate_conforming_data_ok() {
        let req = ElicitationRequest {
            server_name: "s".into(),
            message: "m".into(),
            schema: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" }
                },
                "required": ["name"]
            })),
            request_id: None,
            is_confirmation: false,
        };
        let resp = ElicitationResponse::approve(serde_json::json!({"name": "Alice"}));
        assert!(validate_response(&req, &resp).is_ok());
    }

    #[test]
    fn validate_non_conforming_data_fails() {
        let req = ElicitationRequest {
            server_name: "s".into(),
            message: "m".into(),
            schema: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" }
                },
                "required": ["name"]
            })),
            request_id: None,
            is_confirmation: false,
        };
        let resp = ElicitationResponse::approve(serde_json::json!({"age": 42}));
        let err = validate_response(&req, &resp).unwrap_err();
        assert!(err.contains("does not conform to schema"));
    }

    #[test]
    fn validate_confirmation_with_no_data_ok() {
        let req = ElicitationRequest {
            server_name: "s".into(),
            message: "m".into(),
            schema: Some(serde_json::json!({"type": "object"})),
            request_id: None,
            is_confirmation: true,
        };
        let resp = ElicitationResponse {
            approved: true,
            data: None,
            reason: None,
        };
        assert!(validate_response(&req, &resp).is_ok());
    }

    #[test]
    fn validate_string_schema() {
        let req = ElicitationRequest {
            server_name: "s".into(),
            message: "m".into(),
            schema: Some(serde_json::json!({"type": "string"})),
            request_id: None,
            is_confirmation: false,
        };
        let ok = ElicitationResponse::approve(serde_json::json!("hello"));
        assert!(validate_response(&req, &ok).is_ok());

        let bad = ElicitationResponse::approve(serde_json::json!(42));
        assert!(validate_response(&req, &bad).is_err());
    }
}
