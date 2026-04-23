//! Tool call orchestration — partition, execute, and assemble results.
//!
//! Extracted from `query_loop.rs` for separation of concerns.
//! Corresponds to CC's `toolOrchestration.ts` + `toolExecution.ts`.

use crab_core::event::Event;
use crab_core::message::{ContentBlock, Message, Role};
use crab_core::tool::{ToolContext, ToolOutput};
use crab_plugin::hook::{HookAction, HookContext, HookExecutor, HookTrigger};
use crab_tools::builtin::bash::BASH_TOOL_NAME;
use crab_tools::builtin::plan_mode::EXIT_PLAN_MODE_TOOL_NAME;
use crab_tools::executor::{StreamingOutput, ToolExecutor};
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

/// Partition tool calls into read-only (concurrent) and write (sequential) groups.
pub fn partition_tool_calls<'a>(
    blocks: &'a [ContentBlock],
    registry: &crab_tools::registry::ToolRegistry,
) -> (Vec<ToolCallRef<'a>>, Vec<ToolCallRef<'a>>) {
    let mut reads = Vec::new();
    let mut writes = Vec::new();
    for block in blocks {
        if let ContentBlock::ToolUse { id, name, input } = block {
            let call = ToolCallRef { id, name, input };
            let is_concurrent = registry
                .get(name)
                .is_some_and(|t| t.is_concurrency_safe(input));
            if is_concurrent {
                reads.push(call);
            } else {
                writes.push(call);
            }
        }
    }
    (reads, writes)
}

/// Execute all tool calls from an assistant message.
///
/// Read-only tools run concurrently; write tools run sequentially with
/// pre/post hook support. Plan mode blocks write tools except `ExitPlanMode`.
#[allow(clippy::too_many_arguments)]
pub async fn execute_tool_calls(
    assistant_msg: &Message,
    executor: &ToolExecutor,
    ctx: &ToolContext,
    event_tx: &mpsc::Sender<Event>,
    cancel: &CancellationToken,
    hook_executor: Option<&HookExecutor>,
    session_id: Option<&str>,
    plan_mode: bool,
) -> crab_core::Result<Vec<(String, Result<ToolOutput, crab_core::Error>)>> {
    let registry = executor.registry();
    let mut results = Vec::new();

    let (reads, writes) = partition_tool_calls(&assistant_msg.content, registry);

    // Execute read-only tools concurrently under a batch child token.
    // Cancelling the parent (query-level) token cascades to all reads.
    if !reads.is_empty() {
        let batch_token = cancel.child_token();
        let read_futures: Vec<_> = reads
            .iter()
            .map(|call| {
                let id = call.id.to_string();
                let name = call.name.to_string();
                let input = call.input.clone();
                let event_tx = event_tx.clone();
                let token = batch_token.clone();
                async move {
                    let _ = event_tx
                        .send(Event::ToolUseStart {
                            id: id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                        })
                        .await;
                    let result = tokio::select! {
                        r = executor.execute(&name, input, ctx) => r,
                        () = token.cancelled() => {
                            Err(crab_core::Error::Other("tool cancelled".into()))
                        }
                    };
                    let _ = event_tx
                        .send(Event::ToolResult {
                            id: id.clone(),
                            output: match &result {
                                Ok(o) => o.clone(),
                                Err(e) => ToolOutput::error(e.to_string()),
                            },
                        })
                        .await;
                    (id, result)
                }
            })
            .collect();

        let read_results: Vec<_> = stream::iter(read_futures)
            .buffer_unordered(MAX_CONCURRENT_READS)
            .collect()
            .await;
        results.extend(read_results);
    }

    // Execute write tools sequentially (with hook support).
    // A child token lets the query-level cancel cascade to writes,
    // and a Bash error cancels remaining sibling writes in the batch.
    let write_batch_token = cancel.child_token();
    for call in &writes {
        if write_batch_token.is_cancelled() {
            // Generate synthetic tool_result for all remaining writes
            // to prevent orphaned tool_use blocks (API validation error).
            for remaining in &writes {
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
        let id = call.id.to_string();
        let name = call.name.to_string();
        let mut input = call.input.clone();

        // Plan mode gate
        if plan_mode && name != EXIT_PLAN_MODE_TOOL_NAME {
            let _ = event_tx
                .send(Event::ToolUseStart {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                })
                .await;
            let output = ToolOutput::error(
                "Cannot execute write operations in plan mode. \
                 Use ExitPlanMode to get approval before making changes.",
            );
            let _ = event_tx
                .send(Event::ToolResult {
                    id: id.clone(),
                    output: output.clone(),
                })
                .await;
            results.push((id, Ok(output)));
            continue;
        }

        // PreToolUse hook
        if let Some(hooks) = hook_executor {
            let hook_ctx = HookContext {
                tool_name: name.clone(),
                tool_input: serde_json::to_string(&input).unwrap_or_default(),
                working_dir: Some(ctx.working_dir.clone()),
                tool_output: None,
                tool_exit_code: None,
                session_id: session_id.map(String::from),
            };
            match hooks.run(HookTrigger::PreToolUse, &hook_ctx).await {
                Ok(hr) if hr.action == HookAction::Deny => {
                    let msg = hr
                        .message
                        .unwrap_or_else(|| "blocked by pre-tool-use hook".into());
                    let _ = event_tx
                        .send(Event::ToolUseStart {
                            id: id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                        })
                        .await;
                    let output = ToolOutput::error(format!("<hook-blocked> {msg}"));
                    let _ = event_tx
                        .send(Event::ToolResult {
                            id: id.clone(),
                            output: output.clone(),
                        })
                        .await;
                    results.push((id, Ok(output)));
                    continue;
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

        // Streaming execution for Bash
        let result = if name == BASH_TOOL_NAME {
            let (streaming, mut delta_rx) = StreamingOutput::channel(64);
            let bash_tool = crab_tools::builtin::bash::BashTool;
            let ctx_clone = ctx.clone();
            let input_clone = input.clone();

            let exec_handle = tokio::spawn(async move {
                bash_tool
                    .execute_streaming(input_clone, &ctx_clone, streaming)
                    .await
            });

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

            let result = exec_handle
                .await
                .unwrap_or_else(|e| Err(crab_core::Error::Other(format!("join error: {e}"))));
            let _ = delta_fwd.await;
            result
        } else {
            executor.execute(&name, input.clone(), ctx).await
        };

        let _ = event_tx
            .send(Event::ToolResult {
                id: id.clone(),
                output: match &result {
                    Ok(o) => o.clone(),
                    Err(e) => ToolOutput::error(e.to_string()),
                },
            })
            .await;

        // Bash sibling abort: if Bash failed, cancel remaining writes
        if name == BASH_TOOL_NAME {
            let is_error = match &result {
                Ok(o) => o.is_error,
                Err(_) => true,
            };
            if is_error {
                tracing::debug!("bash tool error, cancelling remaining sibling writes");
                write_batch_token.cancel();
            }
        }

        // PostToolUse hook
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
            };
            if let Err(e) = hooks.run(HookTrigger::PostToolUse, &hook_ctx).await {
                tracing::warn!(error = %e, "PostToolUse hook error");
            }
        }

        results.push((id, result));
    }

    Ok(results)
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
            ext: Default::default(),
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
}
