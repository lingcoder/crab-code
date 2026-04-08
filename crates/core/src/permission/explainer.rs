//! Permission decision explainer.
//!
//! Maps to CCB `utils/permissions/permissionExplainer.ts`.
//!
//! Generates human-readable explanations for permission decisions, telling the
//! user *why* a tool was allowed, denied, or requires confirmation. Useful for
//! debugging permission rules and for surfacing context in the TUI.

use super::PermissionDecision;
use super::rule_parser::PermissionRule;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A human-readable explanation of a permission decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionExplanation {
    /// Summary of the decision (e.g. "Allowed by whitelist rule").
    pub decision: String,
    /// The specific rule that matched, if any (formatted as a string).
    pub matched_rule: Option<String>,
    /// Optional suggestion for the user (e.g. "Add 'Bash(command:git*)'
    /// to `allowed_tools` to auto-approve git commands").
    pub suggestion: Option<String>,
}

// ---------------------------------------------------------------------------
// Functions
// ---------------------------------------------------------------------------

/// Explain a permission decision in terms of the rules that were evaluated.
///
/// Given a tool name, the final decision, and the full set of permission rules,
/// produces a [`PermissionExplanation`] describing why the decision was reached
/// and what rule (if any) matched.
///
/// # Arguments
///
/// * `tool_name` - Name of the tool that was checked.
/// * `decision` - The final [`PermissionDecision`] that was reached.
/// * `rules` - The list of [`PermissionRule`]s that were evaluated.
///
/// # Examples
///
/// ```ignore
/// let explanation = explain_decision("Bash", &PermissionDecision::Allow, &rules);
/// println!("{}", explanation.decision);
/// // => "Allowed: tool 'Bash' matches whitelist rule 'Bash(command:git*)'"
/// ```
pub fn explain_decision(
    tool_name: &str,
    decision: &PermissionDecision,
    rules: &[PermissionRule],
) -> PermissionExplanation {
    todo!(
        "Explain why tool '{tool_name}' got decision {:?} given {} rules",
        decision,
        rules.len()
    )
}

/// Generate a suggestion for how to allow a denied tool invocation.
///
/// Returns `None` if no useful suggestion can be generated.
pub fn suggest_allow_rule(tool_name: &str, tool_input: &serde_json::Value) -> Option<String> {
    todo!(
        "Generate a suggestion for allowing tool '{tool_name}' with input {:?}",
        tool_input
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    // Tests will be added as the implementation progresses.
    // Key test scenarios:
    // - Explain an Allow decision with a matching whitelist rule
    // - Explain a Deny decision with a matching denied rule
    // - Explain an AskUser decision (no matching rule, default behavior)
    // - Suggestion generation for common tool patterns
}
