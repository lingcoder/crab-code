//! Sub-agent worker with independent conversation context.
//!
//! An `AgentWorker` runs a `query_loop` in a spawned tokio task, inheriting
//! the parent's tool registry and backend but with a fresh conversation.
//! It supports timeout limits (max turns, max duration) and graceful
//! cancellation via `CancellationToken`.

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

/// Configuration for spawning a sub-agent worker.
#[derive(Clone)]
pub struct WorkerConfig {
    /// Unique worker identifier.
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

/// Result returned when a worker completes.
#[derive(Debug)]
pub struct WorkerResult {
    pub worker_id: String,
    /// The final assistant text output, if any.
    pub output: Option<String>,
    /// Whether the worker completed without errors.
    pub success: bool,
    /// Cumulative token usage during the worker's run.
    pub usage: TokenUsage,
    /// The worker's conversation history (for inspection or merging).
    pub conversation: Conversation,
}

impl WorkerResult {
    /// Create a lightweight summary (clones everything except conversation).
    ///
    /// Used by the coordinator to keep a record of completed workers
    /// without retaining the full conversation history in memory.
    #[must_use]
    pub fn clone_summary(&self) -> Self {
        Self {
            worker_id: self.worker_id.clone(),
            output: self.output.clone(),
            success: self.success,
            usage: self.usage.clone(),
            conversation: Conversation::new(self.worker_id.clone(), String::new(), 0),
        }
    }
}

/// Sub-agent worker that runs an independent query loop.
///
/// Workers inherit the parent's `LlmBackend`, `ToolExecutor`, and `ToolContext`
/// but get a fresh `Conversation` with their own system prompt. Events are
/// forwarded to the parent's event channel, prefixed with the worker ID.
pub struct AgentWorker {
    config: WorkerConfig,
    backend: Arc<LlmBackend>,
    executor: Arc<ToolExecutor>,
    tool_ctx: ToolContext,
    loop_config: QueryConfig,
    event_tx: mpsc::Sender<Event>,
    cancel: CancellationToken,
}

impl AgentWorker {
    /// Create a new sub-agent worker.
    ///
    /// The worker shares the parent's backend, executor, and tool context.
    /// It creates a fresh conversation with the provided system prompt.
    #[must_use]
    pub fn new(
        config: WorkerConfig,
        backend: Arc<LlmBackend>,
        executor: Arc<ToolExecutor>,
        tool_ctx: ToolContext,
        loop_config: QueryConfig,
        event_tx: mpsc::Sender<Event>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            config,
            backend,
            executor,
            tool_ctx,
            loop_config,
            event_tx,
            cancel,
        }
    }

    /// Spawn the worker as a background tokio task.
    ///
    /// The worker will:
    /// 1. Create a fresh conversation with its system prompt
    /// 2. Push the task prompt as the first user message
    /// 3. Run the query loop until completion, cancellation, or timeout
    /// 4. Emit `AgentWorkerCompleted` event with the result
    ///
    /// Returns a `JoinHandle` that resolves to the `WorkerResult`.
    pub fn spawn(self, task_prompt: String) -> tokio::task::JoinHandle<WorkerResult> {
        tokio::spawn(self.run(task_prompt))
    }

    /// Run the worker to completion (call directly or via `spawn`).
    pub async fn run(self, task_prompt: String) -> WorkerResult {
        let worker_id = self.config.worker_id.clone();

        // Emit start event
        let _ = self
            .event_tx
            .send(Event::AgentWorkerStarted {
                worker_id: worker_id.clone(),
                task_prompt: task_prompt.clone(),
            })
            .await;

        // Create fresh conversation
        let mut conversation = Conversation::new(
            worker_id.clone(),
            self.config.system_prompt.clone(),
            self.config.context_window,
        );
        conversation.push(Message::user(&task_prompt));

        // Set up timeout cancellation
        let timeout_cancel = CancellationToken::new();
        let combined_cancel = self.cancel.child_token();
        if let Some(max_duration) = self.config.max_duration {
            let tc = timeout_cancel.clone();
            tokio::spawn(async move {
                tokio::time::sleep(max_duration).await;
                tc.cancel();
            });
        }

        // Run the query loop with turn limiting
        let mut cost_tracker = crab_session::CostAccumulator::default();
        let result = if let Some(max_turns) = self.config.max_turns {
            run_with_turn_limit(
                &mut conversation,
                &self.backend,
                &self.executor,
                &self.tool_ctx,
                &self.loop_config,
                &mut cost_tracker,
                self.event_tx.clone(),
                combined_cancel.clone(),
                timeout_cancel,
                max_turns,
            )
            .await
        } else {
            let cancel_token = if self.config.max_duration.is_some() {
                // Merge parent cancel with timeout cancel
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
                &mut conversation,
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

        let success = result.is_ok();
        let usage = conversation.total_usage.clone();

        // Extract final assistant text from conversation
        let output = extract_last_assistant_text(&conversation);

        // Emit completion event
        let _ = self
            .event_tx
            .send(Event::AgentWorkerCompleted {
                worker_id: worker_id.clone(),
                result: output.clone(),
                success,
                usage: usage.clone(),
            })
            .await;

        WorkerResult {
            worker_id,
            output,
            success,
            usage,
            conversation,
        }
    }
}

/// Run the query loop with a maximum turn count.
///
/// Each turn consists of one LLM call + tool execution round. When `max_turns`
/// is reached, the cancellation token is triggered to stop the loop gracefully.
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
) -> crab_common::Result<()> {
    // We implement turn limiting by counting TurnStart events.
    // Since query_loop emits TurnStart at each turn, we wrap the event_tx
    // with a counting forwarder that cancels after max_turns.
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
        // Drain remaining events
        while let Some(event) = counting_rx.recv().await {
            let _ = event_tx.send(event).await;
        }
    });

    // Merge timeout cancel into the main cancel
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

/// Legacy worker stub kept for backward compatibility with existing coordinator code.
pub struct Worker {
    pub id: String,
    pub name: String,
    pub tx: mpsc::Sender<crate::message_bus::AgentMessage>,
}

impl Worker {
    #[must_use]
    pub fn new(
        id: String,
        name: String,
        tx: mpsc::Sender<crate::message_bus::AgentMessage>,
    ) -> Self {
        Self { id, name, tx }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::message::{ContentBlock, Message, Role};
    use crab_core::model::TokenUsage;

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
    }

    #[test]
    fn worker_config_no_limits() {
        let config = WorkerConfig {
            worker_id: "w2".into(),
            system_prompt: "test".into(),
            max_turns: None,
            max_duration: None,
            context_window: 200_000,
        };
        assert!(config.max_turns.is_none());
        assert!(config.max_duration.is_none());
    }

    #[test]
    fn worker_config_clone() {
        let config = WorkerConfig {
            worker_id: "w1".into(),
            system_prompt: "test".into(),
            max_turns: Some(10),
            max_duration: None,
            context_window: 200_000,
        };
        let cloned = config;
        assert_eq!(cloned.worker_id, "w1");
        assert_eq!(cloned.max_turns, Some(10));
    }

    #[test]
    fn worker_result_success() {
        let conv = Conversation::new("test".into(), "prompt".into(), 200_000);
        let result = WorkerResult {
            worker_id: "w1".into(),
            output: Some("done".into()),
            success: true,
            usage: TokenUsage::default(),
            conversation: conv,
        };
        assert!(result.success);
        assert_eq!(result.output.as_deref(), Some("done"));
    }

    #[test]
    fn worker_result_failure() {
        let conv = Conversation::new("test".into(), "prompt".into(), 200_000);
        let result = WorkerResult {
            worker_id: "w1".into(),
            output: None,
            success: false,
            usage: TokenUsage::default(),
            conversation: conv,
        };
        assert!(!result.success);
        assert!(result.output.is_none());
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
        // Should find the text block
        assert_eq!(
            extract_last_assistant_text(&conv),
            Some("result text".into())
        );
    }

    #[test]
    fn legacy_worker_construction() {
        let (tx, _rx) = mpsc::channel(16);
        let worker = Worker::new("w1".into(), "Worker 1".into(), tx);
        assert_eq!(worker.id, "w1");
        assert_eq!(worker.name, "Worker 1");
    }

    #[test]
    fn agent_worker_event_variants() {
        // Verify the new Event variants compile and serialize
        let start = Event::AgentWorkerStarted {
            worker_id: "w1".into(),
            task_prompt: "do stuff".into(),
        };
        let json = serde_json::to_string(&start).unwrap();
        assert!(json.contains("AgentWorkerStarted"));

        let completed = Event::AgentWorkerCompleted {
            worker_id: "w1".into(),
            result: Some("done".into()),
            success: true,
            usage: TokenUsage::default(),
        };
        let json = serde_json::to_string(&completed).unwrap();
        assert!(json.contains("AgentWorkerCompleted"));
    }

    #[test]
    fn agent_worker_event_serde_roundtrip() {
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
        let json2 = serde_json::to_string(&parsed).unwrap();
        assert_eq!(json, json2);
    }
}
