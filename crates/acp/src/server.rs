//! Agent-side entry point: [`AcpServer::serve_stdio`].
//!
//! Wraps the upstream [`agent_client_protocol::AgentSideConnection`]
//! constructor with the stdio wiring that every ACP agent needs. The
//! composition root (cli/daemon) implements the upstream [`Agent`]
//! trait against the crab engine and hands the impl here; this crate
//! stays a thin adapter and does not embed any engine logic.
//!
//! [`Agent`]: agent_client_protocol::Agent

use agent_client_protocol::{Agent, AgentSideConnection};
use tokio::task::LocalSet;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

/// Errors raised while serving the ACP connection.
#[derive(Debug, thiserror::Error)]
pub enum AcpServeError {
    /// The I/O loop returned an error (stream closed mid-message,
    /// malformed frame, etc).
    #[error("ACP I/O loop error: {0}")]
    Io(#[from] agent_client_protocol::Error),
}

/// Entry point for running an ACP agent.
///
/// Creates an [`AgentSideConnection`] that reads from stdin and writes
/// to stdout, spawns the required local-set background task that pumps
/// the I/O loop, and awaits connection close.
pub struct AcpServer;

impl AcpServer {
    /// Wire an [`Agent`] impl to stdio and run the ACP message loop
    /// until the connection closes.
    ///
    /// # Errors
    ///
    /// Returns [`AcpServeError::Io`] if the underlying I/O loop fails —
    /// e.g. stdin closes, a malformed frame arrives, or the editor
    /// disconnects mid-request.
    //
    // The returned future is intentionally !Send: the upstream SDK's
    // `spawn` parameter takes a `LocalBoxFuture<'static, ()>`, which
    // forces us into a `LocalSet` (Rc-based, single-thread). This is
    // fine for an ACP agent — the child process runs on the editor's
    // main thread and has no reason to move work between threads.
    #[allow(clippy::future_not_send)]
    pub async fn serve_stdio<A>(handler: A) -> Result<(), AcpServeError>
    where
        A: Agent + 'static,
    {
        // AgentSideConnection requires a `spawn` fn that takes a
        // !Send LocalBoxFuture, so we drive the whole thing inside a
        // tokio LocalSet.
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
}
