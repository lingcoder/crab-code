use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Global config directory name.
const CONFIG_DIR: &str = ".crab";
/// Settings file name within config directories.
const SETTINGS_FILE: &str = "settings.json";

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
        }
    }
}

/// Return the global config directory: `~/.crab/`.
#[must_use]
pub fn global_config_dir() -> PathBuf {
    crab_common::path::home_dir().join(CONFIG_DIR)
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

/// Load and merge settings: global → project.
/// Project-level values override global values.
pub fn load_merged_settings(project_dir: Option<&PathBuf>) -> crab_common::Result<Settings> {
    let global = load_global()?;
    match project_dir {
        Some(dir) => {
            let project = load_project(dir)?;
            Ok(global.merge(&project))
        }
        None => Ok(global),
    }
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
}
