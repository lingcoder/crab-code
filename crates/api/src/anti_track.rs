//! Anti-Tracking and Anti-Upgrade Module
//!
//! Provides mechanisms to:
//! - Disable automatic version checks and updates
//! - Remove/block telemetry and analytics requests
//! - Sanitize network requests to avoid detection
//!
//! This module is inspired by the anti-track implementation in
//! [rusty-ai-cli](https://github.com/lorryjovens-hub/claude-code-rust),
//! contributed with assistance from the Hermes Agent.

use std::sync::LazyLock;

/// Anti-tracking configuration.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AntiTrackConfig {
    /// Disable version check requests.
    pub disable_version_check: bool,
    /// Disable telemetry/analytics.
    pub disable_telemetry: bool,
    /// Disable update checks.
    pub disable_update_check: bool,
    /// Block known tracking domains.
    pub block_tracking_domains: bool,
    /// Use fake version identifiers.
    pub spoof_version: bool,
    /// Spoofed version string.
    pub spoofed_version: String,
    /// Disable crash reporting.
    pub disable_crash_reports: bool,
    /// Disable feedback submission.
    pub disable_feedback: bool,
}

impl AntiTrackConfig {
    /// Create a new config with all protections enabled.
    #[must_use]
    pub fn enabled() -> Self {
        Self {
            disable_version_check: true,
            disable_telemetry: true,
            disable_update_check: true,
            block_tracking_domains: true,
            spoof_version: true,
            spoofed_version: "1.0.0".to_string(),
            disable_crash_reports: true,
            disable_feedback: true,
        }
    }
}

/// Domains known to be used for tracking/telemetry.
pub static TRACKING_DOMAINS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    vec![
        "telemetry.anthropic.com",
        "analytics.anthropic.com",
        "client.telemetry.github.com",
        "api.segment.io",
        "events.devoptix.com",
        "litestream.io",
        "ipfs.io",
        "cloudflare-ipfs.com",
    ]
});

/// Sensitive header names that reveal identity or enable tracking.
pub static SENSITIVE_HEADERS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    vec![
        "X-Claude-Client-Name",
        "X-Claude-Client-Version",
        "X-Anthropic-Telemetry",
        "X-Analytics",
        "X-Tracking-ID",
        "X-Session-ID",
        "X-User-ID",
    ]
});

/// Check if a URL should be blocked based on anti-track rules.
#[must_use]
pub fn should_block_url(url: &str, config: &AntiTrackConfig) -> bool {
    if config.block_tracking_domains && TRACKING_DOMAINS.iter().any(|d| url.contains(d)) {
        return true;
    }

    if config.disable_version_check && (url.contains("/version") || url.contains("/update") || url.contains("/check")) {
        return true;
    }

    false
}

/// Sanitize a `reqwest::Request` by removing tracking headers.
pub fn sanitize_request(request: &mut reqwest::Request, config: &AntiTrackConfig) {
    let headers = request.headers_mut();

    // Remove sensitive headers.
    for name in SENSITIVE_HEADERS.iter() {
        if let Ok(h) = name.parse::<reqwest::header::HeaderName>() {
            headers.remove(h);
        }
    }

    // Remove any header containing telemetry/analytics/tracking keywords.
    let to_remove: Vec<reqwest::header::HeaderName> = headers
        .keys()
        .filter(|k| {
            let lower = k.as_str().to_lowercase();
            lower.contains("telemetry")
                || lower.contains("analytics")
                || lower.contains("tracking")
                || lower.contains("crash")
                || lower.contains("feedback")
        })
        .cloned()
        .collect();

    for key in to_remove {
        headers.remove(key);
    }

    // Spoof User-Agent if enabled.
    if config.spoof_version {
        if let Ok(ua) = reqwest::header::HeaderValue::from_str(&format!(
            "CrabCode/{} (Rust CLI)",
            config.spoofed_version
        )) {
            headers.insert(reqwest::header::USER_AGENT, ua);
        }
    }
}

/// Convenience wrapper that returns `None` when the request should be blocked.
pub fn apply(request: reqwest::Request, config: &AntiTrackConfig) -> Option<reqwest::Request> {
    if should_block_url(request.url().as_str(), config) {
        return None;
    }
    let mut request = request;
    sanitize_request(&mut request, config);
    Some(request)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_block_tracking_domain() {
        let config = AntiTrackConfig::enabled();
        assert!(should_block_url("https://telemetry.anthropic.com/v1/track", &config));
        assert!(!should_block_url("https://api.anthropic.com/v1/messages", &config));
    }

    #[test]
    fn test_sanitize_request() {
        let config = AntiTrackConfig::enabled();
        let mut request = reqwest::Request::new(
            reqwest::Method::GET,
            "https://api.anthropic.com/v1/models".parse().unwrap(),
        );
        request.headers_mut().insert("X-Claude-Client-Version", "1.0.0".parse().unwrap());
        request.headers_mut().insert("Authorization", "Bearer token".parse().unwrap());

        sanitize_request(&mut request, &config);

        assert!(!request.headers().contains_key("X-Claude-Client-Version"));
        assert!(request.headers().contains_key("Authorization"));
    }
}
