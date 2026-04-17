use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Global config directory name.
const CONFIG_DIR: &str = ".crab";
/// Settings file name within config directories.
const SETTINGS_FILE: &str = "settings.json";

/// Anti-tracking and anti-upgrade configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
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

/// Application settings, loaded from `~/.crab/settings.json` (global)
/// and `.crab/settings.json` (project-level).
///
/// All fields are `Option` to support three-level merge: global → project → CLI overrides.
/// Uses `camelCase` for JSON compatibility.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub api_provider: Option<String>,
    pub api_base_url: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub small_model: Option<String>,
    pub max_tokens: Option<u32>,
    pub permission_mode: Option<String>,
    pub system_prompt: Option<String>,
    pub mcp_servers: Option<serde_json::Value>,
    pub hooks: Option<serde_json::Value>,
    pub theme: Option<String>,
    pub git_context: Option<GitContextConfig>,
    /// Environment variables to inject into the process.
    /// CC-compatible: `{"env": {"ANTHROPIC_API_KEY": "sk-ant-xxx"}}`.
    pub env: Option<HashMap<String, String>>,
    /// Anti-tracking and anti-upgrade settings.
    pub anti_track: Option<AntiTrackConfig>,
}

/// Configuration for git context injection into system prompts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct GitContextConfig {
    /// Whether to inject git context into the system prompt (default: true).
    pub enabled: bool,
    /// Maximum number of diff lines to include (default: 200).
    pub max_diff_lines: usize,
}

impl Default for GitContextConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_diff_lines: 200,
        }
    }
}

impl Settings {
    /// Merge another `Settings` on top of `self`.
    /// Non-`None` fields in `other` override fields in `self`.
    #[must_use]
    pub fn merge(self, other: &Self) -> Self {
        Self {
            api_provider: other.api_provider.clone().or(self.api_provider),
            api_base_url: other.api_base_url.clone().or(self.api_base_url),
            api_key: other.api_key.clone().or(self.api_key),
            model: other.model.clone().or(self.model),
            small_model: other.small_model.clone().or(self.small_model),
            max_tokens: other.max_tokens.or(self.max_tokens),
            permission_mode: other.permission_mode.clone().or(self.permission_mode),
            system_prompt: other.system_prompt.clone().or(self.system_prompt),
            mcp_servers: other.mcp_servers.clone().or(self.mcp_servers),
            hooks: other.hooks.clone().or(self.hooks),
            theme: other.theme.clone().or(self.theme),
            git_context: other.git_context.clone().or(self.git_context),
            env: match (&self.env, &other.env) {
                (Some(base), Some(over)) => {
                    let mut merged = base.clone();
                    merged.extend(over.iter().map(|(k, v)| (k.clone(), v.clone())));
                    Some(merged)
                }
                (None, Some(over)) => Some(over.clone()),
                (Some(base), None) => Some(base.clone()),
                (None, None) => None,
            },
            anti_track: other.anti_track.clone().or(self.anti_track),
        }
    }
}

/// Return the global config directory: `~/.crab/`.
#[must_use]
pub fn global_config_dir() -> PathBuf {
    crab_common::utils::path::home_dir().join(CONFIG_DIR)
}

/// Return the project config directory: `<project_dir>/.crab/`.
#[must_use]
pub fn project_config_dir(project_dir: &Path) -> PathBuf {
    project_dir.join(CONFIG_DIR)
}

/// Parse JSONC (JSON with comments) into a `Settings`.
fn parse_jsonc(content: &str) -> crab_common::Result<Settings> {
    let json = jsonc_parser::parse_to_serde_value::<serde_json::Value>(
        content,
        &jsonc_parser::ParseOptions::default(),
    )
    .map_err(|e| crab_common::Error::Config(format!("JSONC parse error: {e}")))?;
    serde_json::from_value(json)
        .map_err(|e| crab_common::Error::Config(format!("settings deserialization error: {e}")))
}

/// Load settings from a specific JSON/JSONC file.
/// Returns `Ok(Settings::default())` if the file does not exist.
fn load_from_file(path: &Path) -> crab_common::Result<Settings> {
    match std::fs::read_to_string(path) {
        Ok(content) => parse_jsonc(&content),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(e) => Err(crab_common::Error::Config(format!(
            "failed to read {}: {e}",
            path.display()
        ))),
    }
}

/// Load settings from an explicit file path. Unlike the internal `load_from_file`,
/// this returns an error if the file does not exist.
pub fn load_settings_from_path(path: &Path) -> crab_common::Result<Settings> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        crab_common::Error::Config(format!("failed to read {}: {e}", path.display()))
    })?;
    parse_jsonc(&content)
}

/// Load global settings from `~/.crab/settings.json`.
pub fn load_global() -> crab_common::Result<Settings> {
    let path = global_config_dir().join(SETTINGS_FILE);
    load_from_file(&path)
}

/// Load project-level settings from `<project_dir>/.crab/settings.json`.
pub fn load_project(project_dir: &Path) -> crab_common::Result<Settings> {
    let path = project_config_dir(project_dir).join(SETTINGS_FILE);
    load_from_file(&path)
}

/// Load project-local settings from `<project_dir>/.crab/settings.local.json`.
/// This file is intended to be gitignored and holds per-developer overrides.
pub fn load_local(project_dir: &Path) -> crab_common::Result<Settings> {
    let path = project_config_dir(project_dir).join("settings.local.json");
    load_from_file(&path)
}

/// Load and merge settings with full priority chain:
///
/// `config.toml defaults → global settings.json → project settings.json → env vars`
///
/// Environment variables checked (highest priority):
/// - `CRAB_API_PROVIDER` — override provider
/// - `CRAB_API_KEY` — override API key
/// - `CRAB_MODEL` — override model
/// - `CRAB_API_BASE_URL` — override base URL
///
/// Provider-specific API key env vars (used when `CRAB_API_KEY` is not set):
/// - `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `DEEPSEEK_API_KEY`
pub fn load_merged_settings(project_dir: Option<&PathBuf>) -> crab_common::Result<Settings> {
    load_merged_settings_with_env(project_dir, |k| std::env::var(k))
}

/// Which setting sources to include in the merge chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingSource {
    User,
    Project,
    Local,
}

impl SettingSource {
    /// Parse a comma-separated string like "user,project,local".
    pub fn parse_list(s: &str) -> Vec<Self> {
        s.split(',')
            .filter_map(|part| match part.trim() {
                "user" => Some(Self::User),
                "project" => Some(Self::Project),
                "local" => Some(Self::Local),
                _ => None,
            })
            .collect()
    }
}

/// Load and merge settings with configurable source layers.
///
/// When `sources` is `None`, all layers are included:
/// config.toml → user → project → local → env vars
///
/// When `sources` is `Some(list)`, only the listed layers are included.
pub fn load_merged_settings_with_sources(
    project_dir: Option<&PathBuf>,
    sources: Option<&[SettingSource]>,
) -> crab_common::Result<Settings> {
    load_merged_settings_with_env_and_sources(project_dir, |k| std::env::var(k), sources)
}

/// Inner merge implementation, parameterized over env var lookup for testability.
///
/// Merge chain: config.toml → user settings.json → project settings.json
///              → local settings.local.json → env vars
fn load_merged_settings_with_env<F>(
    project_dir: Option<&PathBuf>,
    env_lookup: F,
) -> crab_common::Result<Settings>
where
    F: Fn(&str) -> std::result::Result<String, std::env::VarError>,
{
    load_merged_settings_with_env_and_sources(project_dir, env_lookup, None)
}

/// Core merge implementation with source filtering.
fn load_merged_settings_with_env_and_sources<F>(
    project_dir: Option<&PathBuf>,
    env_lookup: F,
    sources: Option<&[SettingSource]>,
) -> crab_common::Result<Settings>
where
    F: Fn(&str) -> std::result::Result<String, std::env::VarError>,
{
    let include_all = sources.is_none();
    let has = |s: SettingSource| include_all || sources.is_some_and(|list| list.contains(&s));

    // 1. config.toml defaults (always loaded, lowest priority)
    let config_toml = crate::config_toml::load_config_toml()?;
    let merged = crate::config_toml::config_toml_to_settings(&config_toml, None);

    // 2. User (~/.crab/settings.json)
    let merged = if has(SettingSource::User) {
        let global = load_global()?;
        merged.merge(&global)
    } else {
        merged
    };

    // 3. Project (.crab/settings.json)
    let merged = if has(SettingSource::Project) {
        match project_dir {
            Some(dir) => {
                let project = load_project(dir)?;
                merged.merge(&project)
            }
            None => merged,
        }
    } else {
        merged
    };

    // 4. Local (.crab/settings.local.json)
    let merged = if has(SettingSource::Local) {
        match project_dir {
            Some(dir) => {
                let local = load_local(dir)?;
                merged.merge(&local)
            }
            None => merged,
        }
    } else {
        merged
    };

    // 5. Environment variable overrides (always applied, highest priority).
    //    The env_lookup falls back to settings.env (CC-compatible) so that
    //    `{"env": {"ANTHROPIC_API_KEY": "sk-..."}}` in settings.json works.
    let settings_env = merged.env.clone();
    let env_with_fallback = move |key: &str| -> std::result::Result<String, std::env::VarError> {
        env_lookup(key).or_else(|_| {
            settings_env
                .as_ref()
                .and_then(|m| m.get(key).cloned())
                .ok_or(std::env::VarError::NotPresent)
        })
    };
    let env_overlay = build_env_overlay(&env_with_fallback, merged.api_provider.as_deref());
    Ok(merged.merge(&env_overlay))
}

/// Build a `Settings` from environment variables.
///
/// Checks generic `CRAB_*` vars first, then falls back to provider-specific
/// API key vars based on the current provider.
fn build_env_overlay<F>(env_lookup: &F, current_provider: Option<&str>) -> Settings
where
    F: Fn(&str) -> std::result::Result<String, std::env::VarError>,
{
    let api_provider = env_lookup("CRAB_API_PROVIDER").ok();
    let effective_provider = api_provider
        .as_deref()
        .or(current_provider)
        .unwrap_or("anthropic");
    let api_base_url = env_lookup("CRAB_API_BASE_URL")
        .ok()
        // CC-compatible: ANTHROPIC_BASE_URL only when using anthropic provider
        .or_else(|| {
            if effective_provider == "anthropic" {
                env_lookup("ANTHROPIC_BASE_URL").ok()
            } else {
                None
            }
        })
        .filter(|v| !v.is_empty());
    let model = env_lookup("CRAB_MODEL").ok();

    // For API key: CRAB_API_KEY takes priority, then provider-specific vars
    let api_key = env_lookup("CRAB_API_KEY")
        .ok()
        .or_else(|| provider_api_key_env(effective_provider, env_lookup));

    Settings {
        api_provider,
        api_base_url,
        api_key,
        model,
        ..Settings::default()
    }
}

/// Resolve a provider-specific API key from environment variables.
fn provider_api_key_env<F>(provider: &str, env_lookup: &F) -> Option<String>
where
    F: Fn(&str) -> std::result::Result<String, std::env::VarError>,
{
    let var_name = match provider {
        "deepseek" => "DEEPSEEK_API_KEY",
        "openai" | "ollama" | "vllm" => "OPENAI_API_KEY",
        _ => "ANTHROPIC_API_KEY",
    };
    env_lookup(var_name)
        .ok()
        .filter(|v| !v.is_empty())
        // CC-compatible: ANTHROPIC_AUTH_TOKEN as fallback for Anthropic providers
        .or_else(|| {
            if var_name == "ANTHROPIC_API_KEY" {
                env_lookup("ANTHROPIC_AUTH_TOKEN")
                    .ok()
                    .filter(|v| !v.is_empty())
            } else {
                None
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_all_none() {
        let s = Settings::default();
        assert!(s.api_provider.is_none());
        assert!(s.api_key.is_none());
        assert!(s.model.is_none());
        assert!(s.max_tokens.is_none());
    }

    #[test]
    fn merge_other_overrides_self() {
        let base = Settings {
            api_provider: Some("anthropic".into()),
            model: Some("old-model".into()),
            max_tokens: Some(1024),
            ..Default::default()
        };
        let overlay = Settings {
            model: Some("new-model".into()),
            theme: Some("dark".into()),
            ..Default::default()
        };
        let merged = base.merge(&overlay);
        assert_eq!(merged.api_provider.as_deref(), Some("anthropic")); // kept
        assert_eq!(merged.model.as_deref(), Some("new-model")); // overridden
        assert_eq!(merged.max_tokens, Some(1024)); // kept
        assert_eq!(merged.theme.as_deref(), Some("dark")); // added
    }

    #[test]
    fn merge_none_does_not_clear() {
        let base = Settings {
            api_key: Some("sk-123".into()),
            ..Default::default()
        };
        let empty = Settings::default();
        let merged = base.merge(&empty);
        assert_eq!(merged.api_key.as_deref(), Some("sk-123"));
    }

    #[test]
    fn parse_jsonc_with_comments() {
        let jsonc = r#"{
            // This is a comment
            "apiProvider": "openai",
            "model": "gpt-4o"
            /* block comment */
        }"#;
        let s = parse_jsonc(jsonc).unwrap();
        assert_eq!(s.api_provider.as_deref(), Some("openai"));
        assert_eq!(s.model.as_deref(), Some("gpt-4o"));
    }

    #[test]
    fn parse_jsonc_empty_object() {
        let s = parse_jsonc("{}").unwrap();
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn parse_jsonc_with_camel_case() {
        let jsonc = r#"{"apiBaseUrl": "http://localhost:8080", "maxTokens": 2048}"#;
        let s = parse_jsonc(jsonc).unwrap();
        assert_eq!(s.api_base_url.as_deref(), Some("http://localhost:8080"));
        assert_eq!(s.max_tokens, Some(2048));
    }

    #[test]
    fn parse_jsonc_unknown_fields_ignored() {
        let jsonc = r#"{"unknownField": true, "model": "test"}"#;
        let s = parse_jsonc(jsonc).unwrap();
        assert_eq!(s.model.as_deref(), Some("test"));
    }

    #[test]
    fn load_from_nonexistent_file_returns_default() {
        let s = load_from_file(Path::new("/nonexistent/path/settings.json")).unwrap();
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn load_from_temp_file() {
        let dir = std::env::temp_dir().join("crab-config-test-load");
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("settings.json");
        std::fs::write(
            &file,
            r#"{"apiProvider": "deepseek", "model": "deepseek-chat"}"#,
        )
        .unwrap();

        let s = load_from_file(&file).unwrap();
        assert_eq!(s.api_provider.as_deref(), Some("deepseek"));
        assert_eq!(s.model.as_deref(), Some("deepseek-chat"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn global_config_dir_under_home() {
        let dir = global_config_dir();
        assert!(dir.ends_with(".crab"));
    }

    #[test]
    fn project_config_dir_under_project() {
        let dir = project_config_dir(Path::new("/my/project"));
        assert!(dir.ends_with(".crab"));
        assert!(dir.starts_with("/my/project"));
    }

    #[test]
    fn load_merged_without_project() {
        // Should not panic even if ~/.crab/ doesn't exist
        let result = load_merged_settings(None);
        assert!(result.is_ok());
    }

    #[test]
    fn load_merged_with_project_overlay() {
        let dir = std::env::temp_dir().join("crab-config-test-merge");
        let crab_dir = dir.join(".crab");
        let _ = std::fs::create_dir_all(&crab_dir);
        std::fs::write(
            crab_dir.join("settings.json"),
            r#"{"model": "project-model"}"#,
        )
        .unwrap();

        let result = load_merged_settings(Some(&dir)).unwrap();
        assert_eq!(result.model.as_deref(), Some("project-model"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_roundtrip_serde() {
        let s = Settings {
            api_provider: Some("anthropic".into()),
            max_tokens: Some(4096),
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        let deserialized: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, deserialized);
    }

    #[test]
    fn merge_all_fields_override() {
        let base = Settings {
            api_provider: Some("anthropic".into()),
            api_base_url: Some("http://old".into()),
            api_key: Some("sk-old".into()),
            model: Some("old-model".into()),
            small_model: Some("old-small".into()),
            max_tokens: Some(1024),
            permission_mode: Some("default".into()),
            system_prompt: Some("old prompt".into()),
            mcp_servers: Some(serde_json::json!({"old": true})),
            hooks: Some(serde_json::json!([])),
            theme: Some("light".into()),
            git_context: Some(GitContextConfig {
                enabled: true,
                max_diff_lines: 100,
            }),
            env: None,
        };
        let overlay = Settings {
            api_provider: Some("openai".into()),
            api_base_url: Some("http://new".into()),
            api_key: Some("sk-new".into()),
            model: Some("new-model".into()),
            small_model: Some("new-small".into()),
            max_tokens: Some(4096),
            permission_mode: Some("dangerously".into()),
            system_prompt: Some("new prompt".into()),
            mcp_servers: Some(serde_json::json!({"new": true})),
            hooks: Some(serde_json::json!([{"trigger": "pre_tool_use"}])),
            theme: Some("dark".into()),
            git_context: Some(GitContextConfig {
                enabled: false,
                max_diff_lines: 50,
            }),
            env: None,
        };
        let merged = base.merge(&overlay);
        assert_eq!(merged.api_provider.as_deref(), Some("openai"));
        assert_eq!(merged.api_base_url.as_deref(), Some("http://new"));
        assert_eq!(merged.api_key.as_deref(), Some("sk-new"));
        assert_eq!(merged.model.as_deref(), Some("new-model"));
        assert_eq!(merged.small_model.as_deref(), Some("new-small"));
        assert_eq!(merged.max_tokens, Some(4096));
        assert_eq!(merged.permission_mode.as_deref(), Some("dangerously"));
        assert_eq!(merged.system_prompt.as_deref(), Some("new prompt"));
        assert_eq!(merged.theme.as_deref(), Some("dark"));
    }

    #[test]
    fn parse_jsonc_trailing_comma() {
        // jsonc_parser should handle trailing commas
        let jsonc = r#"{"model": "gpt-4o",}"#;
        let s = parse_jsonc(jsonc).unwrap();
        assert_eq!(s.model.as_deref(), Some("gpt-4o"));
    }

    #[test]
    fn parse_jsonc_invalid_json_returns_error() {
        let result = parse_jsonc("not json at all");
        assert!(result.is_err());
    }

    #[test]
    fn parse_jsonc_null_values_become_none() {
        let jsonc = r#"{"model": null, "maxTokens": null}"#;
        let s = parse_jsonc(jsonc).unwrap();
        assert!(s.model.is_none());
        assert!(s.max_tokens.is_none());
    }

    #[test]
    fn load_from_invalid_json_file() {
        let dir = std::env::temp_dir().join("crab-config-test-invalid");
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("settings.json");
        std::fs::write(&file, "{ broken json }").unwrap();

        let result = load_from_file(&file);
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_all_fields_serde_roundtrip() {
        let s = Settings {
            api_provider: Some("anthropic".into()),
            api_base_url: Some("http://localhost:8080".into()),
            api_key: Some("sk-test".into()),
            model: Some("claude-3".into()),
            small_model: Some("haiku".into()),
            max_tokens: Some(8192),
            permission_mode: Some("trust-project".into()),
            system_prompt: Some("Be helpful".into()),
            mcp_servers: Some(serde_json::json!({"server1": {}})),
            hooks: Some(serde_json::json!([{"trigger": "pre_tool_use", "command": "echo"}])),
            theme: Some("dark".into()),
            git_context: Some(GitContextConfig::default()),
            env: Some(HashMap::from([("FOO".into(), "bar".into())])),
        };
        let json = serde_json::to_string_pretty(&s).unwrap();
        let deserialized: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, deserialized);
    }

    #[test]
    fn merge_is_not_commutative() {
        let a = Settings {
            model: Some("model-a".into()),
            ..Default::default()
        };
        let b = Settings {
            model: Some("model-b".into()),
            ..Default::default()
        };
        // a.merge(&b) should give model-b, b.clone().merge(&a) should give model-a
        assert_eq!(a.clone().merge(&b).model.as_deref(), Some("model-b"));
        assert_eq!(b.merge(&a).model.as_deref(), Some("model-a"));
    }

    #[test]
    fn load_merged_project_overrides_global() {
        let dir = std::env::temp_dir().join("crab-config-test-merged-override");
        let crab_dir = dir.join(".crab");
        let _ = std::fs::create_dir_all(&crab_dir);
        std::fs::write(
            crab_dir.join("settings.json"),
            r#"{"model": "project-model", "theme": "dark"}"#,
        )
        .unwrap();

        let result = load_merged_settings(Some(&dir)).unwrap();
        assert_eq!(result.model.as_deref(), Some("project-model"));
        assert_eq!(result.theme.as_deref(), Some("dark"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Environment variable override tests ──────────────────────────────

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
    fn build_env_overlay_empty_when_no_vars() {
        let overlay = build_env_overlay(&no_env, None);
        assert!(overlay.api_provider.is_none());
        assert!(overlay.api_key.is_none());
        assert!(overlay.model.is_none());
        assert!(overlay.api_base_url.is_none());
    }

    #[test]
    fn build_env_overlay_crab_vars() {
        let env = fake_env(HashMap::from([
            ("CRAB_API_PROVIDER", "openai"),
            ("CRAB_API_KEY", "sk-env-key"),
            ("CRAB_MODEL", "gpt-4o"),
            ("CRAB_API_BASE_URL", "http://localhost:8080"),
        ]));
        let overlay = build_env_overlay(&env, None);
        assert_eq!(overlay.api_provider.as_deref(), Some("openai"));
        assert_eq!(overlay.api_key.as_deref(), Some("sk-env-key"));
        assert_eq!(overlay.model.as_deref(), Some("gpt-4o"));
        assert_eq!(
            overlay.api_base_url.as_deref(),
            Some("http://localhost:8080")
        );
    }

    #[test]
    fn build_env_overlay_provider_specific_api_key() {
        // No CRAB_API_KEY, but ANTHROPIC_API_KEY is set
        let env = fake_env(HashMap::from([("ANTHROPIC_API_KEY", "ant-key")]));
        let overlay = build_env_overlay(&env, Some("anthropic"));
        assert_eq!(overlay.api_key.as_deref(), Some("ant-key"));
    }

    #[test]
    fn build_env_overlay_crab_api_key_overrides_provider_key() {
        let env = fake_env(HashMap::from([
            ("CRAB_API_KEY", "crab-key"),
            ("ANTHROPIC_API_KEY", "ant-key"),
        ]));
        let overlay = build_env_overlay(&env, Some("anthropic"));
        assert_eq!(overlay.api_key.as_deref(), Some("crab-key"));
    }

    #[test]
    fn build_env_overlay_openai_provider_key() {
        let env = fake_env(HashMap::from([("OPENAI_API_KEY", "oai-key")]));
        let overlay = build_env_overlay(&env, Some("openai"));
        assert_eq!(overlay.api_key.as_deref(), Some("oai-key"));
    }

    #[test]
    fn build_env_overlay_deepseek_provider_key() {
        let env = fake_env(HashMap::from([("DEEPSEEK_API_KEY", "ds-key")]));
        let overlay = build_env_overlay(&env, Some("deepseek"));
        assert_eq!(overlay.api_key.as_deref(), Some("ds-key"));
    }

    #[test]
    fn build_env_overlay_empty_provider_key_is_skipped() {
        let env = fake_env(HashMap::from([("ANTHROPIC_API_KEY", "")]));
        let overlay = build_env_overlay(&env, Some("anthropic"));
        assert!(overlay.api_key.is_none());
    }

    #[test]
    fn build_env_overlay_provider_from_env_affects_key_lookup() {
        // CRAB_API_PROVIDER=openai, no CRAB_API_KEY, OPENAI_API_KEY set
        let env = fake_env(HashMap::from([
            ("CRAB_API_PROVIDER", "openai"),
            ("OPENAI_API_KEY", "oai-from-env"),
        ]));
        let overlay = build_env_overlay(&env, Some("anthropic"));
        // Should use openai key because CRAB_API_PROVIDER overrides current_provider
        assert_eq!(overlay.api_key.as_deref(), Some("oai-from-env"));
    }

    #[test]
    fn provider_api_key_env_routing() {
        let env = fake_env(HashMap::from([
            ("ANTHROPIC_API_KEY", "ant"),
            ("OPENAI_API_KEY", "oai"),
            ("DEEPSEEK_API_KEY", "ds"),
        ]));
        assert_eq!(provider_api_key_env("anthropic", &env), Some("ant".into()));
        assert_eq!(provider_api_key_env("openai", &env), Some("oai".into()));
        assert_eq!(provider_api_key_env("deepseek", &env), Some("ds".into()));
        assert_eq!(provider_api_key_env("ollama", &env), Some("oai".into()));
        assert_eq!(provider_api_key_env("vllm", &env), Some("oai".into()));
        assert_eq!(provider_api_key_env("unknown", &env), Some("ant".into()));
    }

    #[test]
    fn build_env_overlay_anthropic_auth_token_fallback() {
        let env = fake_env(HashMap::from([("ANTHROPIC_AUTH_TOKEN", "cr_token123")]));
        let overlay = build_env_overlay(&env, None);
        assert_eq!(overlay.api_key.as_deref(), Some("cr_token123"));
    }

    #[test]
    fn build_env_overlay_anthropic_base_url_fallback() {
        let env = fake_env(HashMap::from([(
            "ANTHROPIC_BASE_URL",
            "http://proxy.example.com/api",
        )]));
        let overlay = build_env_overlay(&env, None);
        assert_eq!(
            overlay.api_base_url.as_deref(),
            Some("http://proxy.example.com/api")
        );
    }

    #[test]
    fn build_env_overlay_crab_base_url_overrides_anthropic() {
        let env = fake_env(HashMap::from([
            ("CRAB_API_BASE_URL", "http://crab.example.com"),
            ("ANTHROPIC_BASE_URL", "http://anthropic.example.com"),
        ]));
        let overlay = build_env_overlay(&env, None);
        assert_eq!(
            overlay.api_base_url.as_deref(),
            Some("http://crab.example.com")
        );
    }

    #[test]
    fn load_merged_env_overrides_project() {
        // Set up a project with model = "project-model"
        let dir = std::env::temp_dir().join("crab-config-test-env-override");
        let crab_dir = dir.join(".crab");
        let _ = std::fs::create_dir_all(&crab_dir);
        std::fs::write(
            crab_dir.join("settings.json"),
            r#"{"model": "project-model", "apiKey": "project-key"}"#,
        )
        .unwrap();

        // Env vars should override project settings
        let env = fake_env(HashMap::from([
            ("CRAB_MODEL", "env-model"),
            ("CRAB_API_KEY", "env-key"),
        ]));

        let result = load_merged_settings_with_env(Some(&dir), env).unwrap();
        assert_eq!(result.model.as_deref(), Some("env-model")); // env wins
        assert_eq!(result.api_key.as_deref(), Some("env-key")); // env wins

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_merged_no_env_preserves_settings() {
        let dir = std::env::temp_dir().join("crab-config-test-no-env");
        let crab_dir = dir.join(".crab");
        let _ = std::fs::create_dir_all(&crab_dir);
        std::fs::write(
            crab_dir.join("settings.json"),
            r#"{"model": "project-model", "theme": "dark"}"#,
        )
        .unwrap();

        let result = load_merged_settings_with_env(Some(&dir), no_env).unwrap();
        assert_eq!(result.model.as_deref(), Some("project-model"));
        assert_eq!(result.theme.as_deref(), Some("dark"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn full_merge_priority_chain() {
        // This test verifies the complete priority chain:
        // config.toml < global settings.json < project settings.json < env vars
        //
        // We can only control project settings + env vars in tests
        // (global settings.json and config.toml depend on user's home dir),
        // but we verify that env vars override project settings.
        let dir = std::env::temp_dir().join("crab-config-test-full-chain");
        let crab_dir = dir.join(".crab");
        let _ = std::fs::create_dir_all(&crab_dir);
        std::fs::write(
            crab_dir.join("settings.json"),
            r#"{
                "apiProvider": "anthropic",
                "model": "project-model",
                "apiKey": "project-key",
                "theme": "dark"
            }"#,
        )
        .unwrap();

        // Env overrides only model and provider — key and theme come from project
        let env = fake_env(HashMap::from([
            ("CRAB_API_PROVIDER", "openai"),
            ("CRAB_MODEL", "env-model"),
        ]));

        let result = load_merged_settings_with_env(Some(&dir), env).unwrap();
        assert_eq!(result.api_provider.as_deref(), Some("openai")); // env wins
        assert_eq!(result.model.as_deref(), Some("env-model")); // env wins
        assert_eq!(result.api_key.as_deref(), Some("project-key")); // project preserved
        assert_eq!(result.theme.as_deref(), Some("dark")); // project preserved

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── load_settings_from_path tests ──

    #[test]
    fn load_settings_from_path_reads_file() {
        let dir = std::env::temp_dir().join("crab-config-test-from-path");
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("custom.json");
        std::fs::write(&file, r#"{"model": "custom-model"}"#).unwrap();

        let s = load_settings_from_path(&file).unwrap();
        assert_eq!(s.model.as_deref(), Some("custom-model"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_settings_from_path_errors_on_missing_file() {
        let result = load_settings_from_path(Path::new("/nonexistent/custom.json"));
        assert!(result.is_err());
    }

    // ── load_local tests ──

    #[test]
    fn load_local_reads_settings_local_json() {
        let dir = std::env::temp_dir().join("crab-config-test-local");
        let crab_dir = dir.join(".crab");
        let _ = std::fs::create_dir_all(&crab_dir);
        std::fs::write(
            crab_dir.join("settings.local.json"),
            r#"{"apiKey": "local-secret"}"#,
        )
        .unwrap();

        let s = load_local(&dir).unwrap();
        assert_eq!(s.api_key.as_deref(), Some("local-secret"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_local_returns_default_when_missing() {
        let dir = std::env::temp_dir().join("crab-config-test-local-missing");
        let _ = std::fs::create_dir_all(&dir);

        let s = load_local(&dir).unwrap();
        assert_eq!(s, Settings::default());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── SettingSource tests ──

    #[test]
    fn setting_source_parse_list_all() {
        let sources = SettingSource::parse_list("user,project,local");
        assert_eq!(
            sources,
            vec![
                SettingSource::User,
                SettingSource::Project,
                SettingSource::Local
            ]
        );
    }

    #[test]
    fn setting_source_parse_list_single() {
        let sources = SettingSource::parse_list("project");
        assert_eq!(sources, vec![SettingSource::Project]);
    }

    #[test]
    fn setting_source_parse_list_unknown_ignored() {
        let sources = SettingSource::parse_list("user,unknown,local");
        assert_eq!(sources, vec![SettingSource::User, SettingSource::Local]);
    }

    // ── Source-filtered settings loading ──

    #[test]
    fn load_merged_with_sources_skips_project() {
        let dir = std::env::temp_dir().join("crab-config-test-source-skip");
        let crab_dir = dir.join(".crab");
        let _ = std::fs::create_dir_all(&crab_dir);
        std::fs::write(
            crab_dir.join("settings.json"),
            r#"{"model": "project-model"}"#,
        )
        .unwrap();

        // Only load user, not project — project model should not appear
        let result = load_merged_settings_with_env_and_sources(
            Some(&dir),
            no_env,
            Some(&[SettingSource::User]),
        )
        .unwrap();
        // Model should NOT be "project-model" since we skipped the project source
        assert_ne!(result.model.as_deref(), Some("project-model"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_merged_with_all_sources_includes_local() {
        let dir = std::env::temp_dir().join("crab-config-test-source-local");
        let crab_dir = dir.join(".crab");
        let _ = std::fs::create_dir_all(&crab_dir);
        std::fs::write(
            crab_dir.join("settings.json"),
            r#"{"model": "project-model"}"#,
        )
        .unwrap();
        std::fs::write(
            crab_dir.join("settings.local.json"),
            r#"{"model": "local-model"}"#,
        )
        .unwrap();

        // Load all sources — local should override project
        let result = load_merged_settings_with_env_and_sources(
            Some(&dir),
            no_env,
            None, // all sources
        )
        .unwrap();
        assert_eq!(result.model.as_deref(), Some("local-model"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
