//! [`AgentHandler`] — the pluggable boundary between the ACP protocol
//! layer and whatever actually runs prompts.

use crab_core::event::Event;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Boxed error returned by [`AgentHandler::run_prompt`]; rendered into
/// an ACP internal-error response by the server.
pub type HandlerError = Box<dyn std::error::Error + Send + Sync>;

/// One prompt turn handed to the [`AgentHandler`].
pub struct PromptTurn {
    /// ACP session id the turn belongs to.
    pub session_id: String,
    /// Prompt text with the ACP content blocks already flattened.
    pub prompt: String,
    /// Sink for engine events; the server forwards them to the editor
    /// as ACP session notifications.
    pub events: mpsc::Sender<Event>,
    /// Fired when the client cancels the session.
    pub cancel: CancellationToken,
}

/// Runs prompts and emits events — the crate's external boundary.
///
/// `crab-acp` owns the wire protocol; the composition root (cli)
/// implements this trait with its engine glue and passes it to
/// [`AcpServer`](crate::AcpServer). Mirrors how `crab-mcp`'s server
/// takes a `ToolHandler` without embedding a tool backend.
pub trait AgentHandler: Send + Sync + 'static {
    /// Run a single prompt turn to completion, streaming events through
    /// `turn.events`. Cancellation is observed via `turn.cancel`; the
    /// server reports a cancelled turn as `StopReason::Cancelled`
    /// regardless of the returned result.
    fn run_prompt(&self, turn: PromptTurn)
    -> impl Future<Output = Result<(), HandlerError>> + Send;
}
