//! Linux Landlock backend.
//!
//! Installs a Landlock ruleset in the forked child (via `pre_exec`, before
//! `exec`) so only the spawned tool is confined — never the agent process.
//! Read access to the whole filesystem is granted; write access is limited to
//! the derived writable roots (plus `/dev/null`). Uses `ABI::V5` with
//! `CompatLevel::BestEffort` so newer kernels enforce more and older kernels
//! degrade gracefully — but a kernel with **no** Landlock support yields
//! `RulesetStatus::NotEnforced`, which we turn into a spawn error (fail-closed).
//!
//! Network confinement (seccomp) is intentionally out of scope for this MVP;
//! the Linux backend restricts the filesystem only.

use crate::policy::SandboxPolicy;
use crate::traits::{PreparedCommand, Sandbox, SandboxBackend};

/// Linux Landlock sandbox.
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
        cfg!(target_os = "linux")
    }

    #[cfg(target_os = "linux")]
    fn prepare(
        &self,
        policy: &SandboxPolicy,
        program: &str,
        args: &[String],
        cwd: &std::path::Path,
    ) -> crab_core::Result<PreparedCommand> {
        let derived = policy.derive(cwd);
        let writable_roots: Vec<std::path::PathBuf> = derived
            .writable_roots
            .iter()
            .map(|r| r.root.clone())
            .filter(|p| p.exists())
            .collect();

        let mut command = tokio::process::Command::new(program);
        command.args(args);

        // SAFETY: `apply_landlock_rules` only calls async-signal-safe landlock
        // syscalls (ruleset creation + restrict_self); it allocates and touches
        // no shared parent state, so it is safe to run in the post-fork child.
        unsafe {
            command.pre_exec(move || apply_landlock_rules(&writable_roots));
        }

        let description = format!(
            "landlock: {} writable root(s), full-disk read",
            derived.writable_roots.len()
        );
        Ok(PreparedCommand {
            command,
            applied: true,
            backend: SandboxBackend::Landlock,
            description,
        })
    }

    #[cfg(not(target_os = "linux"))]
    fn prepare(
        &self,
        _policy: &SandboxPolicy,
        _program: &str,
        _args: &[String],
        _cwd: &std::path::Path,
    ) -> crab_core::Result<PreparedCommand> {
        Err(crab_core::Error::Other(
            "Landlock sandbox is only available on Linux".into(),
        ))
    }
}

/// Install the Landlock ruleset on the current (child) thread: full-disk read,
/// write restricted to `writable_roots` and `/dev/null`.
///
/// Returns an `io::Error` when the kernel does not enforce the ruleset, which
/// aborts the `exec` and surfaces as a spawn failure (fail-closed).
#[cfg(target_os = "linux")]
fn apply_landlock_rules(writable_roots: &[std::path::PathBuf]) -> std::io::Result<()> {
    use landlock::{
        ABI, Access, AccessFs, CompatLevel, Compatible, Ruleset, RulesetAttr, RulesetCreatedAttr,
        RulesetStatus, path_beneath_rules,
    };

    fn to_io<E: std::fmt::Display>(e: E) -> std::io::Error {
        std::io::Error::other(e.to_string())
    }

    let abi = ABI::V5;
    let full_access = AccessFs::from_all(abi);
    let read_access = AccessFs::from_read(abi);

    let mut ruleset = Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(full_access)
        .map_err(to_io)?
        .create()
        .map_err(to_io)?
        .add_rules(path_beneath_rules(["/"], read_access))
        .map_err(to_io)?
        .no_new_privs(true);

    if std::path::Path::new("/dev/null").exists() {
        ruleset = ruleset
            .add_rules(path_beneath_rules(["/dev/null"], full_access))
            .map_err(to_io)?;
    }

    if !writable_roots.is_empty() {
        ruleset = ruleset
            .add_rules(path_beneath_rules(writable_roots, full_access))
            .map_err(to_io)?;
    }

    let status = ruleset.restrict_self().map_err(to_io)?;
    if status.ruleset == RulesetStatus::NotEnforced {
        return Err(std::io::Error::other(
            "Landlock is not supported by this kernel",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_is_landlock() {
        assert_eq!(LandlockSandbox::new().backend(), SandboxBackend::Landlock);
    }

    #[test]
    fn availability_matches_platform() {
        assert_eq!(
            LandlockSandbox::new().is_available(),
            cfg!(target_os = "linux")
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn prepare_installs_hook_and_keeps_program() {
        let cwd = std::env::current_dir().unwrap();
        let policy = SandboxPolicy::workspace_write(&cwd, false);
        let prepared = LandlockSandbox::new()
            .prepare(&policy, "echo", &["hi".to_string()], &cwd)
            .unwrap();
        assert!(prepared.applied);
        assert_eq!(
            prepared.command.as_std().get_program().to_string_lossy(),
            "echo"
        );
    }
}
