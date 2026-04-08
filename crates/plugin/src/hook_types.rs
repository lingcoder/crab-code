//! Additional hook execution types beyond shell commands.
//!
//! Extends the hook system with multiple execution strategies:
//! - **Command**: Shell commands (existing pattern from `hook.rs`)
//! - **Agent**: Spawn a sub-agent to process the event
//! - **Http**: POST to an HTTP endpoint
//! - **Prompt**: Pass through the LLM with a prompt template
//!
//! Also includes an SSRF guard for HTTP hooks to prevent requests to
//! private/internal network addresses.
//!
//! Maps to Claude Code's `execAgentHook.ts` + `execHttpHook.ts` + `ssrfGuard.ts`.

use std::collections::HashMap;
use std::fmt;
use std::fmt::Write;
use std::net::IpAddr;

// ─── Hook types ─────────────────────────────────────────────────────────

/// The execution strategy for a registered hook.
#[derive(Debug, Clone)]
pub enum HookType {
    /// Execute a shell command (existing pattern from `hook.rs`).
    Command(CommandHook),
    /// Spawn a sub-agent to process the event.
    Agent(AgentHook),
    /// POST to an HTTP endpoint.
    Http(HttpHook),
    /// Pass through the LLM with a prompt template.
    Prompt(PromptHook),
}

impl fmt::Display for HookType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Command(_) => f.write_str("command"),
            Self::Agent(_) => f.write_str("agent"),
            Self::Http(_) => f.write_str("http"),
            Self::Prompt(_) => f.write_str("prompt"),
        }
    }
}

// ─── Command hook ───────────────────────────────────────────────────────

/// A shell command hook.
///
/// This is the same pattern as `HookDef` in `hook.rs` but extracted into
/// a standalone struct for use with the hook registry.
#[derive(Debug, Clone)]
pub struct CommandHook {
    /// Shell command to execute.
    pub command: String,
    /// Timeout in seconds (0 = no timeout).
    pub timeout_secs: u64,
}

// ─── Agent hook ─────────────────────────────────────────────────────────

/// A hook that spawns a sub-agent to process the event.
///
/// The agent receives the event context and the prompt template
/// (with `{{event}}`, `{{tool_name}}`, etc. placeholders expanded).
#[derive(Debug, Clone)]
pub struct AgentHook {
    /// The type/role of agent to spawn (e.g. "researcher", "reviewer").
    pub agent_type: String,
    /// Prompt template with placeholder variables.
    ///
    /// Supported placeholders:
    /// - `{{event}}` — JSON-serialized event data
    /// - `{{tool_name}}` — name of the tool (for tool events)
    /// - `{{input}}` — tool input JSON (for pre-tool events)
    /// - `{{output}}` — tool output JSON (for post-tool events)
    pub prompt_template: String,
}

// ─── HTTP hook ──────────────────────────────────────────────────────────

/// A hook that sends an HTTP request to an endpoint.
///
/// The event context is serialized as JSON in the request body.
/// The URL is validated against the SSRF guard before sending.
#[derive(Debug, Clone)]
pub struct HttpHook {
    /// Target URL (must pass SSRF validation).
    pub url: String,
    /// HTTP method (default: "POST").
    pub method: String,
    /// Additional headers to include in the request.
    pub headers: HashMap<String, String>,
}

impl HttpHook {
    /// Create a new POST hook with the given URL.
    #[must_use]
    pub fn post(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            method: "POST".to_string(),
            headers: HashMap::new(),
        }
    }

    /// Add a header to the request.
    #[must_use]
    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }
}

// ─── Prompt hook ────────────────────────────────────────────────────────

/// A hook that passes event context through the LLM with a prompt template.
///
/// The prompt template supports the same placeholders as `AgentHook`.
#[derive(Debug, Clone)]
pub struct PromptHook {
    /// Prompt template with placeholder variables.
    pub prompt_template: String,
}

// ─── Hook result ────────────────────────────────────────────────────────

/// Result of executing a hook.
#[derive(Debug, Clone)]
pub struct HookResult {
    /// Exit code (0 = success, non-zero = failure).
    /// For HTTP hooks, this is the HTTP status code.
    pub exit_code: i32,
    /// Standard output or response body.
    pub stdout: String,
    /// Standard error or error message.
    pub stderr: String,
}

impl HookResult {
    /// Whether the hook execution was successful.
    #[must_use]
    pub fn is_success(&self) -> bool {
        self.exit_code == 0
    }

    /// Create a successful result with the given output.
    #[must_use]
    pub fn success(stdout: impl Into<String>) -> Self {
        Self {
            exit_code: 0,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    /// Create a failed result with the given error.
    #[must_use]
    pub fn failure(exit_code: i32, stderr: impl Into<String>) -> Self {
        Self {
            exit_code,
            stdout: String::new(),
            stderr: stderr.into(),
        }
    }
}

// ─── Hook execution ────────────────────────────────────────────────────

/// Execute a command hook.
///
/// Runs the shell command with event context passed via environment variables,
/// using `crab_process::spawn::run`. The event JSON is passed as the
/// `CRAB_HOOK_EVENT` environment variable.
pub async fn execute_command_hook(
    hook: &CommandHook,
    event_json: &serde_json::Value,
) -> crab_common::Result<HookResult> {
    let mut opts = crab_process::spawn::shell_command(&hook.command);

    // Pass event data via environment variable
    opts.env
        .push(("CRAB_HOOK_EVENT".into(), event_json.to_string()));

    // Extract tool_name/input/output from event for convenience env vars
    if let Some(tool_name) = event_json.get("tool_name").and_then(|v| v.as_str()) {
        opts.env
            .push(("CRAB_TOOL_NAME".into(), tool_name.to_string()));
    }
    if let Some(input) = event_json.get("input") {
        opts.env.push(("CRAB_TOOL_INPUT".into(), input.to_string()));
    }

    if hook.timeout_secs > 0 {
        opts.timeout = Some(std::time::Duration::from_secs(hook.timeout_secs));
    }

    let output = crab_process::spawn::run(opts).await?;

    Ok(HookResult {
        exit_code: output.exit_code,
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

/// Execute an agent hook.
///
/// Expands the prompt template with event data and returns the expanded
/// prompt as the hook result. The actual sub-agent spawning is handled
/// by the agent layer (Phase 9); this function prepares the prompt.
pub async fn execute_agent_hook(
    hook: &AgentHook,
    event_json: &serde_json::Value,
) -> crab_common::Result<HookResult> {
    let expanded_prompt = expand_template(&hook.prompt_template, event_json);

    // For now, return the expanded prompt as stdout. The agent orchestrator
    // (Phase 9: Swarm/Team) will use this to spawn a sub-agent.
    Ok(HookResult::success(format!(
        "[agent:{}] {}",
        hook.agent_type, expanded_prompt
    )))
}

/// Execute an HTTP hook.
///
/// Validates the URL against the SSRF guard, then sends the HTTP POST
/// request with the event JSON as the body.
///
/// Uses `crab_process::spawn::run` to delegate to `curl` as a subprocess,
/// avoiding the need for an HTTP client dependency in this crate.
pub async fn execute_http_hook(
    hook: &HttpHook,
    event_json: &serde_json::Value,
) -> crab_common::Result<HookResult> {
    // Validate URL against SSRF guard first.
    validate_http_hook_url(&hook.url)?;

    // Build curl command with headers and JSON body
    let body = event_json.to_string();
    let mut cmd_parts = format!(
        "curl -s -S -X {} -H \"Content-Type: application/json\"",
        hook.method
    );
    for (key, value) in &hook.headers {
        let _ = write!(cmd_parts, " -H \"{key}: {value}\"");
    }
    let escaped_body = body.replace('\'', "'\\''");
    let _ = write!(cmd_parts, " -d '{escaped_body}' \"{}\"", hook.url);

    let mut opts = crab_process::spawn::shell_command(&cmd_parts);
    opts.timeout = Some(std::time::Duration::from_secs(600)); // 10 min default

    let output = crab_process::spawn::run(opts).await?;

    Ok(HookResult {
        exit_code: output.exit_code,
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

/// Execute a prompt hook.
///
/// Expands the prompt template with event data and returns the expanded
/// prompt. The actual LLM call is handled by the agent layer; this function
/// prepares the prompt for the caller to send to the LLM.
pub async fn execute_prompt_hook(
    hook: &PromptHook,
    event_json: &serde_json::Value,
) -> crab_common::Result<HookResult> {
    let expanded = expand_template(&hook.prompt_template, event_json);

    // Return the expanded prompt. The caller (hook executor in hook.rs or
    // the agent loop) is responsible for sending this to the LLM and
    // interpreting the response.
    Ok(HookResult::success(expanded))
}

/// Expand placeholder variables in a prompt template.
///
/// Supported placeholders:
/// - `{{event}}` — full JSON event
/// - `{{tool_name}}` — tool name from event
/// - `{{input}}` — tool input from event
/// - `{{output}}` — tool output from event
pub fn expand_template(template: &str, event_json: &serde_json::Value) -> String {
    let mut result = template.to_string();

    result = result.replace("{{event}}", &event_json.to_string());

    if let Some(tool_name) = event_json.get("tool_name").and_then(|v| v.as_str()) {
        result = result.replace("{{tool_name}}", tool_name);
    }

    if let Some(input) = event_json.get("input") {
        result = result.replace("{{input}}", &input.to_string());
    }

    if let Some(output) = event_json.get("output") {
        result = result.replace("{{output}}", &output.to_string());
    }

    result
}

// ─── SSRF Guard ─────────────────────────────────────────────────────────

/// Errors from SSRF URL validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SsrfError {
    /// The URL resolves to a private/internal IP address.
    PrivateIp,
    /// The URL points to localhost.
    Localhost,
    /// The URL uses a disallowed scheme (only HTTPS is permitted).
    InvalidScheme(String),
    /// The URL could not be parsed.
    InvalidUrl(String),
}

impl fmt::Display for SsrfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PrivateIp => f.write_str("URL resolves to a private IP address"),
            Self::Localhost => f.write_str("URL points to localhost"),
            Self::InvalidScheme(scheme) => write!(f, "disallowed URL scheme: {scheme}"),
            Self::InvalidUrl(reason) => write!(f, "invalid URL: {reason}"),
        }
    }
}

impl std::error::Error for SsrfError {}

impl From<SsrfError> for crab_common::Error {
    fn from(e: SsrfError) -> Self {
        Self::Other(format!("SSRF guard: {e}"))
    }
}

/// Validate that an HTTP hook URL is safe to request.
///
/// Checks:
/// 1. URL uses HTTPS scheme (HTTP is rejected)
/// 2. Host is not localhost (`127.0.0.0/8`, `::1`)
/// 3. Host is not a private IP (10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16)
///
/// # Errors
///
/// Returns `SsrfError` if the URL fails validation.
pub fn validate_http_hook_url(url: &str) -> Result<(), SsrfError> {
    // Parse the URL minimally.
    let (scheme, host) = parse_url_parts(url)?;

    // Only HTTPS is allowed.
    if scheme != "https" {
        return Err(SsrfError::InvalidScheme(scheme.to_string()));
    }

    // Check for localhost hostnames.
    let host_lower = host.to_lowercase();
    if host_lower == "localhost" || host_lower == "[::1]" {
        return Err(SsrfError::Localhost);
    }

    // Check if the host is an IP address.
    let cleaned = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = cleaned.parse::<IpAddr>() {
        if is_loopback(&ip) {
            return Err(SsrfError::Localhost);
        }
        if is_private(&ip) {
            return Err(SsrfError::PrivateIp);
        }
    }

    Ok(())
}

/// Parse scheme and host from a URL string (minimal parser, no dependencies).
fn parse_url_parts(url: &str) -> Result<(&str, &str), SsrfError> {
    let scheme_end = url
        .find("://")
        .ok_or_else(|| SsrfError::InvalidUrl("missing scheme".into()))?;
    let scheme = &url[..scheme_end];
    let after_scheme = &url[scheme_end + 3..];

    // Host is everything up to the first `/`, `?`, or `#`, or end of string.
    let host_end = after_scheme
        .find(['/', '?', '#'])
        .unwrap_or(after_scheme.len());
    let host_port = &after_scheme[..host_end];

    // Strip port if present.
    let host = if host_port.starts_with('[') {
        // IPv6: [::1]:8080
        host_port.find(']').map_or(host_port, |i| &host_port[..=i])
    } else if let Some(colon) = host_port.rfind(':') {
        // Check if after colon is all digits (port number).
        if host_port[colon + 1..].chars().all(|c| c.is_ascii_digit()) {
            &host_port[..colon]
        } else {
            host_port
        }
    } else {
        host_port
    };

    if host.is_empty() {
        return Err(SsrfError::InvalidUrl("empty host".into()));
    }

    Ok((scheme, host))
}

/// Check if an IP address is a loopback address.
fn is_loopback(ip: &IpAddr) -> bool {
    ip.is_loopback()
}

/// Check if an IP address is in a private range.
fn is_private(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            // 10.0.0.0/8
            if octets[0] == 10 {
                return true;
            }
            // 172.16.0.0/12
            if octets[0] == 172 && (16..=31).contains(&octets[1]) {
                return true;
            }
            // 192.168.0.0/16
            if octets[0] == 192 && octets[1] == 168 {
                return true;
            }
            // 169.254.0.0/16 (link-local)
            if octets[0] == 169 && octets[1] == 254 {
                return true;
            }
            false
        }
        IpAddr::V6(v6) => {
            // fe80::/10 (link-local)
            let segments = v6.segments();
            if segments[0] & 0xffc0 == 0xfe80 {
                return true;
            }
            // fc00::/7 (unique local)
            if segments[0] & 0xfe00 == 0xfc00 {
                return true;
            }
            false
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── HookType ───────────────────────────────────────────────────

    #[test]
    fn hook_type_display() {
        assert_eq!(
            HookType::Command(CommandHook {
                command: "echo".into(),
                timeout_secs: 10,
            })
            .to_string(),
            "command"
        );
        assert_eq!(
            HookType::Agent(AgentHook {
                agent_type: "test".into(),
                prompt_template: String::new(),
            })
            .to_string(),
            "agent"
        );
        assert_eq!(
            HookType::Http(HttpHook::post("https://example.com")).to_string(),
            "http"
        );
        assert_eq!(
            HookType::Prompt(PromptHook {
                prompt_template: String::new(),
            })
            .to_string(),
            "prompt"
        );
    }

    // ── HookResult ─────────────────────────────────────────────────

    #[test]
    fn hook_result_success() {
        let r = HookResult::success("output");
        assert!(r.is_success());
        assert_eq!(r.stdout, "output");
        assert!(r.stderr.is_empty());
    }

    #[test]
    fn hook_result_failure() {
        let r = HookResult::failure(1, "error msg");
        assert!(!r.is_success());
        assert_eq!(r.exit_code, 1);
        assert_eq!(r.stderr, "error msg");
    }

    // ── HttpHook builder ───────────────────────────────────────────

    #[test]
    fn http_hook_builder() {
        let hook = HttpHook::post("https://example.com/hook")
            .with_header("Authorization", "Bearer token123");
        assert_eq!(hook.url, "https://example.com/hook");
        assert_eq!(hook.method, "POST");
        assert_eq!(
            hook.headers.get("Authorization").unwrap(),
            "Bearer token123"
        );
    }

    // ── Template expansion ─────────────────────────────────────────

    #[test]
    fn expand_template_basic() {
        let tmpl = "Tool {{tool_name}} received {{input}}";
        let event = serde_json::json!({
            "tool_name": "bash",
            "input": {"command": "ls"},
        });
        let result = expand_template(tmpl, &event);
        assert!(result.contains("bash"));
        assert!(result.contains("ls"));
    }

    #[test]
    fn expand_template_no_placeholders() {
        let tmpl = "static text";
        let event = serde_json::json!({});
        assert_eq!(expand_template(tmpl, &event), "static text");
    }

    #[test]
    fn expand_template_missing_keys() {
        let tmpl = "{{tool_name}} {{output}}";
        let event = serde_json::json!({});
        let result = expand_template(tmpl, &event);
        // Unresolved placeholders remain as-is.
        assert!(result.contains("{{tool_name}}"));
        assert!(result.contains("{{output}}"));
    }

    // ── SSRF guard ─────────────────────────────────────────────────

    #[test]
    fn ssrf_allows_https() {
        assert!(validate_http_hook_url("https://example.com/hook").is_ok());
    }

    #[test]
    fn ssrf_rejects_http() {
        let err = validate_http_hook_url("http://example.com/hook").unwrap_err();
        assert_eq!(err, SsrfError::InvalidScheme("http".into()));
    }

    #[test]
    fn ssrf_rejects_localhost() {
        let err = validate_http_hook_url("https://localhost/hook").unwrap_err();
        assert_eq!(err, SsrfError::Localhost);

        let err = validate_http_hook_url("https://127.0.0.1/hook").unwrap_err();
        assert_eq!(err, SsrfError::Localhost);
    }

    #[test]
    fn ssrf_rejects_private_ip() {
        let err = validate_http_hook_url("https://10.0.0.1/hook").unwrap_err();
        assert_eq!(err, SsrfError::PrivateIp);

        let err = validate_http_hook_url("https://192.168.1.1/hook").unwrap_err();
        assert_eq!(err, SsrfError::PrivateIp);

        let err = validate_http_hook_url("https://172.16.0.1/hook").unwrap_err();
        assert_eq!(err, SsrfError::PrivateIp);
    }

    #[test]
    fn ssrf_allows_public_ip() {
        assert!(validate_http_hook_url("https://8.8.8.8/hook").is_ok());
    }

    #[test]
    fn ssrf_rejects_missing_scheme() {
        let err = validate_http_hook_url("example.com/hook").unwrap_err();
        assert!(matches!(err, SsrfError::InvalidUrl(_)));
    }

    #[test]
    fn ssrf_rejects_empty_host() {
        let err = validate_http_hook_url("https:///hook").unwrap_err();
        assert!(matches!(err, SsrfError::InvalidUrl(_)));
    }

    #[test]
    fn ssrf_strips_port() {
        assert!(validate_http_hook_url("https://example.com:8443/hook").is_ok());
    }

    #[test]
    fn ssrf_error_display() {
        assert_eq!(
            SsrfError::PrivateIp.to_string(),
            "URL resolves to a private IP address"
        );
        assert_eq!(SsrfError::Localhost.to_string(), "URL points to localhost");
        assert_eq!(
            SsrfError::InvalidScheme("ftp".into()).to_string(),
            "disallowed URL scheme: ftp"
        );
    }

    // ── URL parsing ────────────────────────────────────────────────

    #[test]
    fn parse_url_parts_basic() {
        let (scheme, host) = parse_url_parts("https://example.com/path").unwrap();
        assert_eq!(scheme, "https");
        assert_eq!(host, "example.com");
    }

    #[test]
    fn parse_url_parts_with_port() {
        let (scheme, host) = parse_url_parts("https://example.com:8443/path").unwrap();
        assert_eq!(scheme, "https");
        assert_eq!(host, "example.com");
    }

    #[test]
    fn parse_url_parts_no_path() {
        let (scheme, host) = parse_url_parts("https://example.com").unwrap();
        assert_eq!(scheme, "https");
        assert_eq!(host, "example.com");
    }

    // ── Private IP checks ──────────────────────────────────────────

    #[test]
    fn is_private_10_range() {
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        assert!(is_private(&ip));
    }

    #[test]
    fn is_private_172_range() {
        let ip: IpAddr = "172.16.0.1".parse().unwrap();
        assert!(is_private(&ip));
        let ip: IpAddr = "172.31.255.255".parse().unwrap();
        assert!(is_private(&ip));
        let ip: IpAddr = "172.32.0.1".parse().unwrap();
        assert!(!is_private(&ip));
    }

    #[test]
    fn is_private_192_168_range() {
        let ip: IpAddr = "192.168.0.1".parse().unwrap();
        assert!(is_private(&ip));
    }

    #[test]
    fn is_private_public_ip() {
        let ip: IpAddr = "8.8.8.8".parse().unwrap();
        assert!(!is_private(&ip));
    }
}
