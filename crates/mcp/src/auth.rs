//! MCP server authentication: `OAuth2`, API keys, token refresh.
//!
//! Supports multiple authentication methods for MCP server connections:
//! - No auth (default)
//! - Static API key (passed as header or query parameter)
//! - `OAuth2` authorization code flow with PKCE and token refresh
//!
//! Gated behind the `mcp_auth` feature flag in settings.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Auth method configuration
// ---------------------------------------------------------------------------

/// Authentication method for an MCP server.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpAuthMethod {
    /// No authentication required.
    #[default]
    None,
    /// Static API key, sent as a header or query parameter.
    ApiKey(ApiKeyConfig),
    /// `OAuth2` authorization code flow.
    OAuth2(OAuthConfig),
}

/// Configuration for API key authentication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyConfig {
    /// The API key value (may reference an env var via `${VAR}` syntax).
    pub key: String,
    /// Where to send the key: `"header"` (default) or `"query"`.
    #[serde(default = "default_key_location")]
    pub location: String,
    /// Header name or query parameter name (default: `"Authorization"`).
    #[serde(default = "default_header_name")]
    pub name: String,
}

fn default_key_location() -> String {
    "header".into()
}

fn default_header_name() -> String {
    "Authorization".into()
}

/// Configuration for `OAuth2` authorization code flow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthConfig {
    /// `OAuth2` client ID.
    pub client_id: String,
    /// `OAuth2` client secret (optional for public clients).
    pub client_secret: Option<String>,
    /// Authorization endpoint URL.
    pub auth_url: String,
    /// Token endpoint URL.
    pub token_url: String,
    /// Redirect URI for the authorization callback.
    #[serde(default = "default_redirect_uri")]
    pub redirect_uri: String,
    /// Requested scopes.
    #[serde(default)]
    pub scopes: Vec<String>,
}

fn default_redirect_uri() -> String {
    "http://localhost:0/callback".into()
}

// ---------------------------------------------------------------------------
// Auth token
// ---------------------------------------------------------------------------

/// A resolved authentication token, ready to attach to requests.
#[derive(Debug, Clone)]
pub struct AuthToken {
    /// The bearer or API key token value.
    pub access_token: String,
    /// Token type (e.g., "Bearer", "`ApiKey`").
    pub token_type: String,
    /// Expiry timestamp (seconds since epoch), if known.
    pub expires_at: Option<u64>,
    /// Refresh token, if available (`OAuth2` flows).
    pub refresh_token: Option<String>,
}

impl AuthToken {
    /// Check whether the token has expired (with a grace window).
    pub fn is_expired(&self) -> bool {
        todo!()
    }
}

// ---------------------------------------------------------------------------
// Auth manager
// ---------------------------------------------------------------------------

/// Manages authentication state for multiple MCP servers.
///
/// Stores resolved tokens per server and handles refresh flows.
pub struct McpAuthManager {
    /// Cached tokens keyed by server name.
    tokens: std::collections::HashMap<String, AuthToken>,
}

impl McpAuthManager {
    /// Create a new manager with no cached tokens.
    pub fn new() -> Self {
        Self {
            tokens: std::collections::HashMap::new(),
        }
    }

    /// Authenticate with an MCP server using the specified method.
    ///
    /// For `ApiKey`, returns immediately. For `OAuth2`, initiates the
    /// authorization flow (opening a browser or returning a URL).
    ///
    /// # Errors
    ///
    /// Returns an error if authentication fails (invalid key, OAuth flow
    /// error, network failure, etc.).
    pub async fn authenticate(
        &mut self,
        _server_name: &str,
        _method: &McpAuthMethod,
    ) -> crab_common::Result<AuthToken> {
        todo!()
    }

    /// Refresh an expired token using its refresh token.
    ///
    /// # Errors
    ///
    /// Returns an error if the refresh token is missing or the refresh
    /// request fails.
    pub async fn refresh_token(
        &mut self,
        _server_name: &str,
        _token: &AuthToken,
    ) -> crab_common::Result<AuthToken> {
        todo!()
    }

    /// Get the cached token for a server, if one exists and is not expired.
    pub fn get_valid_token(&self, _server_name: &str) -> Option<&AuthToken> {
        todo!()
    }

    /// Remove the cached token for a server.
    pub fn clear_token(&mut self, server_name: &str) {
        self.tokens.remove(server_name);
    }
}

impl Default for McpAuthManager {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for McpAuthManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpAuthManager")
            .field("cached_servers", &self.tokens.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_auth_method_is_none() {
        assert!(matches!(McpAuthMethod::default(), McpAuthMethod::None));
    }

    #[test]
    fn auth_method_serde_roundtrip() {
        let method = McpAuthMethod::ApiKey(ApiKeyConfig {
            key: "sk-test".into(),
            location: "header".into(),
            name: "Authorization".into(),
        });
        let json = serde_json::to_string(&method).unwrap();
        let parsed: McpAuthMethod = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, McpAuthMethod::ApiKey(_)));
    }

    #[test]
    fn oauth_config_serde() {
        let config = OAuthConfig {
            client_id: "my-client".into(),
            client_secret: None,
            auth_url: "https://auth.example.com/authorize".into(),
            token_url: "https://auth.example.com/token".into(),
            redirect_uri: "http://localhost:0/callback".into(),
            scopes: vec!["read".into(), "write".into()],
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: OAuthConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.client_id, "my-client");
        assert_eq!(parsed.scopes.len(), 2);
    }

    #[test]
    fn manager_starts_empty() {
        let mgr = McpAuthManager::new();
        assert!(mgr.tokens.is_empty());
    }

    #[test]
    fn clear_token_removes_entry() {
        let mut mgr = McpAuthManager::new();
        mgr.tokens.insert(
            "test-server".into(),
            AuthToken {
                access_token: "tok".into(),
                token_type: "Bearer".into(),
                expires_at: None,
                refresh_token: None,
            },
        );
        mgr.clear_token("test-server");
        assert!(mgr.tokens.is_empty());
    }
}
