//! [`SessionConfig`] — the flat value struct passed into
//! [`super::session::AgentSession::new`].

use std::path::PathBuf;

use crab_core::model::ModelId;
use crab_core::permission::PermissionPolicy;

/// Session configuration needed to start a query loop.
#[derive(Clone)]
pub struct SessionConfig {
    pub session_id: String,
    pub system_prompt: String,
    pub model: ModelId,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
    pub context_window: u64,
    pub working_dir: std::path::PathBuf,
    pub permission_policy: PermissionPolicy,
    /// Path to memory store directory (e.g., `~/.crab/memory/`).
    pub memory_dir: Option<PathBuf>,
    /// Path to session history directory (e.g., `~/.crab/sessions/`).
    pub sessions_dir: Option<PathBuf>,
    /// Session ID to resume from (for `--resume`).
    pub resume_session_id: Option<String>,
    /// Effort level: "low", "medium", "high", "max".
    pub effort: Option<String>,
    /// Thinking mode: "enabled", "adaptive", "disabled".
    pub thinking_mode: Option<String>,
    /// Additional directories the agent may access beyond `working_dir`.
    pub additional_dirs: Vec<PathBuf>,
    /// Session display name (shown in /resume list).
    pub session_name: Option<String>,
    /// Maximum agent turns (print mode only).
    pub max_turns: Option<u32>,
    /// Maximum budget in USD (print mode only).
    pub max_budget_usd: Option<f64>,
    /// Fallback model for overloaded primary.
    pub fallback_model: Option<String>,

    // ─── B-level flags (Steps 10–13) ───
    /// Bare mode — skip hooks, plugins, auto-memory, AGENTS.md discovery.
    pub bare_mode: bool,
    /// Git worktree branch name (empty string = auto-name).
    pub worktree_name: Option<String>,
    /// Fork into new session when resuming instead of continuing old one.
    pub fork_session: bool,
    /// Load context from a GitHub PR (number or URL).
    pub from_pr: Option<String>,
    /// Custom session ID override.
    pub custom_session_id: Option<String>,
    /// JSON Schema path/inline for output validation.
    pub json_schema: Option<String>,
    /// Additional plugin directories.
    pub plugin_dirs: Vec<PathBuf>,
    /// Disable slash commands / skills.
    pub disable_skills: bool,
    /// Extra API beta headers.
    pub beta_headers: Vec<String>,
    /// Connect to IDE extension.
    pub ide_connect: bool,

    // ─── Coordinator Mode gating (Phase 1) ───
    /// Enable Layer 2b Coordinator Mode (tool ACL + anti-pattern prompt overlay).
    /// Gated on `CRAB_COORDINATOR_MODE=1` env only — Agent Teams, `TaskList`, and
    /// `Mailbox` are unconditional base infrastructure and do not need a flag.
    pub coordinator_mode: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_config_construction() {
        let config = SessionConfig {
            session_id: "sess_1".into(),
            system_prompt: "You are helpful.".into(),
            model: ModelId::from("claude-sonnet-4-20250514"),
            max_tokens: 4096,
            temperature: None,
            context_window: 200_000,
            working_dir: std::path::PathBuf::from("/tmp"),
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
            coordinator_mode: false,
        };
        assert_eq!(config.session_id, "sess_1");
        assert_eq!(config.context_window, 200_000);
        assert!(!config.coordinator_mode);
    }
}
