//! TUI REPL runner — wires App, [`AgentSession`], and terminal lifecycle together.
//!
//! Features:
//! - Full agent query loop with tool execution
//! - Permission dialog integration via `PermissionDialog` component
//! - Tool execution progress (spinner) and result display in content area
//! - Session persistence (auto-save on exit, `--resume` support)
//! - Skill `/command` input detection and resolution via `SkillRegistry`

use std::io;

use crate::components::autocomplete::CommandInfo;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
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
use crab_session::{Conversation, SessionHistory};
use crab_skill::SkillRegistry;
use crab_tools::builtin::create_default_registry;
use crab_tools::executor::{PermissionHandler, ToolExecutor};

use crate::app::{App, AppAction};
use crate::app_event::AppEvent;
use crate::event::spawn_event_loop;
use crate::event_broker::EventBroker;
use crate::frame_requester::FrameRequester;

/// Configuration for launching the TUI REPL.
pub struct TuiConfig {
    pub session_config: SessionConfig,
    pub backend: Arc<LlmBackend>,
    /// Skill directories to scan for `/command` support.
    pub skill_dirs: Vec<PathBuf>,
}

/// Run the interactive TUI REPL. This is the main entry point for interactive mode.
///
/// Sets up the terminal, creates the agent components, and runs the render+event loop
/// until the user quits. On exit, auto-saves the session to disk.
#[allow(clippy::too_many_lines)]
pub async fn run(config: TuiConfig) -> anyhow::Result<()> {
    // Build tool registry and executor
    let registry = Arc::new(create_default_registry());
    let tool_schemas = registry.tool_schemas();
    let registry_for_app = Arc::clone(&registry);
    let mut executor = ToolExecutor::new(registry);

    let session_id = config.session_config.session_id.clone();

    // Load memories and build conversation with system prompt
    let memory_store = config
        .session_config
        .memory_dir
        .as_ref()
        .map(|d| crab_session::MemoryStore::new(d.clone()));
    let session_history = config
        .session_config
        .sessions_dir
        .as_ref()
        .map(|d| SessionHistory::new(d.clone()));

    let mut system_prompt = config.session_config.system_prompt.clone();

    // Inject memories into system prompt
    if let Some(ref store) = memory_store
        && let Ok(memories) = store.load_all()
        && !memories.is_empty()
    {
        system_prompt.push_str("\n\n# Loaded Memories\n\n");
        for mem in &memories {
            use std::fmt::Write as _;
            let _ = writeln!(system_prompt, "## {} (type: {})", mem.name, mem.memory_type);
            if !mem.description.is_empty() {
                let _ = writeln!(system_prompt, "> {}", mem.description);
                system_prompt.push('\n');
            }
            let _ = writeln!(system_prompt, "{}", mem.body);
            system_prompt.push('\n');
        }
    }

    let mut conversation = Conversation::new(
        session_id.clone(),
        system_prompt,
        config.session_config.context_window,
    );

    // Resume from previous session if requested
    if let Some(ref resume_id) = config.session_config.resume_session_id
        && let Some(ref history) = session_history
        && let Ok(Some(messages)) = history.load(resume_id)
    {
        for msg in messages {
            conversation.push(msg);
        }
    }

    let tool_ctx = crab_core::tool::ToolContext {
        working_dir: config.session_config.working_dir,
        permission_mode: config.session_config.permission_policy.mode,
        session_id: session_id.clone(),
        cancellation_token: tokio_util::sync::CancellationToken::new(),
        permission_policy: config.session_config.permission_policy,
        ext: crab_core::tool::ToolContextExt::default(),
    };

    let loop_config = crab_agent::QueryLoopConfig {
        model: config.session_config.model.clone(),
        max_tokens: config.session_config.max_tokens,
        temperature: config.session_config.temperature,
        tool_schemas,
        cache_enabled: false,
        _token_budget: None,
        budget_tokens: None,
        retry_policy: None,
        hook_executor: None,
        session_id: Some(config.session_config.session_id.clone()),
        effort: None,
        fallback_model: config
            .session_config
            .fallback_model
            .map(crab_core::model::ModelId::from),
    };

    let (event_tx, event_rx) = mpsc::channel::<Event>(256);

    // Permission response channel: TUI event loop → permission handler
    let (perm_resp_tx, perm_resp_rx) = mpsc::unbounded_channel::<(String, bool)>();
    executor.set_permission_handler(Arc::new(TuiPermissionHandler {
        event_tx: event_tx.clone(),
        response_rx: Arc::new(tokio::sync::Mutex::new(perm_resp_rx)),
    }));
    let executor = Arc::new(executor);

    // Discover skills for /command support
    let skill_registry = SkillRegistry::discover(&config.skill_dirs).unwrap_or_default();

    // Bridge: bounded session events → unbounded TUI channel
    let (agent_ui_tx, agent_ui_rx) = mpsc::unbounded_channel::<Event>();
    spawn_event_forwarder(event_rx, agent_ui_tx);

    // EventBroker controls whether crossterm events flow into the TUI loop.
    // We pause it during the external editor (Ctrl+G) so $EDITOR can own the
    // terminal, then resume.
    let event_broker = Arc::new(EventBroker::new());

    // FrameRequester lets background tasks request a redraw without waiting
    // for the next tick.
    let frame_requester = FrameRequester::default();

    // Spawn the TUI event loop (merges crossterm + agent events + ticks)
    let tick_rate = std::time::Duration::from_millis(100);
    let mut tui_rx = spawn_event_loop(agent_ui_rx, tick_rate, Arc::clone(&event_broker));

    // Set up terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    let term_backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(term_backend)?;

    let model_name = loop_config.model.as_str().to_string();
    let mut app = App::new(&model_name);
    app.tool_registry = Some(registry_for_app);
    if let Ok(cwd) = std::env::current_dir() {
        app.set_working_dir(cwd.display().to_string());
        app.set_completion_cwd(cwd);
    }

    // Register built-in slash commands for Tab completion
    app.set_slash_commands(vec![
        CommandInfo {
            name: "/help".into(),
            description: "Show available commands".into(),
        },
        CommandInfo {
            name: "/clear".into(),
            description: "Clear conversation history".into(),
        },
        CommandInfo {
            name: "/compact".into(),
            description: "Compact conversation (free context)".into(),
        },
        CommandInfo {
            name: "/exit".into(),
            description: "Exit crab-code".into(),
        },
        CommandInfo {
            name: "/model".into(),
            description: "Show or switch the current model".into(),
        },
        CommandInfo {
            name: "/cost".into(),
            description: "Show token usage and cost".into(),
        },
        CommandInfo {
            name: "/status".into(),
            description: "Show session status".into(),
        },
        CommandInfo {
            name: "/memory".into(),
            description: "Show or manage memory files".into(),
        },
        CommandInfo {
            name: "/config".into(),
            description: "Open settings configuration".into(),
        },
        CommandInfo {
            name: "/permissions".into(),
            description: "Show current permission mode".into(),
        },
        CommandInfo {
            name: "/resume".into(),
            description: "Resume a previous session".into(),
        },
        CommandInfo {
            name: "/diff".into(),
            description: "Show recent file changes".into(),
        },
        CommandInfo {
            name: "/review".into(),
            description: "Review recent code changes".into(),
        },
        CommandInfo {
            name: "/commit".into(),
            description: "Create a git commit".into(),
        },
        CommandInfo {
            name: "/plan".into(),
            description: "Enter plan mode".into(),
        },
        CommandInfo {
            name: "/fast".into(),
            description: "Toggle fast mode".into(),
        },
        CommandInfo {
            name: "/thinking".into(),
            description: "Toggle extended thinking".into(),
        },
        CommandInfo {
            name: "/effort".into(),
            description: "Set effort level".into(),
        },
    ]);

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
        &skill_registry,
        session_history.as_ref(),
        &session_id,
        Arc::clone(&event_broker),
        frame_requester.clone(),
    )
    .await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        LeaveAlternateScreen
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
        let request_id = crab_common::utils::id::new_ulid();
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
    skill_registry: &SkillRegistry,
    session_history: Option<&SessionHistory>,
    session_id: &str,
    event_broker: Arc<EventBroker>,
    frame_requester: FrameRequester,
) -> anyhow::Result<()> {
    // Channel to get conversation back from agent task
    let mut conv_return: Option<tokio::sync::oneshot::Receiver<AgentTaskResult>> = None;
    let mut cancel = tool_ctx.cancellation_token.clone();

    // Subscribe once — receivers must be live before any send to observe it.
    let mut frame_rx = frame_requester.subscribe();

    loop {
        // Render
        terminal.draw(|frame| {
            app.render(frame.area(), frame.buffer_mut());
        })?;

        // Wait for TUI event, agent task completion, or an explicit redraw request
        let event = tokio::select! {
            ev = tui_rx.recv() => {
                match ev {
                    Some(e) => Some(e),
                    None => break,
                }
            }
            // A redraw signal alone is enough to loop back and re-render. We use
            // `recv()`'s `Lagged` variant as benign — drain and re-render.
            _ = frame_rx.recv() => {
                continue;
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
                        // Auto-save session after each agent turn
                        if let Some(history) = session_history
                            && let Err(e) = history.save(session_id, conversation.messages())
                        {
                            let _ = event_tx.send(Event::Error {
                                message: format!("Session save failed: {e}"),
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
                    if let Ok(agent_result) = rx.await {
                        conversation = agent_result.conversation;
                    }
                }
                // Final session save on exit
                if let Some(history) = session_history {
                    let _ = history.save(session_id, conversation.messages());
                }
                break;
            }
            AppAction::Submit(text) => {
                // Resolve /commands to skill content
                let effective_text = resolve_slash_command(&text, skill_registry);

                // Fresh cancellation token for this request
                cancel = tokio_util::sync::CancellationToken::new();
                tool_ctx.cancellation_token = cancel.clone();

                // Take conversation, push user message, spawn agent task
                conversation.push(Message::user(&effective_text));
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
                        _token_budget: None,
                        budget_tokens: None,
                        retry_policy: None,
                        hook_executor: None,
                        session_id: None,
                        effort: None,
                        fallback_model: None,
                    };

                    let mut task_cost_tracker = crab_session::CostAccumulator::default();
                    let result = crab_agent::query_loop(
                        &mut task_conversation,
                        &task_backend,
                        &task_executor,
                        &task_ctx,
                        &config,
                        &mut task_cost_tracker,
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
            AppAction::NewSession | AppAction::SwitchSession(_) => {
                // Session management actions are handled by the outer runner
                // when multi-session support is fully wired up.
                // For now, log to content buffer.
                if matches!(action, AppAction::NewSession) {
                    app.content_buffer
                        .push_str("\n[session] New session requested (not yet wired)\n");
                }
            }
            AppAction::ExternalEditor(initial_text) => {
                // Hand the terminal off to $EDITOR. We must restore raw mode etc.
                // before spawning, then re-enter after.
                disable_raw_mode().ok();
                execute!(
                    terminal.backend_mut(),
                    DisableBracketedPaste,
                    LeaveAlternateScreen
                )
                .ok();

                let editor_result = run_external_editor(&event_broker, &initial_text, None).await;

                // Always re-enter the alt screen + raw mode, even on error.
                enable_raw_mode().ok();
                execute!(
                    terminal.backend_mut(),
                    EnterAlternateScreen,
                    EnableBracketedPaste
                )
                .ok();
                terminal.clear().ok();

                match editor_result {
                    Ok(text) => {
                        app.apply_event(AppEvent::ExternalEditorClosed(text));
                    }
                    Err(e) => {
                        use std::fmt::Write as _;
                        let _ = write!(app.content_buffer, "\n[external editor error: {e}]\n");
                    }
                }
                frame_requester.request_frame();
            }
            AppAction::None => {}
        }
    }

    Ok(())
}

/// RAII guard that resumes an `EventBroker` when dropped — used by the external
/// editor flow so the broker is never left paused on an early-return path.
struct ResumeGuard<'a>(&'a EventBroker);

impl Drop for ResumeGuard<'_> {
    fn drop(&mut self) {
        self.0.resume();
    }
}

/// Spawn `$EDITOR` against a tempfile seeded with `initial_text` and return the
/// resulting file contents.
///
/// `editor_override` lets tests force a specific command (e.g. `cmd /c exit`)
/// instead of resolving from the environment.
///
/// Always pauses the broker on entry and resumes on exit (even on error), so
/// crossterm input is never silently swallowed after a failure.
async fn run_external_editor(
    broker: &Arc<EventBroker>,
    initial_text: &str,
    editor_override: Option<&str>,
) -> anyhow::Result<String> {
    use std::io::Write as _;

    broker.pause();
    let _guard = ResumeGuard(broker.as_ref());

    let id = uuid::Uuid::new_v4().simple().to_string();
    let path = std::env::temp_dir().join(format!("crab_edit_{id}.txt"));

    // Seed the file with the current input text so $EDITOR opens with it.
    {
        let mut f = std::fs::File::create(&path)?;
        f.write_all(initial_text.as_bytes())?;
    }

    let editor: String = match editor_override {
        Some(s) => s.to_string(),
        None => std::env::var("EDITOR")
            .or_else(|_| std::env::var("VISUAL"))
            .unwrap_or_else(|_| {
                if cfg!(windows) {
                    "notepad".to_string()
                } else {
                    "vi".to_string()
                }
            }),
    };

    // Split editor into command + leading args (so `code -w` style works).
    let mut parts = editor.split_whitespace();
    let cmd = parts.next().unwrap_or("vi");
    let leading_args: Vec<&str> = parts.collect();

    let status = tokio::process::Command::new(cmd)
        .args(&leading_args)
        .arg(&path)
        .status()
        .await;

    let result = match status {
        Ok(_) => std::fs::read_to_string(&path).map_err(anyhow::Error::from),
        Err(e) => Err(anyhow::Error::from(e)),
    };

    // Best-effort cleanup; ignore failure (e.g. file already gone).
    let _ = std::fs::remove_file(&path);

    result
}

/// Resolve `/command` input to skill content if a matching skill exists.
///
/// If the input starts with `/` and matches a registered skill command,
/// returns the skill's prompt content (with any arguments appended).
/// Otherwise returns the original input unchanged.
fn resolve_slash_command(input: &str, skill_registry: &SkillRegistry) -> String {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return input.to_string();
    }

    let command = trimmed
        .trim_start_matches('/')
        .split_whitespace()
        .next()
        .unwrap_or("");

    // Built-in commands pass through
    if matches!(command, "exit" | "quit" | "help") {
        return input.to_string();
    }

    if let Some(skill) = skill_registry.find_command(command) {
        let args = trimmed
            .trim_start_matches('/')
            .trim_start_matches(command)
            .trim();

        let mut prompt = skill.content.clone();
        if !args.is_empty() {
            prompt.push_str("\n\nUser arguments: ");
            prompt.push_str(args);
        }
        return prompt;
    }

    input.to_string()
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
    use crab_skill::{Skill, SkillTrigger};

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

    #[test]
    fn resolve_slash_command_passthrough() {
        let reg = SkillRegistry::new();
        assert_eq!(resolve_slash_command("hello world", &reg), "hello world");
    }

    #[test]
    fn resolve_slash_command_builtin() {
        let reg = SkillRegistry::new();
        assert_eq!(resolve_slash_command("/exit", &reg), "/exit");
        assert_eq!(resolve_slash_command("/quit", &reg), "/quit");
        assert_eq!(resolve_slash_command("/help", &reg), "/help");
    }

    #[test]
    fn resolve_slash_command_no_match() {
        let reg = SkillRegistry::new();
        assert_eq!(resolve_slash_command("/unknown", &reg), "/unknown");
    }

    #[test]
    fn resolve_slash_command_matches_skill() {
        let mut reg = SkillRegistry::new();
        reg.register(Skill {
            trigger: SkillTrigger::Command {
                name: "commit".into(),
            },
            ..Skill::new("commit", "You are a commit helper.")
        });

        let result = resolve_slash_command("/commit", &reg);
        assert_eq!(result, "You are a commit helper.");
    }

    #[test]
    fn resolve_slash_command_with_args() {
        let mut reg = SkillRegistry::new();
        reg.register(Skill {
            trigger: SkillTrigger::Command {
                name: "review".into(),
            },
            ..Skill::new("review", "Review the code.")
        });

        let result = resolve_slash_command("/review src/main.rs", &reg);
        assert!(result.contains("Review the code."));
        assert!(result.contains("src/main.rs"));
    }

    #[tokio::test]
    async fn run_external_editor_roundtrip_with_noop_editor() {
        // Use a no-op "editor": on Windows `cmd /c exit`, on Unix `true`.
        // The editor exits immediately without modifying the file, so we
        // expect to get back exactly what we seeded.
        let broker = Arc::new(EventBroker::new());
        let initial = "hello from crab";

        #[cfg(windows)]
        let fake_editor = "cmd /c exit";
        #[cfg(not(windows))]
        let fake_editor = "true";

        let result = run_external_editor(&broker, initial, Some(fake_editor)).await;

        assert!(result.is_ok(), "editor flow returned error: {result:?}");
        assert_eq!(result.unwrap(), initial);
        // Broker must be resumed after the editor returns.
        assert!(!broker.is_paused(), "broker not resumed after editor");
    }

    #[tokio::test]
    async fn run_external_editor_resumes_broker_on_failure() {
        // A nonexistent editor must still resume the broker — otherwise the
        // TUI would be stuck silently dropping all keystrokes.
        let broker = Arc::new(EventBroker::new());
        let result = run_external_editor(
            &broker,
            "data",
            Some("definitely-not-an-editor-binary-xyzzy"),
        )
        .await;
        assert!(result.is_err(), "expected spawn failure");
        assert!(
            !broker.is_paused(),
            "broker must be resumed even when editor spawn fails"
        );
    }

    #[test]
    fn tui_config_construction() {
        let config = TuiConfig {
            session_config: SessionConfig {
                session_id: "test".into(),
                system_prompt: "You are helpful.".into(),
                model: crab_core::model::ModelId::from("test-model"),
                max_tokens: 4096,
                temperature: None,
                context_window: 200_000,
                working_dir: PathBuf::from("/tmp"),
                permission_policy: crab_core::permission::PermissionPolicy::default(),
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
                bare_mode: false,
                worktree_name: None,
                fork_session: false,
                from_pr: None,
                custom_session_id: None,
                json_schema: None,
                plugin_dirs: Vec::new(),
                disable_skills: false,
                beta_headers: Vec::new(),
                ide_connect: false,
            },
            backend: Arc::new(crab_api::LlmBackend::OpenAi(
                crab_api::openai::OpenAiClient::new("http://localhost:0/v1", None),
            )),
            skill_dirs: vec![],
        };
        assert_eq!(config.session_config.session_id, "test");
        assert!(config.skill_dirs.is_empty());
    }
}
