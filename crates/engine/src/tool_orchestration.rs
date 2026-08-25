//! Tool call orchestration — partition, execute, and assemble results.
//!
//! Extracted from `query_loop.rs` for separation of concerns.
//! Corresponds to CC's `toolOrchestration.ts` + `toolExecution.ts`.

use std::collections::HashSet;

use crab_core::event::Event;
use crab_core::message::{ContentBlock, Message, Role};
use crab_core::tool::{ToolContext, ToolOutput};
use crab_hooks::{HookAction, HookContext, HookExecutor, HookTrigger};
use crab_tools::builtin::bash::BASH_TOOL_NAME;
use crab_tools::builtin::plan_mode::EXIT_PLAN_MODE_TOOL_NAME;
use crab_tools::executor::{StreamingOutput, ToolExecutor, apply_result_budget};
use futures::stream::{self, StreamExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const MAX_CONCURRENT_READS: usize = 10;

/// A reference to a tool call within a message.
pub struct ToolCallRef<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub input: &'a serde_json::Value,
}

/// Partition tool calls by *scheduling* only: concurrency-safe calls run in the
/// concurrent group, the rest run sequentially.
///
/// This is purely a scheduling decision and says nothing about policy — every
/// call in either group passes through the same [`invoke_tool`] gate (plan-mode,
/// hooks, permission). A non-read-only tool that declares itself
/// concurrency-safe for some input still gets plan-gated and hooked; it just
/// runs in parallel. Keeping scheduling and policy on separate axes is what
/// closes the "concurrency-safe write bypasses the gate" hole.
pub fn partition_tool_calls<'a>(
    blocks: &'a [ContentBlock],
    registry: &crab_tools::registry::ToolRegistry,
) -> (Vec<ToolCallRef<'a>>, Vec<ToolCallRef<'a>>) {
    let mut concurrent = Vec::new();
    let mut sequential = Vec::new();
    for block in blocks {
        if let ContentBlock::ToolUse { id, name, input } = block {
            let call = ToolCallRef { id, name, input };
            let is_concurrent = registry
                .get(name)
                .is_some_and(|t| t.is_concurrency_safe(input));
            if is_concurrent {
                concurrent.push(call);
            } else {
                sequential.push(call);
            }
        }
    }
    (concurrent, sequential)
}

/// Execute all tool calls from an assistant message.
///
/// Concurrency-safe calls run in parallel, the rest sequentially — but every
/// call, regardless of group, passes through the single [`invoke_tool`] gate
/// (plan-mode for non-read-only tools, `PreToolUse`/`PostToolUse` hooks,
/// permission check). Scheduling (the partition) and policy (the gate) are
/// independent: declaring a write tool concurrency-safe changes only *how* it
/// is scheduled, never *whether* it is gated.
#[allow(clippy::too_many_arguments, clippy::implicit_hasher)]
pub async fn execute_tool_calls(
    assistant_msg: &Message,
    executor: &ToolExecutor,
    ctx: &ToolContext,
    event_tx: &mpsc::Sender<Event>,
    cancel: &CancellationToken,
    hook_executor: Option<&HookExecutor>,
    session_id: Option<&str>,
    plan_mode: bool,
    skip_ids: &HashSet<String>,
) -> crab_core::Result<Vec<(String, Result<ToolOutput, crab_core::Error>)>> {
    let registry = executor.registry();
    let mut results = Vec::new();

    let (concurrent, sequential) = partition_tool_calls(&assistant_msg.content, registry);
    let concurrent: Vec<_> = concurrent
        .into_iter()
        .filter(|c| !skip_ids.contains(c.id))
        .collect();
    let sequential: Vec<_> = sequential
        .into_iter()
        .filter(|c| !skip_ids.contains(c.id))
        .collect();

    // Concurrent group: every call still passes through the full `invoke_tool`
    // gate; the only difference from the sequential group is that these run in
    // parallel under a batch child token (cancelling the query cascades here).
    if !concurrent.is_empty() {
        let batch_token = cancel.child_token();
        let read_futures: Vec<_> = concurrent
            .iter()
            .map(|call| {
                let token = batch_token.clone();
                async move {
                    let outcome = tokio::select! {
                        o = invoke_tool(
                            call, executor, ctx, event_tx,
                            hook_executor, session_id, plan_mode,
                        ) => o,
                        () = token.cancelled() => ToolOutcome {
                            id: call.id.to_string(),
                            result: Err(crab_core::Error::Other("tool cancelled".into())),
                            abort_siblings: false,
                        },
                    };
                    (outcome.id, outcome.result)
                }
            })
            .collect();

        let read_results: Vec<_> = stream::iter(read_futures)
            .buffer_unordered(MAX_CONCURRENT_READS)
            .collect()
            .await;
        results.extend(read_results);
    }

    // Sequential group: same gate, run one at a time. A child token lets the
    // query-level cancel cascade, and a failed Bash cancels remaining siblings.
    let write_batch_token = cancel.child_token();
    for call in &sequential {
        if write_batch_token.is_cancelled() {
            // Generate synthetic tool_result for all remaining writes
            // to prevent orphaned tool_use blocks (API validation error).
            for remaining in &sequential {
                if !results.iter().any(|(id, _)| id == remaining.id) {
                    let id = remaining.id.to_string();
                    let output = ToolOutput::error("[Request interrupted by user for tool use]");
                    let _ = event_tx
                        .send(Event::ToolUseStart {
                            id: id.clone(),
                            name: remaining.name.to_string(),
                            input: remaining.input.clone(),
                        })
                        .await;
                    let _ = event_tx
                        .send(Event::ToolResult {
                            id: id.clone(),
                            output: output.clone(),
                        })
                        .await;
                    results.push((id, Ok(output)));
                }
            }
            break;
        }
        let outcome = invoke_tool(
            call,
            executor,
            ctx,
            event_tx,
            hook_executor,
            session_id,
            plan_mode,
        )
        .await;
        if outcome.abort_siblings {
            tracing::debug!("bash tool error, cancelling remaining sibling writes");
            write_batch_token.cancel();
        }
        results.push((outcome.id, outcome.result));
    }

    Ok(results)
}

/// Result of putting one tool call through the gated invocation choke point.
struct ToolOutcome {
    id: String,
    result: Result<ToolOutput, crab_core::Error>,
    /// Set when this call failed in a way that should cancel the remaining
    /// sequential calls in the batch (a failed `Bash`, which often invalidates
    /// the preconditions of the writes the model queued after it).
    abort_siblings: bool,
}

/// The single gated entry EVERY tool call passes through, whether it was
/// scheduled concurrently or sequentially: plan-mode gate (for mutating tools)
/// → `PreToolUse` hook → permission-checked execution (Bash streams its output)
/// → `PostToolUse` hook. Emits the `ToolUseStart`/`ToolResult` events itself so
/// every exit path — plan-mode block, hook deny, or real execution — reports a
/// result and never leaves an orphan `tool_use`.
///
/// The plan-mode gate keys off the tool's *policy* (`is_read_only`), not the
/// scheduling partition: a concurrency-safe-but-mutating tool is still blocked
/// in plan mode. Read-only tools skip the plan gate (they cannot mutate) but
/// still run the hooks and permission check.
#[allow(clippy::too_many_arguments)]
async fn invoke_tool(
    call: &ToolCallRef<'_>,
    executor: &ToolExecutor,
    ctx: &ToolContext,
    event_tx: &mpsc::Sender<Event>,
    hook_executor: Option<&HookExecutor>,
    session_id: Option<&str>,
    plan_mode: bool,
) -> ToolOutcome {
    let id = call.id.to_string();
    let name = call.name.to_string();
    let mut input = call.input.clone();

    // A tool is plan-gated when it can mutate (not read-only). Concurrency
    // safety is irrelevant here — it governs scheduling, not policy.
    let is_read_only = executor
        .registry()
        .get(&name)
        .is_some_and(|t| t.is_read_only());

    // Plan mode gate — block every mutating tool except ExitPlanMode.
    if plan_mode && !is_read_only && name != EXIT_PLAN_MODE_TOOL_NAME {
        let output = ToolOutput::error(
            "Cannot execute write operations in plan mode. \
             Use ExitPlanMode to get approval before making changes.",
        );
        emit_tool_events(event_tx, &id, &name, &input, &output).await;
        return ToolOutcome {
            id,
            result: Ok(output),
            abort_siblings: false,
        };
    }

    // PreToolUse hook — may deny (synthesize an error result) or rewrite input.
    if let Some(hooks) = hook_executor {
        let hook_ctx = HookContext {
            tool_name: name.clone(),
            tool_input: serde_json::to_string(&input).unwrap_or_default(),
            working_dir: Some(ctx.working_dir.clone()),
            tool_output: None,
            tool_exit_code: None,
            session_id: session_id.map(String::from),
            transcript_path: None,
        };
        match hooks.run(HookTrigger::PreToolUse, &hook_ctx).await {
            Ok(hr) if hr.action == HookAction::Deny => {
                let msg = hr
                    .message
                    .unwrap_or_else(|| "blocked by pre-tool-use hook".into());
                let output = ToolOutput::error(format!("<hook-blocked> {msg}"));
                emit_tool_events(event_tx, &id, &name, &input, &output).await;
                return ToolOutcome {
                    id,
                    result: Ok(output),
                    abort_siblings: false,
                };
            }
            Ok(hr) if hr.action == HookAction::Modify => {
                if let Some(modified) = hr.modified_input {
                    input = modified;
                }
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(error = %e, "PreToolUse hook error, proceeding anyway");
            }
        }
    }

    let _ = event_tx
        .send(Event::ToolUseStart {
            id: id.clone(),
            name: name.clone(),
            input: input.clone(),
        })
        .await;

    // Streaming execution for Bash — routed through the executor so the
    // permission gate (deny-list, dangerous-command, mode, handler prompt)
    // applies. Delta forwarding stays here; the gated call drives the tool.
    let result = if name == BASH_TOOL_NAME {
        let (streaming, mut delta_rx) = StreamingOutput::channel(64);

        let event_tx_delta = event_tx.clone();
        let delta_id = id.clone();
        let delta_fwd = tokio::spawn(async move {
            while let Some(delta) = delta_rx.recv().await {
                let _ = event_tx_delta
                    .send(Event::ToolOutputDelta {
                        id: delta_id.clone(),
                        delta,
                    })
                    .await;
            }
        });

        let result = executor
            .execute_bash_streaming(input.clone(), ctx, streaming)
            .await;
        let _ = delta_fwd.await;
        result
    } else {
        executor.execute(&name, input.clone(), ctx).await
    };

    let max_chars = executor
        .registry()
        .get(&name)
        .map_or(100_000, |t| t.max_result_chars());
    let result = result.map(|o| apply_result_budget(o, max_chars, &id));

    let _ = event_tx
        .send(Event::ToolResult {
            id: id.clone(),
            output: match &result {
                Ok(o) => o.clone(),
                Err(e) => ToolOutput::error(e.to_string()),
            },
        })
        .await;

    // A failed Bash cancels the remaining sibling writes in the batch.
    let abort_siblings = name == BASH_TOOL_NAME
        && match &result {
            Ok(o) => o.is_error,
            Err(_) => true,
        };

    // PostToolUse hook — observes the final outcome.
    if let Some(hooks) = hook_executor {
        let output_text = match &result {
            Ok(o) => o.text(),
            Err(e) => e.to_string(),
        };
        let exit_code = match &result {
            Ok(o) if o.is_error => 1,
            Ok(_) => 0,
            Err(_) => 1,
        };
        let hook_ctx = HookContext {
            tool_name: name.clone(),
            tool_input: serde_json::to_string(&input).unwrap_or_default(),
            working_dir: Some(ctx.working_dir.clone()),
            tool_output: Some(output_text),
            tool_exit_code: Some(exit_code),
            session_id: session_id.map(String::from),
            transcript_path: None,
        };
        let trigger = if exit_code == 0 {
            HookTrigger::PostToolUse
        } else {
            HookTrigger::PostToolUseFailure
        };
        if let Err(e) = hooks.run(trigger, &hook_ctx).await {
            tracing::warn!(event = trigger.event_name(), error = %e, "post-tool hook error");
        }

        // Worktree lifecycle events piggyback on the tool boundary: the
        // worktree tools are the only mutation path.
        if exit_code == 0 {
            let worktree_trigger = match name.as_str() {
                n if n == crab_tools::builtin::worktree::ENTER_WORKTREE_TOOL_NAME => {
                    Some(HookTrigger::WorktreeCreate)
                }
                n if n == crab_tools::builtin::worktree::EXIT_WORKTREE_TOOL_NAME => {
                    Some(HookTrigger::WorktreeRemove)
                }
                _ => None,
            };
            if let Some(trigger) = worktree_trigger
                && let Err(e) = hooks.run(trigger, &hook_ctx).await
            {
                tracing::warn!(event = trigger.event_name(), error = %e, "worktree hook error");
            }
        }
    }

    ToolOutcome {
        id,
        result,
        abort_siblings,
    }
}

/// Emit the paired `ToolUseStart` + `ToolResult` events for a gate that
/// produced its result without running the tool (plan-mode block, hook deny).
async fn emit_tool_events(
    event_tx: &mpsc::Sender<Event>,
    id: &str,
    name: &str,
    input: &serde_json::Value,
    output: &ToolOutput,
) {
    let _ = event_tx
        .send(Event::ToolUseStart {
            id: id.to_string(),
            name: name.to_string(),
            input: input.clone(),
        })
        .await;
    let _ = event_tx
        .send(Event::ToolResult {
            id: id.to_string(),
            output: output.clone(),
        })
        .await;
}

/// Build a tool result `Message` (role: User) from tool outputs.
pub fn tool_results_message(
    results: Vec<(String, Result<ToolOutput, crab_core::Error>)>,
) -> Message {
    let content: Vec<ContentBlock> = results
        .into_iter()
        .map(|(id, result)| {
            let (text, is_error) = match result {
                Ok(output) => (output.text(), output.is_error),
                Err(e) => (e.to_string(), true),
            };
            ContentBlock::tool_result(id, text, is_error)
        })
        .collect();
    Message::new(Role::User, content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::message::Role;
    use std::sync::Arc;

    #[test]
    fn tool_results_message_builds_user_message() {
        let results = vec![
            ("tu_1".to_string(), Ok(ToolOutput::success("file contents"))),
            (
                "tu_2".to_string(),
                Err(crab_core::Error::Other("not found".into())),
            ),
        ];
        let msg = tool_results_message(results);
        assert_eq!(msg.role, Role::User);
        assert_eq!(msg.content.len(), 2);
        assert!(msg.has_tool_result());
    }

    #[test]
    fn partition_empty_blocks() {
        let registry = crab_tools::builtin::create_default_registry();
        let (reads, writes) = partition_tool_calls(&[], &registry);
        assert!(reads.is_empty());
        assert!(writes.is_empty());
    }

    #[test]
    fn child_token_cascades_from_parent() {
        let parent = CancellationToken::new();
        let child = parent.child_token();
        assert!(!child.is_cancelled());
        parent.cancel();
        assert!(child.is_cancelled());
    }

    #[test]
    fn child_cancel_does_not_affect_parent() {
        let parent = CancellationToken::new();
        let child = parent.child_token();
        child.cancel();
        assert!(!parent.is_cancelled());
        assert!(child.is_cancelled());
    }

    #[test]
    fn sibling_tokens_are_independent() {
        let parent = CancellationToken::new();
        let child_a = parent.child_token();
        let child_b = parent.child_token();
        child_a.cancel();
        assert!(!child_b.is_cancelled());
        assert!(!parent.is_cancelled());
    }

    #[test]
    fn write_batch_token_aborts_on_cancel() {
        let query_cancel = CancellationToken::new();
        let write_batch = query_cancel.child_token();
        // Simulates bash error → cancel write batch
        write_batch.cancel();
        assert!(write_batch.is_cancelled());
        // Query-level cancel is NOT affected
        assert!(!query_cancel.is_cancelled());
    }

    #[tokio::test]
    async fn orchestrator_gates_denied_bash() {
        use crab_core::permission::{PermissionMode, PermissionPolicy};
        let executor = ToolExecutor::new(std::sync::Arc::new(
            crab_tools::builtin::create_default_registry(),
        ));
        let ctx = ToolContext {
            working_dir: std::env::current_dir().unwrap(),
            permission_mode: PermissionMode::Default,
            session_id: String::new(),
            cancellation_token: CancellationToken::new(),
            permission_policy: PermissionPolicy {
                mode: PermissionMode::Default,
                allowed_tools: vec![],
                denied_tools: vec![BASH_TOOL_NAME.into()],
            },
            ext: crab_core::tool::ToolContextExt::default(),
            job_registry: None,
            nested_memory_triggers: Arc::new(tokio::sync::Mutex::new(
                std::collections::HashSet::new(),
            )),
        };
        let (event_tx, _event_rx) = mpsc::channel(64);
        let cancel = CancellationToken::new();

        // A sentinel file the command would create if it actually ran.
        let sentinel =
            std::env::temp_dir().join(format!("crab_bash_gate_{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&sentinel);
        let cmd = format!("echo gated > '{}'", sentinel.display());

        let assistant_msg = Message::new(
            Role::Assistant,
            vec![ContentBlock::tool_use(
                "tu_1",
                BASH_TOOL_NAME,
                serde_json::json!({ "command": cmd }),
            )],
        );

        let results = execute_tool_calls(
            &assistant_msg,
            &executor,
            &ctx,
            &event_tx,
            &cancel,
            None,
            None,
            false,
            &HashSet::new(),
        )
        .await
        .unwrap();

        assert_eq!(results.len(), 1);
        let output = results[0].1.as_ref().unwrap();
        assert!(output.is_error, "denied bash must report an error");
        assert!(
            !sentinel.exists(),
            "denied bash must not execute the command"
        );
        let _ = std::fs::remove_file(&sentinel);
    }

    #[tokio::test]
    async fn cancelled_writes_get_synthetic_tool_results() {
        use crab_core::permission::{PermissionMode, PermissionPolicy};
        let executor = ToolExecutor::new(std::sync::Arc::new(
            crab_tools::builtin::create_default_registry(),
        ));
        let ctx = ToolContext {
            working_dir: std::env::current_dir().unwrap(),
            permission_mode: PermissionMode::Default,
            session_id: String::new(),
            cancellation_token: CancellationToken::new(),
            permission_policy: PermissionPolicy::default(),
            ext: crab_core::tool::ToolContextExt::default(),
            job_registry: None,
            nested_memory_triggers: Arc::new(tokio::sync::Mutex::new(
                std::collections::HashSet::new(),
            )),
        };
        let (event_tx, _event_rx) = mpsc::channel(64);
        let cancel = CancellationToken::new();

        // Pre-cancel so all writes are skipped
        cancel.cancel();

        let assistant_msg = Message::new(
            Role::Assistant,
            vec![
                ContentBlock::tool_use("tu_1", "Write", serde_json::json!({"path": "a.txt"})),
                ContentBlock::tool_use("tu_2", "Edit", serde_json::json!({"path": "b.txt"})),
            ],
        );

        let results = execute_tool_calls(
            &assistant_msg,
            &executor,
            &ctx,
            &event_tx,
            &cancel,
            None,
            None,
            false,
            &HashSet::new(),
        )
        .await
        .unwrap();

        // Both writes should have synthetic interrupt results
        assert_eq!(results.len(), 2);
        for (id, result) in &results {
            assert!(id == "tu_1" || id == "tu_2", "unexpected id: {id}");
            let output = result.as_ref().unwrap();
            assert!(output.is_error);
            assert!(output.text().contains("interrupted by user"));
        }
    }

    #[tokio::test]
    async fn plan_mode_blocks_write_through_choke_point() {
        use crab_core::permission::{PermissionMode, PermissionPolicy};
        let executor = ToolExecutor::new(std::sync::Arc::new(
            crab_tools::builtin::create_default_registry(),
        ));
        let ctx = ToolContext {
            working_dir: std::env::current_dir().unwrap(),
            permission_mode: PermissionMode::Default,
            session_id: String::new(),
            cancellation_token: CancellationToken::new(),
            permission_policy: PermissionPolicy::default(),
            ext: crab_core::tool::ToolContextExt::default(),
            job_registry: None,
            nested_memory_triggers: Arc::new(tokio::sync::Mutex::new(
                std::collections::HashSet::new(),
            )),
        };
        let (event_tx, _event_rx) = mpsc::channel(64);
        let cancel = CancellationToken::new();

        // A Write submitted while plan mode is active must be blocked by the
        // gate before reaching the tool, and reported as an error result.
        let sentinel =
            std::env::temp_dir().join(format!("crab_plan_gate_{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&sentinel);
        let assistant_msg = Message::new(
            Role::Assistant,
            vec![ContentBlock::tool_use(
                "tu_1",
                "Write",
                serde_json::json!({ "file_path": sentinel.to_string_lossy(), "content": "x" }),
            )],
        );

        let results = execute_tool_calls(
            &assistant_msg,
            &executor,
            &ctx,
            &event_tx,
            &cancel,
            None,
            None,
            true, // plan_mode active
            &HashSet::new(),
        )
        .await
        .unwrap();

        assert_eq!(results.len(), 1);
        let output = results[0].1.as_ref().unwrap();
        assert!(output.is_error, "plan mode must block the write");
        assert!(output.text().contains("plan mode"));
        assert!(
            !sentinel.exists(),
            "plan-mode-blocked write must not touch the filesystem"
        );
        let _ = std::fs::remove_file(&sentinel);
    }

    /// A mutating tool that nonetheless declares itself concurrency-safe — the
    /// dangerous combination that used to slip through the gate. It records
    /// every execution so the test can prove it was (or wasn't) run.
    struct ConcurrentWriteTool {
        ran: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl crab_core::tool::Tool for ConcurrentWriteTool {
        fn name(&self) -> &'static str {
            "concurrent_write"
        }
        fn description(&self) -> &'static str {
            "mutating but concurrency-safe"
        }
        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
        fn execute(
            &self,
            _input: serde_json::Value,
            _ctx: &ToolContext,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = crab_core::Result<ToolOutput>> + Send + '_>,
        > {
            let ran = Arc::clone(&self.ran);
            Box::pin(async move {
                ran.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(ToolOutput::success("mutated"))
            })
        }
        // NOT read-only — it mutates.
        fn is_read_only(&self) -> bool {
            false
        }
        // …yet it claims concurrency safety, so it partitions into the
        // concurrent group. The gate must still plan-block it.
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }
    }

    fn ctx_default() -> ToolContext {
        use crab_core::permission::{PermissionMode, PermissionPolicy};
        ToolContext {
            working_dir: std::env::current_dir().unwrap(),
            permission_mode: PermissionMode::Default,
            session_id: String::new(),
            cancellation_token: CancellationToken::new(),
            permission_policy: PermissionPolicy::default(),
            ext: crab_core::tool::ToolContextExt::default(),
            job_registry: None,
            nested_memory_triggers: Arc::new(tokio::sync::Mutex::new(
                std::collections::HashSet::new(),
            )),
        }
    }

    #[tokio::test]
    async fn concurrency_safe_write_still_hits_plan_gate() {
        let ran = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut reg = crab_tools::registry::ToolRegistry::new();
        reg.register(Arc::new(ConcurrentWriteTool {
            ran: Arc::clone(&ran),
        }));
        let executor = ToolExecutor::new(Arc::new(reg));
        let ctx = ctx_default();
        let (event_tx, _event_rx) = mpsc::channel(64);
        let cancel = CancellationToken::new();

        let assistant_msg = Message::new(
            Role::Assistant,
            vec![ContentBlock::tool_use(
                "tu_1",
                "concurrent_write",
                serde_json::json!({}),
            )],
        );

        let results = execute_tool_calls(
            &assistant_msg,
            &executor,
            &ctx,
            &event_tx,
            &cancel,
            None,
            None,
            true, // plan mode on
            &HashSet::new(),
        )
        .await
        .unwrap();

        assert_eq!(results.len(), 1);
        let output = results[0].1.as_ref().unwrap();
        assert!(
            output.is_error && output.text().contains("plan mode"),
            "a concurrency-safe mutating tool must still be plan-gated"
        );
        assert_eq!(
            ran.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "plan-gated tool must never execute"
        );
    }

    #[tokio::test]
    async fn concurrency_safe_write_runs_when_not_in_plan_mode() {
        // Same tool, plan mode off: it should run (proving the gate blocks on
        // plan mode specifically, not on the tool being concurrency-safe).
        let ran = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut reg = crab_tools::registry::ToolRegistry::new();
        reg.register(Arc::new(ConcurrentWriteTool {
            ran: Arc::clone(&ran),
        }));
        let mut executor = ToolExecutor::new(Arc::new(reg));
        executor.set_allow_unattended(true);
        let ctx = ctx_default();
        let (event_tx, _event_rx) = mpsc::channel(64);
        let cancel = CancellationToken::new();

        let assistant_msg = Message::new(
            Role::Assistant,
            vec![ContentBlock::tool_use(
                "tu_1",
                "concurrent_write",
                serde_json::json!({}),
            )],
        );

        let results = execute_tool_calls(
            &assistant_msg,
            &executor,
            &ctx,
            &event_tx,
            &cancel,
            None,
            None,
            false, // plan mode off
            &HashSet::new(),
        )
        .await
        .unwrap();

        assert_eq!(results.len(), 1);
        assert!(!results[0].1.as_ref().unwrap().is_error);
        assert_eq!(
            ran.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "tool should run once when plan mode is off"
        );
    }
}
