//! Refresh-token flow (RFC 6749 §6).
//!
//! Exchanges a refresh token for a new access token without involving
//! the browser. Used when [`AuthToken::is_expired`](super::types::AuthToken::is_expired)
//! reports true and the stored token has a `refresh_token`.

use super::exchange::TokenResponse;
use super::quirks;
use super::types::{AuthToken, OAuthConfig};

/// Refresh an access token using a previously issued refresh token.
///
/// # Errors
///
/// Returns `Err` on network failure, non-2xx from the token endpoint, or
/// a malformed response body.
pub async fn refresh_token(
    http: &reqwest::Client,
    config: &OAuthConfig,
    refresh_token: &str,
) -> crab_core::Result<AuthToken> {
    let mut params: Vec<(&str, &str)> = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", config.client_id.as_str()),
    ];
    if let Some(secret) = config.client_secret.as_deref() {
        params.push(("client_secret", secret));
    }
    // Most providers accept (but don't require) scope on refresh; if we
    // have any configured scopes, pass them to maintain parity with the
    // initial grant. Slack for instance echoes scopes back.
    let scope_joined;
    if !config.scopes.is_empty() {
        scope_joined = config.scopes.join(" ");
        params.push(("scope", scope_joined.as_str()));
    }

    let resp = http
        .post(&config.token_url)
        .form(&params)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| crab_core::Error::Other(format!("refresh endpoint POST failed: {e}")))?;

    let (status, body) = quirks::normalise_response(resp).await?;
    if !status.is_success() {
        return Err(crab_core::Error::Other(format!(
            "token refresh HTTP {status}: {body}"
        )));
    }

    let mut parsed: TokenResponse = serde_json::from_str(&body).map_err(|e| {
        crab_core::Error::Other(format!(
            "token refresh returned unparseable body: {e}; body was: {body}"
        ))
    })?;

    // RFC 6749 §6: the response may omit `refresh_token`, in which case
    // the client SHOULD keep using the prior one. Preserve caller's
    // refresh token if the provider didn't rotate it.
    if parsed.refresh_token.is_none() {
        parsed.refresh_token = Some(refresh_token.to_string());
    }
    Ok(parsed.into_auth_token())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_response_preserves_input_refresh_when_omitted() {
        // Simulating what happens inside refresh_token: parse, then fill
        // the refresh back in if missing.
        let body = r#"{"access_token":"new","token_type":"Bearer","expires_in":3600}"#;
        let mut parsed: TokenResponse = serde_json::from_str(body).unwrap();
        assert!(parsed.refresh_token.is_none());

        let input_refresh = "original-refresh";
        if parsed.refresh_token.is_none() {
            parsed.refresh_token = Some(input_refresh.to_string());
        }
        let tok = parsed.into_auth_token();
        assert_eq!(tok.refresh_token.as_deref(), Some("original-refresh"));
        assert_eq!(tok.access_token, "new");
    }

    #[test]
    fn refresh_response_accepts_rotated_refresh() {
        let body = r#"{
            "access_token":"new",
            "token_type":"Bearer",
            "expires_in":3600,
            "refresh_token":"rotated"
        }"#;
        let parsed: TokenResponse = serde_json::from_str(body).unwrap();
        let tok = parsed.into_auth_token();
        assert_eq!(tok.refresh_token.as_deref(), Some("rotated"));
    }
}
