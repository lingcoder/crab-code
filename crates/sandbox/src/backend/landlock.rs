//! Linux Landlock backend — empty placeholder.
//!
//! Intended implementation (deferred): install the ruleset in the
//! child between `fork` and `exec` via `Command::pre_exec`. Never
//! call `restrict_self()` in the parent — that would sandbox the
//! entire agent process instead of the spawned tool.

use crate::policy::SandboxPolicy;
use crate::traits::{Sandbox, SandboxBackend, SandboxResult};

/// Linux Landlock sandbox (placeholder; enforcement deferred).
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
        false
    }

    fn apply(
        &self,
        policy: &SandboxPolicy,
        _cmd: &mut tokio::process::Command,
    ) -> crab_core::Result<SandboxResult> {
        tracing::warn!(
            policy = policy.summary(),
            "Linux Landlock sandbox is not implemented; policy NOT applied"
        );
        Ok(SandboxResult {
            applied: false,
            description: "Linux Landlock sandbox not implemented — policy NOT applied".into(),
            backend: SandboxBackend::Landlock,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::PathAccess;

    #[tokio::test]
    async fn placeholder_never_applies() {
        let sandbox = LandlockSandbox::new();
        assert!(!sandbox.is_available());
        let policy = SandboxPolicy::deny_all().with_path("/tmp", PathAccess::ReadOnly);
        let mut cmd = tokio::process::Command::new("echo");
        let result = sandbox.apply(&policy, &mut cmd).unwrap();
        assert!(!result.applied);
        assert_eq!(result.backend, SandboxBackend::Landlock);
    }
}
