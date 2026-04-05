pub mod adaptive_prompt;
pub mod assignment;
pub mod code_nav;
pub mod conversation_tree;
pub mod coordinator;
pub mod health;
pub mod memory_retriever;
pub mod message_bus;
pub mod message_router;
pub mod metrics;
pub mod project_context;
pub mod query_loop;
pub mod repl_commands;
pub mod retry;
pub mod smart_context;
pub mod summarizer;
pub mod system_prompt;
pub mod task;
pub mod team;
pub mod tool_pipeline;
pub mod work_stealing;
pub mod worker;

pub use adaptive_prompt::{
    ContextType, PromptTemplate, PromptTemplateRegistry, ToolRecommender, ToolSelector,
    detect_context,
};
pub use assignment::{AssignmentStrategy, CapabilityBased, LeastLoaded, RoundRobin};
pub use code_nav::{
    CodeNavigator, Language, SymbolKind, SymbolLocation, detect_language, find_definitions,
    find_implementations, find_references, format_nav_results,
};
pub use conversation_tree::{Branch, BranchError, BranchId, ConversationNode, ConversationTree};
pub use coordinator::{AgentCoordinator, AgentHandle, AgentSession, SessionConfig};
pub use memory_retriever::{
    MemoryRanker, RankedMemory, RetrieverConfig, format_retrieved_memories, retrieve_for_context,
    retrieve_memories,
};
pub use health::{HealthConfig, HealthMonitor, HealthStatus};
pub use message_bus::{AgentMessage, AgentStatus, Envelope, event_channel};
pub use message_router::MessageRouter;
pub use metrics::{AgentMetrics, MetricsCollector};
pub use project_context::{
    DependencyGraph, FileScore, ProjectSummary, ProjectType, analyze_project, detect_project_type,
    parse_cargo_deps, score_files,
};
pub use query_loop::{QueryLoopConfig, StreamingToolExecutor, query_loop};
pub use repl_commands::{CommandResult, ReplCommand, execute_command};
pub use retry::{RetryDecision, RetryPolicy, RetryTracker};
pub use summarizer::{
    ConversationSummary, SummarizerConfig, SummaryItem, SummaryItemKind,
    summarize_conversation,
};
pub use smart_context::{
    ContextConfig, ContextSnippet, ContextUsageTracker, QueryTerms, RelevantFile,
    build_context_snippets_from_content, extract_query_terms, format_context_section,
    score_file_relevance, smart_context_for_query,
};
pub use system_prompt::{build_system_prompt, build_system_prompt_with_memories};
pub use task::{SharedTaskList, Task, TaskList, TaskStatus, shared_task_list};
pub use team::{Capability, Team, TeamMember, TeamMode};
pub use tool_pipeline::{
    PipelineResult, PipelineStep, StepCondition, StepResult, ToolChain, ToolPipeline,
};
pub use work_stealing::{QueuedTask, WorkStealingScheduler};
pub use worker::{AgentWorker, Worker, WorkerConfig, WorkerResult};
