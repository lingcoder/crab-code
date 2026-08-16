//! No-op backend — passthrough. Selected on platforms without a supported
//! sandbox primitive (non-linux/macos/windows unix). Never enforces.

use crate::policy::SandboxPolicy;
use crate::traits::{PreparedCommand, Sandbox, SandboxBackend};

/// No-op sandbox: builds the command unchanged and records that nothing was
/// enforced.
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
        false
    }

    fn prepare(
        &self,
        policy: &SandboxPolicy,
        program: &str,
        args: &[String],
        _cwd: &std::path::Path,
    ) -> crab_core::Result<PreparedCommand> {
        let mut command = tokio::process::Command::new(program);
        command.args(args);
        Ok(PreparedCommand {
            command,
            applied: false,
            backend: SandboxBackend::Noop,
            description: format!("noop: no enforcement (policy: {})", policy.summary()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn noop_returns_unchanged_command() {
        let sandbox = NoopSandbox::new();
        assert_eq!(sandbox.backend(), SandboxBackend::Noop);
        let policy = SandboxPolicy::workspace_write("/work", true);
        let prepared = sandbox
            .prepare(&policy, "echo", &[], std::path::Path::new("/work"))
            .unwrap();
        assert!(!prepared.applied);
        assert_eq!(
            prepared.command.as_std().get_program().to_string_lossy(),
            "echo"
        );
    }
}
