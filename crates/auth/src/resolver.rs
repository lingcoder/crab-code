//! Auth key resolution chain — independent of `Config` business fields.
//!
//! Implements the out-of-chain auth resolution order from `docs/config.md` §3:
//!   `ANTHROPIC_AUTH_TOKEN` env
//!     → provider-specific env (`ANTHROPIC_API_KEY` / `OPENAI_API_KEY` / …)
//!     → `apiKeyHelper` script execution
//!     → system keychain
//!     → `~/.crab/auth/tokens.json` (`OAuth` access token)
//!
//! Secret values resolved here never round-trip through `Config`.

use std::process::Command;

use crab_config::Config;

/// Resolve an authentication credential for the active provider.
///
/// Returns the first non-empty value found in the chain, or `None` if no
/// credential is available. Callers should treat `None` as a fatal config
/// error at request time (the network call will produce a 401 otherwise).
#[must_use]
pub fn resolve_auth_key(cfg: &Config) -> Option<String> {
    let provider = cfg.api_provider.as_deref();

    // 1. ANTHROPIC_AUTH_TOKEN: only consulted for anthropic provider (or unset, which
    //    defaults to anthropic). This token is anthropic-specific (CCB-compat OAuth-equivalent);
    //    leaking it to deepseek/openai would send the wrong credential and trigger 401s for
    //    users who have it set in their shell from a prior CCB session.
    if matches!(provider, None | Some("anthropic"))
        && let Some(v) = read_env("ANTHROPIC_AUTH_TOKEN")
    {
        return Some(v);
    }

    // 2. Provider-specific API key env vars.
    for var in provider_env_vars(provider) {
        if let Some(v) = read_env(var) {
            return Some(v);
        }
    }

    // 3. apiKeyHelper script (path is config; the secret it prints never enters Config).
    if let Some(v) = run_api_key_helper(cfg.api_key_helper.as_deref()) {
        return Some(v);
    }

    // 4. System keychain.
    if let Ok(v) = crate::keychain::get_api_key()
        && !v.is_empty()
    {
        return Some(v);
    }

    // 5. OAuth tokens.json — read the access token for the configured provider.
    if let Some(v) = read_oauth_token_file(cfg.api_provider.as_deref()) {
        return Some(v);
    }

    None
}

fn read_env(var: &str) -> Option<String> {
    std::env::var(var).ok().filter(|v| !v.is_empty())
}

/// Map `api_provider` to its API-key env var list (priority order).
fn provider_env_vars(provider: Option<&str>) -> &'static [&'static str] {
    match provider {
        Some("openai" | "ollama" | "vllm") => &["OPENAI_API_KEY"],
        Some("deepseek") => &["DEEPSEEK_API_KEY", "OPENAI_API_KEY"],
        _ => &["ANTHROPIC_API_KEY"],
    }
}

/// Execute the `apiKeyHelper` script and return its trimmed stdout.
///
/// Returns `None` if the path is empty, the script fails to launch, exits
/// non-zero, or produces no output. Failures are silent — the caller falls
/// through to the next link in the chain.
fn run_api_key_helper(path: Option<&str>) -> Option<String> {
    let path = path?.trim();
    if path.is_empty() {
        return None;
    }

    let output = if cfg!(windows) {
        Command::new("cmd").args(["/C", path]).output().ok()?
    } else {
        Command::new("sh").args(["-c", path]).output().ok()?
    };

    if !output.status.success() {
        return None;
    }

    let key = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!key.is_empty()).then_some(key)
}

/// Read the OAuth access token for `provider` from `~/.crab/auth/tokens.json`.
fn read_oauth_token_file(provider: Option<&str>) -> Option<String> {
    let path = crate::oauth::default_token_path();
    let store = crate::oauth::load_token_store(&path).ok()?;
    let provider = provider.unwrap_or("anthropic");
    store
        .get(provider)
        .map(|t| t.access_token.clone())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_env_var_routing() {
        assert_eq!(provider_env_vars(None), &["ANTHROPIC_API_KEY"]);
        assert_eq!(provider_env_vars(Some("anthropic")), &["ANTHROPIC_API_KEY"]);
        assert_eq!(provider_env_vars(Some("openai")), &["OPENAI_API_KEY"]);
        assert_eq!(provider_env_vars(Some("ollama")), &["OPENAI_API_KEY"]);
        assert_eq!(provider_env_vars(Some("vllm")), &["OPENAI_API_KEY"]);
        assert_eq!(
            provider_env_vars(Some("deepseek")),
            &["DEEPSEEK_API_KEY", "OPENAI_API_KEY"]
        );
    }

    #[test]
    fn api_key_helper_empty_path_returns_none() {
        assert!(run_api_key_helper(None).is_none());
        assert!(run_api_key_helper(Some("")).is_none());
        assert!(run_api_key_helper(Some("   ")).is_none());
    }

    #[test]
    fn api_key_helper_executes_simple_script() {
        // `echo` is portable across cmd.exe and /bin/sh.
        let key = run_api_key_helper(Some("echo helper-secret"));
        assert_eq!(key.as_deref(), Some("helper-secret"));
    }

    #[test]
    fn api_key_helper_failing_command_returns_none() {
        // `false` exits 1 on Unix; on Windows `cmd /C exit 1` does the same.
        let cmd = if cfg!(windows) { "exit 1" } else { "false" };
        assert!(run_api_key_helper(Some(cmd)).is_none());
    }

    #[test]
    fn read_env_returns_none_for_unset_var() {
        // Use a unique name unlikely to be set anywhere in the test env.
        assert!(read_env("CRAB_TEST_RESOLVER_DEFINITELY_UNSET_8821_XYZ").is_none());
    }
}
