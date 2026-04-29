//! Raw query loop for Crab Code — extracted from `crab-agent` in v2.3.
//!
//! This crate holds the low-level "messages + backend + tool executor →
//! streaming events" loop. It is intentionally thin: no system-prompt
//! assembly, no REPL state, no swarm — those live in `crab-agent` which
//! wraps the engine.
//!
//! The daemon and any SDK consumer should depend on `crab-engine` directly
//! rather than pulling in the full `crab-agent` surface.

use std::sync::Arc;

use crab_api::LlmBackend;
use crab_api::rate_limit::RetryPolicy;
use crab_core::event::Event;
use crab_core::model::ModelId;
use crab_core::query::QuerySource;
use crab_core::tool::ToolContext;
use crab_plugin::hook::HookExecutor;
use crab_session::{
    CompactionClient, CompactionConfig, Conversation, CostAccumulator, SessionPersister,
};
use crab_tools::executor::ToolExecutor;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub mod effort;
#[path = "loop.rs"]
pub mod query_loop;
pub mod stop_hooks;
pub mod streaming;
pub mod telemetry;
pub mod token_budget;
pub mod tool_orchestration;

pub use effort::{EffortLevel, ThinkingMode};
pub use query_loop::query_loop;
pub use stop_hooks::{StopConditions, StopReason};
pub use streaming::StreamingToolExecutor;
pub use token_budget::{BudgetDecision, TokenBudget};
pub use tool_orchestration::{
    ToolCallRef, execute_tool_calls, partition_tool_calls, tool_results_message,
};

/// Unified query configuration — merged from the former `QueryLoopConfig`
/// and `QueryEngineConfig` in `crab-agent` v2.2.
///
/// Owns everything the loop needs to run one or more turns of LLM + tool
/// execution. Does not include conversation state or an event channel;
/// those are passed as separate parameters to [`QueryEngine::run`].
#[derive(Clone)]
pub struct QueryConfig {
    /// Model identifier to invoke for each API request.
    pub model: ModelId,
    /// Per-request output cap.
    pub max_tokens: u32,
    /// Sampling temperature (`None` = provider default).
    pub temperature: Option<f32>,
    /// Tool JSON schemas advertised to the model each request.
    pub tool_schemas: Vec<serde_json::Value>,
    /// Enable Anthropic prompt caching (ignored by other providers).
    pub cache_enabled: bool,
    /// Extended-thinking token budget — overridden by `effort` when set.
    pub budget_tokens: Option<u32>,
    /// Reasoning effort level (overrides `budget_tokens` via `to_budget_tokens`).
    pub effort: Option<EffortLevel>,
    /// Retry policy for transient API errors.
    pub retry_policy: Option<RetryPolicy>,
    /// Fallback model when the primary is overloaded (HTTP 529) or rate-limited (429).
    pub fallback_model: Option<ModelId>,
    /// Model override for plan mode (stronger model for architectural reasoning).
    /// When set and plan mode is active, this model is used instead of the primary.
    pub plan_model: Option<ModelId>,
    /// Hook executor for `PreToolUse` / `PostToolUse` / `UserPromptSubmit`.
    pub hook_executor: Option<Arc<HookExecutor>>,
    /// Session ID passed to hooks via `CRAB_SESSION_ID` env var.
    pub session_id: Option<String>,
    /// Query origin — gates post-response behavior (memory extraction etc.).
    pub source: QuerySource,
    /// Client for LLM-driven compaction (summarization, microcompaction).
    /// When `None`, compaction falls back to non-LLM strategies (snip/truncate).
    pub compaction_client: Option<Arc<dyn CompactionClient>>,
    /// Compaction configuration (mode, trigger thresholds, preservation rules).
    pub compaction_config: CompactionConfig,
    /// Per-turn message persister for crash-resilient JSONL transcripts.
    /// When set, each message is appended to disk as it enters the conversation.
    pub session_persister: Option<Arc<dyn SessionPersister>>,
}

/// Query engine — bundles the immutable handles needed to run the loop.
///
/// Created once per session by `crab-agent` (or a daemon) and reused for
/// every query. Holds references to the backend, tool executor, and
/// configuration; does not own the `Conversation` (passed in per call).
pub struct QueryEngine {
    pub backend: Arc<LlmBackend>,
    pub executor: ToolExecutor,
    pub tool_ctx: ToolContext,
    pub config: QueryConfig,
    pub cost: CostAccumulator,
    pub event_tx: mpsc::Sender<Event>,
    pub cancel: CancellationToken,
}

impl QueryEngine {
    /// Create a new query engine.
    pub fn new(
        config: QueryConfig,
        backend: Arc<LlmBackend>,
        executor: ToolExecutor,
        tool_ctx: ToolContext,
        event_tx: mpsc::Sender<Event>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            backend,
            executor,
            tool_ctx,
            config,
            cost: CostAccumulator::default(),
            event_tx,
            cancel,
        }
    }

    /// Drive the full query loop against the given conversation.
    pub async fn run(&mut self, conversation: &mut Conversation) -> crab_core::Result<()> {
        query_loop::query_loop(
            conversation,
            &self.backend,
            &self.executor,
            &self.tool_ctx,
            &self.config,
            &mut self.cost,
            self.event_tx.clone(),
            self.cancel.clone(),
        )
        .await
    }

    /// Fire post-response background tasks (memory extraction, etc.).
    ///
    /// Non-blocking — spawns a task and returns immediately.
    pub fn post_response_hooks(&self, conversation: &Conversation) {
        let messages = conversation.messages().to_vec();
        let source = self.config.source.clone();

        tokio::spawn(async move {
            if matches!(source, QuerySource::Repl | QuerySource::Agent { .. }) {
                let extraction =
                    crab_session::memory_extract::extract_memories_from_conversation(&messages);
                if !extraction.memories.is_empty() {
                    tracing::debug!(
                        count = extraction.memories.len(),
                        "post-response: extracted memories"
                    );
                }
            }
        });
    }
}
