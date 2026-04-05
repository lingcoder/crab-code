/// API layer error types.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("API error: status={status}, message={message}")]
    Api { status: u16, message: String },

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("SSE stream error: {0}")]
    Sse(String),

    #[error("rate limited, retry after {retry_after_ms}ms")]
    RateLimited { retry_after_ms: u64 },

    #[error("request timed out")]
    Timeout,

    #[error(transparent)]
    Common(#[from] crab_common::Error),
}

/// Convenience result type for the api crate.
pub type Result<T> = std::result::Result<T, ApiError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_error_display_api() {
        let err = ApiError::Api {
            status: 401,
            message: "unauthorized".into(),
        };
        let s = err.to_string();
        assert!(s.contains("401"));
        assert!(s.contains("unauthorized"));
    }

    #[test]
    fn api_error_display_sse() {
        let err = ApiError::Sse("unexpected EOF".into());
        assert!(err.to_string().contains("unexpected EOF"));
    }

    #[test]
    fn api_error_display_rate_limited() {
        let err = ApiError::RateLimited {
            retry_after_ms: 5000,
        };
        assert!(err.to_string().contains("5000"));
    }

    #[test]
    fn api_error_display_timeout() {
        let err = ApiError::Timeout;
        assert!(err.to_string().contains("timed out"));
    }

    #[test]
    fn api_error_from_common() {
        let common_err = crab_common::Error::Other("test".into());
        let api_err: ApiError = common_err.into();
        assert!(matches!(api_err, ApiError::Common(_)));
    }
}
