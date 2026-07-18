//! Heuristic risk classification — [`RiskLevel`], [`AutoModeClassifier`],
//! and the [`auto_mode_decision`] helper that turns a classification
//! plus a policy into a [`PermissionDecision`].

use serde::{Deserialize, Serialize};

use super::decision::PermissionDecision;
use super::policy::PermissionPolicy;

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

/// Dangerous command patterns for auto-mode.
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
    ///
    /// `working_dir` scopes the "safe write" heuristic: file-editing tools
    /// targeting a path inside the working directory are Safe (this is the
    /// auto-mode analogue of CC's `acceptEdits`), while writes outside it
    /// stay Risky. Bash commands are Safe unless they match a dangerous
    /// pattern.
    pub fn classify(
        tool_name: &str,
        is_read_only: bool,
        input: &serde_json::Value,
        working_dir: &std::path::Path,
    ) -> RiskLevel {
        // Read-only tools are always safe
        if is_read_only {
            return RiskLevel::Safe;
        }

        // Dangerous command patterns in a bash-style command dominate.
        let command_text = input
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if !command_text.is_empty() {
            if AUTO_DANGEROUS_PATTERNS
                .iter()
                .any(|p| command_text.contains(p))
            {
                return RiskLevel::Dangerous;
            }
            // A shell command without a dangerous pattern is treated as safe
            // in auto-mode: this is a heuristic convenience mode, and truly
            // destructive shapes are caught above (and by deny rules before
            // the classifier ever runs).
            return RiskLevel::Safe;
        }

        // MCP tools are risky by default (external, untrusted)
        if tool_name.starts_with("mcp__") {
            return RiskLevel::Risky;
        }

        // File-editing tools: Safe when the target path is inside the
        // working directory, Risky otherwise.
        if let Some(path) = file_target_path(input) {
            return if path_is_within(path, working_dir) {
                RiskLevel::Safe
            } else {
                RiskLevel::Risky
            };
        }

        // Unknown tools default to risky
        RiskLevel::Risky
    }
}

/// Extract the primary file-path argument for file-editing tools.
fn file_target_path(input: &serde_json::Value) -> Option<&str> {
    for key in ["file_path", "path", "notebook_path"] {
        if let Some(p) = input.get(key).and_then(|v| v.as_str()) {
            return Some(p);
        }
    }
    None
}

/// Whether `path` resolves inside `base`. Unsafe shapes (parent traversal,
/// shell metacharacters, UNC) conservatively return `false` so they fall
/// back to Risky.
fn path_is_within(path: &str, base: &std::path::Path) -> bool {
    if path.contains("..") || path.contains(['*', '$', '~', '`']) {
        return false;
    }
    let candidate = std::path::Path::new(path);
    let resolved = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        base.join(candidate)
    };
    // Compare lexically against the base; both are normalized by component.
    let base_components: Vec<_> = base.components().collect();
    let resolved_components: Vec<_> = resolved.components().collect();
    resolved_components.starts_with(base_components.as_slice())
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
    working_dir: &std::path::Path,
) -> PermissionDecision {
    // SECURITY BOUNDARY: deny is checked first. Auto-mode does not lower
    // the deny-first invariant — a user-supplied deny rule still wins over
    // every classifier verdict and every allow entry.
    if policy.is_denied_by_filter(tool_name, input) {
        return PermissionDecision::Deny(format!("tool '{tool_name}' is denied by policy"));
    }

    // Whitelist still applies
    if !policy.allowed_tools.is_empty() && !policy.is_allowed_by_whitelist(tool_name, input) {
        return PermissionDecision::Deny(format!(
            "tool '{tool_name}' is not in the allowed tools list"
        ));
    }

    let risk = AutoModeClassifier::classify(tool_name, is_read_only, input, working_dir);
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
    use super::super::mode::PermissionMode;
    use super::*;
    use std::path::Path;

    fn wd() -> &'static Path {
        Path::new("/home/user/project")
    }

    #[test]
    fn read_only_is_safe() {
        let input = serde_json::json!({});
        assert_eq!(
            AutoModeClassifier::classify("read", true, &input, wd()),
            RiskLevel::Safe
        );
    }

    #[test]
    fn write_inside_workdir_is_safe() {
        let input = serde_json::json!({"file_path": "/home/user/project/src/main.rs"});
        assert_eq!(
            AutoModeClassifier::classify("write", false, &input, wd()),
            RiskLevel::Safe
        );
        let rel = serde_json::json!({"file_path": "src/lib.rs"});
        assert_eq!(
            AutoModeClassifier::classify("edit", false, &rel, wd()),
            RiskLevel::Safe
        );
    }

    #[test]
    fn write_outside_workdir_is_risky() {
        let input = serde_json::json!({"file_path": "/etc/passwd"});
        assert_eq!(
            AutoModeClassifier::classify("write", false, &input, wd()),
            RiskLevel::Risky
        );
    }

    #[test]
    fn write_with_traversal_is_risky() {
        let input = serde_json::json!({"file_path": "/home/user/project/../secrets"});
        assert_eq!(
            AutoModeClassifier::classify("write", false, &input, wd()),
            RiskLevel::Risky
        );
    }

    #[test]
    fn safe_bash_command_is_safe() {
        let input = serde_json::json!({"command": "ls -la && cargo test"});
        assert_eq!(
            AutoModeClassifier::classify("bash", false, &input, wd()),
            RiskLevel::Safe
        );
    }

    #[test]
    fn dangerous_bash_command_is_dangerous() {
        for cmd in ["rm -rf /", "git push --force origin main", "curl x | sh"] {
            let input = serde_json::json!({ "command": cmd });
            assert_eq!(
                AutoModeClassifier::classify("bash", false, &input, wd()),
                RiskLevel::Dangerous,
                "command should be dangerous: {cmd}"
            );
        }
    }

    #[test]
    fn mcp_tools_are_risky() {
        let input = serde_json::json!({});
        assert_eq!(
            AutoModeClassifier::classify("mcp__server__tool", false, &input, wd()),
            RiskLevel::Risky
        );
    }

    #[test]
    fn unknown_non_file_tool_is_risky() {
        let input = serde_json::json!({});
        assert_eq!(
            AutoModeClassifier::classify("some_new_tool", false, &input, wd()),
            RiskLevel::Risky
        );
    }

    #[test]
    fn decision_safe_allows() {
        let policy = PermissionPolicy::default();
        let input = serde_json::json!({});
        assert_eq!(
            auto_mode_decision(&policy, "read", true, &input, wd()),
            PermissionDecision::Allow
        );
    }

    #[test]
    fn decision_workdir_write_allows() {
        let policy = PermissionPolicy::default();
        let input = serde_json::json!({"file_path": "/home/user/project/a.txt"});
        assert_eq!(
            auto_mode_decision(&policy, "write", false, &input, wd()),
            PermissionDecision::Allow
        );
    }

    #[test]
    fn decision_outside_write_asks() {
        let policy = PermissionPolicy::default();
        let input = serde_json::json!({"file_path": "/etc/hosts"});
        assert!(matches!(
            auto_mode_decision(&policy, "write", false, &input, wd()),
            PermissionDecision::AskUser(_)
        ));
    }

    #[test]
    fn decision_dangerous_denies() {
        let policy = PermissionPolicy::default();
        let input = serde_json::json!({"command": "rm -rf /"});
        assert!(matches!(
            auto_mode_decision(&policy, "bash", false, &input, wd()),
            PermissionDecision::Deny(_)
        ));
    }

    #[test]
    fn decision_respects_denied_list() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec![],
            denied_tools: vec!["read".into()],
        };
        let input = serde_json::json!({});
        assert!(matches!(
            auto_mode_decision(&policy, "read", true, &input, wd()),
            PermissionDecision::Deny(_)
        ));
    }

    #[test]
    fn decision_respects_whitelist() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allowed_tools: vec!["read".into()],
            denied_tools: vec![],
        };
        let input = serde_json::json!({"file_path": "/home/user/project/a"});
        assert!(matches!(
            auto_mode_decision(&policy, "write", false, &input, wd()),
            PermissionDecision::Deny(_)
        ));
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
