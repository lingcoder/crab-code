//! Core [`Sandbox`] trait, [`SandboxBackend`] tag, and [`PreparedCommand`].
//!
//! The API is **transform-style**: a backend takes the raw invocation
//! `(policy, program, args, cwd)` and returns a [`tokio::process::Command`]
//! that is ready to spawn. macOS wraps the argv with `sandbox-exec`; Linux
//! installs a Landlock `pre_exec` hook on the command; Windows and the no-op
//! backend return the command unchanged. Rewriting argv only works before the
//! `Command` is constructed, which is why this cannot be a "mutate an existing
//! `Command`" API.

use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::policy::SandboxPolicy;

/// Which sandbox backend is in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxBackend {
    /// Linux Landlock LSM (kernel 5.13+), installed via `pre_exec`.
    Landlock,
    /// macOS Seatbelt (`sandbox-exec` SBPL profiles).
    Seatbelt,
    /// Windows — no real isolation available (fail-open with a warning).
    Windows,
    /// No sandboxing available — policy checked but not enforced.
    Noop,
}

impl fmt::Display for SandboxBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Landlock => f.write_str("landlock"),
            Self::Seatbelt => f.write_str("seatbelt"),
            Self::Windows => f.write_str("windows"),
            Self::Noop => f.write_str("noop"),
        }
    }
}

/// A command prepared for (possibly) sandboxed execution.
///
/// `command` has its program and args set (rewritten for Seatbelt) and any
/// `pre_exec` hook installed (Landlock). The caller still sets stdio, working
/// directory, and environment before spawning.
pub struct PreparedCommand {
    /// The command to spawn.
    pub command: tokio::process::Command,
    /// Whether real enforcement was applied. `false` means the command runs
    /// with full privileges (no available backend, or a no-op policy).
    pub applied: bool,
    /// The backend that produced this command.
    pub backend: SandboxBackend,
    /// Human-readable description of what was enforced.
    pub description: String,
}

impl fmt::Debug for PreparedCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedCommand")
            .field("applied", &self.applied)
            .field("backend", &self.backend)
            .field("description", &self.description)
            .finish_non_exhaustive()
    }
}

/// Trait for platform-specific sandbox implementations.
pub trait Sandbox: Send + Sync {
    /// The backend identifier.
    fn backend(&self) -> SandboxBackend;

    /// Whether this sandbox backend can actually enforce on the current system.
    fn is_available(&self) -> bool;

    /// Prepare `(program, args)` for sandboxed execution under `policy`,
    /// deriving writable roots relative to `cwd`.
    ///
    /// # Errors
    ///
    /// Returns an error when the policy requires isolation but this backend
    /// cannot provide it and the platform's contract is fail-closed
    /// (Linux/macOS). Windows fails open (returns an unenforced command).
    fn prepare(
        &self,
        policy: &SandboxPolicy,
        program: &str,
        args: &[String],
        cwd: &Path,
    ) -> crab_core::Result<PreparedCommand>;
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
        assert_eq!(SandboxBackend::Seatbelt.to_string(), "seatbelt");
        assert_eq!(SandboxBackend::Windows.to_string(), "windows");
        assert_eq!(SandboxBackend::Noop.to_string(), "noop");
    }
}
