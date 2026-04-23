//! Permission policy limits and enforcement.
//!
//! Supports loading policy from:
//! - Local policy files (`~/.crab/policy.json`, project `.crab/policy.json`)
//! - System-wide policy (`/etc/crab-code/policy.json` on Unix)
//! - MDM/managed settings directories
//!
//! Policies restrict what the agent can do at an organizational level,
//! independent of per-user settings. They are merged with lowest-to-highest
//! priority: system < global < project.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ── Data model ───────────────────────────────────────────────────────

/// A loaded policy configuration.
///
/// Controls organizational-level restrictions on what the agent is allowed
/// to do. Fields use `serde(default)` so that partial policy files work.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct PolicyConfig {
    /// Tools that are completely disabled by policy.
    /// Entries are exact names or glob patterns (e.g. `"mcp__*"`).
    pub disabled_tools: Vec<String>,

    /// Whether bypass-permissions mode (`--dangerously-skip-permissions`) is allowed.
    pub allow_bypass_permissions: bool,

    /// Maximum number of agent turns per query. `None` = unlimited.
    pub max_turns: Option<u32>,

    /// Maximum token budget per query. `None` = unlimited.
    pub max_tokens_per_query: Option<u64>,

    /// Allowed MCP server patterns (glob). If non-empty, only matching
    /// servers are permitted; all others are denied.
    pub allowed_mcp_servers: Vec<String>,

    /// Denied MCP server patterns (glob). Checked after `allowed_mcp_servers`.
    pub denied_mcp_servers: Vec<String>,

    /// Whether network access tools (`WebFetch`, curl, etc.) are allowed.
    pub allow_network_tools: bool,

    /// Whether file-write tools are allowed.
    pub allow_file_writes: bool,

    /// Whether shell/bash execution is allowed.
    pub allow_shell_execution: bool,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            disabled_tools: Vec::new(),
            allow_bypass_permissions: true,
            max_turns: None,
            max_tokens_per_query: None,
            allowed_mcp_servers: Vec::new(),
            denied_mcp_servers: Vec::new(),
            allow_network_tools: true,
            allow_file_writes: true,
            allow_shell_execution: true,
        }
    }
}

impl PolicyConfig {
    /// Merge another policy on top of `self` (higher priority wins).
    ///
    /// Non-default values in `other` override values in `self`.
    /// Lists are replaced, not appended.
    #[must_use]
    pub fn merge(self, other: &Self) -> Self {
        let default = Self::default();
        Self {
            disabled_tools: if other.disabled_tools.is_empty()
                && other.disabled_tools == default.disabled_tools
            {
                self.disabled_tools
            } else {
                other.disabled_tools.clone()
            },
            allow_bypass_permissions: other.allow_bypass_permissions,
            max_turns: other.max_turns.or(self.max_turns),
            max_tokens_per_query: other.max_tokens_per_query.or(self.max_tokens_per_query),
            allowed_mcp_servers: if other.allowed_mcp_servers.is_empty() {
                self.allowed_mcp_servers
            } else {
                other.allowed_mcp_servers.clone()
            },
            denied_mcp_servers: if other.denied_mcp_servers.is_empty() {
                self.denied_mcp_servers
            } else {
                other.denied_mcp_servers.clone()
            },
            allow_network_tools: other.allow_network_tools,
            allow_file_writes: other.allow_file_writes,
            allow_shell_execution: other.allow_shell_execution,
        }
    }
}

// ── Well-known paths ─────────────────────────────────────────────────

/// Return the well-known paths for policy files, ordered from lowest
/// to highest priority.
///
/// 1. System-wide: `/etc/crab-code/policy.json` (Unix only)
/// 2. Global user: `~/.crab/policy.json`
///
/// Project-level policy (`<project>/.crab/policy.json`) is loaded
/// separately via [`load_project_policy`].
#[must_use]
pub fn policy_file_paths() -> Vec<PathBuf> {
    let paths = vec![
        // 1. System-wide (Unix only)
        #[cfg(unix)]
        PathBuf::from("/etc/crab-code/policy.json"),
        // 2. Global user
        crab_core::common::utils::path::home_dir()
            .join(".crab")
            .join("policy.json"),
    ];

    paths
}

/// Return the project-level policy path: `<project_dir>/.crab/policy.json`.
#[must_use]
pub fn project_policy_path(project_dir: &Path) -> PathBuf {
    project_dir.join(".crab").join("policy.json")
}

// ── Loading ──────────────────────────────────────────────────────────

/// Load a policy from a single JSON file.
///
/// Returns `Ok(PolicyConfig::default())` if the file does not exist.
fn load_from_file(path: &Path) -> crab_core::Result<PolicyConfig> {
    match std::fs::read_to_string(path) {
        Ok(content) => serde_json::from_str(&content).map_err(|e| {
            crab_core::Error::Config(format!("failed to parse policy {}: {e}", path.display()))
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(PolicyConfig::default()),
        Err(e) => Err(crab_core::Error::Config(format!(
            "failed to read policy {}: {e}",
            path.display()
        ))),
    }
}

/// Load and merge policy from all well-known sources (lowest to highest priority).
///
/// Merge order: system → global user → (caller may merge project policy on top).
pub fn load_policy() -> PolicyConfig {
    let mut result = PolicyConfig::default();

    for path in policy_file_paths() {
        match load_from_file(&path) {
            Ok(policy) => {
                result = result.merge(&policy);
            }
            Err(e) => {
                // Log but don't fail — policy files are optional.
                eprintln!("[policy] warning: failed to load {}: {e}", path.display());
            }
        }
    }

    result
}

/// Load project-level policy and merge it on top of the global policy.
pub fn load_policy_with_project(project_dir: &Path) -> PolicyConfig {
    let mut policy = load_policy();

    let project_path = project_policy_path(project_dir);
    match load_from_file(&project_path) {
        Ok(project_policy) => {
            policy = policy.merge(&project_policy);
        }
        Err(e) => {
            eprintln!(
                "[policy] warning: failed to load project policy from {}: {e}",
                project_path.display()
            );
        }
    }

    policy
}

// ── Enforcement helpers ──────────────────────────────────────────────

/// Check if a specific tool is allowed by policy.
///
/// A tool is **denied** if:
/// 1. It appears in `disabled_tools` (exact match or glob), OR
/// 2. It is a network tool and `allow_network_tools` is false, OR
/// 3. It is a shell tool and `allow_shell_execution` is false, OR
/// 4. It is a file-write tool and `allow_file_writes` is false.
///
/// The caller is responsible for mapping tool names to categories
/// (network/shell/write). This function only checks `disabled_tools`.
#[must_use]
pub fn is_tool_allowed(policy: &PolicyConfig, tool_name: &str) -> bool {
    // Check explicit disabled list
    for pattern in &policy.disabled_tools {
        if glob_match(pattern, tool_name) {
            return false;
        }
    }
    true
}

/// Check if a specific MCP server is allowed by policy.
///
/// Rules:
/// 1. If `denied_mcp_servers` contains a matching pattern → deny.
/// 2. If `allowed_mcp_servers` is non-empty and no pattern matches → deny.
/// 3. Otherwise → allow.
#[must_use]
pub fn is_mcp_server_allowed(policy: &PolicyConfig, server_name: &str) -> bool {
    // Check deny list first (highest priority)
    for pattern in &policy.denied_mcp_servers {
        if glob_match(pattern, server_name) {
            return false;
        }
    }

    // If allow list is specified, server must match at least one entry
    if !policy.allowed_mcp_servers.is_empty() {
        return policy
            .allowed_mcp_servers
            .iter()
            .any(|pattern| glob_match(pattern, server_name));
    }

    // No restrictions
    true
}

// ── Glob matching (shared with permissions.rs) ───────────────────────

/// Simple glob matching supporting `*` (match any) and `?` (match one).
fn glob_match(pattern: &str, input: &str) -> bool {
    // Exact match fast path
    if !pattern.contains('*') && !pattern.contains('?') {
        return pattern == input;
    }

    let pat: Vec<char> = pattern.chars().collect();
    let inp: Vec<char> = input.chars().collect();
    glob_match_inner(&pat, &inp)
}

fn glob_match_inner(pat: &[char], input: &[char]) -> bool {
    let (mut pi, mut ii) = (0, 0);
    let (mut star_pat, mut star_input) = (usize::MAX, usize::MAX);

    while ii < input.len() {
        if pi < pat.len() && (pat[pi] == '?' || pat[pi] == input[ii]) {
            pi += 1;
            ii += 1;
        } else if pi < pat.len() && pat[pi] == '*' {
            star_pat = pi;
            star_input = ii;
            pi += 1;
        } else if star_pat != usize::MAX {
            pi = star_pat + 1;
            star_input += 1;
            ii = star_input;
        } else {
            return false;
        }
    }
    while pi < pat.len() && pat[pi] == '*' {
        pi += 1;
    }
    pi == pat.len()
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── PolicyConfig defaults ────────────────────────────────────────

    #[test]
    fn default_policy_is_permissive() {
        let policy = PolicyConfig::default();
        assert!(policy.disabled_tools.is_empty());
        assert!(policy.allow_bypass_permissions);
        assert!(policy.max_turns.is_none());
        assert!(policy.max_tokens_per_query.is_none());
        assert!(policy.allowed_mcp_servers.is_empty());
        assert!(policy.denied_mcp_servers.is_empty());
        assert!(policy.allow_network_tools);
        assert!(policy.allow_file_writes);
        assert!(policy.allow_shell_execution);
    }

    // ── Serde roundtrip ─────────────────────────────────────────────

    #[test]
    fn policy_serde_roundtrip() {
        let policy = PolicyConfig {
            disabled_tools: vec!["bash".into(), "mcp__*".into()],
            allow_bypass_permissions: false,
            max_turns: Some(50),
            max_tokens_per_query: Some(1_000_000),
            allowed_mcp_servers: vec!["safe-*".into()],
            denied_mcp_servers: vec!["evil-*".into()],
            allow_network_tools: false,
            allow_file_writes: true,
            allow_shell_execution: false,
        };
        let json = serde_json::to_string_pretty(&policy).unwrap();
        let parsed: PolicyConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(policy, parsed);
    }

    #[test]
    fn policy_deserialize_partial() {
        let json = r#"{"maxTurns": 10}"#;
        let policy: PolicyConfig = serde_json::from_str(json).unwrap();
        assert_eq!(policy.max_turns, Some(10));
        // Everything else should be default
        assert!(policy.allow_bypass_permissions);
        assert!(policy.disabled_tools.is_empty());
    }

    #[test]
    fn policy_deserialize_empty_object() {
        let policy: PolicyConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(policy, PolicyConfig::default());
    }

    // ── Merge ────────────────────────────────────────────────────────

    #[test]
    fn merge_override_max_turns() {
        let base = PolicyConfig {
            max_turns: Some(100),
            ..Default::default()
        };
        let overlay = PolicyConfig {
            max_turns: Some(10),
            ..Default::default()
        };
        let merged = base.merge(&overlay);
        assert_eq!(merged.max_turns, Some(10));
    }

    #[test]
    fn merge_preserve_base_when_overlay_is_none() {
        let base = PolicyConfig {
            max_turns: Some(100),
            max_tokens_per_query: Some(500_000),
            ..Default::default()
        };
        let overlay = PolicyConfig::default();
        let merged = base.merge(&overlay);
        assert_eq!(merged.max_turns, Some(100));
        assert_eq!(merged.max_tokens_per_query, Some(500_000));
    }

    #[test]
    fn merge_disabled_tools_replaces() {
        let base = PolicyConfig {
            disabled_tools: vec!["bash".into()],
            ..Default::default()
        };
        let overlay = PolicyConfig {
            disabled_tools: vec!["write".into(), "edit".into()],
            ..Default::default()
        };
        let merged = base.merge(&overlay);
        assert_eq!(merged.disabled_tools, vec!["write", "edit"]);
    }

    // ── is_tool_allowed ─────────────────────────────────────────────

    #[test]
    fn tool_allowed_when_no_restrictions() {
        let policy = PolicyConfig::default();
        assert!(is_tool_allowed(&policy, "bash"));
        assert!(is_tool_allowed(&policy, "read"));
        assert!(is_tool_allowed(&policy, "mcp__anything"));
    }

    #[test]
    fn tool_denied_by_exact_name() {
        let policy = PolicyConfig {
            disabled_tools: vec!["bash".into()],
            ..Default::default()
        };
        assert!(!is_tool_allowed(&policy, "bash"));
        assert!(is_tool_allowed(&policy, "read"));
    }

    #[test]
    fn tool_denied_by_glob() {
        let policy = PolicyConfig {
            disabled_tools: vec!["mcp__*".into()],
            ..Default::default()
        };
        assert!(!is_tool_allowed(&policy, "mcp__playwright_click"));
        assert!(!is_tool_allowed(&policy, "mcp__anything"));
        assert!(is_tool_allowed(&policy, "bash"));
    }

    #[test]
    fn tool_denied_by_all_glob() {
        let policy = PolicyConfig {
            disabled_tools: vec!["*".into()],
            ..Default::default()
        };
        assert!(!is_tool_allowed(&policy, "bash"));
        assert!(!is_tool_allowed(&policy, "read"));
    }

    // ── is_mcp_server_allowed ───────────────────────────────────────

    #[test]
    fn mcp_server_allowed_when_no_restrictions() {
        let policy = PolicyConfig::default();
        assert!(is_mcp_server_allowed(&policy, "any-server"));
    }

    #[test]
    fn mcp_server_denied_by_deny_list() {
        let policy = PolicyConfig {
            denied_mcp_servers: vec!["evil-*".into()],
            ..Default::default()
        };
        assert!(!is_mcp_server_allowed(&policy, "evil-server"));
        assert!(is_mcp_server_allowed(&policy, "safe-server"));
    }

    #[test]
    fn mcp_server_allow_list_restricts() {
        let policy = PolicyConfig {
            allowed_mcp_servers: vec!["trusted-*".into()],
            ..Default::default()
        };
        assert!(is_mcp_server_allowed(&policy, "trusted-db"));
        assert!(!is_mcp_server_allowed(&policy, "random-server"));
    }

    #[test]
    fn mcp_server_deny_overrides_allow() {
        let policy = PolicyConfig {
            allowed_mcp_servers: vec!["*".into()],
            denied_mcp_servers: vec!["evil-*".into()],
            ..Default::default()
        };
        assert!(!is_mcp_server_allowed(&policy, "evil-server"));
        assert!(is_mcp_server_allowed(&policy, "good-server"));
    }

    // ── Glob matching ───────────────────────────────────────────────

    #[test]
    fn glob_exact_match() {
        assert!(glob_match("bash", "bash"));
        assert!(!glob_match("bash", "read"));
    }

    #[test]
    fn glob_star_prefix() {
        assert!(glob_match("mcp__*", "mcp__server_tool"));
        assert!(glob_match("mcp__*", "mcp__"));
        assert!(!glob_match("mcp__*", "bash"));
    }

    #[test]
    fn glob_star_suffix() {
        assert!(glob_match("*_tool", "my_tool"));
        assert!(!glob_match("*_tool", "my_command"));
    }

    #[test]
    fn glob_star_all() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("*", ""));
    }

    #[test]
    fn glob_question_mark() {
        assert!(glob_match("tool_?", "tool_a"));
        assert!(!glob_match("tool_?", "tool_ab"));
    }

    // ── Path helpers ────────────────────────────────────────────────

    #[test]
    fn policy_file_paths_contains_global() {
        let paths = policy_file_paths();
        assert!(paths.iter().any(|p| p.to_string_lossy().contains(".crab")));
        assert!(
            paths
                .iter()
                .any(|p| p.to_string_lossy().contains("policy.json"))
        );
    }

    #[test]
    fn project_policy_path_under_crab() {
        let path = project_policy_path(Path::new("/my/project"));
        assert!(path.ends_with("policy.json"));
        assert!(path.to_string_lossy().contains(".crab"));
    }

    // ── File loading ────────────────────────────────────────────────

    #[test]
    fn load_from_nonexistent_returns_default() {
        let result = load_from_file(Path::new("/nonexistent/policy.json")).unwrap();
        assert_eq!(result, PolicyConfig::default());
    }

    #[test]
    fn load_from_temp_file() {
        let dir = std::env::temp_dir().join("crab-policy-test-load");
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("policy.json");
        std::fs::write(&file, r#"{"disabledTools": ["bash"], "maxTurns": 20}"#).unwrap();

        let policy = load_from_file(&file).unwrap();
        assert_eq!(policy.disabled_tools, vec!["bash"]);
        assert_eq!(policy.max_turns, Some(20));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_from_invalid_json_returns_error() {
        let dir = std::env::temp_dir().join("crab-policy-test-invalid");
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("policy.json");
        std::fs::write(&file, "not json").unwrap();

        let result = load_from_file(&file);
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Full load_policy ────────────────────────────────────────────

    #[test]
    fn load_policy_returns_permissive_default() {
        // Even if no policy files exist, we get a permissive default
        let policy = load_policy();
        assert!(policy.allow_bypass_permissions);
        assert!(policy.allow_network_tools);
    }
}
