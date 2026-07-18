//! CC-format plugin discovery and component collection.
//!
//! A plugin is a directory in the CC layout:
//!
//! ```text
//! my-plugin/
//! ├── plugin.json      # optional manifest
//! ├── commands/*.md    # slash-command skills
//! ├── agents/*.md      # subagent definitions
//! ├── skills/<n>/SKILL.md
//! ├── hooks/hooks.json # hooks in the settings shape
//! └── .mcp.json        # MCP servers ({"mcpServers": {...}})
//! ```
//!
//! The loader collects component *locations and raw values*; actual
//! registration happens in the composition root (skills feed the skill
//! registry via their directories, hooks and MCP servers merge into the
//! effective settings).

use std::path::{Path, PathBuf};

use crate::manifest::{PluginManifest, load_manifest};

/// A discovered plugin and its components.
#[derive(Debug, Clone, Default)]
pub struct LoadedPlugin {
    /// Plugin name (manifest name, falling back to the directory name).
    pub name: String,
    /// Plugin root directory.
    pub root: PathBuf,
    /// Parsed manifest, when `plugin.json` exists.
    pub manifest: Option<PluginManifest>,
    /// Skill source directories to feed the skill registry
    /// (`skills/` and `commands/`, when present).
    pub skill_dirs: Vec<PathBuf>,
    /// Subagent definition directory (`agents/`), when present.
    pub agents_dir: Option<PathBuf>,
    /// Raw hooks value from `hooks/hooks.json` (CC settings shape).
    pub hooks: Option<serde_json::Value>,
    /// Raw `mcpServers` object from `.mcp.json`.
    pub mcp_servers: Option<serde_json::Value>,
}

impl LoadedPlugin {
    /// Whether the plugin carries any usable component.
    #[must_use]
    pub fn has_components(&self) -> bool {
        !self.skill_dirs.is_empty()
            || self.agents_dir.is_some()
            || self.hooks.is_some()
            || self.mcp_servers.is_some()
    }
}

/// Load a single plugin from its root directory.
///
/// Malformed optional components (`hooks.json`, `.mcp.json`) are skipped
/// with a warning; a malformed `plugin.json` fails the whole plugin.
///
/// # Errors
///
/// Returns an error when `plugin.json` exists but is invalid.
pub fn load_plugin(root: &Path) -> crab_core::Result<LoadedPlugin> {
    let manifest = load_manifest(root)?;
    let name = manifest
        .as_ref()
        .map(|m| m.name.clone())
        .filter(|n| !n.is_empty())
        .or_else(|| root.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default();

    let mut skill_dirs = Vec::new();
    for dir in ["skills", "commands"] {
        let path = root.join(dir);
        if path.is_dir() {
            skill_dirs.push(path);
        }
    }

    let agents_dir = Some(root.join("agents")).filter(|p| p.is_dir());

    let hooks_path = root.join("hooks").join("hooks.json");
    let hooks = match std::fs::read_to_string(&hooks_path) {
        Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
            Ok(value) => {
                let mut value = value.get("hooks").cloned().unwrap_or(value);
                // CC resolves ${CLAUDE_PLUGIN_ROOT} at load time so hook
                // commands can reference files shipped with the plugin.
                substitute_plugin_root(&mut value, root);
                Some(value)
            }
            Err(e) => {
                tracing::warn!(
                    path = %hooks_path.display(),
                    error = %e,
                    "skipping malformed plugin hooks.json"
                );
                None
            }
        },
        Err(_) => None,
    };

    let mcp_path = root.join(".mcp.json");
    let mcp_servers = match std::fs::read_to_string(&mcp_path) {
        Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
            Ok(value) => value.get("mcpServers").cloned(),
            Err(e) => {
                tracing::warn!(
                    path = %mcp_path.display(),
                    error = %e,
                    "skipping malformed plugin .mcp.json"
                );
                None
            }
        },
        Err(_) => None,
    };

    Ok(LoadedPlugin {
        name,
        root: root.to_path_buf(),
        manifest,
        skill_dirs,
        agents_dir,
        hooks,
        mcp_servers,
    })
}

/// Replace `${CLAUDE_PLUGIN_ROOT}` in every string of a hooks value with
/// the plugin root path.
fn substitute_plugin_root(value: &mut serde_json::Value, root: &Path) {
    match value {
        serde_json::Value::String(s) => {
            if s.contains("${CLAUDE_PLUGIN_ROOT}") {
                *s = s.replace("${CLAUDE_PLUGIN_ROOT}", &root.display().to_string());
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                substitute_plugin_root(item, root);
            }
        }
        serde_json::Value::Object(map) => {
            for item in map.values_mut() {
                substitute_plugin_root(item, root);
            }
        }
        _ => {}
    }
}

/// Discover plugins across root directories.
///
/// Each entry of `plugin_roots` may be either a plugin itself or a
/// directory of plugins (one level deep). Invalid plugins are skipped
/// with a warning. Later roots win on duplicate names.
#[must_use]
pub fn discover_plugins(plugin_roots: &[PathBuf]) -> Vec<LoadedPlugin> {
    let mut plugins: Vec<LoadedPlugin> = Vec::new();
    let push = |plugins: &mut Vec<LoadedPlugin>, plugin: LoadedPlugin| {
        if !plugin.has_components() {
            return;
        }
        if let Some(existing) = plugins.iter_mut().find(|p| p.name == plugin.name) {
            *existing = plugin;
        } else {
            plugins.push(plugin);
        }
    };

    for root in plugin_roots {
        if !root.is_dir() {
            continue;
        }
        match load_plugin(root) {
            Ok(plugin) if plugin.has_components() => {
                push(&mut plugins, plugin);
                continue;
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(root = %root.display(), error = %e, "skipping invalid plugin");
                continue;
            }
        }
        // Not a plugin itself: treat as a directory of plugins.
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            match load_plugin(&path) {
                Ok(plugin) => push(&mut plugins, plugin),
                Err(e) => {
                    tracing::warn!(root = %path.display(), error = %e, "skipping invalid plugin");
                }
            }
        }
    }

    plugins
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

    fn fixture_plugin(root: &Path) {
        write(
            &root.join("plugin.json"),
            r#"{"name": "demo", "version": "0.1.0"}"#,
        );
        write(
            &root.join("commands").join("ship.md"),
            "---\ndescription: Ship it\n---\nShip the release.",
        );
        write(
            &root.join("skills").join("review").join("SKILL.md"),
            "---\ndescription: Review\n---\nReview the diff.",
        );
        write(
            &root.join("agents").join("tester.md"),
            "---\ndescription: Tester\n---\nRun tests.",
        );
        write(
            &root.join("hooks").join("hooks.json"),
            r#"{"hooks": {"PreToolUse": [{"hooks": [{"type": "command", "command": "check.sh"}]}]}}"#,
        );
        write(
            &root.join(".mcp.json"),
            r#"{"mcpServers": {"fs": {"command": "fs-mcp"}}}"#,
        );
    }

    #[test]
    fn load_plugin_collects_cc_components() {
        let root = std::env::temp_dir().join("crab_plugin_load_cc");
        let _ = std::fs::remove_dir_all(&root);
        fixture_plugin(&root);

        let plugin = load_plugin(&root).unwrap();
        assert_eq!(plugin.name, "demo");
        assert_eq!(plugin.skill_dirs.len(), 2);
        assert!(plugin.agents_dir.is_some());
        assert!(plugin.hooks.unwrap().get("PreToolUse").is_some());
        assert_eq!(plugin.mcp_servers.unwrap()["fs"]["command"], "fs-mcp");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn plugin_name_falls_back_to_dir() {
        let root = std::env::temp_dir().join("crab_plugin_noname");
        let _ = std::fs::remove_dir_all(&root);
        write(
            &root.join("commands").join("x.md"),
            "---\ndescription: X\n---\nX.",
        );
        let plugin = load_plugin(&root).unwrap();
        assert_eq!(plugin.name, "crab_plugin_noname");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn discover_scans_plugin_directories() {
        let base = std::env::temp_dir().join("crab_plugin_discover");
        let _ = std::fs::remove_dir_all(&base);
        fixture_plugin(&base.join("a"));
        write(
            &base.join("b").join("commands").join("y.md"),
            "---\ndescription: Y\n---\nY.",
        );
        write(&base.join("not-a-plugin").join("readme.txt"), "hi");

        let plugins = discover_plugins(std::slice::from_ref(&base));
        assert_eq!(plugins.len(), 2);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn hooks_substitute_plugin_root() {
        let root = std::env::temp_dir().join("crab_plugin_var_sub");
        let _ = std::fs::remove_dir_all(&root);
        write(
            &root.join("hooks").join("hooks.json"),
            r#"{"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "${CLAUDE_PLUGIN_ROOT}/hooks/run.cmd start"}]}]}}"#,
        );
        let plugin = load_plugin(&root).unwrap();
        let hooks = plugin.hooks.unwrap();
        let command = hooks["SessionStart"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert!(!command.contains("${CLAUDE_PLUGIN_ROOT}"));
        assert!(command.contains("run.cmd start"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn discover_direct_plugin_root() {
        let root = std::env::temp_dir().join("crab_plugin_direct");
        let _ = std::fs::remove_dir_all(&root);
        fixture_plugin(&root);
        let plugins = discover_plugins(std::slice::from_ref(&root));
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "demo");
        let _ = std::fs::remove_dir_all(&root);
    }
}
