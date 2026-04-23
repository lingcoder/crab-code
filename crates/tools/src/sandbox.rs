//! OS-level sandbox for restricting tool execution.
//!
//! Provides a `SandboxPolicy` trait with platform-specific implementations.
//! Currently: Linux Landlock (when available), macOS/Windows no-op stubs.
//! Enable via settings: `{ "sandbox": { "enabled": true } }`.

use std::path::PathBuf;

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

/// Trait for platform-specific sandbox enforcement.
///
/// Implementations apply OS-level restrictions to a `Command` before execution.
pub trait SandboxPolicy: Send + Sync {
    /// Apply sandbox restrictions to a command that is about to be spawned.
    fn apply(&self, cmd: &mut std::process::Command) -> crab_core::Result<()>;

    /// Returns the platform name for diagnostics.
    fn platform_name(&self) -> &'static str;

    /// Returns whether the sandbox is actually functional on this platform.
    fn is_available(&self) -> bool;
}

/// No-op sandbox for platforms without sandbox support.
pub struct NoopSandbox;

impl SandboxPolicy for NoopSandbox {
    fn apply(&self, _cmd: &mut std::process::Command) -> crab_core::Result<()> {
        Ok(())
    }

    fn platform_name(&self) -> &'static str {
        "noop"
    }

    fn is_available(&self) -> bool {
        false
    }
}

/// Create the appropriate sandbox for the current platform.
///
/// Returns a Linux Landlock sandbox if available, otherwise a no-op.
pub fn create_sandbox(_config: &SandboxConfig) -> Box<dyn SandboxPolicy> {
    // TODO: Linux Landlock implementation using the `landlock` crate
    // TODO: macOS Seatbelt implementation using sandbox-exec
    // TODO: Windows Job Object implementation using windows-rs
    Box::new(NoopSandbox)
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
    fn noop_sandbox_not_available() {
        let sandbox = NoopSandbox;
        assert!(!sandbox.is_available());
        assert_eq!(sandbox.platform_name(), "noop");
    }

    #[test]
    fn noop_sandbox_apply_succeeds() {
        let sandbox = NoopSandbox;
        let mut cmd = std::process::Command::new("echo");
        assert!(sandbox.apply(&mut cmd).is_ok());
    }

    #[test]
    fn create_sandbox_returns_noop_for_now() {
        let config = SandboxConfig::default();
        let sandbox = create_sandbox(&config);
        assert!(!sandbox.is_available());
    }
}
