//! Backend auto-selection: [`create_sandbox`] and the convenience
//! [`apply_policy`] entry point that bundles "pick + apply" into one
//! call for tool callers.

use super::NoopSandbox;

#[cfg(target_os = "linux")]
use super::LandlockSandbox;

#[cfg(target_os = "windows")]
use super::WindowsJobSandbox;

use crate::policy::SandboxPolicy;
use crate::traits::{Sandbox, SandboxResult};

/// Create the best available sandbox backend for the current platform.
#[must_use]
pub fn create_sandbox() -> Box<dyn Sandbox> {
    #[cfg(target_os = "linux")]
    {
        let landlock = LandlockSandbox::new();
        if landlock.is_available() {
            return Box::new(landlock);
        }
    }

    #[cfg(target_os = "windows")]
    {
        return Box::new(WindowsJobSandbox::new());
    }

    #[allow(unreachable_code)]
    Box::new(NoopSandbox::new())
}

/// Apply a sandbox policy to a command using the best available backend.
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
    use crate::traits::SandboxBackend;

    #[test]
    fn create_sandbox_returns_some_backend() {
        let sandbox = create_sandbox();
        let backend = sandbox.backend();
        assert!(
            backend == SandboxBackend::Landlock
                || backend == SandboxBackend::WindowsJobObject
                || backend == SandboxBackend::Noop
        );
    }

    #[tokio::test]
    async fn apply_policy_works() {
        let policy = SandboxPolicy::tool_default("/project", "/project/out");
        let mut cmd = tokio::process::Command::new("echo");
        let result = apply_policy(&policy, &mut cmd).unwrap();
        assert!(!result.description.is_empty());
    }
}
