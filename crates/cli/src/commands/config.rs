// Configuration management subcommand

use std::path::PathBuf;

use clap::Subcommand;

use crab_config::config;

/// Actions available under `crab config`.
#[derive(Subcommand)]
pub enum ConfigAction {
    /// List all effective configuration values
    List,
    /// Get the value of a specific configuration key
    Get {
        /// Configuration key (e.g. "model", "apiProvider")
        key: String,
    },
    /// Set a configuration value
    Set {
        /// Configuration key (e.g. "model", "apiProvider")
        key: String,
        /// Value to set
        value: String,
        /// Write to global (~/.crab/config.toml) instead of project
        #[arg(long, short)]
        global: bool,
    },
    /// Open the config file in your $EDITOR
    Edit {
        /// Edit global (~/.crab/config.toml) instead of project
        #[arg(long, short)]
        global: bool,
    },
    /// Show configuration file paths
    Path,
}

/// Execute a config subcommand.
pub fn run(action: &ConfigAction) -> anyhow::Result<()> {
    match action {
        ConfigAction::List => cmd_list(),
        ConfigAction::Get { key } => cmd_get(key),
        ConfigAction::Set { key, value, global } => cmd_set(key, value, *global),
        ConfigAction::Edit { global } => cmd_edit(*global),
        ConfigAction::Path => cmd_path(),
    }
}

/// `crab config list` — print all effective config as pretty JSON.
fn cmd_list() -> anyhow::Result<()> {
    let working_dir = std::env::current_dir().ok();
    let ctx = crab_config::ResolveContext::new()
        .with_project_dir(working_dir)
        .with_process_env();
    let merged = crab_config::resolve(&ctx)?;
    let json = serde_json::to_string_pretty(&merged)?;
    println!("{json}");
    Ok(())
}

/// `crab config get <key>` — print a single config value.
fn cmd_get(key: &str) -> anyhow::Result<()> {
    let working_dir = std::env::current_dir().ok();
    let ctx = crab_config::ResolveContext::new()
        .with_project_dir(working_dir)
        .with_process_env();
    let merged = crab_config::resolve(&ctx)?;
    let value = config_to_map(&merged);

    match value.get(key) {
        Some(v) if v.is_null() => {
            println!("(not set)");
        }
        Some(v) => {
            // Print strings without quotes, everything else as JSON
            if let Some(s) = v.as_str() {
                println!("{s}");
            } else {
                println!("{v}");
            }
        }
        None => {
            anyhow::bail!("unknown configuration key: {key}");
        }
    }
    Ok(())
}

/// `crab config set <key> <value>` — write a setting to the chosen config file.
fn cmd_set(key: &str, value: &str, global: bool) -> anyhow::Result<()> {
    // Reject secret fields. Secrets must go through the auth chain, never
    // through a persisted Config field.
    if is_secret_field(key) {
        anyhow::bail!(
            "refusing to write secret field '{key}' via config set; use `crab auth setup-token` instead"
        );
    }

    // Validate key
    let known_keys = known_config_keys();
    if !known_keys.contains(&key) {
        anyhow::bail!(
            "unknown configuration key: {key}\nValid keys: {}",
            known_keys.join(", ")
        );
    }

    let path = target_config_path(global)?;

    // Load existing TOML file (or empty table)
    let mut doc: toml::Table = if path.exists() {
        let content = std::fs::read_to_string(&path)?;
        toml::from_str(&content)
            .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", path.display()))?
    } else {
        toml::Table::new()
    };

    let toml_value = parse_toml_value(value);
    doc.insert(key.to_string(), toml_value);

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let serialized = toml::to_string_pretty(&doc)?;
    std::fs::write(&path, serialized)?;

    eprintln!("Set {key} = {value} in {}", path.display());
    Ok(())
}

/// `crab config edit` — open the config file in $EDITOR.
fn cmd_edit(global: bool) -> anyhow::Result<()> {
    let path = target_config_path(global)?;

    // Ensure the file exists (create empty TOML file if not)
    if !path.exists() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, "")?;
    }

    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| {
            if cfg!(windows) {
                "notepad".to_string()
            } else {
                "vi".to_string()
            }
        });

    let status = std::process::Command::new(&editor).arg(&path).status()?;

    if !status.success() {
        anyhow::bail!("editor exited with status {status}");
    }

    Ok(())
}

/// `crab config path` — show config file paths and existence status.
#[allow(clippy::unnecessary_wraps)]
fn cmd_path() -> anyhow::Result<()> {
    let global_dir = config::global_config_dir();
    let global_config = global_dir.join(config::config_file_name());
    let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let project_dir = config::project_config_dir(&working_dir);
    let project_config = project_dir.join(config::config_file_name());

    println!(
        "Global:  {} {}",
        global_config.display(),
        existence_tag(&global_config)
    );
    println!(
        "Project: {} {}",
        project_config.display(),
        existence_tag(&project_config)
    );

    Ok(())
}

// ── Helpers ────────────────────────────────────────────────────────────

/// Convert `Config` to a JSON map for key lookup.
fn config_to_map(s: &crab_config::Config) -> serde_json::Map<String, serde_json::Value> {
    let v = serde_json::to_value(s).unwrap_or_default();
    match v {
        serde_json::Value::Object(m) => m,
        _ => serde_json::Map::new(),
    }
}

/// List of valid `camelCase` config keys.
fn known_config_keys() -> Vec<&'static str> {
    vec![
        "apiProvider",
        "apiBaseUrl",
        "apiKeyHelper",
        "model",
        "smallModel",
        "maxTokens",
        "permissionMode",
        "systemPrompt",
        "mcpServers",
        "hooks",
        "theme",
    ]
}

/// Top-level config keys that are blacklisted from `crab config set`.
///
/// Match is case-insensitive so that `apikey`, `APIKEY`, and `apiKey` are
/// all rejected — secret material must never reach the persisted `Config`.
fn is_secret_field(key: &str) -> bool {
    const SECRET_KEYS: &[&str] = &["apikey"];
    let lower = key.to_ascii_lowercase();
    SECRET_KEYS.contains(&lower.as_str())
}

/// Resolve the target `config.toml` path.
fn target_config_path(global: bool) -> anyhow::Result<PathBuf> {
    if global {
        Ok(config::global_config_dir().join(config::config_file_name()))
    } else {
        let working_dir = std::env::current_dir()?;
        Ok(config::project_config_dir(&working_dir).join(config::config_file_name()))
    }
}

/// Try to parse a string value as a typed TOML value.
fn parse_toml_value(value: &str) -> toml::Value {
    if let Ok(n) = value.parse::<i64>() {
        return toml::Value::Integer(n);
    }
    match value {
        "true" => return toml::Value::Boolean(true),
        "false" => return toml::Value::Boolean(false),
        _ => {}
    }
    // Inline TOML expressions (e.g. arrays / inline tables) — wrap and parse.
    let trimmed = value.trim_start();
    if (trimmed.starts_with('[') || trimmed.starts_with('{'))
        && let Ok(parsed) = toml::from_str::<toml::Table>(&format!("__v = {value}\n"))
        && let Some(v) = parsed.get("__v")
    {
        return v.clone();
    }
    toml::Value::String(value.to_string())
}

/// Return "(exists)" or "(not found)" for display.
fn existence_tag(path: &std::path::Path) -> &'static str {
    if path.exists() {
        "(exists)"
    } else {
        "(not found)"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_value_integer() {
        assert_eq!(parse_toml_value("42"), toml::Value::Integer(42));
        assert_eq!(parse_toml_value("0"), toml::Value::Integer(0));
        assert_eq!(parse_toml_value("-1"), toml::Value::Integer(-1));
    }

    #[test]
    fn parse_value_bool() {
        assert_eq!(parse_toml_value("true"), toml::Value::Boolean(true));
        assert_eq!(parse_toml_value("false"), toml::Value::Boolean(false));
    }

    #[test]
    fn parse_value_string() {
        assert_eq!(
            parse_toml_value("anthropic"),
            toml::Value::String("anthropic".into())
        );
        assert_eq!(
            parse_toml_value("gpt-4o"),
            toml::Value::String("gpt-4o".into())
        );
    }

    #[test]
    fn parse_value_inline_array() {
        let v = parse_toml_value(r#"["a", "b"]"#);
        assert!(v.is_array());
    }

    #[test]
    fn parse_value_inline_table() {
        let v = parse_toml_value(r#"{ key = "val" }"#);
        assert!(v.is_table());
        assert_eq!(v["key"].as_str(), Some("val"));
    }

    #[test]
    fn parse_value_brace_like_string_not_valid_inline_table() {
        // A string that starts with '{' but is not valid TOML should be a string
        let v = parse_toml_value("{not toml");
        assert!(v.is_str());
    }

    #[test]
    fn known_keys_contains_expected() {
        let keys = known_config_keys();
        assert!(keys.contains(&"apiProvider"));
        assert!(keys.contains(&"model"));
        assert!(keys.contains(&"maxTokens"));
        assert!(keys.contains(&"theme"));
    }

    #[test]
    fn config_to_map_roundtrip() {
        let s = crab_config::Config {
            model: Some("test-model".into()),
            theme: Some("dark".into()),
            ..Default::default()
        };
        let map = config_to_map(&s);
        assert_eq!(
            map.get("model").and_then(|v| v.as_str()),
            Some("test-model")
        );
        assert_eq!(map.get("theme").and_then(|v| v.as_str()), Some("dark"));
    }

    #[test]
    fn config_to_map_none_fields_are_null() {
        let s = crab_config::Config::default();
        let map = config_to_map(&s);
        assert!(map.get("model").is_some_and(serde_json::Value::is_null));
    }

    #[test]
    fn existence_tag_nonexistent() {
        assert_eq!(
            existence_tag(std::path::Path::new("/nonexistent/path")),
            "(not found)"
        );
    }

    #[test]
    fn cmd_list_succeeds() {
        // Should not panic — loads merged config and prints JSON
        let result = cmd_list();
        assert!(result.is_ok());
    }

    #[test]
    fn cmd_get_known_key() {
        // "model" is a valid key, even if its value is null
        let result = cmd_get("model");
        assert!(result.is_ok());
    }

    #[test]
    fn cmd_get_unknown_key() {
        let result = cmd_get("nonexistentKey123");
        assert!(result.is_err());
    }

    #[test]
    fn cmd_set_unknown_key_errors() {
        let result = cmd_set("badKey999", "value", true);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("unknown configuration key"));
    }

    #[test]
    fn cmd_set_rejects_api_key_lowercase() {
        let result = cmd_set("apikey", "sk-test", true);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("refusing to write secret field"));
    }

    #[test]
    fn cmd_set_rejects_api_key_camelcase() {
        let result = cmd_set("apiKey", "sk-test", true);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("refusing to write secret field"));
        assert!(msg.contains("setup-token"));
    }

    #[test]
    fn cmd_set_rejects_api_key_uppercase() {
        let result = cmd_set("APIKEY", "sk-test", true);
        assert!(result.is_err());
    }

    #[test]
    fn is_secret_field_classifies_apikey() {
        assert!(is_secret_field("apiKey"));
        assert!(is_secret_field("apikey"));
        assert!(is_secret_field("APIKEY"));
        assert!(is_secret_field("ApiKey"));
        assert!(!is_secret_field("apiKeyHelper"));
        assert!(!is_secret_field("apiProvider"));
        assert!(!is_secret_field("model"));
    }

    #[test]
    fn cmd_set_and_get_roundtrip() {
        let dir = std::env::temp_dir().join("crab-cli-config-test-set-get");
        let crab_dir = dir.join(".crab");
        let _ = std::fs::create_dir_all(&crab_dir);
        let config_file = crab_dir.join("config.toml");
        std::fs::write(&config_file, "").unwrap();

        // Simulate cmd_set logic without going through the global helper.
        let mut doc = toml::Table::new();
        doc.insert("model".into(), parse_toml_value("test-model-123"));
        let serialized = toml::to_string_pretty(&doc).unwrap();
        std::fs::write(&config_file, serialized).unwrap();

        // Read back
        let content = std::fs::read_to_string(&config_file).unwrap();
        let parsed: toml::Table = toml::from_str(&content).unwrap();
        assert_eq!(parsed["model"].as_str(), Some("test-model-123"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cmd_path_succeeds() {
        let result = cmd_path();
        assert!(result.is_ok());
    }
}
