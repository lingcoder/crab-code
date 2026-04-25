use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Global config directory name.
const CONFIG_DIR: &str = ".crab";
/// User/project config file name within `.crab/`.
const CONFIG_FILE: &str = "config.toml";
/// Project-local override file name within `.crab/` (gitignored).
const LOCAL_CONFIG_FILE: &str = "config.local.toml";

/// Application configuration, loaded from `~/.crab/config.toml` (global)
/// and `.crab/config.toml` (project-level), with `.crab/config.local.toml`
/// applied on top.
///
/// All fields are `Option` to support multi-level merge: global → project →
/// local → CLI overrides. Uses `camelCase` for compatibility with the existing
/// JSON-shaped tooling and CCB ecosystem.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "gen-schema", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    // ── Provider / auth ──
    pub api_provider: Option<String>,
    pub api_base_url: Option<String>,
    /// Shell command that prints an API key to stdout.
    /// The path is configuration; the secret value the script returns
    /// never enters `Config` (resolved by the auth module at request time).
    pub api_key_helper: Option<String>,

    // ── Model ──
    pub model: Option<String>,
    /// Smaller/faster model for auxiliary tasks.
    pub small_model: Option<String>,
    /// Alias for `small_model` (CC-compatible field name).
    #[serde(default)]
    pub advisor_model: Option<String>,
    pub available_models: Option<Vec<String>>,
    /// Model alias map, e.g. `{"fast": "claude-haiku"}`.
    pub model_overrides: Option<HashMap<String, String>>,
    pub max_tokens: Option<u32>,

    // ── Permissions ──
    /// Structured permission rules: `{allow, deny, defaultMode}`.
    pub permissions: Option<PermissionsConfig>,
    /// Flat permission mode shorthand (convenience alias).
    pub permission_mode: Option<String>,

    // ── Prompts / instructions ──
    pub system_prompt: Option<String>,
    pub include_git_instructions: Option<bool>,
    pub custom_instructions: Option<String>,

    // ── MCP ──
    pub mcp_servers: Option<serde_json::Value>,
    pub enable_all_project_mcp_servers: Option<bool>,

    // ── Hooks ──
    pub hooks: Option<serde_json::Value>,
    pub disable_all_hooks: Option<bool>,

    // ── Shell / environment ──
    pub default_shell: Option<String>,
    pub env: Option<HashMap<String, String>>,

    // ── UI / display ──
    pub theme: Option<String>,
    pub language: Option<String>,
    pub output_style: Option<String>,

    // ── Git ──
    pub git_context: Option<GitContextConfig>,
    pub respect_gitignore: Option<bool>,

    // ── Memory ──
    pub auto_memory_enabled: Option<bool>,
    pub auto_memory_directory: Option<String>,

    // ── Plugins ──
    /// Map of `plugin-id@marketplace-id` keys to enablement values.
    /// `Bool(true)` enables the plugin; `Bool(false)` explicitly disables one
    /// that would otherwise be enabled by default (e.g. bundled plugins);
    /// `VersionConstraints(...)` carries semver-like constraint strings.
    pub enabled_plugins: Option<HashMap<String, EnabledPluginValue>>,

    // ── Misc ──
    pub cleanup_period_days: Option<u32>,
}

/// Value variants accepted under `enabledPlugins.<key>`.
/// Aligned with CCB `types.ts:566-574`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "gen-schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum EnabledPluginValue {
    Bool(bool),
    VersionConstraints(Vec<String>),
}

impl EnabledPluginValue {
    /// Return `true` if this value enables the plugin.
    /// Both `Bool(true)` and any `VersionConstraints` list (even empty) are
    /// treated as enabled — the constraint vector implies opt-in.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        match self {
            Self::Bool(b) => *b,
            Self::VersionConstraints(_) => true,
        }
    }
}

/// Structured permission rules: `{allow, deny, defaultMode}`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "gen-schema", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "camelCase")]
pub struct PermissionsConfig {
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
    pub default_mode: Option<String>,
    pub additional_directories: Option<Vec<String>>,
}

/// Configuration for git context injection into system prompts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "gen-schema", derive(schemars::JsonSchema))]
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

impl Config {
    /// Resolve `small_model` / `advisor_model` aliasing.
    /// Both `smallModel` and `advisorModel` are accepted in the config file.
    #[must_use]
    pub fn effective_small_model(&self) -> Option<&str> {
        self.small_model
            .as_deref()
            .or(self.advisor_model.as_deref())
    }
}

/// Return the global config directory: `~/.crab/`.
#[must_use]
pub fn global_config_dir() -> PathBuf {
    crab_core::common::utils::path::home_dir().join(CONFIG_DIR)
}

/// Return the project config directory: `<project_dir>/.crab/`.
#[must_use]
pub fn project_config_dir(project_dir: &Path) -> PathBuf {
    project_dir.join(CONFIG_DIR)
}

/// File name used for the user/project `Config` file (`config.toml`).
#[must_use]
pub const fn config_file_name() -> &'static str {
    CONFIG_FILE
}

/// File name used for the project-local override file (`config.local.toml`).
#[must_use]
pub const fn local_config_file_name() -> &'static str {
    LOCAL_CONFIG_FILE
}

/// Parse a TOML config string into a `Config`.
///
/// Applies schema migrations (if needed) on the raw value before
/// deserialization so that older config files are transparently upgraded.
fn parse_toml(content: &str) -> crab_core::Result<Config> {
    let toml_value: toml::Value = toml::from_str(content)
        .map_err(|e| crab_core::Error::Config(format!("TOML parse error: {e}")))?;
    let mut json = toml_value_to_json(toml_value);
    crate::migration::migrate_settings(&mut json);
    serde_json::from_value(json)
        .map_err(|e| crab_core::Error::Config(format!("config deserialization error: {e}")))
}

/// Recursively convert a `toml::Value` to a `serde_json::Value`.
///
/// Used as a bridge so that the migration layer (which operates on JSON
/// values for historical reasons) and the `serde_json::Value` fields
/// inside `Config` (`mcp_servers`, `hooks`) keep working unchanged.
/// Crate-internal export used by the validation module.
pub(crate) fn toml_value_to_json_for_validation(value: toml::Value) -> serde_json::Value {
    toml_value_to_json(value)
}

fn toml_value_to_json(value: toml::Value) -> serde_json::Value {
    match value {
        toml::Value::String(s) => serde_json::Value::String(s),
        toml::Value::Integer(i) => serde_json::Value::Number(i.into()),
        toml::Value::Float(f) => serde_json::Number::from_f64(f)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        toml::Value::Boolean(b) => serde_json::Value::Bool(b),
        toml::Value::Datetime(dt) => serde_json::Value::String(dt.to_string()),
        toml::Value::Array(arr) => {
            serde_json::Value::Array(arr.into_iter().map(toml_value_to_json).collect())
        }
        toml::Value::Table(tbl) => serde_json::Value::Object(
            tbl.into_iter()
                .map(|(k, v)| (k, toml_value_to_json(v)))
                .collect(),
        ),
    }
}

/// Load config from a specific TOML file.
/// Returns `Ok(Config::default())` if the file does not exist.
fn load_from_file(path: &Path) -> crab_core::Result<Config> {
    match std::fs::read_to_string(path) {
        Ok(content) => parse_toml(&content),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(crab_core::Error::Config(format!(
            "failed to read {}: {e}",
            path.display()
        ))),
    }
}

/// Load config from an explicit TOML file path. Unlike `load_from_file`,
/// this returns an error if the file does not exist.
pub fn load_config_from_path(path: &Path) -> crab_core::Result<Config> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| crab_core::Error::Config(format!("failed to read {}: {e}", path.display())))?;
    parse_toml(&content)
}

/// Load global config from `~/.crab/config.toml`.
pub fn load_global() -> crab_core::Result<Config> {
    let path = global_config_dir().join(CONFIG_FILE);
    load_from_file(&path)
}

/// Load project-level config from `<project_dir>/.crab/config.toml`.
pub fn load_project(project_dir: &Path) -> crab_core::Result<Config> {
    let path = project_config_dir(project_dir).join(CONFIG_FILE);
    load_from_file(&path)
}

/// Load project-local config from `<project_dir>/.crab/config.local.toml`.
/// This file is intended to be gitignored and holds per-developer overrides.
pub fn load_local(project_dir: &Path) -> crab_core::Result<Config> {
    let path = project_config_dir(project_dir).join(LOCAL_CONFIG_FILE);
    load_from_file(&path)
}

/// Which config sources to include in the merge chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSource {
    User,
    Project,
    Local,
}

impl ConfigSource {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_all_none() {
        let s = Config::default();
        assert!(s.api_provider.is_none());
        assert!(s.api_key_helper.is_none());
        assert!(s.model.is_none());
        assert!(s.max_tokens.is_none());
    }

    #[test]
    fn parse_toml_basic() {
        let toml_str = r#"
apiProvider = "openai"
model = "gpt-4o"
"#;
        let s = parse_toml(toml_str).unwrap();
        assert_eq!(s.api_provider.as_deref(), Some("openai"));
        assert_eq!(s.model.as_deref(), Some("gpt-4o"));
    }

    #[test]
    fn parse_toml_empty() {
        let s = parse_toml("").unwrap();
        assert_eq!(s, Config::default());
    }

    #[test]
    fn parse_toml_with_camel_case() {
        let toml_str = r#"
apiBaseUrl = "http://localhost:8080"
maxTokens = 2048
"#;
        let s = parse_toml(toml_str).unwrap();
        assert_eq!(s.api_base_url.as_deref(), Some("http://localhost:8080"));
        assert_eq!(s.max_tokens, Some(2048));
    }

    #[test]
    fn parse_toml_unknown_fields_ignored() {
        let toml_str = r#"
unknownField = true
model = "test"
"#;
        let s = parse_toml(toml_str).unwrap();
        assert_eq!(s.model.as_deref(), Some("test"));
    }

    #[test]
    fn parse_toml_with_table() {
        let toml_str = r#"
[gitContext]
enabled = false
maxDiffLines = 50
"#;
        let s = parse_toml(toml_str).unwrap();
        let git_ctx = s.git_context.unwrap();
        assert!(!git_ctx.enabled);
        assert_eq!(git_ctx.max_diff_lines, 50);
    }

    #[test]
    fn load_from_nonexistent_file_returns_default() {
        let s = load_from_file(Path::new("/nonexistent/path/config.toml")).unwrap();
        assert_eq!(s, Config::default());
    }

    #[test]
    fn load_from_temp_file() {
        let dir = std::env::temp_dir().join("crab-config-test-load");
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("config.toml");
        std::fs::write(
            &file,
            r#"apiProvider = "deepseek"
model = "deepseek-chat"
"#,
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
    fn config_roundtrip_serde() {
        let s = Config {
            api_provider: Some("anthropic".into()),
            max_tokens: Some(4096),
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        let deserialized: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(s, deserialized);
    }

    #[test]
    fn parse_toml_invalid_returns_error() {
        let result = parse_toml("not = valid = toml");
        assert!(result.is_err());
    }

    #[test]
    fn load_from_invalid_toml_file() {
        let dir = std::env::temp_dir().join("crab-config-test-invalid");
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("config.toml");
        std::fs::write(&file, "not = valid = toml").unwrap();

        let result = load_from_file(&file);
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_all_fields_serde_roundtrip() {
        let s = Config {
            api_provider: Some("anthropic".into()),
            api_base_url: Some("http://localhost:8080".into()),
            api_key_helper: Some("/usr/local/bin/get-key.sh".into()),
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
            ..Default::default()
        };
        let json = serde_json::to_string_pretty(&s).unwrap();
        let deserialized: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(s, deserialized);
    }

    // ── load_config_from_path tests ──

    #[test]
    fn load_config_from_path_reads_file() {
        let dir = std::env::temp_dir().join("crab-config-test-from-path");
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("custom.toml");
        std::fs::write(&file, r#"model = "custom-model""#).unwrap();

        let s = load_config_from_path(&file).unwrap();
        assert_eq!(s.model.as_deref(), Some("custom-model"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_config_from_path_errors_on_missing_file() {
        let result = load_config_from_path(Path::new("/nonexistent/custom.toml"));
        assert!(result.is_err());
    }

    // ── load_local tests ──

    #[test]
    fn load_local_reads_local_config() {
        let dir = std::env::temp_dir().join("crab-config-test-local");
        let crab_dir = dir.join(".crab");
        let _ = std::fs::create_dir_all(&crab_dir);
        std::fs::write(
            crab_dir.join("config.local.toml"),
            r#"model = "local-model""#,
        )
        .unwrap();

        let s = load_local(&dir).unwrap();
        assert_eq!(s.model.as_deref(), Some("local-model"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_local_returns_default_when_missing() {
        let dir = std::env::temp_dir().join("crab-config-test-local-missing");
        let _ = std::fs::create_dir_all(&dir);

        let s = load_local(&dir).unwrap();
        assert_eq!(s, Config::default());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── ConfigSource tests ──

    #[test]
    fn config_source_parse_list_all() {
        let sources = ConfigSource::parse_list("user,project,local");
        assert_eq!(
            sources,
            vec![
                ConfigSource::User,
                ConfigSource::Project,
                ConfigSource::Local
            ]
        );
    }

    #[test]
    fn config_source_parse_list_single() {
        let sources = ConfigSource::parse_list("project");
        assert_eq!(sources, vec![ConfigSource::Project]);
    }

    #[test]
    fn config_source_parse_list_unknown_ignored() {
        let sources = ConfigSource::parse_list("user,unknown,local");
        assert_eq!(sources, vec![ConfigSource::User, ConfigSource::Local]);
    }

    #[test]
    fn toml_value_to_json_handles_all_kinds() {
        let toml_str = r#"
s = "x"
i = 42
f = 1.5
b = true
arr = [1, 2, 3]
[tbl]
inner = "v"
"#;
        let val: toml::Value = toml::from_str(toml_str).unwrap();
        let json = toml_value_to_json(val);
        assert_eq!(json["s"], "x");
        assert_eq!(json["i"], 42);
        assert_eq!(json["b"], true);
        assert_eq!(json["arr"][0], 1);
        assert_eq!(json["tbl"]["inner"], "v");
    }
}
