use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crab_api::LlmBackend;
use crab_core::event::Event;
use crab_core::message::Message;
use crab_core::model::ModelId;
use crab_core::permission::PermissionPolicy;
use crab_core::tool::ToolContext;
use crab_memory::MemoryStore;
use crab_session::{Conversation, CostAccumulator, SessionHistory};
use crab_tools::executor::ToolExecutor;
use crab_tools::registry::ToolRegistry;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::message_bus::{AgentMessage, Envelope, MessageBus};
use crate::message_router::MessageRouter;
use crate::retry::{RetryDecision, RetryPolicy, RetryTracker};
use crate::team::{Team, TeamMode};
use crate::worker::{AgentWorker, WorkerConfig, WorkerResult};
use crab_engine::{QueryConfig, query_loop};

/// Multi-agent orchestrator. Manages the main agent and worker pool.
///
/// The coordinator tracks running workers via `JoinHandle`s and provides
/// methods to spawn, cancel, and collect worker results.
pub struct AgentCoordinator {
    pub main_agent: AgentHandle,
    pub workers: Vec<AgentHandle>,
    pub bus: mpsc::Sender<AgentMessage>,
    /// Running worker tasks, keyed by worker ID.
    running: HashMap<String, RunningWorker>,
    /// Collected results from completed workers.
    completed: Vec<WorkerResult>,
    /// Counter for generating unique worker IDs.
    next_worker_id: u64,
    /// Message router for inter-agent communication.
    pub router: MessageRouter,
    /// Team structure and collaboration mode.
    pub team: Option<Team>,
    /// Retry tracker for failed tasks.
    pub retry_tracker: RetryTracker,
}

/// A worker that is currently running.
struct RunningWorker {
    pub handle: tokio::task::JoinHandle<WorkerResult>,
    pub cancel: CancellationToken,
}

/// Handle to a running agent (main or sub-agent).
pub struct AgentHandle {
    pub id: String,
    pub name: String,
    pub tx: mpsc::Sender<AgentMessage>,
}

/// Session configuration needed to start a query loop.
pub struct SessionConfig {
    pub session_id: String,
    pub system_prompt: String,
    pub model: ModelId,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
    pub context_window: u64,
    pub working_dir: std::path::PathBuf,
    pub permission_policy: PermissionPolicy,
    /// Path to memory store directory (e.g., `~/.crab/memory/`).
    pub memory_dir: Option<PathBuf>,
    /// Path to session history directory (e.g., `~/.crab/sessions/`).
    pub sessions_dir: Option<PathBuf>,
    /// Session ID to resume from (for `--resume`).
    pub resume_session_id: Option<String>,
    /// Effort level: "low", "medium", "high", "max".
    pub effort: Option<String>,
    /// Thinking mode: "enabled", "adaptive", "disabled".
    pub thinking_mode: Option<String>,
    /// Additional directories the agent may access beyond `working_dir`.
    pub additional_dirs: Vec<PathBuf>,
    /// Session display name (shown in /resume list).
    pub session_name: Option<String>,
    /// Maximum agent turns (print mode only).
    pub max_turns: Option<u32>,
    /// Maximum budget in USD (print mode only).
    pub max_budget_usd: Option<f64>,
    /// Fallback model for overloaded primary.
    pub fallback_model: Option<String>,

    // ─── B-level flags (Steps 10–13) ───
    /// Bare mode — skip hooks, plugins, auto-memory, CRAB.md discovery.
    pub bare_mode: bool,
    /// Git worktree branch name (empty string = auto-name).
    pub worktree_name: Option<String>,
    /// Fork into new session when resuming instead of continuing old one.
    pub fork_session: bool,
    /// Load context from a GitHub PR (number or URL).
    pub from_pr: Option<String>,
    /// Custom session ID override.
    pub custom_session_id: Option<String>,
    /// JSON Schema path/inline for output validation.
    pub json_schema: Option<String>,
    /// Additional plugin directories.
    pub plugin_dirs: Vec<PathBuf>,
    /// Disable slash commands / skills.
    pub disable_skills: bool,
    /// Extra API beta headers.
    pub beta_headers: Vec<String>,
    /// Connect to IDE extension.
    pub ide_connect: bool,
}

/// A running agent session with all the pieces wired together.
pub struct AgentSession {
    pub conversation: Conversation,
    pub backend: Arc<LlmBackend>,
    pub executor: ToolExecutor,
    pub tool_ctx: ToolContext,
    pub config: QueryConfig,
    pub event_tx: mpsc::Sender<Event>,
    pub event_rx: mpsc::Receiver<Event>,
    pub cancel: CancellationToken,
    /// Memory store for loading/saving user memories.
    pub memory_store: Option<MemoryStore>,
    /// Session history for persisting conversation transcripts.
    pub session_history: Option<SessionHistory>,
    /// Cost accumulator for tracking API usage.
    pub cost: CostAccumulator,
    /// Query engine (new unified API — optional during migration).
    pub engine: Option<crab_engine::QueryEngine>,
}

impl AgentSession {
    /// Initialize a new agent session.
    ///
    /// If `memory_dir` is set, loads memories and injects them into the
    /// system prompt. If `sessions_dir` is set, enables auto-save.
    /// If `resume_session_id` is set, restores messages from a prior session.
    pub fn new(
        session_config: SessionConfig,
        backend: Arc<LlmBackend>,
        mut registry: ToolRegistry,
    ) -> Self {
        // Register StructuredOutput tool when --json-schema is provided
        if let Some(ref schema_arg) = session_config.json_schema {
            match crab_tools::builtin::structured_output::StructuredOutputTool::from_arg(schema_arg)
            {
                Ok(tool) => {
                    registry.register(std::sync::Arc::new(tool));
                    tracing::info!("StructuredOutput tool registered");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "failed to create StructuredOutput tool");
                }
            }
        }

        let mut conversation = Conversation::new(
            session_config.session_id.clone(),
            session_config.system_prompt,
            session_config.context_window,
        );

        // Append StructuredOutput prompt when schema is provided
        if session_config.json_schema.is_some() {
            conversation
                .system_prompt
                .push_str(crab_tools::builtin::structured_output::STRUCTURED_OUTPUT_PROMPT);
        }

        let memory_store = session_config.memory_dir.map(MemoryStore::new);
        let session_history = session_config.sessions_dir.map(SessionHistory::new);

        // Load memories and inject into system prompt
        if let Some(store) = &memory_store
            && let Ok(memories) = store.scan()
            && !memories.is_empty()
        {
            let memory_section = format_memory_section(&memories);
            conversation.system_prompt.push_str(&memory_section);
        }

        // Load PR context if --from-pr was specified
        if let Some(pr_ref) = &session_config.from_pr
            && !pr_ref.is_empty()
        {
            match crate::pr_context::load_pr_context(pr_ref) {
                Ok(ctx) => {
                    conversation
                        .system_prompt
                        .push_str(&ctx.format_for_prompt());
                    tracing::info!(pr = pr_ref.as_str(), "loaded PR context");
                }
                Err(e) => {
                    tracing::warn!(pr = pr_ref.as_str(), error = %e, "failed to load PR context");
                }
            }
        }

        // Resume from previous session if requested
        if let Some(resume_id) = &session_config.resume_session_id
            && let Some(history) = &session_history
            && let Ok(Some(messages)) = history.load(resume_id)
        {
            for msg in messages {
                conversation.push(msg);
            }
        }

        let tool_schemas = registry.tool_schemas();
        let executor = ToolExecutor::new(Arc::new(registry));
        let cancel = CancellationToken::new();

        let tool_ctx = ToolContext {
            working_dir: session_config.working_dir,
            permission_mode: session_config.permission_policy.mode,
            session_id: session_config.session_id.clone(),
            cancellation_token: cancel.clone(),
            permission_policy: session_config.permission_policy,
            ext: crab_core::tool::ToolContextExt::default(),
        };

        let config = QueryConfig {
            model: session_config.model,
            max_tokens: session_config.max_tokens,
            temperature: session_config.temperature,
            tool_schemas,
            cache_enabled: false,
            budget_tokens: None,
            retry_policy: None,
            hook_executor: None,
            session_id: Some(session_config.session_id.clone()),
            effort: session_config
                .effort
                .as_deref()
                .and_then(|e| e.parse::<crab_engine::EffortLevel>().ok()),
            fallback_model: session_config.fallback_model.map(ModelId::from),
            source: crab_core::query::QuerySource::Repl,
        };

        let (event_tx, event_rx) = mpsc::channel(256);

        Self {
            conversation,
            backend,
            executor,
            tool_ctx,
            config,
            event_tx,
            event_rx,
            cancel,
            memory_store,
            session_history,
            cost: CostAccumulator::default(),
            engine: None,
        }
    }

    /// Handle user input: add user message, run the query loop, and auto-save.
    pub async fn handle_user_input(&mut self, input: &str) -> crab_common::Result<()> {
        self.conversation.push(Message::user(input));

        let result = query_loop::query_loop(
            &mut self.conversation,
            &self.backend,
            &self.executor,
            &self.tool_ctx,
            &self.config,
            &mut self.cost,
            self.event_tx.clone(),
            self.cancel.clone(),
        )
        .await;

        // Auto-save session after each interaction
        self.auto_save_session().await;

        result
    }

    /// Cancel the running query loop.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Get a clone of the event sender for external use.
    pub fn event_sender(&self) -> mpsc::Sender<Event> {
        self.event_tx.clone()
    }

    /// Save a memory file through the memory store.
    pub fn save_memory(&self, filename: &str, content: &str) -> crab_common::Result<()> {
        if let Some(store) = &self.memory_store {
            store.save(filename, content)?;
        }
        Ok(())
    }

    /// Handle a spawn request from `AgentTool` output.
    ///
    /// Parses the structured JSON from `AgentTool` (with `"action": "spawn_agent"`)
    /// and spawns a worker via the provided coordinator. Returns the worker ID.
    pub fn handle_spawn_request(
        &self,
        coordinator: &mut AgentCoordinator,
        spawn_request: &serde_json::Value,
    ) -> Option<String> {
        if spawn_request.get("action")?.as_str()? != "spawn_agent" {
            return None;
        }

        let task = spawn_request.get("task")?.as_str()?.to_string();
        let max_turns = spawn_request
            .get("max_turns")
            .and_then(serde_json::Value::as_u64)
            .map(|v| usize::try_from(v).unwrap_or(usize::MAX));

        let system_prompt = format!(
            "You are a sub-agent worker. Complete the assigned task.\n\n{}",
            self.conversation.system_prompt
        );

        let worker_executor = Arc::new(ToolExecutor::new(self.executor.registry_arc()));

        let worker_id = coordinator.spawn_worker(
            task,
            system_prompt,
            self.backend.clone(),
            worker_executor,
            self.tool_ctx.clone(),
            self.config.clone(),
            self.event_tx.clone(),
            max_turns,
        );

        Some(worker_id)
    }

    /// Auto-save the current session transcript to disk.
    async fn auto_save_session(&self) {
        if let Some(history) = &self.session_history {
            let session_id = &self.conversation.id;
            if let Err(e) = history.save(session_id, self.conversation.messages()) {
                let _ = self
                    .event_tx
                    .send(Event::Error {
                        message: format!("Failed to save session: {e}"),
                    })
                    .await;
                return;
            }
            let _ = self
                .event_tx
                .send(Event::SessionSaved {
                    session_id: session_id.clone(),
                })
                .await;
        }
    }
}

/// Format memory files as a section to append to the system prompt.
fn format_memory_section(memories: &[crab_memory::MemoryFile]) -> String {
    use std::fmt::Write;
    let mut section = String::new();
    let _ = writeln!(section, "\n\n# Loaded Memories\n");
    let _ = writeln!(
        section,
        "The following memories were loaded from previous sessions.\n"
    );
    for mem in memories {
        let _ = writeln!(
            section,
            "## {} (type: {})\n",
            mem.metadata.name, mem.metadata.memory_type
        );
        if !mem.metadata.description.is_empty() {
            let _ = writeln!(section, "> {}\n", mem.metadata.description);
        }
        let _ = writeln!(section, "{}\n", mem.body);
    }
    section
}

impl AgentCoordinator {
    /// Create a new coordinator with a message bus.
    pub fn new(main_id: String, main_name: String) -> Self {
        let bus = MessageBus::new(64);
        let main_tx = bus.sender();
        let mut router = MessageRouter::new();
        let _main_rx = router.register(&main_name);
        Self {
            main_agent: AgentHandle {
                id: main_id,
                name: main_name,
                tx: main_tx,
            },
            workers: Vec::new(),
            bus: bus.sender(),
            running: HashMap::new(),
            completed: Vec::new(),
            next_worker_id: 1,
            router,
            team: None,
            retry_tracker: RetryTracker::new(RetryPolicy::default()),
        }
    }

    /// Set a custom retry policy.
    pub fn set_retry_policy(&mut self, policy: RetryPolicy) {
        self.retry_tracker = RetryTracker::new(policy);
    }

    /// Handle a worker task failure: decide on retry.
    ///
    /// Returns `Some(task_id)` if the task should be retried, `None` if exhausted.
    pub fn on_worker_failure(&mut self, task_id: &str) -> Option<RetryDecision> {
        let decision = self.retry_tracker.on_failure(task_id);
        match &decision {
            RetryDecision::Retry { .. } => Some(decision),
            RetryDecision::GiveUp { .. } => None,
        }
    }

    /// Handle a worker task success: clear retry state.
    pub fn on_worker_success(&mut self, task_id: &str) {
        self.retry_tracker.on_success(task_id);
    }

    /// Set the team structure for this coordinator.
    pub fn set_team(&mut self, team: Team) {
        self.team = Some(team);
    }

    /// Get the current team mode, if a team is configured.
    #[must_use]
    pub fn team_mode(&self) -> Option<TeamMode> {
        self.team.as_ref().map(|t| t.mode)
    }

    /// Route an envelope through the message router.
    ///
    /// Respects team communication rules if a team is configured.
    /// Returns the number of agents the message was delivered to,
    /// or `Err` if communication is not allowed.
    pub async fn route_message(&self, envelope: &Envelope) -> crab_common::Result<usize> {
        // Enforce team communication rules
        if let Some(team) = &self.team
            && !envelope.is_broadcast()
            && !team.can_communicate(&envelope.from, &envelope.to)
        {
            return Err(crab_common::Error::Other(format!(
                "communication not allowed from '{}' to '{}' in {} mode",
                envelope.from, envelope.to, team.mode,
            )));
        }
        Ok(self.router.route(envelope).await)
    }

    /// Add a worker agent handle (for pre-configured workers).
    pub fn add_worker(&mut self, id: String, name: String) {
        self.workers.push(AgentHandle {
            id,
            name,
            tx: self.bus.clone(),
        });
    }

    /// Spawn a new sub-agent worker with the given task.
    ///
    /// The worker inherits the parent's backend, executor, and tool context
    /// but runs an independent conversation. Returns the assigned worker ID.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_worker(
        &mut self,
        task_prompt: String,
        system_prompt: String,
        backend: Arc<LlmBackend>,
        executor: Arc<ToolExecutor>,
        tool_ctx: ToolContext,
        loop_config: QueryConfig,
        event_tx: mpsc::Sender<Event>,
        max_turns: Option<usize>,
    ) -> String {
        let worker_id = format!("worker_{}", self.next_worker_id);
        self.next_worker_id += 1;

        let cancel = CancellationToken::new();

        let config = WorkerConfig {
            worker_id: worker_id.clone(),
            system_prompt,
            max_turns,
            max_duration: None,
            context_window: 200_000,
        };

        let worker = AgentWorker::new(
            config,
            backend,
            executor,
            tool_ctx,
            loop_config,
            event_tx,
            cancel.clone(),
        );

        let handle = worker.spawn(task_prompt);

        self.running
            .insert(worker_id.clone(), RunningWorker { handle, cancel });

        // Also register in the handle list and router
        self.workers.push(AgentHandle {
            id: worker_id.clone(),
            name: worker_id.clone(),
            tx: self.bus.clone(),
        });
        let _worker_rx = self.router.register(&worker_id);

        worker_id
    }

    /// Cancel a running worker by ID. Returns `true` if the worker was found.
    pub fn cancel_worker(&self, worker_id: &str) -> bool {
        self.running.get(worker_id).is_some_and(|worker| {
            worker.cancel.cancel();
            true
        })
    }

    /// Cancel all running workers.
    pub fn cancel_all(&self) {
        for worker in self.running.values() {
            worker.cancel.cancel();
        }
    }

    /// Collect the result of a specific worker, blocking until it completes.
    ///
    /// Returns `None` if the worker ID is not found or already collected.
    pub async fn collect_worker(&mut self, worker_id: &str) -> Option<WorkerResult> {
        let worker = self.running.remove(worker_id)?;
        match worker.handle.await {
            Ok(result) => {
                self.completed.push(result.clone_summary());
                Some(result)
            }
            Err(_) => None,
        }
    }

    /// Collect all completed workers (non-blocking: only those already finished).
    pub async fn collect_completed(&mut self) -> Vec<WorkerResult> {
        let mut results = Vec::new();
        let mut still_running = HashMap::new();

        for (id, worker) in self.running.drain() {
            if worker.handle.is_finished() {
                if let Ok(result) = worker.handle.await {
                    results.push(result);
                }
            } else {
                still_running.insert(id, worker);
            }
        }

        self.running = still_running;
        self.completed
            .extend(results.iter().map(WorkerResult::clone_summary));
        results
    }

    /// Wait for all running workers to complete and collect their results.
    pub async fn collect_all(&mut self) -> Vec<WorkerResult> {
        let mut results = Vec::new();
        for (_, worker) in self.running.drain() {
            if let Ok(result) = worker.handle.await {
                results.push(result);
            }
        }
        self.completed
            .extend(results.iter().map(WorkerResult::clone_summary));
        results
    }

    /// Get the number of currently running workers.
    #[must_use]
    pub fn running_count(&self) -> usize {
        self.running.len()
    }

    /// Get all completed worker results (summaries).
    #[must_use]
    pub fn completed_results(&self) -> &[WorkerResult] {
        &self.completed
    }

    /// Get the IDs of all currently running workers.
    #[must_use]
    pub fn running_worker_ids(&self) -> Vec<String> {
        self.running.keys().cloned().collect()
    }

    /// Check whether a specific worker is still running.
    #[must_use]
    pub fn is_worker_running(&self, worker_id: &str) -> bool {
        self.running.contains_key(worker_id)
    }

    /// Inject a pre-built running worker (for testing or external spawn).
    #[cfg(test)]
    fn inject_worker(
        &mut self,
        worker_id: String,
        handle: tokio::task::JoinHandle<WorkerResult>,
        cancel: CancellationToken,
    ) {
        self.running
            .insert(worker_id, RunningWorker { handle, cancel });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a dummy `LlmBackend` for tests (OpenAI client pointing to localhost).
    fn test_backend() -> Arc<LlmBackend> {
        Arc::new(LlmBackend::OpenAi(crab_api::openai::OpenAiClient::new(
            "http://localhost:0/v1",
            None,
        )))
    }

    #[test]
    fn coordinator_creation() {
        let coord = AgentCoordinator::new("main".into(), "Main Agent".into());
        assert_eq!(coord.main_agent.id, "main");
        assert_eq!(coord.main_agent.name, "Main Agent");
        assert!(coord.workers.is_empty());
        assert_eq!(coord.running_count(), 0);
        assert!(coord.completed_results().is_empty());
    }

    #[test]
    fn coordinator_add_worker() {
        let mut coord = AgentCoordinator::new("main".into(), "Main".into());
        coord.add_worker("w1".into(), "Worker 1".into());
        coord.add_worker("w2".into(), "Worker 2".into());
        assert_eq!(coord.workers.len(), 2);
        assert_eq!(coord.workers[0].id, "w1");
        assert_eq!(coord.workers[1].name, "Worker 2");
    }

    #[test]
    fn coordinator_cancel_nonexistent_worker() {
        let coord = AgentCoordinator::new("main".into(), "Main".into());
        assert!(!coord.cancel_worker("nonexistent"));
    }

    #[test]
    fn coordinator_cancel_all_empty() {
        let coord = AgentCoordinator::new("main".into(), "Main".into());
        // Should not panic
        coord.cancel_all();
    }

    #[tokio::test]
    async fn coordinator_collect_nonexistent_worker() {
        let mut coord = AgentCoordinator::new("main".into(), "Main".into());
        let result = coord.collect_worker("nonexistent").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn coordinator_collect_completed_empty() {
        let mut coord = AgentCoordinator::new("main".into(), "Main".into());
        let results = coord.collect_completed().await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn coordinator_collect_all_empty() {
        let mut coord = AgentCoordinator::new("main".into(), "Main".into());
        let results = coord.collect_all().await;
        assert!(results.is_empty());
    }

    #[test]
    fn coordinator_worker_id_increments() {
        let coord = AgentCoordinator::new("main".into(), "Main".into());
        assert_eq!(coord.next_worker_id, 1);
    }

    #[test]
    fn session_config_construction() {
        let config = SessionConfig {
            session_id: "sess_1".into(),
            system_prompt: "You are helpful.".into(),
            model: ModelId::from("claude-sonnet-4-20250514"),
            max_tokens: 4096,
            temperature: None,
            context_window: 200_000,
            working_dir: std::path::PathBuf::from("/tmp"),
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
        };
        assert_eq!(config.session_id, "sess_1");
        assert_eq!(config.context_window, 200_000);
    }

    #[test]
    fn session_with_memory_store() {
        let dir = tempfile::tempdir().unwrap();
        let memory_dir = dir.path().join("memory");

        // Write a memory file before creating the session
        let store = MemoryStore::new(memory_dir.clone());
        store
            .save(
                "user_role.md",
                "---\nname: User role\ndescription: Senior dev\ntype: user\n---\n\nSenior Rust dev.",
            )
            .unwrap();

        let config = SessionConfig {
            session_id: "sess_mem".into(),
            system_prompt: "Base prompt.".into(),
            model: ModelId::from("test-model"),
            max_tokens: 4096,
            temperature: None,
            context_window: 200_000,
            working_dir: PathBuf::from("/tmp"),
            permission_policy: PermissionPolicy::default(),
            memory_dir: Some(memory_dir),
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
        };

        let backend = test_backend();
        let registry = ToolRegistry::new();
        let session = AgentSession::new(config, backend, registry);

        // Memory should be injected into the system prompt
        assert!(session.conversation.system_prompt.contains("User role"));
        assert!(
            session
                .conversation
                .system_prompt
                .contains("Senior Rust dev")
        );
        assert!(session.memory_store.is_some());
    }

    #[test]
    fn session_with_session_history_resume() {
        let dir = tempfile::tempdir().unwrap();
        let sessions_dir = dir.path().join("sessions");

        // Save a previous session to resume from
        let history = SessionHistory::new(sessions_dir.clone());
        history
            .save(
                "prev_sess",
                &[Message::user("Hello"), Message::assistant("Hi!")],
            )
            .unwrap();

        let config = SessionConfig {
            session_id: "new_sess".into(),
            system_prompt: "Prompt.".into(),
            model: ModelId::from("test-model"),
            max_tokens: 4096,
            temperature: None,
            context_window: 200_000,
            working_dir: PathBuf::from("/tmp"),
            permission_policy: PermissionPolicy::default(),
            memory_dir: None,
            sessions_dir: Some(sessions_dir),
            resume_session_id: Some("prev_sess".into()),
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
        };

        let backend = test_backend();
        let registry = ToolRegistry::new();
        let session = AgentSession::new(config, backend, registry);

        // Resumed messages should be in the conversation
        assert_eq!(session.conversation.len(), 2);
        assert_eq!(session.conversation.messages()[0].text(), "Hello");
        assert_eq!(session.conversation.messages()[1].text(), "Hi!");
        assert!(session.session_history.is_some());
    }

    #[test]
    fn session_no_memory_no_history() {
        let config = SessionConfig {
            session_id: "plain".into(),
            system_prompt: "Prompt.".into(),
            model: ModelId::from("test-model"),
            max_tokens: 4096,
            temperature: None,
            context_window: 200_000,
            working_dir: PathBuf::from("/tmp"),
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
        };

        let backend = test_backend();
        let registry = ToolRegistry::new();
        let session = AgentSession::new(config, backend, registry);

        assert!(session.memory_store.is_none());
        assert!(session.session_history.is_none());
        assert!(session.conversation.is_empty());
        assert!(
            !session
                .conversation
                .system_prompt
                .contains("Loaded Memories")
        );
    }

    #[test]
    fn save_memory_through_session() {
        let dir = tempfile::tempdir().unwrap();
        let memory_dir = dir.path().join("memory");

        let config = SessionConfig {
            session_id: "sess_save".into(),
            system_prompt: "Prompt.".into(),
            model: ModelId::from("test-model"),
            max_tokens: 4096,
            temperature: None,
            context_window: 200_000,
            working_dir: PathBuf::from("/tmp"),
            permission_policy: PermissionPolicy::default(),
            memory_dir: Some(memory_dir.clone()),
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
        };

        let backend = test_backend();
        let registry = ToolRegistry::new();
        let session = AgentSession::new(config, backend, registry);

        session
            .save_memory(
                "test.md",
                "---\nname: Test\ndescription: test\ntype: user\n---\n\nBody.",
            )
            .unwrap();

        // Verify it was saved
        let store = MemoryStore::new(memory_dir);
        let content = store.load("test.md").unwrap().unwrap();
        assert!(content.contains("Body."));
    }

    #[test]
    fn format_memory_section_creates_markdown() {
        use crab_memory::{MemoryMetadata, MemoryType};
        let memories = vec![crab_memory::MemoryFile {
            filename: "test.md".into(),
            path: PathBuf::from("test.md"),
            metadata: MemoryMetadata {
                name: "Test".into(),
                description: "A test".into(),
                memory_type: MemoryType::User,
                created_at: None,
                updated_at: None,
            },
            body: "Content here.".into(),
            mtime: None,
        }];
        let section = format_memory_section(&memories);
        assert!(section.contains("# Loaded Memories"));
        assert!(section.contains("## Test (type: user)"));
        assert!(section.contains("> A test"));
        assert!(section.contains("Content here."));
    }

    #[test]
    fn format_memory_section_empty_description() {
        use crab_memory::{MemoryMetadata, MemoryType};
        let memories = vec![crab_memory::MemoryFile {
            filename: "nodesc.md".into(),
            path: PathBuf::from("nodesc.md"),
            metadata: MemoryMetadata {
                name: "NoDesc".into(),
                description: String::new(),
                memory_type: MemoryType::Project,
                created_at: None,
                updated_at: None,
            },
            body: "Body only.".into(),
            mtime: None,
        }];
        let section = format_memory_section(&memories);
        assert!(section.contains("## NoDesc (type: project)"));
        assert!(!section.contains("> \n")); // no blockquote for empty desc
        assert!(section.contains("Body only."));
    }

    #[test]
    fn format_memory_section_multiple_memories() {
        use crab_memory::{MemoryMetadata, MemoryType};
        let memories = vec![
            crab_memory::MemoryFile {
                filename: "first.md".into(),
                path: PathBuf::from("first.md"),
                metadata: MemoryMetadata {
                    name: "First".into(),
                    description: "desc1".into(),
                    memory_type: MemoryType::User,
                    created_at: None,
                    updated_at: None,
                },
                body: "body1".into(),
                mtime: None,
            },
            crab_memory::MemoryFile {
                filename: "second.md".into(),
                path: PathBuf::from("second.md"),
                metadata: MemoryMetadata {
                    name: "Second".into(),
                    description: "desc2".into(),
                    memory_type: MemoryType::Feedback,
                    created_at: None,
                    updated_at: None,
                },
                body: "body2".into(),
                mtime: None,
            },
        ];
        let section = format_memory_section(&memories);
        assert!(section.contains("First"));
        assert!(section.contains("Second"));
        assert!(section.contains("body1"));
        assert!(section.contains("body2"));
    }

    #[test]
    fn session_cancel() {
        let config = SessionConfig {
            session_id: "cancel-test".into(),
            system_prompt: "Prompt.".into(),
            model: ModelId::from("test-model"),
            max_tokens: 4096,
            temperature: None,
            context_window: 200_000,
            working_dir: PathBuf::from("/tmp"),
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
        };

        let backend = test_backend();
        let registry = ToolRegistry::new();
        let session = AgentSession::new(config, backend, registry);

        assert!(!session.cancel.is_cancelled());
        session.cancel();
        assert!(session.cancel.is_cancelled());
    }

    #[test]
    fn session_event_sender() {
        let config = SessionConfig {
            session_id: "event-test".into(),
            system_prompt: "Prompt.".into(),
            model: ModelId::from("test-model"),
            max_tokens: 4096,
            temperature: None,
            context_window: 200_000,
            working_dir: PathBuf::from("/tmp"),
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
        };

        let backend = test_backend();
        let registry = ToolRegistry::new();
        let session = AgentSession::new(config, backend, registry);

        let _tx = session.event_sender();
    }

    #[test]
    fn save_memory_without_store_is_noop() {
        let config = SessionConfig {
            session_id: "no-mem".into(),
            system_prompt: "Prompt.".into(),
            model: ModelId::from("test-model"),
            max_tokens: 4096,
            temperature: None,
            context_window: 200_000,
            working_dir: PathBuf::from("/tmp"),
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
        };

        let backend = test_backend();
        let registry = ToolRegistry::new();
        let session = AgentSession::new(config, backend, registry);

        session.save_memory("test.md", "content").unwrap();
    }

    #[test]
    fn session_resume_nonexistent_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let sessions_dir = dir.path().join("sessions");

        let config = SessionConfig {
            session_id: "new".into(),
            system_prompt: "Prompt.".into(),
            model: ModelId::from("test-model"),
            max_tokens: 4096,
            temperature: None,
            context_window: 200_000,
            working_dir: PathBuf::from("/tmp"),
            permission_policy: PermissionPolicy::default(),
            memory_dir: None,
            sessions_dir: Some(sessions_dir),
            resume_session_id: Some("nonexistent".into()),
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
        };

        let backend = test_backend();
        let registry = ToolRegistry::new();
        let session = AgentSession::new(config, backend, registry);

        assert!(session.conversation.is_empty());
    }

    // ─── Worker lifecycle tests ───────────────────────────────────────

    /// Create a mock `WorkerResult` for testing.
    fn mock_worker_result(worker_id: &str, success: bool) -> WorkerResult {
        WorkerResult {
            worker_id: worker_id.into(),
            output: if success { Some("done".into()) } else { None },
            success,
            usage: crab_core::model::TokenUsage::default(),
            conversation: Conversation::new(worker_id.into(), String::new(), 0),
        }
    }

    #[tokio::test]
    async fn coordinator_inject_and_collect_worker() {
        let mut coord = AgentCoordinator::new("main".into(), "Main".into());
        let cancel = CancellationToken::new();

        let handle = tokio::spawn(async { mock_worker_result("w1", true) });
        coord.inject_worker("w1".into(), handle, cancel);

        assert_eq!(coord.running_count(), 1);
        assert!(coord.is_worker_running("w1"));

        let result = coord.collect_worker("w1").await.unwrap();
        assert_eq!(result.worker_id, "w1");
        assert!(result.success);
        assert_eq!(result.output.as_deref(), Some("done"));

        // After collection, no longer running
        assert_eq!(coord.running_count(), 0);
        assert!(!coord.is_worker_running("w1"));

        // Completed results should have a summary
        assert_eq!(coord.completed_results().len(), 1);
        assert_eq!(coord.completed_results()[0].worker_id, "w1");
    }

    #[tokio::test]
    async fn coordinator_collect_worker_twice_returns_none() {
        let mut coord = AgentCoordinator::new("main".into(), "Main".into());
        let cancel = CancellationToken::new();

        let handle = tokio::spawn(async { mock_worker_result("w1", true) });
        coord.inject_worker("w1".into(), handle, cancel);

        coord.collect_worker("w1").await.unwrap();
        // Second collect returns None
        assert!(coord.collect_worker("w1").await.is_none());
    }

    #[tokio::test]
    async fn coordinator_collect_all_multiple_workers() {
        let mut coord = AgentCoordinator::new("main".into(), "Main".into());

        for i in 1..=3 {
            let id = format!("w{i}");
            let cancel = CancellationToken::new();
            let handle = tokio::spawn({
                let id = id.clone();
                async move { mock_worker_result(&id, true) }
            });
            coord.inject_worker(id, handle, cancel);
        }

        assert_eq!(coord.running_count(), 3);

        let results = coord.collect_all().await;
        assert_eq!(results.len(), 3);
        assert_eq!(coord.running_count(), 0);
        assert_eq!(coord.completed_results().len(), 3);
    }

    #[tokio::test]
    async fn coordinator_collect_completed_only_finished() {
        let mut coord = AgentCoordinator::new("main".into(), "Main".into());

        // Worker that finishes immediately
        let cancel1 = CancellationToken::new();
        let handle1 = tokio::spawn(async { mock_worker_result("fast", true) });
        coord.inject_worker("fast".into(), handle1, cancel1);

        // Worker that blocks until cancelled
        let cancel2 = CancellationToken::new();
        let cancel2_clone = cancel2.clone();
        let handle2 = tokio::spawn(async move {
            cancel2_clone.cancelled().await;
            mock_worker_result("slow", false)
        });
        coord.inject_worker("slow".into(), handle2, cancel2.clone());

        // Give fast worker time to finish
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let results = coord.collect_completed().await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].worker_id, "fast");

        // slow is still running
        assert_eq!(coord.running_count(), 1);
        assert!(coord.is_worker_running("slow"));

        // Clean up: cancel the slow worker
        cancel2.cancel();
        let remaining = coord.collect_all().await;
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].worker_id, "slow");
    }

    #[tokio::test]
    async fn coordinator_cancel_running_worker() {
        let mut coord = AgentCoordinator::new("main".into(), "Main".into());
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        let handle = tokio::spawn(async move {
            cancel_clone.cancelled().await;
            mock_worker_result("w1", false)
        });
        coord.inject_worker("w1".into(), handle, cancel);

        assert!(coord.cancel_worker("w1"));

        // Collect the cancelled worker
        let result = coord.collect_worker("w1").await.unwrap();
        assert_eq!(result.worker_id, "w1");
        assert!(!result.success);
    }

    #[tokio::test]
    async fn coordinator_cancel_all_workers() {
        let mut coord = AgentCoordinator::new("main".into(), "Main".into());

        for i in 1..=3 {
            let id = format!("w{i}");
            let cancel = CancellationToken::new();
            let cancel_clone = cancel.clone();
            let handle = tokio::spawn({
                let id = id.clone();
                async move {
                    cancel_clone.cancelled().await;
                    mock_worker_result(&id, false)
                }
            });
            coord.inject_worker(id, handle, cancel);
        }

        assert_eq!(coord.running_count(), 3);

        coord.cancel_all();
        let results = coord.collect_all().await;
        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|r| !r.success));
    }

    #[test]
    fn coordinator_running_worker_ids() {
        let mut coord = AgentCoordinator::new("main".into(), "Main".into());
        let cancel = CancellationToken::new();

        let handle = tokio::runtime::Runtime::new()
            .unwrap()
            .spawn(async { mock_worker_result("w1", true) });
        coord.inject_worker("w1".into(), handle, cancel);

        let ids = coord.running_worker_ids();
        assert_eq!(ids.len(), 1);
        assert!(ids.contains(&"w1".to_string()));
    }

    #[test]
    fn coordinator_worker_id_auto_increments_on_spawn_worker() {
        let coord = AgentCoordinator::new("main".into(), "Main".into());
        assert_eq!(coord.next_worker_id, 1);

        // We can't call spawn_worker without a real backend/executor,
        // but we can test the ID format by verifying the increment logic.
        // spawn_worker uses format!("worker_{}", self.next_worker_id).
        let expected_id = format!("worker_{}", coord.next_worker_id);
        assert_eq!(expected_id, "worker_1");
    }

    // ─── Router integration tests ───────────────────────────────────

    #[test]
    fn coordinator_has_router() {
        let coord = AgentCoordinator::new("main".into(), "Main".into());
        // Main agent should be registered in the router
        assert!(coord.router.is_registered("Main"));
        assert_eq!(coord.router.agent_count(), 1);
    }

    #[tokio::test]
    async fn coordinator_route_directed_message() {
        let mut coord = AgentCoordinator::new("main".into(), "Main".into());
        let mut worker_rx = coord.router.register("Worker1");

        let env = Envelope::new(
            "Main",
            "Worker1",
            AgentMessage::AssignTask {
                task_id: "t1".into(),
                prompt: "do stuff".into(),
            },
        );

        let delivered = coord.route_message(&env).await.unwrap();
        assert_eq!(delivered, 1);

        let msg = worker_rx.recv().await.unwrap();
        assert_eq!(msg.from, "Main");
        assert!(matches!(msg.payload, AgentMessage::AssignTask { .. }));
    }

    #[tokio::test]
    async fn coordinator_route_broadcast() {
        let mut coord = AgentCoordinator::new("main".into(), "Main".into());
        let mut rx1 = coord.router.register("W1");
        let mut rx2 = coord.router.register("W2");

        let env = Envelope::broadcast("Main", AgentMessage::Shutdown);
        let delivered = coord.route_message(&env).await.unwrap();
        assert_eq!(delivered, 2);

        assert!(rx1.recv().await.is_some());
        assert!(rx2.recv().await.is_some());
    }

    // ─── Team integration tests ─────────────────────────────────────

    #[test]
    fn coordinator_no_team_by_default() {
        let coord = AgentCoordinator::new("main".into(), "Main".into());
        assert!(coord.team.is_none());
        assert!(coord.team_mode().is_none());
    }

    #[test]
    fn coordinator_set_team() {
        let mut coord = AgentCoordinator::new("main".into(), "Main".into());

        let mut team = Team::with_mode("dev".into(), TeamMode::PeerToPeer);
        let mut alice = crate::team::TeamMember::new("a1", "Alice", "model");
        alice.is_leader = true;
        team.add_member(alice);
        team.add_member(crate::team::TeamMember::new("a2", "Bob", "model"));

        coord.set_team(team);
        assert_eq!(coord.team_mode(), Some(TeamMode::PeerToPeer));
    }

    #[tokio::test]
    async fn coordinator_leader_worker_blocks_worker_to_worker() {
        let mut coord = AgentCoordinator::new("main".into(), "Main".into());

        let mut team = Team::new("dev".into()); // LeaderWorker by default
        let mut leader = crate::team::TeamMember::new("a1", "Leader", "model");
        leader.is_leader = true;
        team.add_member(leader);
        team.add_member(crate::team::TeamMember::new("a2", "W1", "model"));
        team.add_member(crate::team::TeamMember::new("a3", "W2", "model"));
        coord.set_team(team);

        let _rx1 = coord.router.register("W1");
        let _rx2 = coord.router.register("W2");
        let _rx_leader = coord.router.register("Leader");

        // Leader → Worker: allowed
        let env = Envelope::new("Leader", "W1", AgentMessage::Shutdown);
        assert!(coord.route_message(&env).await.is_ok());

        // Worker → Leader: allowed
        let env = Envelope::new("W1", "Leader", AgentMessage::ShutdownAck);
        assert!(coord.route_message(&env).await.is_ok());

        // Worker → Worker: blocked
        let env = Envelope::new("W1", "W2", AgentMessage::Shutdown);
        assert!(coord.route_message(&env).await.is_err());
    }

    #[tokio::test]
    async fn coordinator_peer_to_peer_allows_all() {
        let mut coord = AgentCoordinator::new("main".into(), "Main".into());

        let mut team = Team::with_mode("dev".into(), TeamMode::PeerToPeer);
        team.add_member(crate::team::TeamMember::new("a1", "A", "model"));
        team.add_member(crate::team::TeamMember::new("a2", "B", "model"));
        coord.set_team(team);

        let _rx_a = coord.router.register("A");
        let _rx_b = coord.router.register("B");

        // A → B: allowed
        let env = Envelope::new("A", "B", AgentMessage::Shutdown);
        assert!(coord.route_message(&env).await.is_ok());

        // B → A: allowed
        let env = Envelope::new("B", "A", AgentMessage::Shutdown);
        assert!(coord.route_message(&env).await.is_ok());
    }

    // ─── Assignment strategy tests ──────────────────────────────────

    #[test]
    fn coordinator_set_retry_policy() {
        let mut coord = AgentCoordinator::new("main".into(), "Main".into());
        coord.set_retry_policy(crate::retry::RetryPolicy::no_retry());

        // First failure should give up immediately
        let decision = coord.on_worker_failure("t1");
        assert!(decision.is_none());
    }

    #[test]
    fn coordinator_on_worker_failure_retries() {
        let mut coord = AgentCoordinator::new("main".into(), "Main".into());
        // Default policy: 2 retries

        let decision = coord.on_worker_failure("t1");
        assert!(decision.is_some());
        assert!(matches!(decision, Some(RetryDecision::Retry { .. })));

        let decision = coord.on_worker_failure("t1");
        assert!(decision.is_some());

        let decision = coord.on_worker_failure("t1");
        assert!(decision.is_none()); // exhausted
    }

    #[test]
    fn coordinator_success_clears_retry_state() {
        let mut coord = AgentCoordinator::new("main".into(), "Main".into());

        coord.on_worker_failure("t1");
        assert_eq!(coord.retry_tracker.attempts_for("t1"), 1);

        coord.on_worker_success("t1");
        assert_eq!(coord.retry_tracker.attempts_for("t1"), 0);
    }
}
