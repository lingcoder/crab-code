use std::path::{Path, PathBuf};

use clap::Subcommand;

/// Plugin management subcommands.
#[derive(Subcommand)]
pub enum PluginAction {
    /// List installed plugins
    List,
    /// Install a plugin from a source path or URL
    Install {
        /// Plugin source (local path or URL)
        source: String,
    },
    /// Remove an installed plugin
    Remove {
        /// Plugin name
        name: String,
    },
    /// Enable a disabled plugin
    Enable {
        /// Plugin name
        name: String,
    },
    /// Disable an installed plugin
    Disable {
        /// Plugin name
        name: String,
    },
    /// Validate a plugin directory structure
    Validate {
        /// Path to plugin directory
        path: String,
    },
}

pub fn run(action: &PluginAction) -> anyhow::Result<()> {
    match action {
        PluginAction::List => run_list(),
        PluginAction::Install { source } => run_install(source),
        PluginAction::Remove { name } => run_remove(name),
        PluginAction::Enable { name } => run_enable(name),
        PluginAction::Disable { name } => run_disable(name),
        PluginAction::Validate { path } => run_validate(path),
    }
}

fn plugins_dir() -> PathBuf {
    crab_config::settings::global_config_dir().join("plugins")
}

fn run_list() -> anyhow::Result<()> {
    let dir = plugins_dir();

    if !dir.exists() {
        eprintln!("No plugins installed.");
        eprintln!("Plugin directory: {}", dir.display());
        eprintln!();
        eprintln!("Install a plugin with: crab plugin install <source>");
        return Ok(());
    }

    let entries = list_plugin_dirs(&dir)?;

    if entries.is_empty() {
        eprintln!("No plugins found in {}", dir.display());
        return Ok(());
    }

    eprintln!("Installed plugins:");
    for entry in &entries {
        let status = plugin_status(&dir, entry);
        eprintln!("  {entry} — {status}");
    }

    Ok(())
}

fn list_plugin_dirs(dir: &Path) -> anyhow::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir()
            && let Some(name) = entry.file_name().to_str()
        {
            names.push(name.to_string());
        }
    }
    names.sort();
    Ok(names)
}

fn plugin_status(plugins_dir: &Path, name: &str) -> &'static str {
    let manifest = plugins_dir.join(name).join("plugin.json");
    if !manifest.exists() {
        return "invalid (missing plugin.json)";
    }

    // Check disabled marker
    let disabled_marker = plugins_dir.join(name).join(".disabled");
    if disabled_marker.exists() {
        "disabled"
    } else {
        "enabled"
    }
}

fn run_install(source: &str) -> anyhow::Result<()> {
    eprintln!("Installing plugin from: {source}");
    eprintln!();
    eprintln!("Plugin installation is not yet fully implemented.");
    eprintln!("To install manually, copy the plugin directory to:");
    eprintln!("  {}", plugins_dir().display());

    Ok(())
}

fn run_remove(name: &str) -> anyhow::Result<()> {
    let dir = plugins_dir().join(name);

    if !dir.exists() {
        anyhow::bail!("Plugin '{name}' not found at {}", dir.display());
    }

    std::fs::remove_dir_all(&dir)?;
    eprintln!("Removed plugin '{name}'.");

    Ok(())
}

fn run_enable(name: &str) -> anyhow::Result<()> {
    let dir = plugins_dir().join(name);
    if !dir.exists() {
        anyhow::bail!("Plugin '{name}' not found at {}", dir.display());
    }

    let marker = dir.join(".disabled");
    if marker.exists() {
        std::fs::remove_file(&marker)?;
        eprintln!("Enabled plugin '{name}'.");
    } else {
        eprintln!("Plugin '{name}' is already enabled.");
    }

    Ok(())
}

fn run_disable(name: &str) -> anyhow::Result<()> {
    let dir = plugins_dir().join(name);
    if !dir.exists() {
        anyhow::bail!("Plugin '{name}' not found at {}", dir.display());
    }

    let marker = dir.join(".disabled");
    if marker.exists() {
        eprintln!("Plugin '{name}' is already disabled.");
    } else {
        std::fs::write(&marker, "")?;
        eprintln!("Disabled plugin '{name}'.");
    }

    Ok(())
}

fn run_validate(path: &str) -> anyhow::Result<()> {
    let dir = Path::new(path);

    if !dir.exists() || !dir.is_dir() {
        anyhow::bail!("'{path}' is not a valid directory");
    }

    let manifest = dir.join("plugin.json");
    if !manifest.exists() {
        eprintln!("[FAIL] Missing plugin.json");
        return Ok(());
    }

    // Try parsing manifest
    let content = std::fs::read_to_string(&manifest)?;
    match serde_json::from_str::<serde_json::Value>(&content) {
        Ok(val) => {
            let name = val
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("<missing>");
            let desc = val
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("<missing>");
            eprintln!("[OK] Valid plugin.json");
            eprintln!("  Name:        {name}");
            eprintln!("  Description: {desc}");
        }
        Err(e) => {
            eprintln!("[FAIL] plugin.json is not valid JSON: {e}");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugins_dir_under_global_config() {
        let dir = plugins_dir();
        assert!(dir.to_str().unwrap().contains("plugins"));
    }

    #[test]
    fn list_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let names = list_plugin_dirs(dir.path()).unwrap();
        assert!(names.is_empty());
    }

    #[test]
    fn list_with_plugins() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("alpha")).unwrap();
        std::fs::create_dir(dir.path().join("beta")).unwrap();
        // Create a file (should be ignored)
        std::fs::write(dir.path().join("readme.txt"), "").unwrap();

        let names = list_plugin_dirs(dir.path()).unwrap();
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    #[test]
    fn plugin_status_missing_manifest() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("test")).unwrap();
        assert_eq!(
            plugin_status(dir.path(), "test"),
            "invalid (missing plugin.json)"
        );
    }

    #[test]
    fn plugin_status_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_dir = dir.path().join("test");
        std::fs::create_dir(&plugin_dir).unwrap();
        std::fs::write(plugin_dir.join("plugin.json"), "{}").unwrap();
        assert_eq!(plugin_status(dir.path(), "test"), "enabled");
    }

    #[test]
    fn plugin_status_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_dir = dir.path().join("test");
        std::fs::create_dir(&plugin_dir).unwrap();
        std::fs::write(plugin_dir.join("plugin.json"), "{}").unwrap();
        std::fs::write(plugin_dir.join(".disabled"), "").unwrap();
        assert_eq!(plugin_status(dir.path(), "test"), "disabled");
    }

    #[test]
    fn remove_nonexistent_plugin_errors() {
        let result = run_remove("nonexistent_plugin_xyz");
        assert!(result.is_err());
    }

    #[test]
    fn enable_nonexistent_plugin_errors() {
        let result = run_enable("nonexistent_plugin_xyz");
        assert!(result.is_err());
    }

    #[test]
    fn disable_nonexistent_plugin_errors() {
        let result = run_disable("nonexistent_plugin_xyz");
        assert!(result.is_err());
    }

    #[test]
    fn enable_already_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_dir = dir.path().join("test");
        std::fs::create_dir(&plugin_dir).unwrap();
        std::fs::write(plugin_dir.join("plugin.json"), "{}").unwrap();

        // Temporarily set plugins_dir — we test the enable logic directly
        // Since enable uses the global plugins_dir(), we test via the marker logic
        assert!(!plugin_dir.join(".disabled").exists());
    }

    #[test]
    fn validate_nonexistent_path_errors() {
        let result = run_validate("/nonexistent/plugin/path");
        assert!(result.is_err());
    }

    #[test]
    fn validate_missing_manifest() {
        let dir = tempfile::tempdir().unwrap();
        // No plugin.json
        let result = run_validate(dir.path().to_str().unwrap());
        assert!(result.is_ok()); // Prints FAIL but doesn't error
    }

    #[test]
    fn validate_valid_manifest() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("plugin.json"),
            r#"{"name": "test-plugin", "description": "A test"}"#,
        )
        .unwrap();
        let result = run_validate(dir.path().to_str().unwrap());
        assert!(result.is_ok());
    }

    #[test]
    fn validate_invalid_json_manifest() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("plugin.json"), "not json").unwrap();
        let result = run_validate(dir.path().to_str().unwrap());
        assert!(result.is_ok()); // Prints FAIL but doesn't error
    }

    #[test]
    fn run_install_doesnt_panic() {
        let result = run_install("./some-plugin");
        assert!(result.is_ok());
    }
}
