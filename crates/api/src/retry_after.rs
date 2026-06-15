//! `Retry-After` header parsing and rate-limit error construction.
//!
//! Both providers return 429 (rate limited) and 529 (overloaded) with an
//! optional `Retry-After` header. The header may be either delta-seconds
//! (`Retry-After: 30`) or an HTTP-date (`Retry-After: Wed, 21 Oct 2015 07:28:00 GMT`).

use crate::error::ApiError;

/// HTTP status codes that map to `ApiError::RateLimited`.
pub const RATE_LIMIT_STATUSES: [u16; 2] = [429, 529];

/// Whether a status code should be surfaced as a rate-limit / overload error.
#[must_use]
pub fn is_rate_limit_status(status: u16) -> bool {
    RATE_LIMIT_STATUSES.contains(&status)
}

/// Parse a `Retry-After` header value into milliseconds.
///
/// Accepts delta-seconds (`"30"`) and HTTP-date forms. Returns `None` when the
/// value is absent or unparseable.
#[must_use]
pub fn parse_retry_after_ms(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    let value = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    parse_retry_after_value(value)
}

fn parse_retry_after_value(value: &str) -> Option<u64> {
    let trimmed = value.trim();

    if let Ok(seconds) = trimmed.parse::<u64>() {
        return Some(seconds.saturating_mul(1000));
    }

    let target = httpdate::parse_http_date(trimmed).ok()?;
    let now = std::time::SystemTime::now();
    let remaining = target.duration_since(now).ok()?;
    Some(u64::try_from(remaining.as_millis()).unwrap_or(u64::MAX))
}

/// Build a `RateLimited` error from a 429/529 response's headers, reading the
/// `Retry-After` header when present (defaulting to `0` when absent).
#[must_use]
pub fn rate_limited_from_headers(headers: &reqwest::header::HeaderMap) -> ApiError {
    ApiError::RateLimited {
        retry_after_ms: parse_retry_after_ms(headers).unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};

    #[test]
    fn parses_delta_seconds() {
        assert_eq!(parse_retry_after_value("30"), Some(30_000));
        assert_eq!(parse_retry_after_value("  5 "), Some(5_000));
        assert_eq!(parse_retry_after_value("0"), Some(0));
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_retry_after_value("not-a-number"), None);
        assert_eq!(parse_retry_after_value(""), None);
    }

    #[test]
    fn parses_http_date_in_future() {
        let future = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
        let formatted = httpdate::fmt_http_date(future);
        let ms = parse_retry_after_value(&formatted).expect("should parse http-date");
        // Allow slack for clock drift / test execution time.
        assert!(ms <= 60_000 && ms > 55_000, "got {ms}ms");
    }

    #[test]
    fn past_http_date_yields_none() {
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        let formatted = httpdate::fmt_http_date(past);
        assert_eq!(parse_retry_after_value(&formatted), None);
    }

    #[test]
    fn reads_header_from_map() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("42"));
        assert_eq!(parse_retry_after_ms(&headers), Some(42_000));
    }

    #[test]
    fn missing_header_defaults_to_zero() {
        let headers = HeaderMap::new();
        let err = rate_limited_from_headers(&headers);
        assert!(matches!(err, ApiError::RateLimited { retry_after_ms: 0 }));
    }

    #[test]
    fn present_header_populates_error() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("10"));
        let err = rate_limited_from_headers(&headers);
        assert!(matches!(
            err,
            ApiError::RateLimited {
                retry_after_ms: 10_000
            }
        ));
    }

    #[test]
    fn status_classification() {
        assert!(is_rate_limit_status(429));
        assert!(is_rate_limit_status(529));
        assert!(!is_rate_limit_status(500));
        assert!(!is_rate_limit_status(200));
    }
}
