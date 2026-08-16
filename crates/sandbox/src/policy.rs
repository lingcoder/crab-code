//! [`SandboxPolicy`] — what a sandboxed child process is allowed to do.
//!
//! Pure data + derivation helpers. Platform-specific enforcement happens in
//! the [`backend`] modules; this file is portable across all targets.
//!
//! The enforcement model (aligned with codex `WorkspaceWrite`): a sandboxed
//! child gets **full-disk read** plus **write access restricted to a set of
//! writable roots**. Writable roots always include the working directory, and
//! (on unix) `/tmp` and `$TMPDIR` when present. Within each writable root the
//! `.git` and `.crab` subdirectories are forced read-only so a command cannot
//! rewrite `.git/hooks` to escalate. A read-only policy ([`SandboxPolicy::read_only`])
//! grants full read and zero writable roots.
//!
//! [`backend`]: super::backend

use std::fmt;
use std::path::{Path, PathBuf};

use crab_core::permission::PermissionMode;
use serde::{Deserialize, Serialize};

/// Subdirectories that stay read-only even inside a writable root.
///
/// Prevents a sandboxed command from rewriting `.git/hooks` (or the agent's own
/// `.crab` state) to escape the sandbox on the next tool call.
pub const PROTECTED_WRITABLE_SUBDIRS: [&str; 2] = [".git", ".crab"];

/// Access level for a filesystem path in the sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathAccess {
    /// Read-only access.
    ReadOnly,
    /// Read and write access.
    ReadWrite,
    /// Full access including execute.
    Full,
}

impl fmt::Display for PathAccess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadOnly => f.write_str("read_only"),
            Self::ReadWrite => f.write_str("read_write"),
            Self::Full => f.write_str("full"),
        }
    }
}

/// A single filesystem path rule within a sandbox policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathRule {
    /// The directory or file path to allow.
    pub path: PathBuf,
    /// The access level granted.
    pub access: PathAccess,
}

/// Policy describing what a sandboxed process is allowed to do.
///
/// A default policy denies everything. Fields are additive: each
/// `allow_*` field opens up a specific capability.
///
/// `max_memory_bytes` / `max_cpu_seconds` / `max_open_files` are carried for
/// completeness but are **not enforced** by the current MVP backends
/// (Seatbelt / Landlock); they are reserved for a later resource-limit
/// milestone.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SandboxPolicy {
    /// Allowed filesystem paths with their access levels. Paths granted
    /// `ReadWrite`/`Full` become writable roots during derivation.
    pub path_rules: Vec<PathRule>,
    /// When `true`, the child gets full-disk read and **no** writable roots,
    /// regardless of `path_rules`. Used for planning / read-only modes.
    #[serde(default)]
    pub read_only: bool,
    /// Whether the process may access the network.
    pub allow_network: bool,
    /// Whether the process may spawn child processes.
    pub allow_subprocess: bool,
    /// Maximum memory in bytes (0 = unlimited). Not enforced in the MVP.
    pub max_memory_bytes: u64,
    /// Maximum CPU time in seconds (0 = unlimited). Not enforced in the MVP.
    pub max_cpu_seconds: u64,
    /// Maximum number of open file descriptors (0 = unlimited). Not enforced.
    pub max_open_files: u64,
}

/// A writable root plus the subpaths beneath it that stay read-only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WritableRoot {
    /// The directory the child may write to.
    pub root: PathBuf,
    /// Subpaths under `root` that must remain read-only (e.g. `.git`, `.crab`).
    pub read_only_subpaths: Vec<PathBuf>,
}

/// The concrete filesystem restrictions a backend enforces, derived from a
/// [`SandboxPolicy`] for a specific working directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedSandbox {
    /// Roots the child may write to. Empty for a read-only policy.
    pub writable_roots: Vec<WritableRoot>,
    /// Whether the network is reachable.
    pub allow_network: bool,
}

impl SandboxPolicy {
    /// Create a policy that denies everything.
    #[must_use]
    pub fn deny_all() -> Self {
        Self::default()
    }

    /// Build a workspace-write policy: read everything, write only the working
    /// directory (plus `/tmp` / `$TMPDIR`, added during derivation). This is the
    /// policy applied to Bash/PowerShell tool commands in the default modes.
    #[must_use]
    pub fn workspace_write(cwd: impl Into<PathBuf>, allow_network: bool) -> Self {
        Self {
            path_rules: vec![PathRule {
                path: cwd.into(),
                access: PathAccess::ReadWrite,
            }],
            read_only: false,
            allow_network,
            allow_subprocess: true,
            max_memory_bytes: 0,
            max_cpu_seconds: 0,
            max_open_files: 0,
        }
    }

    /// Build a read-only policy: read everything, write nothing, no network.
    #[must_use]
    pub fn read_only() -> Self {
        Self {
            path_rules: Vec::new(),
            read_only: true,
            allow_network: false,
            allow_subprocess: true,
            max_memory_bytes: 0,
            max_cpu_seconds: 0,
            max_open_files: 0,
        }
    }

    /// Derive the sandbox policy for a permission mode + working directory,
    /// or `None` when no sandbox should be applied.
    ///
    /// - `Dangerously` → `None` (run with full privileges; the user opted out).
    /// - `Plan` → read-only, no network (the agent may look but not touch).
    /// - everything else → workspace-write (cwd writable, network allowed).
    ///
    /// Network is left open for workspace-write because the MVP does not enforce
    /// network confinement on Linux (no seccomp filter yet); enabling it only on
    /// macOS would make behaviour platform-dependent. The enforced dimension is
    /// filesystem writes. `Plan` mode still blocks network on macOS since it is
    /// read-only by intent.
    ///
    /// Returns `None` unconditionally when sandboxing is globally disabled
    /// (see [`crate::set_enabled`]).
    #[must_use]
    pub fn for_mode(mode: PermissionMode, cwd: &Path) -> Option<Self> {
        if !crate::is_enabled() {
            return None;
        }
        match mode {
            PermissionMode::Dangerously => None,
            PermissionMode::Plan => Some(Self::read_only()),
            PermissionMode::Default
            | PermissionMode::AcceptEdits
            | PermissionMode::TrustProject
            | PermissionMode::DontAsk
            | PermissionMode::Auto => Some(Self::workspace_write(cwd, true)),
        }
    }

    /// Add a path rule to the policy.
    #[must_use]
    pub fn with_path(mut self, path: impl Into<PathBuf>, access: PathAccess) -> Self {
        self.path_rules.push(PathRule {
            path: path.into(),
            access,
        });
        self
    }

    /// Allow network access.
    #[must_use]
    pub const fn with_network(mut self, allow: bool) -> Self {
        self.allow_network = allow;
        self
    }

    /// Allow subprocess creation.
    #[must_use]
    pub const fn with_subprocess(mut self, allow: bool) -> Self {
        self.allow_subprocess = allow;
        self
    }

    /// Set memory limit in bytes.
    #[must_use]
    pub const fn with_max_memory(mut self, bytes: u64) -> Self {
        self.max_memory_bytes = bytes;
        self
    }

    /// Set CPU time limit in seconds.
    #[must_use]
    pub const fn with_max_cpu(mut self, seconds: u64) -> Self {
        self.max_cpu_seconds = seconds;
        self
    }

    /// Derive the concrete filesystem restrictions for `cwd`.
    ///
    /// Writable roots are the `ReadWrite`/`Full` path rules plus the working
    /// directory and (on unix) `/tmp` and `$TMPDIR` when they exist. A
    /// read-only policy yields no writable roots. Each writable root carries the
    /// protected read-only subpaths ([`PROTECTED_WRITABLE_SUBDIRS`]).
    #[must_use]
    pub fn derive(&self, cwd: &Path) -> DerivedSandbox {
        if self.read_only {
            return DerivedSandbox {
                writable_roots: Vec::new(),
                allow_network: self.allow_network,
            };
        }

        let mut roots: Vec<PathBuf> = Vec::new();
        let push_root = |candidate: PathBuf, roots: &mut Vec<PathBuf>| {
            if !roots.iter().any(|r| r == &candidate) {
                roots.push(candidate);
            }
        };

        push_root(cwd.to_path_buf(), &mut roots);
        for rule in &self.path_rules {
            if matches!(rule.access, PathAccess::ReadWrite | PathAccess::Full) {
                push_root(rule.path.clone(), &mut roots);
            }
        }
        for extra in default_writable_tmp_dirs() {
            push_root(extra, &mut roots);
        }

        let writable_roots = roots
            .into_iter()
            .map(|root| {
                let read_only_subpaths = PROTECTED_WRITABLE_SUBDIRS
                    .iter()
                    .map(|name| root.join(name))
                    .collect();
                WritableRoot {
                    root,
                    read_only_subpaths,
                }
            })
            .collect();

        DerivedSandbox {
            writable_roots,
            allow_network: self.allow_network,
        }
    }

    /// Check whether a given path would be allowed under this policy at
    /// the requested access level.
    #[must_use]
    pub fn check_path(&self, target: &Path, requested: PathAccess) -> bool {
        for rule in &self.path_rules {
            if target.starts_with(&rule.path) && access_sufficient(rule.access, requested) {
                return true;
            }
        }
        false
    }

    /// Return a summary of what this policy allows.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();

        if self.read_only {
            parts.push("read-only (no writable roots)".to_string());
        } else if self.path_rules.is_empty() {
            parts.push("no filesystem access".to_string());
        } else {
            for rule in &self.path_rules {
                parts.push(format!("{}:{}", rule.path.display(), rule.access));
            }
        }

        parts.push(format!(
            "network:{}",
            if self.allow_network { "yes" } else { "no" }
        ));
        parts.push(format!(
            "subprocess:{}",
            if self.allow_subprocess { "yes" } else { "no" }
        ));

        parts.join(", ")
    }
}

/// The always-writable temporary directories on unix: `/tmp` and `$TMPDIR`, but
/// only when they actually exist. Empty on non-unix hosts.
fn default_writable_tmp_dirs() -> Vec<PathBuf> {
    #[cfg(unix)]
    {
        let mut dirs = Vec::new();
        let slash_tmp = Path::new("/tmp");
        if slash_tmp.is_dir() {
            dirs.push(slash_tmp.to_path_buf());
        }
        if let Some(tmpdir) = std::env::var_os("TMPDIR").filter(|v| !v.is_empty()) {
            let tmpdir = PathBuf::from(tmpdir);
            if tmpdir.is_dir() && !dirs.iter().any(|d| d == &tmpdir) {
                dirs.push(tmpdir);
            }
        }
        dirs
    }
    #[cfg(not(unix))]
    {
        Vec::new()
    }
}

/// Check if `granted` access level is sufficient for `requested`.
pub(crate) fn access_sufficient(granted: PathAccess, requested: PathAccess) -> bool {
    match requested {
        PathAccess::ReadOnly => true, // any access level grants read
        PathAccess::ReadWrite => matches!(granted, PathAccess::ReadWrite | PathAccess::Full),
        PathAccess::Full => granted == PathAccess::Full,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deny_all_policy() {
        let policy = SandboxPolicy::deny_all();
        assert!(policy.path_rules.is_empty());
        assert!(!policy.read_only);
        assert!(!policy.allow_network);
        assert!(!policy.allow_subprocess);
    }

    #[test]
    fn builder_pattern() {
        let policy = SandboxPolicy::deny_all()
            .with_path("/tmp", PathAccess::ReadWrite)
            .with_path("/usr", PathAccess::ReadOnly)
            .with_network(true)
            .with_subprocess(false);

        assert_eq!(policy.path_rules.len(), 2);
        assert_eq!(policy.path_rules[0].path, Path::new("/tmp"));
        assert_eq!(policy.path_rules[0].access, PathAccess::ReadWrite);
        assert!(policy.allow_network);
        assert!(!policy.allow_subprocess);
    }

    #[test]
    fn workspace_write_makes_cwd_a_writable_root() {
        let cwd = std::env::current_dir().unwrap();
        let policy = SandboxPolicy::workspace_write(&cwd, false);
        assert!(!policy.read_only);
        let derived = policy.derive(&cwd);
        assert!(derived.writable_roots.iter().any(|r| r.root == cwd));
        assert!(!derived.allow_network);
    }

    #[test]
    fn read_only_policy_has_no_writable_roots() {
        let cwd = std::env::current_dir().unwrap();
        let policy = SandboxPolicy::read_only();
        assert!(policy.read_only);
        let derived = policy.derive(&cwd);
        assert!(derived.writable_roots.is_empty());
        assert!(!derived.allow_network);
    }

    #[test]
    fn derive_marks_git_and_crab_read_only() {
        let cwd = std::env::current_dir().unwrap();
        let policy = SandboxPolicy::workspace_write(&cwd, false);
        let derived = policy.derive(&cwd);
        let root = derived
            .writable_roots
            .iter()
            .find(|r| r.root == cwd)
            .expect("cwd is a writable root");
        assert!(root.read_only_subpaths.contains(&cwd.join(".git")));
        assert!(root.read_only_subpaths.contains(&cwd.join(".crab")));
    }

    #[test]
    fn derive_dedups_cwd_when_also_a_path_rule() {
        let cwd = std::env::current_dir().unwrap();
        let policy = SandboxPolicy::deny_all().with_path(&cwd, PathAccess::ReadWrite);
        let derived = policy.derive(&cwd);
        let cwd_roots = derived
            .writable_roots
            .iter()
            .filter(|r| r.root == cwd)
            .count();
        assert_eq!(cwd_roots, 1, "cwd should not be duplicated");
    }

    #[test]
    fn for_mode_dangerously_is_none() {
        let cwd = std::env::current_dir().unwrap();
        assert!(SandboxPolicy::for_mode(PermissionMode::Dangerously, &cwd).is_none());
    }

    #[test]
    fn for_mode_plan_is_read_only() {
        let cwd = std::env::current_dir().unwrap();
        let policy = SandboxPolicy::for_mode(PermissionMode::Plan, &cwd).unwrap();
        assert!(policy.read_only);
        assert!(!policy.allow_network);
    }

    #[test]
    fn for_mode_default_is_workspace_write() {
        let cwd = std::env::current_dir().unwrap();
        let policy = SandboxPolicy::for_mode(PermissionMode::Default, &cwd).unwrap();
        assert!(!policy.read_only);
        let derived = policy.derive(&cwd);
        assert!(derived.writable_roots.iter().any(|r| r.root == cwd));
    }

    #[test]
    fn check_path_allowed_read() {
        let policy =
            SandboxPolicy::deny_all().with_path("/home/user/project", PathAccess::ReadOnly);
        assert!(policy.check_path(
            Path::new("/home/user/project/src/main.rs"),
            PathAccess::ReadOnly
        ));
    }

    #[test]
    fn check_path_denied_write_on_readonly() {
        let policy =
            SandboxPolicy::deny_all().with_path("/home/user/project", PathAccess::ReadOnly);
        assert!(!policy.check_path(
            Path::new("/home/user/project/src/main.rs"),
            PathAccess::ReadWrite
        ));
    }

    #[test]
    fn access_sufficient_matrix() {
        assert!(access_sufficient(
            PathAccess::ReadOnly,
            PathAccess::ReadOnly
        ));
        assert!(!access_sufficient(
            PathAccess::ReadOnly,
            PathAccess::ReadWrite
        ));
        assert!(access_sufficient(
            PathAccess::ReadWrite,
            PathAccess::ReadOnly
        ));
        assert!(access_sufficient(
            PathAccess::ReadWrite,
            PathAccess::ReadWrite
        ));
        assert!(!access_sufficient(PathAccess::ReadWrite, PathAccess::Full));
        assert!(access_sufficient(PathAccess::Full, PathAccess::Full));
    }

    #[test]
    fn summary_read_only() {
        let policy = SandboxPolicy::read_only();
        let summary = policy.summary();
        assert!(summary.contains("read-only"));
        assert!(summary.contains("network:no"));
    }

    #[test]
    fn summary_with_paths() {
        let policy = SandboxPolicy::deny_all()
            .with_path("/tmp", PathAccess::ReadWrite)
            .with_network(true);
        let summary = policy.summary();
        assert!(summary.contains("read_write"));
        assert!(summary.contains("network:yes"));
    }

    #[test]
    fn policy_serde_roundtrip() {
        let policy = SandboxPolicy::workspace_write("/work", true);
        let json = serde_json::to_string(&policy).unwrap();
        let restored: SandboxPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.path_rules.len(), 1);
        assert!(restored.allow_network);
        assert!(!restored.read_only);
    }

    #[test]
    fn read_only_field_defaults_when_absent_in_json() {
        // Older serialized policies omit `read_only`; it must default to false.
        let json = r#"{"path_rules":[],"allow_network":false,"allow_subprocess":true,"max_memory_bytes":0,"max_cpu_seconds":0,"max_open_files":0}"#;
        let policy: SandboxPolicy = serde_json::from_str(json).unwrap();
        assert!(!policy.read_only);
    }

    #[test]
    fn path_access_serde() {
        let json = serde_json::to_string(&PathAccess::ReadOnly).unwrap();
        assert_eq!(json, "\"read_only\"");
        let back: PathAccess = serde_json::from_str(&json).unwrap();
        assert_eq!(back, PathAccess::ReadOnly);
    }

    #[test]
    fn path_access_display() {
        assert_eq!(PathAccess::ReadOnly.to_string(), "read_only");
        assert_eq!(PathAccess::ReadWrite.to_string(), "read_write");
        assert_eq!(PathAccess::Full.to_string(), "full");
    }
}
