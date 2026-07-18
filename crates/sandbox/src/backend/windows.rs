//! Windows backend — empty placeholder.
//!
//! Intended implementation (deferred): restricted token /
//! `AppContainer` for filesystem and network confinement. A Job
//! Object alone only bounds process lifetime and resources, not
//! filesystem access, so it is not a sandbox.

use crate::policy::SandboxPolicy;
use crate::traits::{Sandbox, SandboxBackend, SandboxResult};

/// Windows sandbox (placeholder; enforcement deferred).
pub struct WindowsSandbox;

impl WindowsSandbox {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Default for WindowsSandbox {
    fn default() -> Self {
        Self::new()
    }
}

impl Sandbox for WindowsSandbox {
    fn backend(&self) -> SandboxBackend {
        SandboxBackend::Windows
    }

    fn is_available(&self) -> bool {
        false
    }

    fn apply(
        &self,
        policy: &SandboxPolicy,
        _cmd: &mut tokio::process::Command,
    ) -> crab_core::Result<SandboxResult> {
        tracing::warn!(
            policy = policy.summary(),
            "Windows sandbox is not implemented; policy NOT applied"
        );
        Ok(SandboxResult {
            applied: false,
            description: "Windows sandbox not implemented — policy NOT applied".into(),
            backend: SandboxBackend::Windows,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::PathAccess;

    #[tokio::test]
    async fn placeholder_never_applies() {
        let sandbox = WindowsSandbox::new();
        assert!(!sandbox.is_available());
        let policy = SandboxPolicy::deny_all().with_path("/tmp", PathAccess::ReadOnly);
        let mut cmd = tokio::process::Command::new("echo");
        let result = sandbox.apply(&policy, &mut cmd).unwrap();
        assert!(!result.applied);
        assert_eq!(result.backend, SandboxBackend::Windows);
    }
}
