/// Environment variable names checked for API keys, in priority order.
const ANTHROPIC_ENV_VARS: &[&str] = &["ANTHROPIC_API_KEY"];
const OPENAI_ENV_VARS: &[&str] = &["OPENAI_API_KEY"];

/// Return the env var names for a given provider.
fn env_vars_for_provider(provider: Option<&str>) -> &'static [&'static str] {
    match provider {
        Some("openai" | "ollama" | "deepseek" | "vllm") => OPENAI_ENV_VARS,
        _ => ANTHROPIC_ENV_VARS,
    }
}

/// Resolve an API key using the priority chain:
/// 1. Explicit key passed in (from settings)
/// 2. Environment variable(s) for the given provider
/// 3. System keychain
///
/// # Errors
///
/// Returns `crab_common::Error::Auth` if no API key can be found anywhere.
pub fn resolve_api_key(
    explicit_key: Option<&str>,
    provider: Option<&str>,
) -> crab_common::Result<String> {
    resolve_with_env(explicit_key, provider, |k| std::env::var(k))
}

/// Inner resolution logic, parameterized over env var lookup for testability.
fn resolve_with_env<F>(
    explicit_key: Option<&str>,
    provider: Option<&str>,
    env_lookup: F,
) -> crab_common::Result<String>
where
    F: Fn(&str) -> std::result::Result<String, std::env::VarError>,
{
    // 1. Explicit key (from settings.api_key)
    if let Some(key) = explicit_key
        && !key.is_empty()
    {
        return Ok(key.to_string());
    }

    // 2. Environment variables — provider-specific
    let env_vars = env_vars_for_provider(provider);

    for var in env_vars {
        if let Ok(key) = env_lookup(var)
            && !key.is_empty()
        {
            return Ok(key);
        }
    }

    // 3. System keychain
    if let Ok(key) = crate::keychain::get_api_key()
        && !key.is_empty()
    {
        return Ok(key);
    }

    Err(crab_common::Error::Auth(format!(
        "no API key found: set {} or store in system keychain",
        env_vars.join(" / ")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Build a fake env lookup from a map.
    fn fake_env(
        map: HashMap<&'static str, &'static str>,
    ) -> impl Fn(&str) -> std::result::Result<String, std::env::VarError> {
        move |key: &str| {
            map.get(key)
                .map(|v| (*v).to_string())
                .ok_or(std::env::VarError::NotPresent)
        }
    }

    fn no_env(_key: &str) -> std::result::Result<String, std::env::VarError> {
        Err(std::env::VarError::NotPresent)
    }

    #[test]
    fn explicit_key_takes_priority() {
        let result = resolve_with_env(Some("sk-test-123"), None, no_env);
        assert_eq!(result.unwrap(), "sk-test-123");
    }

    #[test]
    fn empty_explicit_key_is_skipped() {
        let result = resolve_with_env(Some(""), None, no_env);
        assert!(result.is_err());
    }

    #[test]
    fn env_var_for_anthropic_provider() {
        let env = fake_env(HashMap::from([("ANTHROPIC_API_KEY", "ant-key")]));
        let result = resolve_with_env(None, Some("anthropic"), env);
        assert_eq!(result.unwrap(), "ant-key");
    }

    #[test]
    fn env_var_for_openai_provider() {
        let env = fake_env(HashMap::from([("OPENAI_API_KEY", "oai-key")]));
        let result = resolve_with_env(None, Some("openai"), env);
        assert_eq!(result.unwrap(), "oai-key");
    }

    #[test]
    fn no_key_returns_error() {
        let result = resolve_with_env(None, None, no_env);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("no API key found"));
    }

    #[test]
    fn explicit_key_overrides_env_var() {
        let env = fake_env(HashMap::from([("ANTHROPIC_API_KEY", "env-key")]));
        let result = resolve_with_env(Some("explicit-key"), None, env);
        assert_eq!(result.unwrap(), "explicit-key");
    }

    #[test]
    fn default_provider_uses_anthropic_env() {
        let env = fake_env(HashMap::from([("ANTHROPIC_API_KEY", "default-key")]));
        let result = resolve_with_env(None, None, env);
        assert_eq!(result.unwrap(), "default-key");
    }

    #[test]
    fn empty_env_var_is_skipped() {
        let env = fake_env(HashMap::from([("ANTHROPIC_API_KEY", "")]));
        let result = resolve_with_env(None, None, env);
        assert!(result.is_err());
    }

    #[test]
    fn env_vars_for_provider_routing() {
        assert_eq!(env_vars_for_provider(None), &["ANTHROPIC_API_KEY"]);
        assert_eq!(
            env_vars_for_provider(Some("anthropic")),
            &["ANTHROPIC_API_KEY"]
        );
        assert_eq!(env_vars_for_provider(Some("openai")), &["OPENAI_API_KEY"]);
        assert_eq!(env_vars_for_provider(Some("ollama")), &["OPENAI_API_KEY"]);
        assert_eq!(env_vars_for_provider(Some("deepseek")), &["OPENAI_API_KEY"]);
        assert_eq!(env_vars_for_provider(Some("vllm")), &["OPENAI_API_KEY"]);
    }
}
