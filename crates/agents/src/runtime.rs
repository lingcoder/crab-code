//! [`AgentRuntime`] — high-level facade that owns all L2 service state
//! and exposes a minimal API for the TUI layer.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::mpsc;
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
use crab_tools::builtin::create_default_registry;
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
    executor: Arc<ToolExecutor>,
    tool_ctx: ToolContext,
    loop_config: QueryConfig,
    skill_registry: SkillRegistry,
    session_history: Option<SessionHistory>,
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
        let mut registry = create_default_registry();

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

        let registry = Arc::new(registry);
        let tool_schemas = registry.tool_schemas();
        let mut executor = ToolExecutor::new(Arc::clone(&registry));

        executor.set_permission_handler(Arc::new(ChannelPermissionHandler {
            event_tx: config.perm_event_tx,
            response_rx: Arc::new(tokio::sync::Mutex::new(config.perm_resp_rx)),
        }));
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

        let resumed_grants = config
            .session_config
            .resume_session_id
            .as_ref()
            .zip(session_history.as_ref())
            .and_then(|(resume_id, history)| history.load_with_grants(resume_id).ok().flatten())
            .map(|(messages, grants)| {
                for msg in messages {
                    conversation.push(msg);
                }
                grants
            })
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
        let file_history = Arc::new(std::sync::Mutex::new(FileHistory::new(
            file_history_base,
            &session_id,
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

        let tool_ctx = ToolContext {
            working_dir: config.session_config.working_dir,
            permission_mode: config.session_config.permission_policy.mode,
            session_id: session_id.clone(),
            cancellation_token: CancellationToken::new(),
            permission_policy: config.session_config.permission_policy,
            ext: crab_core::tool::ToolContextExt {
                track_edit: Some(track_edit),
                ..Default::default()
            },
        };

        let compaction_config = CompactionConfig::default();
        let compaction_client: Option<Arc<dyn CompactionClient>> =
            config.backend.as_ref().map(|backend| {
                let client = crate::llm_compaction_client::LlmCompactionClient::new(
                    Arc::clone(backend),
                    config.session_config.model.clone(),
                );
                Arc::new(client) as Arc<dyn CompactionClient>
            });

        let loop_config = QueryConfig {
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
            fallback_model: config.session_config.fallback_model.map(ModelId::from),
            plan_model: None,
            source: crab_core::query::QuerySource::Repl,
            compaction_client: compaction_client.clone(),
            compaction_config: compaction_config.clone(),
            session_persister: None,
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

        let runtime = Self {
            conversation,
            executor,
            tool_ctx,
            loop_config,
            skill_registry,
            session_history,
            _mcp_manager: mcp_manager,
            cost: CostAccumulator::default(),
            memory_dir,
            team_coordinator: crate::teams::coordinator::TeamCoordinator::new(),
            compaction_client,
            compaction_config,
            file_history: Some(file_history),
            default_shell,
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

    /// Take ownership of the conversation (e.g. to move into a spawned task).
    ///
    /// The runtime's conversation is replaced with an empty placeholder.
    /// Call [`restore_conversation`](Self::restore_conversation) after the
    /// task completes.
    pub fn take_conversation(&mut self) -> Conversation {
        std::mem::take(&mut self.conversation)
    }

    pub fn restore_conversation(&mut self, conversation: Conversation) {
        self.conversation = conversation;
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
        let mut task_conversation = self.take_conversation();
        let task_backend = Arc::clone(backend);
        let task_executor = Arc::clone(&self.executor);
        let task_ctx = self.tool_ctx.clone();
        let task_config = self.loop_config.clone();

        let (return_tx, return_rx) = tokio::sync::oneshot::channel();

        tokio::spawn(async move {
            let mut task_cost = CostAccumulator::default();
            let result = crab_engine::query_loop(
                &mut task_conversation,
                &task_backend,
                &task_executor,
                &task_ctx,
                &task_config,
                &mut task_cost,
                event_tx,
                cancel,
            )
            .await;

            let _ = return_tx.send(QueryTaskResult {
                conversation: task_conversation,
                result,
                cost: task_cost,
            });
        });

        return_rx
    }

    // ── Team coordinator ────────────────────────────────────────────────

    /// Scan conversation tool results for the `team_created` marker and
    /// spawn any newly-seen teams. Call after every completed query so the
    /// team browser reflects the latest model decisions.
    ///
    /// `starting_len` is the conversation length *before* the query ran —
    /// only messages added during that query are inspected, so repeated
    /// calls are idempotent.
    pub async fn process_team_markers(&mut self, starting_len: usize) {
        use crab_core::message::ContentBlock;
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
            if let Err(e) = self.team_coordinator.process_tool_result(&payload).await {
                tracing::warn!(error = %e, "team coordinator failed to spawn teammate");
            }
        }
    }

    /// Snapshot of the current team for the TUI team browser.
    ///
    /// Reads from the in-process backend's live teammate list; this is a
    /// pull-on-open design, so callers just call it when opening the
    /// overlay.
    #[must_use]
    pub fn team_snapshot(&self) -> TeamSnapshot {
        use crab_swarm::SwarmBackend as _;
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
            self.conversation
                .push_user(format!("[Previous conversation summary]\n\n{summary_text}"));
        }

        let after_tokens = self.conversation.estimated_tokens();
        let removed = before_count.saturating_sub(self.conversation.len());
        CompactNowResult {
            before_tokens,
            after_tokens,
            removed_messages: removed,
            strategy: "heuristic-summarizer".to_string(),
        }
    }

    // ── Slash command dispatch ──────────────────────────────────────────

    /// Route a `/command` through the built-in registry and skills.
    ///
    /// Priority order:
    /// Access the memory directory, if configured.
    /// Resolved shell choice for the TUI's `!` prefix. The TUI consults
    /// this together with the live tool registry to decide whether to
    /// route to `Bash` or `PowerShell`.
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

    /// Re-discover skills from the given directories.
    pub fn reload_skills(&mut self, skill_dirs: &[PathBuf]) -> usize {
        let mut new_registry = SkillRegistry::new();
        new_registry.register_all(crab_skills::builtin::builtin_skills());
        match SkillRegistry::discover(skill_dirs) {
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
        };
        tokio::spawn(async move {
            if let Err(e) = hooks.run(crab_hooks::HookTrigger::FileChanged, &ctx).await {
                tracing::warn!(error = %e, "file_changed hook failed");
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
                };
                if let Err(e) = hooks.run(crab_hooks::HookTrigger::Notification, &ctx).await {
                    tracing::warn!(error = %e, "notification hook failed");
                }
            });
        }))
    }

    // ── Session persistence ─────────────────────────────────────────────

    pub fn save_session(&self, session_id: &str) {
        if let Some(ref history) = self.session_history
            && let Err(e) = history.save(session_id, self.conversation.messages())
        {
            tracing::warn!(error = %e, "session save failed");
        }
    }

    /// Save the current conversation along with session-level grants.
    ///
    /// Used by the TUI runner so the user's "always allow" decisions
    /// survive `/exit` and resume.
    pub fn save_session_with_grants(&self, session_id: &str, grants: &[String]) {
        if let Some(ref history) = self.session_history
            && let Err(e) =
                history.save_with_grants(session_id, self.conversation.messages(), grants)
        {
            tracing::warn!(error = %e, "session save (with grants) failed");
        }
    }

    pub fn session_history(&self) -> Option<&SessionHistory> {
        self.session_history.as_ref()
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
    }

    /// Switch to a different session by loading its messages.
    ///
    /// Returns `true` if the session was found and loaded.
    pub fn switch_session(&mut self, session_id: &str, target_id: &str) -> bool {
        let Some(ref history) = self.session_history else {
            return false;
        };
        let _ = history.save(session_id, self.conversation.messages());
        match history.load(target_id) {
            Ok(Some(messages)) => {
                self.conversation = Conversation::new(
                    target_id.to_string(),
                    self.conversation.system_prompt.clone(),
                    self.conversation.context_window,
                );
                for msg in messages {
                    self.conversation.push(msg);
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
        let _ = history.save_with_grants(session_id, self.conversation.messages(), outgoing_grants);
        match history.load_with_grants(target_id) {
            Ok(Some((messages, grants))) => {
                self.conversation = Conversation::new(
                    target_id.to_string(),
                    self.conversation.system_prompt.clone(),
                    self.conversation.context_window,
                );
                for msg in messages {
                    self.conversation.push(msg);
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

/// Channel-based permission handler wired to the TUI event system.
///
/// When a tool needs permission, sends `Event::PermissionRequest` through
/// the channel and waits for a response. The response carries an optional
/// free-text feedback note that the executor surfaces back to the model.
type PermissionResponseRx = mpsc::UnboundedReceiver<(String, bool, Option<String>)>;

struct ChannelPermissionHandler {
    event_tx: mpsc::Sender<Event>,
    response_rx: Arc<tokio::sync::Mutex<PermissionResponseRx>>,
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
        let response_rx = self.response_rx.clone();

        Box::pin(async move {
            let _ = event_tx
                .send(Event::PermissionRequest {
                    tool_name,
                    input_summary: prompt,
                    request_id: request_id.clone(),
                    tool_input,
                })
                .await;

            let mut rx = response_rx.lock().await;
            while let Some((id, allowed, feedback)) = rx.recv().await {
                if id == request_id {
                    return PermissionResult { allowed, feedback };
                }
            }
            PermissionResult::deny()
        })
    }
}
