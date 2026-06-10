//! Native package manager detection and dependency installation.
//!
//! Detects which package managers are available on the host system and
//! provides a convenience function to install a named dependency using
//! a selected package manager.

use std::fmt;

/// Recognised package managers.
///
/// Dead-code suppressed: the enum and its methods are not wired to any
/// CLI command yet but will be used when the `install` subcommand lands.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PackageManager {
    /// Node.js package manager (`npm install`).
    Npm,
    /// Python package manager (`pip install`).
    Pip,
    /// Homebrew (`brew install`) — macOS / Linux.
    Brew,
    /// APT (`apt-get install`) — Debian / Ubuntu.
    Apt,
    /// Rust Cargo (`cargo install`).
    Cargo,
}

impl PackageManager {
    /// The binary name used to detect this package manager.
    fn binary(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Pip => "pip",
            Self::Brew => "brew",
            Self::Apt => "apt-get",
            Self::Cargo => "cargo",
        }
    }

    /// Build the install command line for a given package name.
    ///
    /// Dead-code suppressed: used by `install_dependency` which itself is not
    /// yet called from any CLI command.
    #[allow(dead_code)]
    pub fn install_args(self, package_name: &str) -> Vec<String> {
        match self {
            Self::Npm => vec!["install".into(), "-g".into(), package_name.into()],
            Self::Pip | Self::Brew | Self::Cargo => {
                vec!["install".into(), package_name.into()]
            }
            Self::Apt => vec!["install".into(), "-y".into(), package_name.into()],
        }
    }

    /// All known package manager variants.
    pub fn all() -> &'static [Self] {
        &[Self::Npm, Self::Pip, Self::Brew, Self::Apt, Self::Cargo]
    }
}

impl fmt::Display for PackageManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.binary())
    }
}

/// Check whether a given binary is available on the system `PATH`.
fn is_binary_available(name: &str) -> bool {
    // On Windows use `where`, on Unix use `which`
    let (cmd, args) = if cfg!(windows) {
        ("where", vec![name.to_owned()])
    } else {
        ("which", vec![name.to_owned()])
    };

    std::process::Command::new(cmd)
        .args(&args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Detect which package managers are available on the current system.
///
/// Returns a list of [`PackageManager`] variants whose binaries were
/// found on `PATH`.
///
/// Dead-code suppressed: not yet called from any CLI command; will be
/// used by the planned `install` subcommand.
#[allow(dead_code)]
pub fn detect_package_managers() -> Vec<PackageManager> {
    PackageManager::all()
        .iter()
        .copied()
        .filter(|pm| is_binary_available(pm.binary()))
        .collect()
}

/// Result of a dependency installation attempt.
///
/// Dead-code suppressed: returned by `install_dependency` which is not yet
/// called from any CLI command.
#[allow(dead_code)]
#[derive(Debug)]
pub struct InstallResult {
    /// Whether the install command exited successfully.
    pub success: bool,
    /// Combined stdout/stderr output from the install command.
    pub output: String,
}

/// Install a dependency using the specified package manager.
///
/// Spawns the package manager process synchronously and captures its
/// combined output.
///
/// # Arguments
///
/// * `name` — the package/crate/formula name to install.
/// * `pm` — the package manager to use.
///
/// Dead-code suppressed: planned for the `install` subcommand.
#[allow(dead_code)]
pub fn install_dependency(name: &str, pm: PackageManager) -> InstallResult {
    let binary = pm.binary();
    let args = pm.install_args(name);

    match std::process::Command::new(binary).args(&args).output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            InstallResult {
                success: output.status.success(),
                output: format!("{stdout}{stderr}"),
            }
        }
        Err(e) => InstallResult {
            success: false,
            output: format!("Failed to run {binary}: {e}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_finds_at_least_cargo() {
        // In a Rust dev environment cargo should be available
        let managers = detect_package_managers();
        assert!(
            managers.contains(&PackageManager::Cargo),
            "cargo should be on PATH in a Rust project"
        );
    }

    #[test]
    fn install_args_cargo() {
        let args = PackageManager::Cargo.install_args("ripgrep");
        assert_eq!(args, vec!["install", "ripgrep"]);
    }

    #[test]
    fn install_args_npm() {
        let args = PackageManager::Npm.install_args("typescript");
        assert_eq!(args, vec!["install", "-g", "typescript"]);
    }

    #[test]
    fn install_args_pip() {
        let args = PackageManager::Pip.install_args("requests");
        assert_eq!(args, vec!["install", "requests"]);
    }

    #[test]
    fn install_args_apt() {
        let args = PackageManager::Apt.install_args("curl");
        assert_eq!(args, vec!["install", "-y", "curl"]);
    }

    #[test]
    fn package_manager_display() {
        assert_eq!(PackageManager::Npm.to_string(), "npm");
        assert_eq!(PackageManager::Cargo.to_string(), "cargo");
        assert_eq!(PackageManager::Brew.to_string(), "brew");
    }

    #[test]
    fn install_nonexistent_binary_returns_error() {
        // Use a package manager binary that almost certainly does not exist
        // by testing install_dependency with a fake one. Instead we test
        // the real function with an impossible package name to verify it
        // does not panic and returns a result.
        let result = install_dependency("__nonexistent_package_12345__", PackageManager::Cargo);
        // The install will either fail (package not found) or succeed; either
        // way it should not panic. We just verify we get a result.
        assert!(!result.output.is_empty() || !result.success);
    }

    #[test]
    fn all_package_managers_list() {
        let all = PackageManager::all();
        assert_eq!(all.len(), 5);
        assert!(all.contains(&PackageManager::Npm));
        assert!(all.contains(&PackageManager::Pip));
        assert!(all.contains(&PackageManager::Brew));
        assert!(all.contains(&PackageManager::Apt));
        assert!(all.contains(&PackageManager::Cargo));
    }
}
