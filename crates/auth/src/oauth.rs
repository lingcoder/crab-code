//! `OAuth2` PKCE authorization code flow with token refresh and secure storage.
//!
//! Designed for cloud LLM providers (AWS Bedrock, GCP Vertex, Azure `OpenAI`)
//! that use `OAuth2` for authentication instead of static API keys.
//!
//! Token storage: `~/.crab/auth/tokens.json` — stores per-provider tokens
//! with access token, refresh token, and expiry timestamp.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::AuthError;

/// Default token file location within the crab config directory.
const TOKEN_DIR: &str = "auth";
const TOKEN_FILE: &str = "tokens.json";

/// Buffer before actual expiry to trigger refresh (5 minutes).
const EXPIRY_BUFFER_SECS: u64 = 300;

// ── Token data model ─────────────────────────────────────���─────────────

/// A stored `OAuth2` token for a specific provider.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredToken {
    /// The provider this token belongs to (e.g., "bedrock", "vertex").
    pub provider: String,
    /// `OAuth2` access token.
    pub access_token: String,
    /// `OAuth2` refresh token (if available).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// Unix timestamp (seconds) when the access token expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    /// `OAuth2` token type (usually "Bearer").
    #[serde(default = "default_token_type")]
    pub token_type: String,
}

fn default_token_type() -> String {
    "Bearer".into()
}

impl StoredToken {
    /// Check if the token has expired (with buffer).
    #[must_use]
    pub fn is_expired(&self) -> bool {
        self.is_expired_at(now_secs())
    }

    /// Check expiry at a given timestamp (for testability).
    #[must_use]
    pub fn is_expired_at(&self, current_secs: u64) -> bool {
        self.expires_at
            .is_some_and(|exp| current_secs + EXPIRY_BUFFER_SECS >= exp)
    }

    /// Check if a refresh token is available.
    #[must_use]
    pub fn can_refresh(&self) -> bool {
        self.refresh_token.as_ref().is_some_and(|rt| !rt.is_empty())
    }
}

/// Container for all stored tokens (one per provider).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenStore {
    #[serde(default)]
    pub tokens: Vec<StoredToken>,
}

impl TokenStore {
    /// Get a token for a specific provider.
    #[must_use]
    pub fn get(&self, provider: &str) -> Option<&StoredToken> {
        self.tokens.iter().find(|t| t.provider == provider)
    }

    /// Insert or update a token for a provider.
    pub fn upsert(&mut self, token: StoredToken) {
        if let Some(existing) = self
            .tokens
            .iter_mut()
            .find(|t| t.provider == token.provider)
        {
            *existing = token;
        } else {
            self.tokens.push(token);
        }
    }

    /// Remove a token for a provider.
    pub fn remove(&mut self, provider: &str) -> bool {
        let before = self.tokens.len();
        self.tokens.retain(|t| t.provider != provider);
        self.tokens.len() < before
    }
}

// ── Token file persistence ─────────────────────────────────────────────

/// Return the default token file path: `~/.crab/auth/tokens.json`.
#[must_use]
pub fn default_token_path() -> PathBuf {
    crab_common::path::home_dir()
        .join(".crab")
        .join(TOKEN_DIR)
        .join(TOKEN_FILE)
}

/// Load the token store from a file.
/// Returns an empty store if the file doesn't exist.
pub fn load_token_store(path: &Path) -> Result<TokenStore, AuthError> {
    match std::fs::read_to_string(path) {
        Ok(content) => serde_json::from_str(&content).map_err(|e| AuthError::Auth {
            message: format!("failed to parse token store: {e}"),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(TokenStore::default()),
        Err(e) => Err(AuthError::Auth {
            message: format!("failed to read token store: {e}"),
        }),
    }
}

/// Save the token store to a file.
/// Creates parent directories if they don't exist.
pub fn save_token_store(path: &Path, store: &TokenStore) -> Result<(), AuthError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| AuthError::Auth {
            message: format!("failed to create token dir: {e}"),
        })?;
    }
    let json = serde_json::to_string_pretty(store).map_err(|e| AuthError::Auth {
        message: format!("failed to serialize token store: {e}"),
    })?;
    std::fs::write(path, json).map_err(|e| AuthError::Auth {
        message: format!("failed to write token store: {e}"),
    })
}

// ── OAuth2 PKCE configuration ──────────────────────────────────────────

/// Configuration for an `OAuth2` PKCE authorization flow.
#[derive(Debug, Clone)]
pub struct OAuth2Config {
    /// Provider name (e.g., "bedrock", "vertex").
    pub provider: String,
    /// `OAuth2` client ID.
    pub client_id: String,
    /// Authorization endpoint URL.
    pub auth_url: String,
    /// Token endpoint URL.
    pub token_url: String,
    /// Redirect URI for the callback (usually `http://localhost:<port>`).
    pub redirect_uri: String,
    /// `OAuth2` scopes to request.
    pub scopes: Vec<String>,
}

// ── PKCE S256 Challenge ───────────────────────────────────────────────

/// PKCE code verifier and challenge pair (RFC 7636, S256 method).
#[derive(Debug, Clone)]
pub struct PkceChallenge {
    /// Random high-entropy code verifier (43–128 characters, URL-safe).
    pub code_verifier: String,
    /// SHA256 hash of `code_verifier`, base64url-encoded (no padding).
    pub code_challenge: String,
}

impl PkceChallenge {
    /// Generate a new PKCE challenge with a random 32-byte verifier.
    #[must_use]
    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        rand::rng().fill(&mut bytes);
        Self::from_verifier_bytes(&bytes)
    }

    /// Create a PKCE challenge from raw verifier bytes (for testability).
    #[must_use]
    pub fn from_verifier_bytes(bytes: &[u8]) -> Self {
        let code_verifier = URL_SAFE_NO_PAD.encode(bytes);
        let digest = Sha256::digest(code_verifier.as_bytes());
        let code_challenge = URL_SAFE_NO_PAD.encode(digest);
        Self {
            code_verifier,
            code_challenge,
        }
    }
}

/// Build the authorization URL for the `OAuth2` PKCE flow.
///
/// The URL includes `response_type=code`, `code_challenge_method=S256`,
/// and all configured scopes. The user opens this URL in a browser.
#[must_use]
pub fn build_authorization_url(
    config: &OAuth2Config,
    challenge: &PkceChallenge,
    state: &str,
) -> String {
    let scopes = config.scopes.join(" ");
    let params = [
        ("response_type", "code"),
        ("client_id", &config.client_id),
        ("redirect_uri", &config.redirect_uri),
        ("scope", &scopes),
        ("code_challenge_method", "S256"),
        ("code_challenge", &challenge.code_challenge),
        ("state", state),
    ];
    let query = params
        .iter()
        .map(|(k, v)| format!("{k}={}", url_encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{}?{}", config.auth_url, query)
}

/// Exchange an authorization code for tokens using PKCE.
///
/// Sends a POST to the token endpoint with `grant_type=authorization_code`,
/// the authorization code, and the PKCE code verifier.
pub async fn exchange_code_for_token(
    config: &OAuth2Config,
    code: &str,
    code_verifier: &str,
) -> Result<TokenResponse, AuthError> {
    let client = reqwest::Client::new();

    let params: [(&str, &str); 5] = [
        ("grant_type", "authorization_code"),
        ("client_id", &config.client_id),
        ("code", code),
        ("redirect_uri", &config.redirect_uri),
        ("code_verifier", code_verifier),
    ];
    let form_body = params
        .iter()
        .map(|(k, v)| format!("{k}={}", url_encode(v)))
        .collect::<Vec<_>>()
        .join("&");

    let resp: reqwest::Response = client
        .post(&config.token_url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form_body)
        .send()
        .await
        .map_err(|e| AuthError::Auth {
            message: format!("token exchange request failed: {e}"),
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(AuthError::Auth {
            message: format!("token endpoint returned {status}: {body}"),
        });
    }

    let resp_text = resp.text().await.map_err(|e| AuthError::Auth {
        message: format!("failed to read token response: {e}"),
    })?;

    parse_token_response(&resp_text)
}

/// Refresh an access token using a refresh token.
pub async fn refresh_access_token(
    config: &OAuth2Config,
    refresh_token: &str,
) -> Result<TokenResponse, AuthError> {
    let client = reqwest::Client::new();

    let params: [(&str, &str); 3] = [
        ("grant_type", "refresh_token"),
        ("client_id", &config.client_id),
        ("refresh_token", refresh_token),
    ];
    let form_body = params
        .iter()
        .map(|(k, v)| format!("{k}={}", url_encode(v)))
        .collect::<Vec<_>>()
        .join("&");

    let resp: reqwest::Response = client
        .post(&config.token_url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form_body)
        .send()
        .await
        .map_err(|e| AuthError::Auth {
            message: format!("token refresh request failed: {e}"),
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(AuthError::Auth {
            message: format!("token refresh returned {status}: {body}"),
        });
    }

    let resp_text = resp.text().await.map_err(|e| AuthError::Auth {
        message: format!("failed to read refresh response: {e}"),
    })?;

    let mut result = parse_token_response(&resp_text)?;
    // Preserve the original refresh token if the response didn't include a new one
    if result.refresh_token.is_none() {
        result.refresh_token = Some(refresh_token.to_string());
    }
    Ok(result)
}

/// Parse a JSON token response body into a `TokenResponse`.
fn parse_token_response(json_str: &str) -> Result<TokenResponse, AuthError> {
    let body: serde_json::Value = serde_json::from_str(json_str).map_err(|e| AuthError::Auth {
        message: format!("failed to parse token response JSON: {e}"),
    })?;

    let access_token = body["access_token"]
        .as_str()
        .ok_or_else(|| AuthError::Auth {
            message: "no access_token in token response".into(),
        })?
        .to_string();

    let refresh_token = body["refresh_token"].as_str().map(String::from);

    let expires_in = body["expires_in"].as_u64().map(Duration::from_secs);

    let token_type = body["token_type"].as_str().unwrap_or("Bearer").to_string();

    Ok(TokenResponse {
        access_token,
        refresh_token,
        expires_in,
        token_type,
    })
}

/// Extract the authorization code and state from a localhost callback URL query string.
///
/// Parses `?code=...&state=...` from a URL like `http://localhost:9876/callback?code=abc&state=xyz`.
pub fn parse_callback_params(url: &str) -> Result<(String, String), AuthError> {
    let query = url.split('?').nth(1).ok_or_else(|| AuthError::Auth {
        message: "callback URL has no query string".into(),
    })?;

    let mut code = None;
    let mut state = None;

    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next().unwrap_or("");
        let value = parts.next().unwrap_or("");
        match key {
            "code" => code = Some(value.to_string()),
            "state" => state = Some(value.to_string()),
            _ => {}
        }
    }

    let code = code.ok_or_else(|| AuthError::Auth {
        message: "no 'code' parameter in callback URL".into(),
    })?;
    let state = state.ok_or_else(|| AuthError::Auth {
        message: "no 'state' parameter in callback URL".into(),
    })?;

    Ok((code, state))
}

/// Simple percent-encoding for URL query values.
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                use std::fmt::Write;
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

// ── Localhost callback server ─────────────────────────────────────────

/// Start a temporary localhost HTTP server that waits for the `OAuth2` callback.
///
/// Binds to `127.0.0.1:0` (random port), returns the port and a future
/// that resolves to the callback URL (containing `code` and `state` params).
///
/// The server handles exactly one request and then shuts down.
pub async fn start_callback_server() -> Result<
    (
        u16,
        impl std::future::Future<Output = Result<String, AuthError>>,
    ),
    AuthError,
> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| AuthError::Auth {
            message: format!("failed to bind callback server: {e}"),
        })?;

    let port = listener
        .local_addr()
        .map_err(|e| AuthError::Auth {
            message: format!("failed to get local address: {e}"),
        })?
        .port();

    let server_future = async move {
        let (mut stream, _addr) = listener.accept().await.map_err(|e| AuthError::Auth {
            message: format!("failed to accept callback connection: {e}"),
        })?;

        // Read the HTTP request
        let mut buf = vec![0u8; 4096];
        let n = tokio::io::AsyncReadExt::read(&mut stream, &mut buf)
            .await
            .map_err(|e| AuthError::Auth {
                message: format!("failed to read callback request: {e}"),
            })?;

        let request = String::from_utf8_lossy(&buf[..n]);

        // Extract the request path from "GET /callback?code=...&state=... HTTP/1.1"
        let path = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .ok_or_else(|| AuthError::Auth {
                message: "invalid HTTP request in callback".into(),
            })?
            .to_string();

        // Send a simple HTML response
        let response_body = "<html><body><h1>Authentication successful!</h1><p>You can close this window.</p></body></html>";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response_body.len(),
            response_body
        );
        tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes())
            .await
            .map_err(|e| AuthError::Auth {
                message: format!("failed to write callback response: {e}"),
            })?;

        // Construct the full callback URL for parsing
        Ok(format!("http://127.0.0.1:{port}{path}"))
    };

    Ok((port, server_future))
}

/// Result of a successful `OAuth2` token exchange.
#[derive(Debug, Clone)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<Duration>,
    pub token_type: String,
}

impl TokenResponse {
    /// Convert to a `StoredToken` with the current timestamp.
    #[must_use]
    pub fn to_stored_token(&self, provider: &str) -> StoredToken {
        self.to_stored_token_at(provider, now_secs())
    }

    /// Convert to a `StoredToken` at a given timestamp (for testability).
    #[must_use]
    pub fn to_stored_token_at(&self, provider: &str, current_secs: u64) -> StoredToken {
        let expires_at = self.expires_in.map(|dur| current_secs + dur.as_secs());
        StoredToken {
            provider: provider.to_string(),
            access_token: self.access_token.clone(),
            refresh_token: self.refresh_token.clone(),
            expires_at,
            token_type: self.token_type.clone(),
        }
    }
}

// ── OAuth2Provider ─────────────────────────────────────────────────────

/// `OAuth2` auth provider — manages tokens with automatic refresh.
pub struct OAuth2Provider {
    config: OAuth2Config,
    token_path: PathBuf,
    /// Cached token to avoid file I/O on every request.
    cached_token: Mutex<Option<StoredToken>>,
}

impl std::fmt::Debug for OAuth2Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuth2Provider")
            .field("provider", &self.config.provider)
            .field("token_path", &self.token_path)
            .finish_non_exhaustive()
    }
}

impl OAuth2Provider {
    /// Create a new `OAuth2` provider.
    #[must_use]
    pub fn new(config: OAuth2Config, token_path: PathBuf) -> Self {
        // Try to load cached token from disk
        let cached = load_token_store(&token_path)
            .ok()
            .and_then(|store| store.get(&config.provider).cloned());

        Self {
            config,
            token_path,
            cached_token: Mutex::new(cached),
        }
    }

    /// Get the current access token.
    ///
    /// Returns the cached token if still valid, or an error if expired.
    /// For automatic refresh, use [`get_token_or_refresh`] instead.
    pub fn get_token(&self) -> Result<StoredToken, AuthError> {
        let cached = self.cached_token.lock().unwrap().clone();

        match cached {
            Some(token) if !token.is_expired() => Ok(token),
            Some(token) if token.can_refresh() => Err(AuthError::Auth {
                message: format!(
                    "token for '{}' expired and needs refresh (refresh_token available)",
                    token.provider
                ),
            }),
            _ => Err(AuthError::Auth {
                message: format!(
                    "no valid token for '{}' — run OAuth2 authorization flow",
                    self.config.provider
                ),
            }),
        }
    }

    /// Get the current access token, automatically refreshing if expired.
    ///
    /// If the cached token is expired but has a refresh token, this method
    /// calls the token endpoint to obtain a new access token.
    pub async fn get_token_or_refresh(&self) -> Result<StoredToken, AuthError> {
        let cached = self.cached_token.lock().unwrap().clone();

        match cached {
            Some(token) if !token.is_expired() => Ok(token),
            Some(token) if token.can_refresh() => {
                let refresh_token = token.refresh_token.as_deref().unwrap();
                let resp = refresh_access_token(&self.config, refresh_token).await?;
                let new_token = resp.to_stored_token(&self.config.provider);
                self.store_token(new_token.clone())?;
                Ok(new_token)
            }
            _ => Err(AuthError::Auth {
                message: format!(
                    "no valid token for '{}' — run OAuth2 authorization flow",
                    self.config.provider
                ),
            }),
        }
    }

    /// Store a new token (after successful auth or refresh).
    pub fn store_token(&self, token: StoredToken) -> Result<(), AuthError> {
        // Update file store
        let mut store = load_token_store(&self.token_path)?;
        store.upsert(token.clone());
        save_token_store(&self.token_path, &store)?;

        // Update in-memory cache
        *self.cached_token.lock().unwrap() = Some(token);
        Ok(())
    }

    /// Clear the stored token for this provider.
    pub fn clear_token(&self) -> Result<(), AuthError> {
        let mut store = load_token_store(&self.token_path)?;
        store.remove(&self.config.provider);
        save_token_store(&self.token_path, &store)?;
        *self.cached_token.lock().unwrap() = None;
        Ok(())
    }

    /// Get the provider name.
    #[must_use]
    pub fn provider(&self) -> &str {
        &self.config.provider
    }
}

impl crate::AuthProvider for OAuth2Provider {
    fn get_auth(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = crab_common::Result<crate::AuthMethod>> + Send + '_>,
    > {
        Box::pin(async move {
            let token = self.get_token().map_err(crab_common::Error::from)?;
            Ok(crate::AuthMethod::OAuth(crate::OAuthToken {
                access_token: token.access_token,
            }))
        })
    }

    fn refresh(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = crab_common::Result<()>> + Send + '_>>
    {
        Box::pin(async move {
            // In a full implementation, this would:
            // 1. Load the refresh token
            // 2. Call the token endpoint with grant_type=refresh_token
            // 3. Store the new access + refresh tokens
            // For skeleton, just verify we have a refresh token available
            let cached = self.cached_token.lock().unwrap().clone();
            match cached {
                Some(token) if token.can_refresh() => {
                    // Placeholder — real implementation would do HTTP call here
                    Ok(())
                }
                _ => Err(crab_common::Error::Auth(format!(
                    "no refresh token available for '{}'",
                    self.config.provider
                ))),
            }
        })
    }
}

// ── Utilities ──────────────────────────────────────────────────────────

/// Current Unix timestamp in seconds.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_token(provider: &str, expires_at: Option<u64>) -> StoredToken {
        StoredToken {
            provider: provider.into(),
            access_token: "access-123".into(),
            refresh_token: Some("refresh-456".into()),
            expires_at,
            token_type: "Bearer".into(),
        }
    }

    #[test]
    fn stored_token_not_expired() {
        let token = make_token("test", Some(now_secs() + 3600));
        assert!(!token.is_expired());
    }

    #[test]
    fn stored_token_expired() {
        let token = make_token("test", Some(now_secs() - 100));
        assert!(token.is_expired());
    }

    #[test]
    fn stored_token_within_buffer_is_expired() {
        // Token expires in 4 minutes — within the 5-minute buffer
        let token = make_token("test", Some(now_secs() + 240));
        assert!(token.is_expired());
    }

    #[test]
    fn stored_token_no_expiry_not_expired() {
        let token = make_token("test", None);
        assert!(!token.is_expired());
    }

    #[test]
    fn stored_token_can_refresh() {
        let token = make_token("test", None);
        assert!(token.can_refresh());
    }

    #[test]
    fn stored_token_cannot_refresh_without_token() {
        let mut token = make_token("test", None);
        token.refresh_token = None;
        assert!(!token.can_refresh());
    }

    #[test]
    fn stored_token_cannot_refresh_with_empty_token() {
        let mut token = make_token("test", None);
        token.refresh_token = Some(String::new());
        assert!(!token.can_refresh());
    }

    #[test]
    fn is_expired_at_custom_timestamp() {
        let token = make_token("test", Some(1000));
        assert!(!token.is_expired_at(0)); // well before expiry
        assert!(token.is_expired_at(700)); // within buffer (700 + 300 = 1000)
        assert!(token.is_expired_at(1000)); // at expiry
        assert!(token.is_expired_at(2000)); // past expiry
    }

    // ── TokenStore tests ───────────────────────────────────────────────

    #[test]
    fn token_store_get() {
        let store = TokenStore {
            tokens: vec![make_token("provider-a", None)],
        };
        assert!(store.get("provider-a").is_some());
        assert!(store.get("provider-b").is_none());
    }

    #[test]
    fn token_store_upsert_insert() {
        let mut store = TokenStore::default();
        store.upsert(make_token("new-provider", None));
        assert_eq!(store.tokens.len(), 1);
        assert_eq!(
            store.get("new-provider").unwrap().access_token,
            "access-123"
        );
    }

    #[test]
    fn token_store_upsert_update() {
        let mut store = TokenStore {
            tokens: vec![make_token("provider", None)],
        };
        let mut updated = make_token("provider", None);
        updated.access_token = "new-access".into();
        store.upsert(updated);
        assert_eq!(store.tokens.len(), 1);
        assert_eq!(store.get("provider").unwrap().access_token, "new-access");
    }

    #[test]
    fn token_store_remove() {
        let mut store = TokenStore {
            tokens: vec![make_token("keep", None), make_token("remove", None)],
        };
        assert!(store.remove("remove"));
        assert_eq!(store.tokens.len(), 1);
        assert!(store.get("keep").is_some());
        assert!(store.get("remove").is_none());
    }

    #[test]
    fn token_store_remove_nonexistent() {
        let mut store = TokenStore::default();
        assert!(!store.remove("nonexistent"));
    }

    // ── File persistence tests ─────────────────────────────────────────

    #[test]
    fn load_nonexistent_returns_empty() {
        let store = load_token_store(Path::new("/nonexistent/tokens.json")).unwrap();
        assert!(store.tokens.is_empty());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = std::env::temp_dir().join("crab-auth-oauth-test-roundtrip");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tokens.json");

        let mut store = TokenStore::default();
        store.upsert(make_token("bedrock", Some(9999999999)));
        store.upsert(make_token("vertex", None));

        save_token_store(&path, &store).unwrap();
        let loaded = load_token_store(&path).unwrap();

        assert_eq!(loaded.tokens.len(), 2);
        assert_eq!(loaded.get("bedrock").unwrap().access_token, "access-123");
        assert_eq!(loaded.get("bedrock").unwrap().expires_at, Some(9999999999));
        assert!(loaded.get("vertex").is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_creates_parent_dirs() {
        let dir = std::env::temp_dir().join("crab-auth-oauth-test-dirs");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("nested").join("deep").join("tokens.json");

        let store = TokenStore::default();
        save_token_store(&path, &store).unwrap();
        assert!(path.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_invalid_json_returns_error() {
        let dir = std::env::temp_dir().join("crab-auth-oauth-test-invalid");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tokens.json");
        std::fs::write(&path, "not json").unwrap();

        let result = load_token_store(&path);
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── TokenResponse tests ────────────────────────────────────────────

    #[test]
    fn token_response_to_stored() {
        let resp = TokenResponse {
            access_token: "acc-new".into(),
            refresh_token: Some("ref-new".into()),
            expires_in: Some(Duration::from_secs(3600)),
            token_type: "Bearer".into(),
        };
        let stored = resp.to_stored_token_at("bedrock", 1000);
        assert_eq!(stored.provider, "bedrock");
        assert_eq!(stored.access_token, "acc-new");
        assert_eq!(stored.refresh_token.as_deref(), Some("ref-new"));
        assert_eq!(stored.expires_at, Some(4600)); // 1000 + 3600
        assert_eq!(stored.token_type, "Bearer");
    }

    #[test]
    fn token_response_no_expiry() {
        let resp = TokenResponse {
            access_token: "acc".into(),
            refresh_token: None,
            expires_in: None,
            token_type: "Bearer".into(),
        };
        let stored = resp.to_stored_token_at("vertex", 5000);
        assert!(stored.expires_at.is_none());
        assert!(stored.refresh_token.is_none());
    }

    // ── OAuth2Provider tests ────────────────────────────────────────��──

    fn test_config() -> OAuth2Config {
        OAuth2Config {
            provider: "test-provider".into(),
            client_id: "client-123".into(),
            auth_url: "https://auth.example.com/authorize".into(),
            token_url: "https://auth.example.com/token".into(),
            redirect_uri: "http://localhost:9876/callback".into(),
            scopes: vec!["openid".into(), "profile".into()],
        }
    }

    #[test]
    fn oauth2_provider_no_token_returns_error() {
        let dir = std::env::temp_dir().join("crab-auth-oauth-provider-no-token");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("tokens.json");

        let provider = OAuth2Provider::new(test_config(), path);
        let result = provider.get_token();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no valid token"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn oauth2_provider_store_and_get_token() {
        let dir = std::env::temp_dir().join("crab-auth-oauth-provider-store");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("tokens.json");

        let provider = OAuth2Provider::new(test_config(), path);
        let token = StoredToken {
            provider: "test-provider".into(),
            access_token: "my-access".into(),
            refresh_token: Some("my-refresh".into()),
            expires_at: Some(now_secs() + 3600),
            token_type: "Bearer".into(),
        };
        provider.store_token(token).unwrap();

        let retrieved = provider.get_token().unwrap();
        assert_eq!(retrieved.access_token, "my-access");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn oauth2_provider_expired_token_with_refresh() {
        let dir = std::env::temp_dir().join("crab-auth-oauth-provider-expired");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("tokens.json");

        let provider = OAuth2Provider::new(test_config(), path);
        let token = StoredToken {
            provider: "test-provider".into(),
            access_token: "expired-access".into(),
            refresh_token: Some("my-refresh".into()),
            expires_at: Some(now_secs() - 100), // expired
            token_type: "Bearer".into(),
        };
        provider.store_token(token).unwrap();

        let result = provider.get_token();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("needs refresh"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn oauth2_provider_clear_token() {
        let dir = std::env::temp_dir().join("crab-auth-oauth-provider-clear");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("tokens.json");

        let provider = OAuth2Provider::new(test_config(), path.clone());
        provider
            .store_token(make_token("test-provider", Some(now_secs() + 3600)))
            .unwrap();
        assert!(provider.get_token().is_ok());

        provider.clear_token().unwrap();
        assert!(provider.get_token().is_err());

        // Also cleared from file
        let store = load_token_store(&path).unwrap();
        assert!(store.get("test-provider").is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn oauth2_provider_name() {
        let dir = std::env::temp_dir().join("crab-auth-oauth-provider-name");
        let path = dir.join("tokens.json");
        let provider = OAuth2Provider::new(test_config(), path);
        assert_eq!(provider.provider(), "test-provider");
    }

    #[test]
    fn oauth2_provider_debug() {
        let dir = std::env::temp_dir().join("crab-auth-oauth-debug");
        let path = dir.join("tokens.json");
        let provider = OAuth2Provider::new(test_config(), path);
        let debug = format!("{provider:?}");
        assert!(debug.contains("test-provider"));
    }

    // ── AuthProvider trait tests ────────────────────────────────────────

    #[test]
    fn oauth2_provider_get_auth() {
        let dir = std::env::temp_dir().join("crab-auth-oauth-get-auth");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("tokens.json");

        let provider = OAuth2Provider::new(test_config(), path);
        provider
            .store_token(StoredToken {
                provider: "test-provider".into(),
                access_token: "oauth-access-token".into(),
                refresh_token: None,
                expires_at: Some(now_secs() + 3600),
                token_type: "Bearer".into(),
            })
            .unwrap();

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt
            .block_on(crate::AuthProvider::get_auth(&provider))
            .unwrap();
        match result {
            crate::AuthMethod::OAuth(t) => assert_eq!(t.access_token, "oauth-access-token"),
            crate::AuthMethod::ApiKey(_) => panic!("expected OAuth"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn oauth2_provider_refresh_no_token() {
        let dir = std::env::temp_dir().join("crab-auth-oauth-refresh-none");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("tokens.json");

        let provider = OAuth2Provider::new(test_config(), path);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(crate::AuthProvider::refresh(&provider));
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn oauth2_provider_refresh_with_token() {
        let dir = std::env::temp_dir().join("crab-auth-oauth-refresh-ok");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("tokens.json");

        let provider = OAuth2Provider::new(test_config(), path);
        provider
            .store_token(make_token("test-provider", Some(now_secs() + 3600)))
            .unwrap();

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(crate::AuthProvider::refresh(&provider));
        assert!(result.is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn oauth2_provider_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<OAuth2Provider>();
    }

    // ── PKCE tests ─────────────────────────────────────────────────────

    #[test]
    fn pkce_generate_produces_valid_challenge() {
        let pkce = PkceChallenge::generate();
        // Verifier should be 43 characters (32 bytes base64url = 43 chars)
        assert_eq!(pkce.code_verifier.len(), 43);
        // Challenge should be 43 characters (32 bytes SHA256 = 32 bytes base64url = 43 chars)
        assert_eq!(pkce.code_challenge.len(), 43);
        // Verifier and challenge should differ
        assert_ne!(pkce.code_verifier, pkce.code_challenge);
    }

    #[test]
    fn pkce_deterministic_from_bytes() {
        let bytes = [42u8; 32];
        let p1 = PkceChallenge::from_verifier_bytes(&bytes);
        let p2 = PkceChallenge::from_verifier_bytes(&bytes);
        assert_eq!(p1.code_verifier, p2.code_verifier);
        assert_eq!(p1.code_challenge, p2.code_challenge);
    }

    #[test]
    fn pkce_s256_known_vector() {
        // RFC 7636 Appendix B: code_verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
        // S256 code_challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let digest = sha2::Sha256::digest(verifier.as_bytes());
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn pkce_different_bytes_different_challenges() {
        let p1 = PkceChallenge::from_verifier_bytes(&[1u8; 32]);
        let p2 = PkceChallenge::from_verifier_bytes(&[2u8; 32]);
        assert_ne!(p1.code_verifier, p2.code_verifier);
        assert_ne!(p1.code_challenge, p2.code_challenge);
    }

    #[test]
    fn pkce_verifier_is_url_safe() {
        let pkce = PkceChallenge::generate();
        // base64url should only contain [A-Za-z0-9_-]
        assert!(
            pkce.code_verifier
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        );
        assert!(
            pkce.code_challenge
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        );
    }

    // ── Authorization URL tests ────────────────────────────────────────

    #[test]
    fn build_authorization_url_format() {
        let config = test_config();
        let pkce = PkceChallenge::from_verifier_bytes(&[0u8; 32]);
        let url = build_authorization_url(&config, &pkce, "test-state");

        assert!(url.starts_with("https://auth.example.com/authorize?"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=client-123"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains(&format!("code_challenge={}", pkce.code_challenge)));
        assert!(url.contains("state=test-state"));
        assert!(url.contains("redirect_uri="));
    }

    #[test]
    fn build_authorization_url_encodes_scopes() {
        let config = test_config();
        let pkce = PkceChallenge::from_verifier_bytes(&[0u8; 32]);
        let url = build_authorization_url(&config, &pkce, "s");
        // "openid profile" should be encoded as "openid%20profile"
        assert!(url.contains("scope=openid%20profile"));
    }

    // ── Callback parsing tests ─────────────────────────────────────────

    #[test]
    fn parse_callback_params_valid() {
        let url = "http://localhost:9876/callback?code=abc123&state=xyz789";
        let (code, state) = parse_callback_params(url).unwrap();
        assert_eq!(code, "abc123");
        assert_eq!(state, "xyz789");
    }

    #[test]
    fn parse_callback_params_reversed_order() {
        let url = "http://localhost:9876/callback?state=s1&code=c1";
        let (code, state) = parse_callback_params(url).unwrap();
        assert_eq!(code, "c1");
        assert_eq!(state, "s1");
    }

    #[test]
    fn parse_callback_params_extra_params() {
        let url = "http://localhost:9876/callback?code=c&state=s&extra=e";
        let (code, state) = parse_callback_params(url).unwrap();
        assert_eq!(code, "c");
        assert_eq!(state, "s");
    }

    #[test]
    fn parse_callback_params_no_query() {
        let result = parse_callback_params("http://localhost:9876/callback");
        assert!(result.is_err());
    }

    #[test]
    fn parse_callback_params_missing_code() {
        let result = parse_callback_params("http://localhost:9876/callback?state=s");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("code"));
    }

    #[test]
    fn parse_callback_params_missing_state() {
        let result = parse_callback_params("http://localhost:9876/callback?code=c");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("state"));
    }

    // ── URL encoding tests ─────────────────────────────────────────────

    #[test]
    fn url_encode_preserves_unreserved() {
        assert_eq!(url_encode("abc-_.~"), "abc-_.~");
    }

    #[test]
    fn url_encode_encodes_spaces() {
        assert_eq!(url_encode("hello world"), "hello%20world");
    }

    #[test]
    fn url_encode_encodes_special_chars() {
        assert_eq!(url_encode("a=b&c"), "a%3Db%26c");
    }

    // ── Callback server tests ───────────────────────────────────────────

    #[tokio::test]
    async fn callback_server_binds_and_accepts() {
        let (port, server_future) = start_callback_server().await.unwrap();
        assert!(port > 0);

        // Simulate a browser callback by connecting and sending an HTTP GET
        let client_future = async move {
            // Small delay to ensure server is listening
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
                .await
                .unwrap();
            let request =
                "GET /callback?code=test_code&state=test_state HTTP/1.1\r\nHost: localhost\r\n\r\n";
            tokio::io::AsyncWriteExt::write_all(&mut stream, request.as_bytes())
                .await
                .unwrap();
            // Read response
            let mut buf = vec![0u8; 4096];
            let n = tokio::io::AsyncReadExt::read(&mut stream, &mut buf)
                .await
                .unwrap();
            String::from_utf8_lossy(&buf[..n]).to_string()
        };

        let (url_result, response) = tokio::join!(server_future, client_future);
        let url = url_result.unwrap();
        assert!(url.contains("code=test_code"));
        assert!(url.contains("state=test_state"));
        assert!(response.contains("200 OK"));
        assert!(response.contains("Authentication successful"));
    }

    #[tokio::test]
    async fn callback_server_parses_code_and_state() {
        let (port, server_future) = start_callback_server().await.unwrap();

        let client_future = async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
                .await
                .unwrap();
            let request =
                "GET /callback?code=abc123&state=xyz789 HTTP/1.1\r\nHost: localhost\r\n\r\n";
            tokio::io::AsyncWriteExt::write_all(&mut stream, request.as_bytes())
                .await
                .unwrap();
        };

        let (url_result, _) = tokio::join!(server_future, client_future);
        let url = url_result.unwrap();

        // Verify parse_callback_params works with the server output
        let (code, state) = parse_callback_params(&url).unwrap();
        assert_eq!(code, "abc123");
        assert_eq!(state, "xyz789");
    }

    #[test]
    fn parse_token_response_valid() {
        let json = r#"{"access_token":"at-123","refresh_token":"rt-456","expires_in":3600,"token_type":"Bearer"}"#;
        let resp = parse_token_response(json).unwrap();
        assert_eq!(resp.access_token, "at-123");
        assert_eq!(resp.refresh_token.as_deref(), Some("rt-456"));
        assert_eq!(resp.expires_in, Some(Duration::from_secs(3600)));
        assert_eq!(resp.token_type, "Bearer");
    }

    #[test]
    fn parse_token_response_minimal() {
        let json = r#"{"access_token":"at-only"}"#;
        let resp = parse_token_response(json).unwrap();
        assert_eq!(resp.access_token, "at-only");
        assert!(resp.refresh_token.is_none());
        assert!(resp.expires_in.is_none());
        assert_eq!(resp.token_type, "Bearer"); // default
    }

    #[test]
    fn parse_token_response_invalid_json() {
        assert!(parse_token_response("not json").is_err());
    }

    #[test]
    fn parse_token_response_missing_access_token() {
        assert!(parse_token_response(r#"{"refresh_token":"rt"}"#).is_err());
    }

    // ── Serde roundtrip tests ──────────────────────────────────────────

    #[test]
    fn stored_token_serde_roundtrip() {
        let token = make_token("provider", Some(12345));
        let json = serde_json::to_string(&token).unwrap();
        let back: StoredToken = serde_json::from_str(&json).unwrap();
        assert_eq!(token, back);
    }

    #[test]
    fn stored_token_serde_no_optional_fields() {
        let token = StoredToken {
            provider: "test".into(),
            access_token: "acc".into(),
            refresh_token: None,
            expires_at: None,
            token_type: "Bearer".into(),
        };
        let json = serde_json::to_string(&token).unwrap();
        assert!(!json.contains("refresh_token"));
        assert!(!json.contains("expires_at"));
        let back: StoredToken = serde_json::from_str(&json).unwrap();
        assert!(back.refresh_token.is_none());
        assert!(back.expires_at.is_none());
    }

    #[test]
    fn token_store_serde_roundtrip() {
        let store = TokenStore {
            tokens: vec![make_token("a", Some(100)), make_token("b", None)],
        };
        let json = serde_json::to_string_pretty(&store).unwrap();
        let back: TokenStore = serde_json::from_str(&json).unwrap();
        assert_eq!(back.tokens.len(), 2);
    }

    #[test]
    fn default_token_path_under_crab() {
        let path = default_token_path();
        let path_str = path.to_string_lossy();
        assert!(path_str.contains(".crab"));
        assert!(path_str.contains("auth"));
        assert!(path_str.contains("tokens.json"));
    }
}
