//! `crab permissions` subcommand — inspect the permission rule store and
//! audit log.

use clap::Subcommand;

use crab_core::permission::stored::{
    AuditEntry, AuditSource, PermissionRuleSet, RuleVerdict, load_permission_store,
};

#[derive(Subcommand)]
pub enum PermissionsAction {
    /// Show the audit log: every permission decision recorded for this
    /// install, plus the store path that holds them.
    Audit {
        /// Limit output to the most recent N entries (default: 50).
        #[arg(long, default_value = "50")]
        limit: usize,
        /// Use the project-level store instead of the global one.
        #[arg(long)]
        project: bool,
    },
}

pub fn run(action: &PermissionsAction) -> anyhow::Result<()> {
    match action {
        PermissionsAction::Audit { limit, project } => run_audit(*limit, *project),
    }
}

fn run_audit(limit: usize, project: bool) -> anyhow::Result<()> {
    let path = if project {
        let cwd = std::env::current_dir().unwrap_or_default();
        PermissionRuleSet::project_path(&cwd)
    } else {
        PermissionRuleSet::global_path(&crab_utils::path::home_dir())
    };

    let mut ruleset = PermissionRuleSet::new(path.clone());
    // load() silently treats missing file as empty; also fail-soft on
    // parse errors so a corrupted log does not brick the subcommand.
    if path.exists() {
        match load_permission_store(&path) {
            Ok(store) => {
                for entry in store.audit_log {
                    ruleset.record_audit(
                        &entry.tool_name,
                        entry.verdict,
                        entry.source,
                        entry.context,
                    );
                }
            }
            Err(e) => {
                eprintln!("warning: could not read {}: {e}", path.display());
            }
        }
    }

    println!("Permission store: {}", ruleset.store_path().display());
    println!("Audit entries: {}", ruleset.audit_log_len());
    println!();

    let log: &[AuditEntry] = ruleset.audit_log();
    if log.is_empty() {
        println!("(no decisions recorded yet)");
        return Ok(());
    }

    let start = log.len().saturating_sub(limit);
    for entry in &log[start..] {
        let verdict = match entry.verdict {
            RuleVerdict::Allow => "ALLOW",
            RuleVerdict::Deny => "DENY ",
        };
        let source = match entry.source {
            AuditSource::StoredRule => "rule",
            AuditSource::Interactive => "prompt",
            AuditSource::Policy => "policy",
        };
        let ctx = entry.context.as_deref().unwrap_or("");
        println!(
            "  {ts}  {verdict}  {tool:<24}  [{source}]  {ctx}",
            ts = entry.timestamp,
            tool = entry.tool_name,
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::permission::stored::{AuditEntry, PermissionStore, save_permission_store};

    #[test]
    fn run_audit_with_empty_store_is_ok() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("permissions.json");
        let mut ruleset = PermissionRuleSet::new(path.clone());
        assert_eq!(ruleset.audit_log_len(), 0);
        let _ = ruleset.load();
        assert_eq!(ruleset.audit_log_len(), 0);
    }

    #[test]
    fn run_audit_counts_stored_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("permissions.json");
        let store = PermissionStore {
            rules: Vec::new(),
            audit_log: vec![
                AuditEntry {
                    timestamp: "2026-04-24T00:00:00Z".into(),
                    tool_name: "bash".into(),
                    verdict: RuleVerdict::Allow,
                    source: AuditSource::Interactive,
                    context: Some("ls".into()),
                },
                AuditEntry {
                    timestamp: "2026-04-24T00:00:01Z".into(),
                    tool_name: "write".into(),
                    verdict: RuleVerdict::Deny,
                    source: AuditSource::Policy,
                    context: None,
                },
            ],
        };
        save_permission_store(&path, &store).unwrap();

        let mut ruleset = PermissionRuleSet::new(path);
        ruleset.load().unwrap();
        assert_eq!(ruleset.audit_log_len(), 2);
        assert_eq!(ruleset.audit_log()[0].tool_name, "bash");
    }
}
