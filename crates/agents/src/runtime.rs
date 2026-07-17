//! [`AgentRuntime`] — high-level facade that owns all L2 service state
//! and exposes a minimal API for the TUI layer.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crab_core::event::Event;
use crab_core::model::ModelId;
use crab_core::tool::ToolContext;
use crab_engine::QueryConfig;
use crab_mcp::McpManager;
use crab_session::{
    CompactionClient, CompactionConfig, Conversation, CostAccumulator, FileHistory, MemoryStore,
    SessionHistory, SessionMetadata, compact_with_config, expand_at_mentions,
};
use crab_skills::SkillRegistry;
use crab_tools::builtin::cron::{FiredPromptQueue, SharedCronStore, shared_cron_store};
use crab_tools::builtin::register_all_builtins;
use crab_tools::executor::{PermissionHandler, PermissionResult, ToolExecutor};
use crab_tools::registry::ToolRegistry;

use crate::SessionConfig;

/// Input configuration for [`AgentRuntime::init`].
pub struct RuntimeInitConfig {
    pub session_config: SessionConfig,
    pub mcp_servers: Option<serde_json::Value>,
    pub skill_dirs: Vec<PathBuf>,
    pub perm_event_tx: mpsc::Sender<Event>,
    /// Permission response channel — `(request_id, allowed, feedback)`.
    /// `feedback` carries an optional free-text note from the user (typically
    /// only on a deny) that the executor surfaces to the model.
    pub perm_resp_rx: mpsc::UnboundedReceiver<(String, bool, Option<String>)>,
    /// Optional LLM backend used to drive `/compact` and any other
    /// summary-style sidequeries that need a real model. When `None`,
    /// the runtime falls back to the deterministic heuristic summariser.
    pub backend: Option<Arc<crab_api::LlmBackend>>,
    /// Raw `hooks` JSON from user settings (the `hooks` field of
    /// `crab_config::Config`). When present and not disabled, the runtime
    /// builds a [`crab_hooks::HookExecutor`] from it and wires it into the
    /// engine so `PreToolUse` / `PostToolUse` / `Stop` / `PostCompact` /
    /// `Notification` hooks actually run.
    pub hooks: Option<serde_json::Value>,
    /// When `true`, all hooks are skipped even if `hooks` is populated
    /// (mirrors `Config::disable_all_hooks` and `--bare`).
    pub disable_hooks: bool,
    /// Enable Anthropic prompt caching. Defaults to `true` at the call site;
    /// only Anthropic-style backends act on it, others ignore it.
    pub cache_enabled: bool,
}

/// Data returned alongside an [`AgentRuntime`] from [`AgentRuntime::init`].
pub struct RuntimeInitMeta {
    pub tool_registry: Arc<ToolRegistry>,
    pub sidebar_entries: Vec<SessionMetadata>,
    pub mcp_failures: Vec<String>,
    /// Session-level grants loaded from a resumed session (empty for new
    /// sessions or when no `resume_session_id` was supplied). The TUI
    /// rehydrates `app.session_grants` from this so users aren't
    /// re-prompted for tools they already granted in the prior run.
    pub resumed_grants: Vec<String>,
}

/// Result returned when a spawned query task completes.
pub struct QueryTaskResult {
    pub conversation: Conversation,
    pub result: crab_core::Result<()>,
    pub cost: CostAccumulator,
}

/// Fire-and-forget sink for the `Notification` hook, produced by
/// [`AgentRuntime::notification_hook_sink`] and consumed by
/// `NotificationManager::set_on_push` in the TUI crate.
///
/// Exposed as a named alias so both ends carry the same bound set
/// (`Fn(&str) + Send + Sync`, behind `Arc`), and so clippy's
/// `type_complexity` lint is satisfied where this surfaces.
pub type NotificationHookSink = std::sync::Arc<dyn Fn(&str) + Send + Sync>;

/// High-level runtime that owns all L2 service state.
///
/// The TUI holds an `Option<AgentRuntime>` (populated after background init)
/// and drives all agent interaction through this facade.
pub struct AgentRuntime {
    conversation: Conversation,
    /// Authoritative conversation metadata that stays valid even while
    /// [`take_conversation`](AgentRuntime::take_conversation) has moved the
    /// real conversation into a spawned query task (leaving a `Default`
    /// placeholder behind). TUI reads that only need id / context window /
    /// message count route through this so a mid-query settings reload never
    /// observes the empty-shell values (`id=""`, `context_window=0`).
    snapshot: ConversationSnapshot,
    executor: Arc<ToolExecutor>,
    tool_ctx: ToolContext,
    loop_config: QueryConfig,
    skill_registry: SkillRegistry,
    session_history: Option<Arc<SessionHistory>>,
    _mcp_manager: Option<Arc<tokio::sync::Mutex<McpManager>>>,
    cost: CostAccumulator,
    memory_dir: Option<PathBuf>,
    team_coordinator: crate::teams::coordinator::TeamCoordinator,
    /// LLM-backed compaction client. `None` when no backend was wired in
    /// at init time — `compact_now` then falls back to the heuristic
    /// summariser.
    compaction_client: Option<Arc<dyn CompactionClient>>,
    /// Compaction policy passed to [`compact_with_config`] for `/compact`.
    compaction_config: CompactionConfig,
    /// Session-scoped file-edit snapshot store backing `/rewind`. Populated
    /// when a sessions directory is configured (so the parent has a stable
    /// place to root file-history snapshots).
    file_history: Option<Arc<std::sync::Mutex<FileHistory>>>,
    /// Resolved shell choice for the TUI's `!` prefix
    /// (`crab_config::DefaultShell`). Captured at init time from
    /// [`SessionConfig::default_shell`].
    default_shell: crab_config::DefaultShell,
    skill_dirs: Vec<PathBuf>,
    /// Coordinator Mode activation, `Some` only when `coordinator_mode` was set.
    /// Workers it spawns get a clean (overlay-free) registry/prompt.
    coordinator: Option<crate::coordinator::Coordinator>,
    /// The system prompt *before* the Coordinator overlay — workers use this so
    /// they don't inherit the "you do not execute code" guardrail.
    worker_base_prompt: String,
    /// Background task registry — tracks running/completed background tasks
    /// (bash commands, sub-agents, teammates). Shared with tools that need
    /// to read or modify task state (`TaskStopTool`, `TaskOutputTool`).
    task_registry: Arc<std::sync::Mutex<crab_core::task::TaskRegistry>>,
    /// Authoritative session-level "always allow" grants. Seeded from a
    /// resumed session and updated by interactive frontends as the user grants
    /// tools. Auto-save reads this instead of re-reading disk, so a concurrent
    /// process resuming the same session can't clobber it.
    session_grants: std::sync::Mutex<Vec<String>>,
    /// Cron store backing the `CronCreate`/`CronDelete`/`CronList` tools and
    /// the live scheduler. Owned here so the runtime can delete one-shot jobs
    /// once their fired prompt has been consumed.
    cron_store: SharedCronStore,
    /// Queue the cron scheduler pushes fired prompts onto. The frontend drains
    /// it between turns via [`drain_due_cron_prompts`](Self::drain_due_cron_prompts)
    /// and injects each prompt as a new user turn.
    cron_fired_queue: FiredPromptQueue,
}

/// Lightweight authoritative view of the conversation that survives
/// [`AgentRuntime::take_conversation`].
///
/// Holds only the cheap-to-copy metadata the TUI reads out-of-band (id,
/// context window, message count). It is refreshed whenever a conversation
/// is installed on the runtime, so it always reflects the last in-process
/// conversation even while the live one is owned by a running query task.
#[derive(Debug, Clone, Default)]
pub struct ConversationSnapshot {
    /// Session/conversation id.
    pub id: String,
    /// Total context window in tokens (fixed for the session).
    pub context_window: u64,
    /// Message count at the time the snapshot was taken.
    pub message_count: usize,
}

impl ConversationSnapshot {
    fn from_conversation(conv: &Conversation) -> Self {
        Self {
            id: conv.id.clone(),
            context_window: conv.context_window,
            message_count: conv.len(),
        }
    }
}

/// Snapshot of the current team state — rendered by the TUI team browser.
///
/// The runtime owns the coordinator; the TUI reads a snapshot on demand
/// each time the overlay opens (no live broadcast needed because team
/// state changes only at tool-result boundaries).
#[derive(Debug, Clone, Default)]
pub struct TeamSnapshot {
    /// All teammates currently tracked by the in-process backend.
    pub members: Vec<TeamMemberSnapshot>,
}

/// Outcome of [`AgentRuntime::compact_now`], used by the TUI to render the
/// compact-boundary cell.
#[derive(Debug, Clone)]
pub struct CompactNowResult {
    /// Estimated conversation tokens before compaction.
    pub before_tokens: u64,
    /// Estimated conversation tokens after compaction.
    pub after_tokens: u64,
    /// Number of messages dropped or rewritten.
    pub removed_messages: usize,
    /// Short label identifying which strategy actually ran (e.g.
    /// `"llm-summarize"`, `"heuristic-summarizer"`).
    pub strategy: String,
}

/// One row in [`TeamSnapshot::members`].
#[derive(Debug, Clone)]
pub struct TeamMemberSnapshot {
    /// Human-readable teammate name.
    pub name: String,
    /// Role / specialty.
    pub role: String,
    /// Lifecycle state rendered as a string (Idle / Running / Done / Failed).
    pub state: String,
}

impl AgentRuntime {
    /// Perform all heavy initialization (MCP, memory, skills, session resume).
    ///
    /// This is the agent-side equivalent of the old `background_init()` in
    /// `tui/runner.rs`. Call from a spawned task so the TUI stays responsive.
    pub async fn init(config: RuntimeInitConfig) -> (Self, RuntimeInitMeta) {
        // Build the real cron store (reloads durable jobs from disk and
        // re-registers them with the scheduler) and thread it into the registry
        // so the cron tools and the runtime's fired-prompt drain share one
        // store. Registered before the Coordinator strip below so a coordinator
        // leader still loses the cron tools.
        let cron_store = shared_cron_store().await;
        let cron_fired_queue = cron_store.lock().await.fired_queue();
        let mut registry = ToolRegistry::new();
        register_all_builtins(&mut registry, None, Some(Arc::clone(&cron_store)));

        // Coordinator Mode (Layer 2b): strip the leader's registry to the
        // coordinator allow-list and overlay its prompt. `None` for plain
        // sessions. Applied in two halves because the registry and system
        // prompt are finalized at different points below.
        let coordinator =
            crate::coordinator::Coordinator::from_flag(config.session_config.coordinator_mode);

        let mut mcp_failures = Vec::new();
        let mcp_manager = if let Some(ref mcp_value) = config.mcp_servers {
            let mut mgr = McpManager::new();
            let failed = mgr.start_all(mcp_value).await.unwrap_or_else(|e| {
                tracing::warn!("failed to parse MCP config: {e}");
                Vec::new()
            });
            for name in &failed {
                tracing::warn!("MCP server '{name}' failed to connect");
            }
            mcp_failures = failed;
            let mgr_handle = Arc::new(tokio::sync::Mutex::new(mgr));
            let mgr_ref = mgr_handle.lock().await;
            let count = crab_tools::builtin::mcp_tool::register_mcp_tools(
                &mgr_ref,
                &mut registry,
                Some(Arc::clone(&mgr_handle)),
            )
            .await;
            drop(mgr_ref);
            if count > 0 {
                tracing::info!("Registered {count} MCP tool(s)");
            }
            Some(mgr_handle)
        } else {
            None
        };

        // Strip the registry to the coordinator allow-list before the schemas
        // are computed, so the leader model only sees the 3 coordinator tools.
        if let Some(coord) = &coordinator {
            coord.apply_registry(&mut registry);
            tracing::info!(allowed_tools = ?coord.allowed_tools(), "Coordinator Mode active");
        }

        let registry = Arc::new(registry);
        let tool_schemas = registry.tool_schemas();
        let hook_executor = build_hook_executor(config.hooks.as_ref(), config.disable_hooks);
        let mut executor = ToolExecutor::new(Arc::clone(&registry));

        executor.set_permission_handler(Arc::new(ChannelPermissionHandler::new(
            config.perm_event_tx,
            config.perm_resp_rx,
        )));
        if let Some(hooks) = &hook_executor {
            executor.set_permission_hooks(Arc::clone(hooks));
        }
        let executor = Arc::new(executor);

        let memory_store = config
            .session_config
            .memory_dir
            .as_ref()
            .map(|d| MemoryStore::new(d.clone()));
        let session_history = config
            .session_config
            .sessions_dir
            .as_ref()
            .map(|d| Arc::new(SessionHistory::new(d.clone())));

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

        // Snapshot the worker base prompt before overlaying the coordinator
        // guardrail, then append the overlay to the leader's prompt.
        let worker_base_prompt = system_prompt.clone();
        if let Some(coord) = &coordinator {
            coord.apply_prompt(&mut system_prompt);
        }

        let session_id = config.session_config.session_id.clone();
        let mut conversation = Conversation::new(
            session_id.clone(),
            system_prompt,
            config.session_config.context_window,
        );

        let resumed_grants = config
            .session_config
            .resume_session_id
            .as_ref()
            .zip(session_history.as_ref())
            .and_then(|(resume_id, history)| history.load_for_resume(resume_id).ok().flatten())
            .map(|(messages, grants)| {
                for msg in crab_session::sanitize_for_resume(messages) {
                    conversation.push(msg);
                }
                grants
            })
            .unwrap_or_default();

        // Seed the cost accumulator from the resumed session so the cost/token
        // display continues from where it left off instead of restarting at 0.
        let resumed_cost = config
            .session_config
            .resume_session_id
            .as_ref()
            .zip(session_history.as_ref())
            .and_then(|(resume_id, history)| history.load_cost(resume_id))
            .map(|summary| CostAccumulator::from_summary(&summary))
            .unwrap_or_default();

        // Build a session-scoped FileHistory so Edit/Write tools can snapshot
        // pre-edit file contents, and `/rewind` can restore them. The base
        // directory mirrors the sessions dir layout — `<base>/file-history/`
        // sits alongside `<base>/sessions/`. Without a configured sessions
        // dir we fall back to the OS temp dir so the wiring still works in
        // tests and one-off invocations.
        let file_history_base = config
            .session_config
            .sessions_dir
            .as_ref()
            .and_then(|p| p.parent().map(|parent| parent.join("file-history")))
            .unwrap_or_else(|| std::env::temp_dir().join("crab-file-history"));
        // On resume, root file-history at the resumed session's id so prior
        // /rewind snapshots are found (the new session id would be empty).
        let file_history_id = config
            .session_config
            .resume_session_id
            .as_deref()
            .unwrap_or(&session_id);
        let file_history = Arc::new(std::sync::Mutex::new(FileHistory::new(
            file_history_base,
            file_history_id,
        )));

        let track_edit: crab_core::tool::TrackEditFn = {
            let fh = Arc::clone(&file_history);
            Arc::new(move |path: &Path, contents: &[u8]| {
                if let Ok(mut history) = fh.lock()
                    && let Err(e) = history.track_edit(path, contents)
                {
                    tracing::warn!(error = %e, path = %path.display(), "file_history track_edit failed");
                }
            })
        };

        // Session-scoped read-state tracker so Read records reads and Edit/Write
        // can enforce read-before-edit and detect out-of-band changes.
        let (record_read, read_state) = crab_core::tool::new_read_state_tracker();

        let task_registry = Arc::new(std::sync::Mutex::new(crab_core::task::TaskRegistry::new()));
        let tool_ctx = ToolContext {
            working_dir: config.session_config.working_dir,
            permission_mode: config.session_config.permission_policy.mode,
            session_id: session_id.clone(),
            cancellation_token: CancellationToken::new(),
            permission_policy: config.session_config.permission_policy,
            ext: crab_core::tool::ToolContextExt {
                track_edit: Some(track_edit),
                record_read: Some(record_read),
                read_state: Some(read_state),
                ..Default::default()
            },
            task_registry: Some(Arc::clone(&task_registry)),
            nested_memory_triggers: Arc::new(tokio::sync::Mutex::new(HashSet::new())),
        };

        // Build the hook executor from user settings. The on-disk format is
        // `crab_config::hooks::Hook`; map it to `crab_hooks::HookDef` so the
        // engine's hook branches fire on real user-configured commands.
        // System prompt (with AGENTS.md/CLAUDE.md instructions) is fully
        // assembled by this point.
        if let Some(hooks) = &hook_executor {
            let hooks = Arc::clone(hooks);
            let session_id = config.session_config.session_id.clone();
            tokio::spawn(async move {
                let ctx = crab_hooks::HookContext {
                    session_id: Some(session_id),
                    ..crab_hooks::HookContext::default()
                };
                if let Err(e) = hooks
                    .run(crab_hooks::HookTrigger::InstructionsLoaded, &ctx)
                    .await
                {
                    tracing::warn!(error = %e, "InstructionsLoaded hook failed");
                }
            });
        }

        let compaction_config = CompactionConfig::default();
        let compaction_client: Option<Arc<dyn CompactionClient>> =
            config.backend.as_ref().map(|backend| {
                let client = crate::llm_compaction_client::LlmCompactionClient::new(
                    Arc::clone(backend),
                    config.session_config.model.clone(),
                );
                Arc::new(client) as Arc<dyn CompactionClient>
            });

        // Append each in-flight message to the session's `.jsonl` crash log so
        // a crash mid-turn does not lose the turn. The log is folded into the
        // `.json` snapshot (and truncated) after each completed turn.
        let session_persister = session_history.as_ref().map(|h| {
            Arc::new(h.persister(session_id.clone())) as Arc<dyn crab_session::SessionPersister>
        });

        let loop_config = QueryConfig {
            model: config.session_config.model.clone(),
            max_tokens: config.session_config.max_tokens,
            temperature: config.session_config.temperature,
            tool_schemas,
            cache_enabled: config.cache_enabled,
            budget_tokens: None,
            retry_policy: Some(crab_api::rate_limit::RetryPolicy::default()),
            hook_executor,
            session_id: Some(session_id),
            effort: None,
            fallback_model: config.session_config.fallback_model.map(ModelId::from),
            plan_model: None,
            source: crab_core::query::QuerySource::Repl,
            compaction_client: compaction_client.clone(),
            compaction_config: compaction_config.clone(),
            session_persister,
        };

        let mut skill_registry = SkillRegistry::new();
        skill_registry.register_all(crab_skills::builtin::builtin_skills());
        if let Ok(disk) = SkillRegistry::discover(&config.skill_dirs) {
            for skill in disk.list() {
                skill_registry.register(skill.clone());
            }
        }
        if let Some(ref mgr_handle) = mcp_manager {
            let mgr = mgr_handle.lock().await;
            let count =
                crate::mcp_skills::register_mcp_prompt_skills(&mgr, &mut skill_registry).await;
            drop(mgr);
            if count > 0 {
                tracing::info!("Registered {count} MCP prompt skill(s)");
            }
        }

        let sidebar_entries = session_history
            .as_ref()
            .and_then(|h| h.list_sessions_with_metadata().ok())
            .unwrap_or_default();

        let memory_dir = config.session_config.memory_dir.clone();
        let default_shell =
            crab_config::DefaultShell::from_str_or_default(&config.session_config.default_shell);

        let skill_dirs = config.skill_dirs.clone();

        let snapshot = ConversationSnapshot::from_conversation(&conversation);
        let runtime = Self {
            conversation,
            snapshot,
            executor,
            tool_ctx,
            loop_config,
            skill_registry,
            session_history,
            _mcp_manager: mcp_manager,
            cost: resumed_cost,
            memory_dir,
            team_coordinator: crate::teams::coordinator::TeamCoordinator::new(),
            compaction_client,
            compaction_config,
            file_history: Some(file_history),
            default_shell,
            skill_dirs,
            coordinator,
            worker_base_prompt,
            task_registry,
            session_grants: std::sync::Mutex::new(resumed_grants.clone()),
            cron_store,
            cron_fired_queue,
        };

        let meta = RuntimeInitMeta {
            tool_registry: registry,
            sidebar_entries,
            mcp_failures,
            resumed_grants,
        };

        (runtime, meta)
    }

    // ── Conversation access ─────────────────────────────────────────────

    pub fn conversation(&self) -> &Conversation {
        &self.conversation
    }

    pub fn conversation_mut(&mut self) -> &mut Conversation {
        &mut self.conversation
    }

    /// Authoritative conversation metadata that stays valid during a query.
    ///
    /// While a query task owns the real conversation, [`conversation`] returns
    /// an empty placeholder (`id=""`, `context_window=0`). Callers that only
    /// need id / context window / message count should read this snapshot
    /// instead so they never observe the empty shell.
    #[must_use]
    pub fn conversation_snapshot(&self) -> &ConversationSnapshot {
        &self.snapshot
    }

    /// Context window in tokens — valid even mid-query (unlike
    /// `conversation().context_window`, which is 0 while a query owns the
    /// conversation).
    #[must_use]
    pub fn context_window(&self) -> u64 {
        self.snapshot.context_window
    }

    /// Take ownership of the conversation (e.g. to move into a spawned task).
    ///
    /// The runtime's conversation is replaced with an empty placeholder, but
    /// [`conversation_snapshot`](Self::conversation_snapshot) keeps reflecting
    /// the taken conversation's metadata until it is restored. Call
    /// [`restore_conversation`](Self::restore_conversation) after the task
    /// completes.
    pub fn take_conversation(&mut self) -> Conversation {
        self.snapshot = ConversationSnapshot::from_conversation(&self.conversation);
        std::mem::take(&mut self.conversation)
    }

    pub fn restore_conversation(&mut self, conversation: Conversation) {
        self.snapshot = ConversationSnapshot::from_conversation(&conversation);
        self.conversation = conversation;
    }

    // ── Cron fired-prompt injection ─────────────────────────────────────

    /// Drain prompts fired by cron jobs since the last call, returning each
    /// prompt's text in fire order for the frontend to inject as a user turn.
    ///
    /// One-shot jobs (`recurring == false`) are deleted from the store after
    /// their fire is consumed here, so they run exactly once. Recurring jobs
    /// are left scheduled. Returns an empty vec when nothing fired.
    ///
    /// Intended to be called between turns (e.g. on an idle tick) while no
    /// query owns the conversation.
    pub async fn drain_due_cron_prompts(&self) -> Vec<String> {
        drain_and_consume_cron(&self.cron_fired_queue, &self.cron_store).await
    }

    /// Whether any cron prompts are waiting to be drained. Cheap, non-blocking;
    /// lets a frontend skip the async drain path on idle ticks with no work.
    #[must_use]
    pub fn has_pending_cron_prompts(&self) -> bool {
        !self.cron_fired_queue.is_empty()
    }

    // ── Query loop ──────────────────────────────────────────────────────

    /// Spawn a query-loop task and return a oneshot receiver for the result.
    ///
    /// The conversation is moved into the task and returned in
    /// [`QueryTaskResult`] when done. The caller must call
    /// [`restore_conversation`](Self::restore_conversation) with the
    /// returned conversation after awaiting the result.
    pub fn spawn_query(
        &mut self,
        backend: &Arc<crab_api::LlmBackend>,
        event_tx: mpsc::Sender<Event>,
        cancel: CancellationToken,
    ) -> tokio::sync::oneshot::Receiver<QueryTaskResult> {
        // Teammates need a real agent loop, but the runtime only sees the LLM
        // backend here (it is passed per query), so install the teammate
        // runner lazily before the first spawn can happen.
        {
            let runner_backend = Arc::clone(backend);
            let runner_registry = self.executor.registry_arc();
            let runner_ctx = self.tool_ctx.clone();
            let runner_config = self.loop_config.clone();
            let runner_tx = event_tx.clone();
            self.team_coordinator.ensure_runner(move || {
                crate::teams::spawn::teammate_runner(
                    runner_backend,
                    runner_registry,
                    runner_ctx,
                    runner_config,
                    runner_tx,
                )
            });
        }

        let mut task_conversation = self.take_conversation();
        let task_backend = Arc::clone(backend);
        let task_executor = Arc::clone(&self.executor);
        let task_ctx = self.tool_ctx.clone();
        let task_config = self.loop_config.clone();
        // In Coordinator Mode, sub-agent workers get a clean (overlay-free)
        // registry and prompt instead of the leader's stripped 3-tool registry.
        let coordinator = self.coordinator;
        let worker_base_prompt = self.worker_base_prompt.clone();
        let task_registry = Arc::clone(&self.task_registry);

        // Persist the user turn to the crash log before running: the engine
        // loop persists assistant + tool-result messages, but never the user
        // message that started the turn.
        if let Some(persister) = task_config.session_persister.as_ref()
            && let Some(last) = task_conversation.messages().last()
            && last.role == crab_core::message::Role::User
        {
            persister.persist_message(last);
        }

        let (return_tx, return_rx) = tokio::sync::oneshot::channel();

        tokio::spawn(async move {
            let mut task_cost = CostAccumulator::default();
            let starting_len = task_conversation.messages().len();
            let result = crab_engine::query_loop(
                &mut task_conversation,
                &task_backend,
                &task_executor,
                &task_ctx,
                &task_config,
                &mut task_cost,
                event_tx.clone(),
                cancel,
            )
            .await;

            // Run any sub-agents the turn requested via the Agent tool to
            // completion, folding each result back into the conversation.
            let markers = crate::teams::spawn::scan_spawn_markers(&task_conversation, starting_len);
            if !markers.is_empty() {
                let mut pool =
                    crate::teams::WorkerPool::new(task_conversation.id.clone(), "main".into());
                pool.set_task_registry(Arc::clone(&task_registry));
                // Coordinator workers get the clean registry/prompt; plain
                // sessions inherit the parent's.
                let (worker_registry, parent_prompt) = match coordinator {
                    Some(coord) => (
                        Arc::new(coord.build_worker_registry()),
                        worker_base_prompt.clone(),
                    ),
                    None => (
                        task_executor.registry_arc(),
                        task_conversation.system_prompt.clone(),
                    ),
                };
                let mut spawned = false;
                for marker in &markers {
                    if crate::teams::spawn::spawn_worker_from_marker(
                        &mut pool,
                        marker,
                        &task_backend,
                        Arc::clone(&worker_registry),
                        &parent_prompt,
                        &task_ctx,
                        &task_config,
                        &event_tx,
                    )
                    .is_some()
                    {
                        spawned = true;
                    }
                }
                if spawned {
                    for worker_result in pool.collect_all().await {
                        task_conversation
                            .push(crate::teams::spawn::agent_result_message(&worker_result));
                    }
                }
            }

            let _ = return_tx.send(QueryTaskResult {
                conversation: task_conversation,
                result,
                cost: task_cost,
            });
        });

        return_rx
    }

    // ── Team coordinator ────────────────────────────────────────────────

    /// Scan conversation tool results for team markers — named `spawn_agent`
    /// requests (which spawn teammates) and `message_sent` routing. Call
    /// after every completed query so the team browser reflects the latest
    /// model decisions.
    ///
    /// `starting_len` must be the conversation length *before* the query ran —
    /// only messages added during that query are inspected. Re-scanning older
    /// messages would respawn teammates and re-deliver messages.
    pub async fn process_team_markers(&mut self, starting_len: usize) {
        use crab_core::message::ContentBlock;
        let base_prompt = self.conversation.system_prompt.clone();
        let tail: Vec<String> = self
            .conversation
            .messages()
            .iter()
            .skip(starting_len)
            .flat_map(|m| m.content.iter())
            .filter_map(|block| match block {
                ContentBlock::ToolResult { content, .. } => Some(content.clone()),
                _ => None,
            })
            .collect();
        for payload in tail {
            if let Err(e) = self
                .team_coordinator
                .process_tool_result(&payload, &base_prompt)
                .await
            {
                tracing::warn!(error = %e, "team coordinator failed to spawn teammate");
            }
        }
    }

    /// Kill every teammate in the session's implicit team. Call once when
    /// the session ends — teammates live for the session's lifetime and have
    /// no other teardown path.
    pub async fn shutdown_teams(&mut self) {
        self.team_coordinator.shutdown_all().await;
    }

    /// Snapshot of the current team for the TUI team browser.
    ///
    /// Reads from the in-process backend's live teammate list; this is a
    /// pull-on-open design, so callers just call it when opening the
    /// overlay.
    #[must_use]
    pub fn team_snapshot(&self) -> TeamSnapshot {
        use crab_team::TeammateBackend as _;
        let members = self
            .team_coordinator
            .backend()
            .list_teammates()
            .into_iter()
            .map(|t| TeamMemberSnapshot {
                name: t.name.clone(),
                role: t.role.clone(),
                state: t.state.to_string(),
            })
            .collect();
        TeamSnapshot { members }
    }

    // ── Manual compaction ───────────────────────────────────────────────

    /// Result of a `/compact` invocation, used by the TUI to render the
    /// compact-boundary cell without needing an engine-side event round-trip.
    ///
    /// `strategy` is a short label (e.g. `"llm-summarize"`,
    /// `"heuristic-summarizer"`) suitable for direct display.
    pub async fn compact_now(&mut self) -> CompactNowResult {
        let before_tokens = self.conversation.estimated_tokens();
        let before_count = self.conversation.len();

        // Prefer the LLM-driven path when a backend is available.
        if let Some(client) = self.compaction_client.as_deref() {
            match compact_with_config(&mut self.conversation, &self.compaction_config, client).await
            {
                Ok(report) => {
                    self.snapshot = ConversationSnapshot::from_conversation(&self.conversation);
                    let strategy = format!("llm-{:?}", report.strategy_used).to_lowercase();
                    return CompactNowResult {
                        before_tokens: report.tokens_before,
                        after_tokens: report.tokens_after,
                        removed_messages: report.messages_removed(),
                        strategy,
                    };
                }
                Err(e) => {
                    tracing::warn!(error = %e, "LLM compaction failed; falling back to heuristic");
                }
            }
        }

        // Fallback: deterministic heuristic summariser, no network calls.
        let summary = crate::summarizer::summarize_conversation(
            self.conversation.messages(),
            &crate::summarizer::SummarizerConfig::default(),
        );
        let summary_text = summary.to_compact_text();

        self.conversation.clear();
        if !summary_text.is_empty() {
            let prefix = crate::COMPACT_HEURISTIC_USER_PREFIX;
            self.conversation
                .push_user(format!("{prefix}\n\n{summary_text}"));
        }

        self.snapshot = ConversationSnapshot::from_conversation(&self.conversation);
        let after_tokens = self.conversation.estimated_tokens();
        let removed = before_count.saturating_sub(self.conversation.len());
        CompactNowResult {
            before_tokens,
            after_tokens,
            removed_messages: removed,
            strategy: "heuristic-summarizer".to_string(),
        }
    }

    // ── Accessors ──────────────────────────────────────────────────────

    #[must_use]
    pub fn default_shell(&self) -> crab_config::DefaultShell {
        self.default_shell
    }

    pub fn memory_dir(&self) -> Option<&Path> {
        self.memory_dir.as_deref()
    }

    /// Access the skill registry for external lookups.
    pub fn skill_registry(&self) -> &SkillRegistry {
        &self.skill_registry
    }

    /// Re-discover skills from the stored skill directories.
    pub fn reload_skills(&mut self) -> usize {
        let mut new_registry = SkillRegistry::new();
        new_registry.register_all(crab_skills::builtin::builtin_skills());
        match SkillRegistry::discover(&self.skill_dirs) {
            Ok(disk) => {
                for skill in disk.list() {
                    new_registry.register(skill.clone());
                }
            }
            Err(e) => {
                tracing::warn!("failed to reload skills: {e}");
            }
        }
        let count = new_registry.len();
        self.skill_registry = new_registry;
        count
    }

    // ── Settings ────────────────────────────────────────────────────────

    pub fn loop_config(&self) -> &QueryConfig {
        &self.loop_config
    }

    pub fn loop_config_mut(&mut self) -> &mut QueryConfig {
        &mut self.loop_config
    }

    pub fn tool_ctx(&self) -> &ToolContext {
        &self.tool_ctx
    }

    pub fn tool_ctx_mut(&mut self) -> &mut ToolContext {
        &mut self.tool_ctx
    }

    /// Shared background task registry. Tools use the copy in `tool_ctx`;
    /// the runtime uses this one for worker registration.
    #[must_use]
    pub fn task_registry(&self) -> &Arc<std::sync::Mutex<crab_core::task::TaskRegistry>> {
        &self.task_registry
    }

    pub fn executor(&self) -> &Arc<ToolExecutor> {
        &self.executor
    }

    // ── Cost tracking ───────────────────────────────────────────────────

    pub fn cost(&self) -> &CostAccumulator {
        &self.cost
    }

    pub fn merge_cost(&mut self, other: &CostAccumulator) {
        self.cost.merge(other);
    }

    // ── Lifecycle hooks ─────────────────────────────────────────────────

    /// Fire a lifecycle hook in the background (fire-and-forget).
    pub fn fire_lifecycle_hook(
        &self,
        trigger: crab_hooks::HookTrigger,
        session_id: Option<&str>,
        working_dir: Option<&Path>,
    ) {
        let Some(hooks) = self.loop_config.hook_executor.clone() else {
            return;
        };
        let ctx = crab_hooks::HookContext {
            tool_name: String::new(),
            tool_input: String::new(),
            working_dir: working_dir.map(PathBuf::from),
            tool_output: None,
            tool_exit_code: None,
            session_id: session_id.map(String::from),
            transcript_path: None,
        };
        tokio::spawn(async move {
            if let Err(e) = hooks.run(trigger, &ctx).await {
                tracing::warn!(?trigger, error = %e, "lifecycle hook failed");
            }
        });
    }

    /// Fire [`HookTrigger::FileChanged`] in the background, passing the
    /// changed path through `CRAB_TOOL_INPUT` so hooks can act on it.
    pub fn fire_file_changed_hook(
        &self,
        path: &Path,
        session_id: Option<&str>,
        working_dir: Option<&Path>,
    ) {
        let Some(hooks) = self.loop_config.hook_executor.clone() else {
            return;
        };
        let ctx = crab_hooks::HookContext {
            tool_name: String::new(),
            tool_input: path.to_string_lossy().into_owned(),
            working_dir: working_dir.map(PathBuf::from),
            tool_output: None,
            tool_exit_code: None,
            session_id: session_id.map(String::from),
            transcript_path: None,
        };
        tokio::spawn(async move {
            if let Err(e) = hooks.run(crab_hooks::HookTrigger::FileChanged, &ctx).await {
                tracing::warn!(error = %e, "file_changed hook failed");
            }
        });
    }

    /// Fire the `ConfigChange` hook after a settings reload (fire-and-forget).
    pub fn fire_config_change_hook(&self, session_id: Option<&str>, working_dir: Option<&Path>) {
        let Some(hooks) = self.loop_config.hook_executor.clone() else {
            return;
        };
        let ctx = crab_hooks::HookContext {
            working_dir: working_dir.map(PathBuf::from),
            session_id: session_id.map(String::from),
            ..crab_hooks::HookContext::default()
        };
        tokio::spawn(async move {
            if let Err(e) = hooks.run(crab_hooks::HookTrigger::ConfigChange, &ctx).await {
                tracing::warn!(error = %e, "config_change hook failed");
            }
        });
    }

    /// Build a fire-and-forget sink for the `Notification` hook.
    ///
    /// Returns `None` when no `HookExecutor` is configured, so the caller
    /// can skip wiring the callback entirely. Otherwise the returned
    /// closure captures a cloned `Arc<HookExecutor>` and session id; each
    /// call spawns a detached task that runs the hook with the message
    /// passed through `CRAB_TOOL_INPUT`.
    ///
    /// This is the hook-side dual of
    /// [`NotificationManager::set_on_push`](../../crab_tui/components/notification/struct.NotificationManager.html) —
    /// the UI component stays ignorant of `HookExecutor` while the runtime
    /// decides whether hooks run at all.
    #[must_use]
    pub fn notification_hook_sink(&self) -> Option<NotificationHookSink> {
        let hooks = self.loop_config.hook_executor.clone()?;
        let session_id = self.loop_config.session_id.clone();
        Some(std::sync::Arc::new(move |msg: &str| {
            let hooks = hooks.clone();
            let message = msg.to_string();
            let session_id = session_id.clone();
            tokio::spawn(async move {
                let ctx = crab_hooks::HookContext {
                    tool_name: String::new(),
                    tool_input: message,
                    working_dir: None,
                    tool_output: None,
                    tool_exit_code: None,
                    session_id,
                    transcript_path: None,
                };
                if let Err(e) = hooks.run(crab_hooks::HookTrigger::Notification, &ctx).await {
                    tracing::warn!(error = %e, "notification hook failed");
                }
            });
        }))
    }

    // ── Session persistence ─────────────────────────────────────────────

    /// Persist the current conversation, recording the working directory so
    /// `--continue` can stay within this project. The on-disk session name is
    /// preserved; grants are taken from `grants` when supplied (and adopted as
    /// the runtime's authoritative set), otherwise the runtime's in-memory
    /// grant set is used so auto-save never wipes the user's "always allow" set
    /// and never re-reads disk (which a concurrent process could be rewriting).
    fn persist_session(&self, session_id: &str, grants: Option<&[String]>) {
        let Some(history) = self.session_history.as_ref() else {
            return;
        };
        let working_dir = self.tool_ctx.working_dir.to_str();
        let name = history.load_name(session_id);
        let grants = match grants {
            Some(g) => {
                if let Ok(mut store) = self.session_grants.lock() {
                    *store = g.to_vec();
                }
                g.to_vec()
            }
            None => self
                .session_grants
                .lock()
                .map(|g| g.clone())
                .unwrap_or_default(),
        };
        if let Err(e) = history.save_with_metadata(
            session_id,
            name.as_deref(),
            working_dir,
            self.conversation.messages(),
            &grants,
        ) {
            tracing::warn!(error = %e, "session save failed");
            return;
        }
        // The snapshot now contains everything; reset the crash log so it only
        // holds messages from the next (in-flight) turn.
        if let Err(e) = history.truncate_jsonl(session_id) {
            tracing::warn!(error = %e, "session crash-log truncate failed");
        }
        // Persist accumulated cost so resume restores the running total.
        if let Err(e) = history.save_cost(session_id, &self.cost.summary()) {
            tracing::warn!(error = %e, "session cost save failed");
        }
    }

    pub fn save_session(&self, session_id: &str) {
        self.persist_session(session_id, None);
    }

    /// Save the current conversation along with session-level grants.
    ///
    /// Used by the TUI runner so the user's "always allow" decisions
    /// survive `/exit` and resume.
    pub fn save_session_with_grants(&self, session_id: &str, grants: &[String]) {
        self.persist_session(session_id, Some(grants));
    }

    pub fn session_history(&self) -> Option<&SessionHistory> {
        self.session_history.as_deref()
    }

    /// Load only the grants for a session id (e.g. for `--continue` /
    /// startup rehydration where the conversation was loaded via a
    /// different path). Returns an empty `Vec` if there are none or no
    /// session history is configured.
    pub fn load_session_grants(&self, session_id: &str) -> Vec<String> {
        self.session_history
            .as_ref()
            .and_then(|h| h.load_grants(session_id).ok())
            .unwrap_or_default()
    }

    /// Reset conversation for a new session.
    pub fn new_session(&mut self, session_id: &str) {
        self.conversation = Conversation::new(
            session_id.to_string(),
            self.conversation.system_prompt.clone(),
            self.conversation.context_window,
        );
        self.snapshot = ConversationSnapshot::from_conversation(&self.conversation);
    }

    /// Switch to a different session by loading its messages.
    ///
    /// Returns `true` if the session was found and loaded.
    pub fn switch_session(&mut self, session_id: &str, target_id: &str) -> bool {
        let Some(ref history) = self.session_history else {
            return false;
        };
        self.persist_session(session_id, None);
        match history.load(target_id) {
            Ok(Some(messages)) => {
                self.conversation = Conversation::new(
                    target_id.to_string(),
                    self.conversation.system_prompt.clone(),
                    self.conversation.context_window,
                );
                for msg in crab_session::sanitize_for_resume(messages) {
                    self.conversation.push(msg);
                }
                self.snapshot = ConversationSnapshot::from_conversation(&self.conversation);
                // Adopt the target session's on-disk grants as authoritative so
                // the outgoing session's grants don't leak into this one.
                if let Ok(mut store) = self.session_grants.lock() {
                    *store = history.load_grants(target_id).unwrap_or_default();
                }
                true
            }
            _ => false,
        }
    }

    /// Switch sessions while preserving session-level grants.
    ///
    /// Saves the outgoing session's grants alongside its messages, then
    /// loads both messages and grants for the target. Returns the loaded
    /// grants on success so the TUI can rehydrate its in-memory set.
    /// Returns `None` if the target session does not exist.
    pub fn switch_session_with_grants(
        &mut self,
        session_id: &str,
        outgoing_grants: &[String],
        target_id: &str,
    ) -> Option<Vec<String>> {
        let history = self.session_history.as_ref()?;
        self.persist_session(session_id, Some(outgoing_grants));
        match history.load_with_grants(target_id) {
            Ok(Some((messages, grants))) => {
                self.conversation = Conversation::new(
                    target_id.to_string(),
                    self.conversation.system_prompt.clone(),
                    self.conversation.context_window,
                );
                for msg in crab_session::sanitize_for_resume(messages) {
                    self.conversation.push(msg);
                }
                self.snapshot = ConversationSnapshot::from_conversation(&self.conversation);
                if let Ok(mut store) = self.session_grants.lock() {
                    store.clone_from(&grants);
                }
                Some(grants)
            }
            _ => None,
        }
    }

    // ── File history / rewind ───────────────────────────────────────────

    /// Restore tracked file(s) from the session's file-history.
    ///
    /// When `path` is `Some`, only that file is rewound to its most recent
    /// snapshot. When `None`, every file with at least one snapshot is
    /// rewound to its latest version. Returns the list of paths that were
    /// successfully restored (as display strings).
    pub fn rewind(&self, path: Option<&str>) -> crab_core::Result<Vec<String>> {
        let fh = self
            .file_history
            .as_ref()
            .ok_or_else(|| crab_core::Error::Other("file history not available".into()))?;
        let history = fh
            .lock()
            .map_err(|e| crab_core::Error::Other(format!("file history mutex poisoned: {e}")))?;

        let mut restored = Vec::new();
        if let Some(p) = path {
            let abs = PathBuf::from(p);
            history
                .rewind_to_latest(&abs)
                .map_err(|e| crab_core::Error::Other(e.to_string()))?;
            restored.push(p.to_string());
        } else {
            for tracked in history.tracked_files() {
                if history.rewind_to_latest(&tracked).is_ok() {
                    restored.push(tracked.display().to_string());
                }
            }
        }
        Ok(restored)
    }

    // ── Input expansion ─────────────────────────────────────────────────

    /// Expand `@file` mentions in user input.
    pub fn expand_input(&self, input: &str) -> crab_core::message::Message {
        expand_at_mentions(input, &self.tool_ctx.working_dir)
    }

    /// Get the cancellation token from the tool context.
    pub fn cancellation_token(&self) -> &CancellationToken {
        &self.tool_ctx.cancellation_token
    }
}

/// Build a [`crab_hooks::HookExecutor`] from the raw settings `hooks` JSON.
///
/// Returns `None` when hooks are disabled, the value is absent, or it
/// contains no usable hook definitions. The on-disk schema is the CC hooks
/// shape parsed by `crab_config::hooks::parse_hooks`; each flattened
/// `Hook` maps 1:1 onto the engine-facing `crab_hooks::HookDef`.
pub fn build_hook_executor(
    hooks: Option<&serde_json::Value>,
    disable_hooks: bool,
) -> Option<Arc<crab_hooks::HookExecutor>> {
    if disable_hooks {
        return None;
    }
    let value = hooks?;
    let parsed = match crab_config::hooks::parse_hooks(value) {
        Ok(parsed) => parsed,
        Err(e) => {
            tracing::warn!(error = %e, "failed to parse hooks from settings; hooks disabled");
            return None;
        }
    };
    if parsed.is_empty() {
        return None;
    }
    let defs: Vec<crab_hooks::HookDef> = parsed
        .into_iter()
        .map(|h| crab_hooks::HookDef {
            trigger: h.trigger,
            command: h.command,
            timeout_secs: h.timeout_secs,
            matcher: h.matcher,
        })
        .collect();
    Some(Arc::new(crab_hooks::HookExecutor::with_hooks(defs)))
}

/// Permission response payload routed from the TUI: `(allowed, feedback)`.
type PermissionResponse = (bool, Option<String>);

/// Map of in-flight permission requests, keyed by request id, each holding a
/// `oneshot::Sender` that delivers exactly that request's response.
type PendingPermissions =
    Arc<tokio::sync::Mutex<std::collections::HashMap<String, oneshot::Sender<PermissionResponse>>>>;

/// Channel-based permission handler wired to the TUI event system.
///
/// When a tool needs permission, sends `Event::PermissionRequest` through the
/// event channel and waits on a per-request `oneshot`. A single dispatcher
/// task (spawned in [`AgentRuntime::init`]) owns the shared response receiver
/// and routes each `(request_id, allowed, feedback)` to the matching pending
/// request. This prevents the response-stealing hang that a single shared
/// receiver allowed: two concurrent asks (e.g. a teammate worker and the main
/// loop) can no longer consume each other's responses.
struct ChannelPermissionHandler {
    event_tx: mpsc::Sender<Event>,
    pending: PendingPermissions,
}

impl ChannelPermissionHandler {
    /// Build the handler and spawn the dispatcher task that fans responses
    /// out to per-request oneshots.
    fn new(
        event_tx: mpsc::Sender<Event>,
        mut response_rx: mpsc::UnboundedReceiver<(String, bool, Option<String>)>,
    ) -> Self {
        let pending: PendingPermissions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        let dispatch_pending = Arc::clone(&pending);
        tokio::spawn(async move {
            while let Some((id, allowed, feedback)) = response_rx.recv().await {
                let sender = dispatch_pending.lock().await.remove(&id);
                if let Some(sender) = sender {
                    let _ = sender.send((allowed, feedback));
                } else {
                    tracing::debug!(
                        request_id = %id,
                        "permission response for unknown/expired request dropped"
                    );
                }
            }
        });
        Self { event_tx, pending }
    }
}

impl PermissionHandler for ChannelPermissionHandler {
    fn ask_permission(
        &self,
        tool_name: &str,
        prompt: &str,
        tool_input: &serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = PermissionResult> + Send + '_>> {
        let tool_name = tool_name.to_string();
        let prompt = prompt.to_string();
        let tool_input = tool_input.clone();
        let request_id = crab_utils::id::new_ulid();
        let event_tx = self.event_tx.clone();
        let pending = Arc::clone(&self.pending);

        Box::pin(async move {
            let (tx, rx) = oneshot::channel::<PermissionResponse>();
            pending.lock().await.insert(request_id.clone(), tx);

            if event_tx
                .send(Event::PermissionRequest {
                    tool_name,
                    input_summary: prompt,
                    request_id: request_id.clone(),
                    tool_input,
                })
                .await
                .is_err()
            {
                pending.lock().await.remove(&request_id);
                return PermissionResult::deny();
            }

            // Dispatcher drops the sender if the TUI channel closes — deny then.
            if let Ok((allowed, feedback)) = rx.await {
                PermissionResult { allowed, feedback }
            } else {
                pending.lock().await.remove(&request_id);
                PermissionResult::deny()
            }
        })
    }
}

/// Drain the cron fired-prompt queue, deleting one-shot jobs once their fire is
/// consumed, and return the prompts in fire order.
///
/// Factored out of [`AgentRuntime::drain_due_cron_prompts`] so the
/// drain-and-consume contract can be unit-tested against a bare store and queue
/// without standing up a full runtime.
async fn drain_and_consume_cron(queue: &FiredPromptQueue, store: &SharedCronStore) -> Vec<String> {
    let fired = queue.drain();
    if fired.is_empty() {
        return Vec::new();
    }

    let mut store = store.lock().await;
    for fire in &fired {
        // Delete one-shot jobs once consumed. A recurring job, or one already
        // deleted, leaves the store untouched.
        if store.get(&fire.job_id).is_some_and(|job| !job.recurring) {
            store.delete(&fire.job_id).await;
        }
    }
    drop(store);

    fired.into_iter().map(|fire| fire.prompt).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::permission::PermissionPolicy;
    use crab_tools::builtin::cron::{CronStore, FiredPrompt};

    fn test_session_config() -> SessionConfig {
        SessionConfig {
            session_id: "test-session".into(),
            system_prompt: "You are helpful.".into(),
            model: ModelId::from("claude-sonnet-4-20250514"),
            max_tokens: 4096,
            temperature: None,
            context_window: 200_000,
            working_dir: std::env::temp_dir(),
            permission_policy: PermissionPolicy::default(),
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
            default_shell: "bash".into(),
        }
    }

    fn init_config(hooks: Option<serde_json::Value>, disable_hooks: bool) -> RuntimeInitConfig {
        let (perm_event_tx, _perm_event_rx) = mpsc::channel(8);
        let (_perm_resp_tx, perm_resp_rx) = mpsc::unbounded_channel();
        RuntimeInitConfig {
            session_config: test_session_config(),
            mcp_servers: None,
            skill_dirs: Vec::new(),
            perm_event_tx,
            perm_resp_rx,
            backend: None,
            hooks,
            disable_hooks,
            cache_enabled: true,
        }
    }

    #[test]
    fn build_hook_executor_none_when_disabled() {
        let hooks = serde_json::json!([{"trigger": "pre_tool_use", "command": "echo hi"}]);
        assert!(build_hook_executor(Some(&hooks), true).is_none());
    }

    #[test]
    fn build_hook_executor_none_when_absent_or_empty() {
        assert!(build_hook_executor(None, false).is_none());
        let empty = serde_json::json!([]);
        assert!(build_hook_executor(Some(&empty), false).is_none());
    }

    #[test]
    fn build_hook_executor_maps_config_hooks() {
        let hooks = serde_json::json!({
            "PreToolUse": [
                {"matcher": "bash", "hooks": [{"type": "command", "command": "check", "timeout": 3}]}
            ],
            "Stop": [
                {"hooks": [{"type": "command", "command": "echo done"}]}
            ]
        });
        let exec = build_hook_executor(Some(&hooks), false).expect("executor built");
        assert_eq!(exec.len(), 2);
    }

    #[tokio::test]
    async fn assembly_wires_hooks_cache_and_retry() {
        let hooks = serde_json::json!({
            "PreToolUse": [{"hooks": [{"type": "command", "command": "echo hi"}]}]
        });
        let (runtime, _meta) = AgentRuntime::init(init_config(Some(hooks), false)).await;
        let cfg = runtime.loop_config();
        assert!(cfg.hook_executor.is_some(), "hooks should be wired");
        assert!(cfg.cache_enabled, "prompt caching should be enabled");
        assert!(cfg.retry_policy.is_some(), "retry policy should be set");
    }

    #[tokio::test]
    async fn assembly_omits_hooks_when_disabled() {
        let hooks = serde_json::json!({
            "PreToolUse": [{"hooks": [{"type": "command", "command": "echo hi"}]}]
        });
        let (runtime, _meta) = AgentRuntime::init(init_config(Some(hooks), true)).await;
        assert!(runtime.loop_config().hook_executor.is_none());
    }

    #[tokio::test]
    async fn snapshot_survives_take_conversation() {
        let (mut runtime, _meta) = AgentRuntime::init(init_config(None, false)).await;
        let window = runtime.context_window();
        assert_eq!(window, 200_000);

        let conv = runtime.take_conversation();
        // While the conversation is taken, the live one is an empty placeholder.
        assert_eq!(runtime.conversation().context_window, 0);
        // But the snapshot still reports the authoritative window and id.
        assert_eq!(runtime.context_window(), 200_000);
        assert_eq!(runtime.conversation_snapshot().id, "test-session");

        runtime.restore_conversation(conv);
        assert_eq!(runtime.context_window(), 200_000);
    }

    // ── Cron fired-prompt drain ─────────────────────────────────────────

    /// Build a store + queue pair, seed two jobs (one recurring, one one-shot),
    /// and fire both onto the queue.
    async fn seeded_cron() -> (FiredPromptQueue, SharedCronStore) {
        let mut store = CronStore::new();
        let queue = store.fired_queue();
        // Use a per-minute expression so the scheduler does not fire on its own
        // during the test; we push the fires manually.
        store
            .create("0 * * * *".into(), "recurring prompt".into(), true, false)
            .await
            .unwrap();
        store
            .create("0 * * * *".into(), "one-shot prompt".into(), false, false)
            .await
            .unwrap();
        queue.push(FiredPrompt {
            job_id: "cron_1".into(),
            prompt: "recurring prompt".into(),
        });
        queue.push(FiredPrompt {
            job_id: "cron_2".into(),
            prompt: "one-shot prompt".into(),
        });
        let shared: SharedCronStore = Arc::new(tokio::sync::Mutex::new(store));
        (queue, shared)
    }

    #[tokio::test]
    async fn drain_returns_prompts_in_fire_order() {
        let (queue, store) = seeded_cron().await;
        let prompts = drain_and_consume_cron(&queue, &store).await;
        assert_eq!(prompts, vec!["recurring prompt", "one-shot prompt"]);
        assert!(queue.is_empty(), "queue is emptied by the drain");
    }

    #[tokio::test]
    async fn drain_deletes_one_shot_keeps_recurring() {
        let (queue, store) = seeded_cron().await;
        drain_and_consume_cron(&queue, &store).await;
        let jobs = store.lock().await.list();
        // The one-shot job is gone; the recurring job remains scheduled.
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, "cron_1");
        assert!(jobs[0].recurring);
    }

    #[tokio::test]
    async fn drain_empty_queue_is_noop() {
        let store: SharedCronStore = Arc::new(tokio::sync::Mutex::new(CronStore::new()));
        let queue = store.lock().await.fired_queue();
        let prompts = drain_and_consume_cron(&queue, &store).await;
        assert!(prompts.is_empty());
    }

    #[tokio::test]
    async fn runtime_exposes_cron_drain() {
        // The wired runtime starts with an empty cron queue and reports so.
        let (runtime, _meta) = AgentRuntime::init(init_config(None, false)).await;
        assert!(!runtime.has_pending_cron_prompts());
        assert!(runtime.drain_due_cron_prompts().await.is_empty());
    }
}
