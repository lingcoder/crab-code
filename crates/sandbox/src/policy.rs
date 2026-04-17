//! [`SandboxPolicy`] — what a sandboxed child process is allowed to do.
//!
//! Pure data + check helpers. Platform-specific enforcement happens in
//! the [`backend`] modules; this file is portable across all targets.
//!
//! [`backend`]: super::backend

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SandboxPolicy {
    /// Allowed filesystem paths with their access levels.
    pub path_rules: Vec<PathRule>,
    /// Whether the process may access the network.
    pub allow_network: bool,
    /// Whether the process may spawn child processes.
    pub allow_subprocess: bool,
    /// Maximum memory in bytes (0 = unlimited).
    pub max_memory_bytes: u64,
    /// Maximum CPU time in seconds (0 = unlimited).
    pub max_cpu_seconds: u64,
    /// Maximum number of open file descriptors (0 = unlimited).
    pub max_open_files: u64,
}

impl SandboxPolicy {
    /// Create a policy that denies everything.
    #[must_use]
    pub fn deny_all() -> Self {
        Self::default()
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

    /// Build a reasonable default policy for running tool commands.
    ///
    /// Allows read access to the project directory, read-write to a
    /// working directory, and network access (many tools need HTTP).
    #[must_use]
    pub fn tool_default(project_dir: impl Into<PathBuf>, working_dir: impl Into<PathBuf>) -> Self {
        Self::deny_all()
            .with_path(project_dir, PathAccess::ReadOnly)
            .with_path(working_dir, PathAccess::ReadWrite)
            .with_network(true)
            .with_subprocess(true)
            .with_max_memory(512 * 1024 * 1024) // 512 MB
            .with_max_cpu(120) // 2 minutes
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

        if self.path_rules.is_empty() {
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

        if self.max_memory_bytes > 0 {
            parts.push(format!("mem:{}MB", self.max_memory_bytes / (1024 * 1024)));
        }
        if self.max_cpu_seconds > 0 {
            parts.push(format!("cpu:{}s", self.max_cpu_seconds));
        }

        parts.join(", ")
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
        assert!(!policy.allow_network);
        assert!(!policy.allow_subprocess);
        assert_eq!(policy.max_memory_bytes, 0);
        assert_eq!(policy.max_cpu_seconds, 0);
        assert_eq!(policy.max_open_files, 0);
    }

    #[test]
    fn builder_pattern() {
        let policy = SandboxPolicy::deny_all()
            .with_path("/tmp", PathAccess::ReadWrite)
            .with_path("/usr", PathAccess::ReadOnly)
            .with_network(true)
            .with_subprocess(false)
            .with_max_memory(256 * 1024 * 1024)
            .with_max_cpu(60);

        assert_eq!(policy.path_rules.len(), 2);
        assert_eq!(policy.path_rules[0].path, Path::new("/tmp"));
        assert_eq!(policy.path_rules[0].access, PathAccess::ReadWrite);
        assert_eq!(policy.path_rules[1].path, Path::new("/usr"));
        assert_eq!(policy.path_rules[1].access, PathAccess::ReadOnly);
        assert!(policy.allow_network);
        assert!(!policy.allow_subprocess);
        assert_eq!(policy.max_memory_bytes, 256 * 1024 * 1024);
        assert_eq!(policy.max_cpu_seconds, 60);
    }

    #[test]
    fn tool_default_policy() {
        let policy = SandboxPolicy::tool_default("/project", "/project/build");
        assert_eq!(policy.path_rules.len(), 2);
        assert!(policy.allow_network);
        assert!(policy.allow_subprocess);
        assert!(policy.max_memory_bytes > 0);
        assert!(policy.max_cpu_seconds > 0);
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
    fn check_path_allowed_write_on_readwrite() {
        let policy = SandboxPolicy::deny_all().with_path("/tmp", PathAccess::ReadWrite);
        assert!(policy.check_path(Path::new("/tmp/output.txt"), PathAccess::ReadWrite));
        assert!(policy.check_path(Path::new("/tmp/output.txt"), PathAccess::ReadOnly));
    }

    #[test]
    fn check_path_denied_outside_rules() {
        let policy = SandboxPolicy::deny_all().with_path("/home/user/project", PathAccess::Full);
        assert!(!policy.check_path(Path::new("/etc/passwd"), PathAccess::ReadOnly));
    }

    #[test]
    fn check_path_full_grants_everything() {
        let policy = SandboxPolicy::deny_all().with_path("/workspace", PathAccess::Full);
        assert!(policy.check_path(Path::new("/workspace/a"), PathAccess::ReadOnly));
        assert!(policy.check_path(Path::new("/workspace/a"), PathAccess::ReadWrite));
        assert!(policy.check_path(Path::new("/workspace/a"), PathAccess::Full));
    }

    #[test]
    fn check_path_empty_policy_denies_all() {
        let policy = SandboxPolicy::deny_all();
        assert!(!policy.check_path(Path::new("/tmp"), PathAccess::ReadOnly));
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
        assert!(!access_sufficient(PathAccess::ReadOnly, PathAccess::Full));

        assert!(access_sufficient(
            PathAccess::ReadWrite,
            PathAccess::ReadOnly
        ));
        assert!(access_sufficient(
            PathAccess::ReadWrite,
            PathAccess::ReadWrite
        ));
        assert!(!access_sufficient(PathAccess::ReadWrite, PathAccess::Full));

        assert!(access_sufficient(PathAccess::Full, PathAccess::ReadOnly));
        assert!(access_sufficient(PathAccess::Full, PathAccess::ReadWrite));
        assert!(access_sufficient(PathAccess::Full, PathAccess::Full));
    }

    #[test]
    fn summary_deny_all() {
        let policy = SandboxPolicy::deny_all();
        let summary = policy.summary();
        assert!(summary.contains("no filesystem access"));
        assert!(summary.contains("network:no"));
        assert!(summary.contains("subprocess:no"));
    }

    #[test]
    fn summary_with_paths() {
        let policy = SandboxPolicy::deny_all()
            .with_path("/tmp", PathAccess::ReadWrite)
            .with_network(true)
            .with_max_memory(1024 * 1024 * 1024);
        let summary = policy.summary();
        assert!(summary.contains("read_write"));
        assert!(summary.contains("network:yes"));
        assert!(summary.contains("mem:1024MB"));
    }

    #[test]
    fn policy_serde_roundtrip() {
        let policy = SandboxPolicy::deny_all()
            .with_path("/tmp", PathAccess::ReadWrite)
            .with_network(true)
            .with_max_cpu(30);
        let json = serde_json::to_string(&policy).unwrap();
        let restored: SandboxPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.path_rules.len(), 1);
        assert!(restored.allow_network);
        assert_eq!(restored.max_cpu_seconds, 30);
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

    #[test]
    fn check_path_multiple_rules_first_match_wins() {
        let policy = SandboxPolicy::deny_all()
            .with_path("/home", PathAccess::ReadOnly)
            .with_path("/home/user/work", PathAccess::ReadWrite);
        assert!(policy.check_path(Path::new("/home/user/work/file.rs"), PathAccess::ReadWrite));
        assert!(policy.check_path(Path::new("/home/other/file.rs"), PathAccess::ReadOnly));
        assert!(!policy.check_path(Path::new("/home/other/file.rs"), PathAccess::ReadWrite));
    }

    #[test]
    fn policy_default_trait() {
        let p = SandboxPolicy::default();
        assert!(p.path_rules.is_empty());
        assert!(!p.allow_network);
    }
}
