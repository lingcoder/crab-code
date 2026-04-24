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
    CompactionConfig, Conversation, CostAccumulator, MemoryStore, SessionHistory, SessionMetadata,
    expand_at_mentions,
};
use crab_skill::SkillRegistry;
use crab_tools::builtin::create_default_registry;
use crab_tools::executor::{PermissionHandler, ToolExecutor};
use crab_tools::registry::ToolRegistry;

use crate::SessionConfig;

/// Input configuration for [`AgentRuntime::init`].
pub struct RuntimeInitConfig {
    pub session_config: SessionConfig,
    pub mcp_servers: Option<serde_json::Value>,
    pub skill_dirs: Vec<PathBuf>,
    pub perm_event_tx: mpsc::Sender<Event>,
    pub perm_resp_rx: mpsc::UnboundedReceiver<(String, bool)>,
}

/// Data returned alongside an [`AgentRuntime`] from [`AgentRuntime::init`].
pub struct RuntimeInitMeta {
    pub tool_registry: Arc<ToolRegistry>,
    pub sidebar_entries: Vec<SessionMetadata>,
    pub mcp_failures: Vec<String>,
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
    _mcp_manager: Option<McpManager>,
    cost: CostAccumulator,
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
            let count =
                crab_tools::builtin::mcp_tool::register_mcp_tools(&mgr, &mut registry).await;
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

        if let Some(ref resume_id) = config.session_config.resume_session_id
            && let Some(ref history) = session_history
            && let Ok(Some(messages)) = history.load(resume_id)
        {
            for msg in messages {
                conversation.push(msg);
            }
        }

        let tool_ctx = ToolContext {
            working_dir: config.session_config.working_dir,
            permission_mode: config.session_config.permission_policy.mode,
            session_id: session_id.clone(),
            cancellation_token: CancellationToken::new(),
            permission_policy: config.session_config.permission_policy,
            ext: crab_core::tool::ToolContextExt::default(),
        };

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
            compaction_client: None,
            compaction_config: CompactionConfig::default(),
            session_persister: None,
        };

        let skill_registry = SkillRegistry::discover(&config.skill_dirs).unwrap_or_default();

        let sidebar_entries = session_history
            .as_ref()
            .and_then(|h| h.list_sessions_with_metadata().ok())
            .unwrap_or_default();

        let runtime = Self {
            conversation,
            executor,
            tool_ctx,
            loop_config,
            skill_registry,
            session_history,
            _mcp_manager: mcp_manager,
            cost: CostAccumulator::default(),
        };

        let meta = RuntimeInitMeta {
            tool_registry: registry,
            sidebar_entries,
            mcp_failures,
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

    // ── Skill resolution ────────────────────────────────────────────────

    /// Resolve a `/command` to its skill prompt, or pass through as-is.
    pub fn resolve_slash(&self, input: &str) -> String {
        let trimmed = input.trim();
        if !trimmed.starts_with('/') {
            return input.to_string();
        }

        let command = trimmed
            .trim_start_matches('/')
            .split_whitespace()
            .next()
            .unwrap_or("");

        if matches!(command, "exit" | "quit" | "help") {
            return input.to_string();
        }

        if let Some(skill) = self.skill_registry.find_command(command) {
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

    /// Access the skill registry for external lookups.
    pub fn skill_registry(&self) -> &SkillRegistry {
        &self.skill_registry
    }

    /// Re-discover skills from the given directories.
    pub fn reload_skills(&mut self, skill_dirs: &[PathBuf]) -> usize {
        match SkillRegistry::discover(skill_dirs) {
            Ok(new_registry) => {
                let count = new_registry.len();
                self.skill_registry = new_registry;
                count
            }
            Err(e) => {
                tracing::warn!("failed to reload skills: {e}");
                self.skill_registry.len()
            }
        }
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
        trigger: crab_plugin::hook::HookTrigger,
        session_id: Option<&str>,
        working_dir: Option<&Path>,
    ) {
        let Some(hooks) = self.loop_config.hook_executor.clone() else {
            return;
        };
        let ctx = crab_plugin::hook::HookContext {
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
                let ctx = crab_plugin::hook::HookContext {
                    tool_name: String::new(),
                    tool_input: message,
                    working_dir: None,
                    tool_output: None,
                    tool_exit_code: None,
                    session_id,
                };
                if let Err(e) = hooks
                    .run(crab_plugin::hook::HookTrigger::Notification, &ctx)
                    .await
                {
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

    pub fn session_history(&self) -> Option<&SessionHistory> {
        self.session_history.as_ref()
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
/// the channel and waits for a response.
struct ChannelPermissionHandler {
    event_tx: mpsc::Sender<Event>,
    response_rx: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<(String, bool)>>>,
}

impl PermissionHandler for ChannelPermissionHandler {
    fn ask_permission(
        &self,
        tool_name: &str,
        prompt: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + '_>> {
        let tool_name = tool_name.to_string();
        let prompt = prompt.to_string();
        let request_id = crab_core::common::utils::id::new_ulid();
        let event_tx = self.event_tx.clone();
        let response_rx = self.response_rx.clone();

        Box::pin(async move {
            let _ = event_tx
                .send(Event::PermissionRequest {
                    tool_name,
                    input_summary: prompt,
                    request_id: request_id.clone(),
                })
                .await;

            let mut rx = response_rx.lock().await;
            while let Some((id, allowed)) = rx.recv().await {
                if id == request_id {
                    return allowed;
                }
            }
            false
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_slash_passthrough() {
        let rt = make_test_runtime();
        assert_eq!(rt.resolve_slash("hello world"), "hello world");
    }

    #[test]
    fn resolve_slash_builtin() {
        let rt = make_test_runtime();
        assert_eq!(rt.resolve_slash("/exit"), "/exit");
        assert_eq!(rt.resolve_slash("/quit"), "/quit");
        assert_eq!(rt.resolve_slash("/help"), "/help");
    }

    #[test]
    fn resolve_slash_no_match() {
        let rt = make_test_runtime();
        assert_eq!(rt.resolve_slash("/unknown"), "/unknown");
    }

    #[test]
    fn resolve_slash_matches_skill() {
        let mut rt = make_test_runtime();
        rt.skill_registry.register(crab_skill::Skill {
            trigger: crab_skill::SkillTrigger::Command {
                name: "commit".into(),
            },
            ..crab_skill::Skill::new("commit", "You are a commit helper.")
        });

        let result = rt.resolve_slash("/commit");
        assert_eq!(result, "You are a commit helper.");
    }

    #[test]
    fn resolve_slash_with_args() {
        let mut rt = make_test_runtime();
        rt.skill_registry.register(crab_skill::Skill {
            trigger: crab_skill::SkillTrigger::Command {
                name: "review".into(),
            },
            ..crab_skill::Skill::new("review", "Review the code.")
        });

        let result = rt.resolve_slash("/review src/main.rs");
        assert!(result.contains("Review the code."));
        assert!(result.contains("src/main.rs"));
    }

    fn make_test_runtime() -> AgentRuntime {
        let conversation = Conversation::new("test".into(), "prompt".into(), 200_000);
        let registry = Arc::new(ToolRegistry::new());
        let executor = Arc::new(ToolExecutor::new(registry));
        let (_event_tx, _event_rx) = mpsc::channel::<Event>(1);

        AgentRuntime {
            conversation,
            executor,
            tool_ctx: ToolContext {
                working_dir: PathBuf::new(),
                permission_mode: crab_core::permission::PermissionMode::Default,
                session_id: "test".into(),
                cancellation_token: CancellationToken::new(),
                permission_policy: crab_core::permission::PermissionPolicy::default(),
                ext: crab_core::tool::ToolContextExt::default(),
            },
            loop_config: QueryConfig {
                model: ModelId::from("test"),
                max_tokens: 4096,
                temperature: None,
                tool_schemas: vec![],
                cache_enabled: false,
                budget_tokens: None,
                retry_policy: None,
                hook_executor: None,
                session_id: Some("test".into()),
                effort: None,
                fallback_model: None,
                plan_model: None,
                source: crab_core::query::QuerySource::Repl,
                compaction_client: None,
                compaction_config: CompactionConfig::default(),
                session_persister: None,
            },
            skill_registry: SkillRegistry::new(),
            session_history: None,
            _mcp_manager: None,
            cost: CostAccumulator::default(),
        }
    }
}
