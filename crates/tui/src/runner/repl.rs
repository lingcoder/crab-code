//! TUI REPL — the read-eval-print loop that drives the interactive session.
//!
//! Handles user input, agent events, init completion, filesystem watch
//! events, SIGCONT (Unix), and external-editor handoffs.

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
use crab_agents::runtime::{AgentRuntime, RuntimeInitMeta};
use crab_commands::CommandRegistry;
use crab_core::event::Event;

use crate::app::{App, AppAction};
use crate::app_event::AppEvent;
use crate::event_broker::EventBroker;
use crate::frame_requester::FrameRequester;

use super::slash::{SubmitOutcome, handle_submit};

/// The core render + event loop.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(super) async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    tui_rx: &mut mpsc::UnboundedReceiver<crate::event::TuiEvent>,
    init_rx: tokio::sync::oneshot::Receiver<(AgentRuntime, RuntimeInitMeta)>,
    watch_rx: &mut mpsc::UnboundedReceiver<crate::watcher::WatchEvent>,
    backend: Arc<LlmBackend>,
    event_tx: mpsc::Sender<Event>,
    tagged_tx: mpsc::UnboundedSender<(u64, Event)>,
    perm_resp_tx: mpsc::UnboundedSender<(String, bool, Option<String>)>,
    session_id: &str,
    event_broker: Arc<EventBroker>,
    frame_requester: FrameRequester,
    skill_dirs: &[PathBuf],
) -> anyhow::Result<()> {
    // `state` starts as None (Initializing) and is populated by InitComplete.
    let mut state: Option<AgentRuntime> = None;
    let slash_registry = CommandRegistry::new();
    let mut init_rx = Some(init_rx);

    let mut conv_return: Option<tokio::sync::oneshot::Receiver<crab_agents::QueryTaskResult>> =
        None;
    let mut cancel = tokio_util::sync::CancellationToken::new();
    // Monotonically incremented for every spawned query. The REPL discards
    // any `TuiEvent::Agent` whose tagged epoch does not match. This guards
    // the main loop against stale events from a query that was cancelled
    // (Ctrl+C) but is still draining its event channel before its task
    // notices the cancellation token.
    let mut query_epoch: u64 = 0;

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
                if let Ok((runtime, meta)) = result {
                    app.tool_registry = Some(meta.tool_registry);
                    app.session_sidebar.set_sessions(to_sidebar_entries(&meta.sidebar_entries));
                    for name in &meta.mcp_failures {
                        app.notifications.warn(format!("MCP server '{name}' failed to connect"));
                    }
                    if !meta.resumed_grants.is_empty() {
                        app.session_grants =
                            meta.resumed_grants.iter().cloned().collect();
                    }
                    app.state = crate::app::AppState::Idle;

                    push_welcome_if_needed(app);
                    push_startup_overlays(app);

                    cancel = runtime.cancellation_token().clone();
                    let working_dir = app.working_dir.clone();
                    // Route future notification pushes through the Notification
                    // hook. `notification_hook_sink` returns `None` when no
                    // HookExecutor is configured, so this is a no-op in that case.
                    if let Some(sink) = runtime.notification_hook_sink() {
                        app.notifications.set_on_push(sink);
                    }
                    state = Some(runtime);
                    if let Some(ref rt) = state {
                        rt.fire_lifecycle_hook(
                            crab_agents::HookTrigger::SessionStart,
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
                            rt.restore_conversation(agent_result.conversation);
                            rt.merge_cost(&agent_result.cost);
                            if let Err(e) = agent_result.result {
                                let _ = event_tx.send(Event::Error {
                                    message: e.to_string(),
                                }).await;
                            }
                            // Intercept any TeamCreate markers the model
                            // emitted during this turn. The coordinator
                            // deduplicates by team name so scanning the
                            // full conversation each turn is safe and
                            // cheap (only new tool_result content matches).
                            rt.process_team_markers(0).await;
                            // Refresh the TUI-side snapshot so /team
                            // shows the latest teammate roster without
                            // needing to reach back into the runtime
                            // from the render thread.
                            app.team_snapshot = rt.team_snapshot();
                            let grants: Vec<String> =
                                app.session_grants.iter().cloned().collect();
                            rt.save_session_with_grants(session_id, &grants);
                        }
                        Err(_) => {
                            let _ = event_tx.send(Event::Error {
                                message: "agent task panicked".into(),
                            }).await;
                        }
                    }

                    if let Some(queued_text) = app.dequeue_command() {
                        match handle_submit(rt, app, &slash_registry, &queued_text, session_id).await {
                            SubmitOutcome::SpawnQuery(prompt) => {
                                cancel = tokio_util::sync::CancellationToken::new();
                                rt.tool_ctx_mut().cancellation_token = cancel.clone();

                                let user_msg = rt.expand_input(&prompt);
                                rt.conversation_mut().push(user_msg);

                                query_epoch = query_epoch.wrapping_add(1);
                                let engine_tx =
                                    spawn_query_event_forwarder(query_epoch, &tagged_tx);
                                conv_return = Some(rt.spawn_query(
                                    &backend,
                                    engine_tx,
                                    cancel.clone(),
                                ));
                            }
                            SubmitOutcome::Handled => {
                                // Slash command fully handled locally — go
                                // back to Idle so the user can type again.
                                app.state = crate::app::AppState::Idle;
                                app.spinner.stop();
                            }
                            SubmitOutcome::Quit => {
                                // /exit hit from the queued path — mirror
                                // the AppAction::Quit branch: cancel, fire
                                // SessionEnd, save, break out of run_loop.
                                cancel.cancel();
                                rt.fire_lifecycle_hook(
                                    crab_agents::HookTrigger::SessionEnd,
                                    Some(session_id),
                                    if app.working_dir.is_empty() {
                                        None
                                    } else {
                                        Some(std::path::Path::new(&app.working_dir))
                                    },
                                );
                                let grants: Vec<String> =
                                    app.session_grants.iter().cloned().collect();
                                rt.save_session_with_grants(session_id, &grants);
                                app.should_quit = true;
                            }
                        }
                        if app.should_quit {
                            break;
                        }
                    }
                }
                continue;
            }
            // Filesystem watch events (settings/skills changed)
            watch_event = watch_rx.recv() => {
                if let Some(we) = watch_event {
                    let wd = if app.working_dir.is_empty() {
                        None
                    } else {
                        Some(std::path::PathBuf::from(&app.working_dir))
                    };
                    match we {
                        crate::watcher::WatchEvent::SettingsChanged => {
                            let warnings = reload_settings(app, state.as_mut());
                            app.apply_event(crate::app_event::AppEvent::SettingsReloaded { warnings });
                            // FileChanged hook fires after reload so hooks
                            // observe the already-applied state; the
                            // virtual path "config.toml" lets glob-based
                            // filters match without having to know the
                            // full merged path chain.
                            if let Some(ref rt) = state {
                                rt.fire_file_changed_hook(
                                    std::path::Path::new("config.toml"),
                                    Some(session_id),
                                    wd.as_deref(),
                                );
                            }
                        }
                        crate::watcher::WatchEvent::SkillsChanged => {
                            if let Some(ref mut rt) = state {
                                let count = rt.reload_skills(skill_dirs);
                                app.apply_event(crate::app_event::AppEvent::SkillsReloaded { count });
                                rt.fire_file_changed_hook(
                                    std::path::Path::new("skills/"),
                                    Some(session_id),
                                    wd.as_deref(),
                                );
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
        // Drop agent events tagged with a stale epoch — they belong to a
        // query the user already cancelled. The sentinel
        // `ALWAYS_CURRENT_EPOCH` bypasses the filter for events that are
        // not tied to a single query.
        if let crate::event::TuiEvent::Agent { epoch, .. } = &event
            && *epoch != query_epoch
            && *epoch != crate::event::ALWAYS_CURRENT_EPOCH
        {
            continue;
        }
        let action = app.handle_event(event);

        match action {
            AppAction::Quit => {
                cancel.cancel();
                if let Some(rx) = conv_return.take()
                    && let Ok(agent_result) = rx.await
                    && let Some(ref mut rt) = state
                {
                    rt.restore_conversation(agent_result.conversation);
                }
                if let Some(ref rt) = state {
                    let working_dir = &app.working_dir;
                    rt.fire_lifecycle_hook(
                        crab_agents::HookTrigger::SessionEnd,
                        Some(session_id),
                        if working_dir.is_empty() {
                            None
                        } else {
                            Some(std::path::Path::new(working_dir))
                        },
                    );
                    let grants: Vec<String> = app.session_grants.iter().cloned().collect();
                    rt.save_session_with_grants(session_id, &grants);
                }
                break;
            }
            AppAction::Submit(text) => {
                let Some(ref mut rt) = state else {
                    continue;
                };

                match handle_submit(rt, app, &slash_registry, &text, session_id).await {
                    SubmitOutcome::SpawnQuery(prompt) => {
                        cancel = tokio_util::sync::CancellationToken::new();
                        rt.tool_ctx_mut().cancellation_token = cancel.clone();

                        let user_msg = rt.expand_input(&prompt);
                        rt.conversation_mut().push(user_msg);

                        query_epoch = query_epoch.wrapping_add(1);
                        let engine_tx = spawn_query_event_forwarder(query_epoch, &tagged_tx);
                        conv_return = Some(rt.spawn_query(&backend, engine_tx, cancel.clone()));
                    }
                    SubmitOutcome::Handled => {
                        app.state = crate::app::AppState::Idle;
                        app.spinner.stop();
                    }
                    SubmitOutcome::Quit => {
                        cancel.cancel();
                        rt.fire_lifecycle_hook(
                            crab_agents::HookTrigger::SessionEnd,
                            Some(session_id),
                            if app.working_dir.is_empty() {
                                None
                            } else {
                                Some(std::path::Path::new(&app.working_dir))
                            },
                        );
                        let grants: Vec<String> = app.session_grants.iter().cloned().collect();
                        rt.save_session_with_grants(session_id, &grants);
                        break;
                    }
                }
            }
            AppAction::PermissionResponse {
                request_id,
                allowed,
                feedback,
            } => {
                let _ = perm_resp_tx.send((request_id, allowed, feedback));
            }
            AppAction::InterruptPermissions { rejected_ids } => {
                for id in rejected_ids {
                    let _ = perm_resp_tx.send((id, false, None));
                }
                cancel.cancel();
            }
            AppAction::InterruptProcessing => {
                cancel.cancel();
            }
            AppAction::NewSession => {
                if let Some(ref mut rt) = state {
                    let grants: Vec<String> = app.session_grants.iter().cloned().collect();
                    rt.save_session_with_grants(session_id, &grants);
                    rt.new_session(session_id);
                }
                app.reset_for_new_session();
            }
            AppAction::SwitchSession(target_id) => {
                if let Some(ref mut rt) = state {
                    let outgoing: Vec<String> = app.session_grants.iter().cloned().collect();
                    if let Some(loaded_grants) =
                        rt.switch_session_with_grants(session_id, &outgoing, &target_id)
                    {
                        app.load_session_messages(rt.conversation());
                        app.session_grants = loaded_grants.into_iter().collect();
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
            AppAction::FireSetupHook { project_path } => {
                if let Some(ref rt) = state {
                    rt.fire_lifecycle_hook(
                        crab_agents::HookTrigger::Setup,
                        Some(session_id),
                        Some(std::path::Path::new(&project_path)),
                    );
                }
            }
            AppAction::None => {}
        }
    }

    Ok(())
}

/// Decide whether the welcome panel should display on this start.
///
/// Three independent triggers (mirroring upstream's logo logic):
///   1. The current binary version differs from `state.last_welcome_version`
///   2. The project has no `AGENTS.md` (new project → show creation hint)
///   3. `CRAB_FORCE_FULL_LOGO` env var is truthy
fn welcome_triggers(
    state: &crate::global_state::GlobalState,
    project_dir: &std::path::Path,
) -> (bool, bool) {
    let force = std::env::var("CRAB_FORCE_FULL_LOGO")
        .is_ok_and(|v| !matches!(v.as_str(), "" | "0" | "false" | "no" | "off"));
    let version_new = crate::global_state::should_show_welcome(state, env!("CARGO_PKG_VERSION"));
    let is_new_project =
        !project_dir.as_os_str().is_empty() && !project_dir.join("AGENTS.md").exists();
    let should_show = force || version_new || is_new_project;
    (should_show, is_new_project)
}

/// Push a `ChatMessage::Welcome` at the front of the transcript when
/// conditions warrant. Also updates `last_welcome_version` so subsequent
/// starts on the same version stay quiet.
///
/// Recent activity lives in the sidebar, so this helper takes no session
/// arguments — the welcome cell only shows the banner + release notes +
/// a one-line action hint.
fn push_welcome_if_needed(app: &mut App) {
    let mut state = crate::global_state::load();
    let project_dir = std::path::PathBuf::from(&app.working_dir);
    let (should_show, show_project_hint) = welcome_triggers(&state, &project_dir);
    if !should_show {
        return;
    }

    // What's new — top bullets from docs/CHANGELOG.md's most recent entry.
    let whats_new = crate::changelog::whats_new(3);

    let msg = crate::app::ChatMessage::Welcome {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        whats_new,
        show_project_hint,
    };
    app.messages.insert(0, msg);

    crate::global_state::record_welcome_seen(&mut state, env!("CARGO_PKG_VERSION"));
    if let Err(e) = crate::global_state::save(&state) {
        app.notifications
            .warn(format!("Failed to persist welcome state: {e}"));
    }
}

/// Push the trust overlay if the current project needs it.
///
/// Called once after background init completes.
fn push_startup_overlays(app: &mut App) {
    if app.working_dir.is_empty() {
        return;
    }
    let global_state = crate::global_state::load();
    let project_dir = std::path::PathBuf::from(&app.working_dir);
    if !crate::global_state::needs_trust_prompt(&global_state, &project_dir) {
        return;
    }
    let ctx = crate::components::trust_dialog::TrustContext::from_project(&project_dir);
    if ctx.is_empty() {
        return;
    }
    let overlay =
        crate::components::trust_dialog::TrustDialogOverlay::new(app.working_dir.clone(), ctx);
    app.overlay_stack.push(Box::new(overlay));
}

/// Reload settings from disk, returning any validation warnings.
fn reload_settings(app: &mut App, rt: Option<&mut AgentRuntime>) -> Vec<String> {
    let project_dir = if app.working_dir.is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(&app.working_dir))
    };

    let ctx = crab_config::ResolveContext::new()
        .with_project_dir(project_dir.clone())
        .with_process_env();
    let validation_warnings = crab_config::validate_all_config_files(project_dir.as_deref());
    match crab_config::resolve(&ctx) {
        Ok(settings) => {
            let errors = validation_warnings;
            if let Some(rt) = rt {
                if let Some(ref model) = settings.model {
                    let model_id = crab_core::model::ModelId::from(model.as_str());
                    rt.loop_config_mut().model = model_id;
                    app.model_name.clone_from(model);
                }
                if let Some(max_tokens) = settings.max_tokens {
                    rt.loop_config_mut().max_tokens = max_tokens;
                }
                if let Some(ref mode_str) = settings.permission_mode
                    && let Ok(mode) = mode_str.parse()
                {
                    app.permission_mode = mode;
                    rt.tool_ctx_mut().permission_mode = mode;
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

/// Convert session metadata into sidebar entries for the TUI.
fn to_sidebar_entries(
    metas: &[crab_agents::SessionMetadata],
) -> Vec<crate::components::session_sidebar::SessionEntry> {
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

/// Spawn a per-query forwarder task that stamps every engine `Event` with
/// the query's `epoch` and pushes the tagged pair into the TUI's long-lived
/// tagged channel.
///
/// Returns the bounded `Sender<Event>` to hand to `AgentRuntime::spawn_query`
/// — the engine writes plain `Event`s and stays ignorant of epochs. When the
/// query task drops its sender (or the receiver) the forwarder exits.
fn spawn_query_event_forwarder(
    epoch: u64,
    tagged_tx: &mpsc::UnboundedSender<(u64, Event)>,
) -> mpsc::Sender<Event> {
    let (engine_tx, mut engine_rx) = mpsc::channel::<Event>(256);
    let tagged = tagged_tx.clone();
    tokio::spawn(async move {
        while let Some(event) = engine_rx.recv().await {
            if tagged.send((epoch, event)).is_err() {
                break;
            }
        }
    });
    engine_tx
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
}
