//! Permission decision persistence and audit logging.
//!
//! Stores user-granted or denied permission rules at `.crab/permissions.json`
//! (project-level) or `~/.crab/permissions.json` (global). Rules can be
//! session-scoped (ephemeral) or permanent (persisted to disk).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ── Data model ────────────────────────────────────────────────────────

/// Scope of a permission rule — session-only or persisted permanently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleScope {
    /// Lasts only for the current session (kept in memory, not on disk).
    Session,
    /// Persisted to `permissions.json` and survives restarts.
    Permanent,
}

/// The verdict recorded for a permission rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleVerdict {
    Allow,
    Deny,
}

/// A single permission rule — records a user's decision about a tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRule {
    /// Tool name or glob pattern (e.g. `"bash"`, `"mcp__*"`).
    pub tool_pattern: String,
    /// Whether this tool invocation is allowed or denied.
    pub verdict: RuleVerdict,
    /// Session-only or permanent.
    pub scope: RuleScope,
    /// ISO 8601 timestamp of when the rule was created.
    pub created_at: String,
    /// Optional description of the specific command/args that triggered this rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

/// An entry in the audit log — records every permission decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    /// ISO 8601 timestamp.
    pub timestamp: String,
    /// The tool that was checked.
    pub tool_name: String,
    /// What the user decided.
    pub verdict: RuleVerdict,
    /// Whether the decision was from a stored rule or interactive.
    pub source: AuditSource,
    /// Optional extra context (e.g. the command string for bash).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

/// How a permission decision was reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditSource {
    /// Matched a stored rule (session or permanent).
    StoredRule,
    /// User was prompted interactively.
    Interactive,
    /// Policy auto-decision (e.g. `Dangerously` mode).
    Policy,
}

/// On-disk format for `permissions.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionStore {
    /// Only permanent rules are persisted.
    #[serde(default)]
    pub rules: Vec<PermissionRule>,
    /// Audit log of recent permission decisions.
    #[serde(default)]
    pub audit_log: Vec<AuditEntry>,
}

// ── PermissionRuleSet — in-memory rule manager ────────────────────────

/// Manages permission rules in memory, with load/save to disk.
///
/// Holds both session-scoped and permanent rules. Only permanent rules
/// are written to / read from disk.
pub struct PermissionRuleSet {
    /// All rules (session + permanent).
    rules: Vec<PermissionRule>,
    /// Audit log entries accumulated during this session.
    audit_log: Vec<AuditEntry>,
    /// Path to the permissions file on disk.
    store_path: PathBuf,
}

impl std::fmt::Debug for PermissionRuleSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PermissionRuleSet")
            .field("rules_count", &self.rules.len())
            .field("audit_log_count", &self.audit_log.len())
            .field("store_path", &self.store_path)
            .finish_non_exhaustive()
    }
}

impl PermissionRuleSet {
    /// Create a new empty rule set that will persist to `store_path`.
    #[must_use]
    pub fn new(store_path: PathBuf) -> Self {
        Self {
            rules: Vec::new(),
            audit_log: Vec::new(),
            store_path,
        }
    }

    /// Default global permissions path: `~/.crab/permissions.json`.
    #[must_use]
    pub fn global_path() -> PathBuf {
        crab_core::common::utils::path::home_dir()
            .join(".crab")
            .join("permissions.json")
    }

    /// Project-level permissions path: `<project_dir>/.crab/permissions.json`.
    #[must_use]
    pub fn project_path(project_dir: &Path) -> PathBuf {
        project_dir.join(".crab").join("permissions.json")
    }

    // ── CRUD API ──────────────────────────────────────────────────────

    /// Add a new permission rule. If a rule with the same `tool_pattern`
    /// and `scope` already exists, it is replaced.
    pub fn add_rule(&mut self, rule: PermissionRule) {
        self.rules
            .retain(|r| !(r.tool_pattern == rule.tool_pattern && r.scope == rule.scope));
        self.rules.push(rule);
    }

    /// Remove all rules matching the given tool pattern (both scopes).
    /// Returns the number of rules removed.
    pub fn remove_rules(&mut self, tool_pattern: &str) -> usize {
        let before = self.rules.len();
        self.rules.retain(|r| r.tool_pattern != tool_pattern);
        before - self.rules.len()
    }

    /// Remove rules matching a specific tool pattern and scope.
    /// Returns the number of rules removed.
    pub fn remove_rules_by_scope(&mut self, tool_pattern: &str, scope: RuleScope) -> usize {
        let before = self.rules.len();
        self.rules
            .retain(|r| !(r.tool_pattern == tool_pattern && r.scope == scope));
        before - self.rules.len()
    }

    /// List all current rules.
    #[must_use]
    pub fn list_rules(&self) -> &[PermissionRule] {
        &self.rules
    }

    /// List only permanent rules.
    #[must_use]
    pub fn list_permanent_rules(&self) -> Vec<&PermissionRule> {
        self.rules
            .iter()
            .filter(|r| r.scope == RuleScope::Permanent)
            .collect()
    }

    /// List only session rules.
    #[must_use]
    pub fn list_session_rules(&self) -> Vec<&PermissionRule> {
        self.rules
            .iter()
            .filter(|r| r.scope == RuleScope::Session)
            .collect()
    }

    /// Clear all session-scoped rules (e.g. on session end).
    pub fn clear_session_rules(&mut self) {
        self.rules.retain(|r| r.scope != RuleScope::Session);
    }

    // ── Query ─────────────────────────────────────────────────────────

    /// Look up the first matching rule for a tool name.
    ///
    /// Checks exact matches first, then glob patterns. Returns `None`
    /// if no rule matches (meaning the caller should prompt the user).
    #[must_use]
    pub fn check(&self, tool_name: &str) -> Option<&PermissionRule> {
        // Exact match first (more specific)
        if let Some(rule) = self.rules.iter().find(|r| r.tool_pattern == tool_name) {
            return Some(rule);
        }
        // Glob match
        self.rules
            .iter()
            .find(|r| r.tool_pattern.contains('*') && glob_match(&r.tool_pattern, tool_name))
    }

    // ── Audit log ─────────────────────────────────────────────────────

    /// Record a permission decision in the audit log.
    pub fn record_audit(
        &mut self,
        tool_name: &str,
        verdict: RuleVerdict,
        source: AuditSource,
        context: Option<String>,
    ) {
        self.audit_log.push(AuditEntry {
            timestamp: now_iso8601(),
            tool_name: tool_name.to_string(),
            verdict,
            source,
            context,
        });
    }

    /// Get the audit log.
    #[must_use]
    pub fn audit_log(&self) -> &[AuditEntry] {
        &self.audit_log
    }

    /// Get the number of audit entries.
    #[allow(dead_code)]
    #[must_use]
    pub fn audit_log_len(&self) -> usize {
        self.audit_log.len()
    }

    // ── Persistence ───────────────────────────────────────────────────

    /// Load permanent rules (and audit log) from disk.
    /// Missing file is treated as empty. Session rules are not affected.
    pub fn load(&mut self) -> crab_core::Result<()> {
        let store = load_permission_store(&self.store_path)?;
        // Merge loaded permanent rules, keeping existing session rules
        let session_rules: Vec<PermissionRule> = self
            .rules
            .drain(..)
            .filter(|r| r.scope == RuleScope::Session)
            .collect();
        self.rules = store.rules;
        self.rules.extend(session_rules);
        self.audit_log.extend(store.audit_log);
        Ok(())
    }

    /// Save permanent rules and audit log to disk.
    /// Session-scoped rules are excluded from the persisted file.
    pub fn save(&self) -> crab_core::Result<()> {
        let store = PermissionStore {
            rules: self
                .rules
                .iter()
                .filter(|r| r.scope == RuleScope::Permanent)
                .cloned()
                .collect(),
            audit_log: self.audit_log.clone(),
        };
        save_permission_store(&self.store_path, &store)
    }

    /// Get the store path.
    #[allow(dead_code)]
    #[must_use]
    pub fn store_path(&self) -> &Path {
        &self.store_path
    }
}

// ── File I/O helpers ──────────────────────────────────────────────────

/// Load a `PermissionStore` from a JSON file.
/// Returns `Ok(default)` if the file does not exist.
pub fn load_permission_store(path: &Path) -> crab_core::Result<PermissionStore> {
    match std::fs::read_to_string(path) {
        Ok(content) => serde_json::from_str(&content).map_err(|e| {
            crab_core::Error::Config(format!("failed to parse {}: {e}", path.display()))
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(PermissionStore::default()),
        Err(e) => Err(crab_core::Error::Config(format!(
            "failed to read {}: {e}",
            path.display()
        ))),
    }
}

/// Save a `PermissionStore` to a JSON file, creating parent directories.
pub fn save_permission_store(path: &Path, store: &PermissionStore) -> crab_core::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            crab_core::Error::Config(format!("failed to create {}: {e}", parent.display()))
        })?;
    }
    let json = serde_json::to_string_pretty(store)
        .map_err(|e| crab_core::Error::Config(format!("failed to serialize permissions: {e}")))?;
    std::fs::write(path, json)
        .map_err(|e| crab_core::Error::Config(format!("failed to write {}: {e}", path.display())))
}

// ── Helpers ───────────────────────────────────────────────────────────

/// Simple glob matching (reuse from `policy.rs` logic).
fn glob_match(pattern: &str, input: &str) -> bool {
    let pat: Vec<char> = pattern.chars().collect();
    let inp: Vec<char> = input.chars().collect();
    glob_match_inner(&pat, &inp)
}

fn glob_match_inner(pat: &[char], input: &[char]) -> bool {
    let (mut pi, mut ii) = (0, 0);
    let (mut star_pat, mut star_input) = (usize::MAX, usize::MAX);

    while ii < input.len() {
        if pi < pat.len() && (pat[pi] == '?' || pat[pi] == input[ii]) {
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

/// Return current time as ISO 8601 string (UTC).
fn now_iso8601() -> String {
    // Use std::time for a simple UTC timestamp without pulling in chrono.
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    // Convert to a basic ISO 8601 representation.
    // days since epoch → year/month/day, then hours:minutes:seconds
    format_unix_timestamp(secs)
}

/// Format a Unix timestamp as `YYYY-MM-DDTHH:MM:SSZ`.
fn format_unix_timestamp(secs: u64) -> String {
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Compute year/month/day from days since 1970-01-01
    let (year, month, day) = days_to_ymd(days);
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

/// Convert days since epoch to (year, month, day).
fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Algorithm from Howard Hinnant's civil_from_days
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a unique temporary directory + `permissions.json` path under it.
    ///
    /// Returns `(TempDir, PathBuf)` — the caller MUST keep `TempDir` alive for
    /// the duration of the test, otherwise the directory is removed on drop.
    /// Uses `tempfile` (OS-level uniqueness) to avoid race conditions when
    /// multiple nextest processes hit the same nanosecond-based path on
    /// coarse-clock CI runners (observed on macOS).
    fn temp_store() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::Builder::new()
            .prefix("crab-perm-test-")
            .tempdir()
            .expect("create tempdir");
        let path = dir.path().join("permissions.json");
        (dir, path)
    }

    fn make_rule(pattern: &str, verdict: RuleVerdict, scope: RuleScope) -> PermissionRule {
        PermissionRule {
            tool_pattern: pattern.to_string(),
            verdict,
            scope,
            created_at: "2026-04-05T00:00:00Z".to_string(),
            context: None,
        }
    }

    // ── RuleScope / RuleVerdict serde ─────────────────────────────────

    #[test]
    fn rule_scope_serde_roundtrip() {
        for scope in [RuleScope::Session, RuleScope::Permanent] {
            let json = serde_json::to_string(&scope).unwrap();
            let parsed: RuleScope = serde_json::from_str(&json).unwrap();
            assert_eq!(scope, parsed);
        }
    }

    #[test]
    fn rule_verdict_serde_roundtrip() {
        for verdict in [RuleVerdict::Allow, RuleVerdict::Deny] {
            let json = serde_json::to_string(&verdict).unwrap();
            let parsed: RuleVerdict = serde_json::from_str(&json).unwrap();
            assert_eq!(verdict, parsed);
        }
    }

    #[test]
    fn audit_source_serde_roundtrip() {
        for source in [
            AuditSource::StoredRule,
            AuditSource::Interactive,
            AuditSource::Policy,
        ] {
            let json = serde_json::to_string(&source).unwrap();
            let parsed: AuditSource = serde_json::from_str(&json).unwrap();
            assert_eq!(source, parsed);
        }
    }

    // ── PermissionRule serde ──────────────────────────────────────────

    #[test]
    fn permission_rule_serde_roundtrip() {
        let rule = PermissionRule {
            tool_pattern: "bash".to_string(),
            verdict: RuleVerdict::Allow,
            scope: RuleScope::Permanent,
            created_at: "2026-04-05T12:00:00Z".to_string(),
            context: Some("rm -rf /tmp/test".to_string()),
        };
        let json = serde_json::to_string_pretty(&rule).unwrap();
        let parsed: PermissionRule = serde_json::from_str(&json).unwrap();
        assert_eq!(rule, parsed);
    }

    #[test]
    fn permission_rule_no_context() {
        let rule = make_rule("read", RuleVerdict::Allow, RuleScope::Session);
        let json = serde_json::to_string(&rule).unwrap();
        assert!(!json.contains("context"));
        let parsed: PermissionRule = serde_json::from_str(&json).unwrap();
        assert!(parsed.context.is_none());
    }

    // ── PermissionStore serde ─────────────────────────────────────────

    #[test]
    fn permission_store_default_is_empty() {
        let store = PermissionStore::default();
        assert!(store.rules.is_empty());
        assert!(store.audit_log.is_empty());
    }

    #[test]
    fn permission_store_serde_roundtrip() {
        let store = PermissionStore {
            rules: vec![make_rule("bash", RuleVerdict::Allow, RuleScope::Permanent)],
            audit_log: vec![AuditEntry {
                timestamp: "2026-04-05T12:00:00Z".to_string(),
                tool_name: "bash".to_string(),
                verdict: RuleVerdict::Allow,
                source: AuditSource::Interactive,
                context: None,
            }],
        };
        let json = serde_json::to_string_pretty(&store).unwrap();
        let parsed: PermissionStore = serde_json::from_str(&json).unwrap();
        assert_eq!(store, parsed);
    }

    // ── PermissionRuleSet CRUD ────────────────────────────────────────

    #[test]
    fn add_rule_and_list() {
        let (_tmp, path) = temp_store();
        let mut set = PermissionRuleSet::new(path);
        set.add_rule(make_rule("bash", RuleVerdict::Allow, RuleScope::Session));
        set.add_rule(make_rule("read", RuleVerdict::Allow, RuleScope::Permanent));

        assert_eq!(set.list_rules().len(), 2);
        assert_eq!(set.list_session_rules().len(), 1);
        assert_eq!(set.list_permanent_rules().len(), 1);
    }

    #[test]
    fn add_rule_replaces_same_pattern_and_scope() {
        let (_tmp, path) = temp_store();
        let mut set = PermissionRuleSet::new(path);
        set.add_rule(make_rule("bash", RuleVerdict::Allow, RuleScope::Session));
        set.add_rule(make_rule("bash", RuleVerdict::Deny, RuleScope::Session));

        assert_eq!(set.list_rules().len(), 1);
        assert_eq!(set.list_rules()[0].verdict, RuleVerdict::Deny);
    }

    #[test]
    fn add_rule_different_scopes_coexist() {
        let (_tmp, path) = temp_store();
        let mut set = PermissionRuleSet::new(path);
        set.add_rule(make_rule("bash", RuleVerdict::Allow, RuleScope::Session));
        set.add_rule(make_rule("bash", RuleVerdict::Deny, RuleScope::Permanent));

        assert_eq!(set.list_rules().len(), 2);
    }

    #[test]
    fn remove_rules_by_pattern() {
        let (_tmp, path) = temp_store();
        let mut set = PermissionRuleSet::new(path);
        set.add_rule(make_rule("bash", RuleVerdict::Allow, RuleScope::Session));
        set.add_rule(make_rule("bash", RuleVerdict::Deny, RuleScope::Permanent));
        set.add_rule(make_rule("read", RuleVerdict::Allow, RuleScope::Session));

        let removed = set.remove_rules("bash");
        assert_eq!(removed, 2);
        assert_eq!(set.list_rules().len(), 1);
        assert_eq!(set.list_rules()[0].tool_pattern, "read");
    }

    #[test]
    fn remove_rules_by_scope() {
        let (_tmp, path) = temp_store();
        let mut set = PermissionRuleSet::new(path);
        set.add_rule(make_rule("bash", RuleVerdict::Allow, RuleScope::Session));
        set.add_rule(make_rule("bash", RuleVerdict::Deny, RuleScope::Permanent));

        let removed = set.remove_rules_by_scope("bash", RuleScope::Session);
        assert_eq!(removed, 1);
        assert_eq!(set.list_rules().len(), 1);
        assert_eq!(set.list_rules()[0].scope, RuleScope::Permanent);
    }

    #[test]
    fn remove_nonexistent_returns_zero() {
        let (_tmp, path) = temp_store();
        let mut set = PermissionRuleSet::new(path);
        assert_eq!(set.remove_rules("nonexistent"), 0);
    }

    #[test]
    fn clear_session_rules() {
        let (_tmp, path) = temp_store();
        let mut set = PermissionRuleSet::new(path);
        set.add_rule(make_rule("bash", RuleVerdict::Allow, RuleScope::Session));
        set.add_rule(make_rule("read", RuleVerdict::Allow, RuleScope::Permanent));
        set.add_rule(make_rule("write", RuleVerdict::Deny, RuleScope::Session));

        set.clear_session_rules();
        assert_eq!(set.list_rules().len(), 1);
        assert_eq!(set.list_rules()[0].tool_pattern, "read");
    }

    // ── Query (check) ─────────────────────────────────────────────────

    #[test]
    fn check_exact_match() {
        let (_tmp, path) = temp_store();
        let mut set = PermissionRuleSet::new(path);
        set.add_rule(make_rule("bash", RuleVerdict::Allow, RuleScope::Session));

        let result = set.check("bash");
        assert!(result.is_some());
        assert_eq!(result.unwrap().verdict, RuleVerdict::Allow);
    }

    #[test]
    fn check_glob_match() {
        let (_tmp, path) = temp_store();
        let mut set = PermissionRuleSet::new(path);
        set.add_rule(make_rule("mcp__*", RuleVerdict::Deny, RuleScope::Permanent));

        let result = set.check("mcp__playwright_click");
        assert!(result.is_some());
        assert_eq!(result.unwrap().verdict, RuleVerdict::Deny);
    }

    #[test]
    fn check_exact_takes_priority_over_glob() {
        let (_tmp, path) = temp_store();
        let mut set = PermissionRuleSet::new(path);
        set.add_rule(make_rule("mcp__*", RuleVerdict::Deny, RuleScope::Permanent));
        set.add_rule(make_rule(
            "mcp__safe_tool",
            RuleVerdict::Allow,
            RuleScope::Permanent,
        ));

        let result = set.check("mcp__safe_tool");
        assert!(result.is_some());
        assert_eq!(result.unwrap().verdict, RuleVerdict::Allow);
    }

    #[test]
    fn check_no_match_returns_none() {
        let (_tmp, path) = temp_store();
        let mut set = PermissionRuleSet::new(path);
        set.add_rule(make_rule("bash", RuleVerdict::Allow, RuleScope::Session));

        assert!(set.check("write").is_none());
    }

    // ── Audit log ─────────────────────────────────────────────────────

    #[test]
    fn record_audit_entries() {
        let (_tmp, path) = temp_store();
        let mut set = PermissionRuleSet::new(path);
        set.record_audit("bash", RuleVerdict::Allow, AuditSource::Interactive, None);
        set.record_audit(
            "write",
            RuleVerdict::Deny,
            AuditSource::StoredRule,
            Some("write to /etc".to_string()),
        );

        assert_eq!(set.audit_log().len(), 2);
        assert_eq!(set.audit_log()[0].tool_name, "bash");
        assert_eq!(set.audit_log()[1].tool_name, "write");
        assert_eq!(set.audit_log()[1].source, AuditSource::StoredRule);
        assert!(set.audit_log()[1].context.is_some());
    }

    #[test]
    fn audit_entry_has_timestamp() {
        let (_tmp, path) = temp_store();
        let mut set = PermissionRuleSet::new(path);
        set.record_audit("bash", RuleVerdict::Allow, AuditSource::Policy, None);

        let entry = &set.audit_log()[0];
        // Should be a valid ISO 8601 timestamp
        assert!(entry.timestamp.contains('T'));
        assert!(entry.timestamp.ends_with('Z'));
    }

    // ── Persistence (save/load) ───────────────────────────────────────

    #[test]
    fn save_and_load_roundtrip() {
        let (_tmp, path) = temp_store();
        let mut set = PermissionRuleSet::new(path.clone());

        // Add rules of both scopes
        set.add_rule(make_rule("bash", RuleVerdict::Allow, RuleScope::Permanent));
        set.add_rule(make_rule("read", RuleVerdict::Allow, RuleScope::Session));
        set.add_rule(make_rule("mcp__*", RuleVerdict::Deny, RuleScope::Permanent));
        set.record_audit("bash", RuleVerdict::Allow, AuditSource::Interactive, None);

        set.save().unwrap();

        // Load into a fresh rule set
        let mut set2 = PermissionRuleSet::new(path);
        set2.load().unwrap();

        // Only permanent rules should be loaded
        assert_eq!(set2.list_rules().len(), 2);
        assert!(set2.list_session_rules().is_empty());
        assert_eq!(set2.list_permanent_rules().len(), 2);
        assert_eq!(set2.audit_log().len(), 1);
    }

    #[test]
    fn load_preserves_session_rules() {
        let (_tmp, path) = temp_store();

        // Save a permanent rule
        let store = PermissionStore {
            rules: vec![make_rule("bash", RuleVerdict::Allow, RuleScope::Permanent)],
            audit_log: Vec::new(),
        };
        save_permission_store(&path, &store).unwrap();

        // Create a rule set with a session rule, then load
        let mut set = PermissionRuleSet::new(path);
        set.add_rule(make_rule("write", RuleVerdict::Deny, RuleScope::Session));
        set.load().unwrap();

        // Both should be present
        assert_eq!(set.list_rules().len(), 2);
        assert!(set.check("bash").is_some());
        assert!(set.check("write").is_some());
    }

    #[test]
    fn load_nonexistent_file_returns_empty() {
        let mut set = PermissionRuleSet::new(PathBuf::from("/nonexistent/permissions.json"));
        assert!(set.load().is_ok());
        assert!(set.list_rules().is_empty());
    }

    #[test]
    fn save_creates_parent_dirs() {
        let (_tmp, path) = temp_store();
        let set = PermissionRuleSet::new(path.clone());
        assert!(set.save().is_ok());
        assert!(path.exists());
    }

    // ── File I/O helpers ──────────────────────────────────────────────

    #[test]
    fn load_invalid_json_returns_error() {
        let (_tmp, path) = temp_store();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, "not json").unwrap();

        let result = load_permission_store(&path);
        assert!(result.is_err());
    }

    // ── Path helpers ──────────────────────────────────────────────────

    #[test]
    fn global_path_under_crab() {
        let path = PermissionRuleSet::global_path();
        assert!(path.ends_with("permissions.json"));
        let parent = path.parent().unwrap();
        assert!(parent.ends_with(".crab"));
    }

    #[test]
    fn project_path_under_crab() {
        let path = PermissionRuleSet::project_path(Path::new("/my/project"));
        assert!(path.ends_with("permissions.json"));
        assert!(path.to_string_lossy().contains(".crab"));
    }

    // ── Timestamp helpers ─────────────────────────────────────────────

    #[test]
    fn format_unix_timestamp_epoch() {
        assert_eq!(format_unix_timestamp(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn format_unix_timestamp_known_date() {
        // 2026-04-05T00:00:00Z = 1775347200
        let ts = format_unix_timestamp(1_775_347_200);
        assert!(ts.starts_with("2026-04-05"));
        assert!(ts.ends_with('Z'));
    }

    #[test]
    fn now_iso8601_looks_reasonable() {
        let ts = now_iso8601();
        assert!(ts.contains('T'));
        assert!(ts.ends_with('Z'));
        assert!(ts.starts_with("20")); // Year 20xx
    }

    // ── Glob matching ─────────────────────────────────────────────────

    #[test]
    fn glob_exact() {
        assert!(glob_match("bash", "bash"));
        assert!(!glob_match("bash", "read"));
    }

    #[test]
    fn glob_star() {
        assert!(glob_match("mcp__*", "mcp__playwright_click"));
        assert!(glob_match("*tool", "my_tool"));
        assert!(!glob_match("mcp__*", "bash"));
    }

    #[test]
    fn glob_question() {
        assert!(glob_match("tool_?", "tool_a"));
        assert!(!glob_match("tool_?", "tool_ab"));
    }

    // ── Debug impl ────────────────────────────────────────────────────

    #[test]
    fn debug_impl() {
        let set = PermissionRuleSet::new(PathBuf::from("/tmp/test.json"));
        let debug = format!("{set:?}");
        assert!(debug.contains("PermissionRuleSet"));
        assert!(debug.contains("rules_count"));
    }
}
