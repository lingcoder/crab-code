//! macOS Seatbelt backend.
//!
//! Generates an SBPL profile from the derived policy and wraps the invocation
//! as `/usr/bin/sandbox-exec -p <profile> -D<KEY>=<path> ... -- <program> <args>`.
//! The executable path is hardcoded to `/usr/bin/sandbox-exec` so a malicious
//! `sandbox-exec` earlier on `PATH` cannot hijack the wrap (mirrors codex).
//!
//! Model: deny-by-default, allow full-disk read, restrict writes to the derived
//! writable roots, and carve `.git` / `.crab` back out as read-only. Network is
//! denied unless the policy allows it.

use std::fmt::Write as _;
use std::path::PathBuf;

use crate::policy::SandboxPolicy;
use crate::traits::{PreparedCommand, Sandbox, SandboxBackend};

/// Only ever invoke `sandbox-exec` from `/usr/bin` — if that binary is
/// compromised the attacker already has root, so PATH injection is the only
/// thing worth defending against here.
const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// Base SBPL profile: closed-by-default with the minimum a shell/toolchain
/// needs to run (exec/fork, signals to same-sandbox peers, `/dev/null`, ptys,
/// read-only sysctls, cfprefs). Embedded verbatim from codex's proven
/// `seatbelt_base_policy.sbpl` so the profile is guaranteed to parse. Full-disk
/// read, the writable-root rules, and network are appended per policy.
const BASE_POLICY: &str = include_str!("seatbelt_base_policy.sbpl");

/// macOS Seatbelt sandbox.
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
        std::path::Path::new(SANDBOX_EXEC).exists()
    }

    fn prepare(
        &self,
        policy: &SandboxPolicy,
        program: &str,
        args: &[String],
        cwd: &std::path::Path,
    ) -> crab_core::Result<PreparedCommand> {
        if !self.is_available() {
            // macOS always ships sandbox-exec; missing it means we cannot honor
            // a policy that asked for isolation, so fail closed.
            return Err(crab_core::Error::Other(format!(
                "sandbox required but {SANDBOX_EXEC} is not available"
            )));
        }

        let derived = policy.derive(cwd);
        let (profile, params) = build_profile(&derived);

        let mut sandbox_args: Vec<String> = vec!["-p".to_string(), profile];
        for (key, path) in params {
            sandbox_args.push(format!("-D{key}={}", path.to_string_lossy()));
        }
        sandbox_args.push("--".to_string());
        sandbox_args.push(program.to_string());
        sandbox_args.extend(args.iter().cloned());

        let mut command = tokio::process::Command::new(SANDBOX_EXEC);
        command.args(&sandbox_args);

        Ok(PreparedCommand {
            command,
            applied: true,
            backend: SandboxBackend::Seatbelt,
            description: format!(
                "seatbelt: {} writable root(s), network {}",
                derived.writable_roots.len(),
                if derived.allow_network { "on" } else { "off" }
            ),
        })
    }
}

/// Build the full SBPL profile string and the `-D` param bindings.
fn build_profile(derived: &crate::policy::DerivedSandbox) -> (String, Vec<(String, PathBuf)>) {
    let mut profile = String::from(BASE_POLICY);
    let mut params: Vec<(String, PathBuf)> = Vec::new();

    // Full-disk read: the MVP grants read everywhere and restricts only writes.
    profile.push_str("\n; allow read-only file operations\n(allow file-read*)");

    if !derived.writable_roots.is_empty() {
        let mut components: Vec<String> = Vec::new();
        for (i, root) in derived.writable_roots.iter().enumerate() {
            let root_key = format!("WRITABLE_ROOT_{i}");
            params.push((root_key.clone(), root.root.clone()));

            if root.read_only_subpaths.is_empty() {
                components.push(format!("(subpath (param \"{root_key}\"))"));
                continue;
            }

            let mut parts = vec![format!("(subpath (param \"{root_key}\"))")];
            for (j, ro) in root.read_only_subpaths.iter().enumerate() {
                let ro_key = format!("WRITABLE_ROOT_{i}_RO_{j}");
                params.push((ro_key.clone(), ro.clone()));
                // Exclude both the directory itself and everything beneath it,
                // so a protected dir cannot be recreated with new contents.
                parts.push(format!("(require-not (literal (param \"{ro_key}\")))"));
                parts.push(format!("(require-not (subpath (param \"{ro_key}\")))"));
            }
            components.push(format!("(require-all {} )", parts.join(" ")));
        }
        let _ = write!(
            profile,
            "\n(allow file-write*\n{}\n)",
            components.join("\n")
        );
    }

    if derived.allow_network {
        profile.push_str("\n(allow network-outbound)\n(allow network-inbound)");
    }

    (profile, params)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_is_seatbelt() {
        assert_eq!(SeatbeltSandbox::new().backend(), SandboxBackend::Seatbelt);
    }

    #[test]
    fn profile_denies_by_default_and_grants_read() {
        let policy = SandboxPolicy::workspace_write("/work", false);
        let derived = policy.derive(std::path::Path::new("/work"));
        let (profile, params) = build_profile(&derived);
        assert!(profile.contains("(deny default)"));
        assert!(profile.contains("(allow file-read*)"));
        assert!(profile.contains("(allow file-write*"));
        // cwd + its two protected subpaths become params.
        assert!(params.iter().any(|(k, _)| k == "WRITABLE_ROOT_0"));
        assert!(params.iter().any(|(k, _)| k.contains("_RO_")));
    }

    #[test]
    fn read_only_profile_has_no_write_section() {
        let policy = SandboxPolicy::read_only();
        let derived = policy.derive(std::path::Path::new("/work"));
        let (profile, params) = build_profile(&derived);
        assert!(!profile.contains("(allow file-write*"));
        assert!(params.is_empty());
    }

    #[test]
    fn network_section_gated_on_policy() {
        let off = SandboxPolicy::workspace_write("/work", false);
        let (p_off, _) = build_profile(&off.derive(std::path::Path::new("/work")));
        assert!(!p_off.contains("network-outbound"));

        let on = SandboxPolicy::workspace_write("/work", true);
        let (p_on, _) = build_profile(&on.derive(std::path::Path::new("/work")));
        assert!(p_on.contains("(allow network-outbound)"));
    }

    #[tokio::test]
    async fn prepare_wraps_argv_when_available() {
        let sandbox = SeatbeltSandbox::new();
        if !sandbox.is_available() {
            return; // not on macOS — nothing to assert
        }
        let policy = SandboxPolicy::workspace_write("/tmp", false);
        let prepared = sandbox
            .prepare(
                &policy,
                "echo",
                &["hi".to_string()],
                std::path::Path::new("/tmp"),
            )
            .unwrap();
        assert!(prepared.applied);
        assert_eq!(
            prepared.command.as_std().get_program().to_string_lossy(),
            SANDBOX_EXEC
        );
    }
}
