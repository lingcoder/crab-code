use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionMode {
    /// All non-read-only tools require user confirmation.
    Default,
    /// Auto-approve file edits within the project; other mutations still prompt.
    AcceptEdits,
    /// Trust in-project file operations; out-of-project and dangerous still prompt.
    TrustProject,
    /// Auto-approve everything without prompting the user.
    DontAsk,
    /// Auto-approve everything (except `denied_tools`). Use with caution.
    Dangerously,
    /// Planning-only mode: the agent may read but not mutate.
    Plan,
}

impl fmt::Display for PermissionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Default => f.write_str("default"),
            Self::AcceptEdits => f.write_str("acceptEdits"),
            Self::TrustProject => f.write_str("trust-project"),
            Self::DontAsk => f.write_str("dontAsk"),
            Self::Dangerously => f.write_str("dangerously"),
            Self::Plan => f.write_str("plan"),
        }
    }
}

impl FromStr for PermissionMode {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "default" => Ok(Self::Default),
            "acceptEdits" | "accept-edits" | "accept_edits" => Ok(Self::AcceptEdits),
            "trust-project" | "trust_project" => Ok(Self::TrustProject),
            "dontAsk" | "dont-ask" | "dont_ask" => Ok(Self::DontAsk),
            "bypassPermissions" | "bypass-permissions" | "bypass_permissions" | "dangerously" => {
                Ok(Self::Dangerously)
            }
            "plan" => Ok(Self::Plan),
            other => Err(format!("unknown permission mode: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionPolicy {
    pub mode: PermissionMode,
    pub allowed_tools: Vec<String>,
    /// Supports glob pattern matching (e.g. "mcp__*", "bash").
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
    /// When `allowed_tools` is non-empty, it acts as a whitelist: only tools
    /// matching at least one pattern are permitted. Supports glob patterns
    /// and parameter-level matching via `matches_tool_filter`.
    pub fn is_explicitly_allowed(&self, tool_name: &str) -> bool {
        self.allowed_tools.iter().any(|a| a == tool_name)
    }

    /// Check whether a tool invocation is allowed by the `allowed_tools`
    /// whitelist, using full glob + parameter matching.
    ///
    /// Returns `true` if `allowed_tools` is empty (no whitelist) or if the
    /// tool matches at least one allowed pattern.
    pub fn is_allowed_by_whitelist(
        &self,
        tool_name: &str,
        tool_input: &serde_json::Value,
    ) -> bool {
        if self.allowed_tools.is_empty() {
            return true; // no whitelist = everything allowed
        }
        self.allowed_tools
            .iter()
            .any(|pattern| matches_tool_filter(pattern, tool_name, tool_input))
    }

    /// Check whether a tool invocation is denied by the `denied_tools`
    /// blacklist, using full glob + parameter matching.
    pub fn is_denied_by_filter(
        &self,
        tool_name: &str,
        tool_input: &serde_json::Value,
    ) -> bool {
        self.denied_tools
            .iter()
            .any(|pattern| matches_tool_filter(pattern, tool_name, tool_input))
    }
}

/// Result of a permission check for a tool invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    /// Tool execution is allowed without user interaction.
    Allow,
    /// Tool execution is denied; includes the reason.
    Deny(String),
    /// Tool execution requires user confirmation; includes a prompt message.
    AskUser(String),
}

/// Check whether a tool filter matches a tool invocation.
///
/// Supports the following filter formats:
/// - `*` — matches any tool
/// - `Bash` — matches all bash invocations
/// - `Bash(git:*)` — matches bash invocations where the `command` field starts with "git"
/// - `Edit` — exact tool name match
///
/// The `tool_input` is the JSON input object passed to the tool (used for
/// parameter-level matching like `Bash(git:*)`).
pub fn matches_tool_filter(filter: &str, tool_name: &str, tool_input: &serde_json::Value) -> bool {
    let filter = filter.trim();

    // Wildcard: match everything
    if filter == "*" {
        return true;
    }

    // Check for Name(pattern) format
    if let Some(paren_start) = filter.find('(') && filter.ends_with(')') {
        let name_part = &filter[..paren_start];
        let pattern_part = &filter[paren_start + 1..filter.len() - 1];

        // Tool name must match
        if !glob_match(name_part, tool_name) {
            return false;
        }

        // Parse the parameter constraint: "key:pattern"
        if let Some(colon_pos) = pattern_part.find(':') {
            let key = &pattern_part[..colon_pos];
            let value_pattern = &pattern_part[colon_pos + 1..];

            // Look up the key in tool_input
            if let Some(value) = tool_input.get(key) {
                let value_str = match value {
                    serde_json::Value::String(s) => s.as_str(),
                    _ => return false,
                };
                return glob_match(value_pattern, value_str);
            }
            return false;
        }

        // No colon — just name match (weird format but handle gracefully)
        return true;
    }

    // Plain name match (may contain globs)
    glob_match(filter, tool_name)
}

/// Simple glob matching supporting `*` (any chars), `?` (single char),
/// and `[abc]` (character class). This avoids pulling in globset for
/// a small pattern set used only in permission checks.
fn glob_match(pattern: &str, input: &str) -> bool {
    let pat_chars: Vec<char> = pattern.chars().collect();
    let input_chars: Vec<char> = input.chars().collect();
    glob_match_inner(&pat_chars, &input_chars)
}

fn glob_match_inner(pat: &[char], input: &[char]) -> bool {
    let (mut pi, mut ii) = (0, 0);
    let (mut star_pat, mut star_input) = (usize::MAX, usize::MAX);

    while ii < input.len() {
        if pi < pat.len() && pat[pi] == '[' {
            // Character class
            if let Some((matched, end)) = match_char_class(&pat[pi..], input[ii])
                && matched
            {
                pi += end;
                ii += 1;
                continue;
            }
            // No match in class -- fall through to star backtrack
            if star_pat != usize::MAX {
                pi = star_pat + 1;
                star_input += 1;
                ii = star_input;
                continue;
            }
            return false;
        } else if pi < pat.len() && (pat[pi] == '?' || pat[pi] == input[ii]) {
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

/// Match a `[abc]` or `[a-z]` character class. Returns (matched, consumed count) or `None` if malformed.
fn match_char_class(pat: &[char], ch: char) -> Option<(bool, usize)> {
    if pat.is_empty() || pat[0] != '[' {
        return None;
    }
    let mut i = 1;
    let mut matched = false;
    while i < pat.len() && pat[i] != ']' {
        if i + 2 < pat.len() && pat[i + 1] == '-' && pat[i + 2] != ']' {
            if ch >= pat[i] && ch <= pat[i + 2] {
                matched = true;
            }
            i += 3;
        } else {
            if ch == pat[i] {
                matched = true;
            }
            i += 1;
        }
    }
    if i < pat.len() && pat[i] == ']' {
        Some((matched, i + 1))
    } else {
        None // Malformed: no closing bracket
    }
}

// ─── Auto-mode: AI/heuristic risk classification ──────────────────────

/// Risk level for a tool invocation, used by auto-mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    /// Read-only, no side effects (auto-approve).
    Safe,
    /// Side effects but recoverable (prompt the user).
    Risky,
    /// Destructive or irreversible (deny or prompt with strong warning).
    Dangerous,
}

/// Heuristic risk classifier for auto-mode permission decisions.
///
/// Classifies tool invocations based on tool name, read-only status,
/// and input patterns. This is the fallback when the LLM classifier
/// is unavailable.
pub struct AutoModeClassifier;

/// Read-only tools that are always safe.
const SAFE_TOOLS: &[&str] = &[
    "read",
    "glob",
    "grep",
    "notebook_read",
    "list_directory",
    "get_diagnostics",
];

/// Write tools that are risky but not dangerous.
const RISKY_TOOLS: &[&str] = &["write", "edit", "notebook_edit", "bash"];

/// Dangerous command patterns for auto-mode (superset of permission.rs patterns).
const AUTO_DANGEROUS_PATTERNS: &[&str] = &[
    "rm -rf",
    "rm -fr",
    "sudo ",
    "| sh",
    "|sh",
    "| bash",
    "|bash",
    "chmod ",
    "chown ",
    "eval ",
    "mkfs",
    "> /dev/",
    "dd if=",
    ":(){ :|:& };:",
    "git push --force",
    "git push -f",
    "git reset --hard",
    "git clean -f",
    "DROP TABLE",
    "DROP DATABASE",
    "TRUNCATE ",
    "kill -9",
    "pkill ",
];

impl AutoModeClassifier {
    /// Classify the risk level of a tool invocation using heuristics.
    pub fn classify(
        tool_name: &str,
        is_read_only: bool,
        input: &serde_json::Value,
    ) -> RiskLevel {
        // Read-only tools are always safe
        if is_read_only || SAFE_TOOLS.contains(&tool_name) {
            return RiskLevel::Safe;
        }

        // Check for dangerous patterns in input
        let command_text = input
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or_default();

        if !command_text.is_empty()
            && AUTO_DANGEROUS_PATTERNS
                .iter()
                .any(|p| command_text.contains(p))
        {
            return RiskLevel::Dangerous;
        }

        // MCP tools are risky by default (external, untrusted)
        if tool_name.starts_with("mcp__") {
            return RiskLevel::Risky;
        }

        // Known risky tools
        if RISKY_TOOLS.contains(&tool_name) {
            return RiskLevel::Risky;
        }

        // Unknown tools default to risky
        RiskLevel::Risky
    }
}

/// Make a permission decision using auto-mode classification.
///
/// - Safe → Allow
/// - Risky → `AskUser`
/// - Dangerous → Deny
pub fn auto_mode_decision(
    policy: &PermissionPolicy,
    tool_name: &str,
    is_read_only: bool,
    input: &serde_json::Value,
) -> PermissionDecision {
    // Denied list still applies
    if policy.is_denied_by_filter(tool_name, input) {
        return PermissionDecision::Deny(format!("tool '{tool_name}' is denied by policy"));
    }

    // Whitelist still applies
    if !policy.allowed_tools.is_empty() && !policy.is_allowed_by_whitelist(tool_name, input) {
        return PermissionDecision::Deny(format!(
            "tool '{tool_name}' is not in the allowed tools list"
        ));
    }

    let risk = AutoModeClassifier::classify(tool_name, is_read_only, input);
    match risk {
        RiskLevel::Safe => PermissionDecision::Allow,
        RiskLevel::Risky => {
            PermissionDecision::AskUser(format!("Auto-mode: '{tool_name}' classified as risky"))
        }
        RiskLevel::Dangerous => PermissionDecision::Deny(format!(
            "Auto-mode: '{tool_name}' classified as dangerous and blocked"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_mode_display() {
        assert_eq!(PermissionMode::Default.to_string(), "default");
        assert_eq!(PermissionMode::AcceptEdits.to_string(), "acceptEdits");
        assert_eq!(PermissionMode::TrustProject.to_string(), "trust-project");
        assert_eq!(PermissionMode::DontAsk.to_string(), "dontAsk");
        assert_eq!(PermissionMode::Dangerously.to_string(), "dangerously");
        assert_eq!(PermissionMode::Plan.to_string(), "plan");
    }

    #[test]
    fn permission_mode_from_str() {
        assert_eq!("default".parse::<PermissionMode>().unwrap(), PermissionMode::Default);
        assert_eq!("acceptEdits".parse::<PermissionMode>().unwrap(), PermissionMode::AcceptEdits);
        assert_eq!("accept-edits".parse::<PermissionMode>().unwrap(), PermissionMode::AcceptEdits);
        assert_eq!("trust-project".parse::<PermissionMode>().unwrap(), PermissionMode::TrustProject);
        assert_eq!("trust_project".parse::<PermissionMode>().unwrap(), PermissionMode::TrustProject);
        assert_eq!("dontAsk".parse::<PermissionMode>().unwrap(), PermissionMode::DontAsk);
        assert_eq!("bypassPermissions".parse::<PermissionMode>().unwrap(), PermissionMode::Dangerously);
        assert_eq!("dangerously".parse::<PermissionMode>().unwrap(), PermissionMode::Dangerously);
        assert_eq!("plan".parse::<PermissionMode>().unwrap(), PermissionMode::Plan);
        assert!("unknown".parse::<PermissionMode>().is_err());
    }

    #[test]
    fn permission_mode_serde_roundtrip() {
        let mode = PermissionMode::TrustProject;
        let json = serde_json::to_string(&mode).unwrap();
        let parsed: PermissionMode = serde_json::from_str(&json).unwrap();
        assert_eq!(mode, parsed);
    }

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
    fn permission_decision_variants() {
        let allow = PermissionDecision::Allow;
        let deny = PermissionDecision::Deny("denied by policy".into());
        let ask = PermissionDecision::AskUser("confirm bash execution?".into());

        assert_eq!(allow, PermissionDecision::Allow);
        assert_eq!(deny, PermissionDecision::Deny("denied by policy".into()));
        assert_eq!(
            ask,
            PermissionDecision::AskUser("confirm bash execution?".into())
        );
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
    fn glob_match_wildcard_patterns() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("*", ""));
        assert!(glob_match("foo*", "foobar"));
        assert!(glob_match("*bar", "foobar"));
        assert!(glob_match("f*r", "foobar"));
        assert!(!glob_match("foo*", "barfoo"));
    }

    #[test]
    fn glob_match_exact() {
        assert!(glob_match("bash", "bash"));
        assert!(!glob_match("bash", "Bash"));
        assert!(!glob_match("bash", "bash_exec"));
    }

    #[test]
    fn glob_match_empty_pattern() {
        assert!(glob_match("", ""));
        assert!(!glob_match("", "nonempty"));
    }

    #[test]
    fn glob_match_multiple_stars() {
        assert!(glob_match("*foo*bar*", "xxxfooYYYbarZZZ"));
        assert!(!glob_match("*foo*bar*", "xxxbarYYYfooZZZ"));
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
    fn permission_mode_all_variants_serde() {
        for mode in [
            PermissionMode::Default,
            PermissionMode::AcceptEdits,
            PermissionMode::TrustProject,
            PermissionMode::DontAsk,
            PermissionMode::Dangerously,
            PermissionMode::Plan,
        ] {
            let json = serde_json::to_string(&mode).unwrap();
            let parsed: PermissionMode = serde_json::from_str(&json).unwrap();
            assert_eq!(mode, parsed);
        }
    }

    // ─── Additional glob_match edge cases ───

    #[test]
    fn glob_match_char_class_range() {
        assert!(glob_match("[a-z]_tool", "m_tool"));
        assert!(!glob_match("[a-z]_tool", "M_tool"));
        assert!(!glob_match("[a-z]_tool", "1_tool"));
    }

    #[test]
    fn glob_match_malformed_char_class() {
        // Missing closing bracket — should not match
        assert!(!glob_match("[abc", "a"));
    }

    #[test]
    fn glob_match_star_at_start_and_end() {
        assert!(glob_match("*bash*", "bash"));
        assert!(glob_match("*bash*", "my_bash_tool"));
        assert!(!glob_match("*bash*", "ba_sh"));
    }

    #[test]
    fn glob_match_question_mark_only() {
        assert!(glob_match("?", "a"));
        assert!(!glob_match("?", "ab"));
        assert!(!glob_match("?", ""));
    }

    #[test]
    fn glob_match_complex_pattern() {
        assert!(glob_match("mcp__*__[a-z]*", "mcp__server__tool_name"));
        assert!(!glob_match("mcp__*__[a-z]*", "mcp__server__9tool"));
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
    fn permission_decision_deny_message() {
        let decision = PermissionDecision::Deny("tool is in denied list".into());
        if let PermissionDecision::Deny(msg) = &decision {
            assert!(msg.contains("denied"));
        } else {
            panic!("expected Deny");
        }
    }

    #[test]
    fn permission_decision_ask_message() {
        let decision = PermissionDecision::AskUser("Allow bash to run 'rm -rf /'?".into());
        if let PermissionDecision::AskUser(msg) = &decision {
            assert!(msg.contains("bash"));
        } else {
            panic!("expected AskUser");
        }
    }

    #[test]
    fn glob_match_consecutive_stars() {
        // Multiple consecutive stars should behave like one
        assert!(glob_match("**", "anything"));
        assert!(glob_match("a**b", "aXXXb"));
    }

    #[test]
    fn glob_match_char_class_single_char() {
        assert!(glob_match("[x]", "x"));
        assert!(!glob_match("[x]", "y"));
    }

    // ─── matches_tool_filter tests ───

    #[test]
    fn tool_filter_wildcard_matches_any() {
        let input = serde_json::json!({"command": "echo hello"});
        assert!(matches_tool_filter("*", "bash", &input));
        assert!(matches_tool_filter("*", "read", &input));
    }

    #[test]
    fn tool_filter_exact_name() {
        let input = serde_json::json!({});
        assert!(matches_tool_filter("bash", "bash", &input));
        assert!(!matches_tool_filter("bash", "read", &input));
        assert!(matches_tool_filter("Edit", "Edit", &input));
    }

    #[test]
    fn tool_filter_name_glob() {
        let input = serde_json::json!({});
        assert!(matches_tool_filter("mcp__*", "mcp__playwright", &input));
        assert!(!matches_tool_filter("mcp__*", "bash", &input));
    }

    #[test]
    fn tool_filter_name_with_param_pattern() {
        let input = serde_json::json!({"command": "git status"});
        assert!(matches_tool_filter("Bash(command:git*)", "Bash", &input));
        assert!(matches_tool_filter("Bash(command:git *)", "Bash", &input));

        let other = serde_json::json!({"command": "rm -rf /"});
        assert!(!matches_tool_filter("Bash(command:git*)", "Bash", &other));
    }

    #[test]
    fn tool_filter_param_wrong_tool_name() {
        let input = serde_json::json!({"command": "git log"});
        assert!(!matches_tool_filter("Bash(command:git*)", "read", &input));
    }

    #[test]
    fn tool_filter_param_missing_key() {
        let input = serde_json::json!({"file_path": "/tmp/foo"});
        assert!(!matches_tool_filter("Bash(command:git*)", "Bash", &input));
    }

    #[test]
    fn tool_filter_param_non_string_value() {
        let input = serde_json::json!({"command": 42});
        assert!(!matches_tool_filter("Bash(command:git*)", "Bash", &input));
    }

    #[test]
    fn tool_filter_case_sensitive() {
        let input = serde_json::json!({});
        assert!(!matches_tool_filter("bash", "Bash", &input));
        assert!(matches_tool_filter("Bash", "Bash", &input));
    }

    // ─── is_allowed_by_whitelist tests ───

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

    // ─── is_denied_by_filter tests ───

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

    // ─── Auto-mode classifier tests ───

    #[test]
    fn auto_classify_read_only_is_safe() {
        let input = serde_json::json!({});
        assert_eq!(
            AutoModeClassifier::classify("read", true, &input),
            RiskLevel::Safe
        );
    }

    #[test]
    fn auto_classify_known_safe_tools() {
        let input = serde_json::json!({});
        for tool in SAFE_TOOLS {
            assert_eq!(
                AutoModeClassifier::classify(tool, false, &input),
                RiskLevel::Safe,
                "tool {tool} should be safe"
            );
        }
    }

    #[test]
    fn auto_classify_write_tools_are_risky() {
        let input = serde_json::json!({"file_path": "/tmp/foo.rs"});
        assert_eq!(
            AutoModeClassifier::classify("write", false, &input),
            RiskLevel::Risky
        );
        assert_eq!(
            AutoModeClassifier::classify("edit", false, &input),
            RiskLevel::Risky
        );
    }

    #[test]
    fn auto_classify_dangerous_command() {
        let input = serde_json::json!({"command": "rm -rf /"});
        assert_eq!(
            AutoModeClassifier::classify("bash", false, &input),
            RiskLevel::Dangerous
        );
    }

    #[test]
    fn auto_classify_force_push_is_dangerous() {
        let input = serde_json::json!({"command": "git push --force origin main"});
        assert_eq!(
            AutoModeClassifier::classify("bash", false, &input),
            RiskLevel::Dangerous
        );
    }

    #[test]
    fn auto_classify_safe_bash_command() {
        let input = serde_json::json!({"command": "ls -la"});
        assert_eq!(
            AutoModeClassifier::classify("bash", false, &input),
            RiskLevel::Risky
        );
    }

    #[test]
    fn auto_classify_mcp_tools_are_risky() {
        let input = serde_json::json!({});
        assert_eq!(
            AutoModeClassifier::classify("mcp__server__tool", false, &input),
            RiskLevel::Risky
        );
    }

    #[test]
    fn auto_classify_unknown_tool_is_risky() {
        let input = serde_json::json!({});
        assert_eq!(
            AutoModeClassifier::classify("some_new_tool", false, &input),
            RiskLevel::Risky
        );
    }

    #[test]
    fn auto_mode_decision_safe_allows() {
        let policy = PermissionPolicy::default();
        let input = serde_json::json!({});
        let result = auto_mode_decision(&policy, "read", true, &input);
        assert_eq!(result, PermissionDecision::Allow);
    }

    #[test]
    fn auto_mode_decision_risky_asks() {
        let policy = PermissionPolicy::default();
        let input = serde_json::json!({"file_path": "/tmp/foo"});
        let result = auto_mode_decision(&policy, "write", false, &input);
        assert!(matches!(result, PermissionDecision::AskUser(_)));
    }

    #[test]
    fn auto_mode_decision_dangerous_denies() {
        let policy = PermissionPolicy::default();
        let input = serde_json::json!({"command": "rm -rf /"});
        let result = auto_mode_decision(&policy, "bash", false, &input);
        assert!(matches!(result, PermissionDecision::Deny(_)));
    }

    #[test]
    fn auto_mode_decision_respects_denied_list() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec![],
            denied_tools: vec!["read".into()],
        };
        let input = serde_json::json!({});
        let result = auto_mode_decision(&policy, "read", true, &input);
        assert!(matches!(result, PermissionDecision::Deny(_)));
    }

    #[test]
    fn auto_mode_decision_respects_whitelist() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec!["read".into()],
            denied_tools: vec![],
        };
        let input = serde_json::json!({});
        // "write" is not in whitelist
        let result = auto_mode_decision(&policy, "write", false, &input);
        assert!(matches!(result, PermissionDecision::Deny(_)));
    }

    #[test]
    fn risk_level_serde_roundtrip() {
        for level in [RiskLevel::Safe, RiskLevel::Risky, RiskLevel::Dangerous] {
            let json = serde_json::to_string(&level).unwrap();
            let parsed: RiskLevel = serde_json::from_str(&json).unwrap();
            assert_eq!(level, parsed);
        }
    }
}
