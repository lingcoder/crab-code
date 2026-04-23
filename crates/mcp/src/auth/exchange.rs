//! Authorization-code → access-token exchange (RFC 6749 §4.1.3).
//!
//! After the browser returns to the callback with a code, this module
//! POSTs the code + PKCE verifier to the token endpoint and parses the
//! resulting `AuthToken`. Applies the [`super::quirks`] normaliser so
//! Slack's peculiar `200 + error body` responses still error correctly.

use std::time::SystemTime;

use serde::Deserialize;

use super::quirks;
use super::types::{AuthToken, OAuthConfig};

/// Response shape defined by RFC 6749 §5.1 + common extensions.
#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(default = "default_token_type")]
    pub token_type: String,
    pub expires_in: Option<u64>,
    pub refresh_token: Option<String>,
    /// Present if the provider echoes scopes back (space-separated).
    pub scope: Option<String>,
}

fn default_token_type() -> String {
    "Bearer".into()
}

impl TokenResponse {
    /// Convert into the crate-wide [`AuthToken`], computing the absolute
    /// expiry timestamp from `expires_in` if present.
    #[must_use]
    pub fn into_auth_token(self) -> AuthToken {
        let expires_at = self.expires_in.map(|seconds| {
            let now = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            now.saturating_add(seconds)
        });
        AuthToken {
            access_token: self.access_token,
            token_type: self.token_type,
            expires_at,
            refresh_token: self.refresh_token,
        }
    }
}

/// Exchange an authorization code for an access token.
///
/// Arguments:
/// - `http` — shared `reqwest::Client` (re-used so connection pool benefits)
/// - `config` — OAuth config; supplies `token_url`, `client_id`, optional secret
/// - `code` — the `code` returned by the callback
/// - `verifier` — the PKCE verifier paired with the `code_challenge` sent earlier
///
/// # Errors
///
/// Returns `Err` on network failure, non-2xx response (after [`quirks::normalise_response`]
/// adjusts Slack-style 200+error-body), or malformed JSON.
pub async fn exchange_code(
    http: &reqwest::Client,
    config: &OAuthConfig,
    code: &str,
    verifier: &str,
) -> crab_core::Result<AuthToken> {
    let mut params: Vec<(&str, &str)> = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", config.redirect_uri.as_str()),
        ("client_id", config.client_id.as_str()),
        ("code_verifier", verifier),
    ];
    if let Some(secret) = config.client_secret.as_deref() {
        params.push(("client_secret", secret));
    }

    let resp = http
        .post(&config.token_url)
        .form(&params)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| crab_core::Error::Other(format!("token endpoint POST failed: {e}")))?;

    let (status, body) = quirks::normalise_response(resp).await?;

    if !status.is_success() {
        return Err(crab_core::Error::Other(format!(
            "token exchange HTTP {status}: {body}"
        )));
    }

    let parsed: TokenResponse = serde_json::from_str(&body).map_err(|e| {
        crab_core::Error::Other(format!(
            "token exchange returned unparseable body: {e}; body was: {body}"
        ))
    })?;
    Ok(parsed.into_auth_token())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_response_parses_minimal() {
        let json = r#"{"access_token":"tok","token_type":"Bearer"}"#;
        let r: TokenResponse = serde_json::from_str(json).unwrap();
        assert_eq!(r.access_token, "tok");
        assert_eq!(r.token_type, "Bearer");
        assert!(r.expires_in.is_none());
        assert!(r.refresh_token.is_none());
    }

    #[test]
    fn token_response_parses_full() {
        let json = r#"{
            "access_token":"tok",
            "token_type":"Bearer",
            "expires_in":3600,
            "refresh_token":"r-tok",
            "scope":"read write"
        }"#;
        let r: TokenResponse = serde_json::from_str(json).unwrap();
        assert_eq!(r.expires_in, Some(3600));
        assert_eq!(r.refresh_token.as_deref(), Some("r-tok"));
    }

    #[test]
    fn default_token_type_is_bearer() {
        let json = r#"{"access_token":"tok"}"#;
        let r: TokenResponse = serde_json::from_str(json).unwrap();
        assert_eq!(r.token_type, "Bearer");
    }

    #[test]
    fn into_auth_token_computes_expiry() {
        let r = TokenResponse {
            access_token: "a".into(),
            token_type: "Bearer".into(),
            expires_in: Some(10),
            refresh_token: Some("r".into()),
            scope: None,
        };
        let t = r.into_auth_token();
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let exp = t.expires_at.unwrap();
        assert!(exp >= now && exp <= now + 11);
        assert_eq!(t.refresh_token.as_deref(), Some("r"));
    }

    #[test]
    fn into_auth_token_without_expiry_stays_none() {
        let r = TokenResponse {
            access_token: "a".into(),
            token_type: "Bearer".into(),
            expires_in: None,
            refresh_token: None,
            scope: None,
        };
        let t = r.into_auth_token();
        assert!(t.expires_at.is_none());
        assert!(!t.is_expired());
    }
}
