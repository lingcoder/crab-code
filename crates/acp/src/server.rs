//! `AcpServer` — protocol dispatch for crab running as an ACP agent.
//!
//! Flow:
//!
//! ```text
//! Editor ──initialize───►  builder handlers  ──new_session──►  allocate id
//!        ──prompt──────►                     ──handle_prompt──►  AgentHandler::run_prompt
//!        ◄──AgentNotification::SessionNotification (ContentDelta → AgentMessageChunk)
//!        ◄──PromptResponse { stop_reason } when the turn completes
//! ```

use std::sync::Arc;

use agent_client_protocol as sdk;
use sdk::schema::v1::{
    AuthenticateRequest, AuthenticateResponse, CancelNotification, ContentBlock, Implementation,
    InitializeRequest, InitializeResponse, LoadSessionRequest, LoadSessionResponse,
    NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, SetSessionModeRequest,
    SetSessionModeResponse, StopReason,
};
use sdk::{Agent, Client, ConnectionTo, Responder};
use tokio::sync::mpsc;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::handler::{AgentHandler, HandlerError, PromptTurn};
use crate::notify;
use crate::sessions::SessionMap;

const EVENT_CHANNEL_CAPACITY: usize = 256;

/// Errors raised while serving the ACP connection.
#[derive(Debug, thiserror::Error)]
pub enum AcpServeError {
    #[error("ACP connection error: {0}")]
    Connection(#[from] sdk::Error),
}

/// ACP agent server: owns the session registry and protocol dispatch,
/// delegating prompt execution to the supplied [`AgentHandler`].
pub struct AcpServer<H: AgentHandler> {
    state: Arc<ServerState<H>>,
}

struct ServerState<H> {
    handler: H,
    sessions: SessionMap,
}

impl<H: AgentHandler> AcpServer<H> {
    pub fn new(handler: H) -> Self {
        Self {
            state: Arc::new(ServerState {
                handler,
                sessions: SessionMap::default(),
            }),
        }
    }

    /// Serve the agent over the process's stdin/stdout until the
    /// connection closes.
    pub async fn serve_stdio(self) -> Result<(), AcpServeError> {
        let transport = sdk::ByteStreams::new(
            tokio::io::stdout().compat_write(),
            tokio::io::stdin().compat(),
        );
        build_agent(self.state)
            .connect_to(transport)
            .await
            .map_err(Into::into)
    }
}

/// Construct a fully-configured ACP agent builder over the shared state.
fn build_agent<H: AgentHandler>(
    state: Arc<ServerState<H>>,
) -> sdk::Builder<Agent, impl sdk::HandleDispatchFrom<Client> + 'static> {
    Agent
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
            sdk::on_receive_request!(),
        )
        // ── authenticate ────────────────────────────────────────────
        .on_receive_request(
            async move |_req: AuthenticateRequest,
                        responder: Responder<AuthenticateResponse>,
                        _cx: ConnectionTo<Client>| {
                responder.respond(AuthenticateResponse::default())
            },
            sdk::on_receive_request!(),
        )
        // ── new_session ─────────────────────────────────────────────
        .on_receive_request(
            {
                let state = state.clone();
                async move |_req: NewSessionRequest,
                            responder: Responder<NewSessionResponse>,
                            _cx: ConnectionTo<Client>| {
                    let id = state.sessions.create().await;
                    responder.respond(NewSessionResponse::new(id))
                }
            },
            sdk::on_receive_request!(),
        )
        // ── load_session ────────────────────────────────────────────
        .on_receive_request(
            async move |_req: LoadSessionRequest,
                        responder: Responder<LoadSessionResponse>,
                        _cx: ConnectionTo<Client>| {
                responder.respond(LoadSessionResponse::new())
            },
            sdk::on_receive_request!(),
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
            sdk::on_receive_request!(),
        )
        // ── set_session_mode ────────────────────────────────────────
        .on_receive_request(
            async move |_req: SetSessionModeRequest,
                        responder: Responder<SetSessionModeResponse>,
                        _cx: ConnectionTo<Client>| {
                responder.respond(SetSessionModeResponse::default())
            },
            sdk::on_receive_request!(),
        )
        // ── set_session_config_option ───────────────────────────────
        .on_receive_request(
            async move |_req: SetSessionConfigOptionRequest,
                        responder: Responder<SetSessionConfigOptionResponse>,
                        _cx: ConnectionTo<Client>| {
                responder.respond(SetSessionConfigOptionResponse::new(vec![]))
            },
            sdk::on_receive_request!(),
        )
        // ── cancel (notification) ───────────────────────────────────
        .on_receive_notification(
            {
                async move |notif: CancelNotification, _cx: ConnectionTo<Client>| {
                    state.sessions.cancel(&notif.session_id.to_string()).await;
                    Ok(())
                }
            },
            sdk::on_receive_notification!(),
        )
        // ── ext_notification (via ClientNotification enum) ──────────
        .on_receive_notification(
            async move |_notif: sdk::schema::v1::ClientNotification, _cx: ConnectionTo<Client>| {
                Ok(())
            },
            sdk::on_receive_notification!(),
        )
}

/// Handle a prompt request — runs in a spawned task so it does not
/// block the event loop.
async fn handle_prompt<H: AgentHandler>(
    state: Arc<ServerState<H>>,
    req: PromptRequest,
    responder: Responder<PromptResponse>,
    cx: ConnectionTo<Client>,
) -> Result<(), sdk::Error> {
    let session_id = req.session_id.to_string();
    let cancel = state.sessions.begin_turn(&session_id).await;

    let prompt = flatten_prompt_blocks(&req.prompt);
    if prompt.trim().is_empty() {
        return responder.respond(PromptResponse::new(StopReason::EndTurn));
    }

    let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_CAPACITY);
    tokio::spawn(notify::run_event_bridge(
        req.session_id.clone(),
        event_rx,
        cx,
    ));

    let turn = PromptTurn {
        session_id,
        prompt,
        events: event_tx,
        cancel: cancel.clone(),
    };
    let result = state.handler.run_prompt(turn).await;

    match turn_stop_reason(cancel.is_cancelled(), result) {
        Ok(stop_reason) => responder.respond(PromptResponse::new(stop_reason)),
        Err(e) => {
            tracing::warn!(error = %e, "ACP prompt failed");
            responder.respond_with_internal_error(format!("prompt failed: {e}"))
        }
    }
}

/// Map the handler outcome onto an ACP stop reason. A cancelled turn
/// reports `Cancelled` even when the handler returned an error, since
/// cancellation typically surfaces as an aborted query.
fn turn_stop_reason(
    cancelled: bool,
    result: Result<(), HandlerError>,
) -> Result<StopReason, HandlerError> {
    if cancelled {
        return Ok(StopReason::Cancelled);
    }
    result.map(|()| StopReason::EndTurn)
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

#[cfg(test)]
mod tests {
    use super::*;
    use sdk::schema::v1::TextContent;

    #[test]
    fn flatten_joins_text_blocks_with_newlines() {
        let blocks = vec![
            ContentBlock::Text(TextContent::new("first")),
            ContentBlock::Text(TextContent::new("second")),
        ];
        assert_eq!(flatten_prompt_blocks(&blocks), "first\nsecond");
    }

    #[test]
    fn flatten_of_empty_prompt_is_empty() {
        assert_eq!(flatten_prompt_blocks(&[]), "");
    }

    #[test]
    fn completed_turn_ends_the_turn() {
        let stop = turn_stop_reason(false, Ok(())).expect("ok maps to a stop reason");
        assert_eq!(stop, StopReason::EndTurn);
    }

    #[test]
    fn cancelled_turn_wins_over_success_and_error() {
        let stop = turn_stop_reason(true, Ok(())).expect("cancelled ok maps to a stop reason");
        assert_eq!(stop, StopReason::Cancelled);
        let stop = turn_stop_reason(true, Err("query aborted".into()))
            .expect("cancelled error still maps to a stop reason");
        assert_eq!(stop, StopReason::Cancelled);
    }

    #[test]
    fn failed_turn_propagates_the_error() {
        let err = turn_stop_reason(false, Err("backend down".into()))
            .expect_err("error propagates when not cancelled");
        assert_eq!(err.to_string(), "backend down");
    }
}
