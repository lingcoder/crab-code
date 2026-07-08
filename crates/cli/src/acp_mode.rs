//! ACP agent mode assembly — resolve config, build the LLM backend,
//! and serve `crab_acp::AcpServer` over stdio with an
//! [`AgentHandler`] implementation that delegates to `crab-agents`.

use std::path::PathBuf;
use std::sync::Arc;

use crab_acp::{AcpServer, AgentHandler, HandlerError, PromptTurn};
use crab_agents::{AgentSession, SessionConfig};
use crab_api::LlmBackend;
use crab_core::event::Event;
use crab_core::model::ModelId;
use crab_core::permission::PermissionPolicy;
use crab_tools::builtin::create_default_registry;
use tokio::sync::mpsc;

/// Build the ACP agent from config and run it until stdin closes.
pub async fn run() -> anyhow::Result<()> {
    let working_dir = std::env::current_dir()?;
    let ctx = crab_config::ResolveContext::new()
        .with_project_dir(Some(working_dir.clone()))
        .with_process_env();
    let settings = crab_config::resolve(&ctx).unwrap_or_default();
    let backend = Arc::new(crab_api::create_backend(&settings));

    let handler = CrabAgentHandler {
        backend,
        system_prompt: "You are crab, an AI coding assistant.".to_string(),
        working_dir,
        settings,
    };

    AcpServer::new(handler).serve_stdio().await?;
    Ok(())
}

/// Engine glue passed to `crab-acp`: each prompt turn runs on a fresh
/// `AgentSession` wired to the turn's event sink and cancel token.
struct CrabAgentHandler {
    backend: Arc<LlmBackend>,
    system_prompt: String,
    working_dir: PathBuf,
    settings: crab_config::Config,
}

impl AgentHandler for CrabAgentHandler {
    async fn run_prompt(&self, turn: PromptTurn) -> Result<(), HandlerError> {
        let registry = create_default_registry();
        let mut session = AgentSession::new(
            self.session_config(&turn.session_id),
            Arc::clone(&self.backend),
            registry,
        );

        // ACP has no interactive permission prompt yet; the editor/ACP
        // client is the trust boundary. Opt into unattended auto-approval
        // explicitly so the session is usable, rather than relying on a
        // silent fail-open default. A real ACP permission relay to the
        // client would replace this.
        session.executor.set_allow_unattended(true);

        // Route engine events into the ACP turn channel. The query loop
        // emits through `event_tx`; teammate runners hold a clone of the
        // construction-time sender, so its receiver is drained too.
        session.event_tx = turn.events.clone();
        let mut internal_rx = take_event_rx(&mut session);
        let forward = turn.events;
        tokio::spawn(async move {
            while let Some(event) = internal_rx.recv().await {
                if forward.send(event).await.is_err() {
                    break;
                }
            }
        });
        session.cancel = turn.cancel;

        let result = session.handle_user_input(&turn.prompt).await;
        // The session is per-turn here: kill any teammates it spawned so no
        // orphan tokio tasks outlive the turn (run even when the turn errs).
        session.shutdown().await;
        result?;
        Ok(())
    }
}

impl CrabAgentHandler {
    fn session_config(&self, session_id: &str) -> SessionConfig {
        let model = ModelId::from(
            self.settings
                .model
                .as_deref()
                .unwrap_or("claude-sonnet-4-5"),
        );
        SessionConfig {
            session_id: session_id.to_string(),
            system_prompt: self.system_prompt.clone(),
            model,
            max_tokens: self.settings.max_tokens.unwrap_or(4096),
            temperature: None,
            context_window: 200_000,
            working_dir: self.working_dir.clone(),
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
            bare_mode: true,
            worktree_name: None,
            fork_session: false,
            from_pr: None,
            custom_session_id: None,
            json_schema: None,
            plugin_dirs: Vec::new(),
            disable_skills: true,
            beta_headers: Vec::new(),
            ide_connect: false,
            coordinator_mode: false,
            default_shell: "bash".into(),
        }
    }
}

/// Detach the session's construction-time event receiver, leaving a
/// closed placeholder channel in its place.
fn take_event_rx(session: &mut AgentSession) -> mpsc::Receiver<Event> {
    let (_closed_tx, closed_rx) = mpsc::channel(1);
    std::mem::replace(&mut session.event_rx, closed_rx)
}
