//! Streaming tool executor — starts tool execution as soon as a `tool_use`
//! block's JSON is fully parsed during SSE streaming.
//!
//! Eligibility is decided per-block via [`Tool::is_concurrency_safe`]. Eligible
//! blocks are spawned immediately and run in parallel with the still-streaming
//! assistant turn; ineligible blocks are reported as deferred (the caller
//! falls back to the post-stream batch executor for them).

use std::collections::HashSet;
use std::sync::Arc;

use crab_api::streaming::CompletedToolBlock;
use crab_core::event::Event;
use crab_core::permission::PermissionDecision;
use crab_core::tool::{ToolContext, ToolOutput};
use crab_hooks::{HookExecutor, HookTrigger};
use crab_tools::executor::apply_result_budget;
use crab_tools::permission::check_permission;
use crab_tools::registry::ToolRegistry;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Streaming tool executor — spawns tool tasks eagerly as inputs complete.
///
/// Created once per query turn. Eligible (concurrency-safe) blocks are
/// dispatched via [`spawn_if_eligible`](Self::spawn_if_eligible) the moment
/// their JSON arrives; the caller awaits them all via
/// [`collect_all`](Self::collect_all) once the stream ends.
pub struct StreamingToolExecutor {
    /// Query-level cancel token; child tokens are derived per task so the
    /// whole batch unwinds when the query is cancelled.
    cancel: CancellationToken,
    /// Spawned tasks, each paired with its `tool_use` id so a panicked task
    /// still maps back to the block it was executing.
    pending: Vec<(String, JoinHandle<(String, ToolOutput)>)>,
    /// IDs of tool blocks already spawned, so the caller can skip them when
    /// running the post-stream batch for ineligible blocks.
    spawned: HashSet<String>,
}

impl StreamingToolExecutor {
    /// Create a new executor bound to a query-level cancel token.
    #[must_use]
    pub fn new(cancel: CancellationToken) -> Self {
        Self {
            cancel,
            pending: Vec::new(),
            spawned: HashSet::new(),
        }
    }

    /// Spawn `block` eagerly if its tool reports `is_concurrency_safe(&input)`.
    ///
    /// Returns `true` when a task was spawned (caller should treat the block
    /// as in-flight) and `false` when the block is ineligible, unknown, gated,
    /// or has a `PreToolUse` hook (in which case it defers to the batch).
    ///
    /// Each spawned task derives a child cancel token from `self.cancel`,
    /// emits `ToolUseStart` before invoking the tool, applies the configured
    /// result budget, and emits `ToolResult` after completion. Errors are
    /// converted into `ToolOutput::error` so the loop never sees an `Err`.
    pub fn spawn_if_eligible(
        &mut self,
        block: &CompletedToolBlock,
        registry: Arc<ToolRegistry>,
        ctx: ToolContext,
        hook_executor: Option<&HookExecutor>,
        event_tx: mpsc::Sender<Event>,
    ) -> bool {
        let Some(tool) = registry.get(&block.name) else {
            return false;
        };
        // Malformed tool input never streams inline — the batch path reports it
        // as an error tool result instead of executing a degraded call.
        let Ok(input) = &block.input else {
            return false;
        };
        if !tool.is_concurrency_safe(input) {
            return false;
        }

        // A tool with a configured PreToolUse hook must not stream inline: the
        // hook has to run before the tool. Defer it to the batch, which runs
        // the hook then the tool. The eager path here has no hook plumbing.
        if hook_executor.is_some_and(|h| h.has_matching_hook(HookTrigger::PreToolUse, &block.name))
        {
            return false;
        }

        // Gate the eager path through the same `check_permission` the executor
        // uses, but accept only an outright `Allow`: this inline pre-filter has
        // no permission handler, so `Deny`/`AskUser` fall through to the
        // post-stream batch (return false), which owns the handler and renders
        // the canonical deny/prompt outcome. This is the no-prompt arm of the
        // executor's unified permission gate.
        let decision = check_permission(
            &ctx.permission_policy,
            &block.name,
            &tool.source(),
            tool.is_read_only(),
            input,
            &ctx.working_dir,
        );
        if !matches!(decision, PermissionDecision::Allow) {
            return false;
        }

        let id = block.id.clone();
        let name = block.name.clone();
        let input = input.clone();
        let max_chars = tool.max_result_chars();
        let task_token = self.cancel.child_token();

        let handle = tokio::spawn(async move {
            let _ = event_tx
                .send(Event::ToolUseStart {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                })
                .await;

            let tool = registry.get(&name).expect("tool checked above");
            let result = tokio::select! {
                r = tool.execute(input, &ctx) => r,
                () = task_token.cancelled() => {
                    Err(crab_core::Error::Other("tool cancelled".into()))
                }
            };

            let output = match result {
                Ok(o) => apply_result_budget(o, max_chars, &id),
                Err(e) => ToolOutput::error(e.to_string()),
            };

            let _ = event_tx
                .send(Event::ToolResult {
                    id: id.clone(),
                    output: output.clone(),
                })
                .await;

            (id, output)
        });

        self.pending.push((block.id.clone(), handle));
        self.spawned.insert(block.id.clone());
        true
    }

    /// Await all spawned tasks and collect their `(id, output)` results.
    ///
    /// Drains the internal handle list. Panicked tasks surface as a synthetic
    /// `ToolOutput::error("task panicked: …")` rather than propagating the
    /// panic — the loop must always be able to assemble a tool-result
    /// message for every `tool_use` block in the assistant turn.
    pub async fn collect_all(&mut self) -> Vec<(String, ToolOutput)> {
        let mut results = Vec::with_capacity(self.pending.len());
        for (id, handle) in self.pending.drain(..) {
            match handle.await {
                Ok(pair) => results.push(pair),
                Err(join_err) => {
                    let output = ToolOutput::error(format!("task panicked: {join_err}"));
                    results.push((id, output));
                }
            }
        }
        results
    }

    /// Abort every in-flight task and drain the handle list without awaiting
    /// results. Used when a stream attempt fails and will be retried: the tasks
    /// spawned during the doomed attempt must not survive to emit stale
    /// `ToolResult` events against the next attempt.
    pub fn abort_all(&mut self) {
        self.cancel.cancel();
        for (_, handle) in self.pending.drain(..) {
            handle.abort();
        }
        self.spawned.clear();
    }

    /// IDs of `tool_use` blocks already dispatched in this turn.
    ///
    /// The caller uses this set to skip blocks already in flight when it
    /// runs the post-stream batch executor for ineligible blocks.
    #[must_use]
    pub fn spawned_ids(&self) -> &HashSet<String> {
        &self.spawned
    }

    /// Whether any tasks have been spawned in this turn.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Number of tasks currently in flight (not yet collected).
    #[must_use]
    pub fn len(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::permission::{PermissionMode, PermissionPolicy};
    use crab_core::tool::{Tool, ToolContextExt};
    use serde_json::Value;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn test_ctx() -> ToolContext {
        ToolContext {
            working_dir: std::env::temp_dir(),
            permission_mode: PermissionMode::Default,
            session_id: String::new(),
            cancellation_token: CancellationToken::new(),
            permission_policy: PermissionPolicy::default(),
            ext: ToolContextExt::default(),
            job_registry: None,
            nested_memory_triggers: Arc::new(tokio::sync::Mutex::new(
                std::collections::HashSet::new(),
            )),
        }
    }

    /// Minimal tool whose concurrency-safety + behaviour are configurable.
    struct TestTool {
        name: &'static str,
        concurrency_safe: bool,
        call_count: Arc<AtomicUsize>,
    }

    impl Tool for TestTool {
        fn name(&self) -> &'static str {
            self.name
        }
        fn description(&self) -> &'static str {
            "test tool"
        }
        fn input_schema(&self) -> Value {
            serde_json::json!({"type": "object"})
        }
        fn execute(
            &self,
            _input: Value,
            _ctx: &ToolContext,
        ) -> Pin<Box<dyn Future<Output = crab_core::Result<ToolOutput>> + Send + '_>> {
            let counter = Arc::clone(&self.call_count);
            Box::pin(async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(ToolOutput::success("ok"))
            })
        }
        fn is_concurrency_safe(&self, _input: &Value) -> bool {
            self.concurrency_safe
        }
        fn is_read_only(&self) -> bool {
            // Concurrency-safe test tools model read-only tools, which the
            // permission gate always allows to stream inline.
            self.concurrency_safe
        }
    }

    fn make_registry(tool: TestTool) -> Arc<ToolRegistry> {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(tool));
        Arc::new(registry)
    }

    fn block(id: &str, name: &str) -> CompletedToolBlock {
        CompletedToolBlock {
            id: id.into(),
            name: name.into(),
            input: Ok(serde_json::json!({})),
        }
    }

    fn malformed_block(id: &str, name: &str) -> CompletedToolBlock {
        CompletedToolBlock {
            id: id.into(),
            name: name.into(),
            input: Err("tool input JSON was malformed or truncated".into()),
        }
    }

    #[tokio::test]
    async fn new_starts_empty() {
        let exec = StreamingToolExecutor::new(CancellationToken::new());
        assert!(exec.is_empty());
        assert_eq!(exec.len(), 0);
        assert!(exec.spawned_ids().is_empty());
    }

    #[tokio::test]
    async fn spawn_if_eligible_skips_unknown_tool() {
        let registry = make_registry(TestTool {
            name: "known",
            concurrency_safe: true,
            call_count: Arc::new(AtomicUsize::new(0)),
        });
        let (tx, _rx) = mpsc::channel::<Event>(8);
        let mut ste = StreamingToolExecutor::new(CancellationToken::new());

        let spawned = ste.spawn_if_eligible(
            &block("tu_1", "missing"),
            Arc::clone(&registry),
            test_ctx(),
            None,
            tx,
        );
        assert!(!spawned);
        assert!(ste.is_empty());
        assert!(ste.spawned_ids().is_empty());
    }

    #[tokio::test]
    async fn spawn_if_eligible_skips_unsafe_tool() {
        let registry = make_registry(TestTool {
            name: "writer",
            concurrency_safe: false,
            call_count: Arc::new(AtomicUsize::new(0)),
        });
        let (tx, _rx) = mpsc::channel::<Event>(8);
        let mut ste = StreamingToolExecutor::new(CancellationToken::new());

        let spawned = ste.spawn_if_eligible(
            &block("tu_1", "writer"),
            Arc::clone(&registry),
            test_ctx(),
            None,
            tx,
        );
        assert!(!spawned);
        assert!(ste.is_empty());
    }

    #[tokio::test]
    async fn spawn_if_eligible_dispatches_safe_tool() {
        let calls = Arc::new(AtomicUsize::new(0));
        let registry = make_registry(TestTool {
            name: "reader",
            concurrency_safe: true,
            call_count: Arc::clone(&calls),
        });
        let (tx, mut rx) = mpsc::channel::<Event>(8);
        let mut ste = StreamingToolExecutor::new(CancellationToken::new());

        let spawned = ste.spawn_if_eligible(
            &block("tu_1", "reader"),
            Arc::clone(&registry),
            test_ctx(),
            None,
            tx,
        );
        assert!(spawned);
        assert_eq!(ste.len(), 1);
        assert!(ste.spawned_ids().contains("tu_1"));

        let results = ste.collect_all().await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "tu_1");
        assert!(!results[0].1.is_error);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Both ToolUseStart and ToolResult should have been emitted.
        let mut saw_start = false;
        let mut saw_result = false;
        while let Ok(ev) = rx.try_recv() {
            match ev {
                Event::ToolUseStart { id, .. } if id == "tu_1" => saw_start = true,
                Event::ToolResult { id, .. } if id == "tu_1" => saw_result = true,
                _ => {}
            }
        }
        assert!(saw_start, "ToolUseStart not emitted");
        assert!(saw_result, "ToolResult not emitted");
    }

    #[tokio::test]
    async fn spawn_if_eligible_skips_denied_tool() {
        let registry = make_registry(TestTool {
            name: "reader",
            concurrency_safe: true,
            call_count: Arc::new(AtomicUsize::new(0)),
        });
        let (tx, _rx) = mpsc::channel::<Event>(8);
        let mut ste = StreamingToolExecutor::new(CancellationToken::new());
        let mut ctx = test_ctx();
        ctx.permission_policy.denied_tools = vec!["reader".into()];

        // Even though the tool is concurrency-safe, a deny rule must keep it off
        // the eager inline path (it falls through to the gated batch executor).
        let spawned = ste.spawn_if_eligible(
            &block("tu_1", "reader"),
            Arc::clone(&registry),
            ctx,
            None,
            tx,
        );
        assert!(!spawned, "denied read-only tool must not stream inline");
        assert!(ste.is_empty());
    }

    #[tokio::test]
    async fn cancellation_aborts_in_flight_task() {
        /// A tool that blocks until its inner cancellation token fires.
        struct SlowTool;
        impl Tool for SlowTool {
            fn name(&self) -> &'static str {
                "slow"
            }
            fn description(&self) -> &'static str {
                "slow"
            }
            fn input_schema(&self) -> Value {
                serde_json::json!({})
            }
            fn execute(
                &self,
                _input: Value,
                ctx: &ToolContext,
            ) -> Pin<Box<dyn Future<Output = crab_core::Result<ToolOutput>> + Send + '_>>
            {
                let token = ctx.cancellation_token.clone();
                Box::pin(async move {
                    token.cancelled().await;
                    Err(crab_core::Error::Other("inner cancel".into()))
                })
            }
            fn is_concurrency_safe(&self, _input: &Value) -> bool {
                true
            }
            fn is_read_only(&self) -> bool {
                true
            }
        }

        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(SlowTool));
        let registry = Arc::new(registry);
        let cancel = CancellationToken::new();
        let mut ste = StreamingToolExecutor::new(cancel.clone());
        let (tx, _rx) = mpsc::channel::<Event>(8);

        let mut ctx = test_ctx();
        ctx.cancellation_token = cancel.child_token();

        let spawned = ste.spawn_if_eligible(
            &block("tu_slow", "slow"),
            Arc::clone(&registry),
            ctx,
            None,
            tx,
        );
        assert!(spawned);

        // Cancel and ensure collection still completes (with an error result).
        cancel.cancel();
        let results = ste.collect_all().await;
        assert_eq!(results.len(), 1);
        assert!(results[0].1.is_error);
    }

    #[tokio::test]
    async fn spawn_if_eligible_skips_malformed_input() {
        let registry = make_registry(TestTool {
            name: "reader",
            concurrency_safe: true,
            call_count: Arc::new(AtomicUsize::new(0)),
        });
        let (tx, _rx) = mpsc::channel::<Event>(8);
        let mut ste = StreamingToolExecutor::new(CancellationToken::new());

        // A block whose JSON failed to parse must never stream inline; it falls
        // through to the batch path which reports the error as a tool result.
        let spawned = ste.spawn_if_eligible(
            &malformed_block("tu_1", "reader"),
            Arc::clone(&registry),
            test_ctx(),
            None,
            tx,
        );
        assert!(!spawned, "malformed input must not stream inline");
        assert!(ste.is_empty());
        assert!(ste.spawned_ids().is_empty());
    }

    #[tokio::test]
    async fn spawn_if_eligible_defers_tool_with_pre_hook() {
        use crab_hooks::{HookDef, HookExecutor, HookTrigger};

        let registry = make_registry(TestTool {
            name: "reader",
            concurrency_safe: true,
            call_count: Arc::new(AtomicUsize::new(0)),
        });
        let (tx, _rx) = mpsc::channel::<Event>(8);
        let mut ste = StreamingToolExecutor::new(CancellationToken::new());

        // A PreToolUse hook scoped to "reader" must keep it off the eager path
        // (the hook has to run before the tool), even though it is read-only,
        // concurrency-safe, and permission-allowed.
        let hooks = HookExecutor::with_hooks(vec![HookDef {
            trigger: HookTrigger::PreToolUse,
            command: "guard".into(),
            timeout_secs: 10,
            matcher: Some("reader".into()),
        }]);

        let spawned = ste.spawn_if_eligible(
            &block("tu_1", "reader"),
            Arc::clone(&registry),
            test_ctx(),
            Some(&hooks),
            tx.clone(),
        );
        assert!(
            !spawned,
            "a tool with a PreToolUse hook must not stream inline"
        );
        assert!(ste.is_empty());

        // Same tool, no matching hook → it streams inline as before.
        let unrelated = HookExecutor::with_hooks(vec![HookDef {
            trigger: HookTrigger::PreToolUse,
            command: "guard".into(),
            timeout_secs: 10,
            matcher: Some("other_tool".into()),
        }]);
        let spawned = ste.spawn_if_eligible(
            &block("tu_2", "reader"),
            Arc::clone(&registry),
            test_ctx(),
            Some(&unrelated),
            tx,
        );
        assert!(spawned, "no matching hook → inline path is taken");
    }

    #[tokio::test]
    async fn abort_all_clears_pending_and_spawned() {
        /// Tool that never completes on its own.
        struct HangTool;
        impl Tool for HangTool {
            fn name(&self) -> &'static str {
                "hang"
            }
            fn description(&self) -> &'static str {
                "hang"
            }
            fn input_schema(&self) -> Value {
                serde_json::json!({})
            }
            fn execute(
                &self,
                _input: Value,
                _ctx: &ToolContext,
            ) -> Pin<Box<dyn Future<Output = crab_core::Result<ToolOutput>> + Send + '_>>
            {
                Box::pin(async move {
                    std::future::pending::<()>().await;
                    unreachable!()
                })
            }
            fn is_concurrency_safe(&self, _input: &Value) -> bool {
                true
            }
            fn is_read_only(&self) -> bool {
                true
            }
        }

        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(HangTool));
        let registry = Arc::new(registry);
        let mut ste = StreamingToolExecutor::new(CancellationToken::new());
        let (tx, _rx) = mpsc::channel::<Event>(8);

        let spawned = ste.spawn_if_eligible(
            &block("tu_hang", "hang"),
            Arc::clone(&registry),
            test_ctx(),
            None,
            tx,
        );
        assert!(spawned);
        assert_eq!(ste.len(), 1);
        assert!(ste.spawned_ids().contains("tu_hang"));

        ste.abort_all();
        assert!(ste.is_empty(), "abort_all must drain pending tasks");
        assert!(
            ste.spawned_ids().is_empty(),
            "abort_all must clear spawned ids so a retry cannot double-skip"
        );
    }
}
