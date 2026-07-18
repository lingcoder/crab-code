//! macOS Seatbelt backend — empty placeholder.
//!
//! Intended implementation (deferred): generate an SBPL profile
//! from the policy and wrap the invocation with `sandbox-exec -p`.

use crate::policy::SandboxPolicy;
use crate::traits::{Sandbox, SandboxBackend, SandboxResult};

/// macOS Seatbelt sandbox (placeholder; enforcement deferred).
pub struct SeatbeltSandbox;

impl SeatbeltSandbox {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Default for SeatbeltSandbox {
    fn default() -> Self {
        Self::new()
    }
}

impl Sandbox for SeatbeltSandbox {
    fn backend(&self) -> SandboxBackend {
        SandboxBackend::Seatbelt
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
            "macOS Seatbelt sandbox is not implemented; policy NOT applied"
        );
        Ok(SandboxResult {
            applied: false,
            description: "macOS Seatbelt sandbox not implemented — policy NOT applied".into(),
            backend: SandboxBackend::Seatbelt,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::PathAccess;

    #[tokio::test]
    async fn placeholder_never_applies() {
        let sandbox = SeatbeltSandbox::new();
        assert!(!sandbox.is_available());
        let policy = SandboxPolicy::deny_all().with_path("/tmp", PathAccess::ReadOnly);
        let mut cmd = tokio::process::Command::new("echo");
        let result = sandbox.apply(&policy, &mut cmd).unwrap();
        assert!(!result.applied);
        assert_eq!(result.backend, SandboxBackend::Seatbelt);
    }
}
