//! Coordinator Mode activation gate.
//!
//! Kept behind a dedicated module (rather than a bare `if config.coordinator_mode`
//! in [`crate::session::AgentSession::new`]) so that future extensions —
//! settings.json overrides, per-project disables, GrowthBook-style kill
//! switches — can land here without touching session init.

use crate::session::SessionConfig;

/// Whether Coordinator Mode should be active for the given session config.
///
/// Today this is a simple read of `SessionConfig::coordinator_mode`, which
/// the CLI wires from the `CRAB_COORDINATOR_MODE` env var. More signals
/// (settings file, experimental kill switch) can AND into this predicate
/// without changing callers.
#[must_use]
pub fn is_active(config: &SessionConfig) -> bool {
    config.coordinator_mode
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::model::ModelId;
    use crab_core::permission::PermissionPolicy;
    use std::path::PathBuf;

    fn config_with(coordinator_mode: bool) -> SessionConfig {
        SessionConfig {
            session_id: "t".into(),
            system_prompt: String::new(),
            model: ModelId::from("test-model"),
            max_tokens: 0,
            temperature: None,
            context_window: 0,
            working_dir: PathBuf::from("/tmp"),
            permission_policy: PermissionPolicy::default(),
            memory_dir: None,
            sessions_dir: None,
            resume_session_id: None,
            effort: None,
            thinking_mode: None,
            additional_dirs: Vec::new(),
            session_name: None,
            max_turns: None,
            max_budget_usd: None,
            fallback_model: None,
            bare_mode: false,
            worktree_name: None,
            fork_session: false,
            from_pr: None,
            custom_session_id: None,
            json_schema: None,
            plugin_dirs: Vec::new(),
            disable_skills: false,
            beta_headers: Vec::new(),
            ide_connect: false,
            coordinator_mode,
            default_shell: "bash".into(),
        }
    }

    #[test]
    fn inactive_by_default() {
        assert!(!is_active(&config_with(false)));
    }

    #[test]
    fn active_when_flag_set() {
        assert!(is_active(&config_with(true)));
    }
}
