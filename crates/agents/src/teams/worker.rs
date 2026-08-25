//! The agent loop a spawned teammate runs.
//!
//! [`AgentWorker`] drives `query_loop` against a conversation, inheriting the
//! parent's tool registry and backend. It supports timeout limits (max turns,
//! max duration) and graceful cancellation via `CancellationToken`.
//!
//! One worker serves both lifetimes: [`AgentWorker::run_turn`] takes `&self`
//! and an existing conversation, so a resident teammate reuses it across
//! messages and keeps its context, while an ephemeral teammate calls it once
//! on a fresh conversation via [`AgentWorker::run_once`].

use std::sync::Arc;
use std::time::Duration;

use crab_api::LlmBackend;
use crab_core::event::Event;
use crab_core::message::Message;
use crab_core::model::TokenUsage;
use crab_core::tool::ToolContext;
use crab_session::Conversation;
use crab_tools::executor::ToolExecutor;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crab_engine::{QueryConfig, query_loop};

/// Fire a subagent lifecycle hook in the background (fire-and-forget).
fn fire_subagent_hook(
    hook_executor: Option<&Arc<crab_hooks::HookExecutor>>,
    worker_id: &str,
    trigger: crab_hooks::HookTrigger,
) {
    let Some(hooks) = hook_executor.cloned() else {
        return;
    };
    let ctx = crab_hooks::HookContext {
        session_id: Some(worker_id.to_string()),
        ..crab_hooks::HookContext::default()
    };
    tokio::spawn(async move {
        if let Err(e) = hooks.run(trigger, &ctx).await {
            tracing::warn!(event = trigger.event_name(), error = %e, "subagent hook failed");
        }
    });
}

/// Fire a task lifecycle hook in the background (fire-and-forget).
pub(crate) fn fire_task_hook(
    hook_executor: Option<&Arc<crab_hooks::HookExecutor>>,
    task_id: &str,
    trigger: crab_hooks::HookTrigger,
) {
    let Some(hooks) = hook_executor.cloned() else {
        return;
    };
    let ctx = crab_hooks::HookContext {
        tool_input: task_id.to_string(),
        ..crab_hooks::HookContext::default()
    };
    tokio::spawn(async move {
        if let Err(e) = hooks.run(trigger, &ctx).await {
            tracing::warn!(event = trigger.event_name(), error = %e, "task hook failed");
        }
    });
}

/// Configuration for a spawned teammate's agent loop.
#[derive(Clone)]
pub struct WorkerConfig {
    /// Unique worker identifier (the teammate's backend id).
    pub worker_id: String,
    /// System prompt for the worker's conversation.
    pub system_prompt: String,
    /// Maximum number of query loop turns before forced shutdown.
    pub max_turns: Option<usize>,
    /// Maximum wall-clock duration before forced shutdown.
    pub max_duration: Option<Duration>,
    /// Context window size for the worker's conversation.
    pub context_window: u64,
}

/// Result of one completed task.
///
/// The conversation stays with the teammate rather than travelling in the
/// result: a resident teammate keeps accumulating into it across messages.
#[derive(Debug, Clone)]
pub struct WorkerResult {
    pub worker_id: String,
    /// Addressable teammate name, for rendering the result back to the parent.
    pub name: String,
    /// The final assistant text output, if any.
    pub output: Option<String>,
    /// Whether the worker completed without errors.
    pub success: bool,
    /// Error detail when `success` is false, so the parent can distinguish a
    /// worker that did nothing from one that crashed.
    pub error: Option<String>,
    /// Cumulative token usage during the worker's run.
    pub usage: TokenUsage,
}

/// Runs a spawned teammate's independent query loop.
///
/// Workers inherit the parent's `LlmBackend`, `ToolExecutor`, and
/// `ToolContext` but get their own `Conversation` and system prompt. Events
/// are forwarded to the parent's event channel, tagged with the worker ID.
pub struct AgentWorker {
    config: WorkerConfig,
    name: String,
    backend: Arc<LlmBackend>,
    executor: Arc<ToolExecutor>,
    tool_ctx: ToolContext,
    loop_config: QueryConfig,
    event_tx: mpsc::Sender<Event>,
    cancel: CancellationToken,
}

impl AgentWorker {
    /// Create a worker. `name` is the addressable teammate name; for an
    /// ephemeral spawn it is the same as the worker id.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: WorkerConfig,
        name: String,
        backend: Arc<LlmBackend>,
        executor: Arc<ToolExecutor>,
        tool_ctx: ToolContext,
        loop_config: QueryConfig,
        event_tx: mpsc::Sender<Event>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            config,
            name,
            backend,
            executor,
            tool_ctx,
            loop_config,
            event_tx,
            cancel,
        }
    }

    /// Build a fresh conversation matching this worker's prompt and window.
    #[must_use]
    pub fn new_conversation(&self) -> Conversation {
        Conversation::new(
            self.config.worker_id.clone(),
            self.config.system_prompt.clone(),
            self.config.context_window,
        )
    }

    /// Run one task against `conversation`, appending to it.
    ///
    /// Resident teammates call this repeatedly on the same conversation, so
    /// context accumulates across messages. Ephemeral teammates call it once.
    pub async fn run_turn(
        &self,
        conversation: &mut Conversation,
        task_prompt: String,
    ) -> WorkerResult {
        let worker_id = self.config.worker_id.clone();

        let _ = self
            .event_tx
            .send(Event::AgentWorkerStarted {
                worker_id: worker_id.clone(),
                task_prompt: task_prompt.clone(),
            })
            .await;
        fire_subagent_hook(
            self.loop_config.hook_executor.as_ref(),
            &worker_id,
            crab_hooks::HookTrigger::SubagentStart,
        );

        conversation.push(Message::user(&task_prompt));

        // Set up timeout cancellation.
        let timeout_cancel = CancellationToken::new();
        let combined_cancel = self.cancel.child_token();
        if let Some(max_duration) = self.config.max_duration {
            let tc = timeout_cancel.clone();
            tokio::spawn(async move {
                tokio::time::sleep(max_duration).await;
                tc.cancel();
            });
        }

        let mut cost_tracker = crab_session::CostAccumulator::default();
        let result = if let Some(max_turns) = self.config.max_turns {
            run_with_turn_limit(
                conversation,
                &self.backend,
                &self.executor,
                &self.tool_ctx,
                &self.loop_config,
                &mut cost_tracker,
                self.event_tx.clone(),
                combined_cancel,
                timeout_cancel,
                max_turns,
            )
            .await
        } else {
            let cancel_token = if self.config.max_duration.is_some() {
                let merged = combined_cancel.clone();
                let tc = timeout_cancel;
                tokio::spawn(async move {
                    tc.cancelled().await;
                    merged.cancel();
                });
                combined_cancel
            } else {
                combined_cancel
            };

            query_loop::query_loop(
                conversation,
                &self.backend,
                &self.executor,
                &self.tool_ctx,
                &self.loop_config,
                &mut cost_tracker,
                self.event_tx.clone(),
                cancel_token,
            )
            .await
        };

        let error = result.err().map(|e| e.to_string());
        let success = error.is_none();
        let usage = conversation.total_usage.clone();
        let output = extract_last_assistant_text(conversation);

        let _ = self
            .event_tx
            .send(Event::AgentWorkerCompleted {
                worker_id: worker_id.clone(),
                result: output.clone(),
                success,
                usage: usage.clone(),
            })
            .await;
        fire_subagent_hook(
            self.loop_config.hook_executor.as_ref(),
            &worker_id,
            crab_hooks::HookTrigger::SubagentStop,
        );
        fire_task_hook(
            self.loop_config.hook_executor.as_ref(),
            &worker_id,
            crab_hooks::HookTrigger::TaskCompleted,
        );

        WorkerResult {
            worker_id,
            name: self.name.clone(),
            output,
            success,
            error,
            usage,
        }
    }

    /// Run a single task on a fresh conversation — the ephemeral path.
    pub async fn run_once(&self, task_prompt: String) -> WorkerResult {
        let mut conversation = self.new_conversation();
        self.run_turn(&mut conversation, task_prompt).await
    }
}

/// Run the query loop with a maximum turn count.
///
/// Each turn is one LLM call + tool execution round. When `max_turns` is
/// reached, the cancellation token is triggered to stop the loop gracefully.
#[allow(clippy::too_many_arguments)]
async fn run_with_turn_limit(
    conversation: &mut Conversation,
    backend: &LlmBackend,
    executor: &ToolExecutor,
    tool_ctx: &ToolContext,
    config: &QueryConfig,
    cost_tracker: &mut crab_session::CostAccumulator,
    event_tx: mpsc::Sender<Event>,
    cancel: CancellationToken,
    timeout_cancel: CancellationToken,
    max_turns: usize,
) -> crab_core::Result<()> {
    // Turn limiting counts TurnStart events: wrap event_tx with a counting
    // forwarder that cancels once the budget is spent.
    let (counting_tx, mut counting_rx) = mpsc::channel::<Event>(256);
    let turn_cancel = cancel.clone();

    tokio::spawn(async move {
        let mut turn_count = 0usize;
        while let Some(event) = counting_rx.recv().await {
            if let Event::TurnStart { .. } = &event {
                turn_count += 1;
                if turn_count > max_turns {
                    turn_cancel.cancel();
                    break;
                }
            }
            if event_tx.send(event).await.is_err() {
                break;
            }
        }
        while let Some(event) = counting_rx.recv().await {
            let _ = event_tx.send(event).await;
        }
    });

    if timeout_cancel.is_cancelled() {
        cancel.cancel();
    } else {
        let c = cancel.clone();
        tokio::spawn(async move {
            timeout_cancel.cancelled().await;
            c.cancel();
        });
    }

    query_loop::query_loop(
        conversation,
        backend,
        executor,
        tool_ctx,
        config,
        cost_tracker,
        counting_tx,
        cancel,
    )
    .await
}

/// Extract the last assistant text block from a conversation.
fn extract_last_assistant_text(conversation: &Conversation) -> Option<String> {
    use crab_core::message::{ContentBlock, Role};
    conversation
        .messages()
        .iter()
        .rev()
        .find(|m| m.role == Role::Assistant)
        .and_then(|m| {
            m.content.iter().find_map(|block| {
                if let ContentBlock::Text { text } = block {
                    Some(text.clone())
                } else {
                    None
                }
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::message::{ContentBlock, Message, Role};
    use crab_core::model::TokenUsage;

    fn sample_result(worker_id: &str, success: bool) -> WorkerResult {
        WorkerResult {
            worker_id: worker_id.into(),
            name: worker_id.into(),
            output: success.then(|| "done".to_string()),
            success,
            error: (!success).then(|| "query aborted".to_string()),
            usage: TokenUsage::default(),
        }
    }

    #[test]
    fn worker_config_construction() {
        let config = WorkerConfig {
            worker_id: "w1".into(),
            system_prompt: "You are a helper.".into(),
            max_turns: Some(5),
            max_duration: Some(Duration::from_secs(30)),
            context_window: 100_000,
        };
        assert_eq!(config.worker_id, "w1");
        assert_eq!(config.max_turns, Some(5));
        assert_eq!(config.max_duration, Some(Duration::from_secs(30)));

        let cloned = config.clone();
        assert_eq!(cloned.worker_id, config.worker_id);
        assert_eq!(cloned.context_window, config.context_window);
        assert_eq!(cloned.max_turns, config.max_turns);
    }

    #[test]
    fn worker_result_success_and_failure() {
        let ok = sample_result("w1", true);
        assert!(ok.success);
        assert_eq!(ok.output.as_deref(), Some("done"));

        let failed = sample_result("w2", false);
        assert!(!failed.success);
        assert!(failed.output.is_none());
        assert_eq!(failed.error.as_deref(), Some("query aborted"));
    }

    #[test]
    fn worker_result_is_cloneable() {
        let original = sample_result("w1", true);
        let cloned = original.clone();
        assert_eq!(cloned.worker_id, original.worker_id);
        assert_eq!(cloned.name, original.name);
        assert_eq!(cloned.output, original.output);
        assert_eq!(cloned.success, original.success);
    }

    #[test]
    fn extract_last_assistant_text_found() {
        let mut conv = Conversation::new("test".into(), "prompt".into(), 200_000);
        conv.push(Message::user("hello"));
        conv.push(Message::new(
            Role::Assistant,
            vec![ContentBlock::text("world")],
        ));
        assert_eq!(extract_last_assistant_text(&conv), Some("world".into()));
    }

    #[test]
    fn extract_last_assistant_text_none() {
        let mut conv = Conversation::new("test".into(), "prompt".into(), 200_000);
        conv.push(Message::user("hello"));
        assert_eq!(extract_last_assistant_text(&conv), None);
    }

    #[test]
    fn extract_last_assistant_text_picks_last() {
        let mut conv = Conversation::new("test".into(), "prompt".into(), 200_000);
        conv.push(Message::new(
            Role::Assistant,
            vec![ContentBlock::text("first")],
        ));
        conv.push(Message::user("more"));
        conv.push(Message::new(
            Role::Assistant,
            vec![ContentBlock::text("second")],
        ));
        assert_eq!(extract_last_assistant_text(&conv), Some("second".into()));
    }

    #[test]
    fn extract_last_assistant_text_skips_tool_use() {
        let mut conv = Conversation::new("test".into(), "prompt".into(), 200_000);
        conv.push(Message::new(
            Role::Assistant,
            vec![
                ContentBlock::tool_use("tu_1", "bash", serde_json::json!({})),
                ContentBlock::text("result text"),
            ],
        ));
        assert_eq!(
            extract_last_assistant_text(&conv),
            Some("result text".into())
        );
    }

    #[test]
    fn agent_worker_event_serde_roundtrip() {
        let start = Event::AgentWorkerStarted {
            worker_id: "w1".into(),
            task_prompt: "do stuff".into(),
        };
        assert!(
            serde_json::to_string(&start)
                .unwrap()
                .contains("AgentWorkerStarted")
        );

        let event = Event::AgentWorkerCompleted {
            worker_id: "w1".into(),
            result: None,
            success: false,
            usage: TokenUsage {
                input_tokens: 100,
                output_tokens: 50,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            },
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(json, serde_json::to_string(&parsed).unwrap());
    }
}
