//! Enhanced permission rules with expiration, ordered evaluation, and scoping.
//!
//! Builds on the base [`crate::permissions`] module by adding:
//! - [`RuleAction::Ask`] for interactive prompting
//! - Expiration timestamps for temporary grants
//! - [`RuleSet`] with first-match evaluation semantics

use serde::{Deserialize, Serialize};

use crate::glob_matcher::GlobMatcher;

// ── Rule action ────────────────────────────────────────────────────

/// What should happen when a permission rule matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleAction {
    /// Allow the operation without prompting.
    Allow,
    /// Deny the operation without prompting.
    Deny,
    /// Ask the user interactively.
    Ask,
}

impl std::fmt::Display for RuleAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Allow => write!(f, "allow"),
            Self::Deny => write!(f, "deny"),
            Self::Ask => write!(f, "ask"),
        }
    }
}

// ── Rule scope ─────────────────────────────────────────────────────

/// The scope at which a permission rule applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum PermRuleScope {
    /// Applies to the current session only (most specific).
    Session,
    /// Applies to the current project (`.crab/` directory).
    Project,
    /// Applies globally (`~/.crab/`).
    Global,
}

impl std::fmt::Display for PermRuleScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Session => write!(f, "session"),
            Self::Project => write!(f, "project"),
            Self::Global => write!(f, "global"),
        }
    }
}

// ── Permission rule ────────────────────────────────────────────────

/// A single enhanced permission rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnhancedPermissionRule {
    /// Glob pattern for tool names (e.g. `"bash"`, `"mcp__*"`).
    pub pattern: String,
    /// What to do when this rule matches.
    pub action: RuleAction,
    /// Scope of the rule.
    pub scope: PermRuleScope,
    /// Optional Unix timestamp (seconds) after which the rule expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    /// Optional path pattern — if set, the rule only applies when the
    /// tool operates on paths matching this glob.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_pattern: Option<String>,
    /// Optional human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl EnhancedPermissionRule {
    /// Check whether this rule has expired relative to the given Unix timestamp.
    #[must_use]
    pub fn is_expired(&self, now_secs: u64) -> bool {
        self.expires_at.is_some_and(|exp| now_secs >= exp)
    }

    /// Check whether this rule's tool pattern matches the given tool name.
    #[must_use]
    pub fn matches_tool(&self, tool_name: &str) -> bool {
        GlobMatcher::new(&self.pattern).is_match(tool_name)
    }

    /// Check whether this rule's optional path pattern matches a given path.
    ///
    /// Returns `true` if no path pattern is set (rule applies to all paths).
    #[must_use]
    pub fn matches_path(&self, path: &str) -> bool {
        self.path_pattern
            .as_ref()
            .is_none_or(|pat| GlobMatcher::new(pat).is_match(path))
    }
}

// ── Rule set ───────────────────────────────────────────────────────

/// An ordered set of permission rules with first-match evaluation.
///
/// Rules are evaluated in order; the first non-expired rule whose pattern
/// matches determines the action. If no rule matches, the default action
/// (configurable, defaults to [`RuleAction::Ask`]) is returned.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleSet {
    rules: Vec<EnhancedPermissionRule>,
    #[serde(default = "default_action")]
    default_action: RuleAction,
}

fn default_action() -> RuleAction {
    RuleAction::Ask
}

impl RuleSet {
    /// Create an empty rule set with the given default action.
    #[must_use]
    pub fn new(default_action: RuleAction) -> Self {
        Self {
            rules: Vec::new(),
            default_action,
        }
    }

    /// Add a rule to the end of the set.
    pub fn push(&mut self, rule: EnhancedPermissionRule) {
        self.rules.push(rule);
    }

    /// Insert a rule at the given index (0 = highest priority).
    pub fn insert(&mut self, index: usize, rule: EnhancedPermissionRule) {
        let index = index.min(self.rules.len());
        self.rules.insert(index, rule);
    }

    /// Remove a rule by index. Returns the removed rule if the index was valid.
    pub fn remove(&mut self, index: usize) -> Option<EnhancedPermissionRule> {
        if index < self.rules.len() {
            Some(self.rules.remove(index))
        } else {
            None
        }
    }

    /// Remove all expired rules given the current Unix timestamp.
    /// Returns the number of rules removed.
    pub fn remove_expired(&mut self, now_secs: u64) -> usize {
        let before = self.rules.len();
        self.rules.retain(|r| !r.is_expired(now_secs));
        before - self.rules.len()
    }

    /// All rules in evaluation order.
    #[must_use]
    pub fn rules(&self) -> &[EnhancedPermissionRule] {
        &self.rules
    }

    /// Number of rules.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Whether the rule set is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// The default action when no rule matches.
    #[must_use]
    pub fn default_action(&self) -> RuleAction {
        self.default_action
    }

    /// Set the default action.
    pub fn set_default_action(&mut self, action: RuleAction) {
        self.default_action = action;
    }
}

// ── Evaluation ─────────────────────────────────────────────────────

/// Evaluate a tool invocation against a rule set.
///
/// Returns the action from the first matching, non-expired rule. If no
/// rule matches, returns the rule set's default action.
#[must_use]
pub fn evaluate_rules(
    rules: &RuleSet,
    tool_name: &str,
    path: Option<&str>,
    now_secs: u64,
) -> RuleAction {
    for rule in &rules.rules {
        if rule.is_expired(now_secs) {
            continue;
        }
        if !rule.matches_tool(tool_name) {
            continue;
        }
        if let Some(p) = path
            && !rule.matches_path(p)
        {
            continue;
        }
        return rule.action;
    }
    rules.default_action
}

/// Result of evaluation with additional context.
#[derive(Debug, Clone, Serialize)]
pub struct EvaluationResult {
    /// The decided action.
    pub action: RuleAction,
    /// Index of the matching rule, or `None` if the default was used.
    pub matched_rule_index: Option<usize>,
    /// Whether the result came from the default action.
    pub is_default: bool,
}

/// Like [`evaluate_rules`] but returns detailed context about which rule matched.
#[must_use]
pub fn evaluate_rules_detailed(
    rules: &RuleSet,
    tool_name: &str,
    path: Option<&str>,
    now_secs: u64,
) -> EvaluationResult {
    for (i, rule) in rules.rules.iter().enumerate() {
        if rule.is_expired(now_secs) {
            continue;
        }
        if !rule.matches_tool(tool_name) {
            continue;
        }
        if let Some(p) = path
            && !rule.matches_path(p)
        {
            continue;
        }
        return EvaluationResult {
            action: rule.action,
            matched_rule_index: Some(i),
            is_default: false,
        };
    }
    EvaluationResult {
        action: rules.default_action,
        matched_rule_index: None,
        is_default: true,
    }
}

// ── Default impl ───────────────────────────────────────────────────

impl Default for RuleSet {
    fn default() -> Self {
        Self::new(RuleAction::Ask)
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_rule(
        pattern: &str,
        action: RuleAction,
        scope: PermRuleScope,
    ) -> EnhancedPermissionRule {
        EnhancedPermissionRule {
            pattern: pattern.to_owned(),
            action,
            scope,
            expires_at: None,
            path_pattern: None,
            description: None,
        }
    }

    fn make_rule_with_expiry(
        pattern: &str,
        action: RuleAction,
        expires_at: u64,
    ) -> EnhancedPermissionRule {
        EnhancedPermissionRule {
            pattern: pattern.to_owned(),
            action,
            scope: PermRuleScope::Session,
            expires_at: Some(expires_at),
            path_pattern: None,
            description: None,
        }
    }

    fn make_rule_with_path(
        pattern: &str,
        action: RuleAction,
        path_pattern: &str,
    ) -> EnhancedPermissionRule {
        EnhancedPermissionRule {
            pattern: pattern.to_owned(),
            action,
            scope: PermRuleScope::Project,
            expires_at: None,
            path_pattern: Some(path_pattern.to_owned()),
            description: None,
        }
    }

    // ── RuleAction ─────────────────────────────────────────

    #[test]
    fn action_display() {
        assert_eq!(RuleAction::Allow.to_string(), "allow");
        assert_eq!(RuleAction::Deny.to_string(), "deny");
        assert_eq!(RuleAction::Ask.to_string(), "ask");
    }

    #[test]
    fn action_serde_roundtrip() {
        for action in [RuleAction::Allow, RuleAction::Deny, RuleAction::Ask] {
            let json = serde_json::to_string(&action).unwrap();
            let back: RuleAction = serde_json::from_str(&json).unwrap();
            assert_eq!(action, back);
        }
    }

    // ── PermRuleScope ──────────────────────────────────────

    #[test]
    fn scope_display() {
        assert_eq!(PermRuleScope::Session.to_string(), "session");
        assert_eq!(PermRuleScope::Project.to_string(), "project");
        assert_eq!(PermRuleScope::Global.to_string(), "global");
    }

    #[test]
    fn scope_serde_roundtrip() {
        for scope in [
            PermRuleScope::Session,
            PermRuleScope::Project,
            PermRuleScope::Global,
        ] {
            let json = serde_json::to_string(&scope).unwrap();
            let back: PermRuleScope = serde_json::from_str(&json).unwrap();
            assert_eq!(scope, back);
        }
    }

    #[test]
    fn scope_ordering() {
        assert!(PermRuleScope::Session < PermRuleScope::Project);
        assert!(PermRuleScope::Project < PermRuleScope::Global);
    }

    // ── EnhancedPermissionRule ──────────────────────────────

    #[test]
    fn rule_not_expired() {
        let rule = make_rule("bash", RuleAction::Allow, PermRuleScope::Session);
        assert!(!rule.is_expired(1000));
    }

    #[test]
    fn rule_expired() {
        let rule = make_rule_with_expiry("bash", RuleAction::Allow, 500);
        assert!(rule.is_expired(500));
        assert!(rule.is_expired(1000));
        assert!(!rule.is_expired(499));
    }

    #[test]
    fn rule_matches_exact() {
        let rule = make_rule("bash", RuleAction::Allow, PermRuleScope::Session);
        assert!(rule.matches_tool("bash"));
        assert!(!rule.matches_tool("read"));
    }

    #[test]
    fn rule_matches_glob() {
        let rule = make_rule("mcp__*", RuleAction::Deny, PermRuleScope::Global);
        assert!(rule.matches_tool("mcp__playwright"));
        assert!(!rule.matches_tool("bash"));
    }

    #[test]
    fn rule_matches_path_none() {
        let rule = make_rule("bash", RuleAction::Allow, PermRuleScope::Session);
        assert!(rule.matches_path("/any/path"));
    }

    #[test]
    fn rule_matches_path_pattern() {
        let rule = make_rule_with_path("bash", RuleAction::Allow, "/tmp/*");
        assert!(rule.matches_path("/tmp/test"));
        assert!(!rule.matches_path("/etc/config"));
    }

    #[test]
    fn rule_serde_roundtrip() {
        let rule = EnhancedPermissionRule {
            pattern: "mcp__*".to_owned(),
            action: RuleAction::Deny,
            scope: PermRuleScope::Project,
            expires_at: Some(9999),
            path_pattern: Some("/src/*".to_owned()),
            description: Some("block mcp in src".to_owned()),
        };
        let json = serde_json::to_string_pretty(&rule).unwrap();
        let back: EnhancedPermissionRule = serde_json::from_str(&json).unwrap();
        assert_eq!(rule, back);
    }

    #[test]
    fn rule_serde_skips_none_fields() {
        let rule = make_rule("bash", RuleAction::Allow, PermRuleScope::Session);
        let json = serde_json::to_string(&rule).unwrap();
        assert!(!json.contains("expires_at"));
        assert!(!json.contains("path_pattern"));
        assert!(!json.contains("description"));
    }

    // ── RuleSet ────────────────────────────────────────────

    #[test]
    fn ruleset_default_is_ask() {
        let rs = RuleSet::default();
        assert!(rs.is_empty());
        assert_eq!(rs.default_action(), RuleAction::Ask);
    }

    #[test]
    fn ruleset_push_and_len() {
        let mut rs = RuleSet::new(RuleAction::Deny);
        rs.push(make_rule("bash", RuleAction::Allow, PermRuleScope::Session));
        rs.push(make_rule("read", RuleAction::Allow, PermRuleScope::Global));
        assert_eq!(rs.len(), 2);
        assert!(!rs.is_empty());
    }

    #[test]
    fn ruleset_insert_at_front() {
        let mut rs = RuleSet::new(RuleAction::Ask);
        rs.push(make_rule("bash", RuleAction::Allow, PermRuleScope::Session));
        rs.insert(
            0,
            make_rule("read", RuleAction::Deny, PermRuleScope::Global),
        );
        assert_eq!(rs.rules()[0].pattern, "read");
        assert_eq!(rs.rules()[1].pattern, "bash");
    }

    #[test]
    fn ruleset_insert_beyond_end() {
        let mut rs = RuleSet::new(RuleAction::Ask);
        rs.push(make_rule("bash", RuleAction::Allow, PermRuleScope::Session));
        rs.insert(
            100,
            make_rule("read", RuleAction::Deny, PermRuleScope::Global),
        );
        assert_eq!(rs.len(), 2);
        assert_eq!(rs.rules()[1].pattern, "read");
    }

    #[test]
    fn ruleset_remove() {
        let mut rs = RuleSet::new(RuleAction::Ask);
        rs.push(make_rule("bash", RuleAction::Allow, PermRuleScope::Session));
        rs.push(make_rule("read", RuleAction::Deny, PermRuleScope::Global));
        let removed = rs.remove(0);
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().pattern, "bash");
        assert_eq!(rs.len(), 1);
    }

    #[test]
    fn ruleset_remove_invalid_index() {
        let mut rs = RuleSet::new(RuleAction::Ask);
        assert!(rs.remove(0).is_none());
    }

    #[test]
    fn ruleset_remove_expired() {
        let mut rs = RuleSet::new(RuleAction::Ask);
        rs.push(make_rule_with_expiry("a", RuleAction::Allow, 100));
        rs.push(make_rule("b", RuleAction::Deny, PermRuleScope::Global));
        rs.push(make_rule_with_expiry("c", RuleAction::Allow, 200));

        let removed = rs.remove_expired(150);
        assert_eq!(removed, 1);
        assert_eq!(rs.len(), 2);
        assert_eq!(rs.rules()[0].pattern, "b");
    }

    #[test]
    fn ruleset_set_default_action() {
        let mut rs = RuleSet::default();
        rs.set_default_action(RuleAction::Deny);
        assert_eq!(rs.default_action(), RuleAction::Deny);
    }

    #[test]
    fn ruleset_serde_roundtrip() {
        let mut rs = RuleSet::new(RuleAction::Deny);
        rs.push(make_rule("bash", RuleAction::Allow, PermRuleScope::Session));
        let json = serde_json::to_string_pretty(&rs).unwrap();
        let back: RuleSet = serde_json::from_str(&json).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back.default_action(), RuleAction::Deny);
    }

    // ── evaluate_rules ─────────────────────────────────────

    #[test]
    fn eval_first_match_wins() {
        let mut rs = RuleSet::new(RuleAction::Ask);
        rs.push(make_rule("bash", RuleAction::Deny, PermRuleScope::Session));
        rs.push(make_rule("bash", RuleAction::Allow, PermRuleScope::Global));

        let action = evaluate_rules(&rs, "bash", None, 0);
        assert_eq!(action, RuleAction::Deny);
    }

    #[test]
    fn eval_skips_expired() {
        let mut rs = RuleSet::new(RuleAction::Ask);
        rs.push(make_rule_with_expiry("bash", RuleAction::Deny, 100));
        rs.push(make_rule("bash", RuleAction::Allow, PermRuleScope::Global));

        let action = evaluate_rules(&rs, "bash", None, 200);
        assert_eq!(action, RuleAction::Allow);
    }

    #[test]
    fn eval_uses_default_when_no_match() {
        let rs = RuleSet::new(RuleAction::Deny);
        let action = evaluate_rules(&rs, "bash", None, 0);
        assert_eq!(action, RuleAction::Deny);
    }

    #[test]
    fn eval_with_path() {
        let mut rs = RuleSet::new(RuleAction::Ask);
        rs.push(make_rule_with_path("bash", RuleAction::Deny, "/etc/*"));
        rs.push(make_rule("bash", RuleAction::Allow, PermRuleScope::Global));

        // Path matches restrictive rule
        assert_eq!(
            evaluate_rules(&rs, "bash", Some("/etc/passwd"), 0),
            RuleAction::Deny
        );
        // Path doesn't match restrictive rule, falls through
        assert_eq!(
            evaluate_rules(&rs, "bash", Some("/tmp/test"), 0),
            RuleAction::Allow
        );
    }

    #[test]
    fn eval_no_path_skips_path_check() {
        let mut rs = RuleSet::new(RuleAction::Ask);
        rs.push(make_rule_with_path("bash", RuleAction::Deny, "/etc/*"));

        // No path provided — path_pattern rule still matches (matches_path returns true for None path)
        // Actually path is None so the path check is skipped entirely
        let action = evaluate_rules(&rs, "bash", None, 0);
        assert_eq!(action, RuleAction::Deny);
    }

    // ── evaluate_rules_detailed ────────────────────────────

    #[test]
    fn eval_detailed_match() {
        let mut rs = RuleSet::new(RuleAction::Ask);
        rs.push(make_rule("read", RuleAction::Allow, PermRuleScope::Session));
        rs.push(make_rule("bash", RuleAction::Deny, PermRuleScope::Global));

        let result = evaluate_rules_detailed(&rs, "bash", None, 0);
        assert_eq!(result.action, RuleAction::Deny);
        assert_eq!(result.matched_rule_index, Some(1));
        assert!(!result.is_default);
    }

    #[test]
    fn eval_detailed_default() {
        let rs = RuleSet::new(RuleAction::Ask);
        let result = evaluate_rules_detailed(&rs, "bash", None, 0);
        assert_eq!(result.action, RuleAction::Ask);
        assert!(result.matched_rule_index.is_none());
        assert!(result.is_default);
    }

    #[test]
    fn eval_detailed_serializes() {
        let result = EvaluationResult {
            action: RuleAction::Allow,
            matched_rule_index: Some(2),
            is_default: false,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("allow"));
        assert!(json.contains("matched_rule_index"));
    }
}
