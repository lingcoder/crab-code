//! Agent-side entry point: [`AcpServer::serve_stdio`].
//!
//! Wraps the upstream [`agent_client_protocol::AgentSideConnection`]
//! constructor with the stdio + notification-draining wiring every ACP
//! agent needs. The composition root (cli/daemon) implements the
//! upstream [`Agent`] trait against the crab engine and hands the impl
//! here; this crate stays a thin adapter and does not embed any engine
//! logic.
//!
//! [`Agent`]: agent_client_protocol::Agent

use agent_client_protocol::{Agent, AgentSideConnection, Client as _, SessionNotification};
use tokio::sync::{mpsc, oneshot};
use tokio::task::LocalSet;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

/// Queue the agent uses to push server-side `session/update` frames
/// back to the connected editor. Each send carries a oneshot so the
/// sender knows when the frame has cleared.
pub type NotificationTx = mpsc::UnboundedSender<(SessionNotification, oneshot::Sender<()>)>;

/// Receiver half of a [`NotificationTx`] — owned by [`AcpServer`] and
/// drained into [`agent_client_protocol::Client::session_notification`].
pub type NotificationRx = mpsc::UnboundedReceiver<(SessionNotification, oneshot::Sender<()>)>;

/// Allocate a fresh notification channel for use by [`AcpServer::serve_stdio_with_notifications`].
pub fn notification_channel() -> (NotificationTx, NotificationRx) {
    mpsc::unbounded_channel()
}

/// Errors raised while serving the ACP connection.
#[derive(Debug, thiserror::Error)]
pub enum AcpServeError {
    /// The I/O loop returned an error (stream closed mid-message,
    /// malformed frame, etc).
    #[error("ACP I/O loop error: {0}")]
    Io(#[from] agent_client_protocol::Error),
}

/// Entry point for running an ACP agent over stdio.
///
/// Constructs an [`AgentSideConnection`], wraps tokio stdin/stdout in
/// `tokio_util::compat` adapters, spawns the required local-set
/// background task that pumps the I/O loop, optionally drains a
/// notification channel into `session/update` frames, and awaits
/// connection close.
pub struct AcpServer;

impl AcpServer {
    /// Run an agent with no server-initiated notifications.
    ///
    /// Use this when the agent handles prompts synchronously (single
    /// final response) and never streams intermediate updates back to
    /// the editor. Most real agents want
    /// [`Self::serve_stdio_with_notifications`] instead.
    ///
    /// # Errors
    ///
    /// Returns [`AcpServeError::Io`] if the underlying I/O loop fails.
    //
    // The returned future is intentionally !Send: the upstream SDK's
    // `spawn` parameter takes a `LocalBoxFuture<'static, ()>`, which
    // forces us into a `LocalSet` (Rc-based, single-thread). That is
    // fine for an ACP agent — it runs on the editor's main thread and
    // has no reason to move work between threads.
    #[allow(clippy::future_not_send)]
    pub async fn serve_stdio<A>(handler: A) -> Result<(), AcpServeError>
    where
        A: Agent + 'static,
    {
        let local = LocalSet::new();
        local
            .run_until(async move {
                let stdin = tokio::io::stdin().compat();
                let stdout = tokio::io::stdout().compat_write();
                let (_conn, io_future) = AgentSideConnection::new(handler, stdout, stdin, |fut| {
                    tokio::task::spawn_local(fut);
                });
                io_future.await.map_err(AcpServeError::from)
            })
            .await
    }

    /// Run an agent that streams server-initiated notifications.
    ///
    /// Build an agent that owns a [`NotificationTx`] (via
    /// [`notification_channel`]); pass the matching [`NotificationRx`]
    /// here and this function will drain it into
    /// `Client::session_notification` while the I/O loop runs.
    ///
    /// # Errors
    ///
    /// Returns [`AcpServeError::Io`] if the underlying I/O loop fails.
    #[allow(clippy::future_not_send)]
    pub async fn serve_stdio_with_notifications<A>(
        handler: A,
        mut rx: NotificationRx,
    ) -> Result<(), AcpServeError>
    where
        A: Agent + 'static,
    {
        let local = LocalSet::new();
        local
            .run_until(async move {
                let stdin = tokio::io::stdin().compat();
                let stdout = tokio::io::stdout().compat_write();
                let (conn, io_future) = AgentSideConnection::new(handler, stdout, stdin, |fut| {
                    tokio::task::spawn_local(fut);
                });

                // Drain notifications in parallel with the I/O loop.
                tokio::task::spawn_local(async move {
                    while let Some((notification, ack)) = rx.recv().await {
                        if let Err(e) = conn.session_notification(notification).await {
                            tracing::warn!(error = %e, "ACP session_notification failed");
                            break;
                        }
                        // Acks are best-effort: caller may have dropped
                        // the receiver (e.g. session cancelled).
                        let _ = ack.send(());
                    }
                });

                io_future.await.map_err(AcpServeError::from)
            })
            .await
    }
}
