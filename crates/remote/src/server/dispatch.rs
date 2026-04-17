//! Per-connection dispatch loop.
//!
//! Owns one WebSocket and (once the client attaches) one
//! [`SessionHandle`]. Drives three concurrent futures:
//!
//! 1. Read inbound WS frames → parse JSON-RPC → dispatch:
//!    - `initialize` → reply immediately
//!    - `session/create` / `session/attach` → call handler, store handle
//!    - `session/sendInput` / `session/cancel` → push into `handle.inbound_tx`
//! 2. Drain `handle.outbound_rx` → serialise as `session/event` notification → send WS frame.
//! 3. Respect the server-wide cancel token for graceful shutdown.
//!
//! Any error / close / cancel terminates all three and drops the handle,
//! which in turn signals the backend that the client left.

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::handler::{
    InboundCmd, OutboundEvent, SessionError, SessionHandle, SessionHandler, attach_result,
    create_result,
};
use crate::protocol::{
    ClientInfo, ErrorCode, InitializeParams, InitializeResult, JsonRpcError, JsonRpcNotification,
    JsonRpcRequest, JsonRpcResponse, PROTOCOL_VERSION, ServerInfo, SessionAttachParams,
    SessionCancelParams, SessionCreateParams, SessionEventParams, SessionSendInputParams, method,
};

/// Run the dispatch loop for one connection. Consumes the socket;
/// returns only when the connection ends (client close, read error,
/// or server shutdown via `cancel`).
pub async fn run_connection(
    socket: WebSocket,
    handler: Arc<dyn SessionHandler>,
    cancel: CancellationToken,
    server_name: String,
) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // Session state, populated after session/create or session/attach.
    let mut session: Option<SessionHandle> = None;
    let mut client_info: Option<ClientInfo> = None;

    loop {
        // Compute the "pending outbound event" future only if we have a session.
        let outbound_fut = async {
            if let Some(s) = session.as_mut() {
                s.outbound_rx.recv().await
            } else {
                // No session yet; park forever. futures::pending::<T>()
                // would also work — this is equivalent and needs no dep.
                std::future::pending::<Option<OutboundEvent>>().await
            }
        };

        tokio::select! {
            // Branch 1: incoming WS frame.
            msg = ws_rx.next() => {
                let Some(msg) = msg else { break };
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Err(e) = handle_text_frame(
                            &text,
                            &mut ws_tx,
                            &handler,
                            &mut session,
                            &mut client_info,
                            &server_name,
                        ).await {
                            tracing::warn!(error = %e, "dispatch error; closing connection");
                            break;
                        }
                    }
                    Ok(Message::Close(_)) => break,
                    Ok(Message::Ping(_) | Message::Pong(_) | Message::Binary(_)) => {
                        // Pings/pongs are handled by axum; binary frames ignored
                        // (crab-proto is text JSON-RPC only).
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "WebSocket read error");
                        break;
                    }
                }
            }

            // Branch 2: outbound event from the backend.
            ev = outbound_fut => {
                let Some(ev) = ev else {
                    // Session backend dropped its sender; session ended.
                    break;
                };
                if let Err(e) = forward_event(&mut ws_tx, session.as_ref().expect("ev only polled when session is Some"), ev).await {
                    tracing::warn!(error = %e, "outbound send failed; closing connection");
                    break;
                }
            }

            // Branch 3: server-wide shutdown.
            () = cancel.cancelled() => {
                let _ = ws_tx.send(Message::Close(None)).await;
                break;
            }
        }
    }

    if let Some(handle) = session {
        tracing::debug!(session = %handle.session_id, "connection closed");
    }
}

/// Type shortcut for the split WS sink/stream pair used throughout.
type WsSink = futures::stream::SplitSink<WebSocket, Message>;
type WsStream = futures::stream::SplitStream<WebSocket>;

// Bring the futures traits into scope without re-importing at every call site.
use futures::SinkExt as _;
use futures::StreamExt as _;

/// Parse one text frame as JSON-RPC and act on it.
#[allow(clippy::too_many_arguments)]
async fn handle_text_frame(
    text: &str,
    ws_tx: &mut WsSink,
    handler: &Arc<dyn SessionHandler>,
    session: &mut Option<SessionHandle>,
    client_info: &mut Option<ClientInfo>,
    server_name: &str,
) -> Result<(), DispatchError> {
    let req: JsonRpcRequest = match serde_json::from_str(text) {
        Ok(r) => r,
        Err(e) => {
            // Parse errors have no id to reply against — JSON-RPC spec
            // allows id=null for this case; our envelope narrows id to
            // u64, so we just log and continue.
            tracing::warn!(error = %e, snippet = %truncate_for_log(text), "parse error");
            return Ok(());
        }
    };

    let id = req.id;
    let method = req.method.clone();
    let params = req.params.unwrap_or(Value::Null);

    let result = dispatch(&method, params, handler, session, client_info, server_name).await;

    let response = match result {
        Ok(value) => JsonRpcResponse::ok(id, value),
        Err(err) => JsonRpcResponse::err(id, err),
    };
    send_json(ws_tx, &response).await
}

/// Returns either the typed result value or a `JsonRpcError` to relay.
async fn dispatch(
    method: &str,
    params: Value,
    handler: &Arc<dyn SessionHandler>,
    session: &mut Option<SessionHandle>,
    client_info: &mut Option<ClientInfo>,
    server_name: &str,
) -> Result<Value, JsonRpcError> {
    match method {
        method::INITIALIZE => {
            let p: InitializeParams = parse_params(params)?;
            if !version_compatible(&p.protocol_version) {
                return Err(JsonRpcError::simple(
                    ErrorCode::UnsupportedVersion.into(),
                    format!(
                        "client speaks protocol {}, server speaks {}",
                        p.protocol_version, PROTOCOL_VERSION
                    ),
                ));
            }
            *client_info = Some(p.client_info);
            Ok(serde_json::to_value(InitializeResult {
                protocol_version: PROTOCOL_VERSION.into(),
                server_info: ServerInfo {
                    name: server_name.into(),
                    version: env!("CARGO_PKG_VERSION").into(),
                },
            })
            .expect("InitializeResult serialises"))
        }

        method::INITIALIZED => {
            // Notification fired-and-forgotten by client; no reply.
            Ok(Value::Null)
        }

        method::SESSION_CREATE => {
            let p: SessionCreateParams = parse_params(params)?;
            match handler.create(p).await {
                Ok(handle) => {
                    let result = create_result(&handle);
                    *session = Some(handle);
                    Ok(serde_json::to_value(result).unwrap_or(Value::Null))
                }
                Err(e) => Err(session_error_to_rpc(&e)),
            }
        }

        method::SESSION_ATTACH => {
            let p: SessionAttachParams = parse_params(params)?;
            match handler.attach(p).await {
                Ok(handle) => {
                    let result = attach_result(&handle);
                    *session = Some(handle);
                    Ok(serde_json::to_value(result).unwrap_or(Value::Null))
                }
                Err(e) => Err(session_error_to_rpc(&e)),
            }
        }

        method::SESSION_SEND_INPUT => {
            let p: SessionSendInputParams = parse_params(params)?;
            let Some(handle) = session.as_ref() else {
                return Err(JsonRpcError::simple(
                    ErrorCode::NotAttached.into(),
                    "call session/create or session/attach first",
                ));
            };
            send_to_backend(&handle.inbound_tx, InboundCmd::SendInput(p)).await?;
            Ok(Value::Null)
        }

        method::SESSION_CANCEL => {
            let p: SessionCancelParams = parse_params(params)?;
            let Some(handle) = session.as_ref() else {
                return Err(JsonRpcError::simple(
                    ErrorCode::NotAttached.into(),
                    "call session/create or session/attach first",
                ));
            };
            send_to_backend(&handle.inbound_tx, InboundCmd::Cancel(p)).await?;
            Ok(Value::Null)
        }

        other => Err(JsonRpcError::simple(
            ErrorCode::MethodNotFound.into(),
            format!("unknown method: {other}"),
        )),
    }
}

async fn forward_event(
    ws_tx: &mut WsSink,
    handle: &SessionHandle,
    ev: OutboundEvent,
) -> Result<(), DispatchError> {
    let OutboundEvent::Event(event_json) = ev;
    let notif = JsonRpcNotification::new(
        method::SESSION_EVENT,
        Some(
            serde_json::to_value(SessionEventParams {
                session_id: handle.session_id.clone(),
                event: event_json,
            })
            .expect("SessionEventParams serialises"),
        ),
    );
    send_json(ws_tx, &notif).await
}

fn parse_params<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, JsonRpcError> {
    serde_json::from_value(value).map_err(|e| {
        JsonRpcError::simple(
            ErrorCode::InvalidParams.into(),
            format!("invalid params: {e}"),
        )
    })
}

async fn send_to_backend(
    tx: &mpsc::Sender<InboundCmd>,
    cmd: InboundCmd,
) -> Result<(), JsonRpcError> {
    tx.send(cmd).await.map_err(|_| {
        JsonRpcError::simple(
            ErrorCode::InternalError.into(),
            "session backend dropped its receiver; session is gone",
        )
    })
}

fn session_error_to_rpc(err: &SessionError) -> JsonRpcError {
    match err {
        SessionError::NotFound(id) => JsonRpcError::simple(
            ErrorCode::SessionNotFound.into(),
            format!("session not found: {id}"),
        ),
        SessionError::Backend(msg) => {
            JsonRpcError::simple(ErrorCode::InternalError.into(), msg.clone())
        }
    }
}

/// Major-version compat: client and server must agree on the leading
/// number. Additive minors / patches are forward-compatible.
fn version_compatible(client_version: &str) -> bool {
    let client_major = client_version.split('.').next().unwrap_or("0");
    let server_major = PROTOCOL_VERSION.split('.').next().unwrap_or("0");
    client_major == server_major
}

async fn send_json<T: serde::Serialize + Sync + ?Sized>(
    ws_tx: &mut WsSink,
    value: &T,
) -> Result<(), DispatchError> {
    let json = serde_json::to_string(value).map_err(DispatchError::Serialise)?;
    ws_tx
        .send(Message::Text(json.into()))
        .await
        .map_err(DispatchError::Socket)
}

fn truncate_for_log(s: &str) -> &str {
    if s.len() > 120 { &s[..120] } else { s }
}

#[derive(Debug, thiserror::Error)]
enum DispatchError {
    #[error("failed to serialise outbound message: {0}")]
    Serialise(#[source] serde_json::Error),
    #[error("failed to send WebSocket frame: {0}")]
    Socket(#[source] axum::Error),
}

// Rust-analyzer needs to see WsStream as used somewhere.
#[allow(dead_code)]
fn _assert_ws_stream_type(_s: &WsStream) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compat_allows_same_major() {
        assert!(version_compatible("0.1.0"));
        assert!(version_compatible("0.9.99"));
    }

    #[test]
    fn version_compat_rejects_different_major() {
        assert!(!version_compatible("1.0.0"));
        assert!(!version_compatible("2.5.1"));
    }

    #[test]
    fn truncate_respects_boundary() {
        let long = "a".repeat(200);
        assert_eq!(truncate_for_log(&long).len(), 120);
        assert_eq!(truncate_for_log("short").len(), 5);
    }

    #[test]
    fn session_error_maps_to_rpc_codes() {
        let not_found = session_error_to_rpc(&SessionError::NotFound("x".into()));
        assert_eq!(not_found.code, i32::from(ErrorCode::SessionNotFound));
        let backend = session_error_to_rpc(&SessionError::Backend("boom".into()));
        assert_eq!(backend.code, i32::from(ErrorCode::InternalError));
    }
}
