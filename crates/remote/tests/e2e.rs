//! End-to-end integration tests for the crab-proto server.
//!
//! Spins up a real [`RemoteServer`] on a loopback port, drives it
//! through a spy [`SessionHandler`], and verifies the full wire flow
//! from `initialize` through `session/create` / `sendInput` / `cancel`
//! plus server→client `session/event` notifications.

use std::sync::Arc;
use std::time::Duration;

use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

use crab_remote::protocol::{
    ClientInfo, ErrorCode, InitializeParams, InitializeResult, JsonRpcNotification, JsonRpcRequest,
    JsonRpcResponse, PROTOCOL_VERSION, SessionCreateParams, SessionCreateResult,
    SessionEventParams, SessionSendInputParams, method,
};
use crab_remote::server::{
    InboundCmd, OutboundEvent, RemoteServer, ServerConfig, SessionError, SessionHandle,
    SessionHandler,
};
use futures::{SinkExt as _, StreamExt as _};
use tokio::sync::{Mutex, mpsc};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_util::sync::CancellationToken;

const SECRET: &str = "a-shared-secret-at-least-32-bytes!!";
const SESSION_ID: &str = "sess_test";

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Spy channels a test uses to drive a running session from the
/// handler side: push outbound events and peek at inbound commands.
struct Spy {
    outbound_tx: mpsc::Sender<OutboundEvent>,
    inbound_rx: Mutex<mpsc::Receiver<InboundCmd>>,
}

/// Handler that answers `create` by packaging up a spy + the matching
/// `SessionHandle`; rejects every `attach` with `NotFound`.
struct SpyHandler {
    /// The most recent spy from a `create()` call lands here.
    latest_spy: Mutex<Option<Arc<Spy>>>,
}

impl SessionHandler for SpyHandler {
    fn create(
        &self,
        _params: SessionCreateParams,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<SessionHandle, SessionError>> + Send + '_>,
    > {
        Box::pin(async move {
            let (inbound_tx, inbound_rx) = mpsc::channel(8);
            let (outbound_tx, outbound_rx) = mpsc::channel(8);
            *self.latest_spy.lock().await = Some(Arc::new(Spy {
                outbound_tx,
                inbound_rx: Mutex::new(inbound_rx),
            }));
            Ok(SessionHandle {
                session_id: SESSION_ID.into(),
                inbound_tx,
                outbound_rx,
                busy: false,
            })
        })
    }

    fn attach(
        &self,
        params: crab_remote::protocol::SessionAttachParams,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<SessionHandle, SessionError>> + Send + '_>,
    > {
        Box::pin(async move { Err(SessionError::NotFound(params.session_id)) })
    }
}

async fn setup_server() -> (
    String,
    Arc<SpyHandler>,
    CancellationToken,
    tokio::task::JoinHandle<()>,
) {
    let handler = Arc::new(SpyHandler {
        latest_spy: Mutex::new(None),
    });
    let handler_trait: Arc<dyn SessionHandler> = handler.clone();

    // Resolve a free port by binding, noting the addr, and releasing.
    let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = probe.local_addr().unwrap();
    drop(probe);

    let mut config = ServerConfig::new(SECRET);
    config.bind = addr;

    let cancel = CancellationToken::new();
    let server = RemoteServer::new(config, handler_trait, "crab-test");
    let cancel_clone = cancel.clone();
    let join = tokio::spawn(async move {
        let _ = server.serve(cancel_clone).await;
    });

    // Give the listener a moment to rebind.
    tokio::time::sleep(Duration::from_millis(150)).await;

    (format!("ws://{addr}/"), handler, cancel, join)
}

fn bearer_token() -> String {
    crab_remote::auth::jwt::sign(SECRET.as_bytes(), "", "test-device", 300).unwrap()
}

async fn connect(url: &str) -> Ws {
    let mut req = url.into_client_request().unwrap();
    req.headers_mut().insert(
        "authorization",
        format!("Bearer {}", bearer_token()).parse().unwrap(),
    );
    let (ws, _resp) = tokio_tungstenite::connect_async(req).await.unwrap();
    ws
}

async fn send<T: serde::Serialize + Sync + ?Sized>(ws: &mut Ws, value: &T) {
    let json = serde_json::to_string(value).unwrap();
    ws.send(WsMessage::Text(json.into())).await.unwrap();
}

async fn recv_response(ws: &mut Ws) -> JsonRpcResponse {
    let msg = ws.next().await.expect("ws closed").unwrap();
    serde_json::from_str(&msg.into_text().unwrap()).unwrap()
}

async fn recv_notification(ws: &mut Ws) -> JsonRpcNotification {
    let msg = ws.next().await.expect("ws closed").unwrap();
    serde_json::from_str(&msg.into_text().unwrap()).unwrap()
}

fn init_request() -> JsonRpcRequest {
    JsonRpcRequest::new(
        method::INITIALIZE,
        Some(
            serde_json::to_value(InitializeParams {
                protocol_version: PROTOCOL_VERSION.into(),
                client_info: ClientInfo {
                    name: "test".into(),
                    version: "0.0.1".into(),
                },
            })
            .unwrap(),
        ),
    )
}

#[tokio::test]
async fn initialize_handshake_succeeds() {
    let (url, _handler, cancel, join) = setup_server().await;
    let mut ws = connect(&url).await;

    send(&mut ws, &init_request()).await;
    let resp = recv_response(&mut ws).await;
    assert!(!resp.is_error(), "response was error: {resp:?}");
    let result: InitializeResult = serde_json::from_value(resp.result.unwrap()).unwrap();
    assert_eq!(result.protocol_version, PROTOCOL_VERSION);
    assert_eq!(result.server_info.name, "crab-test");

    cancel.cancel();
    let _ = join.await;
}

#[tokio::test]
async fn create_session_delivers_event_and_receives_input() {
    let (url, handler, cancel, join) = setup_server().await;
    let mut ws = connect(&url).await;

    send(&mut ws, &init_request()).await;
    let _ = recv_response(&mut ws).await;

    send(
        &mut ws,
        &JsonRpcRequest::new(
            method::SESSION_CREATE,
            Some(
                serde_json::to_value(SessionCreateParams {
                    working_dir: "/tmp".into(),
                    initial_prompt: None,
                })
                .unwrap(),
            ),
        ),
    )
    .await;
    let resp = recv_response(&mut ws).await;
    assert!(!resp.is_error(), "create failed: {resp:?}");
    let result: SessionCreateResult = serde_json::from_value(resp.result.unwrap()).unwrap();
    assert_eq!(result.session_id, SESSION_ID);

    let spy = handler.latest_spy.lock().await.clone().expect("create ran");

    // Backend pushes event → client must observe it as a session/event notification.
    spy.outbound_tx
        .send(OutboundEvent::Event(
            serde_json::json!({ "type": "TestEvent", "tag": "hello" }),
        ))
        .await
        .unwrap();
    let notif = recv_notification(&mut ws).await;
    assert_eq!(notif.method, method::SESSION_EVENT);
    let params: SessionEventParams = serde_json::from_value(notif.params.unwrap()).unwrap();
    assert_eq!(params.session_id, SESSION_ID);
    assert_eq!(params.event["type"], "TestEvent");

    // Client sends input → backend must observe it via inbound_rx.
    send(
        &mut ws,
        &JsonRpcRequest::new(
            method::SESSION_SEND_INPUT,
            Some(
                serde_json::to_value(SessionSendInputParams {
                    text: "hi there".into(),
                })
                .unwrap(),
            ),
        ),
    )
    .await;
    let resp = recv_response(&mut ws).await;
    assert!(!resp.is_error(), "sendInput failed: {resp:?}");
    let received = tokio::time::timeout(Duration::from_secs(2), spy.inbound_rx.lock().await.recv())
        .await
        .expect("timeout waiting for input")
        .expect("channel closed");
    match received {
        InboundCmd::SendInput(p) => assert_eq!(p.text, "hi there"),
        InboundCmd::Cancel(_) => panic!("expected SendInput, got Cancel"),
    }

    cancel.cancel();
    let _ = join.await;
}

#[tokio::test]
async fn send_input_before_attach_errors() {
    let (url, _handler, cancel, join) = setup_server().await;
    let mut ws = connect(&url).await;

    send(&mut ws, &init_request()).await;
    let _ = recv_response(&mut ws).await;

    send(
        &mut ws,
        &JsonRpcRequest::new(
            method::SESSION_SEND_INPUT,
            Some(
                serde_json::to_value(SessionSendInputParams {
                    text: "hello".into(),
                })
                .unwrap(),
            ),
        ),
    )
    .await;
    let resp = recv_response(&mut ws).await;
    assert!(resp.is_error());
    let err = resp.error.unwrap();
    assert_eq!(err.code, i32::from(ErrorCode::NotAttached));

    cancel.cancel();
    let _ = join.await;
}

#[tokio::test]
async fn unauthorized_connection_rejected() {
    let (url, _handler, cancel, join) = setup_server().await;

    // Connect without an Authorization header — the upgrade must 401.
    let req = url.as_str().into_client_request().unwrap();
    let result = tokio_tungstenite::connect_async(req).await;
    assert!(
        result.is_err(),
        "unauthenticated connect should fail at handshake"
    );

    cancel.cancel();
    let _ = join.await;
}
