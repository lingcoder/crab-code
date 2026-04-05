//! Plugin registry — registration, lifecycle, and per-plugin configuration.
//!
//! This module lives in the config crate so that higher layers (plugin, agent)
//! can read/write plugin state without circular dependencies.  The runtime
//! details (WASM execution, manifest discovery) remain in the plugin crate;
//! this module manages the *persistent configuration* side.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// ── PluginState ─────────────────────────────────────────────────────────

/// Lifecycle state of a plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginState {
    /// Registered but not yet initialised.
    Registered,
    /// Initialised (loaded manifest, validated).
    Initialized,
    /// Running / active.
    Running,
    /// Explicitly stopped by the user.
    Stopped,
    /// Disabled — will not be started automatically.
    Disabled,
}

impl std::fmt::Display for PluginState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Registered => f.write_str("registered"),
            Self::Initialized => f.write_str("initialized"),
            Self::Running => f.write_str("running"),
            Self::Stopped => f.write_str("stopped"),
            Self::Disabled => f.write_str("disabled"),
        }
    }
}

// ── PluginEntry ─────────────────────────────────────────────────────────

/// Config-level metadata for a registered plugin.
///
/// This is the persistent record stored in `~/.crab/plugins/registry.json`.
/// It mirrors core manifest fields so the config layer can work without
/// loading the full plugin runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginEntry {
    /// Unique plugin identifier.
    pub name: String,
    /// Plugin version.
    pub version: String,
    /// Human-readable description.
    #[serde(default)]
    pub description: String,
    /// Author name.
    #[serde(default)]
    pub author: String,
    /// Required permissions (e.g. `["fs:read", "net:http"]`).
    #[serde(default)]
    pub permissions: Vec<String>,
    /// Whether the plugin is enabled.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Current lifecycle state.
    #[serde(default = "default_state")]
    pub state: PluginState,
}

fn default_enabled() -> bool {
    true
}

fn default_state() -> PluginState {
    PluginState::Registered
}

// ── PluginConfig helpers ────────────────────────────────────────────────

/// Return the base directory for all plugin data: `~/.crab/plugins/`.
#[must_use]
pub fn plugins_dir() -> PathBuf {
    crate::settings::global_config_dir().join("plugins")
}

/// Return the per-plugin config directory: `~/.crab/plugins/{name}/`.
#[must_use]
pub fn plugin_config_dir(name: &str) -> PathBuf {
    plugins_dir().join(name)
}

/// Return the per-plugin config file: `~/.crab/plugins/{name}/config.json`.
#[must_use]
pub fn plugin_config_path(name: &str) -> PathBuf {
    plugin_config_dir(name).join("config.json")
}

/// Load per-plugin config as opaque JSON.
/// Returns `Ok(Value::Object({}))` if the file does not exist.
pub fn load_plugin_config(name: &str) -> crab_common::Result<serde_json::Value> {
    let path = plugin_config_path(name);
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).map_err(|e| {
            crab_common::Error::Config(format!(
                "invalid plugin config for '{name}': {e}"
            ))
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(serde_json::Value::Object(serde_json::Map::new()))
        }
        Err(e) => Err(crab_common::Error::Config(format!(
            "cannot read plugin config for '{name}': {e}"
        ))),
    }
}

/// Save per-plugin config from opaque JSON.
pub fn save_plugin_config(name: &str, config: &serde_json::Value) -> crab_common::Result<()> {
    let dir = plugin_config_dir(name);
    std::fs::create_dir_all(&dir).map_err(|e| {
        crab_common::Error::Config(format!(
            "cannot create plugin config dir {}: {e}",
            dir.display()
        ))
    })?;
    let path = plugin_config_path(name);
    let content = serde_json::to_string_pretty(config).map_err(|e| {
        crab_common::Error::Config(format!(
            "cannot serialize plugin config for '{name}': {e}"
        ))
    })?;
    std::fs::write(&path, content).map_err(|e| {
        crab_common::Error::Config(format!(
            "cannot write plugin config to {}: {e}",
            path.display()
        ))
    })
}

// ── PluginRegistry ──────────────────────────────────────────────────────

/// Manages the set of registered plugins and their state.
///
/// Persisted as `~/.crab/plugins/registry.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginRegistry {
    /// Plugins keyed by name. `BTreeMap` for deterministic serialization order.
    #[serde(default)]
    plugins: BTreeMap<String, PluginEntry>,
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            plugins: BTreeMap::new(),
        }
    }

    /// Path to the registry file.
    #[must_use]
    pub fn registry_path() -> PathBuf {
        plugins_dir().join("registry.json")
    }

    /// Load the registry from disk.
    /// Returns an empty registry if the file does not exist.
    pub fn load() -> crab_common::Result<Self> {
        Self::load_from(&Self::registry_path())
    }

    /// Load from a specific path (useful for tests).
    pub fn load_from(path: &Path) -> crab_common::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(content) => serde_json::from_str(&content).map_err(|e| {
                crab_common::Error::Config(format!(
                    "invalid plugin registry: {e}"
                ))
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::new()),
            Err(e) => Err(crab_common::Error::Config(format!(
                "cannot read plugin registry: {e}"
            ))),
        }
    }

    /// Save the registry to disk.
    pub fn save(&self) -> crab_common::Result<()> {
        self.save_to(&Self::registry_path())
    }

    /// Save to a specific path (useful for tests).
    pub fn save_to(&self, path: &Path) -> crab_common::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                crab_common::Error::Config(format!(
                    "cannot create registry dir {}: {e}",
                    parent.display()
                ))
            })?;
        }
        let content = serde_json::to_string_pretty(self).map_err(|e| {
            crab_common::Error::Config(format!(
                "cannot serialize plugin registry: {e}"
            ))
        })?;
        std::fs::write(path, content).map_err(|e| {
            crab_common::Error::Config(format!(
                "cannot write plugin registry to {}: {e}",
                path.display()
            ))
        })
    }

    // ── Registration ────────────────────────────────────────────────────

    /// Register a plugin. Overwrites if already registered.
    pub fn register(&mut self, entry: PluginEntry) {
        self.plugins.insert(entry.name.clone(), entry);
    }

    /// Unregister (remove) a plugin by name. Returns the removed entry.
    pub fn unregister(&mut self, name: &str) -> Option<PluginEntry> {
        self.plugins.remove(name)
    }

    /// Get a plugin entry by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&PluginEntry> {
        self.plugins.get(name)
    }

    /// Get a mutable reference to a plugin entry.
    pub fn get_mut(&mut self, name: &str) -> Option<&mut PluginEntry> {
        self.plugins.get_mut(name)
    }

    /// Check if a plugin is registered.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.plugins.contains_key(name)
    }

    /// List all registered plugins.
    #[must_use]
    pub fn list(&self) -> Vec<&PluginEntry> {
        self.plugins.values().collect()
    }

    /// List only enabled plugins.
    #[must_use]
    pub fn list_enabled(&self) -> Vec<&PluginEntry> {
        self.plugins.values().filter(|e| e.enabled).collect()
    }

    /// Number of registered plugins.
    #[must_use]
    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    /// Whether the registry is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    // ── Enable / Disable ────────────────────────────────────────────────

    /// Enable a plugin. Returns `false` if not found.
    pub fn enable(&mut self, name: &str) -> bool {
        if let Some(entry) = self.plugins.get_mut(name) {
            entry.enabled = true;
            if entry.state == PluginState::Disabled {
                entry.state = PluginState::Registered;
            }
            true
        } else {
            false
        }
    }

    /// Disable a plugin. Returns `false` if not found.
    pub fn disable(&mut self, name: &str) -> bool {
        if let Some(entry) = self.plugins.get_mut(name) {
            entry.enabled = false;
            entry.state = PluginState::Disabled;
            true
        } else {
            false
        }
    }

    // ── Lifecycle transitions ───────────────────────────────────────────

    /// Transition a plugin to a new lifecycle state.
    ///
    /// Validates the transition:
    /// - `Registered` → `Initialized`
    /// - `Initialized` → `Running`
    /// - `Running` → `Stopped`
    /// - `Stopped` → `Running` (restart)
    /// - Any → `Disabled` (via [`disable`])
    ///
    /// Returns `Err` if the transition is invalid or the plugin is not found.
    pub fn transition(
        &mut self,
        name: &str,
        target: PluginState,
    ) -> crab_common::Result<()> {
        let entry = self.plugins.get_mut(name).ok_or_else(|| {
            crab_common::Error::Config(format!("plugin '{name}' not found"))
        })?;

        let valid = matches!(
            (entry.state, target),
            (PluginState::Registered, PluginState::Initialized)
                | (PluginState::Initialized | PluginState::Stopped, PluginState::Running)
                | (PluginState::Running, PluginState::Stopped)
                | (PluginState::Stopped, PluginState::Registered)
        );

        if valid {
            entry.state = target;
            Ok(())
        } else {
            Err(crab_common::Error::Config(format!(
                "invalid plugin state transition: {} → {target} for '{name}'",
                entry.state
            )))
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry(name: &str) -> PluginEntry {
        PluginEntry {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            description: format!("Test plugin {name}"),
            author: "dev".to_string(),
            permissions: vec!["fs:read".to_string()],
            enabled: true,
            state: PluginState::Registered,
        }
    }

    // ── PluginState ─────────────────────────────────────────────────────

    #[test]
    fn plugin_state_serde_roundtrip() {
        for state in [
            PluginState::Registered,
            PluginState::Initialized,
            PluginState::Running,
            PluginState::Stopped,
            PluginState::Disabled,
        ] {
            let json = serde_json::to_string(&state).unwrap();
            let parsed: PluginState = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, state);
        }
    }

    #[test]
    fn plugin_state_display() {
        assert_eq!(PluginState::Registered.to_string(), "registered");
        assert_eq!(PluginState::Initialized.to_string(), "initialized");
        assert_eq!(PluginState::Running.to_string(), "running");
        assert_eq!(PluginState::Stopped.to_string(), "stopped");
        assert_eq!(PluginState::Disabled.to_string(), "disabled");
    }

    // ── PluginEntry ─────────────────────────────────────────────────────

    #[test]
    fn plugin_entry_serde_roundtrip() {
        let entry = sample_entry("test-plugin");
        let json = serde_json::to_string_pretty(&entry).unwrap();
        let parsed: PluginEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "test-plugin");
        assert_eq!(parsed.version, "1.0.0");
        assert!(parsed.enabled);
        assert_eq!(parsed.state, PluginState::Registered);
        assert_eq!(parsed.permissions, vec!["fs:read"]);
    }

    #[test]
    fn plugin_entry_defaults() {
        let json = r#"{"name": "minimal", "version": "0.1.0"}"#;
        let entry: PluginEntry = serde_json::from_str(json).unwrap();
        assert!(entry.enabled); // default true
        assert_eq!(entry.state, PluginState::Registered); // default
        assert!(entry.permissions.is_empty());
        assert!(entry.description.is_empty());
        assert!(entry.author.is_empty());
    }

    // ── PluginRegistry basic ops ────────────────────────────────────────

    #[test]
    fn registry_new_is_empty() {
        let reg = PluginRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        assert!(reg.list().is_empty());
    }

    #[test]
    fn registry_register_and_get() {
        let mut reg = PluginRegistry::new();
        reg.register(sample_entry("alpha"));
        assert_eq!(reg.len(), 1);
        assert!(reg.contains("alpha"));
        assert!(!reg.contains("beta"));

        let entry = reg.get("alpha").unwrap();
        assert_eq!(entry.version, "1.0.0");
    }

    #[test]
    fn registry_register_overwrites() {
        let mut reg = PluginRegistry::new();
        reg.register(sample_entry("alpha"));

        let mut updated = sample_entry("alpha");
        updated.version = "2.0.0".to_string();
        reg.register(updated);

        assert_eq!(reg.len(), 1);
        assert_eq!(reg.get("alpha").unwrap().version, "2.0.0");
    }

    #[test]
    fn registry_unregister() {
        let mut reg = PluginRegistry::new();
        reg.register(sample_entry("alpha"));
        reg.register(sample_entry("beta"));

        let removed = reg.unregister("alpha");
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().name, "alpha");
        assert_eq!(reg.len(), 1);
        assert!(!reg.contains("alpha"));
    }

    #[test]
    fn registry_unregister_nonexistent() {
        let mut reg = PluginRegistry::new();
        assert!(reg.unregister("ghost").is_none());
    }

    #[test]
    fn registry_list_all() {
        let mut reg = PluginRegistry::new();
        reg.register(sample_entry("alpha"));
        reg.register(sample_entry("beta"));
        reg.register(sample_entry("gamma"));

        let all = reg.list();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn registry_list_enabled() {
        let mut reg = PluginRegistry::new();
        reg.register(sample_entry("alpha"));
        reg.register(sample_entry("beta"));
        reg.disable("beta");

        let enabled = reg.list_enabled();
        assert_eq!(enabled.len(), 1);
        assert_eq!(enabled[0].name, "alpha");
    }

    // ── Enable / Disable ────────────────────────────────────────────────

    #[test]
    fn registry_disable_and_enable() {
        let mut reg = PluginRegistry::new();
        reg.register(sample_entry("alpha"));

        assert!(reg.disable("alpha"));
        let entry = reg.get("alpha").unwrap();
        assert!(!entry.enabled);
        assert_eq!(entry.state, PluginState::Disabled);

        assert!(reg.enable("alpha"));
        let entry = reg.get("alpha").unwrap();
        assert!(entry.enabled);
        assert_eq!(entry.state, PluginState::Registered);
    }

    #[test]
    fn registry_enable_nonexistent_returns_false() {
        let mut reg = PluginRegistry::new();
        assert!(!reg.enable("ghost"));
        assert!(!reg.disable("ghost"));
    }

    // ── Lifecycle transitions ───────────────────────────────────────────

    #[test]
    fn lifecycle_registered_to_initialized() {
        let mut reg = PluginRegistry::new();
        reg.register(sample_entry("alpha"));
        assert!(reg.transition("alpha", PluginState::Initialized).is_ok());
        assert_eq!(reg.get("alpha").unwrap().state, PluginState::Initialized);
    }

    #[test]
    fn lifecycle_initialized_to_running() {
        let mut reg = PluginRegistry::new();
        reg.register(sample_entry("alpha"));
        reg.transition("alpha", PluginState::Initialized).unwrap();
        assert!(reg.transition("alpha", PluginState::Running).is_ok());
        assert_eq!(reg.get("alpha").unwrap().state, PluginState::Running);
    }

    #[test]
    fn lifecycle_running_to_stopped() {
        let mut reg = PluginRegistry::new();
        reg.register(sample_entry("alpha"));
        reg.transition("alpha", PluginState::Initialized).unwrap();
        reg.transition("alpha", PluginState::Running).unwrap();
        assert!(reg.transition("alpha", PluginState::Stopped).is_ok());
        assert_eq!(reg.get("alpha").unwrap().state, PluginState::Stopped);
    }

    #[test]
    fn lifecycle_stopped_to_running_restart() {
        let mut reg = PluginRegistry::new();
        reg.register(sample_entry("alpha"));
        reg.transition("alpha", PluginState::Initialized).unwrap();
        reg.transition("alpha", PluginState::Running).unwrap();
        reg.transition("alpha", PluginState::Stopped).unwrap();
        assert!(reg.transition("alpha", PluginState::Running).is_ok());
        assert_eq!(reg.get("alpha").unwrap().state, PluginState::Running);
    }

    #[test]
    fn lifecycle_stopped_to_registered_reinit() {
        let mut reg = PluginRegistry::new();
        reg.register(sample_entry("alpha"));
        reg.transition("alpha", PluginState::Initialized).unwrap();
        reg.transition("alpha", PluginState::Running).unwrap();
        reg.transition("alpha", PluginState::Stopped).unwrap();
        assert!(reg.transition("alpha", PluginState::Registered).is_ok());
    }

    #[test]
    fn lifecycle_invalid_transition() {
        let mut reg = PluginRegistry::new();
        reg.register(sample_entry("alpha"));
        // Registered → Running (skipping Initialized) should fail
        let result = reg.transition("alpha", PluginState::Running);
        assert!(result.is_err());
    }

    #[test]
    fn lifecycle_nonexistent_plugin() {
        let mut reg = PluginRegistry::new();
        let result = reg.transition("ghost", PluginState::Initialized);
        assert!(result.is_err());
    }

    // ── Persistence ─────────────────────────────────────────────────────

    #[test]
    fn registry_save_and_load() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("registry.json");

        let mut reg = PluginRegistry::new();
        reg.register(sample_entry("alpha"));
        reg.register(sample_entry("beta"));
        reg.disable("beta");
        reg.save_to(&path).unwrap();

        let loaded = PluginRegistry::load_from(&path).unwrap();
        assert_eq!(loaded.len(), 2);
        assert!(loaded.get("alpha").unwrap().enabled);
        assert!(!loaded.get("beta").unwrap().enabled);
        assert_eq!(loaded.get("beta").unwrap().state, PluginState::Disabled);
    }

    #[test]
    fn registry_load_nonexistent_returns_empty() {
        let loaded = PluginRegistry::load_from(Path::new("/nonexistent/registry.json")).unwrap();
        assert!(loaded.is_empty());
    }

    #[test]
    fn registry_load_invalid_json() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("registry.json");
        std::fs::write(&path, "not json").unwrap();
        assert!(PluginRegistry::load_from(&path).is_err());
    }

    #[test]
    fn registry_save_creates_parent_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("deep").join("nested").join("registry.json");
        let reg = PluginRegistry::new();
        assert!(reg.save_to(&path).is_ok());
        assert!(path.exists());
    }

    #[test]
    fn registry_serde_roundtrip() {
        let mut reg = PluginRegistry::new();
        reg.register(sample_entry("alpha"));
        reg.register(sample_entry("beta"));
        reg.transition("alpha", PluginState::Initialized).unwrap();

        let json = serde_json::to_string(&reg).unwrap();
        let parsed: PluginRegistry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(
            parsed.get("alpha").unwrap().state,
            PluginState::Initialized
        );
    }

    // ── Plugin config helpers ───────────────────────────────────────────

    #[test]
    fn plugin_config_path_structure() {
        let path = plugin_config_path("my-plugin");
        let path_str = path.to_string_lossy();
        assert!(path_str.contains("plugins"));
        assert!(path_str.contains("my-plugin"));
        assert!(path_str.ends_with("config.json"));
    }

    #[test]
    fn save_and_load_plugin_config() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("plugins").join("test-plugin");
        std::fs::create_dir_all(&dir).unwrap();
        let config_path = dir.join("config.json");

        let config = serde_json::json!({"logLevel": "debug", "maxRetries": 3});
        let content = serde_json::to_string_pretty(&config).unwrap();
        std::fs::write(&config_path, content).unwrap();

        // Read back
        let loaded: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
        assert_eq!(loaded["logLevel"], "debug");
        assert_eq!(loaded["maxRetries"], 3);
    }

    #[test]
    fn get_mut_modifies_entry() {
        let mut reg = PluginRegistry::new();
        reg.register(sample_entry("alpha"));
        if let Some(entry) = reg.get_mut("alpha") {
            entry.version = "9.9.9".to_string();
        }
        assert_eq!(reg.get("alpha").unwrap().version, "9.9.9");
    }

    #[test]
    fn default_registry_is_empty() {
        let reg = PluginRegistry::default();
        assert!(reg.is_empty());
    }

    #[test]
    fn list_order_is_deterministic() {
        let mut reg = PluginRegistry::new();
        reg.register(sample_entry("gamma"));
        reg.register(sample_entry("alpha"));
        reg.register(sample_entry("beta"));

        let names: Vec<&str> = reg.list().iter().map(|e| e.name.as_str()).collect();
        // BTreeMap keeps sorted order
        assert_eq!(names, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn enable_from_non_disabled_state() {
        let mut reg = PluginRegistry::new();
        reg.register(sample_entry("alpha"));
        reg.transition("alpha", PluginState::Initialized).unwrap();
        // Enable a plugin that's not disabled — should keep its state
        assert!(reg.enable("alpha"));
        assert_eq!(reg.get("alpha").unwrap().state, PluginState::Initialized);
    }

    #[test]
    fn full_lifecycle_flow() {
        let mut reg = PluginRegistry::new();
        reg.register(sample_entry("my-tool"));

        // init → start → stop → restart → stop → re-init
        reg.transition("my-tool", PluginState::Initialized).unwrap();
        reg.transition("my-tool", PluginState::Running).unwrap();
        reg.transition("my-tool", PluginState::Stopped).unwrap();
        reg.transition("my-tool", PluginState::Running).unwrap();
        reg.transition("my-tool", PluginState::Stopped).unwrap();
        reg.transition("my-tool", PluginState::Registered).unwrap();

        assert_eq!(reg.get("my-tool").unwrap().state, PluginState::Registered);
    }
}
