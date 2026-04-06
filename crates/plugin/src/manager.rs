//! Plugin lifecycle manager.
//!
//! Discovers, loads, enables/disables plugins from:
//! - `~/.crab/plugins/` (global)
//! - `--plugin-dir` directories (CLI override)
//! - Project-level `.crab/plugins/` (optional)
//!
//! Maintains an enabled/disabled state that persists via the `enabled_plugins`
//! list in settings.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::manifest::{discover_plugins, load_manifest, PluginManifest};

/// Runtime state for a loaded plugin.
#[derive(Debug, Clone)]
pub struct PluginEntry {
    /// Parsed manifest.
    pub manifest: PluginManifest,
    /// Whether this plugin is currently enabled.
    pub enabled: bool,
}

/// Manages plugin discovery, enable/disable, and lifecycle.
#[derive(Debug)]
pub struct PluginManager {
    /// All discovered plugins keyed by name.
    plugins: HashMap<String, PluginEntry>,
    /// Search directories in priority order (last wins on name collision).
    search_dirs: Vec<PathBuf>,
}

impl PluginManager {
    /// Create a new manager with the given search directories.
    ///
    /// Directories are scanned in order; later directories override earlier
    /// ones when plugin names collide.
    pub fn new(search_dirs: Vec<PathBuf>) -> Self {
        Self {
            plugins: HashMap::new(),
            search_dirs,
        }
    }

    /// Create a manager using the default global plugins directory.
    pub fn with_defaults() -> Self {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .map_or_else(|_| PathBuf::from("."), PathBuf::from);
        let global_dir = home.join(".crab").join("plugins");
        Self::new(vec![global_dir])
    }

    /// Add an additional search directory (e.g. from `--plugin-dir`).
    pub fn add_search_dir(&mut self, dir: PathBuf) {
        self.search_dirs.push(dir);
    }

    /// Scan all search directories and populate the plugin registry.
    ///
    /// Plugins discovered in later directories override earlier ones
    /// with the same name.
    pub fn discover(&mut self) {
        self.plugins.clear();
        for dir in &self.search_dirs {
            let manifests = discover_plugins(dir);
            for manifest in manifests {
                let name = manifest.name.clone();
                self.plugins.insert(
                    name,
                    PluginEntry {
                        manifest,
                        enabled: false,
                    },
                );
            }
        }
    }

    /// Apply the enabled list from settings.
    ///
    /// Plugins whose names appear in `enabled_names` are marked enabled;
    /// all others are disabled. An empty list disables everything.
    pub fn apply_enabled_list(&mut self, enabled_names: &[String]) {
        for (name, entry) in &mut self.plugins {
            entry.enabled = enabled_names.iter().any(|e| e == name);
        }
    }

    /// Enable a plugin by name. Returns `false` if not found.
    pub fn enable(&mut self, name: &str) -> bool {
        if let Some(entry) = self.plugins.get_mut(name) {
            entry.enabled = true;
            true
        } else {
            false
        }
    }

    /// Disable a plugin by name. Returns `false` if not found.
    pub fn disable(&mut self, name: &str) -> bool {
        if let Some(entry) = self.plugins.get_mut(name) {
            entry.enabled = false;
            true
        } else {
            false
        }
    }

    /// Get a plugin entry by name.
    pub fn get(&self, name: &str) -> Option<&PluginEntry> {
        self.plugins.get(name)
    }

    /// List all discovered plugins.
    pub fn list(&self) -> Vec<&PluginEntry> {
        let mut entries: Vec<_> = self.plugins.values().collect();
        entries.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
        entries
    }

    /// List only enabled plugins.
    pub fn enabled(&self) -> Vec<&PluginEntry> {
        self.list().into_iter().filter(|e| e.enabled).collect()
    }

    /// Get the names of all enabled plugins (for persisting to settings).
    pub fn enabled_names(&self) -> Vec<String> {
        self.enabled()
            .iter()
            .map(|e| e.manifest.name.clone())
            .collect()
    }

    /// Number of discovered plugins.
    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    /// Whether no plugins are discovered.
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Install a plugin from a directory path.
    ///
    /// Copies the plugin directory into the first search dir (global).
    /// Returns the loaded manifest on success.
    pub fn install_from_path(&mut self, source: &Path) -> crab_common::Result<PluginManifest> {
        let manifest_path = source.join("plugin.json");
        let manifest = load_manifest(&manifest_path)?;

        let target_dir = self
            .search_dirs
            .first()
            .ok_or_else(|| crab_common::Error::Other("no plugin search directory".into()))?
            .join(&manifest.name);

        if target_dir.exists() {
            return Err(crab_common::Error::Other(format!(
                "plugin '{}' already installed at {}",
                manifest.name,
                target_dir.display()
            )));
        }

        copy_dir_recursive(source, &target_dir)?;

        // Re-load from installed location
        let installed_manifest = load_manifest(&target_dir.join("plugin.json"))?;
        let name = installed_manifest.name.clone();
        self.plugins.insert(
            name,
            PluginEntry {
                manifest: installed_manifest.clone(),
                enabled: true,
            },
        );
        Ok(installed_manifest)
    }

    /// Remove a plugin by name. Deletes from disk.
    pub fn remove(&mut self, name: &str) -> crab_common::Result<()> {
        let entry = self
            .plugins
            .remove(name)
            .ok_or_else(|| crab_common::Error::Other(format!("plugin '{name}' not found")))?;

        if let Some(dir) = &entry.manifest.source_dir && dir.exists() {
            std::fs::remove_dir_all(dir).map_err(|e| {
                crab_common::Error::Other(format!(
                    "failed to remove plugin directory {}: {e}",
                    dir.display()
                ))
            })?;
        }
        Ok(())
    }
}

/// Recursively copy a directory.
fn copy_dir_recursive(src: &Path, dst: &Path) -> crab_common::Result<()> {
    std::fs::create_dir_all(dst)
        .map_err(|e| crab_common::Error::Other(format!("mkdir {}: {e}", dst.display())))?;

    for entry in std::fs::read_dir(src)
        .map_err(|e| crab_common::Error::Other(format!("read_dir {}: {e}", src.display())))?
    {
        let entry =
            entry.map_err(|e| crab_common::Error::Other(format!("dir entry: {e}")))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path).map_err(|e| {
                crab_common::Error::Other(format!(
                    "copy {} -> {}: {e}",
                    src_path.display(),
                    dst_path.display()
                ))
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_manager_is_empty() {
        let mgr = PluginManager::new(vec![]);
        assert!(mgr.is_empty());
        assert_eq!(mgr.len(), 0);
        assert!(mgr.list().is_empty());
    }

    #[test]
    fn discover_finds_plugins() {
        let tmp = std::env::temp_dir().join("crab-pm-test-discover");
        let plugin_dir = tmp.join("my-plugin");
        let _ = std::fs::create_dir_all(&plugin_dir);
        std::fs::write(
            plugin_dir.join("plugin.json"),
            r#"{"name": "my-plugin", "description": "test"}"#,
        )
        .unwrap();

        let mut mgr = PluginManager::new(vec![tmp.clone()]);
        mgr.discover();
        assert_eq!(mgr.len(), 1);
        assert_eq!(mgr.list()[0].manifest.name, "my-plugin");
        assert!(!mgr.list()[0].enabled);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn enable_disable_plugin() {
        let tmp = std::env::temp_dir().join("crab-pm-test-enable");
        let plugin_dir = tmp.join("test-plugin");
        let _ = std::fs::create_dir_all(&plugin_dir);
        std::fs::write(
            plugin_dir.join("plugin.json"),
            r#"{"name": "test-plugin"}"#,
        )
        .unwrap();

        let mut mgr = PluginManager::new(vec![tmp.clone()]);
        mgr.discover();

        assert!(!mgr.get("test-plugin").unwrap().enabled);
        assert!(mgr.enable("test-plugin"));
        assert!(mgr.get("test-plugin").unwrap().enabled);
        assert!(mgr.disable("test-plugin"));
        assert!(!mgr.get("test-plugin").unwrap().enabled);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn enable_nonexistent_returns_false() {
        let mgr = PluginManager::new(vec![]);
        assert!(!PluginManager::new(vec![]).enable("nope"));
        assert!(!PluginManager::new(vec![]).disable("nope"));
        drop(mgr);
    }

    #[test]
    fn apply_enabled_list() {
        let tmp = std::env::temp_dir().join("crab-pm-test-apply");
        for name in ["alpha", "beta", "gamma"] {
            let dir = tmp.join(name);
            let _ = std::fs::create_dir_all(&dir);
            std::fs::write(dir.join("plugin.json"), format!(r#"{{"name": "{name}"}}"#)).unwrap();
        }

        let mut mgr = PluginManager::new(vec![tmp.clone()]);
        mgr.discover();
        assert_eq!(mgr.len(), 3);

        mgr.apply_enabled_list(&["alpha".into(), "gamma".into()]);
        assert!(mgr.get("alpha").unwrap().enabled);
        assert!(!mgr.get("beta").unwrap().enabled);
        assert!(mgr.get("gamma").unwrap().enabled);

        let enabled = mgr.enabled_names();
        assert!(enabled.contains(&"alpha".into()));
        assert!(enabled.contains(&"gamma".into()));
        assert!(!enabled.contains(&"beta".into()));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn discover_empty_dir() {
        let tmp = std::env::temp_dir().join("crab-pm-test-empty");
        let _ = std::fs::create_dir_all(&tmp);
        let mut mgr = PluginManager::new(vec![tmp.clone()]);
        mgr.discover();
        assert!(mgr.is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn discover_nonexistent_dir() {
        let mut mgr = PluginManager::new(vec![PathBuf::from("/nonexistent/plugins")]);
        mgr.discover();
        assert!(mgr.is_empty());
    }

    #[test]
    fn later_dir_overrides_earlier() {
        let dir1 = std::env::temp_dir().join("crab-pm-test-override1");
        let dir2 = std::env::temp_dir().join("crab-pm-test-override2");
        for (dir, desc) in [(&dir1, "first"), (&dir2, "second")] {
            let plugin_dir = dir.join("conflict");
            let _ = std::fs::create_dir_all(&plugin_dir);
            std::fs::write(
                plugin_dir.join("plugin.json"),
                format!(r#"{{"name": "conflict", "description": "{desc}"}}"#),
            )
            .unwrap();
        }

        let mut mgr = PluginManager::new(vec![dir1.clone(), dir2.clone()]);
        mgr.discover();
        assert_eq!(mgr.len(), 1);
        assert_eq!(mgr.get("conflict").unwrap().manifest.description, "second");

        let _ = std::fs::remove_dir_all(&dir1);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    #[test]
    fn install_and_remove() {
        let source = std::env::temp_dir().join("crab-pm-test-install-src");
        let target = std::env::temp_dir().join("crab-pm-test-install-dst");
        let _ = std::fs::create_dir_all(&source);
        let _ = std::fs::create_dir_all(&target);

        std::fs::write(
            source.join("plugin.json"),
            r#"{"name": "installed-plugin", "description": "from install"}"#,
        )
        .unwrap();
        std::fs::write(source.join("README.md"), "hello").unwrap();

        let mut mgr = PluginManager::new(vec![target.clone()]);
        let manifest = mgr.install_from_path(&source).unwrap();
        assert_eq!(manifest.name, "installed-plugin");
        assert!(mgr.get("installed-plugin").unwrap().enabled);
        assert!(target.join("installed-plugin").join("plugin.json").exists());
        assert!(target.join("installed-plugin").join("README.md").exists());

        // Double-install should fail
        assert!(mgr.install_from_path(&source).is_err());

        // Remove
        mgr.remove("installed-plugin").unwrap();
        assert!(mgr.get("installed-plugin").is_none());
        assert!(!target.join("installed-plugin").exists());

        let _ = std::fs::remove_dir_all(&source);
        let _ = std::fs::remove_dir_all(&target);
    }

    #[test]
    fn remove_nonexistent_errors() {
        let mut mgr = PluginManager::new(vec![]);
        assert!(mgr.remove("nope").is_err());
    }

    #[test]
    fn add_search_dir() {
        let tmp = std::env::temp_dir().join("crab-pm-test-add-dir");
        let plugin_dir = tmp.join("extra-plugin");
        let _ = std::fs::create_dir_all(&plugin_dir);
        std::fs::write(
            plugin_dir.join("plugin.json"),
            r#"{"name": "extra-plugin"}"#,
        )
        .unwrap();

        let mut mgr = PluginManager::new(vec![]);
        mgr.discover();
        assert!(mgr.is_empty());

        mgr.add_search_dir(tmp.clone());
        mgr.discover();
        assert_eq!(mgr.len(), 1);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn enabled_list_empty_when_none_enabled() {
        let tmp = std::env::temp_dir().join("crab-pm-test-enabled-empty");
        let dir = tmp.join("p");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("plugin.json"), r#"{"name": "p"}"#).unwrap();

        let mut mgr = PluginManager::new(vec![tmp.clone()]);
        mgr.discover();
        assert!(mgr.enabled().is_empty());
        assert!(mgr.enabled_names().is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
