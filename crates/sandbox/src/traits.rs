//! Core [`Sandbox`] trait, [`SandboxBackend`] tag, and [`SandboxResult`].
//!
//! Platform-agnostic; the concrete backends live under
//! [`crate::backend`].

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::policy::SandboxPolicy;

/// Which sandbox backend is in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxBackend {
    /// Linux Landlock LSM.
    Landlock,
    /// Windows Job Object with restricted token.
    WindowsJobObject,
    /// No sandboxing available — policy checked but not enforced.
    Noop,
}

impl fmt::Display for SandboxBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Landlock => f.write_str("landlock"),
            Self::WindowsJobObject => f.write_str("windows_job_object"),
            Self::Noop => f.write_str("noop"),
        }
    }
}

/// Result of applying a sandbox to a process.
#[derive(Debug, Clone)]
pub struct SandboxResult {
    /// Whether the sandbox was successfully applied.
    pub applied: bool,
    /// Human-readable description of what was enforced.
    pub description: String,
    /// The backend that was used.
    pub backend: SandboxBackend,
}

/// Trait for platform-specific sandbox implementations.
pub trait Sandbox: Send + Sync {
    /// The backend identifier.
    fn backend(&self) -> SandboxBackend;

    /// Whether this sandbox backend is available on the current system.
    fn is_available(&self) -> bool;

    /// Apply the sandbox policy to a command before spawning.
    ///
    /// Implementations should configure the command (e.g., pre-exec
    /// hooks, environment, Job Object handles) so that the spawned
    /// process is restricted according to the policy.
    fn apply(
        &self,
        policy: &SandboxPolicy,
        cmd: &mut tokio::process::Command,
    ) -> crab_common::Result<SandboxResult>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_backend_serde() {
        let json = serde_json::to_string(&SandboxBackend::Landlock).unwrap();
        assert_eq!(json, "\"landlock\"");
        let back: SandboxBackend = serde_json::from_str(&json).unwrap();
        assert_eq!(back, SandboxBackend::Landlock);
    }

    #[test]
    fn sandbox_backend_display() {
        assert_eq!(SandboxBackend::Landlock.to_string(), "landlock");
        assert_eq!(
            SandboxBackend::WindowsJobObject.to_string(),
            "windows_job_object"
        );
        assert_eq!(SandboxBackend::Noop.to_string(), "noop");
    }
}
