//! `QueryEngine` — unified query orchestration for LLM calls + tool execution.
//!
//! Corresponds to CC's `query()` + `toolOrchestration` + `handleStopHooks`.

pub mod streaming;
pub mod tool_orchestration;

use std::sync::Arc;

use crab_api::LlmBackend;
use crab_api::rate_limit::RetryPolicy;
use crab_core::event::Event;
use crab_core::model::ModelId;
use crab_core::tool::ToolContext;
use crab_plugin::hook::HookExecutor;
use crab_session::{Conversation, CostAccumulator};
use crab_tools::executor::ToolExecutor;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Query source tag — identifies where a query originated.
///
/// Used to gate behavior: which hooks run, whether to persist, etc.
/// Corresponds to CC's `QuerySource` type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum QuerySource {
    /// User input from the REPL main thread.
    #[default]
    Repl,
    /// Sub-agent query.
    Agent { agent_id: String },
    /// Auto-compaction triggered query.
    Compact,
    /// Session memory extraction.
    SessionMemory,
    /// SDK / API call.
    Sdk,
    /// Print mode (non-interactive, single-shot).
    Print,
}

/// Configuration for the query engine.
#[derive(Clone)]
pub struct QueryEngineConfig {
    pub model: ModelId,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
    pub tool_schemas: Vec<serde_json::Value>,
    pub cache_enabled: bool,
    pub budget_tokens: Option<u32>,
    pub effort: Option<crate::effort::EffortLevel>,
    pub retry_policy: Option<RetryPolicy>,
    pub fallback_model: Option<ModelId>,
    pub hook_executor: Option<Arc<HookExecutor>>,
    pub session_id: Option<String>,
    pub source: QuerySource,
}

/// The query engine — orchestrates LLM calls, tool execution, retries.
///
/// Holds all the pieces needed to run a multi-turn conversation loop.
/// Created once per session, used for every query.
pub struct QueryEngine {
    pub backend: Arc<LlmBackend>,
    pub executor: ToolExecutor,
    pub tool_ctx: ToolContext,
    pub config: QueryEngineConfig,
    pub cost: CostAccumulator,
    pub event_tx: mpsc::Sender<Event>,
    pub cancel: CancellationToken,
}

impl QueryEngine {
    /// Create a new query engine.
    pub fn new(
        config: QueryEngineConfig,
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

    /// Run the full query loop — delegates to `crate::query_loop::query_loop`.
    ///
    /// This is a thin wrapper that converts `QueryEngineConfig` to `QueryLoopConfig`
    /// and calls the existing implementation. Future refactoring will move the
    /// loop logic directly into this method.
    pub async fn run(&mut self, conversation: &mut Conversation) -> crab_common::Result<()> {
        let loop_config = crate::query_loop::QueryLoopConfig {
            model: self.config.model.clone(),
            max_tokens: self.config.max_tokens,
            temperature: self.config.temperature,
            tool_schemas: self.config.tool_schemas.clone(),
            cache_enabled: self.config.cache_enabled,
            _token_budget: None,
            budget_tokens: self.config.budget_tokens,
            retry_policy: self.config.retry_policy.clone(),
            hook_executor: self.config.hook_executor.clone(),
            session_id: self.config.session_id.clone(),
            effort: self.config.effort,
            fallback_model: self.config.fallback_model.clone(),
        };

        crate::query_loop::query_loop(
            conversation,
            &self.backend,
            &self.executor,
            &self.tool_ctx,
            &loop_config,
            &mut self.cost,
            self.event_tx.clone(),
            self.cancel.clone(),
        )
        .await
    }

    /// Execute post-response lifecycle hooks (memory extraction, etc.).
    ///
    /// Called after the query loop returns. Runs background tasks that
    /// don't block the next user turn. Corresponds to CC's `handleStopHooks`.
    pub fn post_response_hooks(&self, conversation: &Conversation) {
        // Fire-and-forget background tasks (non-blocking)
        let messages = conversation.messages().to_vec();
        let source = self.config.source.clone();

        tokio::spawn(async move {
            // Memory extraction (only for REPL/Agent queries, not compact/print)
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

            // Prompt suggestion (placeholder — needs LLM integration)
            // Auto-dream (placeholder — needs time/session gating)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_source_default_is_repl() {
        assert_eq!(QuerySource::default(), QuerySource::Repl);
    }

    #[test]
    fn query_source_serde_roundtrip() {
        let source = QuerySource::Agent {
            agent_id: "worker-1".into(),
        };
        let json = serde_json::to_string(&source).unwrap();
        let parsed: QuerySource = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, source);
    }
}
