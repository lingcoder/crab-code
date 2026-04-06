//! Permission inheritance — merges global, project, and session rule sets.
//!
//! The inheritance chain is **Global → Project → Session**, where more
//! specific scopes (session > project > global) override more general ones.
//!
//! [`PermissionInheritance`] merges three [`RuleSet`]s into a single
//! [`EffectivePermissions`] that can be queried.

use serde::{Deserialize, Serialize};

use crate::permission_rules::{
    EnhancedPermissionRule, EvaluationResult, PermRuleScope, RuleAction, RuleSet,
    evaluate_rules_detailed,
};

// ── Effective permissions ──────────────────────────────────────────

/// The merged permission set produced by combining global, project, and
/// session rules. More specific scopes take priority.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectivePermissions {
    /// Merged rules in evaluation order (session first, then project, then global).
    merged: RuleSet,
    /// How many rules came from session scope.
    session_rule_count: usize,
    /// How many rules came from project scope.
    project_rule_count: usize,
    /// How many rules came from global scope.
    global_rule_count: usize,
}

impl EffectivePermissions {
    /// Evaluate a tool invocation against the merged permissions.
    #[must_use]
    pub fn evaluate(&self, tool_name: &str, path: Option<&str>, now_secs: u64) -> RuleAction {
        crate::permission_rules::evaluate_rules(&self.merged, tool_name, path, now_secs)
    }

    /// Evaluate with detailed result (which rule matched).
    #[must_use]
    pub fn evaluate_detailed(
        &self,
        tool_name: &str,
        path: Option<&str>,
        now_secs: u64,
    ) -> EvaluationResult {
        evaluate_rules_detailed(&self.merged, tool_name, path, now_secs)
    }

    /// The merged rule set.
    #[must_use]
    pub fn rules(&self) -> &RuleSet {
        &self.merged
    }

    /// Total number of rules across all scopes.
    #[must_use]
    pub fn total_rules(&self) -> usize {
        self.merged.len()
    }

    /// Number of session rules.
    #[must_use]
    pub fn session_rule_count(&self) -> usize {
        self.session_rule_count
    }

    /// Number of project rules.
    #[must_use]
    pub fn project_rule_count(&self) -> usize {
        self.project_rule_count
    }

    /// Number of global rules.
    #[must_use]
    pub fn global_rule_count(&self) -> usize {
        self.global_rule_count
    }
}

// ── Inheritance builder ────────────────────────────────────────────

/// Builds [`EffectivePermissions`] by merging global, project, and session rules.
///
/// The merge strategy is:
/// 1. Session rules are evaluated first (highest priority).
/// 2. Project rules come next.
/// 3. Global rules come last.
///
/// Within each scope, rule order is preserved. The default action comes
/// from the most specific scope that defines one (session > project > global).
pub struct PermissionInheritance;

impl PermissionInheritance {
    /// Merge permission rule sets from three scopes.
    ///
    /// Rules from more specific scopes are placed earlier in the evaluation
    /// order, giving them priority via first-match semantics.
    #[must_use]
    pub fn merge(global: &RuleSet, project: &RuleSet, session: &RuleSet) -> EffectivePermissions {
        let session_rules: Vec<EnhancedPermissionRule> = session.rules().to_vec();
        let project_rules: Vec<EnhancedPermissionRule> = project.rules().to_vec();
        let global_rules: Vec<EnhancedPermissionRule> = global.rules().to_vec();

        let session_count = session_rules.len();
        let project_count = project_rules.len();
        let global_count = global_rules.len();

        // Determine default action: most specific scope wins
        let default_action = if !session.is_empty() {
            session.default_action()
        } else if !project.is_empty() {
            project.default_action()
        } else {
            global.default_action()
        };

        let mut merged = RuleSet::new(default_action);

        // Session rules first (highest priority)
        for rule in session_rules {
            merged.push(rule);
        }
        // Then project rules
        for rule in project_rules {
            merged.push(rule);
        }
        // Then global rules (lowest priority)
        for rule in global_rules {
            merged.push(rule);
        }

        EffectivePermissions {
            merged,
            session_rule_count: session_count,
            project_rule_count: project_count,
            global_rule_count: global_count,
        }
    }

    /// Merge only global and project scopes (no session rules).
    #[must_use]
    pub fn merge_without_session(global: &RuleSet, project: &RuleSet) -> EffectivePermissions {
        let empty = RuleSet::new(RuleAction::Ask);
        Self::merge(global, project, &empty)
    }

    /// Determine which scope a rule at `index` in the merged set belongs to.
    #[must_use]
    pub fn scope_of_rule(effective: &EffectivePermissions, index: usize) -> Option<PermRuleScope> {
        if index < effective.session_rule_count {
            Some(PermRuleScope::Session)
        } else if index < effective.session_rule_count + effective.project_rule_count {
            Some(PermRuleScope::Project)
        } else if index < effective.total_rules() {
            Some(PermRuleScope::Global)
        } else {
            None
        }
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

    // ── Basic merge ────────────────────────────────────────

    #[test]
    fn merge_empty() {
        let global = RuleSet::new(RuleAction::Ask);
        let project = RuleSet::new(RuleAction::Ask);
        let session = RuleSet::new(RuleAction::Ask);

        let effective = PermissionInheritance::merge(&global, &project, &session);
        assert_eq!(effective.total_rules(), 0);
        assert_eq!(effective.session_rule_count(), 0);
        assert_eq!(effective.project_rule_count(), 0);
        assert_eq!(effective.global_rule_count(), 0);
    }

    #[test]
    fn merge_counts() {
        let mut global = RuleSet::new(RuleAction::Ask);
        global.push(make_rule("g1", RuleAction::Allow, PermRuleScope::Global));
        global.push(make_rule("g2", RuleAction::Deny, PermRuleScope::Global));

        let mut project = RuleSet::new(RuleAction::Ask);
        project.push(make_rule("p1", RuleAction::Allow, PermRuleScope::Project));

        let mut session = RuleSet::new(RuleAction::Ask);
        session.push(make_rule("s1", RuleAction::Deny, PermRuleScope::Session));
        session.push(make_rule("s2", RuleAction::Allow, PermRuleScope::Session));
        session.push(make_rule("s3", RuleAction::Ask, PermRuleScope::Session));

        let effective = PermissionInheritance::merge(&global, &project, &session);
        assert_eq!(effective.total_rules(), 6);
        assert_eq!(effective.session_rule_count(), 3);
        assert_eq!(effective.project_rule_count(), 1);
        assert_eq!(effective.global_rule_count(), 2);
    }

    // ── Priority / override ────────────────────────────────

    #[test]
    fn session_overrides_project() {
        let global = RuleSet::new(RuleAction::Ask);

        let mut project = RuleSet::new(RuleAction::Ask);
        project.push(make_rule("bash", RuleAction::Allow, PermRuleScope::Project));

        let mut session = RuleSet::new(RuleAction::Ask);
        session.push(make_rule("bash", RuleAction::Deny, PermRuleScope::Session));

        let effective = PermissionInheritance::merge(&global, &project, &session);
        let action = effective.evaluate("bash", None, 0);
        assert_eq!(action, RuleAction::Deny);
    }

    #[test]
    fn project_overrides_global() {
        let mut global = RuleSet::new(RuleAction::Ask);
        global.push(make_rule("bash", RuleAction::Allow, PermRuleScope::Global));

        let mut project = RuleSet::new(RuleAction::Ask);
        project.push(make_rule("bash", RuleAction::Deny, PermRuleScope::Project));

        let session = RuleSet::new(RuleAction::Ask);

        let effective = PermissionInheritance::merge(&global, &project, &session);
        let action = effective.evaluate("bash", None, 0);
        assert_eq!(action, RuleAction::Deny);
    }

    #[test]
    fn session_overrides_global() {
        let mut global = RuleSet::new(RuleAction::Ask);
        global.push(make_rule("bash", RuleAction::Deny, PermRuleScope::Global));

        let project = RuleSet::new(RuleAction::Ask);

        let mut session = RuleSet::new(RuleAction::Ask);
        session.push(make_rule("bash", RuleAction::Allow, PermRuleScope::Session));

        let effective = PermissionInheritance::merge(&global, &project, &session);
        let action = effective.evaluate("bash", None, 0);
        assert_eq!(action, RuleAction::Allow);
    }

    #[test]
    fn global_applies_when_no_override() {
        let mut global = RuleSet::new(RuleAction::Ask);
        global.push(make_rule("bash", RuleAction::Allow, PermRuleScope::Global));

        let project = RuleSet::new(RuleAction::Ask);
        let session = RuleSet::new(RuleAction::Ask);

        let effective = PermissionInheritance::merge(&global, &project, &session);
        let action = effective.evaluate("bash", None, 0);
        assert_eq!(action, RuleAction::Allow);
    }

    // ── Default action inheritance ─────────────────────────

    #[test]
    fn default_action_from_session() {
        let global = RuleSet::new(RuleAction::Allow);
        let project = RuleSet::new(RuleAction::Deny);
        let mut session = RuleSet::new(RuleAction::Ask);
        session.push(make_rule("x", RuleAction::Allow, PermRuleScope::Session));

        let effective = PermissionInheritance::merge(&global, &project, &session);
        // Default from session since it has rules
        assert_eq!(effective.rules().default_action(), RuleAction::Ask);
    }

    #[test]
    fn default_action_from_project_when_no_session() {
        let global = RuleSet::new(RuleAction::Allow);
        let mut project = RuleSet::new(RuleAction::Deny);
        project.push(make_rule("x", RuleAction::Allow, PermRuleScope::Project));
        let session = RuleSet::new(RuleAction::Ask);

        let effective = PermissionInheritance::merge(&global, &project, &session);
        assert_eq!(effective.rules().default_action(), RuleAction::Deny);
    }

    #[test]
    fn default_action_from_global_when_nothing_else() {
        let mut global = RuleSet::new(RuleAction::Allow);
        global.push(make_rule("x", RuleAction::Allow, PermRuleScope::Global));
        let project = RuleSet::new(RuleAction::Deny);
        let session = RuleSet::new(RuleAction::Ask);

        let effective = PermissionInheritance::merge(&global, &project, &session);
        assert_eq!(effective.rules().default_action(), RuleAction::Allow);
    }

    // ── scope_of_rule ──────────────────────────────────────

    #[test]
    fn scope_of_rule_identifies_correctly() {
        let mut global = RuleSet::new(RuleAction::Ask);
        global.push(make_rule("g", RuleAction::Allow, PermRuleScope::Global));

        let mut project = RuleSet::new(RuleAction::Ask);
        project.push(make_rule("p", RuleAction::Allow, PermRuleScope::Project));

        let mut session = RuleSet::new(RuleAction::Ask);
        session.push(make_rule("s", RuleAction::Allow, PermRuleScope::Session));

        let effective = PermissionInheritance::merge(&global, &project, &session);

        assert_eq!(
            PermissionInheritance::scope_of_rule(&effective, 0),
            Some(PermRuleScope::Session)
        );
        assert_eq!(
            PermissionInheritance::scope_of_rule(&effective, 1),
            Some(PermRuleScope::Project)
        );
        assert_eq!(
            PermissionInheritance::scope_of_rule(&effective, 2),
            Some(PermRuleScope::Global)
        );
        assert_eq!(PermissionInheritance::scope_of_rule(&effective, 3), None);
    }

    // ── merge_without_session ──────────────────────────────

    #[test]
    fn merge_without_session_works() {
        let mut global = RuleSet::new(RuleAction::Ask);
        global.push(make_rule("bash", RuleAction::Allow, PermRuleScope::Global));

        let mut project = RuleSet::new(RuleAction::Ask);
        project.push(make_rule("bash", RuleAction::Deny, PermRuleScope::Project));

        let effective = PermissionInheritance::merge_without_session(&global, &project);
        assert_eq!(effective.session_rule_count(), 0);
        assert_eq!(effective.evaluate("bash", None, 0), RuleAction::Deny);
    }

    // ── evaluate_detailed through effective ─────────────────

    #[test]
    fn evaluate_detailed_through_effective() {
        let mut global = RuleSet::new(RuleAction::Ask);
        global.push(make_rule("bash", RuleAction::Allow, PermRuleScope::Global));

        let mut session = RuleSet::new(RuleAction::Ask);
        session.push(make_rule("read", RuleAction::Deny, PermRuleScope::Session));

        let project = RuleSet::new(RuleAction::Ask);

        let effective = PermissionInheritance::merge(&global, &project, &session);
        let result = effective.evaluate_detailed("bash", None, 0);
        assert_eq!(result.action, RuleAction::Allow);
        assert_eq!(result.matched_rule_index, Some(1)); // index 1 = first global rule
        assert!(!result.is_default);
    }

    #[test]
    fn evaluate_detailed_default() {
        let global = RuleSet::new(RuleAction::Deny);
        let project = RuleSet::new(RuleAction::Ask);
        let session = RuleSet::new(RuleAction::Ask);

        let effective = PermissionInheritance::merge(&global, &project, &session);
        let result = effective.evaluate_detailed("unknown_tool", None, 0);
        assert!(result.is_default);
    }

    // ── Serde ──────────────────────────────────────────────

    #[test]
    fn effective_permissions_serializes() {
        let global = RuleSet::new(RuleAction::Ask);
        let project = RuleSet::new(RuleAction::Ask);
        let session = RuleSet::new(RuleAction::Ask);

        let effective = PermissionInheritance::merge(&global, &project, &session);
        let json = serde_json::to_string(&effective).unwrap();
        assert!(json.contains("merged"));
        assert!(json.contains("session_rule_count"));
    }

    #[test]
    fn effective_permissions_serde_roundtrip() {
        let mut global = RuleSet::new(RuleAction::Deny);
        global.push(make_rule("bash", RuleAction::Allow, PermRuleScope::Global));

        let project = RuleSet::new(RuleAction::Ask);
        let session = RuleSet::new(RuleAction::Ask);

        let effective = PermissionInheritance::merge(&global, &project, &session);
        let json = serde_json::to_string_pretty(&effective).unwrap();
        let back: EffectivePermissions = serde_json::from_str(&json).unwrap();
        assert_eq!(back.total_rules(), 1);
        assert_eq!(back.global_rule_count(), 1);
    }
}
