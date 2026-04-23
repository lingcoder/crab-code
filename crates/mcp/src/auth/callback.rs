//! Localhost HTTP callback server for `OAuth2` authorization-code redirects.
//!
//! OAuth providers redirect the browser back to a URL like
//! `http://localhost:<port>/callback?code=...&state=...`. This module
//! spins up a one-shot HTTP listener on the port embedded in the
//! `redirect_uri`, captures the first request, parses out the `code`
//! and `state` query parameters, sends a friendly HTML response, and
//! shuts down.
//!
//! Implemented with bare `tokio::net::TcpListener` + a tiny HTTP parser
//! so we avoid pulling in `axum` for this one-shot path. Axum is already
//! an optional dep for `bridge`'s REST control plane; we keep MCP auth
//! light by not forcing that dependency.

use std::collections::HashMap;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Parsed callback parameters returned by the provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallbackResult {
    /// Authorisation code to exchange for a token. `None` when the
    /// provider returned an error.
    pub code: Option<String>,
    /// Echoed CSRF state. Must be verified by the caller against the
    /// original `AuthorizationRequest::state`.
    pub state: Option<String>,
    /// Provider-reported error code (e.g. `"access_denied"`), if any.
    pub error: Option<String>,
    /// Human-readable error description, if any.
    pub error_description: Option<String>,
}

impl CallbackResult {
    /// `Ok(code)` if the callback contained a code and no error,
    /// otherwise `Err` with the provider's error or a generic message.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the provider reported an error or if the callback
    /// contained neither a code nor an error.
    pub fn into_code(self) -> crab_core::Result<String> {
        if let Some(err) = self.error {
            let detail = self.error_description.unwrap_or_default();
            return Err(crab_core::Error::Other(format!(
                "OAuth provider returned error '{err}': {detail}"
            )));
        }
        self.code.ok_or_else(|| {
            crab_core::Error::Other(
                "OAuth callback missing both 'code' and 'error' parameters".into(),
            )
        })
    }
}

/// HTML shown to the user in the browser after the redirect lands.
///
/// Kept minimal and ASCII-only so it works over any encoding and can
/// be read at a glance in a terminal screenshot.
const SUCCESS_HTML: &str = "\
<!DOCTYPE html>
<html><head><title>Crab Code - Authorization Complete</title>
<meta charset=\"utf-8\">
<style>body{font-family:system-ui,sans-serif;max-width:520px;margin:4em auto;padding:2em;text-align:center;color:#1a1a1a}
h1{color:#2d7a2d}</style></head>
<body><h1>Authorization complete</h1>
<p>You can close this tab and return to Crab Code.</p>
</body></html>";

const ERROR_HTML: &str = "\
<!DOCTYPE html>
<html><head><title>Crab Code - Authorization Failed</title>
<meta charset=\"utf-8\">
<style>body{font-family:system-ui,sans-serif;max-width:520px;margin:4em auto;padding:2em;text-align:center;color:#1a1a1a}
h1{color:#a33}</style></head>
<body><h1>Authorization failed</h1>
<p>The provider reported an error. Check Crab Code for details.</p>
</body></html>";

/// Default per-request read timeout.
const READ_TIMEOUT: Duration = Duration::from_secs(2);

/// Default overall wait for the user to complete the browser auth.
pub const DEFAULT_WAIT: Duration = Duration::from_secs(300);

/// Bind a one-shot HTTP listener on `addr` (typically `"127.0.0.1:PORT"`),
/// wait up to `timeout` for the first inbound GET, and return the parsed
/// callback parameters.
///
/// Typical `addr` is parsed from the `OAuthConfig::redirect_uri` by
/// [`redirect_uri_addr`]. Passing `"127.0.0.1:0"` lets the OS pick a
/// free port, which callers can discover via the returned listener's
/// local address.
///
/// # Errors
///
/// Returns `Err` on bind failure, timeout, malformed HTTP request, or
/// IO errors while writing the response.
pub async fn await_callback(addr: &str, timeout: Duration) -> crab_core::Result<CallbackResult> {
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|e| crab_core::Error::Other(format!("bind {addr} failed: {e}")))?;

    let accept = async {
        let (mut stream, _peer) = listener
            .accept()
            .await
            .map_err(|e| crab_core::Error::Other(format!("accept failed: {e}")))?;

        let mut buf = vec![0u8; 4096];
        let n = match tokio::time::timeout(READ_TIMEOUT, stream.read(&mut buf)).await {
            Ok(Ok(n)) => n,
            Ok(Err(e)) => return Err(crab_core::Error::Other(format!("read failed: {e}"))),
            Err(_) => return Err(crab_core::Error::Other("callback read timeout".into())),
        };
        let req = std::str::from_utf8(&buf[..n]).map_err(|e| {
            crab_core::Error::Other(format!("callback HTTP request is not UTF-8: {e}"))
        })?;

        let parsed = parse_request_line(req);
        let body = if parsed.error.is_some() {
            ERROR_HTML
        } else {
            SUCCESS_HTML
        };
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        // Best-effort write: even if the browser dropped early, we have
        // the params parsed already so the caller can proceed.
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.shutdown().await;
        Ok(parsed)
    };

    match tokio::time::timeout(timeout, accept).await {
        Ok(result) => result,
        Err(_) => Err(crab_core::Error::Other(format!(
            "no callback received within {}s — user did not complete browser auth",
            timeout.as_secs()
        ))),
    }
}

/// Extract a socket address string from an OAuth redirect URI like
/// `http://localhost:12345/callback`.
///
/// Returns `Err` for non-HTTP schemes, missing host/port, or non-numeric
/// port. IPv6 brackets are supported.
///
/// # Errors
///
/// Returns `Err` when the URL cannot be parsed as `http://host:port/path`.
pub fn redirect_uri_addr(uri: &str) -> crab_core::Result<String> {
    // Cheap inline parser — the URL shape for OAuth callbacks is fixed.
    let Some(rest) = uri.strip_prefix("http://") else {
        return Err(crab_core::Error::Other(format!(
            "redirect_uri must be http:// (got {uri})"
        )));
    };
    let rest = rest.split_once('/').map_or(rest, |(host, _)| host);
    if rest.is_empty() {
        return Err(crab_core::Error::Other(format!(
            "redirect_uri missing host:port ({uri})"
        )));
    }
    Ok(rest.to_string())
}

/// Parse the HTTP request-line, extracting `code` / `state` / `error` from
/// the query string. Unknown parameters are ignored.
fn parse_request_line(req: &str) -> CallbackResult {
    let mut result = CallbackResult {
        code: None,
        state: None,
        error: None,
        error_description: None,
    };

    let Some(first_line) = req.lines().next() else {
        return result;
    };
    // first_line = "GET /callback?code=...&state=... HTTP/1.1"
    let parts: Vec<&str> = first_line.split_whitespace().collect();
    if parts.len() < 2 {
        return result;
    }
    let path_and_query = parts[1];
    let Some(query) = path_and_query.split_once('?').map(|(_, q)| q) else {
        return result;
    };

    let params: HashMap<String, String> = query
        .split('&')
        .filter(|s| !s.is_empty())
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            // Historically some providers use `+` for space in query
            // strings (application/x-www-form-urlencoded style). The
            // `urlencoding` crate only decodes `%20`, so normalise `+`
            // to space up front so callers don't see "User+cancelled".
            let v_normalised = v.replace('+', " ");
            let k = urlencoding::decode(k).ok()?.into_owned();
            let v = urlencoding::decode(&v_normalised).ok()?.into_owned();
            Some((k, v))
        })
        .collect();

    result.code = params.get("code").cloned();
    result.state = params.get("state").cloned();
    result.error = params.get("error").cloned();
    result.error_description = params.get("error_description").cloned();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_success_request_line() {
        let raw = "GET /callback?code=abc123&state=xyz789 HTTP/1.1\r\n\
                   Host: localhost:12345\r\n\r\n";
        let r = parse_request_line(raw);
        assert_eq!(r.code.as_deref(), Some("abc123"));
        assert_eq!(r.state.as_deref(), Some("xyz789"));
        assert!(r.error.is_none());
    }

    #[test]
    fn parse_error_request_line() {
        let raw = "GET /callback?error=access_denied&error_description=User+cancelled HTTP/1.1";
        let r = parse_request_line(raw);
        assert!(r.code.is_none());
        assert_eq!(r.error.as_deref(), Some("access_denied"));
        assert_eq!(r.error_description.as_deref(), Some("User cancelled"));
    }

    #[test]
    fn parse_no_query_yields_empty_result() {
        let raw = "GET /callback HTTP/1.1";
        let r = parse_request_line(raw);
        assert!(r.code.is_none());
        assert!(r.state.is_none());
    }

    #[test]
    fn parse_ignores_unknown_params() {
        let raw = "GET /callback?code=c&session_state=s&foo=bar HTTP/1.1";
        let r = parse_request_line(raw);
        assert_eq!(r.code.as_deref(), Some("c"));
    }

    #[test]
    fn parse_decodes_percent_encoding() {
        let raw = "GET /callback?code=a%2Bb%2Fc&state=s%20pace HTTP/1.1";
        let r = parse_request_line(raw);
        assert_eq!(r.code.as_deref(), Some("a+b/c"));
        assert_eq!(r.state.as_deref(), Some("s pace"));
    }

    #[test]
    fn into_code_returns_code() {
        let r = CallbackResult {
            code: Some("abc".into()),
            state: Some("xyz".into()),
            error: None,
            error_description: None,
        };
        assert_eq!(r.into_code().unwrap(), "abc");
    }

    #[test]
    fn into_code_propagates_provider_error() {
        let r = CallbackResult {
            code: None,
            state: None,
            error: Some("invalid_scope".into()),
            error_description: Some("read:admin not allowed".into()),
        };
        let err = r.into_code().unwrap_err().to_string();
        assert!(err.contains("invalid_scope"));
        assert!(err.contains("read:admin not allowed"));
    }

    #[test]
    fn into_code_errors_when_empty() {
        let r = CallbackResult {
            code: None,
            state: None,
            error: None,
            error_description: None,
        };
        assert!(r.into_code().is_err());
    }

    #[test]
    fn redirect_uri_addr_parses_localhost() {
        assert_eq!(
            redirect_uri_addr("http://localhost:12345/callback").unwrap(),
            "localhost:12345"
        );
        assert_eq!(
            redirect_uri_addr("http://127.0.0.1:8080/cb").unwrap(),
            "127.0.0.1:8080"
        );
        assert_eq!(
            redirect_uri_addr("http://localhost:0").unwrap(),
            "localhost:0"
        );
    }

    #[test]
    fn redirect_uri_addr_rejects_https_and_empty() {
        assert!(redirect_uri_addr("https://localhost:1234/cb").is_err());
        assert!(redirect_uri_addr("ftp://localhost:1234/cb").is_err());
        assert!(redirect_uri_addr("http:///cb").is_err());
    }

    #[tokio::test]
    async fn await_callback_captures_success() {
        // Start the listener in a task, grab its port, then send a
        // manual HTTP request simulating the browser redirect.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener); // release so await_callback can re-bind

        let server =
            tokio::spawn(
                async move { await_callback(&addr.to_string(), Duration::from_secs(5)).await },
            );

        // Give the server a moment to rebind
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(
                b"GET /callback?code=the-code&state=the-state HTTP/1.1\r\n\
                  Host: localhost\r\n\r\n",
            )
            .await
            .unwrap();
        // Read response so writer-side doesn't ECONNRESET
        let mut response_buf = [0u8; 1024];
        let _ = stream.read(&mut response_buf).await;

        let result = server.await.unwrap().unwrap();
        assert_eq!(result.code.as_deref(), Some("the-code"));
        assert_eq!(result.state.as_deref(), Some("the-state"));
    }
}
