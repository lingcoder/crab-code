//! Windows backend — no real isolation.
//!
//! A production Windows sandbox needs a restricted-token / `AppContainer`
//! launcher (codex ships an entire `windows-sandbox` crate for it); that is out
//! of scope here. To keep crab usable on Windows we **fail open**: the command
//! runs unchanged and every attempt to sandbox emits a warning so the operator
//! knows isolation is not in effect.

use crate::policy::SandboxPolicy;
use crate::traits::{PreparedCommand, Sandbox, SandboxBackend};

/// Windows sandbox placeholder — never enforces.
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

    fn prepare(
        &self,
        policy: &SandboxPolicy,
        program: &str,
        args: &[String],
        _cwd: &std::path::Path,
    ) -> crab_core::Result<PreparedCommand> {
        tracing::warn!(
            policy = policy.summary(),
            "Windows has no sandbox isolation — command runs unconfined"
        );
        let mut command = tokio::process::Command::new(program);
        command.args(args);
        Ok(PreparedCommand {
            command,
            applied: false,
            backend: SandboxBackend::Windows,
            description: "windows: no isolation available (command runs unconfined)".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn windows_fails_open_unchanged() {
        let sandbox = WindowsSandbox::new();
        assert!(!sandbox.is_available());
        let policy = SandboxPolicy::workspace_write("/work", false);
        let prepared = sandbox
            .prepare(
                &policy,
                "echo",
                &["hi".to_string()],
                std::path::Path::new("/work"),
            )
            .unwrap();
        assert!(!prepared.applied);
        assert_eq!(
            prepared.command.as_std().get_program().to_string_lossy(),
            "echo"
        );
    }
}
