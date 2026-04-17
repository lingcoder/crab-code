//! MCP server authentication: `OAuth2` + API keys + token refresh.
//!
//! Supports three authentication methods for MCP server connections:
//! - [`McpAuthMethod::None`] — no authentication
//! - [`McpAuthMethod::ApiKey`] — static API key sent as header or query parameter
//! - [`McpAuthMethod::OAuth2`] — authorisation code flow with PKCE, full browser round-trip
//!
//! ## Module layout
//!
//! - [`types`] — `McpAuthMethod` / `ApiKeyConfig` / `OAuthConfig` / `AuthToken`
//! - [`api_key`] — API-key resolver (env-var expansion)
//! - [`pkce`] — PKCE code verifier + SHA-256 challenge (RFC 7636)
//! - [`discovery`] — RFC 9728 + RFC 8414 metadata discovery
//! - [`flow`] — authorisation URL construction + CSRF state generation
//! - [`callback`] — localhost HTTP callback server for the redirect
//! - [`exchange`] — authorisation code → access token (RFC 6749 §4.1.3)
//! - [`refresh`] — refresh token → new access token (RFC 6749 §6)
//! - [`quirks`] — provider quirks (Slack 200-with-error-body → 400)
//! - [`store`] — persistent per-server token store (`~/.crab/mcp/tokens/`)
//! - [`manager`] — [`McpAuthManager`] that coordinates all the above

pub mod api_key;
pub mod callback;
pub mod discovery;
pub mod exchange;
pub mod flow;
pub mod manager;
pub mod pkce;
pub mod quirks;
pub mod refresh;
pub mod store;
pub mod types;

pub use callback::{CallbackResult, await_callback, redirect_uri_addr};
pub use discovery::{
    AuthServerMetadata, ResourceMetadata, discover_auth_server, discover_resource,
};
pub use exchange::{TokenResponse, exchange_code};
pub use flow::{AuthorizationRequest, random_state};
pub use manager::{DEFAULT_BROWSER_TIMEOUT, McpAuthManager};
pub use pkce::PkceChallenge;
pub use store::{TokenStore, default_token_dir};
pub use types::{ApiKeyConfig, AuthToken, McpAuthMethod, OAuthConfig};
