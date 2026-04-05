//! Tool versioning with semantic version comparison.
//!
//! Provides [`ToolVersion`] for managing tool versions using the standard
//! `MAJOR.MINOR.PATCH` scheme, with ordering and compatibility checks.

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

/// A semantic version for a tool: `MAJOR.MINOR.PATCH`.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct ToolVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl ToolVersion {
    /// Create a new version.
    #[must_use]
    pub fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Check if this version is compatible with `other` (same major, >= minor).
    ///
    /// Two versions are compatible when they share the same major version and
    /// `self` is at least as recent as `other` (i.e. `self >= other` within the
    /// same major line).
    #[must_use]
    pub fn is_compatible_with(&self, other: &Self) -> bool {
        self.major == other.major && (self.minor, self.patch) >= (other.minor, other.patch)
    }

    /// Whether this is a pre-1.0 version.
    #[must_use]
    pub fn is_prerelease(&self) -> bool {
        self.major == 0
    }
}

impl fmt::Display for ToolVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl Ord for ToolVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        self.major
            .cmp(&other.major)
            .then(self.minor.cmp(&other.minor))
            .then(self.patch.cmp(&other.patch))
    }
}

impl PartialOrd for ToolVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Error returned when a version string cannot be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseVersionError(String);

impl fmt::Display for ParseVersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid version string: {}", self.0)
    }
}

impl std::error::Error for ParseVersionError {}

impl FromStr for ToolVersion {
    type Err = ParseVersionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 3 {
            return Err(ParseVersionError(s.to_string()));
        }
        let major = parts[0]
            .parse()
            .map_err(|_| ParseVersionError(s.to_string()))?;
        let minor = parts[1]
            .parse()
            .map_err(|_| ParseVersionError(s.to_string()))?;
        let patch = parts[2]
            .parse()
            .map_err(|_| ParseVersionError(s.to_string()))?;
        Ok(Self::new(major, minor, patch))
    }
}

/// A versioned tool entry associating a tool name with its version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionedTool {
    pub name: String,
    pub version: ToolVersion,
    #[serde(default)]
    pub deprecated: bool,
    /// Optional message explaining deprecation or upgrade path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecation_message: Option<String>,
}

impl VersionedTool {
    /// Create a new versioned tool.
    #[must_use]
    pub fn new(name: impl Into<String>, version: ToolVersion) -> Self {
        Self {
            name: name.into(),
            version,
            deprecated: false,
            deprecation_message: None,
        }
    }

    /// Mark this tool as deprecated with an optional message.
    #[must_use]
    pub fn deprecated(mut self, message: impl Into<String>) -> Self {
        self.deprecated = true;
        self.deprecation_message = Some(message.into());
        self
    }
}

/// Registry that tracks versions for multiple tools.
#[derive(Debug, Default)]
pub struct ToolVersionRegistry {
    entries: Vec<VersionedTool>,
}

impl ToolVersionRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a versioned tool. If a tool with the same name exists, it is
    /// replaced only if the new version is greater.
    pub fn register(&mut self, entry: VersionedTool) {
        if let Some(existing) = self.entries.iter_mut().find(|e| e.name == entry.name) {
            if entry.version > existing.version {
                *existing = entry;
            }
        } else {
            self.entries.push(entry);
        }
    }

    /// Look up the current version of a tool by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&VersionedTool> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// List all registered versioned tools.
    #[must_use]
    pub fn list(&self) -> &[VersionedTool] {
        &self.entries
    }

    /// Check if `required` version is satisfied by the registered version.
    #[must_use]
    pub fn satisfies(&self, name: &str, required: &ToolVersion) -> bool {
        self.get(name)
            .map_or(false, |e| e.version.is_compatible_with(required))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parse_and_display() {
        let v: ToolVersion = "1.2.3".parse().unwrap();
        assert_eq!(v, ToolVersion::new(1, 2, 3));
        assert_eq!(v.to_string(), "1.2.3");
    }

    #[test]
    fn version_parse_error() {
        assert!("1.2".parse::<ToolVersion>().is_err());
        assert!("abc".parse::<ToolVersion>().is_err());
        assert!("1.2.x".parse::<ToolVersion>().is_err());
    }

    #[test]
    fn version_ordering() {
        let v1 = ToolVersion::new(1, 0, 0);
        let v2 = ToolVersion::new(1, 1, 0);
        let v3 = ToolVersion::new(2, 0, 0);
        let v1a = ToolVersion::new(1, 0, 1);
        assert!(v1 < v2);
        assert!(v2 < v3);
        assert!(v1 < v1a);
        assert!(v1a < v2);
    }

    #[test]
    fn version_compatibility() {
        let v1_2_0 = ToolVersion::new(1, 2, 0);
        let v1_3_0 = ToolVersion::new(1, 3, 0);
        let v2_0_0 = ToolVersion::new(2, 0, 0);
        let v1_1_0 = ToolVersion::new(1, 1, 0);

        // Same major, higher minor → compatible
        assert!(v1_3_0.is_compatible_with(&v1_2_0));
        // Same major, lower minor → not compatible
        assert!(!v1_1_0.is_compatible_with(&v1_2_0));
        // Different major → not compatible
        assert!(!v2_0_0.is_compatible_with(&v1_2_0));
        // Equal → compatible
        assert!(v1_2_0.is_compatible_with(&v1_2_0));
    }

    #[test]
    fn version_is_prerelease() {
        assert!(ToolVersion::new(0, 1, 0).is_prerelease());
        assert!(!ToolVersion::new(1, 0, 0).is_prerelease());
    }

    #[test]
    fn version_serde_roundtrip() {
        let v = ToolVersion::new(3, 14, 159);
        let json = serde_json::to_string(&v).unwrap();
        let back: ToolVersion = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn versioned_tool_deprecated() {
        let t = VersionedTool::new("old_tool", ToolVersion::new(1, 0, 0))
            .deprecated("Use new_tool instead");
        assert!(t.deprecated);
        assert_eq!(t.deprecation_message.as_deref(), Some("Use new_tool instead"));
    }

    #[test]
    fn registry_register_and_get() {
        let mut reg = ToolVersionRegistry::new();
        reg.register(VersionedTool::new("read", ToolVersion::new(1, 0, 0)));
        reg.register(VersionedTool::new("write", ToolVersion::new(2, 1, 0)));
        assert_eq!(reg.list().len(), 2);
        assert_eq!(reg.get("read").unwrap().version, ToolVersion::new(1, 0, 0));
    }

    #[test]
    fn registry_replace_higher_version_only() {
        let mut reg = ToolVersionRegistry::new();
        reg.register(VersionedTool::new("tool", ToolVersion::new(1, 2, 0)));
        // Lower version should not replace
        reg.register(VersionedTool::new("tool", ToolVersion::new(1, 1, 0)));
        assert_eq!(reg.get("tool").unwrap().version, ToolVersion::new(1, 2, 0));
        // Higher version should replace
        reg.register(VersionedTool::new("tool", ToolVersion::new(1, 3, 0)));
        assert_eq!(reg.get("tool").unwrap().version, ToolVersion::new(1, 3, 0));
    }

    #[test]
    fn registry_satisfies() {
        let mut reg = ToolVersionRegistry::new();
        reg.register(VersionedTool::new("tool", ToolVersion::new(1, 5, 0)));
        assert!(reg.satisfies("tool", &ToolVersion::new(1, 3, 0)));
        assert!(reg.satisfies("tool", &ToolVersion::new(1, 5, 0)));
        assert!(!reg.satisfies("tool", &ToolVersion::new(1, 6, 0)));
        assert!(!reg.satisfies("tool", &ToolVersion::new(2, 0, 0)));
        assert!(!reg.satisfies("missing", &ToolVersion::new(1, 0, 0)));
    }

    #[test]
    fn versioned_tool_serde_roundtrip() {
        let t = VersionedTool::new("test", ToolVersion::new(1, 0, 0))
            .deprecated("obsolete");
        let json = serde_json::to_string(&t).unwrap();
        let back: VersionedTool = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "test");
        assert!(back.deprecated);
        assert_eq!(back.deprecation_message.as_deref(), Some("obsolete"));
    }
}
