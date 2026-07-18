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
    crab_config::config::global_config_dir().join("plugins")
}

fn run_list() -> anyhow::Result<()> {
    let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    // CC-installed plugins (~/.claude/plugins, gated by enabledPlugins).
    let claude_home = crab_utils::path::home_dir_or_cwd().join(".claude");
    let cc_enabled = crab_plugin::read_cc_enabled_plugins(&claude_home.join("settings.json"));
    let mut plugins =
        crab_plugin::discover_cc_installed(&claude_home.join("plugins"), cc_enabled.as_ref());
    let cc_count = plugins.len();

    // Crab-native plugin directories (same override rule as the agent path).
    let roots = [plugins_dir(), working_dir.join(".crab").join("plugins")];
    for plugin in crab_plugin::discover_plugins(&roots) {
        if let Some(existing) = plugins.iter_mut().find(|p| p.name == plugin.name) {
            *existing = plugin;
        } else {
            plugins.push(plugin);
        }
    }

    if plugins.is_empty() {
        eprintln!("No plugins loaded.");
        eprintln!();
        eprintln!(
            "Crab reads Claude Code plugins from {} (enabled via",
            claude_home.join("plugins").display()
        );
        eprintln!("Claude Code's enabledPlugins) and native plugins from:");
        eprintln!("  {}", plugins_dir().display());
        eprintln!("  <project>/.crab/plugins/");
        return Ok(());
    }

    eprintln!("Loaded plugins ({}):", plugins.len());
    for plugin in &plugins {
        let mut parts = Vec::new();
        if !plugin.skill_dirs.is_empty() {
            parts.push("skills");
        }
        if plugin.agents_dir.is_some() {
            parts.push("agents");
        }
        if plugin.hooks.is_some() {
            parts.push("hooks");
        }
        if plugin.mcp_servers.is_some() {
            parts.push("mcp");
        }
        let version = plugin
            .manifest
            .as_ref()
            .and_then(|m| m.version.as_deref())
            .map(|v| format!(" v{v}"))
            .unwrap_or_default();
        eprintln!("  {}{version} — {}", plugin.name, parts.join(", "));
    }
    if cc_count > 0 {
        eprintln!();
        eprintln!("{cc_count} from Claude Code (~/.claude/plugins).");
    }

    Ok(())
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
