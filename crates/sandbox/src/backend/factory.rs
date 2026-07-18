//! Backend selection: [`create_sandbox`] and the convenience
//! [`apply_policy`] entry point that bundles pick + apply into one
//! call for tool callers.

use super::{LandlockSandbox, NoopSandbox, SeatbeltSandbox, WindowsSandbox};
use crate::policy::SandboxPolicy;
use crate::traits::{Sandbox, SandboxResult};

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

/// Apply a sandbox policy to a command using the platform backend.
///
/// Main entry point for callers (e.g. `BashTool`) that want to sandbox
/// a child process without caring which backend is active.
pub fn apply_policy(
    policy: &SandboxPolicy,
    cmd: &mut tokio::process::Command,
) -> crab_core::Result<SandboxResult> {
    let sandbox = create_sandbox();
    sandbox.apply(policy, cmd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::PathAccess;
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
    async fn apply_policy_reports_not_applied() {
        let policy = SandboxPolicy::deny_all().with_path("/tmp", PathAccess::ReadOnly);
        let mut cmd = tokio::process::Command::new("echo");
        let result = apply_policy(&policy, &mut cmd).unwrap();
        assert!(!result.applied);
    }
}
