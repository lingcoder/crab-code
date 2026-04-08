//! Shadowed rule detection for permission configurations.
//!
//! Maps to CCB `utils/permissions/shadowedRuleDetection.ts`.
//!
//! Detects permission rules that are "shadowed" (unreachable) because an
//! earlier, broader rule already covers them. For example, if `allowed_tools`
//! contains `["*", "Bash(command:git*)"]`, the second rule is shadowed because
//! the wildcard `"*"` already matches everything.
//!
//! This is used to warn users about redundant or misconfigured rules.

use super::rule_parser::PermissionRule;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A rule that is shadowed (made unreachable) by another rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowedRule {
    /// The rule that is unreachable.
    pub rule: PermissionRule,
    /// The earlier/broader rule that shadows it.
    pub shadowed_by: PermissionRule,
    /// Human-readable explanation of why it's shadowed.
    pub reason: String,
}

// ---------------------------------------------------------------------------
// Functions
// ---------------------------------------------------------------------------

/// Detect rules in the list that are shadowed by earlier, broader rules.
///
/// Iterates through the rule list and, for each rule, checks whether any
/// preceding rule makes it redundant. Returns a list of all shadowed rules
/// with explanations.
///
/// # Arguments
///
/// * `rules` - The ordered list of permission rules to analyze.
///
/// # Examples
///
/// ```ignore
/// use crate::permission::rule_parser::parse_rule;
///
/// let rules = vec![
///     parse_rule("*").unwrap(),
///     parse_rule("Bash(command:git*)").unwrap(),
/// ];
/// let shadowed = detect_shadowed_rules(&rules);
/// assert_eq!(shadowed.len(), 1);
/// assert_eq!(shadowed[0].rule.tool_name, "Bash");
/// ```
pub fn detect_shadowed_rules(rules: &[PermissionRule]) -> Vec<ShadowedRule> {
    todo!(
        "Analyze {} rules for shadowed/unreachable entries",
        rules.len()
    )
}

/// Check whether rule `a` completely covers (shadows) rule `b`.
///
/// A rule `a` shadows `b` if every tool invocation matched by `b` would
/// also be matched by `a`.
#[allow(dead_code)]
fn rule_shadows(a: &PermissionRule, b: &PermissionRule) -> bool {
    todo!("Check if rule '{}' shadows rule '{}'", a, b)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    // Tests will be added as the implementation progresses.
    // Key test scenarios:
    // - Wildcard shadows everything
    // - `Bash` shadows `Bash(command:git*)`
    // - `Bash(command:git*)` does NOT shadow `Bash(command:npm*)`
    // - Identical rules shadow each other
    // - Non-overlapping rules don't shadow
}
