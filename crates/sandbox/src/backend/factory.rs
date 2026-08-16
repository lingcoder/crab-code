//! Backend selection and command preparation.
//!
//! [`create_sandbox`] picks the platform backend, and [`prepare_command`] is the
//! one call tool/process code makes to turn a raw invocation into a spawnable,
//! (possibly) sandboxed [`PreparedCommand`].

use std::path::Path;

use super::{LandlockSandbox, NoopSandbox, SeatbeltSandbox, WindowsSandbox};
use crate::policy::SandboxPolicy;
use crate::traits::{PreparedCommand, Sandbox};

/// Create the sandbox backend designated for the current platform.
#[must_use]
pub fn create_sandbox() -> Box<dyn Sandbox> {
    if cfg!(target_os = "linux") {
        Box::new(LandlockSandbox::new())
    } else if cfg!(target_os = "macos") {
        Box::new(SeatbeltSandbox::new())
    } else if cfg!(target_os = "windows") {
        Box::new(WindowsSandbox::new())
    } else {
        Box::new(NoopSandbox::new())
    }
}

/// Prepare a command for sandboxed execution under the platform backend.
///
/// The returned [`PreparedCommand`] has its program/args set (rewritten on
/// macOS) and any `pre_exec` hook installed (Linux). Fail-open/closed is decided
/// by the selected backend: Linux/macOS fail closed (error) when isolation was
/// requested but cannot be provided; Windows and the no-op backend fail open.
///
/// # Errors
///
/// Propagates the backend's error when a fail-closed platform cannot enforce.
pub fn prepare_command(
    policy: &SandboxPolicy,
    program: &str,
    args: &[String],
    cwd: &Path,
) -> crab_core::Result<PreparedCommand> {
    create_sandbox().prepare(policy, program, args, cwd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::SandboxBackend;

    #[test]
    fn create_sandbox_selects_platform_backend() {
        let expected = if cfg!(target_os = "linux") {
            SandboxBackend::Landlock
        } else if cfg!(target_os = "macos") {
            SandboxBackend::Seatbelt
        } else if cfg!(target_os = "windows") {
            SandboxBackend::Windows
        } else {
            SandboxBackend::Noop
        };
        assert_eq!(create_sandbox().backend(), expected);
    }

    #[tokio::test]
    async fn prepare_command_returns_spawnable() {
        // On every platform this must produce a command; whether it is actually
        // enforced depends on backend availability.
        let cwd = std::env::current_dir().unwrap();
        let policy = SandboxPolicy::workspace_write(&cwd, false);
        let prepared = prepare_command(&policy, "echo", &["hi".to_string()], &cwd);
        // macOS with sandbox-exec + Linux both succeed; Windows/noop succeed
        // (fail-open). The only error path is a fail-closed platform missing its
        // primitive, which the CI matrix covers.
        assert!(prepared.is_ok());
    }
}
