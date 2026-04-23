//! No-op backend — always allows. Used as dev fallback and when the
//! platform lacks a supported sandbox primitive.

use crate::policy::SandboxPolicy;
use crate::traits::{Sandbox, SandboxBackend, SandboxResult};

/// No-op sandbox. Records the policy in the result description but
/// performs no enforcement.
pub struct NoopSandbox;

impl NoopSandbox {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Default for NoopSandbox {
    fn default() -> Self {
        Self::new()
    }
}

impl Sandbox for NoopSandbox {
    fn backend(&self) -> SandboxBackend {
        SandboxBackend::Noop
    }

    fn is_available(&self) -> bool {
        true // always "available" — just doesn't enforce anything
    }

    fn apply(
        &self,
        policy: &SandboxPolicy,
        _cmd: &mut tokio::process::Command,
    ) -> crab_core::Result<SandboxResult> {
        Ok(SandboxResult {
            applied: false,
            description: format!(
                "No sandbox enforcement available on this platform. Policy: {}",
                policy.summary()
            ),
            backend: SandboxBackend::Noop,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_available_returns_true() {
        let sandbox = NoopSandbox::new();
        assert!(sandbox.is_available());
        assert_eq!(sandbox.backend(), SandboxBackend::Noop);
    }

    #[tokio::test]
    async fn apply_returns_not_applied() {
        let sandbox = NoopSandbox::new();
        let policy = SandboxPolicy::deny_all().with_network(true);
        let mut cmd = tokio::process::Command::new("echo");

        let result = sandbox.apply(&policy, &mut cmd).unwrap();
        assert!(!result.applied);
        assert_eq!(result.backend, SandboxBackend::Noop);
        assert!(result.description.contains("No sandbox enforcement"));
    }
}
