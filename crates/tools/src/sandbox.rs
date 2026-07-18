//! Re-exports from `crab_sandbox` for backward compatibility.
//!
//! All real sandbox logic lives in the `crab-sandbox` crate. This
//! module provides the legacy `SandboxConfig` type and a
//! `create_sandbox` facade that delegates to `crab_sandbox::create_sandbox`.

use std::path::PathBuf;

use crab_sandbox::Sandbox;

/// Configuration for sandbox restrictions.
#[derive(Debug, Clone, Default)]
pub struct SandboxConfig {
    /// Paths the sandboxed process is allowed to access.
    pub allowed_paths: Vec<PathBuf>,
    /// Whether to deny network access.
    pub deny_network: bool,
    /// Whether the sandbox is enabled at all.
    pub enabled: bool,
}

impl SandboxConfig {
    /// Create a new sandbox config with the given working directory allowed.
    pub fn with_working_dir(working_dir: PathBuf) -> Self {
        Self {
            allowed_paths: vec![working_dir],
            deny_network: false,
            enabled: false,
        }
    }

    /// Check if a path is allowed by this sandbox config.
    pub fn is_path_allowed(&self, path: &std::path::Path) -> bool {
        if !self.enabled {
            return true;
        }
        self.allowed_paths
            .iter()
            .any(|allowed| path.starts_with(allowed))
    }
}

/// Create the appropriate sandbox for the current platform.
///
/// Delegates to `crab_sandbox::create_sandbox()` which selects the
/// best backend: Linux Landlock, Windows Job Object, or Noop on
/// unsupported platforms.
#[must_use]
pub fn create_sandbox(_config: &SandboxConfig) -> Box<dyn Sandbox> {
    crab_sandbox::create_sandbox()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_config_default_disabled() {
        let config = SandboxConfig::default();
        assert!(!config.enabled);
        assert!(config.allowed_paths.is_empty());
        assert!(!config.deny_network);
    }

    #[test]
    fn sandbox_config_with_working_dir() {
        let config = SandboxConfig::with_working_dir(PathBuf::from("/tmp/work"));
        assert!(!config.enabled);
        assert_eq!(config.allowed_paths.len(), 1);
        assert_eq!(config.allowed_paths[0], PathBuf::from("/tmp/work"));
    }

    #[test]
    fn disabled_sandbox_allows_all_paths() {
        let config = SandboxConfig::default();
        assert!(config.is_path_allowed(std::path::Path::new("/etc/passwd")));
        assert!(config.is_path_allowed(std::path::Path::new("/tmp/anything")));
    }

    #[test]
    fn enabled_sandbox_checks_paths() {
        let config = SandboxConfig {
            allowed_paths: vec![PathBuf::from("/tmp/work")],
            deny_network: false,
            enabled: true,
        };
        assert!(config.is_path_allowed(std::path::Path::new("/tmp/work/file.rs")));
        assert!(config.is_path_allowed(std::path::Path::new("/tmp/work")));
        assert!(!config.is_path_allowed(std::path::Path::new("/etc/passwd")));
        assert!(!config.is_path_allowed(std::path::Path::new("/tmp/other")));
    }

    #[test]
    fn create_sandbox_returns_platform_backend() {
        let config = SandboxConfig::default();
        let sandbox = create_sandbox(&config);
        let backend = sandbox.backend();
        assert!(
            backend == crab_sandbox::SandboxBackend::Windows
                || backend == crab_sandbox::SandboxBackend::Landlock
                || backend == crab_sandbox::SandboxBackend::Seatbelt
                || backend == crab_sandbox::SandboxBackend::Noop
        );
    }
}
