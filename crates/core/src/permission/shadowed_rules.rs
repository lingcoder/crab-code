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

use super::rule_parser::{PermissionRule, RuleContent};

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
pub fn detect_shadowed_rules(rules: &[PermissionRule]) -> Vec<ShadowedRule> {
    let mut shadowed = Vec::new();

    for (i, rule) in rules.iter().enumerate() {
        for earlier in &rules[..i] {
            if rule_shadows(earlier, rule) {
                shadowed.push(ShadowedRule {
                    rule: rule.clone(),
                    shadowed_by: earlier.clone(),
                    reason: format!(
                        "rule '{rule}' is unreachable because earlier rule '{earlier}' already matches everything it would"
                    ),
                });
                break; // only report the first shadowing rule
            }
        }
    }

    shadowed
}

/// Check whether rule `a` completely covers (shadows) rule `b`.
///
/// A rule `a` shadows `b` if every tool invocation matched by `b` would
/// also be matched by `a`.
fn rule_shadows(a: &PermissionRule, b: &PermissionRule) -> bool {
    // First, `a`'s tool name pattern must cover `b`'s tool name pattern.
    if !tool_name_covers(&a.tool_name, &b.tool_name) {
        return false;
    }

    // Then check content coverage.
    match (&a.content, &b.content) {
        // `a` has no content constraint or matches Any → covers everything for that tool
        (None | Some(RuleContent::Any), _) => true,
        // Both have glob content on the same key
        (
            Some(RuleContent::Glob {
                key: key_a,
                pattern: pat_a,
            }),
            Some(RuleContent::Glob {
                key: key_b,
                pattern: pat_b,
            }),
        ) => key_a == key_b && glob_pattern_covers(pat_a, pat_b),
        // Both have exact content on the same key
        (
            Some(RuleContent::Exact {
                key: key_a,
                value: val_a,
            }),
            Some(RuleContent::Exact {
                key: key_b,
                value: val_b,
            }),
        ) => key_a == key_b && val_a == val_b,
        // Glob `a` covers exact `b` if the glob matches the exact value
        (
            Some(RuleContent::Glob {
                key: key_a,
                pattern: pat_a,
            }),
            Some(RuleContent::Exact {
                key: key_b,
                value: val_b,
            }),
        ) => key_a == key_b && super::filter::glob_match(pat_a, val_b),
        // All other combinations: conservative — don't claim shadowing
        _ => false,
    }
}

/// Check whether tool name pattern `a` covers pattern `b`.
///
/// `"*"` covers everything. Otherwise `a` must equal `b` or be a broader glob.
fn tool_name_covers(a: &str, b: &str) -> bool {
    if a == "*" {
        return true;
    }
    if a == b {
        return true;
    }
    // Check if `a` as a glob pattern would match `b`
    // e.g., "mcp__*" covers "mcp__server__tool"
    super::filter::glob_match(a, b)
}

/// Check whether glob pattern `a` covers all matches of glob pattern `b`.
///
/// This is a conservative approximation: `"*"` covers everything, and
/// identical patterns cover each other. For prefix patterns like `"git*"`,
/// if `a` is a prefix of `b`, `a` covers `b`.
fn glob_pattern_covers(a: &str, b: &str) -> bool {
    // `*` covers everything
    if a == "*" {
        return true;
    }
    // Identical patterns
    if a == b {
        return true;
    }
    // Prefix coverage: "git*" covers "git status*", "git commit*"
    if let Some(prefix_a) = a.strip_suffix('*') {
        if let Some(prefix_b) = b.strip_suffix('*') {
            // "git*" covers "git st*" because prefix_a is a prefix of prefix_b
            return prefix_b.starts_with(prefix_a);
        }
        // "git*" covers exact "git status" if it starts with "git"
        return b.starts_with(prefix_a);
    }
    false
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::rule_parser::parse_rule;

    #[test]
    fn wildcard_shadows_everything() {
        let rules = vec![
            parse_rule("*").unwrap(),
            parse_rule("Bash(command:git*)").unwrap(),
        ];
        let shadowed = detect_shadowed_rules(&rules);
        assert_eq!(shadowed.len(), 1);
        assert_eq!(shadowed[0].rule.tool_name, "Bash");
        assert_eq!(shadowed[0].shadowed_by.tool_name, "*");
    }

    #[test]
    fn tool_wide_shadows_specific_content() {
        let rules = vec![
            parse_rule("Bash").unwrap(),
            parse_rule("Bash(command:git*)").unwrap(),
        ];
        let shadowed = detect_shadowed_rules(&rules);
        assert_eq!(shadowed.len(), 1);
        assert_eq!(shadowed[0].shadowed_by.tool_name, "Bash");
    }

    #[test]
    fn tool_wide_any_shadows_specific() {
        let rules = vec![
            parse_rule("Bash(*)").unwrap(),
            parse_rule("Bash(command:git*)").unwrap(),
        ];
        let shadowed = detect_shadowed_rules(&rules);
        assert_eq!(shadowed.len(), 1);
    }

    #[test]
    fn non_overlapping_rules_not_shadowed() {
        let rules = vec![
            parse_rule("Bash(command:git*)").unwrap(),
            parse_rule("Bash(command:npm*)").unwrap(),
        ];
        let shadowed = detect_shadowed_rules(&rules);
        assert!(shadowed.is_empty());
    }

    #[test]
    fn different_tools_not_shadowed() {
        let rules = vec![parse_rule("Bash").unwrap(), parse_rule("Edit").unwrap()];
        let shadowed = detect_shadowed_rules(&rules);
        assert!(shadowed.is_empty());
    }

    #[test]
    fn identical_rules_shadow() {
        let rules = vec![
            parse_rule("Bash(command:git*)").unwrap(),
            parse_rule("Bash(command:git*)").unwrap(),
        ];
        let shadowed = detect_shadowed_rules(&rules);
        assert_eq!(shadowed.len(), 1);
    }

    #[test]
    fn broader_glob_shadows_narrower() {
        let rules = vec![
            parse_rule("Bash(command:git*)").unwrap(),
            parse_rule("Bash(command:git status*)").unwrap(),
        ];
        let shadowed = detect_shadowed_rules(&rules);
        assert_eq!(shadowed.len(), 1);
    }

    #[test]
    fn narrower_glob_does_not_shadow_broader() {
        let rules = vec![
            parse_rule("Bash(command:git status*)").unwrap(),
            parse_rule("Bash(command:git*)").unwrap(),
        ];
        let shadowed = detect_shadowed_rules(&rules);
        assert!(shadowed.is_empty());
    }

    #[test]
    fn glob_tool_name_shadows_specific() {
        let rules = vec![
            parse_rule("mcp__*").unwrap(),
            parse_rule("mcp__server__tool").unwrap(),
        ];
        let shadowed = detect_shadowed_rules(&rules);
        assert_eq!(shadowed.len(), 1);
    }

    #[test]
    fn exact_content_shadowed_by_glob() {
        let rules = vec![
            parse_rule("Bash(command:git*)").unwrap(),
            parse_rule("Bash(command=git status)").unwrap(),
        ];
        let shadowed = detect_shadowed_rules(&rules);
        assert_eq!(shadowed.len(), 1);
    }

    #[test]
    fn exact_content_not_shadowed_by_non_matching_glob() {
        let rules = vec![
            parse_rule("Bash(command:npm*)").unwrap(),
            parse_rule("Bash(command=git status)").unwrap(),
        ];
        let shadowed = detect_shadowed_rules(&rules);
        assert!(shadowed.is_empty());
    }

    #[test]
    fn multiple_shadows_detected() {
        let rules = vec![
            parse_rule("*").unwrap(),
            parse_rule("Bash").unwrap(),
            parse_rule("Edit").unwrap(),
            parse_rule("Read").unwrap(),
        ];
        let shadowed = detect_shadowed_rules(&rules);
        assert_eq!(shadowed.len(), 3);
    }

    #[test]
    fn empty_rules_no_shadows() {
        let shadowed = detect_shadowed_rules(&[]);
        assert!(shadowed.is_empty());
    }

    #[test]
    fn single_rule_no_shadows() {
        let rules = vec![parse_rule("Bash").unwrap()];
        let shadowed = detect_shadowed_rules(&rules);
        assert!(shadowed.is_empty());
    }
}
