//! TUI REPL runner — wires App, agent runtime, and terminal lifecycle together.
//!
//! Features:
//! - Full agent query loop with tool execution
//! - Permission dialog integration via `PermissionDialog` component
//! - Tool execution progress (spinner) and result display in content area
//! - Session persistence (auto-save on exit, `--resume` support)
//! - Skill `/command` input detection and resolution via `SkillRegistry`

use std::path::PathBuf;
use std::sync::Arc;

use crossterm::event::DisableBracketedPaste;
use crossterm::execute;
use crossterm::terminal::{LeaveAlternateScreen, disable_raw_mode};

use crab_agents::{LlmBackend, SessionConfig};

mod init;
mod repl;
mod slash;

/// Information returned when the TUI exits.
pub struct ExitInfo {
    pub session_id: String,
    pub had_conversation: bool,
}

/// Configuration for launching the TUI REPL.
pub struct TuiConfig {
    pub session_config: SessionConfig,
    pub backend: Arc<LlmBackend>,
    /// Skill directories to scan for `/command` support.
    pub skill_dirs: Vec<PathBuf>,
    /// MCP server configuration from settings (for dynamic tool registration).
    pub mcp_servers: Option<serde_json::Value>,
    /// Validation warnings from settings loading (shown as toasts after init).
    pub settings_warnings: Vec<String>,
}

/// Run the interactive TUI REPL. This is the main entry point for interactive mode.
///
/// Uses a UI-first strategy: the TUI is displayed immediately in an
/// `Initializing` state while MCP, memory, session, and skill loading
/// happen in a background task. Once ready, the event loop receives
/// `InitResult` via a oneshot channel and transitions to `Idle`.
pub async fn run(config: TuiConfig) -> anyhow::Result<ExitInfo> {
    let mut prepared = init::prepare(config)?;

    let result = repl::run_loop(
        &mut prepared.terminal,
        &mut prepared.app,
        &mut prepared.tui_rx,
        prepared.init_rx,
        &mut prepared.watch_rx,
        prepared.backend,
        prepared.event_tx,
        prepared.tagged_tx,
        prepared.perm_resp_tx,
        &prepared.session_id,
        Arc::clone(&prepared.event_broker),
        prepared.frame_requester.clone(),
        &prepared.skill_dirs,
    )
    .await;

    let exit_info = ExitInfo {
        session_id: prepared.app.session_id.clone(),
        had_conversation: !prepared.app.messages.is_empty(),
    };

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        prepared.terminal.backend_mut(),
        DisableBracketedPaste,
        LeaveAlternateScreen
    )?;
    prepared.terminal.show_cursor()?;

    result.map(|()| exit_info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tui_config_construction() {
        let config = TuiConfig {
            session_config: SessionConfig {
                session_id: "test".into(),
                system_prompt: "You are helpful.".into(),
                model: crab_core::model::ModelId::from("test-model"),
                max_tokens: 4096,
                temperature: None,
                context_window: 200_000,
                working_dir: PathBuf::from("/tmp"),
                permission_policy: crab_core::permission::PermissionPolicy::default(),
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
                default_shell: "bash".into(),
            },
            backend: Arc::new(LlmBackend::OpenAi(crab_agents::openai::OpenAiClient::new(
                "http://localhost:0/v1",
                None,
            ))),
            skill_dirs: vec![],
            mcp_servers: None,
            settings_warnings: vec![],
        };
        assert_eq!(config.session_config.session_id, "test");
        assert!(config.skill_dirs.is_empty());
    }
}
