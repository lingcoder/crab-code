//! [`PermissionPolicy`] — the user-configured whitelist / blacklist of
//! tools that gate execution in concert with [`super::PermissionMode`].

use serde::{Deserialize, Serialize};

use super::filter::{glob_match, matches_tool_filter, tool_name_matches_pattern};
use super::mode::PermissionMode;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionPolicy {
    pub mode: PermissionMode,
    pub allowed_tools: Vec<String>,
    /// Supports glob pattern matching (e.g. `mcp__*`, `Bash`).
    pub denied_tools: Vec<String>,
}

impl Default for PermissionPolicy {
    fn default() -> Self {
        Self {
            mode: PermissionMode::Default,
            allowed_tools: Vec::new(),
            denied_tools: Vec::new(),
        }
    }
}

impl PermissionPolicy {
    /// Check whether a tool name matches any `denied_tools` glob pattern.
    pub fn is_denied(&self, tool_name: &str) -> bool {
        self.denied_tools
            .iter()
            .any(|pattern| glob_match(pattern, tool_name))
    }

    /// Check whether a tool name is in the `allowed_tools` list.
    ///
    /// When `allowed_tools` is non-empty, it acts as a whitelist: only
    /// tools matching at least one pattern are permitted. Supports glob
    /// patterns and parameter-level matching via
    /// [`super::filter::matches_tool_filter`].
    pub fn is_explicitly_allowed(&self, tool_name: &str) -> bool {
        self.allowed_tools
            .iter()
            .any(|pattern| tool_name_matches_pattern(pattern, tool_name))
    }

    /// Check whether a tool invocation is allowed by the `allowed_tools`
    /// whitelist, using full glob + parameter matching.
    ///
    /// Returns `true` if `allowed_tools` is empty (no whitelist) or if
    /// the tool matches at least one allowed pattern.
    pub fn is_allowed_by_whitelist(&self, tool_name: &str, tool_input: &serde_json::Value) -> bool {
        if self.allowed_tools.is_empty() {
            return true; // no whitelist = everything allowed
        }
        self.allowed_tools
            .iter()
            .any(|pattern| matches_tool_filter(pattern, tool_name, tool_input))
    }

    /// Check whether a tool invocation is denied by the `denied_tools`
    /// blacklist, using full glob + parameter matching.
    pub fn is_denied_by_filter(&self, tool_name: &str, tool_input: &serde_json::Value) -> bool {
        self.denied_tools
            .iter()
            .any(|pattern| matches_tool_filter(pattern, tool_name, tool_input))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_default() {
        let policy = PermissionPolicy::default();
        assert_eq!(policy.mode, PermissionMode::Default);
        assert!(policy.allowed_tools.is_empty());
        assert!(policy.denied_tools.is_empty());
    }

    #[test]
    fn policy_is_denied_exact() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec![],
            denied_tools: vec!["bash".to_string()],
        };
        assert!(policy.is_denied("bash"));
        assert!(!policy.is_denied("read"));
    }

    #[test]
    fn policy_is_denied_glob_star() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec![],
            denied_tools: vec!["mcp__*".to_string()],
        };
        assert!(policy.is_denied("mcp__playwright_click"));
        assert!(policy.is_denied("mcp__"));
        assert!(!policy.is_denied("bash"));
    }

    #[test]
    fn policy_is_denied_glob_question() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec![],
            denied_tools: vec!["tool_?".to_string()],
        };
        assert!(policy.is_denied("tool_a"));
        assert!(policy.is_denied("tool_1"));
        assert!(!policy.is_denied("tool_ab"));
        assert!(!policy.is_denied("tool_"));
    }

    #[test]
    fn policy_is_denied_glob_char_class() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec![],
            denied_tools: vec!["tool_[abc]".to_string()],
        };
        assert!(policy.is_denied("tool_a"));
        assert!(policy.is_denied("tool_b"));
        assert!(policy.is_denied("tool_c"));
        assert!(!policy.is_denied("tool_d"));
    }

    #[test]
    fn policy_is_denied_glob_char_range() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec![],
            denied_tools: vec!["v[0-9]".to_string()],
        };
        assert!(policy.is_denied("v0"));
        assert!(policy.is_denied("v9"));
        assert!(!policy.is_denied("va"));
    }

    #[test]
    fn policy_is_explicitly_allowed() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec!["read".to_string(), "glob".to_string()],
            denied_tools: vec![],
        };
        assert!(policy.is_explicitly_allowed("read"));
        assert!(policy.is_explicitly_allowed("glob"));
        assert!(!policy.is_explicitly_allowed("bash"));
    }

    #[test]
    fn policy_serde_roundtrip() {
        let policy = PermissionPolicy {
            mode: PermissionMode::TrustProject,
            allowed_tools: vec!["read".into(), "write".into()],
            denied_tools: vec!["mcp__*".into()],
        };
        let json = serde_json::to_string(&policy).unwrap();
        let parsed: PermissionPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.mode, PermissionMode::TrustProject);
        assert_eq!(parsed.allowed_tools, vec!["read", "write"]);
        assert_eq!(parsed.denied_tools, vec!["mcp__*"]);
    }

    #[test]
    fn policy_denied_takes_priority_concept() {
        // If a tool is both allowed and denied, is_denied should still return true.
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec!["bash".into()],
            denied_tools: vec!["bash".into()],
        };
        assert!(policy.is_denied("bash"));
        assert!(policy.is_explicitly_allowed("bash"));
    }

    #[test]
    fn policy_empty_denied_allows_everything() {
        let policy = PermissionPolicy::default();
        assert!(!policy.is_denied("bash"));
        assert!(!policy.is_denied("read"));
        assert!(!policy.is_denied("mcp__anything"));
    }

    #[test]
    fn policy_multiple_denied_patterns() {
        let policy = PermissionPolicy {
            mode: PermissionMode::TrustProject,
            allowed_tools: vec![],
            denied_tools: vec!["bash".into(), "mcp__*".into(), "dangerous_[a-z]".into()],
        };
        assert!(policy.is_denied("bash"));
        assert!(policy.is_denied("mcp__server__tool"));
        assert!(policy.is_denied("dangerous_x"));
        assert!(!policy.is_denied("read"));
        assert!(!policy.is_denied("dangerous_1"));
    }

    #[test]
    fn whitelist_empty_allows_all() {
        let policy = PermissionPolicy::default();
        let input = serde_json::json!({});
        assert!(policy.is_allowed_by_whitelist("bash", &input));
        assert!(policy.is_allowed_by_whitelist("read", &input));
    }

    #[test]
    fn whitelist_exact_match() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec!["read".into(), "write".into()],
            denied_tools: vec![],
        };
        let input = serde_json::json!({});
        assert!(policy.is_allowed_by_whitelist("read", &input));
        assert!(policy.is_allowed_by_whitelist("write", &input));
        assert!(!policy.is_allowed_by_whitelist("bash", &input));
    }

    #[test]
    fn whitelist_glob_pattern() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec!["mcp__*".into()],
            denied_tools: vec![],
        };
        let input = serde_json::json!({});
        assert!(policy.is_allowed_by_whitelist("mcp__server__tool", &input));
        assert!(!policy.is_allowed_by_whitelist("bash", &input));
    }

    #[test]
    fn whitelist_param_pattern() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec!["Bash(command:git*)".into()],
            denied_tools: vec![],
        };
        let git_input = serde_json::json!({"command": "git status"});
        let rm_input = serde_json::json!({"command": "rm -rf /"});
        assert!(policy.is_allowed_by_whitelist("Bash", &git_input));
        assert!(!policy.is_allowed_by_whitelist("Bash", &rm_input));
    }

    #[test]
    fn denied_filter_exact() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec![],
            denied_tools: vec!["bash".into()],
        };
        let input = serde_json::json!({});
        assert!(policy.is_denied_by_filter("bash", &input));
        assert!(!policy.is_denied_by_filter("read", &input));
    }

    #[test]
    fn denied_filter_glob() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec![],
            denied_tools: vec!["mcp__*".into()],
        };
        let input = serde_json::json!({});
        assert!(policy.is_denied_by_filter("mcp__server__tool", &input));
        assert!(!policy.is_denied_by_filter("bash", &input));
    }

    #[test]
    fn denied_filter_param_pattern() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec![],
            denied_tools: vec!["Bash(command:rm*)".into()],
        };
        let rm_input = serde_json::json!({"command": "rm -rf /"});
        let ls_input = serde_json::json!({"command": "ls -la"});
        assert!(policy.is_denied_by_filter("Bash", &rm_input));
        assert!(!policy.is_denied_by_filter("Bash", &ls_input));
    }
}
