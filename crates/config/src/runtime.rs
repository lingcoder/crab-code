//! Runtime-layer projection: env vars and CLI flags → `toml::Value`.
//!
//! Both kinds of input live above the file layer in the merge chain
//! (`docs/config.md` §9). They are projected into partial `toml::Value`
//! tables here so the loader can deep-merge them with everything else.
//!
//! Two functions live in this module:
//!
//! - [`env_to_value`] maps the *non-secret* `CRAB_*` environment variables
//!   onto their corresponding `Config` fields. Secret env vars (API keys,
//!   OAuth tokens) are intentionally **not** mapped here — they flow
//!   through the `auth` module so they never round-trip through `Config`.
//! - [`cli_overrides_to_value`] parses `-c key.path=value` strings into a
//!   nested table (TOML grammar first, falling back to a string).

use std::collections::HashMap;

use toml::Value;
use toml::value::Table;

/// Project the captured process environment into a partial `toml::Value`.
///
/// Public, non-secret env knobs:
///
/// | env var              | config field   | notes                                 |
/// |----------------------|----------------|----------------------------------------|
/// | `CRAB_MODEL`         | `model`        |                                        |
/// | `CRAB_API_PROVIDER`  | `apiProvider`  |                                        |
/// | `CRAB_DEFAULT_SHELL` | `default_shell`| `"bash"` or `"powershell"` for `!` routing |
/// | `CRAB_BASE_URL`       | `base_url`   | universal override (highest priority)  |
/// | `ANTHROPIC_BASE_URL` | `base_url`   | only when provider is anthropic/unset (CCB-compat name) |
/// | `OPENAI_BASE_URL`     | `base_url`   | only when provider is openai          |
/// | `DEEPSEEK_BASE_URL`   | `base_url`   | only when provider is deepseek         |
///
/// URL vars are **mutually exclusive** — `CRAB_BASE_URL` wins outright; otherwise the
/// provider-specific URL matching `CRAB_API_PROVIDER` (defaulting to anthropic if unset)
/// is used. Empty values behave the same as unset.
///
/// Secret env vars (`CRAB_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `<PROVIDER>_API_KEY`) are
/// deliberately **not** mapped here — they flow through the `auth` module so they never
/// round-trip through `Config`.
#[must_use]
#[allow(clippy::implicit_hasher)]
pub fn env_to_value(env: &HashMap<String, String>) -> Value {
    let mut root = Table::new();

    let mut put = |env_key: &str, config_key: &str| {
        if let Some(value) = env.get(env_key)
            && !value.is_empty()
        {
            root.insert(config_key.into(), Value::String(value.clone()));
        }
    };

    put("CRAB_MODEL", "model");
    put("CRAB_API_PROVIDER", "api_provider");
    put("CRAB_DEFAULT_SHELL", "default_shell");

    // API base URL: mutually exclusive, applied conservatively.
    //   1. CRAB_BASE_URL — universal override (highest priority).
    //   2. <PROVIDER>_BASE_URL — only when CRAB_API_PROVIDER env is **explicitly**
    //      set to that provider. We never pick a provider-specific URL by default,
    //      because the active provider may come from the file layer (which env_to_value
    //      cannot see); guessing here would silently override the wrong target.
    let url = env
        .get("CRAB_BASE_URL")
        .filter(|s| !s.is_empty())
        .or_else(|| {
            let provider = env.get("CRAB_API_PROVIDER")?;
            let var = match provider.as_str() {
                "anthropic" => "ANTHROPIC_BASE_URL",
                "openai" => "OPENAI_BASE_URL",
                "deepseek" => "DEEPSEEK_BASE_URL",
                _ => return None,
            };
            env.get(var).filter(|s| !s.is_empty())
        });
    if let Some(v) = url {
        root.insert("base_url".into(), Value::String(v.clone()));
    }

    Value::Table(root)
}

/// Parse a list of `-c / --config-override` `KEY.PATH=VALUE` strings into a
/// nested partial `toml::Value`.
///
/// The right-hand side is parsed as a TOML scalar/table/array first
/// (`__tmp = <raw>`); if that fails, it falls back to a plain string —
/// matching `codex -c key=value` ergonomics. Multiple overrides are
/// merged in order; later entries deep-merge into earlier ones via the
/// shared [`crate::merge::merge_toml_values`] semantics, so an override
/// like `-c permissions.allow='["Bash"]'` cleanly nests under `[permissions]`
/// without clobbering sibling keys.
///
/// # Errors
/// Returns an error if a spec is missing the `=` separator or carries an
/// empty key path.
pub fn cli_overrides_to_value(overrides: &[String]) -> crab_core::Result<Value> {
    let mut root = Value::Table(Table::new());
    for spec in overrides {
        let (path, raw) = spec.split_once('=').ok_or_else(|| {
            crab_core::Error::Config(format!(
                "invalid -c override '{spec}': expected KEY.PATH=VALUE"
            ))
        })?;
        let path = path.trim();
        if path.is_empty() {
            return Err(crab_core::Error::Config(format!(
                "invalid -c override '{spec}': empty key path"
            )));
        }
        let parts: Vec<&str> = path.split('.').collect();
        if parts.iter().any(|s| s.is_empty()) {
            return Err(crab_core::Error::Config(format!(
                "invalid -c override '{spec}': empty segment in key path '{path}'"
            )));
        }

        let value = parse_override_value(raw);
        let nested = nest_value(&parts, value);
        crate::merge::merge_toml_values(&mut root, nested);
    }
    Ok(root)
}

/// Parse the right-hand side of a `-c` override as TOML, falling back to a
/// plain string when TOML parsing fails. This lets users write either
/// `-c model=opus` (plain identifier) or `-c max_tokens=8192` (integer) or
/// `-c permissions.allow='["Bash(git:*)"]'` (array literal).
fn parse_override_value(raw: &str) -> Value {
    // `toml::from_str` requires a key=value pair, so synthesize one and
    // pluck the value back out. If parsing fails, treat the input as a
    // raw string — codex behaves the same way.
    let synthetic = format!("__tmp__ = {raw}");
    match toml::from_str::<Table>(&synthetic) {
        Ok(table) => table
            .get("__tmp__")
            .cloned()
            .unwrap_or_else(|| Value::String(raw.to_string())),
        Err(_) => Value::String(raw.to_string()),
    }
}

/// Wrap `value` in nested tables according to `parts`, leftmost first.
///
/// `nest_value(&["a","b","c"], 1)` produces `{a:{b:{c:1}}}`.
fn nest_value(parts: &[&str], value: Value) -> Value {
    let mut current = value;
    for segment in parts.iter().rev() {
        let mut table = Table::new();
        table.insert((*segment).to_string(), current);
        current = Value::Table(table);
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_overrides_simple_scalar() {
        let v = cli_overrides_to_value(&["model=opus".to_string()]).unwrap();
        assert_eq!(v["model"].as_str(), Some("opus"));
    }

    #[test]
    fn cli_overrides_nested_path() {
        let v = cli_overrides_to_value(&["permissions.defaultMode=plan".to_string()]).unwrap();
        let perms = v["permissions"].as_table().unwrap();
        assert_eq!(perms["defaultMode"].as_str(), Some("plan"));
    }

    #[test]
    fn cli_overrides_parses_integer() {
        let v = cli_overrides_to_value(&["max_tokens=8192".to_string()]).unwrap();
        assert_eq!(v["max_tokens"].as_integer(), Some(8192));
    }

    #[test]
    fn cli_overrides_parses_array() {
        let v =
            cli_overrides_to_value(&["permissions.allow=[\"Bash(git:*)\"]".to_string()]).unwrap();
        let arr = v["permissions"]["allow"].as_array().unwrap();
        assert_eq!(arr[0].as_str(), Some("Bash(git:*)"));
    }

    #[test]
    fn cli_overrides_parses_boolean() {
        let v = cli_overrides_to_value(&["respectGitignore=false".to_string()]).unwrap();
        assert_eq!(v["respectGitignore"].as_bool(), Some(false));
    }

    #[test]
    fn cli_overrides_falls_back_to_string_for_unquoted_words() {
        // `opus-4-6` is not a legal TOML scalar literal; we accept it as a string.
        let v = cli_overrides_to_value(&["model=opus-4-6".to_string()]).unwrap();
        assert_eq!(v["model"].as_str(), Some("opus-4-6"));
    }

    #[test]
    fn cli_overrides_multiple_merge_into_table() {
        let v = cli_overrides_to_value(&[
            "permissions.allow=[\"Bash\"]".to_string(),
            "permissions.deny=[\"Write\"]".to_string(),
        ])
        .unwrap();
        let perms = v["permissions"].as_table().unwrap();
        assert!(perms.contains_key("allow"));
        assert!(perms.contains_key("deny"));
    }

    #[test]
    fn cli_overrides_later_wins_for_same_key() {
        let v =
            cli_overrides_to_value(&["model=haiku".to_string(), "model=opus".to_string()]).unwrap();
        assert_eq!(v["model"].as_str(), Some("opus"));
    }

    #[test]
    fn cli_overrides_rejects_missing_equals() {
        let err = cli_overrides_to_value(&["model".to_string()]).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("expected KEY.PATH=VALUE"), "got: {msg}");
    }

    #[test]
    fn cli_overrides_rejects_empty_path() {
        let err = cli_overrides_to_value(&["=opus".to_string()]).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("empty key path"), "got: {msg}");
    }

    #[test]
    fn cli_overrides_rejects_empty_segment() {
        let err = cli_overrides_to_value(&["permissions..allow=[]".to_string()]).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("empty segment"), "got: {msg}");
    }

    #[test]
    fn cli_overrides_value_with_equals_sign_is_preserved() {
        // Only the *first* `=` separates key from value; everything after
        // is the raw RHS, including more `=` signs.
        let v =
            cli_overrides_to_value(&["env.PATH=\"/usr/bin:/usr/local/bin\"".to_string()]).unwrap();
        let env_tbl = v["env"].as_table().unwrap();
        assert_eq!(env_tbl["PATH"].as_str(), Some("/usr/bin:/usr/local/bin"));
    }

    fn env_map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn env_to_value_maps_known_keys() {
        let v = env_to_value(&env_map(&[
            ("CRAB_MODEL", "haiku"),
            ("CRAB_API_PROVIDER", "openai"),
            ("CRAB_BASE_URL", "https://example.test"),
        ]));
        let table = v.as_table().unwrap();
        assert_eq!(table["model"].as_str(), Some("haiku"));
        assert_eq!(table["api_provider"].as_str(), Some("openai"));
        assert_eq!(table["base_url"].as_str(), Some("https://example.test"));
    }

    #[test]
    fn env_to_value_url_provider_routing() {
        // DEEPSEEK_BASE_URL only honored when CRAB_API_PROVIDER=deepseek
        let v = env_to_value(&env_map(&[
            ("CRAB_API_PROVIDER", "deepseek"),
            ("DEEPSEEK_BASE_URL", "https://api.deepseek.com/v1"),
            ("ANTHROPIC_BASE_URL", "https://wrong"),
        ]));
        assert_eq!(
            v.as_table().unwrap()["base_url"].as_str(),
            Some("https://api.deepseek.com/v1"),
        );
    }

    #[test]
    fn env_to_value_crab_api_url_wins_over_provider_specific() {
        let v = env_to_value(&env_map(&[
            ("CRAB_API_PROVIDER", "deepseek"),
            ("DEEPSEEK_BASE_URL", "https://provider-specific"),
            ("CRAB_BASE_URL", "https://universal"),
        ]));
        assert_eq!(
            v.as_table().unwrap()["base_url"].as_str(),
            Some("https://universal"),
        );
    }

    #[test]
    fn env_to_value_ignores_unrelated_and_empty() {
        let v = env_to_value(&env_map(&[
            ("PATH", "/usr/bin"),
            ("CRAB_MODEL", ""),
            ("HOME", "/root"),
        ]));
        let table = v.as_table().unwrap();
        assert!(table.is_empty());
    }

    #[test]
    fn env_to_value_maps_default_shell() {
        let v = env_to_value(&env_map(&[("CRAB_DEFAULT_SHELL", "powershell")]));
        assert_eq!(
            v.as_table().unwrap()["default_shell"].as_str(),
            Some("powershell"),
        );
    }

    #[test]
    fn env_to_value_does_not_map_secret_keys() {
        let v = env_to_value(&env_map(&[
            ("CRAB_API_KEY", "sk-secret"),
            ("ANTHROPIC_API_KEY", "sk-anthropic"),
            ("CRAB_MODEL", "opus"),
        ]));
        let table = v.as_table().unwrap();
        assert!(table.contains_key("model"));
        assert!(!table.contains_key("api_key"));
        assert!(!table.contains_key("anthropicApiKey"));
        assert_eq!(table.len(), 1);
    }
}
