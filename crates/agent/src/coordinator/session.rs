//! [`AgentSession`] — a running session with conversation, executor,
//! memory store, and cost accumulator wired together.
//!
//! Also contains the `format_memory_section` helper used during session
//! initialisation to inject loaded memories into the system prompt.

use std::sync::Arc;

use crab_api::LlmBackend;
use crab_core::event::Event;
use crab_core::message::Message;
use crab_core::model::ModelId;
use crab_core::tool::ToolContext;
use crab_memory::MemoryStore;
use crab_session::{Conversation, CostAccumulator, SessionHistory};
use crab_tools::executor::ToolExecutor;
use crab_tools::registry::ToolRegistry;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crab_engine::{QueryConfig, query_loop};

use super::manager::AgentCoordinator;
use super::session_config::SessionConfig;

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
