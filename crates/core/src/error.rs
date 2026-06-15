use std::time::Duration;

use thiserror::Error;

/// Coarse classification of an API-layer failure.
///
/// Derived from HTTP status codes and transport conditions, this lets the agent
/// loop decide retry/backoff behaviour by matching on structured data instead
/// of substring-sniffing error strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiErrorKind {
    /// Temporary server-side failure (5xx other than 501).
    Transient,
    /// Rate limited or overloaded (429, 529).
    RateLimited,
    /// The prompt exceeded the model's context window.
    PromptTooLong,
    /// Authentication or authorization failure (401, 403).
    Auth,
    /// Client-side invalid request that retrying will not fix (400, 422, 501).
    InvalidRequest,
    /// Network-level failure (DNS, connection refused, reset).
    Network,
    /// Request timed out.
    Timeout,
    /// Unclassified.
    Unknown,
}

impl ApiErrorKind {
    /// Whether an error of this kind is worth retrying on the same model.
    #[must_use]
    pub fn is_retryable(self) -> bool {
        matches!(
            self,
            Self::Transient | Self::RateLimited | Self::Network | Self::Timeout
        )
    }

    /// Whether this kind indicates an overloaded/rate-limited upstream, which
    /// is the trigger for falling back to a secondary model.
    #[must_use]
    pub fn is_overloaded(self) -> bool {
        matches!(self, Self::RateLimited)
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("config error: {0}")]
    Config(String),

    /// Structured API failure carrying the originating HTTP status, a coarse
    /// `kind` for retry decisions, and an optional server-provided retry delay.
    #[error("API error{}: {message}", .status.map(|s| format!(" (status {s})")).unwrap_or_default())]
    Api {
        status: Option<u16>,
        kind: ApiErrorKind,
        retry_after: Option<Duration>,
        message: String,
    },

    #[error("auth error: {0}")]
    Auth(String),

    #[error("tool error: {0}")]
    Tool(String),

    #[error("permission error: {0}")]
    Permission(String),

    #[error("{0}")]
    Other(String),
}

impl Error {
    /// Construct a structured API error with no status or retry hint.
    #[must_use]
    pub fn api(kind: ApiErrorKind, message: impl Into<String>) -> Self {
        Self::Api {
            status: None,
            kind,
            retry_after: None,
            message: message.into(),
        }
    }

    /// The structured API `kind`, if this is an `Api` error.
    #[must_use]
    pub fn api_kind(&self) -> Option<ApiErrorKind> {
        match self {
            Self::Api { kind, .. } => Some(*kind),
            _ => None,
        }
    }

    /// Server-provided retry delay, if this is an `Api` error carrying one.
    #[must_use]
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::Api { retry_after, .. } => *retry_after,
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_error_from_conversion() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err: Error = io_err.into();
        assert!(matches!(err, Error::Io(_)));
        assert!(err.to_string().contains("file not found"));
    }

    #[test]
    fn config_error_display() {
        let err = Error::Config("bad value".into());
        assert_eq!(err.to_string(), "config error: bad value");
    }

    #[test]
    fn api_error_display_with_status() {
        let err = Error::Api {
            status: Some(429),
            kind: ApiErrorKind::RateLimited,
            retry_after: Some(Duration::from_secs(2)),
            message: "rate limited".into(),
        };
        assert_eq!(err.to_string(), "API error (status 429): rate limited");
        assert_eq!(err.api_kind(), Some(ApiErrorKind::RateLimited));
        assert_eq!(err.retry_after(), Some(Duration::from_secs(2)));
    }

    #[test]
    fn api_error_display_without_status() {
        let err = Error::api(ApiErrorKind::Network, "connection reset");
        assert_eq!(err.to_string(), "API error: connection reset");
        assert_eq!(err.api_kind(), Some(ApiErrorKind::Network));
        assert_eq!(err.retry_after(), None);
    }

    #[test]
    fn api_error_kind_retryable() {
        assert!(ApiErrorKind::Transient.is_retryable());
        assert!(ApiErrorKind::RateLimited.is_retryable());
        assert!(ApiErrorKind::Network.is_retryable());
        assert!(ApiErrorKind::Timeout.is_retryable());
        assert!(!ApiErrorKind::Auth.is_retryable());
        assert!(!ApiErrorKind::InvalidRequest.is_retryable());
        assert!(!ApiErrorKind::PromptTooLong.is_retryable());
        assert!(!ApiErrorKind::Unknown.is_retryable());
    }

    #[test]
    fn api_error_kind_overloaded() {
        assert!(ApiErrorKind::RateLimited.is_overloaded());
        assert!(!ApiErrorKind::Transient.is_overloaded());
        assert!(!ApiErrorKind::Timeout.is_overloaded());
    }

    #[test]
    fn non_api_error_has_no_kind() {
        let err = Error::Other("boom".into());
        assert_eq!(err.api_kind(), None);
        assert_eq!(err.retry_after(), None);
    }

    #[test]
    fn auth_error_display() {
        let err = Error::Auth("token expired".into());
        assert_eq!(err.to_string(), "auth error: token expired");
    }

    #[test]
    fn tool_error_display() {
        let err = Error::Tool("bash failed".into());
        assert_eq!(err.to_string(), "tool error: bash failed");
    }

    #[test]
    fn permission_error_display() {
        let err = Error::Permission("denied".into());
        assert_eq!(err.to_string(), "permission error: denied");
    }

    #[test]
    fn other_error_display() {
        let err = Error::Other("something went wrong".into());
        assert_eq!(err.to_string(), "something went wrong");
    }

    #[test]
    fn error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Error>();
    }
}
