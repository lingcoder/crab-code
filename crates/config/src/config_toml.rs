//! Multi-provider TOML configuration support.
//!
//! Loads `~/.config/crab-code/config.toml` which supports multiple provider
//! profiles. The active provider's settings are merged into a `Settings` struct.
//!
//! Example `config.toml`:
//! ```toml
//! default_provider = "anthropic"
//!
//! [providers.anthropic]
//! api_key = "sk-ant-..."
//! model = "claude-sonnet-4-20250514"
//!
//! [providers.openai]
//! api_base_url = "https://api.openai.com/v1"
//! api_key = "sk-..."
//! model = "gpt-4o"
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::Settings;

/// Top-level structure of `config.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ConfigToml {
    /// Which provider profile to use by default.
    pub default_provider: Option<String>,
    /// Named provider profiles.
    #[serde(default)]
    pub providers: HashMap<String, ProviderProfile>,
}

/// A single provider profile within `config.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderProfile {
    pub api_base_url: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub small_model: Option<String>,
    pub max_tokens: Option<u32>,
}

/// Return the path to the multi-provider config file:
/// `~/.config/crab-code/config.toml` (XDG-style).
#[must_use]
pub fn config_toml_path() -> PathBuf {
    crab_core::common::utils::path::home_dir()
        .join(".config")
        .join("crab-code")
        .join("config.toml")
}

/// Load `ConfigToml` from a file path.
/// Returns `Ok(ConfigToml::default())` if the file does not exist.
fn load_config_toml_from(path: &Path) -> crab_core::Result<ConfigToml> {
    match std::fs::read_to_string(path) {
        Ok(content) => toml::from_str(&content)
            .map_err(|e| crab_core::Error::Config(format!("config.toml parse error: {e}"))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ConfigToml::default()),
        Err(e) => Err(crab_core::Error::Config(format!(
            "failed to read {}: {e}",
            path.display()
        ))),
    }
}

/// Load the global `config.toml` from `~/.config/crab-code/config.toml`.
pub fn load_config_toml() -> crab_core::Result<ConfigToml> {
    load_config_toml_from(&config_toml_path())
}

/// Convert a `ConfigToml` into `Settings` by selecting the active provider.
///
/// If `provider_override` is `Some`, that profile is used.
/// Otherwise, `default_provider` from the config is used.
/// If neither is set, returns default (empty) settings with just
/// the `default_provider` as `api_provider`.
#[must_use]
pub fn config_toml_to_settings(config: &ConfigToml, provider_override: Option<&str>) -> Settings {
    let provider_name = provider_override
        .or(config.default_provider.as_deref())
        .unwrap_or("anthropic");

    let profile = config.providers.get(provider_name);

    Settings {
        api_provider: Some(provider_name.to_string()),
        api_base_url: profile.and_then(|p| p.api_base_url.clone()),
        api_key: profile.and_then(|p| p.api_key.clone()),
        model: profile.and_then(|p| p.model.clone()),
        small_model: profile.and_then(|p| p.small_model.clone()),
        max_tokens: profile.and_then(|p| p.max_tokens),
        ..Settings::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_toml_path_under_config() {
        let path = config_toml_path();
        assert!(path.ends_with("crab-code/config.toml"));
    }

    #[test]
    fn load_nonexistent_returns_default() {
        let result = load_config_toml_from(Path::new("/nonexistent/config.toml"));
        assert!(result.is_ok());
        let config = result.unwrap();
        assert!(config.default_provider.is_none());
        assert!(config.providers.is_empty());
    }

    #[test]
    fn parse_valid_toml() {
        let dir = std::env::temp_dir().join("crab-config-toml-test-parse");
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("config.toml");
        std::fs::write(
            &file,
            r#"
default_provider = "openai"

[providers.anthropic]
api_key = "sk-ant-123"
model = "claude-sonnet-4-20250514"

[providers.openai]
api_base_url = "https://api.openai.com/v1"
api_key = "sk-oai-456"
model = "gpt-4o"
max_tokens = 8192
"#,
        )
        .unwrap();

        let config = load_config_toml_from(&file).unwrap();
        assert_eq!(config.default_provider.as_deref(), Some("openai"));
        assert_eq!(config.providers.len(), 2);

        let anthropic = &config.providers["anthropic"];
        assert_eq!(anthropic.api_key.as_deref(), Some("sk-ant-123"));

        let openai = &config.providers["openai"];
        assert_eq!(openai.model.as_deref(), Some("gpt-4o"));
        assert_eq!(openai.max_tokens, Some(8192));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_toml_to_settings_uses_default_provider() {
        let config = ConfigToml {
            default_provider: Some("openai".into()),
            providers: HashMap::from([(
                "openai".into(),
                ProviderProfile {
                    model: Some("gpt-4o".into()),
                    api_key: Some("sk-oai".into()),
                    ..Default::default()
                },
            )]),
        };

        let settings = config_toml_to_settings(&config, None);
        assert_eq!(settings.api_provider.as_deref(), Some("openai"));
        assert_eq!(settings.model.as_deref(), Some("gpt-4o"));
        assert_eq!(settings.api_key.as_deref(), Some("sk-oai"));
    }

    #[test]
    fn config_toml_to_settings_provider_override() {
        let config = ConfigToml {
            default_provider: Some("openai".into()),
            providers: HashMap::from([
                (
                    "openai".into(),
                    ProviderProfile {
                        model: Some("gpt-4o".into()),
                        ..Default::default()
                    },
                ),
                (
                    "anthropic".into(),
                    ProviderProfile {
                        model: Some("claude-3".into()),
                        ..Default::default()
                    },
                ),
            ]),
        };

        let settings = config_toml_to_settings(&config, Some("anthropic"));
        assert_eq!(settings.api_provider.as_deref(), Some("anthropic"));
        assert_eq!(settings.model.as_deref(), Some("claude-3"));
    }

    #[test]
    fn config_toml_to_settings_missing_profile_returns_defaults() {
        let config = ConfigToml {
            default_provider: Some("unknown".into()),
            providers: HashMap::new(),
        };

        let settings = config_toml_to_settings(&config, None);
        assert_eq!(settings.api_provider.as_deref(), Some("unknown"));
        assert!(settings.model.is_none());
        assert!(settings.api_key.is_none());
    }

    #[test]
    fn config_toml_to_settings_no_default_falls_to_anthropic() {
        let config = ConfigToml::default();
        let settings = config_toml_to_settings(&config, None);
        assert_eq!(settings.api_provider.as_deref(), Some("anthropic"));
    }

    #[test]
    fn parse_invalid_toml_returns_error() {
        let dir = std::env::temp_dir().join("crab-config-toml-test-invalid");
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("config.toml");
        std::fs::write(&file, "not valid toml [[[").unwrap();

        let result = load_config_toml_from(&file);
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_empty_toml() {
        let dir = std::env::temp_dir().join("crab-config-toml-test-empty");
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("config.toml");
        std::fs::write(&file, "").unwrap();

        let config = load_config_toml_from(&file).unwrap();
        assert!(config.default_provider.is_none());
        assert!(config.providers.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_toml_serde_roundtrip() {
        let config = ConfigToml {
            default_provider: Some("anthropic".into()),
            providers: HashMap::from([(
                "anthropic".into(),
                ProviderProfile {
                    api_key: Some("sk-123".into()),
                    model: Some("claude-3".into()),
                    small_model: Some("haiku".into()),
                    max_tokens: Some(4096),
                    api_base_url: None,
                },
            )]),
        };
        let toml_str = toml::to_string(&config).unwrap();
        let back: ConfigToml = toml::from_str(&toml_str).unwrap();
        assert_eq!(back.default_provider, config.default_provider);
        assert_eq!(
            back.providers["anthropic"].api_key,
            config.providers["anthropic"].api_key
        );
    }
}
