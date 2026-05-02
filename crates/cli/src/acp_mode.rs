//! ACP agent mode — run crab as an external agent over stdio so Zed /
//! Neovim / Helix can spawn `crab --acp` and drive it via the Agent
//! Client Protocol.
//!
//! Flow:
//!
//! ```text
//! Zed ──initialize───►  builder handlers  ──new_session──►  allocate id
//!     ──prompt──────►                     ──handle_prompt──►  query_loop
//!     ◄──AgentNotification::SessionNotification (ContentDelta → AgentMessageChunk)
//!     ◄──PromptResponse { stop_reason } when the turn completes
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use acp::schema::{
    AgentNotification, AuthenticateRequest, AuthenticateResponse, CancelNotification, ContentBlock,
    Implementation, InitializeRequest, InitializeResponse, LoadSessionRequest, LoadSessionResponse,
    NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, SessionNotification,
    SessionUpdate, SetSessionConfigOptionRequest, SetSessionConfigOptionResponse,
    SetSessionModeRequest, SetSessionModeResponse, StopReason, TextContent,
};
use acp::{Client, ConnectionTo, Responder};
use crab_acp::sdk as acp;
use crab_agents::{AgentSession, SessionConfig};
use crab_api::LlmBackend;
use crab_core::event::Event;
use crab_core::model::ModelId;
use crab_core::permission::PermissionPolicy;
use crab_tools::builtin::create_default_registry;
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

/// Build the ACP agent from config and run it until stdin closes.
pub async fn run() -> anyhow::Result<()> {
    let working_dir = std::env::current_dir()?;
    let ctx = crab_config::ResolveContext::new()
        .with_project_dir(Some(working_dir.clone()))
        .with_process_env();
    let settings = crab_config::resolve(&ctx).unwrap_or_default();

    let backend = Arc::new(crab_api::create_backend(&settings));
    let system_prompt = "You are crab, an AI coding assistant.".to_string();

    let state = Arc::new(AgentState {
        backend,
        system_prompt,
        working_dir,
        settings,
        sessions: Mutex::new(HashMap::new()),
    });

    let builder = build_agent(state);
    crab_acp::serve_stdio(builder).await?;
    Ok(())
}

/// Per-session state — just the cancel token for now.
struct SessionState {
    cancel: CancellationToken,
}

/// Shared state across all handler closures.
struct AgentState {
    backend: Arc<LlmBackend>,
    system_prompt: String,
    working_dir: std::path::PathBuf,
    settings: crab_config::Config,
    sessions: Mutex<HashMap<String, SessionState>>,
}

fn new_session_id() -> String {
    crab_utils::id::new_ulid()
}

/// Construct a fully-configured ACP agent builder.
#[allow(clippy::needless_pass_by_value)]
fn build_agent(
    state: Arc<AgentState>,
) -> acp::Builder<acp::Agent, impl acp::HandleDispatchFrom<Client> + 'static> {
    acp::Agent
        .builder()
        .name("crab")
        // ── initialize ──────────────────────────────────────────────
        .on_receive_request(
            async move |req: InitializeRequest,
                        responder: Responder<InitializeResponse>,
                        _cx: ConnectionTo<Client>| {
                responder.respond(InitializeResponse::new(req.protocol_version).agent_info(
                    Implementation::new("crab", env!("CARGO_PKG_VERSION")).title("Crab"),
                ))
            },
            acp::on_receive_request!(),
        )
        // ── authenticate ────────────────────────────────────────────
        .on_receive_request(
            async move |_req: AuthenticateRequest,
                        responder: Responder<AuthenticateResponse>,
                        _cx: ConnectionTo<Client>| {
                responder.respond(AuthenticateResponse::default())
            },
            acp::on_receive_request!(),
        )
        // ── new_session ─────────────────────────────────────────────
        .on_receive_request(
            {
                let state = state.clone();
                async move |_req: NewSessionRequest,
                            responder: Responder<NewSessionResponse>,
                            _cx: ConnectionTo<Client>| {
                    let id = new_session_id();
                    state.sessions.lock().await.insert(
                        id.clone(),
                        SessionState {
                            cancel: CancellationToken::new(),
                        },
                    );
                    responder.respond(NewSessionResponse::new(id))
                }
            },
            acp::on_receive_request!(),
        )
        // ── load_session ────────────────────────────────────────────
        .on_receive_request(
            async move |_req: LoadSessionRequest,
                        responder: Responder<LoadSessionResponse>,
                        _cx: ConnectionTo<Client>| {
                responder.respond(LoadSessionResponse::new())
            },
            acp::on_receive_request!(),
        )
        // ── prompt ──────────────────────────────────────────────────
        .on_receive_request(
            {
                let state = state.clone();
                async move |req: PromptRequest,
                            responder: Responder<PromptResponse>,
                            cx: ConnectionTo<Client>| {
                    cx.spawn(handle_prompt(state.clone(), req, responder, cx.clone()))
                }
            },
            acp::on_receive_request!(),
        )
        // ── set_session_mode ────────────────────────────────────────
        .on_receive_request(
            async move |_req: SetSessionModeRequest,
                        responder: Responder<SetSessionModeResponse>,
                        _cx: ConnectionTo<Client>| {
                responder.respond(SetSessionModeResponse::default())
            },
            acp::on_receive_request!(),
        )
        // ── set_session_config_option ───────────────────────────────
        .on_receive_request(
            async move |_req: SetSessionConfigOptionRequest,
                        responder: Responder<SetSessionConfigOptionResponse>,
                        _cx: ConnectionTo<Client>| {
                responder.respond(SetSessionConfigOptionResponse::new(vec![]))
            },
            acp::on_receive_request!(),
        )
        // ── cancel (notification) ───────────────────────────────────
        .on_receive_notification(
            {
                async move |notif: CancelNotification, _cx: ConnectionTo<Client>| {
                    let id = notif.session_id.to_string();
                    if let Some(s) = state.sessions.lock().await.get(&id) {
                        s.cancel.cancel();
                    }
                    Ok(())
                }
            },
            acp::on_receive_notification!(),
        )
        // ── ext_notification (via ClientNotification enum) ──────────
        .on_receive_notification(
            async move |_notif: acp::ClientNotification, _cx: ConnectionTo<Client>| Ok(()),
            acp::on_receive_notification!(),
        )
}

/// Handle a prompt request — runs in a spawned task so it does not
/// block the event loop.
async fn handle_prompt(
    state: Arc<AgentState>,
    req: PromptRequest,
    responder: Responder<PromptResponse>,
    cx: ConnectionTo<Client>,
) -> Result<(), acp::Error> {
    let session_id_str = req.session_id.to_string();

    let cancel = {
        let mut sessions = state.sessions.lock().await;
        sessions
            .entry(session_id_str.clone())
            .or_insert_with(|| SessionState {
                cancel: CancellationToken::new(),
            })
            .cancel
            .clone()
    };

    let text = flatten_prompt_blocks(&req.prompt);
    if text.trim().is_empty() {
        return responder.respond(PromptResponse::new(StopReason::EndTurn));
    }

    let model = ModelId::from(
        state
            .settings
            .model
            .as_deref()
            .unwrap_or("claude-sonnet-4-5"),
    );
    let session_config = SessionConfig {
        session_id: session_id_str.clone(),
        system_prompt: state.system_prompt.clone(),
        model,
        max_tokens: state.settings.max_tokens.unwrap_or(4096),
        temperature: None,
        context_window: 200_000,
        working_dir: state.working_dir.clone(),
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
    };

    let registry = create_default_registry();
    let mut session = AgentSession::new(session_config, Arc::clone(&state.backend), registry);

    let event_rx = take_event_rx(&mut session);
    inject_cancel(&mut session, cancel.clone());

    let bridge_session_id = req.session_id.clone();
    tokio::spawn(run_event_bridge(bridge_session_id, event_rx, cx));

    let stop_reason = match session.handle_user_input(&text).await {
        Ok(()) | Err(_) if cancel.is_cancelled() => StopReason::Cancelled,
        Ok(()) => StopReason::EndTurn,
        Err(e) => {
            tracing::warn!(error = %e, "ACP prompt failed");
            return responder.respond_with_internal_error(format!("prompt failed: {e}"));
        }
    };

    responder.respond(PromptResponse::new(stop_reason))
}

/// Swap the session's event receiver with a fresh one and return the
/// old receiver.
fn take_event_rx(session: &mut AgentSession) -> mpsc::Receiver<Event> {
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
fn flatten_prompt_blocks(blocks: &[ContentBlock]) -> String {
    let mut out = String::new();
    for block in blocks {
        if let ContentBlock::Text(t) = block {
            out.push_str(&t.text);
            out.push('\n');
        }
    }
    out.trim_end().to_string()
}

/// Drain crab `Event`s and forward ACP-relevant ones as
/// `AgentNotification::SessionNotification` frames.
async fn run_event_bridge(
    session_id: acp::schema::SessionId,
    mut event_rx: mpsc::Receiver<Event>,
    cx: ConnectionTo<Client>,
) {
    while let Some(event) = event_rx.recv().await {
        if let Some(update) = event_to_update(&event) {
            let notif = SessionNotification::new(session_id.clone(), update);
            if cx
                .send_notification(AgentNotification::SessionNotification(notif))
                .is_err()
            {
                break;
            }
        }
    }
}

/// Map a crab [`Event`] onto an ACP [`SessionUpdate`], returning
/// `None` for events with no ACP counterpart yet.
fn event_to_update(event: &Event) -> Option<SessionUpdate> {
    match event {
        Event::ContentDelta { delta, .. } => Some(SessionUpdate::AgentMessageChunk(
            acp::schema::ContentChunk::new(ContentBlock::Text(TextContent::new(delta.clone()))),
        )),
        Event::ThinkingDelta { delta, .. } => Some(SessionUpdate::AgentThoughtChunk(
            acp::schema::ContentChunk::new(ContentBlock::Text(TextContent::new(delta.clone()))),
        )),
        _ => None,
    }
}
