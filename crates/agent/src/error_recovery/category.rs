//! [`ErrorCategory`] + [`ErrorClassifier`].
//!
//! Turns an error message or HTTP status into a category so recovery
//! strategies (see [`super::strategy`]) can dispatch on it.

/// Classified error category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCategory {
    /// Temporary failure likely to succeed on retry (network hiccup, 503).
    Transient,
    /// Permanent failure — retrying won't help (invalid input, 404).
    Permanent,
    /// Rate limit exceeded — retry after backoff (429).
    RateLimit,
    /// Authentication/authorization failure (401, 403).
    Auth,
    /// Request timed out — may succeed with longer timeout or retry.
    Timeout,
    /// Unknown error category.
    Unknown,
}

impl std::fmt::Display for ErrorCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transient => write!(f, "transient"),
            Self::Permanent => write!(f, "permanent"),
            Self::RateLimit => write!(f, "rate_limit"),
            Self::Auth => write!(f, "auth"),
            Self::Timeout => write!(f, "timeout"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// Classifies errors into categories based on error message patterns and
/// HTTP status codes.
pub struct ErrorClassifier;

impl ErrorClassifier {
    /// Classify an error from its message string.
    #[must_use]
    pub fn classify(error_msg: &str) -> ErrorCategory {
        let lower = error_msg.to_lowercase();

        // Rate limit patterns
        if lower.contains("rate limit")
            || lower.contains("429")
            || lower.contains("too many requests")
            || lower.contains("quota exceeded")
        {
            return ErrorCategory::RateLimit;
        }

        // Auth patterns
        if lower.contains("401")
            || lower.contains("403")
            || lower.contains("unauthorized")
            || lower.contains("forbidden")
            || lower.contains("invalid api key")
            || lower.contains("authentication")
            || lower.contains("permission denied")
        {
            return ErrorCategory::Auth;
        }

        // Timeout patterns
        if lower.contains("timeout")
            || lower.contains("timed out")
            || lower.contains("deadline exceeded")
            || lower.contains("request took too long")
        {
            return ErrorCategory::Timeout;
        }

        // Permanent patterns
        if lower.contains("404")
            || lower.contains("not found")
            || lower.contains("invalid")
            || lower.contains("malformed")
            || lower.contains("bad request")
            || lower.contains("400")
            || lower.contains("unsupported")
            || lower.contains("unprocessable")
            || lower.contains("422")
        {
            return ErrorCategory::Permanent;
        }

        // Transient patterns
        if lower.contains("500")
            || lower.contains("502")
            || lower.contains("503")
            || lower.contains("504")
            || lower.contains("internal server error")
            || lower.contains("service unavailable")
            || lower.contains("bad gateway")
            || lower.contains("connection refused")
            || lower.contains("connection reset")
            || lower.contains("broken pipe")
            || lower.contains("temporarily")
        {
            return ErrorCategory::Transient;
        }

        ErrorCategory::Unknown
    }

    /// Classify from an HTTP status code.
    #[must_use]
    pub fn classify_status(status: u16) -> ErrorCategory {
        match status {
            429 => ErrorCategory::RateLimit,
            401 | 403 => ErrorCategory::Auth,
            408 | 504 => ErrorCategory::Timeout,
            400 | 404 | 405 | 422 | 200..=299 => ErrorCategory::Permanent,
            500 | 502 | 503 => ErrorCategory::Transient,
            _ => ErrorCategory::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_category_display() {
        assert_eq!(ErrorCategory::Transient.to_string(), "transient");
        assert_eq!(ErrorCategory::Permanent.to_string(), "permanent");
        assert_eq!(ErrorCategory::RateLimit.to_string(), "rate_limit");
        assert_eq!(ErrorCategory::Auth.to_string(), "auth");
        assert_eq!(ErrorCategory::Timeout.to_string(), "timeout");
        assert_eq!(ErrorCategory::Unknown.to_string(), "unknown");
    }

    // ── ErrorClassifier ────────────────────────────────────────────

    #[test]
    fn classify_rate_limit() {
        assert_eq!(
            ErrorClassifier::classify("Rate limit exceeded"),
            ErrorCategory::RateLimit
        );
        assert_eq!(
            ErrorClassifier::classify("HTTP 429 Too Many Requests"),
            ErrorCategory::RateLimit
        );
        assert_eq!(
            ErrorClassifier::classify("Quota exceeded for model"),
            ErrorCategory::RateLimit
        );
    }

    #[test]
    fn classify_auth() {
        assert_eq!(
            ErrorClassifier::classify("401 Unauthorized"),
            ErrorCategory::Auth
        );
        assert_eq!(
            ErrorClassifier::classify("403 Forbidden"),
            ErrorCategory::Auth
        );
        assert_eq!(
            ErrorClassifier::classify("Invalid API key provided"),
            ErrorCategory::Auth
        );
        assert_eq!(
            ErrorClassifier::classify("Permission denied"),
            ErrorCategory::Auth
        );
    }

    #[test]
    fn classify_timeout() {
        assert_eq!(
            ErrorClassifier::classify("Request timeout"),
            ErrorCategory::Timeout
        );
        assert_eq!(
            ErrorClassifier::classify("Connection timed out"),
            ErrorCategory::Timeout
        );
        assert_eq!(
            ErrorClassifier::classify("Deadline exceeded"),
            ErrorCategory::Timeout
        );
    }

    #[test]
    fn classify_permanent() {
        assert_eq!(
            ErrorClassifier::classify("404 Not Found"),
            ErrorCategory::Permanent
        );
        assert_eq!(
            ErrorClassifier::classify("Invalid request body"),
            ErrorCategory::Permanent
        );
        assert_eq!(
            ErrorClassifier::classify("400 Bad Request"),
            ErrorCategory::Permanent
        );
        assert_eq!(
            ErrorClassifier::classify("Malformed JSON input"),
            ErrorCategory::Permanent
        );
    }

    #[test]
    fn classify_transient() {
        assert_eq!(
            ErrorClassifier::classify("500 Internal Server Error"),
            ErrorCategory::Transient
        );
        assert_eq!(
            ErrorClassifier::classify("503 Service Unavailable"),
            ErrorCategory::Transient
        );
        assert_eq!(
            ErrorClassifier::classify("Connection refused"),
            ErrorCategory::Transient
        );
        assert_eq!(
            ErrorClassifier::classify("Connection reset by peer"),
            ErrorCategory::Transient
        );
    }

    #[test]
    fn classify_unknown() {
        assert_eq!(
            ErrorClassifier::classify("Something went wrong"),
            ErrorCategory::Unknown
        );
        assert_eq!(ErrorClassifier::classify(""), ErrorCategory::Unknown);
    }

    #[test]
    fn classify_status_codes() {
        assert_eq!(
            ErrorClassifier::classify_status(429),
            ErrorCategory::RateLimit
        );
        assert_eq!(ErrorClassifier::classify_status(401), ErrorCategory::Auth);
        assert_eq!(ErrorClassifier::classify_status(403), ErrorCategory::Auth);
        assert_eq!(
            ErrorClassifier::classify_status(408),
            ErrorCategory::Timeout
        );
        assert_eq!(
            ErrorClassifier::classify_status(504),
            ErrorCategory::Timeout
        );
        assert_eq!(
            ErrorClassifier::classify_status(400),
            ErrorCategory::Permanent
        );
        assert_eq!(
            ErrorClassifier::classify_status(404),
            ErrorCategory::Permanent
        );
        assert_eq!(
            ErrorClassifier::classify_status(500),
            ErrorCategory::Transient
        );
        assert_eq!(
            ErrorClassifier::classify_status(503),
            ErrorCategory::Transient
        );
        assert_eq!(
            ErrorClassifier::classify_status(418),
            ErrorCategory::Unknown
        );
    }

    // ── RecoveryAction ─────────────────────────────────────────────
}
