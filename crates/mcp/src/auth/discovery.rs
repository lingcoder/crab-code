//! OAuth server metadata discovery: RFC 9728 + RFC 8414.
//!
//! Two-step dance used by MCP servers that support OAuth:
//!
//! 1. Fetch `<server_url>/.well-known/oauth-protected-resource` (RFC 9728)
//!    to learn which authorisation server(s) protect the resource.
//! 2. Fetch `<issuer>/.well-known/oauth-authorization-server` (RFC 8414)
//!    on the discovered issuer to learn the endpoints (`authorization_endpoint`,
//!    `token_endpoint`, etc.) and supported PKCE methods.
//!
//! The returned metadata feeds into the auth URL builder in a later sub-phase.

use serde::{Deserialize, Serialize};

/// Resource metadata per RFC 9728 §3.
///
/// Minimal shape — only fields the flow actually consumes are modelled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceMetadata {
    /// Resource identifier (usually the server's canonical URL).
    pub resource: String,
    /// Authorisation server issuer URLs that protect this resource.
    pub authorization_servers: Vec<String>,
    /// Scopes the resource recognises.
    #[serde(default)]
    pub scopes_supported: Option<Vec<String>>,
    /// Bearer token transport methods supported (`"header"`, `"body"`, `"query"`).
    #[serde(default)]
    pub bearer_methods_supported: Option<Vec<String>>,
}

/// Authorisation server metadata per RFC 8414 §3.
///
/// Only the subset of fields the PKCE flow needs; additional fields are
/// silently ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthServerMetadata {
    /// Canonical issuer URL.
    pub issuer: String,
    /// Authorisation endpoint.
    pub authorization_endpoint: String,
    /// Token endpoint.
    pub token_endpoint: String,
    /// Dynamic-registration endpoint, if the server supports it.
    #[serde(default)]
    pub registration_endpoint: Option<String>,
    /// Scopes the authorisation server recognises.
    #[serde(default)]
    pub scopes_supported: Option<Vec<String>>,
    /// PKCE challenge methods the server accepts (`"S256"` is required).
    #[serde(default)]
    pub code_challenge_methods_supported: Option<Vec<String>>,
    /// Response types the server accepts (`"code"` for auth-code flow).
    #[serde(default)]
    pub response_types_supported: Option<Vec<String>>,
    /// Grant types the server accepts (`"authorization_code"` + `"refresh_token"`).
    #[serde(default)]
    pub grant_types_supported: Option<Vec<String>>,
}

/// Fetch and parse `<base>/.well-known/oauth-protected-resource`.
///
/// `base` should be the scheme + host + optional base path of the MCP
/// server; the well-known suffix is appended.
///
/// # Errors
///
/// Returns `Err` if the HTTP request fails, the response is non-2xx, or
/// the JSON body does not match `ResourceMetadata`.
pub async fn discover_resource(
    base: &str,
    http: &reqwest::Client,
) -> crab_core::Result<ResourceMetadata> {
    let url = join_well_known(base, "oauth-protected-resource");
    let resp = http
        .get(&url)
        .send()
        .await
        .map_err(|e| crab_core::Error::Other(format!("RFC 9728 fetch failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(crab_core::Error::Other(format!(
            "RFC 9728 discovery at {url} returned {}",
            resp.status()
        )));
    }
    resp.json::<ResourceMetadata>()
        .await
        .map_err(|e| crab_core::Error::Other(format!("RFC 9728 parse failed: {e}")))
}

/// Fetch and parse `<issuer>/.well-known/oauth-authorization-server`.
///
/// # Errors
///
/// Returns `Err` if the HTTP request fails, the response is non-2xx, or
/// the JSON body does not match `AuthServerMetadata`.
pub async fn discover_auth_server(
    issuer: &str,
    http: &reqwest::Client,
) -> crab_core::Result<AuthServerMetadata> {
    let url = join_well_known(issuer, "oauth-authorization-server");
    let resp = http
        .get(&url)
        .send()
        .await
        .map_err(|e| crab_core::Error::Other(format!("RFC 8414 fetch failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(crab_core::Error::Other(format!(
            "RFC 8414 discovery at {url} returned {}",
            resp.status()
        )));
    }
    resp.json::<AuthServerMetadata>()
        .await
        .map_err(|e| crab_core::Error::Other(format!("RFC 8414 parse failed: {e}")))
}

/// Join a base URL with `/.well-known/<suffix>`, handling trailing slashes.
fn join_well_known(base: &str, suffix: &str) -> String {
    let trimmed = base.trim_end_matches('/');
    format!("{trimmed}/.well-known/{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_metadata_serde_roundtrip() {
        let m = ResourceMetadata {
            resource: "https://mcp.example.com".into(),
            authorization_servers: vec!["https://auth.example.com".into()],
            scopes_supported: Some(vec!["read".into(), "write".into()]),
            bearer_methods_supported: Some(vec!["header".into()]),
        };
        let json = serde_json::to_string(&m).unwrap();
        let back: ResourceMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn resource_metadata_minimal_parse() {
        let json = r#"{
            "resource": "https://mcp.example.com",
            "authorization_servers": ["https://auth.example.com"]
        }"#;
        let parsed: ResourceMetadata = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.resource, "https://mcp.example.com");
        assert_eq!(parsed.authorization_servers.len(), 1);
        assert!(parsed.scopes_supported.is_none());
        assert!(parsed.bearer_methods_supported.is_none());
    }

    #[test]
    fn auth_server_metadata_serde_roundtrip() {
        let m = AuthServerMetadata {
            issuer: "https://auth.example.com".into(),
            authorization_endpoint: "https://auth.example.com/oauth/authorize".into(),
            token_endpoint: "https://auth.example.com/oauth/token".into(),
            registration_endpoint: Some("https://auth.example.com/oauth/register".into()),
            scopes_supported: Some(vec!["read".into()]),
            code_challenge_methods_supported: Some(vec!["S256".into()]),
            response_types_supported: Some(vec!["code".into()]),
            grant_types_supported: Some(vec!["authorization_code".into(), "refresh_token".into()]),
        };
        let json = serde_json::to_string(&m).unwrap();
        let back: AuthServerMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn auth_server_metadata_minimal_parse() {
        let json = r#"{
            "issuer": "https://auth.example.com",
            "authorization_endpoint": "https://auth.example.com/authorize",
            "token_endpoint": "https://auth.example.com/token"
        }"#;
        let parsed: AuthServerMetadata = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.issuer, "https://auth.example.com");
        assert!(parsed.registration_endpoint.is_none());
        assert!(parsed.scopes_supported.is_none());
    }

    #[test]
    fn well_known_join_strips_trailing_slash() {
        assert_eq!(
            join_well_known("https://example.com/", "oauth-protected-resource"),
            "https://example.com/.well-known/oauth-protected-resource"
        );
        assert_eq!(
            join_well_known("https://example.com", "oauth-protected-resource"),
            "https://example.com/.well-known/oauth-protected-resource"
        );
    }

    #[test]
    fn well_known_join_preserves_path() {
        assert_eq!(
            join_well_known("https://example.com/mcp", "oauth-authorization-server"),
            "https://example.com/mcp/.well-known/oauth-authorization-server"
        );
    }
}
