pub mod auto_dream;
pub mod coordinator;
pub mod error_recovery;
pub mod file_history;
pub mod proactive;
pub mod repl_commands;
pub mod runtime;
pub mod session;
pub mod summarizer;
pub mod system_prompt;
pub mod teams;

pub use coordinator::{PermissionDecisionEvent, PermissionSyncManager};
pub use error_recovery::{ErrorCategory, ErrorClassifier, RecoveryAction, RecoveryStrategy};
pub use file_history::{FileHistory, Snapshot, SnapshotError};
pub use repl_commands::{CommandResult, ReplCommand};
pub use runtime::{
    AgentRuntime, NotificationHookSink, QueryTaskResult, RuntimeInitConfig, RuntimeInitMeta,
    TeamMemberSnapshot, TeamSnapshot,
};
pub use session::{AgentSession, SessionConfig};
pub use summarizer::{
    ConversationSummary, SummarizerConfig, SummaryItem, SummaryItemKind, summarize_conversation,
};
pub use system_prompt::{build_system_prompt, build_system_prompt_with_memories};
pub use teams::{
    AgentHandle, AgentMessage, AgentStatus, AgentWorker, Capability, Envelope, InProcessBackend,
    MessageRouter, RetryDecision, RetryPolicy, RetryTracker, SharedTaskList, SwarmBackend, Task,
    TaskList, TaskStatus, Team, TeamMember, TeamMode, Teammate, TeammateConfig, TeammateState,
    Worker, WorkerConfig, WorkerPool, WorkerResult, event_channel, shared_task_list,
};

// Re-exports: allow tui to depend only on crab-agent instead of individual L2 crates.
pub use crab_api::LlmBackend;
pub use crab_api::openai;
pub use crab_engine::{EffortLevel, QueryConfig};
pub use crab_mcp::McpManager;
pub use crab_plugin::hook::{HookExecutor, HookTrigger};
pub use crab_session::{Conversation, CostAccumulator, SessionHistory, SessionMetadata};
pub use crab_skills::{Skill, SkillRegistry, SkillTrigger};
pub use crab_tools::executor::{PermissionHandler, ToolExecutor};
pub use crab_tools::registry::ToolRegistry;
