//! Terminal setup and background-task spawning for the TUI runner.
//!
//! The TUI uses a UI-first strategy: the terminal is brought up immediately
//! in an `Initializing` state while MCP, memory, session, and skill loading
//! happen on a background task. Once ready, the event loop receives an
//! `InitResult` via a oneshot channel and transitions to `Idle`.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;

use crab_agents::LlmBackend;
use crab_agents::runtime::{AgentRuntime, RuntimeInitConfig, RuntimeInitMeta};
use crab_core::event::Event;

use crate::app::App;
use crate::event::spawn_event_loop;
use crate::event_broker::EventBroker;
use crate::frame_requester::FrameRequester;

use super::TuiConfig;
use super::repl::spawn_event_forwarder;
use super::slash::builtin_slash_commands;

/// All resources prepared by [`prepare`] and consumed by the event loop.
///
/// `_file_watcher` is held purely for its `Drop` side effect (it stops the
/// background notify thread when dropped), so it stays alive as long as the
/// `PreparedRuntime` does.
pub(super) struct PreparedRuntime {
    pub(super) terminal: Terminal<CrosstermBackend<io::Stdout>>,
    pub(super) app: App,
    pub(super) tui_rx: mpsc::UnboundedReceiver<crate::event::TuiEvent>,
    pub(super) init_rx: tokio::sync::oneshot::Receiver<(AgentRuntime, RuntimeInitMeta)>,
    pub(super) watch_rx: mpsc::UnboundedReceiver<crate::watcher::WatchEvent>,
    pub(super) backend: Arc<LlmBackend>,
    pub(super) event_tx: mpsc::Sender<Event>,
    pub(super) perm_resp_tx: mpsc::UnboundedSender<(String, bool)>,
    pub(super) session_id: String,
    pub(super) event_broker: Arc<EventBroker>,
    pub(super) frame_requester: FrameRequester,
    pub(super) skill_dirs: Vec<PathBuf>,
    pub(super) _file_watcher: Option<crate::watcher::FileWatcher>,
}

/// Set up the terminal, build the initial [`App`], spawn background
/// initialization, and return everything needed to enter the event loop.
pub(super) fn prepare(config: TuiConfig) -> anyhow::Result<PreparedRuntime> {
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
    let terminal = Terminal::new(term_backend)?;

    // ── Phase 2: Create App in Initializing state ────────────────────────

    let model_name = config.session_config.model.as_str().to_string();
    let mut app = App::new(&model_name);
    app.state = crate::app::AppState::Initializing;
    if let Ok(cwd) = std::env::current_dir() {
        app.set_working_dir(cwd.display().to_string());
        app.set_completion_cwd(cwd);
    }
    if let Some(memory_dir) = config.session_config.memory_dir.clone() {
        app.set_memory_dir(memory_dir);
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
    let tui_rx = spawn_event_loop(agent_ui_rx, tick_rate, Arc::clone(&event_broker));

    // ── Phase 4a: API preconnect (fire-and-forget TCP+TLS warmup) ──────

    let preconnect_backend = Arc::clone(&config.backend);
    tokio::spawn(async move {
        let _ = preconnect_backend.health_check().await;
    });

    // ── Phase 4b: Spawn background initialization ────────────────────────

    let init_config = RuntimeInitConfig {
        session_config: config.session_config.clone(),
        mcp_servers: config.mcp_servers.clone(),
        skill_dirs: config.skill_dirs.clone(),
        perm_event_tx: event_tx.clone(),
        perm_resp_rx,
    };
    let (init_tx, init_rx) = tokio::sync::oneshot::channel::<(AgentRuntime, RuntimeInitMeta)>();
    tokio::spawn(async move {
        let result = AgentRuntime::init(init_config).await;
        let _ = init_tx.send(result);
    });

    // ── Phase 4c: Settings & skills filesystem watcher ─────────────────

    let home = crab_utils::path::home_dir();
    let config_file = crab_config::config::config_file_name();
    let mut settings_watch_paths = vec![home.join(".crab").join(config_file)];
    if let Ok(cwd) = std::env::current_dir() {
        settings_watch_paths.push(cwd.join(".crab").join(config_file));
    }

    let (watch_tx, watch_rx) = mpsc::unbounded_channel();
    let file_watcher =
        crate::watcher::FileWatcher::new(&settings_watch_paths, &config.skill_dirs, watch_tx);
    let watch_rx = crate::watcher::debounced_watch(watch_rx, std::time::Duration::from_millis(500));

    // ── Phase 4d: Queue settings validation warnings as toasts ────────────

    for warning in &config.settings_warnings {
        app.notifications.warn(warning.clone());
    }

    let session_id = config.session_config.session_id.clone();
    let skill_dirs = config.skill_dirs.clone();

    Ok(PreparedRuntime {
        terminal,
        app,
        tui_rx,
        init_rx,
        watch_rx,
        backend: config.backend,
        event_tx,
        perm_resp_tx,
        session_id,
        event_broker,
        frame_requester,
        skill_dirs,
        _file_watcher: file_watcher,
    })
}
