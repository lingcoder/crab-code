//! [`AgentSession`] — a running session with conversation, executor,
//! memory store, and cost accumulator wired together.
//!
//! Also contains the `format_memory_section` helper used during session
//! initialisation to inject loaded memories into the system prompt.

use std::sync::Arc;

use crab_api::LlmBackend;
use crab_core::event::Event;
use crab_core::model::ModelId;
use crab_core::tool::ToolContext;
use crab_memory::MemoryStore;
use crab_session::{Conversation, CostAccumulator, FileHistory, SessionHistory};
use crab_tools::executor::ToolExecutor;
use crab_tools::registry::ToolRegistry;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crab_engine::{QueryConfig, query_loop};

use crate::teams::WorkerPool;

use super::session_config::SessionConfig;

/// Extra state carried only when Layer 2b Coordinator Mode is active on a
/// session. Used by [`AgentSession::handle_spawn_request`] to give workers
/// a filtered registry and an overlay-free prompt.
pub struct CoordinatorContext {
    pub coordinator: crate::coordinator::Coordinator,
    /// The session's system prompt *before* the Coordinator overlay was
    /// appended. Workers must not see the overlay; they use this as their
    /// base prompt instead.
    pub worker_base_prompt: String,
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
    /// Coordinator Mode state, `Some` only when `SessionConfig::coordinator_mode`
    /// was set at construction.
    pub coordinator_ctx: Option<CoordinatorContext>,
    /// Tracks teams the model creates via `TeamCreateTool` and spawns
    /// teammates through the in-process backend.
    pub team_coordinator: crate::teams::coordinator::TeamCoordinator,
    /// Session-scoped file-edit snapshot store backing `/rewind`.
    pub file_history: Arc<std::sync::Mutex<FileHistory>>,
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
        // Snapshot the coordinator_mode flag before any partial-move of
        // session_config below. `bool: Copy`, so this is cheap.
        let coordinator_mode = session_config.coordinator_mode;

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
        let file_history_base = session_config
            .sessions_dir
            .as_ref()
            .and_then(|p| p.parent().map(|parent| parent.join("file-history")))
            .unwrap_or_else(|| std::env::temp_dir().join("crab-file-history"));
        let session_history = session_config.sessions_dir.map(SessionHistory::new);
        let file_history = Arc::new(std::sync::Mutex::new(FileHistory::new(
            file_history_base,
            &session_config.session_id,
        )));

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
            match crate::prompt::pr_context::load_pr_context(pr_ref) {
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

        // Layer 2b — apply Coordinator Mode if gated on. This strips the
        // registry to the allow-list and appends the anti-pattern prompt
        // overlay. No-op when coordinator_mode is false.
        //
        // The `worker_base_prompt` is snapshotted BEFORE the overlay is
        // applied so workers spawned later can be given a clean prompt.
        let coordinator_ctx = if coordinator_mode
            && let Some(coordinator) = crate::coordinator::Coordinator::from_flag(true)
        {
            let worker_base_prompt = conversation.system_prompt.clone();
            coordinator.apply(&mut registry, &mut conversation.system_prompt);
            tracing::info!(
                allowed_tools = ?coordinator.allowed_tools(),
                "Coordinator Mode active"
            );
            Some(CoordinatorContext {
                coordinator,
                worker_base_prompt,
            })
        } else {
            None
        };

        let tool_schemas = registry.tool_schemas();
        let executor = ToolExecutor::new(Arc::new(registry));
        let cancel = CancellationToken::new();

        let track_edit: crab_core::tool::TrackEditFn = {
            let fh = Arc::clone(&file_history);
            Arc::new(move |path: &std::path::Path, contents: &[u8]| {
                if let Ok(mut history) = fh.lock()
                    && let Err(e) = history.track_edit(path, contents)
                {
                    tracing::warn!(error = %e, path = %path.display(), "file_history track_edit failed");
                }
            })
        };
        let tool_ctx = ToolContext {
            working_dir: session_config.working_dir,
            permission_mode: session_config.permission_policy.mode,
            session_id: session_config.session_id.clone(),
            cancellation_token: cancel.clone(),
            permission_policy: session_config.permission_policy,
            ext: crab_core::tool::ToolContextExt {
                track_edit: Some(track_edit),
                ..Default::default()
            },
        };

        let compaction_client: Arc<dyn crab_session::CompactionClient> =
            Arc::new(crate::llm_compaction_client::LlmCompactionClient::new(
                Arc::clone(&backend),
                session_config.model.clone(),
            ));

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
            plan_model: None,
            source: crab_core::query::QuerySource::Repl,
            compaction_client: Some(compaction_client),
            compaction_config: crab_session::CompactionConfig::default(),
            session_persister: None,
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
            coordinator_ctx,
            team_coordinator: crate::teams::coordinator::TeamCoordinator::new(),
            file_history,
        }
    }

    /// Restore tracked file(s) from this session's file-history.
    ///
    /// `path = Some` rewinds just that path to its latest snapshot;
    /// `path = None` rewinds every tracked file. Returns the list of
    /// successfully restored paths as display strings.
    pub fn rewind(&self, path: Option<&str>) -> crab_core::Result<Vec<String>> {
        let history = self
            .file_history
            .lock()
            .map_err(|e| crab_core::Error::Other(format!("file history mutex poisoned: {e}")))?;
        let mut restored = Vec::new();
        if let Some(p) = path {
            let abs = std::path::PathBuf::from(p);
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

    /// Handle user input: add user message, run the query loop, and auto-save.
    ///
    /// If the conversation is above the 80% context-window watermark when
    /// this is called, [`compact_conversation`](Self::compact_conversation)
    /// runs first so the new turn starts with headroom.
    ///
    /// Fires the `UserPromptSubmit` hook before the message is appended so a
    /// `Deny` action can cleanly short-circuit the turn. When the hook
    /// returns an accepted action with a `message`, that message is
    /// appended as additional context after the user's own prompt.
    pub async fn handle_user_input(&mut self, input: &str) -> crab_core::Result<()> {
        if self.conversation.needs_compaction() {
            self.compact_conversation().await;
        }

        let additional_context = self.fire_user_prompt_submit_hook(input).await?;

        let user_msg = crab_session::expand_at_mentions(input, &self.tool_ctx.working_dir);
        self.conversation.push(user_msg);
        if let Some(ctx_msg) = additional_context {
            self.conversation.push_user(ctx_msg);
        }

        let starting_len = self.conversation.messages().len();

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

        // Intercept `team_created` markers in freshly appended tool results
        // so the teammate backend can spawn before the next turn needs them.
        self.process_team_markers(starting_len).await;

        // Auto-save session after each interaction
        self.auto_save_session().await;

        result
    }

    /// Fire `UserPromptSubmit` hooks before the user message enters the
    /// conversation.
    ///
    /// Returns `Ok(Some(msg))` when an accepting hook supplied a message
    /// that should be appended to the conversation as additional context;
    /// `Ok(None)` when there is no hook or the hook accepted without a
    /// message; `Err` when a hook's `Deny` action blocks the turn.
    ///
    /// Hook execution failures (process spawn errors) do not block the
    /// turn — they are logged and treated as Allow so a misconfigured
    /// script cannot lock the user out of their session.
    async fn fire_user_prompt_submit_hook(&self, input: &str) -> crab_core::Result<Option<String>> {
        let Some(hooks) = self.config.hook_executor.as_deref() else {
            return Ok(None);
        };
        let hook_ctx = crab_hooks::HookContext {
            tool_name: String::new(),
            tool_input: input.to_string(),
            working_dir: Some(self.tool_ctx.working_dir.clone()),
            tool_output: None,
            tool_exit_code: None,
            session_id: self.config.session_id.clone(),
        };
        match hooks
            .run(crab_hooks::HookTrigger::UserPromptSubmit, &hook_ctx)
            .await
        {
            Ok(hr) if hr.action == crab_hooks::HookAction::Deny => {
                Err(crab_core::Error::Other(hr.message.unwrap_or_else(|| {
                    "user prompt denied by UserPromptSubmit hook".to_string()
                })))
            }
            Ok(hr) => Ok(hr.message),
            Err(e) => {
                tracing::warn!(error = %e, "UserPromptSubmit hook execution failed");
                Ok(None)
            }
        }
    }

    /// Walk conversation messages appended during the last turn, looking
    /// for tool-result text that carries the `team_created` marker, and
    /// hand each hit to [`TeamCoordinator::process_tool_result`].
    async fn process_team_markers(&mut self, starting_len: usize) {
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

    /// Replace the conversation message history with a single summary
    /// message, freeing context-window space.
    ///
    /// Uses the heuristic summarizer (no LLM call) so the compaction itself
    /// is free and deterministic. Emits [`Event::CompactStart`] and
    /// [`Event::CompactEnd`]. System prompt, cost accumulator, and context
    /// window are preserved; the id stays the same.
    pub async fn compact_conversation(&mut self) -> crate::summarizer::ConversationSummary {
        let before_tokens = self.conversation.estimated_tokens();
        let before_count = self.conversation.len();

        let _ = self
            .event_tx
            .send(Event::CompactStart {
                strategy: "heuristic-summarizer".into(),
                before_tokens,
            })
            .await;

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
        let removed_messages = before_count.saturating_sub(self.conversation.len());

        let _ = self
            .event_tx
            .send(Event::CompactEnd {
                after_tokens,
                removed_messages,
            })
            .await;

        summary
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
    pub fn save_memory(&self, filename: &str, content: &str) -> crab_core::Result<()> {
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
        coordinator: &mut WorkerPool,
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

        // Coordinator Mode splits the worker's inputs from the parent session:
        //  - prompt:   use the pre-overlay base (workers must not see the
        //              "You do not execute code" coordinator guardrail).
        //  - registry: build a fresh default registry minus WORKER_DENIED_TOOLS
        //              so workers can Bash/Edit/Read but cannot spawn nested
        //              teams or message peers directly.
        // Regular sessions inherit both from the parent unchanged.
        let (parent_prompt, worker_executor) = if let Some(ctx) = &self.coordinator_ctx {
            let worker_reg = ctx.coordinator.build_worker_registry();
            let exec = Arc::new(ToolExecutor::new(Arc::new(worker_reg)));
            (ctx.worker_base_prompt.as_str(), exec)
        } else {
            let exec = Arc::new(ToolExecutor::new(self.executor.registry_arc()));
            (self.conversation.system_prompt.as_str(), exec)
        };

        let system_prompt =
            format!("You are a sub-agent worker. Complete the assigned task.\n\n{parent_prompt}");

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
pub(super) fn format_memory_section(memories: &[crab_memory::MemoryFile]) -> String {
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crab_core::message::Message;
    use crab_core::permission::PermissionPolicy;

    /// Create a dummy `LlmBackend` for tests (`OpenAI` client pointing to localhost).
    fn test_backend() -> Arc<LlmBackend> {
        Arc::new(LlmBackend::OpenAi(crab_api::openai::OpenAiClient::new(
            "http://localhost:0/v1",
            None,
        )))
    }

    fn base_config(session_id: &str) -> SessionConfig {
        SessionConfig {
            session_id: session_id.into(),
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
            coordinator_mode: false,
            default_shell: "bash".into(),
        }
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

        let mut config = base_config("sess_mem");
        config.system_prompt = "Base prompt.".into();
        config.memory_dir = Some(memory_dir);

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

        let mut config = base_config("new_sess");
        config.sessions_dir = Some(sessions_dir);
        config.resume_session_id = Some("prev_sess".into());

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
        let config = base_config("plain");

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

        let mut config = base_config("sess_save");
        config.memory_dir = Some(memory_dir.clone());

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
        let config = base_config("cancel-test");

        let backend = test_backend();
        let registry = ToolRegistry::new();
        let session = AgentSession::new(config, backend, registry);

        assert!(!session.cancel.is_cancelled());
        session.cancel();
        assert!(session.cancel.is_cancelled());
    }

    #[test]
    fn session_event_sender() {
        let config = base_config("event-test");

        let backend = test_backend();
        let registry = ToolRegistry::new();
        let session = AgentSession::new(config, backend, registry);

        let _tx = session.event_sender();
    }

    #[test]
    fn save_memory_without_store_is_noop() {
        let config = base_config("no-mem");

        let backend = test_backend();
        let registry = ToolRegistry::new();
        let session = AgentSession::new(config, backend, registry);

        session.save_memory("test.md", "content").unwrap();
    }

    #[test]
    fn coordinator_mode_session_injects_overlay_into_system_prompt() {
        use crab_tools::builtin::create_default_registry;

        let mut config = base_config("coord-overlay");
        config.system_prompt = "Base instructions.".into();
        config.coordinator_mode = true;

        let session = AgentSession::new(config, test_backend(), create_default_registry());

        // System prompt retains the original base plus the coordinator overlay.
        let prompt = &session.conversation.system_prompt;
        assert!(prompt.contains("Base instructions."));
        assert!(prompt.contains("Coordinator Mode"));
        assert!(prompt.contains("Based on your findings"));
    }

    #[tokio::test]
    async fn compact_conversation_replaces_history_with_summary() {
        use crab_tools::builtin::create_default_registry;

        let config = base_config("compact-test");
        let mut session = AgentSession::new(config, test_backend(), create_default_registry());

        // Build a non-trivial history.
        session.conversation.push(Message::user("Refactor foo.rs"));
        session
            .conversation
            .push(Message::assistant("Proposed three changes."));
        session.conversation.push(Message::user("Apply them."));
        session.conversation.push(Message::assistant("Applied."));
        let before = session.conversation.len();
        assert_eq!(before, 4);

        let summary = session.compact_conversation().await;

        // History collapsed to a single synthetic user message with the
        // summary text embedded.
        assert!(session.conversation.len() <= 1);
        if !summary.items.is_empty() {
            let text = session.conversation.messages()[0].text();
            assert!(text.contains("Previous conversation summary"));
        }
    }

    #[test]
    fn coordinator_session_captures_worker_base_prompt_without_overlay() {
        use crab_tools::builtin::create_default_registry;

        let mut config = base_config("coord-workerbase");
        config.system_prompt = "Base instructions.".into();
        config.coordinator_mode = true;

        let session = AgentSession::new(config, test_backend(), create_default_registry());

        let ctx = session
            .coordinator_ctx
            .as_ref()
            .expect("coordinator_ctx set when coordinator_mode is true");

        // Conversation prompt has overlay; worker base does not.
        assert!(
            session
                .conversation
                .system_prompt
                .contains("Coordinator Mode")
        );
        assert!(!ctx.worker_base_prompt.contains("Coordinator Mode"));
        assert!(ctx.worker_base_prompt.contains("Base instructions."));
    }

    #[test]
    fn non_coordinator_session_has_no_coordinator_ctx() {
        use crab_tools::builtin::create_default_registry;

        let config = base_config("no-coord-ctx");
        // coordinator_mode default = false
        let session = AgentSession::new(config, test_backend(), create_default_registry());
        assert!(session.coordinator_ctx.is_none());
    }

    #[test]
    fn non_coordinator_session_leaves_system_prompt_untouched() {
        use crab_tools::builtin::create_default_registry;

        let mut config = base_config("no-coord");
        config.system_prompt = "Base instructions.".into();
        // coordinator_mode default is false
        let session = AgentSession::new(config, test_backend(), create_default_registry());

        let prompt = &session.conversation.system_prompt;
        assert!(prompt.contains("Base instructions."));
        assert!(
            !prompt.contains("Coordinator Mode"),
            "overlay must not leak into non-coordinator sessions"
        );
    }

    #[tokio::test]
    async fn user_prompt_submit_hook_no_executor_returns_none() {
        // When no HookExecutor is configured, the helper must be a no-op:
        // Ok(None) so handle_user_input pushes the message unchanged.
        let config = base_config("no-hook");
        let session = AgentSession::new(config, test_backend(), ToolRegistry::new());
        let out = session.fire_user_prompt_submit_hook("hi").await;
        assert!(matches!(out, Ok(None)));
    }

    #[tokio::test]
    async fn user_prompt_submit_hook_accept_passes_message_through() {
        // An accepting hook whose stdout is plain text falls back to
        // exit-code semantics (0 = Allow, message=None). The helper
        // returns Ok(None) so no additional context is injected.
        use crab_hooks::{HookDef, HookExecutor, HookTrigger};
        let config = base_config("accept-hook");
        let mut session = AgentSession::new(config, test_backend(), ToolRegistry::new());
        let hook = HookDef {
            trigger: HookTrigger::UserPromptSubmit,
            command: "echo ok".into(),
            timeout_secs: 10,
            tool_filter: vec![],
            match_pattern: None,
        };
        session.config.hook_executor = Some(Arc::new(HookExecutor::with_hooks(vec![hook])));
        let out = session.fire_user_prompt_submit_hook("hello").await;
        assert!(matches!(out, Ok(None)));
    }

    #[test]
    fn session_resume_nonexistent_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let sessions_dir = dir.path().join("sessions");

        let mut config = base_config("new");
        config.sessions_dir = Some(sessions_dir);
        config.resume_session_id = Some("nonexistent".into());

        let backend = test_backend();
        let registry = ToolRegistry::new();
        let session = AgentSession::new(config, backend, registry);

        assert!(session.conversation.is_empty());
    }
}
