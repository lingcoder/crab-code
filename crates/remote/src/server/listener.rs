//! HTTP/WS listener built on axum.
//!
//! Accepts WebSocket upgrades on a single route (`/`), validates the
//! JWT in the `Authorization` header, and hands the upgraded socket off
//! to [`super::dispatch::run_connection`]. Loopback-only by default (see
//! [`super::ServerConfig::bind`]); callers who want to expose the server
//! to a LAN or Tailscale should set `bind = "0.0.0.0:PORT"` explicitly.

use std::sync::Arc;

use axum::Router;
use axum::extract::{State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tokio_util::sync::CancellationToken;

use super::ServerConfig;
use super::dispatch::run_connection;
use super::handler::SessionHandler;

/// Header the crab-proto client sends the JWT in.
///
/// Canonical form in capital prefix to match the `Authorization: Bearer`
/// convention; value is `"Bearer <token>"`.
pub const AUTH_HEADER: &str = "authorization";

/// Bearer prefix stripped off [`AUTH_HEADER`] before verification.
pub const BEARER_PREFIX: &str = "Bearer ";

/// Shared state handed to each axum handler: the config (for `jwt_secret`)
/// plus the `SessionHandler` and the cancel token.
#[derive(Clone)]
struct AppState {
    config: Arc<ServerConfig>,
    handler: Arc<dyn SessionHandler>,
    cancel: CancellationToken,
    server_name: String,
}

/// Top-level server. Holds config + handler and can be `serve()`d.
pub struct RemoteServer {
    config: Arc<ServerConfig>,
    handler: Arc<dyn SessionHandler>,
    server_name: String,
}

impl RemoteServer {
    /// Build a new server. Does not bind the port; call [`serve`] for that.
    ///
    /// `server_name` is reported back to clients during the `initialize`
    /// handshake — useful when multiple crab instances run on the same LAN.
    pub fn new(
        config: ServerConfig,
        handler: Arc<dyn SessionHandler>,
        server_name: impl Into<String>,
    ) -> Self {
        Self {
            config: Arc::new(config),
            handler,
            server_name: server_name.into(),
        }
    }

    /// Bind the listener and serve until `cancel` fires.
    ///
    /// Returns `Ok(())` on graceful shutdown (cancel token fired with no
    /// in-flight connections); returns an error only if the initial bind
    /// or config validation fails.
    pub async fn serve(&self, cancel: CancellationToken) -> Result<(), ServeError> {
        self.config.validate()?;

        let state = AppState {
            config: Arc::clone(&self.config),
            handler: Arc::clone(&self.handler),
            cancel: cancel.clone(),
            server_name: self.server_name.clone(),
        };

        let app = Router::new().route("/", get(ws_upgrade)).with_state(state);

        let listener = tokio::net::TcpListener::bind(self.config.bind)
            .await
            .map_err(ServeError::Bind)?;

        tracing::info!(addr = %self.config.bind, "remote server listening");

        let serve_fut = axum::serve(listener, app.into_make_service())
            .with_graceful_shutdown(async move { cancel.cancelled().await });

        serve_fut.await.map_err(ServeError::Serve)
    }
}

/// Errors from starting or running the server. Validation and bind
/// errors are separated so callers can distinguish "user config is
/// broken" from "port already in use".
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    #[error("configuration invalid: {0}")]
    Config(#[from] super::config::ServerConfigError),
    #[error("failed to bind: {0}")]
    Bind(#[source] std::io::Error),
    #[error("serve loop error: {0}")]
    Serve(#[source] std::io::Error),
}

/// axum handler for `GET /` that upgrades to a WebSocket if the JWT
/// checks out, or responds 401 if it does not.
async fn ws_upgrade(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let secret = state.config.jwt_secret.as_bytes();
    match extract_and_verify(&headers, secret) {
        Ok(_claims) => {
            // Claims are not handed into the dispatch task in this
            // phase — they become relevant when session/attach needs to
            // gate by `sub`. Stored in state on a follow-up phase.
            let handler = Arc::clone(&state.handler);
            let cancel = state.cancel.clone();
            let name = state.server_name.clone();
            ws.on_upgrade(move |socket| async move {
                run_connection(socket, handler, cancel, name).await;
            })
        }
        Err(e) => {
            tracing::debug!(error = %e, "rejecting ws upgrade: bad auth");
            (StatusCode::UNAUTHORIZED, e.to_string()).into_response()
        }
    }
}

/// Pull the bearer token out of `headers` and verify its signature.
fn extract_and_verify(
    headers: &HeaderMap,
    secret: &[u8],
) -> Result<crate::auth::jwt::Claims, AuthError> {
    let raw = headers.get(AUTH_HEADER).ok_or(AuthError::Missing)?;
    let raw = raw.to_str().map_err(|_| AuthError::NonAscii)?;
    let token = raw
        .strip_prefix(BEARER_PREFIX)
        .ok_or(AuthError::WrongPrefix)?;
    crate::auth::jwt::verify(secret, token).map_err(AuthError::Verify)
}

#[derive(Debug, thiserror::Error)]
enum AuthError {
    #[error("missing {AUTH_HEADER} header")]
    Missing,
    #[error("{AUTH_HEADER} header is not valid ASCII")]
    NonAscii,
    #[error("expected `Bearer <token>` prefix")]
    WrongPrefix,
    #[error("invalid token: {0}")]
    Verify(#[source] crate::auth::jwt::JwtError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    const SECRET: &[u8] = b"a-shared-secret-at-least-32-bytes!";

    fn headers_with(auth: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(AUTH_HEADER, HeaderValue::from_str(auth).unwrap());
        h
    }

    #[test]
    fn extract_rejects_missing_header() {
        let h = HeaderMap::new();
        assert!(matches!(
            extract_and_verify(&h, SECRET).unwrap_err(),
            AuthError::Missing
        ));
    }

    #[test]
    fn extract_rejects_wrong_prefix() {
        let h = headers_with("Basic dXNlcjpwYXNz");
        assert!(matches!(
            extract_and_verify(&h, SECRET).unwrap_err(),
            AuthError::WrongPrefix
        ));
    }

    #[test]
    fn extract_accepts_valid_bearer() {
        let token = crate::auth::jwt::sign(SECRET, "sess_1", "dev_home", 60).unwrap();
        let h = headers_with(&format!("Bearer {token}"));
        let claims = extract_and_verify(&h, SECRET).unwrap();
        assert_eq!(claims.sub, "sess_1");
    }

    #[test]
    fn extract_rejects_invalid_signature() {
        let token = crate::auth::jwt::sign(SECRET, "s", "d", 60).unwrap();
        let h = headers_with(&format!("Bearer {token}"));
        let other_secret: &[u8] = b"a-totally-different-secret-at-least-32!";
        assert!(matches!(
            extract_and_verify(&h, other_secret).unwrap_err(),
            AuthError::Verify(_)
        ));
    }
}
