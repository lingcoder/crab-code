//! TUI REPL runner — wires App, [`AgentSession`], and terminal lifecycle together.

use std::io;
use std::pin::Pin;
use std::sync::Arc;

use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;

use crab_agent::SessionConfig;
use crab_api::LlmBackend;
use crab_core::event::Event;
use crab_core::message::Message;
use crab_session::Conversation;
use crab_tools::builtin::create_default_registry;
use crab_tools::executor::{PermissionHandler, ToolExecutor};

use crate::app::{App, AppAction};
use crate::event::spawn_event_loop;

/// Configuration for launching the TUI REPL.
pub struct TuiConfig {
    pub session_config: SessionConfig,
    pub backend: Arc<LlmBackend>,
}

/// Run the interactive TUI REPL. This is the main entry point for interactive mode.
///
/// Sets up the terminal, creates the agent components, and runs the render+event loop
/// until the user quits.
pub async fn run(config: TuiConfig) -> anyhow::Result<()> {
    // Build tool registry and executor
    let registry = create_default_registry();
    let tool_schemas = registry.tool_schemas();
    let mut executor = ToolExecutor::new(Arc::new(registry));

    let conversation = Conversation::new(
        config.session_config.session_id.clone(),
        config.session_config.system_prompt,
        config.session_config.context_window,
    );

    let tool_ctx = crab_core::tool::ToolContext {
        working_dir: config.session_config.working_dir,
        permission_mode: config.session_config.permission_policy.mode,
        session_id: config.session_config.session_id,
        cancellation_token: tokio_util::sync::CancellationToken::new(),
        permission_policy: config.session_config.permission_policy,
    };

    let loop_config = crab_agent::QueryLoopConfig {
        model: config.session_config.model.clone(),
        max_tokens: config.session_config.max_tokens,
        temperature: config.session_config.temperature,
        tool_schemas,
        cache_enabled: false,
    };

    let (event_tx, event_rx) = mpsc::channel::<Event>(256);

    // Permission response channel: TUI event loop → permission handler
    let (perm_resp_tx, perm_resp_rx) = mpsc::unbounded_channel::<(String, bool)>();
    executor.set_permission_handler(Arc::new(TuiPermissionHandler {
        event_tx: event_tx.clone(),
        response_rx: Arc::new(tokio::sync::Mutex::new(perm_resp_rx)),
    }));
    let executor = Arc::new(executor);

    // Bridge: bounded session events → unbounded TUI channel
    let (agent_ui_tx, agent_ui_rx) = mpsc::unbounded_channel::<Event>();
    spawn_event_forwarder(event_rx, agent_ui_tx);

    // Spawn the TUI event loop (merges crossterm + agent events + ticks)
    let tick_rate = std::time::Duration::from_millis(100);
    let mut tui_rx = spawn_event_loop(agent_ui_rx, tick_rate);

    // Set up terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let term_backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(term_backend)?;

    let model_name = loop_config.model.as_str().to_string();
    let mut app = App::new(&model_name);

    // Main render + event loop
    let result = run_loop(
        &mut terminal,
        &mut app,
        &mut tui_rx,
        conversation,
        config.backend,
        executor,
        tool_ctx,
        loop_config,
        event_tx,
        perm_resp_tx,
    )
    .await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

/// TUI-based permission handler.
///
/// When the executor encounters a tool that needs user confirmation, this handler:
/// 1. Sends a `PermissionRequest` event through the event channel to the TUI
/// 2. Waits for the TUI to send back a `PermissionResponse` via a oneshot channel
///
/// The TUI event loop listens for `AppAction::PermissionResponse` and sends
/// the response back through the event channel, which the forwarder picks up
/// and delivers to the waiting oneshot receiver.
struct TuiPermissionHandler {
    event_tx: mpsc::Sender<Event>,
    /// Receiver for permission responses from the TUI.
    /// Each request creates a fresh oneshot; we use an unbounded channel
    /// indexed by `request_id`.
    response_rx: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<(String, bool)>>>,
}

impl PermissionHandler for TuiPermissionHandler {
    fn ask_permission(
        &self,
        tool_name: &str,
        prompt: &str,
    ) -> Pin<Box<dyn std::future::Future<Output = bool> + Send + '_>> {
        let tool_name = tool_name.to_string();
        let prompt = prompt.to_string();
        let request_id = crab_common::id::new_ulid();
        let event_tx = self.event_tx.clone();
        let response_rx = self.response_rx.clone();

        Box::pin(async move {
            // Send permission request to TUI
            let _ = event_tx
                .send(Event::PermissionRequest {
                    tool_name,
                    input_summary: prompt,
                    request_id: request_id.clone(),
                })
                .await;

            // Wait for response from TUI
            let mut rx = response_rx.lock().await;
            while let Some((id, allowed)) = rx.recv().await {
                if id == request_id {
                    return allowed;
                }
            }
            false // channel closed — deny by default
        })
    }
}

/// Wrapper to shuttle conversation back from a spawned agent task.
struct AgentTaskResult {
    conversation: Conversation,
    result: crab_common::Result<()>,
}

/// The core render + event loop.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    tui_rx: &mut mpsc::UnboundedReceiver<crate::event::TuiEvent>,
    mut conversation: Conversation,
    backend: Arc<LlmBackend>,
    executor: Arc<ToolExecutor>,
    mut tool_ctx: crab_core::tool::ToolContext,
    loop_config: crab_agent::QueryLoopConfig,
    event_tx: mpsc::Sender<Event>,
    perm_resp_tx: mpsc::UnboundedSender<(String, bool)>,
) -> anyhow::Result<()> {
    // Channel to get conversation back from agent task
    let mut conv_return: Option<tokio::sync::oneshot::Receiver<AgentTaskResult>> = None;
    let mut cancel = tool_ctx.cancellation_token.clone();

    loop {
        // Render
        terminal.draw(|frame| {
            app.render(frame.area(), frame.buffer_mut());
        })?;

        // Wait for TUI event or agent task completion
        let event = tokio::select! {
            ev = tui_rx.recv() => {
                match ev {
                    Some(e) => Some(e),
                    None => break,
                }
            }
            result = async {
                match conv_return.as_mut() {
                    Some(rx) => rx.await,
                    None => std::future::pending().await,
                }
            } => {
                conv_return = None;
                match result {
                    Ok(agent_result) => {
                        conversation = agent_result.conversation;
                        if let Err(e) = agent_result.result {
                            let _ = event_tx.send(Event::Error {
                                message: e.to_string(),
                            }).await;
                        }
                    }
                    Err(_) => {
                        let _ = event_tx.send(Event::Error {
                            message: "agent task panicked".into(),
                        }).await;
                    }
                }
                continue;
            }
        };

        let Some(event) = event else { break };
        let action = app.handle_event(event);

        match action {
            AppAction::Quit => {
                cancel.cancel();
                if let Some(rx) = conv_return.take() {
                    // Wait for agent task to return conversation (for clean shutdown)
                    let _ = rx.await;
                }
                break;
            }
            AppAction::Submit(text) => {
                // Fresh cancellation token for this request
                cancel = tokio_util::sync::CancellationToken::new();
                tool_ctx.cancellation_token = cancel.clone();

                // Take conversation, push user message, spawn agent task
                conversation.push(Message::user(&text));
                let mut task_conversation = std::mem::take(&mut conversation);
                let task_backend = backend.clone();
                let task_executor = executor.clone();
                let task_ctx = tool_ctx.clone();
                let task_model = loop_config.model.clone();
                let task_max_tokens = loop_config.max_tokens;
                let task_temperature = loop_config.temperature;
                let task_schemas = loop_config.tool_schemas.clone();
                let task_cache = loop_config.cache_enabled;
                let task_event_tx = event_tx.clone();
                let task_cancel = cancel.clone();

                let (return_tx, return_rx) = tokio::sync::oneshot::channel();
                conv_return = Some(return_rx);

                tokio::spawn(async move {
                    let config = crab_agent::QueryLoopConfig {
                        model: task_model,
                        max_tokens: task_max_tokens,
                        temperature: task_temperature,
                        tool_schemas: task_schemas,
                        cache_enabled: task_cache,
                    };

                    let result = crab_agent::query_loop(
                        &mut task_conversation,
                        &task_backend,
                        &task_executor,
                        &task_ctx,
                        &config,
                        task_event_tx,
                        task_cancel,
                    )
                    .await;

                    let _ = return_tx.send(AgentTaskResult {
                        conversation: task_conversation,
                        result,
                    });
                });
            }
            AppAction::PermissionResponse {
                request_id,
                allowed,
            } => {
                // Send response to the permission handler waiting in the agent task
                let _ = perm_resp_tx.send((request_id, allowed));
            }
            AppAction::None => {}
        }
    }

    Ok(())
}

/// Spawn a task that forwards agent events from a bounded `mpsc::Receiver`
/// to the TUI's unbounded channel.
fn spawn_event_forwarder(mut rx: mpsc::Receiver<Event>, tx: mpsc::UnboundedSender<Event>) {
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if tx.send(event).is_err() {
                break;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_task_result_struct() {
        let conv = Conversation::new("test".into(), "prompt".into(), 200_000);
        let result = AgentTaskResult {
            conversation: conv,
            result: Ok(()),
        };
        assert!(result.result.is_ok());
    }

    #[test]
    fn agent_task_result_with_error() {
        let conv = Conversation::new("test".into(), "prompt".into(), 200_000);
        let result = AgentTaskResult {
            conversation: conv,
            result: Err(crab_common::Error::Other("test error".into())),
        };
        assert!(result.result.is_err());
    }
}
