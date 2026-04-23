//! The top-level [`McpAuthManager`] that coordinates auth flows and
//! caches resolved tokens across MCP servers.
//!
//! Lives as a sibling of `mod.rs` so that file stays a thin module
//! tree + re-exports.

use std::collections::HashMap;
use std::time::Duration;

use super::api_key;
use super::callback::{await_callback, redirect_uri_addr};
use super::exchange::exchange_code;
use super::flow::AuthorizationRequest;
use super::refresh;
use super::types::{AuthToken, McpAuthMethod, OAuthConfig};

/// Default overall timeout for the browser round-trip. Five minutes
/// matches typical provider session window + user "where did I leave
/// that tab" grace.
pub const DEFAULT_BROWSER_TIMEOUT: Duration = Duration::from_secs(300);

/// Manages authentication state for multiple MCP servers.
///
/// Stores resolved tokens per server and delegates to the appropriate
/// flow based on [`McpAuthMethod`]. Holds a shared `reqwest::Client`
/// across all auth operations so connection pooling works.
pub struct McpAuthManager {
    /// Cached tokens keyed by server name.
    tokens: HashMap<String, AuthToken>,
    /// Shared HTTP client. Created on first use so the manager stays
    /// cheap to instantiate in tests.
    http: Option<reqwest::Client>,
}

impl McpAuthManager {
    /// Create a new manager with no cached tokens and no HTTP client.
    #[must_use]
    pub fn new() -> Self {
        Self {
            tokens: HashMap::new(),
            http: None,
        }
    }

    /// Return the shared HTTP client, initialising it on first use.
    fn http(&mut self) -> crab_core::Result<&reqwest::Client> {
        if self.http.is_none() {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .map_err(|e| {
                    crab_core::Error::Other(format!("failed to build HTTP client: {e}"))
                })?;
            self.http = Some(client);
        }
        Ok(self.http.as_ref().expect("http client just set"))
    }

    /// Authenticate with an MCP server using the specified method.
    ///
    /// - `None` returns an empty token immediately.
    /// - `ApiKey` resolves env-var references and stores the value.
    /// - `OAuth2` runs the full browser flow: opens the user's default
    ///   browser to the authorisation URL, listens on localhost for the
    ///   redirect, exchanges the code for a token, and caches the result.
    ///
    /// # Errors
    ///
    /// Returns an error if authentication fails: env var missing, HTTP
    /// failure, user denies in browser, state mismatch (CSRF), timeout,
    /// or invalid token response.
    pub async fn authenticate(
        &mut self,
        server_name: &str,
        method: &McpAuthMethod,
    ) -> crab_core::Result<AuthToken> {
        let token = match method {
            McpAuthMethod::None => AuthToken {
                access_token: String::new(),
                token_type: "None".into(),
                expires_at: None,
                refresh_token: None,
            },
            McpAuthMethod::ApiKey(config) => api_key::resolve_api_key(config),
            McpAuthMethod::OAuth2(config) => self.run_oauth_flow(server_name, config).await?,
        };

        self.tokens.insert(server_name.to_string(), token.clone());
        Ok(token)
    }

    /// Full `OAuth2` authorisation code + PKCE flow. Public for tests
    /// that want to drive it with a custom browser stub; production
    /// callers should use [`Self::authenticate`].
    ///
    /// # Errors
    ///
    /// Returns `Err` on any step that fails: callback bind, browser
    /// launch, timeout, CSRF state mismatch, provider error, or token
    /// exchange HTTP failure.
    pub async fn run_oauth_flow(
        &mut self,
        server_name: &str,
        config: &OAuthConfig,
    ) -> crab_core::Result<AuthToken> {
        let req = AuthorizationRequest::build(config);
        let addr = redirect_uri_addr(&req.redirect_uri)?;

        tracing::info!(
            server = server_name,
            auth_url = %req.authorize_url,
            "opening browser for OAuth authorization"
        );

        // Start the callback listener first so it's bound before the
        // browser hits it.
        let addr_for_await = addr.clone();
        let callback_task =
            tokio::spawn(
                async move { await_callback(&addr_for_await, DEFAULT_BROWSER_TIMEOUT).await },
            );

        // Small yield so the listener is bound before we open the browser.
        tokio::task::yield_now().await;

        if let Err(e) = webbrowser::open(&req.authorize_url) {
            // Non-fatal: the user can copy-paste the URL. Log it.
            tracing::warn!(
                server = server_name,
                error = %e,
                "failed to open browser automatically; user must open this URL manually: {}",
                req.authorize_url
            );
        }

        let callback = callback_task
            .await
            .map_err(|e| crab_core::Error::Other(format!("callback task panicked: {e}")))??;

        // CSRF state verification
        match &callback.state {
            Some(got) if got == &req.state => {}
            Some(got) => {
                return Err(crab_core::Error::Other(format!(
                    "OAuth CSRF state mismatch: expected {}, got {got}",
                    req.state
                )));
            }
            None => {
                return Err(crab_core::Error::Other(
                    "OAuth callback missing state parameter".into(),
                ));
            }
        }

        let code = callback.into_code()?;

        // Exchange the code for a token.
        let http = self.http()?.clone();
        exchange_code(&http, config, &code, req.pkce.verifier()).await
    }

    /// Refresh an expired token using its refresh token. Updates the
    /// cached token on success.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the token has no `refresh_token`, the HTTP call
    /// fails, or the response body is unparseable.
    pub async fn refresh(
        &mut self,
        server_name: &str,
        config: &OAuthConfig,
    ) -> crab_core::Result<AuthToken> {
        let Some(current) = self.tokens.get(server_name) else {
            return Err(crab_core::Error::Config(format!(
                "no token cached for '{server_name}' — call authenticate() first"
            )));
        };
        let Some(refresh) = current.refresh_token.clone() else {
            return Err(crab_core::Error::Config(format!(
                "cached token for '{server_name}' has no refresh_token; re-run full auth"
            )));
        };

        let http = self.http()?.clone();
        let new_token = refresh::refresh_token(&http, config, &refresh).await?;
        self.tokens
            .insert(server_name.to_string(), new_token.clone());
        Ok(new_token)
    }

    /// Get the cached token for a server, if one exists and is not expired.
    #[must_use]
    pub fn get_valid_token(&self, server_name: &str) -> Option<&AuthToken> {
        self.tokens.get(server_name).filter(|t| !t.is_expired())
    }

    /// Insert a token directly into the cache. Primarily for tests and
    /// for bootstrapping from [`super::store::TokenStore::load_from_disk`].
    pub fn insert_token(&mut self, server_name: impl Into<String>, token: AuthToken) {
        self.tokens.insert(server_name.into(), token);
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
            .field("http_initialised", &self.http.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::super::types::ApiKeyConfig;
    use super::*;

    #[test]
    fn manager_starts_empty() {
        let mgr = McpAuthManager::new();
        assert!(mgr.tokens.is_empty());
        assert!(mgr.http.is_none());
    }

    #[test]
    fn clear_token_removes_entry() {
        let mut mgr = McpAuthManager::new();
        mgr.insert_token(
            "test-server",
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

    #[tokio::test]
    async fn authenticate_none_returns_empty_token() {
        let mut mgr = McpAuthManager::new();
        let tok = mgr.authenticate("any", &McpAuthMethod::None).await.unwrap();
        assert_eq!(tok.token_type, "None");
        assert!(tok.access_token.is_empty());
    }

    #[tokio::test]
    async fn authenticate_api_key_delegates_to_api_key_module() {
        let mut mgr = McpAuthManager::new();
        let method = McpAuthMethod::ApiKey(ApiKeyConfig {
            key: "sk-static".into(),
            location: "header".into(),
            name: "Authorization".into(),
        });
        let tok = mgr.authenticate("github", &method).await.unwrap();
        assert_eq!(tok.access_token, "sk-static");
        assert_eq!(tok.token_type, "ApiKey");
        assert!(mgr.get_valid_token("github").is_some());
    }

    #[tokio::test]
    async fn refresh_without_cached_token_errors() {
        let mut mgr = McpAuthManager::new();
        let config = OAuthConfig {
            client_id: "id".into(),
            client_secret: None,
            auth_url: "https://auth.example.com/authorize".into(),
            token_url: "https://auth.example.com/token".into(),
            redirect_uri: "http://localhost:0/callback".into(),
            scopes: vec![],
        };
        let err = mgr.refresh("svc", &config).await.unwrap_err();
        assert!(err.to_string().contains("no token cached"));
    }

    #[tokio::test]
    async fn refresh_without_refresh_token_errors() {
        let mut mgr = McpAuthManager::new();
        mgr.insert_token(
            "svc",
            AuthToken {
                access_token: "a".into(),
                token_type: "Bearer".into(),
                expires_at: Some(1),
                refresh_token: None,
            },
        );
        let config = OAuthConfig {
            client_id: "id".into(),
            client_secret: None,
            auth_url: "https://auth.example.com/authorize".into(),
            token_url: "https://auth.example.com/token".into(),
            redirect_uri: "http://localhost:0/callback".into(),
            scopes: vec![],
        };
        let err = mgr.refresh("svc", &config).await.unwrap_err();
        assert!(err.to_string().contains("no refresh_token"));
    }

    #[test]
    fn get_valid_token_filters_expired() {
        let mut mgr = McpAuthManager::new();
        mgr.insert_token(
            "fresh",
            AuthToken {
                access_token: "a".into(),
                token_type: "Bearer".into(),
                expires_at: None,
                refresh_token: None,
            },
        );
        mgr.insert_token(
            "stale",
            AuthToken {
                access_token: "b".into(),
                token_type: "Bearer".into(),
                expires_at: Some(1),
                refresh_token: None,
            },
        );
        assert!(mgr.get_valid_token("fresh").is_some());
        assert!(mgr.get_valid_token("stale").is_none());
    }

    #[test]
    fn http_client_lazy_init() {
        let mut mgr = McpAuthManager::new();
        assert!(mgr.http.is_none());
        let _ = mgr.http().unwrap();
        assert!(mgr.http.is_some());
    }
}
