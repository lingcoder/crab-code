//! CC-format `plugin.json` manifest.
//!
//! The manifest is optional metadata; a directory is a plugin as soon as it
//! carries any component directory (`commands/`, `agents/`, `skills/`,
//! `hooks/`, `.mcp.json`).

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Plugin author information.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginAuthor {
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// CC `plugin.json` manifest. Unknown keys are ignored.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Plugin identifier (kebab-case by convention).
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<PluginAuthor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
}

/// Load `plugin.json` from a plugin root.
///
/// Returns `None` when the file is absent (a manifest is optional).
///
/// # Errors
///
/// Returns an error when the file exists but is not valid JSON.
pub fn load_manifest(plugin_root: &Path) -> crab_core::Result<Option<PluginManifest>> {
    let path = plugin_root.join("plugin.json");
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(crab_core::Error::Other(format!(
                "failed to read '{}': {e}",
                path.display()
            )));
        }
    };
    let manifest: PluginManifest = serde_json::from_str(&content).map_err(|e| {
        crab_core::Error::Other(format!("invalid plugin.json '{}': {e}", path.display()))
    })?;
    Ok(Some(manifest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_parses_cc_fields() {
        let json = r#"{
            "name": "my-plugin",
            "version": "1.2.0",
            "description": "does things",
            "author": {"name": "Dev", "email": "d@example.com"},
            "keywords": ["ci", "review"],
            "unknownField": {"ignored": true}
        }"#;
        let m: PluginManifest = serde_json::from_str(json).unwrap();
        assert_eq!(m.name, "my-plugin");
        assert_eq!(m.version.as_deref(), Some("1.2.0"));
        assert_eq!(m.author.unwrap().name, "Dev");
        assert_eq!(m.keywords, vec!["ci", "review"]);
    }

    #[test]
    fn load_manifest_absent_is_none() {
        let dir = std::env::temp_dir().join("crab_plugin_manifest_none");
        let _ = std::fs::create_dir_all(&dir);
        assert!(load_manifest(&dir).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_manifest_invalid_errors() {
        let dir = std::env::temp_dir().join("crab_plugin_manifest_bad");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("plugin.json"), "not json").unwrap();
        assert!(load_manifest(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
