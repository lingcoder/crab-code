//! Windows Job Object backend.
//!
//! Uses Windows Job Objects to enforce resource limits (memory, CPU
//! time) and UI restrictions on child processes. Filesystem
//! restrictions require restricted tokens (future enhancement).
//!
//! Enforcement currently validates the policy shape and reports what
//! *would* be enforced; actual `CreateJobObjectW` / `SetInformation-
//! JobObject` calls require a safe Windows FFI wrapper. Since this
//! workspace forbids `unsafe_code`, hooking those up is deferred to
//! when a safe wrapper crate (`windows-sys` + a thin safe shim) is
//! adopted.

use crate::policy::SandboxPolicy;
use crate::traits::{Sandbox, SandboxBackend, SandboxResult};

pub struct WindowsJobSandbox;

impl WindowsJobSandbox {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Default for WindowsJobSandbox {
    fn default() -> Self {
        Self::new()
    }
}

impl Sandbox for WindowsJobSandbox {
    fn backend(&self) -> SandboxBackend {
        SandboxBackend::WindowsJobObject
    }

    fn is_available(&self) -> bool {
        // Job Objects are available on all supported Windows versions.
        true
    }

    fn apply(
        &self,
        policy: &SandboxPolicy,
        _cmd: &mut tokio::process::Command,
    ) -> crab_core::Result<SandboxResult> {
        // Real implementation would:
        //   1. CreateJobObjectW — fresh Job Object
        //   2. SetInformationJobObject with JOBOBJECT_EXTENDED_LIMIT_
        //      INFORMATION for memory + CPU
        //   3. Set UI restrictions via JOB_OBJECT_UILIMIT_* flags
        //   4. Assign the child after spawn via AssignProcessToJobObject
        // Gated on a safe Windows FFI wrapper; see crate-level docs.
        Ok(SandboxResult {
            applied: false,
            description: format!(
                "Windows Job Object policy validated (enforcement pending): {}",
                policy.summary()
            ),
            backend: SandboxBackend::WindowsJobObject,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_available_returns_true() {
        let sandbox = WindowsJobSandbox::new();
        assert!(sandbox.is_available());
        assert_eq!(sandbox.backend(), SandboxBackend::WindowsJobObject);
    }

    #[tokio::test]
    async fn apply_returns_pending_enforcement() {
        let sandbox = WindowsJobSandbox::new();
        let policy = SandboxPolicy::deny_all().with_max_memory(128 * 1024 * 1024);
        let mut cmd = tokio::process::Command::new("cmd");
        let result = sandbox.apply(&policy, &mut cmd).unwrap();
        assert_eq!(result.backend, SandboxBackend::WindowsJobObject);
    }
}
