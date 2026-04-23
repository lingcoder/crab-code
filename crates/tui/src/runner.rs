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
use crab_session::{Conversation, SessionHistory};
use crab_skill::SkillRegistry;
use crab_tools::builtin::create_default_registry;
use crab_tools::executor::{PermissionHandler, ToolExecutor};

use crate::app::{App, AppAction};
use crate::app_event::AppEvent;
use crate::event::spawn_event_loop;
use crate::event_broker::EventBroker;
use crate::frame_requester::FrameRequester;

/// Information returned when the TUI exits.
pub struct ExitInfo {
    pub session_id: String,
    pub had_conversation: bool,
}

/// Configuration for launching the TUI REPL.
pub struct TuiConfig {
    pub session_config: SessionConfig,
    pub backend: Arc<LlmBackend>,
    /// Skill directories to scan for `/command` support.
    pub skill_dirs: Vec<PathBuf>,
    /// MCP server configuration from settings (for dynamic tool registration).
    pub mcp_servers: Option<serde_json::Value>,
    /// Validation warnings from settings loading (shown as toasts after init).
    pub settings_warnings: Vec<String>,
}

/// Run the interactive TUI REPL. This is the main entry point for interactive mode.
///
/// Uses a UI-first strategy: the TUI is displayed immediately in an
/// `Initializing` state while MCP, memory, session, and skill loading
/// happen in a background task. Once ready, the event loop receives
/// `InitResult` via a oneshot channel and transitions to `Idle`.
#[allow(clippy::too_many_lines)]
pub async fn run(config: TuiConfig) -> anyhow::Result<ExitInfo> {
    // ── Phase 1: Terminal setup (instant) ────────────────────────────────

    enable_raw_mode()?;

    // Probe the terminal background color via OSC 11 while we still own
    // stdout exclusively and before switching to the alternate screen.
    let detection = crate::theme::detect_background(std::time::Duration::from_millis(80));
    let selected_theme = match detection {
        crate::theme::Detection::Light => crate::theme::Theme::light(),
        _ => crate::theme::Theme::dark(),
    };
    tracing::debug!(?detection, "terminal background detection");
    crate::theme::init_current(selected_theme);

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), DisableBracketedPaste, LeaveAlternateScreen);
        default_hook(info);
    }));

    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    let term_backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(term_backend)?;

    // ── Phase 2: Create App in Initializing state ────────────────────────

    let model_name = config.session_config.model.as_str().to_string();
    let mut app = App::new(&model_name);
    app.state = crate::app::AppState::Initializing;
    if let Ok(cwd) = std::env::current_dir() {
        app.set_working_dir(cwd.display().to_string());
        app.set_completion_cwd(cwd);
    }

    // Register built-in slash commands for Tab completion
    app.set_slash_commands(builtin_slash_commands());

    // ── Phase 3: Event infrastructure ────────────────────────────────────

    let (event_tx, event_rx) = mpsc::channel::<Event>(256);
    let (perm_resp_tx, perm_resp_rx) = mpsc::unbounded_channel::<(String, bool)>();

    let (agent_ui_tx, agent_ui_rx) = mpsc::unbounded_channel::<Event>();
    spawn_event_forwarder(event_rx, agent_ui_tx);

    let event_broker = Arc::new(EventBroker::new());
    let frame_requester = FrameRequester::default();
    let tick_rate = std::time::Duration::from_millis(100);
    let mut tui_rx = spawn_event_loop(agent_ui_rx, tick_rate, Arc::clone(&event_broker));

    // ── Phase 4a: API preconnect (fire-and-forget TCP+TLS warmup) ──────

    let preconnect_backend = Arc::clone(&config.backend);
    tokio::spawn(async move {
        let _ = preconnect_backend.health_check().await;
    });

    // ── Phase 4b: Spawn background initialization ────────────────────────

    let init_config = BackgroundInitConfig {
        session_config: config.session_config.clone(),
        mcp_servers: config.mcp_servers.clone(),
        skill_dirs: config.skill_dirs.clone(),
        perm_event_tx: event_tx.clone(),
        perm_resp_rx,
    };
    let (init_tx, init_rx) = tokio::sync::oneshot::channel::<InitResult>();
    tokio::spawn(async move {
        let result = background_init(init_config).await;
        let _ = init_tx.send(result);
    });

    // ── Phase 4c: Settings & skills filesystem watcher ─────────────────

    let home = crab_core::common::utils::path::home_dir();
    let mut settings_watch_paths = vec![home.join(".crab").join("settings.json")];
    if let Ok(cwd) = std::env::current_dir() {
        settings_watch_paths.push(cwd.join(".crab").join("settings.json"));
    }

    let (watch_tx, watch_rx) = mpsc::unbounded_channel();
    let _file_watcher =
        crate::watcher::FileWatcher::new(&settings_watch_paths, &config.skill_dirs, watch_tx);
    let mut watch_rx =
        crate::watcher::debounced_watch(watch_rx, std::time::Duration::from_millis(500));

    // ── Phase 4d: Queue settings validation warnings as toasts ────────────

    for warning in &config.settings_warnings {
        app.notifications.warn(warning.clone());
    }

    // ── Phase 5: Enter event loop immediately ────────────────────────────

    let session_id = config.session_config.session_id.clone();
    let skill_dirs = config.skill_dirs.clone();
    let result = run_loop(
        &mut terminal,
        &mut app,
        &mut tui_rx,
        init_rx,
        &mut watch_rx,
        config.backend,
        event_tx,
        perm_resp_tx,
        &session_id,
        Arc::clone(&event_broker),
        frame_requester.clone(),
        &skill_dirs,
    )
    .await;

    let exit_info = ExitInfo {
        session_id: app.session_id.clone(),
        had_conversation: !app.messages.is_empty(),
    };

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;

    result.map(|()| exit_info)
}

/// Static list of built-in slash commands for Tab completion.
fn builtin_slash_commands() -> Vec<CommandInfo> {
    vec![
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
    ]
}

/// Push onboarding and/or trust overlays based on `GlobalState`.
///
/// Called once after background init completes. The overlay stack is LIFO, so
/// we push the trust dialog first (if needed), then onboarding on top — the
/// user sees onboarding first, then trust dialog after dismissing it.
fn push_startup_overlays(app: &mut App) {
    let global_state = crab_config::global_state::load();

    if !app.working_dir.is_empty() {
        let project_dir = std::path::PathBuf::from(&app.working_dir);
        if crab_config::global_state::needs_trust_prompt(&global_state, &project_dir) {
            let ctx = crate::components::trust_dialog::TrustContext::from_project(&project_dir);
            if !ctx.is_empty() {
                let overlay = crate::components::trust_dialog::TrustDialogOverlay::new(
                    app.working_dir.clone(),
                    ctx,
                );
                app.overlay_stack.push(Box::new(overlay));
            }
        }
    }

    if !global_state.has_completed_onboarding {
        let overlay = crate::components::onboarding::OnboardingOverlay::new();
        app.overlay_stack.push(Box::new(overlay));
    }
}

/// Fire a lifecycle hook (`session_start`, `session_end`, `compact`) in the background.
///
/// Lifecycle hooks are fire-and-forget — they cannot influence control flow.
fn fire_lifecycle_hook(
    hook_executor: Option<&Arc<crab_plugin::hook::HookExecutor>>,
    trigger: crab_plugin::hook::HookTrigger,
    session_id: Option<&str>,
    working_dir: Option<&std::path::Path>,
) {
    let Some(hooks) = hook_executor.cloned() else {
        return;
    };
    let ctx = crab_plugin::hook::HookContext {
        tool_name: String::new(),
        tool_input: String::new(),
        working_dir: working_dir.map(std::path::PathBuf::from),
        tool_output: None,
        tool_exit_code: None,
        session_id: session_id.map(String::from),
    };
    tokio::spawn(async move {
        if let Err(e) = hooks.run(trigger, &ctx).await {
            tracing::warn!(?trigger, error = %e, "lifecycle hook failed");
        }
    });
}

/// Reload settings from disk, returning any validation warnings.
fn reload_settings(app: &mut App, rt: Option<&mut RuntimeState>) -> Vec<String> {
    let project_dir = if app.working_dir.is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(&app.working_dir))
    };

    match crab_config::settings::load_merged_settings_validated(project_dir.as_ref(), None) {
        Ok((settings, errors)) => {
            if let Some(rt) = rt {
                if let Some(ref model) = settings.model {
                    let model_id = crab_core::model::ModelId::from(model.as_str());
                    rt.loop_config.model = model_id;
                    app.model_name.clone_from(model);
                }
                if let Some(max_tokens) = settings.max_tokens {
                    rt.loop_config.max_tokens = max_tokens;
                }
                if let Some(ref mode_str) = settings.permission_mode
                    && let Ok(mode) = mode_str.parse()
                {
                    app.permission_mode = mode;
                    rt.tool_ctx.permission_mode = mode;
                }
            }
            errors
                .into_iter()
                .map(|e| format!("{}: {}", e.field, e.message))
                .collect()
        }
        Err(e) => {
            app.notifications
                .warn(format!("Failed to reload settings: {e}"));
            Vec::new()
        }
    }
}

/// Re-discover skills from disk directories.
fn reload_skills(skill_registry: &mut SkillRegistry, skill_dirs: &[PathBuf]) -> usize {
    match SkillRegistry::discover(skill_dirs) {
        Ok(new_registry) => {
            let count = new_registry.len();
            *skill_registry = new_registry;
            count
        }
        Err(e) => {
            tracing::warn!("failed to reload skills: {e}");
            skill_registry.len()
        }
    }
}

struct BackgroundInitConfig {
    session_config: SessionConfig,
    mcp_servers: Option<serde_json::Value>,
    skill_dirs: Vec<PathBuf>,
    perm_event_tx: mpsc::Sender<Event>,
    perm_resp_rx: mpsc::UnboundedReceiver<(String, bool)>,
}

/// Result of background initialization ��� delivered via oneshot to the event loop.
pub struct InitResult {
    pub registry: Arc<crab_tools::registry::ToolRegistry>,
    pub executor: Arc<ToolExecutor>,
    pub conversation: Conversation,
    pub tool_ctx: crab_core::tool::ToolContext,
    pub loop_config: crab_engine::QueryConfig,
    pub skill_registry: SkillRegistry,
    pub session_history: Option<SessionHistory>,
    pub sidebar_entries: Vec<crate::components::session_sidebar::SessionEntry>,
    pub mcp_failures: Vec<String>,
    pub mcp_manager: Option<crab_mcp::McpManager>,
}

impl std::fmt::Debug for InitResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InitResult")
            .field("mcp_failures", &self.mcp_failures)
            .finish_non_exhaustive()
    }
}

/// Perform all heavy initialization in the background.
async fn background_init(config: BackgroundInitConfig) -> InitResult {
    // Build tool registry and connect MCP servers
    let mut registry = create_default_registry();

    let mut mcp_failures = Vec::new();
    let mcp_manager = if let Some(ref mcp_value) = config.mcp_servers {
        let mut mgr = crab_mcp::McpManager::new();
        let failed = mgr.start_all(mcp_value).await.unwrap_or_else(|e| {
            tracing::warn!("failed to parse MCP config: {e}");
            Vec::new()
        });
        for name in &failed {
            tracing::warn!("MCP server '{name}' failed to connect");
        }
        mcp_failures = failed;
        let count = crab_tools::builtin::mcp_tool::register_mcp_tools(&mgr, &mut registry).await;
        if count > 0 {
            tracing::info!("Registered {count} MCP tool(s)");
        }
        Some(mgr)
    } else {
        None
    };

    let registry = Arc::new(registry);
    let tool_schemas = registry.tool_schemas();
    let mut executor = ToolExecutor::new(Arc::clone(&registry));

    // Wire up the TUI permission handler
    executor.set_permission_handler(Arc::new(TuiPermissionHandler {
        event_tx: config.perm_event_tx,
        response_rx: Arc::new(tokio::sync::Mutex::new(config.perm_resp_rx)),
    }));
    let executor = Arc::new(executor);

    // Load memories and build system prompt
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

    if let Some(ref store) = memory_store
        && let Ok(memories) = store.scan()
        && !memories.is_empty()
    {
        system_prompt.push_str("\n\n# Loaded Memories\n\n");
        for mem in &memories {
            use std::fmt::Write as _;
            let _ = writeln!(
                system_prompt,
                "## {} (type: {})",
                mem.metadata.name, mem.metadata.memory_type
            );
            if !mem.metadata.description.is_empty() {
                let _ = writeln!(system_prompt, "> {}", mem.metadata.description);
                system_prompt.push('\n');
            }
            let _ = writeln!(system_prompt, "{}", mem.body);
            system_prompt.push('\n');
        }
    }

    let session_id = config.session_config.session_id.clone();
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

    let loop_config = crab_engine::QueryConfig {
        model: config.session_config.model.clone(),
        max_tokens: config.session_config.max_tokens,
        temperature: config.session_config.temperature,
        tool_schemas,
        cache_enabled: false,
        budget_tokens: None,
        retry_policy: None,
        hook_executor: None,
        session_id: Some(session_id),
        effort: None,
        fallback_model: config
            .session_config
            .fallback_model
            .map(crab_core::model::ModelId::from),
        plan_model: None,
        source: crab_core::query::QuerySource::Repl,
        compaction_client: None,
        compaction_config: crab_session::CompactionConfig::default(),
        session_persister: None,
    };

    // Discover skills for /command support
    let skill_registry = SkillRegistry::discover(&config.skill_dirs).unwrap_or_default();

    // Populate session sidebar entries
    let sidebar_entries = session_history
        .as_ref()
        .and_then(|h| h.list_sessions_with_metadata().ok())
        .map(|metas| {
            metas
                .iter()
                .map(|m| crate::components::session_sidebar::SessionEntry {
                    id: m.session_id.clone(),
                    name: m.name.clone().unwrap_or_else(|| m.session_id.clone()),
                    last_active: m
                        .modified
                        .and_then(|t| t.elapsed().ok())
                        .map_or_else(|| "unknown".into(), |d| format!("{}s ago", d.as_secs())),
                    message_count: m.message_count,
                })
                .collect()
        })
        .unwrap_or_default();

    InitResult {
        registry,
        executor,
        conversation,
        tool_ctx,
        loop_config,
        skill_registry,
        session_history,
        sidebar_entries,
        mcp_failures,
        mcp_manager,
    }
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
        let request_id = crab_core::common::utils::id::new_ulid();
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
    result: crab_core::Result<()>,
    cost: crab_session::CostAccumulator,
}

/// Resources populated by `background_init` and delivered via oneshot.
///
/// Before receiving these, the event loop renders the app in
/// `Initializing` state and rejects user submissions.
struct RuntimeState {
    conversation: Conversation,
    executor: Arc<ToolExecutor>,
    tool_ctx: crab_core::tool::ToolContext,
    loop_config: crab_engine::QueryConfig,
    skill_registry: SkillRegistry,
    session_history: Option<SessionHistory>,
    _mcp_manager: Option<crab_mcp::McpManager>,
    cost: crab_session::CostAccumulator,
}

/// The core render + event loop.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    tui_rx: &mut mpsc::UnboundedReceiver<crate::event::TuiEvent>,
    init_rx: tokio::sync::oneshot::Receiver<InitResult>,
    watch_rx: &mut mpsc::UnboundedReceiver<crate::watcher::WatchEvent>,
    backend: Arc<LlmBackend>,
    event_tx: mpsc::Sender<Event>,
    perm_resp_tx: mpsc::UnboundedSender<(String, bool)>,
    session_id: &str,
    event_broker: Arc<EventBroker>,
    frame_requester: FrameRequester,
    skill_dirs: &[PathBuf],
) -> anyhow::Result<()> {
    // `state` starts as None (Initializing) and is populated by InitComplete.
    let mut state: Option<RuntimeState> = None;
    let mut init_rx = Some(init_rx);

    let mut conv_return: Option<tokio::sync::oneshot::Receiver<AgentTaskResult>> = None;
    let mut cancel = tokio_util::sync::CancellationToken::new();

    let mut frame_rx = frame_requester.subscribe();

    let mut sigcont_stream = SigcontStream::new()?;

    loop {
        terminal.draw(|frame| {
            app.render(frame.area(), frame.buffer_mut());
        })?;

        let event = tokio::select! {
            ev = tui_rx.recv() => {
                match ev {
                    Some(e) => Some(e),
                    None => break,
                }
            }
            _ = frame_rx.recv() => {
                continue;
            }
            // Wait for background init to complete
            result = async {
                match init_rx.as_mut() {
                    Some(rx) => rx.await,
                    None => std::future::pending().await,
                }
            } => {
                init_rx = None;
                if let Ok(init) = result {
                    app.tool_registry = Some(init.registry);
                    app.session_sidebar.set_sessions(init.sidebar_entries);
                    for name in &init.mcp_failures {
                        app.notifications.warn(format!("MCP server '{name}' failed to connect"));
                    }
                    app.state = crate::app::AppState::Idle;

                    // Push onboarding / trust overlays (trust first so
                    // onboarding renders on top and is dismissed first).
                    push_startup_overlays(app);

                    cancel = init.tool_ctx.cancellation_token.clone();
                    let working_dir = app.working_dir.clone();
                    state = Some(RuntimeState {
                        conversation: init.conversation,
                        executor: init.executor,
                        tool_ctx: init.tool_ctx,
                        loop_config: init.loop_config,
                        skill_registry: init.skill_registry,
                        session_history: init.session_history,
                        _mcp_manager: init.mcp_manager,
                        cost: crab_session::CostAccumulator::default(),
                    });
                    if let Some(ref rt) = state {
                        fire_lifecycle_hook(
                            rt.loop_config.hook_executor.as_ref(),
                            crab_plugin::hook::HookTrigger::SessionStart,
                            Some(session_id),
                            if working_dir.is_empty() {
                                None
                            } else {
                                Some(std::path::Path::new(&working_dir))
                            },
                        );
                    }
                } else {
                    app.notifications.warn("Background initialization failed".to_string());
                    app.state = crate::app::AppState::Idle;
                }
                continue;
            }
            // Wait for agent task to return conversation
            result = async {
                match conv_return.as_mut() {
                    Some(rx) => rx.await,
                    None => std::future::pending().await,
                }
            } => {
                conv_return = None;
                if let Some(ref mut rt) = state {
                    match result {
                        Ok(agent_result) => {
                            rt.conversation = agent_result.conversation;
                            rt.cost.merge(&agent_result.cost);
                            if let Err(e) = agent_result.result {
                                let _ = event_tx.send(Event::Error {
                                    message: e.to_string(),
                                }).await;
                            }
                            if let Some(ref history) = rt.session_history
                                && let Err(e) = history.save(session_id, rt.conversation.messages())
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

                    if let Some(queued_text) = app.dequeue_command() {
                        let effective_text =
                            resolve_slash_command(&queued_text, &rt.skill_registry);

                        cancel = tokio_util::sync::CancellationToken::new();
                        rt.tool_ctx.cancellation_token = cancel.clone();

                        let user_msg = crab_session::expand_at_mentions(
                            &effective_text,
                            &rt.tool_ctx.working_dir,
                        );
                        rt.conversation.push(user_msg);
                        let mut task_conversation = std::mem::take(&mut rt.conversation);
                        let task_backend = backend.clone();
                        let task_executor = rt.executor.clone();
                        let task_ctx = rt.tool_ctx.clone();
                        let task_config = rt.loop_config.clone();
                        let task_event_tx = event_tx.clone();
                        let task_cancel = cancel.clone();

                        let (return_tx, return_rx) = tokio::sync::oneshot::channel();
                        conv_return = Some(return_rx);

                        tokio::spawn(async move {
                            let mut task_cost = crab_session::CostAccumulator::default();
                            let result = crab_engine::query_loop(
                                &mut task_conversation,
                                &task_backend,
                                &task_executor,
                                &task_ctx,
                                &task_config,
                                &mut task_cost,
                                task_event_tx,
                                task_cancel,
                            )
                            .await;

                            let _ = return_tx.send(AgentTaskResult {
                                conversation: task_conversation,
                                result,
                                cost: task_cost,
                            });
                        });
                    }
                }
                continue;
            }
            // Filesystem watch events (settings/skills changed)
            watch_event = watch_rx.recv() => {
                if let Some(we) = watch_event {
                    match we {
                        crate::watcher::WatchEvent::SettingsChanged => {
                            let warnings = reload_settings(app, state.as_mut());
                            app.apply_event(crate::app_event::AppEvent::SettingsReloaded { warnings });
                        }
                        crate::watcher::WatchEvent::SkillsChanged => {
                            if let Some(ref mut rt) = state {
                                let count = reload_skills(&mut rt.skill_registry, skill_dirs);
                                app.apply_event(crate::app_event::AppEvent::SkillsReloaded { count });
                            }
                        }
                    }
                }
                continue;
            }
            () = sigcont_stream.recv() => {
                let _ = enable_raw_mode();
                let _ = execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste);
                terminal.clear()?;
                continue;
            }
        };

        let Some(event) = event else { break };
        let action = app.handle_event(event);

        match action {
            AppAction::Quit => {
                cancel.cancel();
                if let Some(rx) = conv_return.take()
                    && let Ok(agent_result) = rx.await
                    && let Some(ref mut rt) = state
                {
                    rt.conversation = agent_result.conversation;
                }
                if let Some(ref rt) = state {
                    let working_dir = &app.working_dir;
                    fire_lifecycle_hook(
                        rt.loop_config.hook_executor.as_ref(),
                        crab_plugin::hook::HookTrigger::SessionEnd,
                        Some(session_id),
                        if working_dir.is_empty() {
                            None
                        } else {
                            Some(std::path::Path::new(working_dir))
                        },
                    );
                    if let Some(ref history) = rt.session_history {
                        let _ = history.save(session_id, rt.conversation.messages());
                    }
                }
                break;
            }
            AppAction::Submit(text) => {
                let Some(ref mut rt) = state else {
                    continue;
                };

                let effective_text = resolve_slash_command(&text, &rt.skill_registry);

                cancel = tokio_util::sync::CancellationToken::new();
                rt.tool_ctx.cancellation_token = cancel.clone();

                let user_msg =
                    crab_session::expand_at_mentions(&effective_text, &rt.tool_ctx.working_dir);
                rt.conversation.push(user_msg);
                let mut task_conversation = std::mem::take(&mut rt.conversation);
                let task_backend = backend.clone();
                let task_executor = rt.executor.clone();
                let task_ctx = rt.tool_ctx.clone();
                let task_config = rt.loop_config.clone();
                let task_event_tx = event_tx.clone();
                let task_cancel = cancel.clone();

                let (return_tx, return_rx) = tokio::sync::oneshot::channel();
                conv_return = Some(return_rx);

                tokio::spawn(async move {
                    let mut task_cost = crab_session::CostAccumulator::default();
                    let result = crab_engine::query_loop(
                        &mut task_conversation,
                        &task_backend,
                        &task_executor,
                        &task_ctx,
                        &task_config,
                        &mut task_cost,
                        task_event_tx,
                        task_cancel,
                    )
                    .await;

                    let _ = return_tx.send(AgentTaskResult {
                        conversation: task_conversation,
                        result,
                        cost: task_cost,
                    });
                });
            }
            AppAction::PermissionResponse {
                request_id,
                allowed,
            } => {
                let _ = perm_resp_tx.send((request_id, allowed));
            }
            AppAction::InterruptPermissions { rejected_ids } => {
                for id in rejected_ids {
                    let _ = perm_resp_tx.send((id, false));
                }
                cancel.cancel();
            }
            AppAction::NewSession => {
                if let Some(ref mut rt) = state {
                    if let Some(ref history) = rt.session_history {
                        let _ = history.save(session_id, rt.conversation.messages());
                    }
                    rt.conversation = Conversation::new(
                        session_id.to_string(),
                        rt.conversation.system_prompt.clone(),
                        rt.conversation.context_window,
                    );
                }
                app.reset_for_new_session();
            }
            AppAction::SwitchSession(target_id) => {
                if let Some(ref mut rt) = state
                    && let Some(ref history) = rt.session_history
                {
                    let _ = history.save(session_id, rt.conversation.messages());
                    if let Ok(Some(messages)) = history.load(&target_id) {
                        rt.conversation = Conversation::new(
                            target_id.clone(),
                            rt.conversation.system_prompt.clone(),
                            rt.conversation.context_window,
                        );
                        for msg in messages {
                            rt.conversation.push(msg);
                        }
                        app.load_session_messages(&rt.conversation);
                    }
                }
            }
            AppAction::ExternalEditor(initial_text) => {
                disable_raw_mode().ok();
                execute!(
                    terminal.backend_mut(),
                    DisableBracketedPaste,
                    LeaveAlternateScreen
                )
                .ok();

                let editor_result = run_external_editor(&event_broker, &initial_text, None).await;

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

/// Cross-platform SIGCONT wrapper.
///
/// On Unix, listens for SIGCONT (sent after `fg` resumes a stopped process).
/// On other platforms, `recv()` is always pending.
struct SigcontStream {
    #[cfg(unix)]
    inner: tokio::signal::unix::Signal,
}

impl SigcontStream {
    fn new() -> io::Result<Self> {
        #[cfg(unix)]
        {
            let sig = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::from_raw(
                libc::SIGCONT,
            ))?;
            Ok(Self { inner: sig })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {})
        }
    }

    #[allow(clippy::needless_pass_by_ref_mut)] // &mut required on Unix
    async fn recv(&mut self) {
        #[cfg(unix)]
        {
            self.inner.recv().await;
        }
        #[cfg(not(unix))]
        {
            std::future::pending::<()>().await;
        }
    }
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
            cost: crab_session::CostAccumulator::default(),
        };
        assert!(result.result.is_ok());
    }

    #[test]
    fn agent_task_result_with_error() {
        let conv = Conversation::new("test".into(), "prompt".into(), 200_000);
        let result = AgentTaskResult {
            conversation: conv,
            result: Err(crab_core::Error::Other("test error".into())),
            cost: crab_session::CostAccumulator::default(),
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
                coordinator_mode: false,
            },
            backend: Arc::new(crab_api::LlmBackend::OpenAi(
                crab_api::openai::OpenAiClient::new("http://localhost:0/v1", None),
            )),
            skill_dirs: vec![],
            mcp_servers: None,
            settings_warnings: vec![],
        };
        assert_eq!(config.session_config.session_id, "test");
        assert!(config.skill_dirs.is_empty());
    }
}
