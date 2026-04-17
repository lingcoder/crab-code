//! Linux Landlock backend.
//!
//! Uses the Landlock LSM (Linux Security Module), available on Linux
//! kernel 5.13+. When Landlock is not available, `apply` returns a
//! non-applied result rather than failing hard — callers decide whether
//! that degradation is acceptable.
//!
//! Enforcement currently validates the policy shape and reports what
//! *would* be enforced; the actual `landlock_*` syscalls require a
//! safe FFI wrapper. Since this workspace forbids `unsafe_code`,
//! hooking those up is deferred to when a safe wrapper crate is
//! adopted (e.g. the upstream `landlock` crate's higher-level API).

use crate::policy::SandboxPolicy;
use crate::traits::{Sandbox, SandboxBackend, SandboxResult};

pub struct LandlockSandbox;

impl LandlockSandbox {
    #[must_use]
    pub fn new() -> Self {
        Self
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
        // Heuristic: Landlock is in-kernel since 5.13. The actual ABI
        // version check needs a syscall; this keeps the check
        // allocation-free.
        let info = sysinfo::System::kernel_version();
        if let Some(version) = info
            && let Some((major, minor)) = parse_kernel_version(&version)
        {
            return major > 5 || (major == 5 && minor >= 13);
        }
        false
    }

    fn apply(
        &self,
        policy: &SandboxPolicy,
        _cmd: &mut tokio::process::Command,
    ) -> crab_common::Result<SandboxResult> {
        if !self.is_available() {
            return Ok(SandboxResult {
                applied: false,
                description: "Landlock not available on this kernel".into(),
                backend: SandboxBackend::Landlock,
            });
        }

        // Real Landlock implementation would use pre_exec to:
        //   1. landlock_create_ruleset — create a ruleset at current ABI
        //   2. landlock_add_rule — per path_rule add the matching access
        //   3. landlock_restrict_self — enforce on the current thread
        //      before exec
        // Gated on a safe wrapper; see the crate-level docs.
        Ok(SandboxResult {
            applied: false,
            description: format!(
                "Landlock policy validated (enforcement pending safe wrapper): {}",
                policy.summary()
            ),
            backend: SandboxBackend::Landlock,
        })
    }
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

    #[test]
    fn parse_kernel_version_valid() {
        assert_eq!(parse_kernel_version("5.15.0-generic"), Some((5, 15)));
        assert_eq!(parse_kernel_version("6.1.0"), Some((6, 1)));
    }

    #[test]
    fn parse_kernel_version_invalid() {
        assert_eq!(parse_kernel_version("invalid"), None);
    }
}
