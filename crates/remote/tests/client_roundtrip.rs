//! End-to-end test: real `RemoteClient` talking to a real
//! `RemoteServer` over loopback. Verifies the full round-trip —
//! handshake, create, sendInput observed on the server side, cancel,
//! and events delivered to the client's broadcast channel.

use std::sync::Arc;
use std::time::Duration;

use crab_remote::protocol::{SessionAttachParams, SessionCreateParams};
use crab_remote::server::{
    InboundCmd, OutboundEvent, RemoteServer, ServerConfig, SessionError, SessionHandle,
    SessionHandler,
};
use crab_remote::{ClientConfig, ClientError, RemoteClient};
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

const SECRET: &str = "a-shared-secret-at-least-32-bytes!!";
const SESSION_ID: &str = "sess_test";

struct Spy {
    outbound_tx: mpsc::Sender<OutboundEvent>,
    inbound_rx: Mutex<mpsc::Receiver<InboundCmd>>,
}

struct SpyHandler {
    latest: Mutex<Option<Arc<Spy>>>,
    reject_attach: bool,
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
            *self.latest.lock().await = Some(Arc::new(Spy {
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
        params: SessionAttachParams,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<SessionHandle, SessionError>> + Send + '_>,
    > {
        if self.reject_attach {
            Box::pin(async move { Err(SessionError::NotFound(params.session_id)) })
        } else {
            self.create(SessionCreateParams {
                working_dir: "/tmp".into(),
                initial_prompt: None,
            })
        }
    }
}

async fn spawn_server(
    reject_attach: bool,
) -> (
    String,
    Arc<SpyHandler>,
    CancellationToken,
    tokio::task::JoinHandle<()>,
) {
    let handler = Arc::new(SpyHandler {
        latest: Mutex::new(None),
        reject_attach,
    });
    let handler_trait: Arc<dyn SessionHandler> = handler.clone();

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

    tokio::time::sleep(Duration::from_millis(150)).await;
    (format!("ws://{addr}/"), handler, cancel, join)
}

fn client_token() -> String {
    crab_remote::auth::jwt::sign(SECRET.as_bytes(), "", "client-dev", 300).unwrap()
}

#[tokio::test]
async fn handshake_create_send_event_cancel() {
    let (url, handler, cancel, join) = spawn_server(true).await;

    let client = RemoteClient::connect(ClientConfig::new(&url, client_token()))
        .await
        .unwrap();

    let created = client
        .create_session(SessionCreateParams {
            working_dir: "/tmp".into(),
            initial_prompt: None,
        })
        .await
        .unwrap();
    assert_eq!(created.session_id, SESSION_ID);

    let spy = handler.latest.lock().await.clone().expect("create ran");
    let mut events = client.subscribe_events();

    // Backend → client event must reach the broadcast receiver.
    spy.outbound_tx
        .send(OutboundEvent::Event(serde_json::json!({
            "type": "TestEvent", "tag": "hi"
        })))
        .await
        .unwrap();
    let ev = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("event timeout")
        .expect("event channel closed");
    assert_eq!(ev.session_id, SESSION_ID);
    assert_eq!(ev.event["type"], "TestEvent");

    // Client → backend input must appear on the spy's inbound_rx.
    client.send_input("hello".to_string()).await.unwrap();
    let cmd = tokio::time::timeout(Duration::from_secs(2), spy.inbound_rx.lock().await.recv())
        .await
        .expect("input timeout")
        .expect("inbound channel closed");
    match cmd {
        InboundCmd::SendInput(p) => assert_eq!(p.text, "hello"),
        InboundCmd::Cancel(_) => panic!("expected SendInput"),
    }

    // Cancel is fire-and-forget from the caller's view; the backend
    // observes it through the same channel.
    client.cancel().await.unwrap();
    let cmd = tokio::time::timeout(Duration::from_secs(2), spy.inbound_rx.lock().await.recv())
        .await
        .expect("cancel timeout")
        .expect("inbound channel closed");
    assert!(matches!(cmd, InboundCmd::Cancel(_)));

    client.close().await.unwrap();
    cancel.cancel();
    let _ = join.await;
}

#[tokio::test]
async fn attach_errors_propagate() {
    let (url, _handler, cancel, join) = spawn_server(true).await;
    let client = RemoteClient::connect(ClientConfig::new(&url, client_token()))
        .await
        .unwrap();

    let result = client
        .attach_session(SessionAttachParams {
            session_id: "nonexistent".into(),
        })
        .await;
    let err = result.unwrap_err();
    match err {
        ClientError::ServerError(e) => {
            // Server returned SessionNotFound (-32003).
            assert_eq!(e.code, -32003);
        }
        other => panic!("expected ServerError, got {other:?}"),
    }

    client.close().await.unwrap();
    cancel.cancel();
    let _ = join.await;
}

#[tokio::test]
async fn close_is_idempotent() {
    let (url, _handler, cancel, join) = spawn_server(true).await;
    let client = RemoteClient::connect(ClientConfig::new(&url, client_token()))
        .await
        .unwrap();

    client.close().await.unwrap();
    let second = client.close().await.unwrap_err();
    assert!(matches!(second, ClientError::AlreadyClosed));

    cancel.cancel();
    let _ = join.await;
}

#[tokio::test]
async fn invalid_token_rejected_at_handshake() {
    let (url, _handler, cancel, join) = spawn_server(true).await;

    // Sign with a different secret → server's verify rejects → 401.
    let wrong =
        crab_remote::auth::jwt::sign(b"a-different-secret-also-at-least-32!", "", "dev", 300)
            .unwrap();
    let err = RemoteClient::connect(ClientConfig::new(&url, wrong))
        .await
        .unwrap_err();
    assert!(matches!(err, ClientError::Handshake(_)));

    cancel.cancel();
    let _ = join.await;
}
