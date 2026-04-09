pub mod coordinator;
pub mod effort;
pub mod engine;
pub mod error_recovery;
pub mod git_context;
pub mod message_bus;
pub mod message_router;
pub mod pr_context;
pub mod prompt_suggestion;
pub mod query_loop;
pub mod repl_commands;
pub mod retry;
pub mod rollback;
pub mod slash_commands;
pub mod stop_hooks;
pub mod summarizer;
pub mod swarm;
pub mod system_prompt;
pub mod task;
pub mod team;
pub mod tips;
pub mod token_budget;
pub mod worker;

pub use coordinator::{AgentCoordinator, AgentHandle, AgentSession, SessionConfig};
pub use effort::{EffortLevel, ThinkingMode};
pub use engine::streaming::StreamingToolExecutor;
pub use engine::tool_orchestration::{ToolCallRef, partition_tool_calls, tool_results_message};
pub use engine::{QueryEngine, QueryEngineConfig, QuerySource};
pub use error_recovery::{
    CircuitBreaker, CircuitBreakerConfig, CircuitState, DegradableFeature, ErrorCategory,
    ErrorClassifier, FeaturePriority, GracefulDegradation, RecoveryAction, RecoveryStrategy,
};
pub use message_bus::{AgentMessage, AgentStatus, Envelope, event_channel};
pub use message_router::MessageRouter;
pub use query_loop::{QueryLoopConfig, query_loop};
pub use repl_commands::{CommandResult, ReplCommand};
pub use retry::{RetryDecision, RetryPolicy, RetryTracker};
pub use rollback::{ActionType, RollbackEntry, RollbackManager, UndoStack};
pub use slash_commands::{
    SlashAction, SlashCommandContext, SlashCommandRegistry, SlashCommandResult,
};
pub use stop_hooks::{StopConditions, StopReason};
pub use summarizer::{
    ConversationSummary, SummarizerConfig, SummaryItem, SummaryItemKind, summarize_conversation,
};
pub use swarm::{
    InProcessBackend, PaneInfo, PaneManager, PermissionDecisionEvent, PermissionSyncManager,
    SwarmBackend, Teammate, TeammateConfig, TeammateState, TmuxBackend, generate_init_script,
};
pub use system_prompt::{build_system_prompt, build_system_prompt_with_memories};
pub use task::{SharedTaskList, Task, TaskList, TaskStatus, shared_task_list};
pub use team::{Capability, Team, TeamMember, TeamMode};
pub use token_budget::{BudgetDecision, TokenBudget};
pub use worker::{AgentWorker, Worker, WorkerConfig, WorkerResult};
