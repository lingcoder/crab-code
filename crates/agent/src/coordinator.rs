use std::path::PathBuf;
use std::sync::Arc;

use crab_api::LlmBackend;
use crab_core::event::Event;
use crab_core::message::Message;
use crab_core::model::ModelId;
use crab_core::permission::PermissionPolicy;
use crab_core::tool::ToolContext;
use crab_session::{Conversation, CostAccumulator, MemoryStore, SessionHistory};
use crab_tools::executor::ToolExecutor;
use crab_tools::registry::ToolRegistry;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::message_bus::{AgentMessage, MessageBus};
use crate::query_loop::{self, QueryLoopConfig};

/// Multi-agent orchestrator. Manages the main agent and worker pool.
pub struct AgentCoordinator {
    pub main_agent: AgentHandle,
    pub workers: Vec<AgentHandle>,
    pub bus: mpsc::Sender<AgentMessage>,
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
}

/// A running agent session with all the pieces wired together.
pub struct AgentSession {
    pub conversation: Conversation,
    pub backend: Arc<LlmBackend>,
    pub executor: ToolExecutor,
    pub tool_ctx: ToolContext,
    pub config: QueryLoopConfig,
    pub event_tx: mpsc::Sender<Event>,
    pub event_rx: mpsc::Receiver<Event>,
    pub cancel: CancellationToken,
    /// Memory store for loading/saving user memories.
    pub memory_store: Option<MemoryStore>,
    /// Session history for persisting conversation transcripts.
    pub session_history: Option<SessionHistory>,
    /// Cost accumulator for tracking API usage.
    pub cost: CostAccumulator,
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
        registry: ToolRegistry,
    ) -> Self {
        let mut conversation = Conversation::new(
            session_config.session_id.clone(),
            session_config.system_prompt,
            session_config.context_window,
        );

        let memory_store = session_config.memory_dir.map(MemoryStore::new);
        let session_history = session_config.sessions_dir.map(SessionHistory::new);

        // Load memories and inject into system prompt
        if let Some(store) = &memory_store
            && let Ok(memories) = store.load_all()
            && !memories.is_empty()
        {
            let memory_section = format_memory_section(&memories);
            conversation.system_prompt.push_str(&memory_section);
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
            session_id: session_config.session_id,
            cancellation_token: cancel.clone(),
            permission_policy: session_config.permission_policy,
        };

        let config = QueryLoopConfig {
            model: session_config.model,
            max_tokens: session_config.max_tokens,
            temperature: session_config.temperature,
            tool_schemas,
            cache_enabled: false,
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
fn format_memory_section(memories: &[crab_session::MemoryFile]) -> String {
    use std::fmt::Write;
    let mut section = String::new();
    let _ = writeln!(section, "\n\n# Loaded Memories\n");
    let _ = writeln!(
        section,
        "The following memories were loaded from previous sessions.\n"
    );
    for mem in memories {
        let _ = writeln!(section, "## {} (type: {})\n", mem.name, mem.memory_type);
        if !mem.description.is_empty() {
            let _ = writeln!(section, "> {}\n", mem.description);
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
        Self {
            main_agent: AgentHandle {
                id: main_id,
                name: main_name,
                tx: main_tx,
            },
            workers: Vec::new(),
            bus: bus.sender(),
        }
    }

    /// Add a worker agent.
    pub fn add_worker(&mut self, id: String, name: String) {
        self.workers.push(AgentHandle {
            id,
            name,
            tx: self.bus.clone(),
        });
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
        let memories = vec![crab_session::MemoryFile {
            name: "Test".into(),
            description: "A test".into(),
            memory_type: "user".into(),
            body: "Content here.".into(),
            filename: "test.md".into(),
        }];
        let section = format_memory_section(&memories);
        assert!(section.contains("# Loaded Memories"));
        assert!(section.contains("## Test (type: user)"));
        assert!(section.contains("> A test"));
        assert!(section.contains("Content here."));
    }
}
