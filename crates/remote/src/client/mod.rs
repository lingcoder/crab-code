//! `RemoteClient` — outbound crab-proto client.
//!
//! Paired with [`super::server::RemoteServer`]; speaks the same wire and
//! follows the same lifecycle (initialize → session create/attach →
//! `send_input` / cancel → events). Target is any server that speaks
//! crab-proto — another crab instance, a user-built bot, or a
//! third-party adapter.
//!
//! Architecture: one background task owns the WebSocket. Public API
//! methods push work to it through an `mpsc::Sender<Outbox>` and await
//! the reply on a per-request oneshot. Server-initiated notifications
//! fan out over a `broadcast::Sender<SessionEventParams>` so multiple
//! consumers (TUI, logger, tests) can listen independently.

pub mod config;
pub mod error;

pub use config::ClientConfig;
pub use error::ClientError;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt as _, StreamExt as _};
use serde_json::Value;
use tokio::sync::{Mutex, broadcast, mpsc, oneshot};
use tokio::time::timeout;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

use crate::protocol::{
    InitializeParams, InitializeResult, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse,
    MessageId, PROTOCOL_VERSION, SessionAttachParams, SessionAttachResult, SessionCancelParams,
    SessionCreateParams, SessionCreateResult, SessionEventParams, SessionSendInputParams, method,
};

/// Size of the dispatcher's inbound queue. One slot per in-flight
/// request — with 32 a heavily-chatty client can pipeline well beyond
/// what any user-driven workload will produce.
const DISPATCHER_QUEUE: usize = 32;

/// Outbound client. Cheap to clone (internally an `Arc`ed dispatcher
/// handle), so a TUI and a logger subscribing to the same session
/// clone from one parent.
#[derive(Clone)]
pub struct RemoteClient {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for RemoteClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteClient").finish_non_exhaustive()
    }
}

struct Inner {
    outbox: mpsc::Sender<OutboxItem>,
    events: broadcast::Sender<SessionEventParams>,
    request_timeout: Duration,
    /// Set when `close()` has been called. Guards against use-after-close.
    closed: Mutex<bool>,
}

enum OutboxItem {
    /// A request/response round-trip. Response lands on `reply_tx`.
    Request {
        request: JsonRpcRequest,
        reply_tx: oneshot::Sender<JsonRpcResponse>,
    },
    /// Close the WebSocket and stop the dispatcher.
    Close,
}

impl RemoteClient {
    /// Connect to `config.url`, perform the `initialize` handshake, and
    /// return a ready-to-use client. The background dispatcher is
    /// already running by the time this function returns.
    ///
    /// Rejects mismatched major protocol versions.
    pub async fn connect(config: ClientConfig) -> Result<Self, ClientError> {
        let mut req = config
            .url
            .as_str()
            .into_client_request()
            .map_err(|e| ClientError::InvalidUrl(e.to_string()))?;
        let header = format!("Bearer {}", config.auth_token).parse().map_err(
            |e: axum::http::header::InvalidHeaderValue| {
                ClientError::InvalidAuthToken(e.to_string())
            },
        )?;
        req.headers_mut().insert("authorization", header);

        let (ws, _resp) = tokio_tungstenite::connect_async(req)
            .await
            .map_err(ClientError::Handshake)?;

        let (outbox_tx, outbox_rx) = mpsc::channel::<OutboxItem>(DISPATCHER_QUEUE);
        let (event_tx, _) = broadcast::channel::<SessionEventParams>(config.event_buffer);

        // Spawn the dispatcher before the handshake request so the
        // response reader is already pumping when it comes back.
        let dispatcher_event_tx = event_tx.clone();
        tokio::spawn(run_dispatcher(ws, outbox_rx, dispatcher_event_tx));

        let client = Self {
            inner: Arc::new(Inner {
                outbox: outbox_tx,
                events: event_tx,
                request_timeout: config.request_timeout,
                closed: Mutex::new(false),
            }),
        };

        // Perform the initialize round-trip. Any failure here shuts the
        // dispatcher down before handing the client to the caller.
        match client
            .request::<InitializeResult>(
                method::INITIALIZE,
                InitializeParams {
                    protocol_version: PROTOCOL_VERSION.into(),
                    client_info: config.client_info,
                },
            )
            .await
        {
            Ok(result) => {
                if major(&result.protocol_version) != major(PROTOCOL_VERSION) {
                    let _ = client.inner.outbox.send(OutboxItem::Close).await;
                    return Err(ClientError::IncompatibleProtocol {
                        server: result.protocol_version,
                        client: PROTOCOL_VERSION.into(),
                    });
                }
                Ok(client)
            }
            Err(e) => {
                let _ = client.inner.outbox.send(OutboxItem::Close).await;
                Err(e)
            }
        }
    }

    /// Create a new session on the server. Returns the server-assigned id.
    pub async fn create_session(
        &self,
        params: SessionCreateParams,
    ) -> Result<SessionCreateResult, ClientError> {
        self.request(method::SESSION_CREATE, params).await
    }

    /// Attach to an existing session by id.
    pub async fn attach_session(
        &self,
        params: SessionAttachParams,
    ) -> Result<SessionAttachResult, ClientError> {
        self.request(method::SESSION_ATTACH, params).await
    }

    /// Push user input into the currently attached session.
    pub async fn send_input(&self, text: impl Into<String>) -> Result<(), ClientError> {
        self.request::<Value>(
            method::SESSION_SEND_INPUT,
            SessionSendInputParams { text: text.into() },
        )
        .await?;
        Ok(())
    }

    /// Cancel any in-flight work on the attached session.
    pub async fn cancel(&self) -> Result<(), ClientError> {
        self.request::<Value>(method::SESSION_CANCEL, SessionCancelParams::default())
            .await?;
        Ok(())
    }

    /// Subscribe to server→client `session/event` notifications. Each
    /// call returns an independent receiver — past events are not replayed.
    pub fn subscribe_events(&self) -> broadcast::Receiver<SessionEventParams> {
        self.inner.events.subscribe()
    }

    /// Close the connection. Idempotent: further calls return
    /// [`ClientError::AlreadyClosed`].
    pub async fn close(&self) -> Result<(), ClientError> {
        {
            let mut closed = self.inner.closed.lock().await;
            if *closed {
                return Err(ClientError::AlreadyClosed);
            }
            *closed = true;
        }
        // Best-effort: if the dispatcher already exited, the send
        // fails silently, which is fine.
        let _ = self.inner.outbox.send(OutboxItem::Close).await;
        Ok(())
    }

    async fn request<R: serde::de::DeserializeOwned>(
        &self,
        method_name: &str,
        params: impl serde::Serialize,
    ) -> Result<R, ClientError> {
        let params_value = serde_json::to_value(&params).map_err(|source| ClientError::Serde {
            what: "request params",
            source,
        })?;
        let request = JsonRpcRequest::new(
            method_name,
            if params_value.is_null() {
                None
            } else {
                Some(params_value)
            },
        );
        let (reply_tx, reply_rx) = oneshot::channel();
        self.inner
            .outbox
            .send(OutboxItem::Request { request, reply_tx })
            .await
            .map_err(|_| ClientError::ConnectionClosed(0))?;

        let resp = timeout(self.inner.request_timeout, reply_rx)
            .await
            .map_err(|_| ClientError::ConnectionClosed(0))?
            .map_err(|_| ClientError::ConnectionClosed(0))?;

        if let Some(err) = resp.error {
            return Err(ClientError::ServerError(err));
        }
        let value = resp.result.unwrap_or(Value::Null);
        serde_json::from_value(value).map_err(|source| ClientError::Serde {
            what: "response body",
            source,
        })
    }
}

fn major(version: &str) -> &str {
    version.split('.').next().unwrap_or("0")
}

/// The WS-owning background task. Reads inbound frames and fans them
/// out to pending request handlers or event subscribers; serialises
/// outbound requests back onto the socket.
async fn run_dispatcher(
    ws: WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    mut outbox: mpsc::Receiver<OutboxItem>,
    events: broadcast::Sender<SessionEventParams>,
) {
    let (mut ws_tx, mut ws_rx) = ws.split();
    let mut pending: HashMap<MessageId, oneshot::Sender<JsonRpcResponse>> = HashMap::new();

    loop {
        tokio::select! {
            // 1. Local caller sent something out.
            item = outbox.recv() => {
                let Some(item) = item else { break };
                match item {
                    OutboxItem::Request { request, reply_tx } => {
                        pending.insert(request.id, reply_tx);
                        let Ok(json) = serde_json::to_string(&request) else {
                            // Drop the pending entry — the caller will time out.
                            pending.remove(&request.id);
                            continue;
                        };
                        if ws_tx.send(Message::Text(json.into())).await.is_err() {
                            break;
                        }
                    }
                    OutboxItem::Close => {
                        let _ = ws_tx.send(Message::Close(None)).await;
                        break;
                    }
                }
            }

            // 2. Server sent something in.
            msg = ws_rx.next() => {
                let Some(msg) = msg else { break };
                let Ok(msg) = msg else { break };
                match msg {
                    Message::Text(text) => {
                        if let Ok(resp) = serde_json::from_str::<JsonRpcResponse>(&text) {
                            if let Some(reply) = pending.remove(&resp.id) {
                                let _ = reply.send(resp);
                            }
                        } else if let Ok(notif) = serde_json::from_str::<JsonRpcNotification>(&text)
                            && notif.method == method::SESSION_EVENT
                            && let Some(params) = notif.params
                            && let Ok(ev) = serde_json::from_value::<SessionEventParams>(params)
                        {
                            // Err on broadcast.send means no subscribers — fine.
                            let _ = events.send(ev);
                        }
                    }
                    Message::Close(_) => break,
                    Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_) => {}
                }
            }
        }
    }

    // Wake every pending caller so their `await` unblocks with a
    // ConnectionClosed error.
    pending.clear();
}
