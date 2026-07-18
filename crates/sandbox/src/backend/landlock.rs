//! Linux Landlock backend — availability probing only.
//!
//! Landlock (kernel 5.13+) restricts filesystem access, but a correct
//! implementation must install the ruleset **in the child** between
//! `fork` and `exec` (`Command::pre_exec`). Calling `restrict_self()`
//! from `apply()` would sandbox the *entire agent process* — every
//! subsequent file operation of the main loop, not just the spawned
//! tool — which is why this backend deliberately does not enforce yet.
//!
//! Until pre-exec enforcement lands (deferred with the rest of the
//! sandbox wiring), `apply` reports `applied: false` so callers and
//! `crab doctor` can tell the truth about the protection level.

use landlock::ABI;

use crate::policy::SandboxPolicy;
use crate::traits::{Sandbox, SandboxBackend, SandboxResult};

/// Linux Landlock sandbox (probe-only; enforcement deferred).
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
        _cmd: &mut tokio::process::Command,
    ) -> crab_core::Result<SandboxResult> {
        let description = match self.abi {
            Some(abi) => {
                tracing::warn!(
                    policy = policy.summary(),
                    "Landlock enforcement is not wired yet (requires pre_exec in the child); \
                     running WITHOUT filesystem restrictions"
                );
                format!(
                    "Landlock available (ABI {}) but enforcement is deferred — policy NOT applied",
                    abi as u32
                )
            }
            None => "Landlock not available on this kernel".to_string(),
        };
        Ok(SandboxResult {
            applied: false,
            description,
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
        && (major > 5 || (major == 5 && minor >= 13))
    {
        return Some(ABI::V3);
    }
    None
}

fn parse_kernel_version(version: &str) -> Option<(u32, u32)> {
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::PathAccess;

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
        if cfg!(target_os = "linux") {
            // May or may not be available depending on kernel version.
            let _ = sandbox.is_available();
        } else {
            assert!(!sandbox.is_available());
        }
    }

    #[tokio::test]
    async fn apply_never_enforces_and_never_restricts_parent() {
        // Enforcement is deferred: apply must report not-applied even when
        // Landlock is available, and must not touch the current process.
        for abi in [None, Some(ABI::V3)] {
            let sandbox = LandlockSandbox { abi };
            let policy = SandboxPolicy::deny_all().with_path("/tmp", PathAccess::ReadOnly);
            let mut cmd = tokio::process::Command::new("echo");
            let result = sandbox.apply(&policy, &mut cmd).unwrap();
            assert!(!result.applied);
            assert_eq!(result.backend, SandboxBackend::Landlock);
        }
        // Prove the parent is unrestricted: writing outside the policy's
        // allowed paths still works.
        let probe = std::env::temp_dir().join("crab_landlock_parent_probe");
        std::fs::write(&probe, "ok").expect("parent process must remain unsandboxed");
        let _ = std::fs::remove_file(&probe);
    }
}
