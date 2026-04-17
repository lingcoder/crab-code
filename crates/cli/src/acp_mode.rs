//! ACP agent mode — run crab as an external agent over stdio so Zed /
//! Neovim / Helix can spawn `crab --acp` and drive it via the Agent
//! Client Protocol.
//!
//! Flow:
//!
//! ```text
//! Zed ──initialize───►  CrabAcpAgent  ──new_session──►  allocate id
//!     ──prompt──────►                ──handle_user_input──►  query_loop
//!     ◄──session/update notifications (ContentDelta → AgentMessageChunk)
//!     ◄──PromptResponse { stop_reason } when the turn completes
//! ```
//!
//! Scope of this initial cut:
//!
//! - Real `initialize` / `authenticate` / `new_session` / `cancel`.
//! - `prompt` runs the full crab query loop (tool calls execute locally)
//!   and streams `AgentMessageChunk` / `AgentThoughtChunk` frames back to
//!   the editor as the model produces text.
//! - `load_session` / `set_session_mode` / `set_session_config_option` /
//!   `ext_method` / `ext_notification` accept cleanly.
//!
//! Not yet mapped to ACP wire events: `tool_use` start / result, plans,
//! usage updates. Those extend [`event_to_update`] in a follow-up.

use std::collections::HashMap;
use std::sync::Arc;

use agent_client_protocol as acp;
use crab_acp::{NotificationTx, notification_channel};
use crab_agent::{AgentSession, SessionConfig};
use crab_api::LlmBackend;
use crab_core::event::Event;
use crab_core::model::ModelId;
use crab_core::permission::PermissionPolicy;
use crab_tools::builtin::create_default_registry;
use crab_tools::registry::ToolRegistry;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// Build the ACP agent from config and run it until stdin closes.
///
/// Minimal bootstrap: LLM backend + default tool registry + default
/// system prompt. Does not honour `--model` / `--provider` / `--resume`
/// flags yet; those can be wired in once Zed's UX around them settles.
//
// The returned future is intentionally !Send: upstream `AcpServer`
// runs on a `LocalSet` because the SDK's `spawn` takes a non-Send
// `LocalBoxFuture`. `main()` drives this on a `current_thread` runtime.
#[allow(clippy::future_not_send)]
pub async fn run() -> anyhow::Result<()> {
    let working_dir = std::env::current_dir()?;
    let settings =
        crab_config::settings::load_merged_settings_with_sources(Some(&working_dir), None)
            .unwrap_or_default();

    let backend = Arc::new(crab_api::create_backend(&settings));
    let registry = create_default_registry();

    let system_prompt = "You are crab, an AI coding assistant.".to_string();

    let (notification_tx, notification_rx) = notification_channel();

    let agent = CrabAcpAgent::new(
        notification_tx,
        backend,
        registry,
        system_prompt,
        working_dir,
        settings,
    );

    crab_acp::AcpServer::serve_stdio_with_notifications(agent, notification_rx).await?;
    Ok(())
}

/// Per-session state — just the cancel token for now. A future
/// iteration keeps the `AgentSession` itself here so multi-turn
/// `Conversation` history persists across prompts.
struct SessionState {
    cancel: CancellationToken,
}

/// The upstream [`acp::Agent`] impl that maps ACP lifecycle calls onto
/// crab sessions.
pub struct CrabAcpAgent {
    notification_tx: NotificationTx,
    backend: Arc<LlmBackend>,
    /// A prototype registry we re-build per session via
    /// [`create_default_registry`] rather than cloning, because
    /// `ToolRegistry` is not `Clone`.
    _registry_template: ToolRegistry,
    system_prompt: String,
    working_dir: std::path::PathBuf,
    settings: crab_config::Settings,
    sessions: Mutex<HashMap<String, SessionState>>,
}

impl CrabAcpAgent {
    pub fn new(
        notification_tx: NotificationTx,
        backend: Arc<LlmBackend>,
        registry: ToolRegistry,
        system_prompt: String,
        working_dir: std::path::PathBuf,
        settings: crab_config::Settings,
    ) -> Self {
        Self {
            notification_tx,
            backend,
            _registry_template: registry,
            system_prompt,
            working_dir,
            settings,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    fn new_session_id() -> String {
        crab_common::utils::id::new_ulid()
    }
}

#[async_trait::async_trait(?Send)]
impl acp::Agent for CrabAcpAgent {
    async fn initialize(
        &self,
        _arguments: acp::InitializeRequest,
    ) -> Result<acp::InitializeResponse, acp::Error> {
        Ok(acp::InitializeResponse::new(acp::ProtocolVersion::V1)
            .agent_info(acp::Implementation::new("crab", env!("CARGO_PKG_VERSION")).title("Crab")))
    }

    async fn authenticate(
        &self,
        _arguments: acp::AuthenticateRequest,
    ) -> Result<acp::AuthenticateResponse, acp::Error> {
        Ok(acp::AuthenticateResponse::default())
    }

    async fn new_session(
        &self,
        _arguments: acp::NewSessionRequest,
    ) -> Result<acp::NewSessionResponse, acp::Error> {
        let id = Self::new_session_id();
        self.sessions.lock().await.insert(
            id.clone(),
            SessionState {
                cancel: CancellationToken::new(),
            },
        );
        Ok(acp::NewSessionResponse::new(id))
    }

    async fn load_session(
        &self,
        _arguments: acp::LoadSessionRequest,
    ) -> Result<acp::LoadSessionResponse, acp::Error> {
        Ok(acp::LoadSessionResponse::new())
    }

    async fn prompt(
        &self,
        arguments: acp::PromptRequest,
    ) -> Result<acp::PromptResponse, acp::Error> {
        let session_id_str = arguments.session_id.to_string();

        // Look up (or lazily allocate) the cancel token for this session.
        let cancel = {
            let mut sessions = self.sessions.lock().await;
            sessions
                .entry(session_id_str.clone())
                .or_insert_with(|| SessionState {
                    cancel: CancellationToken::new(),
                })
                .cancel
                .clone()
        };

        let text = flatten_prompt_blocks(&arguments.prompt);
        if text.trim().is_empty() {
            return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
        }

        // Fresh AgentSession per turn. Multi-turn continuity is a follow-up.
        let model = ModelId::from(
            self.settings
                .model
                .as_deref()
                .unwrap_or("claude-sonnet-4-5"),
        );
        let session_config = SessionConfig {
            session_id: session_id_str.clone(),
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
        };

        let registry = create_default_registry();
        let mut session = AgentSession::new(session_config, Arc::clone(&self.backend), registry);

        // Swap in a fresh event_rx so we control the drain; hand ownership
        // of the existing rx to the bridge task.
        let event_rx = take_event_rx(&mut session);

        // Inject our cancel token into the session so `cancel` fires it.
        inject_cancel(&mut session, cancel.clone());

        // Bridge crab events → ACP session/update notifications.
        let bridge_id = arguments.session_id.clone();
        let notification_tx = self.notification_tx.clone();
        tokio::task::spawn_local(spawn_event_bridge(bridge_id, event_rx, notification_tx));

        // Run the turn to completion.
        let stop_reason = match session.handle_user_input(&text).await {
            Ok(()) => {
                if cancel.is_cancelled() {
                    acp::StopReason::Cancelled
                } else {
                    acp::StopReason::EndTurn
                }
            }
            Err(_) if cancel.is_cancelled() => acp::StopReason::Cancelled,
            Err(e) => {
                tracing::warn!(error = %e, "ACP prompt failed");
                return Err(acp::Error::internal_error());
            }
        };

        Ok(acp::PromptResponse::new(stop_reason))
    }

    async fn cancel(&self, args: acp::CancelNotification) -> Result<(), acp::Error> {
        let id = args.session_id.to_string();
        if let Some(state) = self.sessions.lock().await.get(&id) {
            state.cancel.cancel();
        }
        Ok(())
    }

    async fn set_session_mode(
        &self,
        _args: acp::SetSessionModeRequest,
    ) -> Result<acp::SetSessionModeResponse, acp::Error> {
        Ok(acp::SetSessionModeResponse::default())
    }

    async fn set_session_config_option(
        &self,
        _args: acp::SetSessionConfigOptionRequest,
    ) -> Result<acp::SetSessionConfigOptionResponse, acp::Error> {
        Ok(acp::SetSessionConfigOptionResponse::new(vec![]))
    }

    async fn ext_method(&self, _args: acp::ExtRequest) -> Result<acp::ExtResponse, acp::Error> {
        Err(acp::Error::method_not_found())
    }

    async fn ext_notification(&self, _args: acp::ExtNotification) -> Result<(), acp::Error> {
        Ok(())
    }
}

/// Swap the session's event receiver with a fresh one and return the
/// old receiver — same technique as interactive mode (`take_event_rx`
/// in `main.rs`).
fn take_event_rx(session: &mut AgentSession) -> mpsc::Receiver<Event> {
    // Replace the session's event_rx with a freshly-paired one so we
    // own the drain side, then return the original receiver for the
    // bridge task to consume.
    let (tx, new_rx) = mpsc::channel(256);
    let old_rx = std::mem::replace(&mut session.event_rx, new_rx);
    session.event_tx = tx;
    old_rx
}

/// Replace the session's cancel token with our external one so ACP
/// `cancel` notifications can fire it.
fn inject_cancel(session: &mut AgentSession, cancel: CancellationToken) {
    session.cancel = cancel;
}

/// Extract text from the ordered `ContentBlock`s in a prompt.
fn flatten_prompt_blocks(blocks: &[acp::ContentBlock]) -> String {
    let mut out = String::new();
    for block in blocks {
        if let acp::ContentBlock::Text(t) = block {
            out.push_str(&t.text);
            out.push('\n');
        }
    }
    out.trim_end().to_string()
}

/// Drain crab `Event`s and forward ACP-relevant ones as `session/update`
/// notifications. Returns when `event_rx` closes.
async fn spawn_event_bridge(
    session_id: acp::SessionId,
    mut event_rx: mpsc::Receiver<Event>,
    notification_tx: NotificationTx,
) {
    while let Some(event) = event_rx.recv().await {
        let Some(update) = event_to_update(&event) else {
            continue;
        };
        let (ack_tx, ack_rx) = oneshot::channel();
        let notification = acp::SessionNotification::new(session_id.clone(), update);
        if notification_tx.send((notification, ack_tx)).is_err() {
            break;
        }
        let _ = ack_rx.await;
    }
}

/// Map a crab [`Event`] onto an ACP [`acp::SessionUpdate`], returning
/// `None` for events with no ACP counterpart yet.
fn event_to_update(event: &Event) -> Option<acp::SessionUpdate> {
    match event {
        Event::ContentDelta { delta, .. } => Some(acp::SessionUpdate::AgentMessageChunk(
            acp::ContentChunk::new(acp::ContentBlock::Text(acp::TextContent::new(
                delta.clone(),
            ))),
        )),
        Event::ThinkingDelta { delta, .. } => Some(acp::SessionUpdate::AgentThoughtChunk(
            acp::ContentChunk::new(acp::ContentBlock::Text(acp::TextContent::new(
                delta.clone(),
            ))),
        )),
        _ => None,
    }
}
