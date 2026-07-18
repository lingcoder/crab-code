//! Discovery of Claude Code installed plugins.
//!
//! CC installs marketplace plugins under `~/.claude/plugins/`:
//!
//! ```text
//! ~/.claude/plugins/
//! ├── installed_plugins.json   # {"plugins": {"name@marketplace": [{installPath, ...}]}}
//! └── cache/<marketplace>/<plugin>/<version>/   # CC plugin layout
//! ```
//!
//! Activation is controlled by the `enabledPlugins` map in CC settings
//! (`{"name@marketplace": true}`). Only entries explicitly enabled load.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::loader::{LoadedPlugin, load_plugin};

#[derive(Debug, Deserialize)]
struct InstalledRegistry {
    #[serde(default)]
    plugins: std::collections::BTreeMap<String, Vec<InstalledEntry>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstalledEntry {
    install_path: PathBuf,
}

/// Discover Claude Code installed plugins from a `.claude/plugins` directory.
///
/// `enabled_plugins` is the `enabledPlugins` map from CC settings; a plugin
/// loads only when its `name@marketplace` key is explicitly `true`. The
/// returned plugin names are the plugin-name part of the registry key.
#[must_use]
pub fn discover_cc_installed(
    claude_plugins_dir: &Path,
    enabled_plugins: Option<&serde_json::Value>,
) -> Vec<LoadedPlugin> {
    let registry_path = claude_plugins_dir.join("installed_plugins.json");
    let Ok(content) = std::fs::read_to_string(&registry_path) else {
        return Vec::new();
    };
    let registry: InstalledRegistry = match serde_json::from_str(&content) {
        Ok(registry) => registry,
        Err(e) => {
            tracing::warn!(
                path = %registry_path.display(),
                error = %e,
                "ignoring malformed CC installed_plugins.json"
            );
            return Vec::new();
        }
    };

    let mut plugins = Vec::new();
    for (key, entries) in registry.plugins {
        let enabled = enabled_plugins
            .and_then(|map| map.get(&key))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if !enabled {
            continue;
        }
        let Some(install_path) = entries
            .iter()
            .map(|e| e.install_path.as_path())
            .find(|p| p.is_dir())
        else {
            tracing::warn!(
                plugin = key.as_str(),
                "CC plugin install path missing; skipping"
            );
            continue;
        };
        match load_plugin(install_path) {
            Ok(mut plugin) => {
                plugin.name = key
                    .split_once('@')
                    .map_or_else(|| key.clone(), |(name, _)| name.to_string());
                if plugin.has_components() {
                    plugins.push(plugin);
                }
            }
            Err(e) => {
                tracing::warn!(
                    plugin = key.as_str(),
                    error = %e,
                    "skipping invalid CC installed plugin"
                );
            }
        }
    }
    plugins
}

/// Read the `enabledPlugins` map from a CC settings file, if present.
#[must_use]
pub fn read_cc_enabled_plugins(claude_settings_path: &Path) -> Option<serde_json::Value> {
    let content = std::fs::read_to_string(claude_settings_path).ok()?;
    let settings: serde_json::Value = serde_json::from_str(&content).ok()?;
    settings.get("enabledPlugins").cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(path, content).unwrap();
    }

    fn fixture(base: &Path) -> PathBuf {
        let plugins_dir = base.join("plugins");
        let install = plugins_dir
            .join("cache")
            .join("mp")
            .join("demo")
            .join("1.0");
        write(
            &install.join("commands").join("ship.md"),
            "---\ndescription: Ship\n---\nShip.",
        );
        let registry = serde_json::json!({
            "version": 2,
            "plugins": {
                "demo@mp": [{
                    "scope": "user",
                    "installPath": install.to_string_lossy(),
                    "version": "1.0"
                }],
                "disabled-one@mp": [{
                    "scope": "user",
                    "installPath": install.to_string_lossy(),
                    "version": "1.0"
                }]
            }
        });
        write(
            &plugins_dir.join("installed_plugins.json"),
            &registry.to_string(),
        );
        plugins_dir
    }

    #[test]
    fn discovers_only_enabled_plugins() {
        let base = std::env::temp_dir().join("crab_cc_installed_enabled");
        let _ = std::fs::remove_dir_all(&base);
        let plugins_dir = fixture(&base);

        let enabled = serde_json::json!({"demo@mp": true, "disabled-one@mp": false});
        let plugins = discover_cc_installed(&plugins_dir, Some(&enabled));
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "demo");
        assert_eq!(plugins[0].skill_dirs.len(), 1);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn no_enabled_map_loads_nothing() {
        let base = std::env::temp_dir().join("crab_cc_installed_none");
        let _ = std::fs::remove_dir_all(&base);
        let plugins_dir = fixture(&base);
        assert!(discover_cc_installed(&plugins_dir, None).is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn missing_registry_is_empty() {
        let dir = std::env::temp_dir().join("crab_cc_installed_missing");
        let _ = std::fs::create_dir_all(&dir);
        assert!(discover_cc_installed(&dir, None).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_enabled_plugins_from_settings() {
        let dir = std::env::temp_dir().join("crab_cc_settings_read");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("settings.json");
        write(&path, r#"{"enabledPlugins": {"a@m": true}}"#);
        let map = read_cc_enabled_plugins(&path).unwrap();
        assert_eq!(map["a@m"], true);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
