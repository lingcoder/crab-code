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
    pub fn classify(tool_name: &str, is_read_only: bool, input: &serde_json::Value) -> RiskLevel {
        // Read-only tools are always safe
        if is_read_only {
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
    use super::super::mode::PermissionMode;
    use super::*;

    #[test]
    fn auto_classify_read_only_is_safe() {
        let input = serde_json::json!({});
        assert_eq!(
            AutoModeClassifier::classify("read", true, &input),
            RiskLevel::Safe
        );
    }

    #[test]
    fn auto_classify_non_read_only_tool_is_risky_by_default() {
        let input = serde_json::json!({});
        assert_eq!(
            AutoModeClassifier::classify("Read", false, &input),
            RiskLevel::Risky
        );
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
