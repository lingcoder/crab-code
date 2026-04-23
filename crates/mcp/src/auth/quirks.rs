//! Provider-specific HTTP response normalisation.
//!
//! Some OAuth providers deviate from RFC 6749 in ways that require
//! special handling before generic JSON parsing. This module centralises
//! those deviations so the rest of the auth flow can treat every
//! provider uniformly.
//!
//! ## Slack 200 + `{"ok":false}`
//!
//! Slack's `oauth.v2.access` returns HTTP 200 even for auth failures,
//! with an error indicator embedded in the JSON body (`"ok":false,"error":"..."`).
//! Treating a 200 as success and handing the body to a generic JSON
//! parser would silently produce bogus `AuthToken`s. We detect the
//! marker and upgrade the status to 400 so the caller's
//! `status.is_success()` check fires.

use reqwest::StatusCode;

/// Read the response status + body, applying provider quirks before
/// returning. This does not parse JSON — it just adjusts the `(status, body)`
/// pair so downstream code can treat failures uniformly.
///
/// # Errors
///
/// Returns `Err` if the body cannot be read.
pub async fn normalise_response(
    resp: reqwest::Response,
) -> crab_core::Result<(StatusCode, String)> {
    let mut status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| crab_core::Error::Other(format!("read response body failed: {e}")))?;

    if status == StatusCode::OK && looks_like_slack_error(&body) {
        tracing::debug!("normalising Slack 200+error body → 400");
        status = StatusCode::BAD_REQUEST;
    }

    Ok((status, body))
}

/// Heuristic: the body parses as JSON, is an object, has `"ok":false`.
/// We keep this narrow so non-Slack providers with a literal `"ok":false`
/// inside an unrelated field (unlikely but possible) aren't misflagged.
fn looks_like_slack_error(body: &str) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return false;
    };
    match v {
        serde_json::Value::Object(ref map) => {
            map.get("ok") == Some(&serde_json::Value::Bool(false))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_slack_error_shape() {
        assert!(looks_like_slack_error(
            r#"{"ok":false,"error":"invalid_code"}"#
        ));
    }

    #[test]
    fn does_not_flag_slack_success() {
        assert!(!looks_like_slack_error(
            r#"{"ok":true,"access_token":"tok"}"#
        ));
    }

    #[test]
    fn does_not_flag_non_slack_providers() {
        assert!(!looks_like_slack_error(
            r#"{"access_token":"tok","token_type":"Bearer"}"#
        ));
    }

    #[test]
    fn does_not_flag_non_object_bodies() {
        assert!(!looks_like_slack_error("null"));
        assert!(!looks_like_slack_error("\"string body\""));
        assert!(!looks_like_slack_error("[1, 2, 3]"));
    }

    #[test]
    fn does_not_flag_unparseable_bodies() {
        assert!(!looks_like_slack_error(""));
        assert!(!looks_like_slack_error("not json at all"));
    }

    #[test]
    fn does_not_flag_ok_as_string() {
        // `"ok":"false"` (string) shouldn't trigger — we want literal
        // boolean false.
        assert!(!looks_like_slack_error(r#"{"ok":"false"}"#));
    }
}
