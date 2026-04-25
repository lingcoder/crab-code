// Configuration management subcommand

use std::path::PathBuf;

use clap::Subcommand;

use crab_config::config;
use crab_config::writer::{self, WriteTarget};

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
        /// Configuration key (e.g. "model", "apiProvider", "permissions.allow")
        key: String,
        /// Value to set
        value: String,
        /// Write to global (~/.crab/config.toml) instead of project
        #[arg(long, short, conflicts_with = "local")]
        global: bool,
        /// Write to project-local (.crab/config.local.toml), gitignored
        #[arg(long, short, conflicts_with = "global")]
        local: bool,
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
        ConfigAction::Set {
            key,
            value,
            global,
            local,
        } => cmd_set(key, value, *global, *local),
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
///
/// Routes through [`writer::set_value`] so comments and key order are preserved
/// via `toml_edit`, secret-adjacent fields are rejected, and the post-write
/// schema check rolls back any change that would leave a broken file on disk.
fn cmd_set(key: &str, value: &str, global: bool, local: bool) -> anyhow::Result<()> {
    let target = if global {
        WriteTarget::User
    } else if local {
        WriteTarget::Local
    } else {
        WriteTarget::Project
    };

    writer::set_value(target, key, value).map_err(anyhow::Error::from)?;

    eprintln!(
        "Set {key} = {value} in {}",
        describe_target(target).display()
    );
    Ok(())
}

/// Resolve a human-readable on-disk path for a `WriteTarget`, matching the
/// path the writer actually mutated. Used purely for the success log line.
fn describe_target(target: WriteTarget) -> PathBuf {
    let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match target {
        WriteTarget::User => config::global_config_dir().join(config::config_file_name()),
        WriteTarget::Project => {
            config::project_config_dir(&working_dir).join(config::config_file_name())
        }
        WriteTarget::Local => {
            config::project_config_dir(&working_dir).join(config::local_config_file_name())
        }
    }
}

/// `crab config edit` — open the config file in $EDITOR.
///
/// Per `docs/config.md` §10.2 the CLI never creates config dirs or files just
/// to read them. We hand the path to the editor as-is; the editor (or the OS)
/// creates the file when the user actually saves a non-empty buffer. The
/// writer module owns first-time directory creation for `crab config set`.
fn cmd_edit(global: bool) -> anyhow::Result<()> {
    let path = target_config_path(global)?;

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

/// Resolve the target `config.toml` path used by `crab config edit`.
fn target_config_path(global: bool) -> anyhow::Result<PathBuf> {
    if global {
        Ok(config::global_config_dir().join(config::config_file_name()))
    } else {
        let working_dir = std::env::current_dir()?;
        Ok(config::project_config_dir(&working_dir).join(config::config_file_name()))
    }
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
    fn cmd_set_rejects_api_key_camelcase() {
        let result = cmd_set(
            "apiKey", "sk-test", /*global=*/ true, /*local=*/ false,
        );
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("refusing to write secret"), "{msg}");
    }

    #[test]
    fn describe_target_user_uses_global_config_filename() {
        let path = describe_target(WriteTarget::User);
        assert_eq!(
            path.file_name().and_then(|s| s.to_str()),
            Some(config::config_file_name())
        );
    }

    #[test]
    fn describe_target_local_uses_local_filename() {
        let path = describe_target(WriteTarget::Local);
        assert_eq!(
            path.file_name().and_then(|s| s.to_str()),
            Some(config::local_config_file_name())
        );
    }

    #[test]
    fn cmd_path_succeeds() {
        let result = cmd_path();
        assert!(result.is_ok());
    }
}
