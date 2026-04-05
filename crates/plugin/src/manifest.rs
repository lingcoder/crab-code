//! Plugin manifest parsing and validation.
//!
//! A plugin manifest (`plugin.json`) describes a plugin's metadata,
//! capabilities, and entry points. This module handles loading and
//! validating these manifests.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Supported plugin types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginKind {
    /// A skill-based plugin (markdown prompt templates).
    Skill,
    /// A WASM-based plugin (sandboxed binary).
    Wasm,
}

/// Plugin manifest loaded from `plugin.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Unique plugin identifier (e.g. "my-plugin").
    pub name: String,
    /// Human-readable description.
    #[serde(default)]
    pub description: String,
    /// Semantic version string.
    #[serde(default = "default_version")]
    pub version: String,
    /// Plugin type.
    #[serde(default = "default_kind")]
    pub kind: PluginKind,
    /// Author name or identifier.
    #[serde(default)]
    pub author: String,
    /// Entry point relative to the plugin directory.
    /// For skill plugins: a `.md` file or directory of `.md` files.
    /// For WASM plugins: a `.wasm` file.
    #[serde(default)]
    pub entry: String,
    /// Required permissions for this plugin.
    #[serde(default)]
    pub permissions: Vec<String>,
    /// Source directory this manifest was loaded from.
    #[serde(skip)]
    pub source_dir: Option<PathBuf>,
}

fn default_version() -> String {
    "0.1.0".to_string()
}

fn default_kind() -> PluginKind {
    PluginKind::Skill
}

impl PluginManifest {
    /// Validate the manifest for required fields and consistency.
    pub fn validate(&self) -> crab_common::Result<()> {
        if self.name.is_empty() {
            return Err(crab_common::Error::Other(
                "plugin manifest: 'name' is required".into(),
            ));
        }
        if self.name.contains(char::is_whitespace) {
            return Err(crab_common::Error::Other(
                "plugin manifest: 'name' must not contain whitespace".into(),
            ));
        }
        Ok(())
    }

    /// Resolve the entry point to an absolute path relative to `source_dir`.
    #[must_use]
    pub fn resolved_entry(&self) -> Option<PathBuf> {
        self.source_dir.as_ref().map(|dir| dir.join(&self.entry))
    }
}

/// Load a plugin manifest from a `plugin.json` file.
pub fn load_manifest(path: &Path) -> crab_common::Result<PluginManifest> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| crab_common::Error::Other(format!("failed to read plugin manifest: {e}")))?;

    let mut manifest: PluginManifest = serde_json::from_str(&content)
        .map_err(|e| crab_common::Error::Other(format!("invalid plugin manifest JSON: {e}")))?;

    manifest.source_dir = path.parent().map(Path::to_path_buf);
    manifest.validate()?;

    Ok(manifest)
}

/// Discover plugin manifests in a directory.
///
/// Scans for subdirectories containing `plugin.json` files.
pub fn discover_plugins(dir: &Path) -> Vec<PluginManifest> {
    let mut manifests = Vec::new();

    if !dir.exists() {
        return manifests;
    }

    let Ok(entries) = std::fs::read_dir(dir) else {
        return manifests;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let manifest_path = path.join("plugin.json");
            if manifest_path.exists() {
                match load_manifest(&manifest_path) {
                    Ok(manifest) => {
                        tracing::debug!(
                            name = manifest.name.as_str(),
                            path = %manifest_path.display(),
                            "loaded plugin manifest"
                        );
                        manifests.push(manifest);
                    }
                    Err(e) => {
                        tracing::warn!(
                            path = %manifest_path.display(),
                            error = %e,
                            "failed to load plugin manifest"
                        );
                    }
                }
            }
        }
    }

    manifests
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_kind_serde() {
        let skill = serde_json::to_string(&PluginKind::Skill).unwrap();
        assert_eq!(skill, "\"skill\"");
        let wasm = serde_json::to_string(&PluginKind::Wasm).unwrap();
        assert_eq!(wasm, "\"wasm\"");
        let parsed: PluginKind = serde_json::from_str(&skill).unwrap();
        assert_eq!(parsed, PluginKind::Skill);
    }

    #[test]
    fn manifest_deserialize_minimal() {
        let json = r#"{"name": "test-plugin"}"#;
        let m: PluginManifest = serde_json::from_str(json).unwrap();
        assert_eq!(m.name, "test-plugin");
        assert_eq!(m.version, "0.1.0");
        assert_eq!(m.kind, PluginKind::Skill);
        assert!(m.permissions.is_empty());
    }

    #[test]
    fn manifest_deserialize_full() {
        let json = r#"{
            "name": "my-plugin",
            "description": "A test plugin",
            "version": "1.2.3",
            "kind": "wasm",
            "author": "dev",
            "entry": "plugin.wasm",
            "permissions": ["fs:read", "net:http"]
        }"#;
        let m: PluginManifest = serde_json::from_str(json).unwrap();
        assert_eq!(m.name, "my-plugin");
        assert_eq!(m.description, "A test plugin");
        assert_eq!(m.version, "1.2.3");
        assert_eq!(m.kind, PluginKind::Wasm);
        assert_eq!(m.author, "dev");
        assert_eq!(m.entry, "plugin.wasm");
        assert_eq!(m.permissions.len(), 2);
    }

    #[test]
    fn validate_empty_name_fails() {
        let m = PluginManifest {
            name: String::new(),
            description: String::new(),
            version: "0.1.0".into(),
            kind: PluginKind::Skill,
            author: String::new(),
            entry: String::new(),
            permissions: vec![],
            source_dir: None,
        };
        assert!(m.validate().is_err());
    }

    #[test]
    fn validate_whitespace_name_fails() {
        let m = PluginManifest {
            name: "bad name".into(),
            description: String::new(),
            version: "0.1.0".into(),
            kind: PluginKind::Skill,
            author: String::new(),
            entry: String::new(),
            permissions: vec![],
            source_dir: None,
        };
        assert!(m.validate().is_err());
    }

    #[test]
    fn validate_good_name_ok() {
        let m = PluginManifest {
            name: "good-plugin".into(),
            description: String::new(),
            version: "0.1.0".into(),
            kind: PluginKind::Skill,
            author: String::new(),
            entry: String::new(),
            permissions: vec![],
            source_dir: None,
        };
        assert!(m.validate().is_ok());
    }

    #[test]
    fn resolved_entry_with_source_dir() {
        let m = PluginManifest {
            name: "test".into(),
            description: String::new(),
            version: "0.1.0".into(),
            kind: PluginKind::Wasm,
            author: String::new(),
            entry: "plugin.wasm".into(),
            permissions: vec![],
            source_dir: Some(PathBuf::from("/plugins/test")),
        };
        let resolved = m.resolved_entry().unwrap();
        assert_eq!(resolved, PathBuf::from("/plugins/test/plugin.wasm"));
    }

    #[test]
    fn resolved_entry_without_source_dir() {
        let m = PluginManifest {
            name: "test".into(),
            description: String::new(),
            version: "0.1.0".into(),
            kind: PluginKind::Skill,
            author: String::new(),
            entry: String::new(),
            permissions: vec![],
            source_dir: None,
        };
        assert!(m.resolved_entry().is_none());
    }

    #[test]
    fn discover_empty_dir() {
        let tmp = std::env::temp_dir().join("crab_plugin_test_empty");
        let _ = std::fs::create_dir_all(&tmp);
        let manifests = discover_plugins(&tmp);
        assert!(manifests.is_empty());
        let _ = std::fs::remove_dir(&tmp);
    }

    #[test]
    fn discover_nonexistent_dir() {
        let manifests = discover_plugins(Path::new("/nonexistent/path/plugins"));
        assert!(manifests.is_empty());
    }

    #[test]
    fn load_and_discover_roundtrip() {
        let tmp = std::env::temp_dir().join("crab_plugin_test_discover");
        let plugin_dir = tmp.join("my-plugin");
        let _ = std::fs::create_dir_all(&plugin_dir);

        let manifest_json = r#"{"name": "my-plugin", "description": "test"}"#;
        std::fs::write(plugin_dir.join("plugin.json"), manifest_json).unwrap();

        let manifests = discover_plugins(&tmp);
        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0].name, "my-plugin");

        // Cleanup
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
