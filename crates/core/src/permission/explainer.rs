//! Permission decision explainer.
//!
//! Generates human-readable explanations for permission decisions, telling the
//! user *why* a tool was allowed, denied, or requires confirmation. Useful for
//! debugging permission rules and for surfacing context in the TUI.

use super::PermissionDecision;
use super::rule_parser::{PermissionRule, matches_rule};

// Types

/// A human-readable explanation of a permission decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionExplanation {
    /// Summary of the decision (e.g. "Allowed by whitelist rule").
    pub decision: String,
    /// The specific rule that matched, if any (formatted as a string).
    pub matched_rule: Option<String>,
    /// Optional suggestion for the user (e.g. "Add 'Bash(git *)'
    /// to `allowed_tools` to auto-approve git commands").
    pub suggestion: Option<String>,
}

// Functions

/// Explain a permission decision in terms of the rules that were evaluated.
///
/// Given a tool name, the final decision, and the full set of permission rules,
/// produces a [`PermissionExplanation`] describing why the decision was reached
/// and what rule (if any) matched.
pub fn explain_decision(
    tool_name: &str,
    decision: &PermissionDecision,
    rules: &[PermissionRule],
) -> PermissionExplanation {
    // Try to find the first matching rule for context
    let empty_input = serde_json::json!({});
    let matched = rules
        .iter()
        .find(|r| matches_rule(r, tool_name, &empty_input));

    match decision {
        PermissionDecision::Allow => {
            if let Some(rule) = matched {
                PermissionExplanation {
                    decision: format!("Allowed: tool '{tool_name}' matches rule '{rule}'"),
                    matched_rule: Some(rule.to_string()),
                    suggestion: None,
                }
            } else {
                PermissionExplanation {
                    decision: format!(
                        "Allowed: tool '{tool_name}' permitted by current permission mode"
                    ),
                    matched_rule: None,
                    suggestion: None,
                }
            }
        }
        PermissionDecision::Deny(reason) => {
            let suggestion = suggest_allow_rule(tool_name, &empty_input);
            if let Some(rule) = matched {
                PermissionExplanation {
                    decision: format!(
                        "Denied: tool '{tool_name}' blocked by rule '{rule}' — {reason}"
                    ),
                    matched_rule: Some(rule.to_string()),
                    suggestion,
                }
            } else {
                PermissionExplanation {
                    decision: format!("Denied: tool '{tool_name}' — {reason}"),
                    matched_rule: None,
                    suggestion,
                }
            }
        }
        PermissionDecision::AskUser(prompt) => {
            let suggestion = suggest_allow_rule(tool_name, &empty_input);
            PermissionExplanation {
                decision: format!("Requires confirmation: tool '{tool_name}' — {prompt}"),
                matched_rule: matched.map(std::string::ToString::to_string),
                suggestion,
            }
        }
    }
}

/// Generate a suggestion for how to allow a denied tool invocation.
///
/// Returns `None` if no useful suggestion can be generated.
pub fn suggest_allow_rule(tool_name: &str, tool_input: &serde_json::Value) -> Option<String> {
    // For Bash tools, suggest a command-specific allow rule if possible
    if tool_name == "Bash" || tool_name == "bash" {
        if let Some(command) = tool_input.get("command").and_then(|v| v.as_str()) {
            // Extract the base command (first word) for a prefix suggestion
            let base_cmd = command.split_whitespace().next().unwrap_or(command);
            return Some(format!(
                "Add 'Bash({base_cmd} *)' to `allowed_tools` to auto-approve {base_cmd} commands"
            ));
        }
        return Some(
            "Add 'Bash(*)' to `allowed_tools` to auto-approve all shell commands".to_string(),
        );
    }

    // For Edit/Write tools, suggest a path-scoped rule if possible
    if (tool_name == "Edit" || tool_name == "Write" || tool_name == "edit" || tool_name == "write")
        && let Some(path) = tool_input.get("file_path").and_then(|v| v.as_str())
        && let Some(parent) = std::path::Path::new(path).parent()
    {
        return Some(format!(
            "Add '{tool_name}(file_path:{}/**)' to `allowed_tools` to auto-approve edits in that directory",
            parent.display()
        ));
    }

    // For Read tools, suggest a path-scoped rule if possible
    if (tool_name == "Read" || tool_name == "read")
        && let Some(path) = tool_input.get("file_path").and_then(|v| v.as_str())
        && let Some(parent) = std::path::Path::new(path).parent()
    {
        return Some(format!(
            "Add 'Read(file_path:{}/**)' to `allowed_tools` to auto-approve reads in that directory",
            parent.display()
        ));
    }

    // For MCP tools, suggest the server-level wildcard
    if tool_name.starts_with("mcp__") {
        // Extract the server name: mcp__<server>__<tool>
        let parts: Vec<&str> = tool_name.splitn(3, "__").collect();
        if parts.len() >= 2 {
            let server = parts[1];
            return Some(format!(
                "Add 'mcp__{server}__*' to `allowed_tools` to auto-approve all tools from this MCP server"
            ));
        }
    }

    // Generic: suggest allowing the tool by name
    Some(format!(
        "Add '{tool_name}' to `allowed_tools` to auto-approve this tool"
    ))
}

// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::rule_parser::parse_rule;

    #[test]
    fn explain_allow_with_matching_rule() {
        // Use a tool-wide rule (no content constraint) so it matches with empty input
        let rules = vec![parse_rule("Bash").unwrap()];
        let explanation = explain_decision("Bash", &PermissionDecision::Allow, &rules);
        assert!(explanation.decision.contains("Allowed"));
        assert!(explanation.decision.contains("Bash"));
        assert!(explanation.matched_rule.is_some());
        assert!(explanation.suggestion.is_none());
    }

    #[test]
    fn explain_allow_without_matching_rule() {
        let rules = vec![parse_rule("Edit").unwrap()];
        let explanation = explain_decision("Bash", &PermissionDecision::Allow, &rules);
        assert!(explanation.decision.contains("Allowed"));
        assert!(explanation.decision.contains("permission mode"));
        assert!(explanation.matched_rule.is_none());
    }

    #[test]
    fn explain_deny_with_reason() {
        let rules = vec![parse_rule("Bash").unwrap()];
        let explanation = explain_decision(
            "Bash",
            &PermissionDecision::Deny("tool is in denied list".to_string()),
            &rules,
        );
        assert!(explanation.decision.contains("Denied"));
        assert!(explanation.decision.contains("denied list"));
        assert!(explanation.matched_rule.is_some());
        assert!(explanation.suggestion.is_some());
    }

    #[test]
    fn explain_ask_user() {
        let rules = vec![];
        let explanation = explain_decision(
            "Bash",
            &PermissionDecision::AskUser("confirm execution".to_string()),
            &rules,
        );
        assert!(explanation.decision.contains("Requires confirmation"));
        assert!(explanation.suggestion.is_some());
    }

    #[test]
    fn suggest_bash_command_rule() {
        let input = serde_json::json!({"command": "git status"});
        let suggestion = suggest_allow_rule("Bash", &input);
        assert!(suggestion.is_some());
        let s = suggestion.unwrap();
        assert!(s.contains("git"));
        assert!(s.contains("allowed_tools"));
    }

    #[test]
    fn suggest_bash_generic() {
        let input = serde_json::json!({});
        let suggestion = suggest_allow_rule("Bash", &input);
        assert!(suggestion.is_some());
        assert!(suggestion.unwrap().contains("Bash(*)"));
    }

    #[test]
    fn suggest_mcp_server_rule() {
        let input = serde_json::json!({});
        let suggestion = suggest_allow_rule("mcp__playwright__click", &input);
        assert!(suggestion.is_some());
        assert!(suggestion.unwrap().contains("mcp__playwright__*"));
    }

    #[test]
    fn suggest_generic_tool_rule() {
        let input = serde_json::json!({});
        let suggestion = suggest_allow_rule("CustomTool", &input);
        assert!(suggestion.is_some());
        assert!(suggestion.unwrap().contains("CustomTool"));
    }

    #[test]
    fn explain_allow_with_wildcard_rule() {
        let rules = vec![parse_rule("*").unwrap()];
        let explanation = explain_decision("AnyTool", &PermissionDecision::Allow, &rules);
        assert!(explanation.decision.contains("Allowed"));
        assert_eq!(explanation.matched_rule, Some("*".to_string()));
    }
}
