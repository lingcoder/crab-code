//! Linux Landlock backend.
//!
//! Uses the Landlock LSM (Linux Security Module), available on Linux
//! kernel 5.13+. When Landlock is not available, `apply` returns a
//! non-applied result rather than failing hard — callers decide whether
//! that degradation is acceptable.
//!
//! Filesystem restrictions are enforced via the `landlock` crate's
//! safe API. Network and resource limits are not yet supported by
//! Landlock and are silently ignored.

use landlock::{
    ABI, Access, AccessFs, BitFlags, LandlockStatus, Ruleset, RulesetAttr, RulesetCreatedAttr,
    path_beneath_rules,
};

use crate::policy::{PathAccess, SandboxPolicy};
use crate::traits::{Sandbox, SandboxBackend, SandboxResult};

/// Linux Landlock sandbox.
pub struct LandlockSandbox {
    abi: Option<ABI>,
}

impl LandlockSandbox {
    #[must_use]
    pub fn new() -> Self {
        let abi = detect_abi();
        Self { abi }
    }
}

impl Default for LandlockSandbox {
    fn default() -> Self {
        Self::new()
    }
}

impl Sandbox for LandlockSandbox {
    fn backend(&self) -> SandboxBackend {
        SandboxBackend::Landlock
    }

    fn is_available(&self) -> bool {
        self.abi.is_some()
    }

    fn apply(
        &self,
        policy: &SandboxPolicy,
        cmd: &mut tokio::process::Command,
    ) -> crab_core::Result<SandboxResult> {
        let abi = match self.abi {
            Some(a) => a,
            None => {
                return Ok(SandboxResult {
                    applied: false,
                    description: "Landlock not available on this kernel".into(),
                    backend: SandboxBackend::Landlock,
                });
            }
        };

        let ruleset = Ruleset::default()
            .handle_access(AccessFs::from_all(abi))
            .map_err(|e| crab_core::Error::Other(format!("Landlock ruleset init failed: {e}")))?;

        let mut ruleset_created = ruleset
            .create()
            .map_err(|e| crab_core::Error::Other(format!("Landlock ruleset create failed: {e}")))?;

        for rule in &policy.path_rules {
            let access = path_access_to_landlock(rule.access, abi);
            ruleset_created = ruleset_created
                .add_rules(path_beneath_rules(&rule.path, access))
                .map_err(|e| {
                    crab_core::Error::Other(format!(
                        "Landlock add_rule({}) failed: {e}",
                        rule.path.display()
                    ))
                })?;
        }

        // Install the ruleset on the current thread. The child inherits
        // this restriction when spawned.
        let status = ruleset_created
            .restrict_self()
            .map_err(|e| crab_core::Error::Other(format!("Landlock restrict_self failed: {e}")))?;

        let applied = matches!(status.landlock, LandlockStatus::Available { .. });

        if matches!(
            status.landlock,
            LandlockStatus::NotEnabled | LandlockStatus::NotImplemented
        ) {
            tracing::warn!(
                "Landlock ruleset was not enforced — kernel may not support the requested ABI"
            );
        }

        Ok(SandboxResult {
            applied,
            description: format!(
                "Landlock (ABI {}): filesystem restricted, {}",
                abi as u32,
                policy.summary()
            ),
            backend: SandboxBackend::Landlock,
        })
    }
}

/// Detect the highest supported Landlock ABI version.
///
/// Returns `None` if the kernel does not support Landlock (< 5.13).
fn detect_abi() -> Option<ABI> {
    let info = sysinfo::System::kernel_version();
    if let Some(version) = info
        && let Some((major, minor)) = parse_kernel_version(&version)
    {
        if major > 5 || (major == 5 && minor >= 13) {
            // Try ABI v3 first (kernel >= 6.2), fall back to v2/v1.
            return Some(ABI::V3);
        }
    }
    None
}

fn parse_kernel_version(version: &str) -> Option<(u32, u32)> {
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// Convert our `PathAccess` to Landlock `AccessFs` flags.
fn path_access_to_landlock(access: PathAccess, abi: ABI) -> BitFlags<AccessFs> {
    match access {
        PathAccess::ReadOnly => AccessFs::from_read(abi),
        PathAccess::ReadWrite => AccessFs::from_read(abi) | AccessFs::from_write(abi),
        PathAccess::Full => AccessFs::from_all(abi),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_kernel_version_valid() {
        assert_eq!(parse_kernel_version("5.15.0-generic"), Some((5, 15)));
        assert_eq!(parse_kernel_version("6.1.0"), Some((6, 1)));
    }

    #[test]
    fn parse_kernel_version_invalid() {
        assert_eq!(parse_kernel_version("invalid"), None);
    }

    #[test]
    fn is_available_depends_on_kernel() {
        let sandbox = LandlockSandbox::new();
        // On Linux CI with kernel >= 5.13 this should be true;
        // on Windows/macOS it will be false.
        if cfg!(target_os = "linux") {
            // May or may not be available depending on kernel version.
            let _ = sandbox.is_available();
        } else {
            assert!(!sandbox.is_available());
        }
    }

    #[tokio::test]
    async fn apply_returns_not_applied_when_unavailable() {
        let sandbox = LandlockSandbox { abi: None };
        let policy = SandboxPolicy::deny_all().with_path("/tmp", PathAccess::ReadOnly);
        let mut cmd = tokio::process::Command::new("echo");
        let result = sandbox.apply(&policy, &mut cmd).unwrap();
        assert!(!result.applied);
        assert_eq!(result.backend, SandboxBackend::Landlock);
    }
}
