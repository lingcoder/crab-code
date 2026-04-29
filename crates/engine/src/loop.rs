use std::borrow::Cow;

use crab_api::LlmBackend;
use crab_api::capabilities::StreamingUsage;
use crab_api::rate_limit::RetryPolicy;
use crab_api::streaming::StreamingToolParser;
use crab_api::types::{CacheBreakpoint, MessageRequest, StreamEvent};
use crab_core::event::Event;
use crab_core::message::{ContentBlock, Message, Role};
use crab_core::model::{ModelId, TokenUsage};
use crab_core::tool::{ToolContext, ToolOutput};
use crab_plugin::hook::{HookAction, HookContext, HookExecutor, HookTrigger};
use crab_session::{
    AutoCompactState, CompactionClient, CompactionConfig, CompactionMode, CompactionStrategy,
    ContextAction, ContextManager, Conversation, CostAccumulator, compact_with_config,
};
use crab_tools::builtin::bash::BASH_TOOL_NAME;
use crab_tools::builtin::plan_mode::{ENTER_PLAN_MODE_TOOL_NAME, EXIT_PLAN_MODE_TOOL_NAME};
use crab_tools::executor::{StreamingOutput, ToolExecutor};
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::QueryConfig;

const MAX_PTL_RETRIES: u32 = 3;
const MAX_OUTPUT_TOKEN_RETRIES: u32 = 3;
const OUTPUT_TOKEN_SCALE_FACTOR: f64 = 1.5;

/// Core agent loop: user input -> LLM SSE stream -> parse tool calls ->
/// execute tools -> serialize results -> next round.
/// Exits when the model produces a final message without tool calls.
pub async fn query_loop(
    conversation: &mut Conversation,
    backend: &LlmBackend,
    executor: &ToolExecutor,
    tool_ctx: &ToolContext,
    config: &QueryConfig,
    cost_tracker: &mut CostAccumulator,
    event_tx: mpsc::Sender<Event>,
    cancel: CancellationToken,
) -> crab_core::Result<()> {
    let metrics = crab_telemetry::MetricsCollector::new();
    let mut turn_index: usize = 0;
    let mut plan_mode = false;
    let context_mgr = ContextManager::default();
    let retry_policy = config.retry_policy.clone().unwrap_or_default();
    let mut ptl_retries: u32 = 0;
    let mut output_token_retries: u32 = 0;
    let mut effective_max_tokens = config.max_tokens;
    let mut compact_state = AutoCompactState::default();
    // Live model — may be swapped to a larger-context variant before compaction.
    let mut active_model: ModelId = config.model.clone();

    loop {
        if cancel.is_cancelled() {
            return Ok(());
        }

        // Check context usage: first try upgrading to a larger-context model
        // variant; fall through to compaction if no upgrade is available.
        try_upgrade_or_compact(
            conversation,
            &context_mgr,
            backend,
            &mut active_model,
            &event_tx,
            config.compaction_client.as_deref(),
            &config.compaction_config,
            &mut compact_state,
            config.hook_executor.as_ref(),
            config.session_id.as_deref(),
        )
        .await;

        // Emit turn start
        let _ = event_tx.send(Event::TurnStart { turn_index }).await;
        let iter_span = crab_telemetry::metrics::agent_loop_span(turn_index as u32);
        turn_index += 1;

        // Build cache breakpoints
        let cache_breakpoints = if config.cache_enabled {
            vec![CacheBreakpoint::System, CacheBreakpoint::Tools]
        } else {
            vec![]
        };

        // Select model: use plan_model when in plan mode (if configured),
        // otherwise the live `active_model` (possibly upgraded from config.model).
        let effective_model = if plan_mode {
            config.plan_model.as_ref().unwrap_or(&active_model).clone()
        } else {
            active_model.clone()
        };

        // Build the API request from conversation state
        let req = MessageRequest {
            model: effective_model,
            messages: Cow::Borrowed(conversation.messages()),
            system: Some(conversation.system_prompt.clone()),
            max_tokens: effective_max_tokens,
            tools: config.tool_schemas.clone(),
            temperature: config.temperature,
            cache_breakpoints,
            budget_tokens: config
                .effort
                .as_ref()
                .map_or(config.budget_tokens, |e| e.to_budget_tokens()),
            response_format: None,
            tool_choice: None,
        };

        // Stream the LLM response with retry support + fallback + PTL recovery
        let (mut assistant_msg, total_usage, stop_reason) = match stream_with_retry(
            backend,
            req.clone(),
            &retry_policy,
            &event_tx,
            &cancel,
        )
        .await
        {
            Ok(result) => result,
            Err(e) if is_prompt_too_long_error(&e) && ptl_retries < MAX_PTL_RETRIES => {
                ptl_retries += 1;
                let _ = event_tx
                    .send(Event::Error {
                        message: format!(
                            "Prompt too long, compacting and retrying ({ptl_retries}/{MAX_PTL_RETRIES})"
                        ),
                    })
                    .await;
                force_compact(
                    conversation,
                    &event_tx,
                    config.compaction_client.as_deref(),
                    &config.compaction_config,
                    config.hook_executor.as_ref(),
                    config.session_id.as_deref(),
                )
                .await;
                continue;
            }
            Err(e) if is_overloaded_error(&e) && config.fallback_model.is_some() => {
                let fallback = config.fallback_model.as_ref().unwrap();
                let _ = event_tx
                    .send(Event::Error {
                        message: format!(
                            "Primary model overloaded, falling back to {}",
                            fallback.as_str()
                        ),
                    })
                    .await;
                let fallback_req = MessageRequest {
                    model: fallback.clone(),
                    ..req
                };
                stream_with_retry(backend, fallback_req, &retry_policy, &event_tx, &cancel).await?
            }
            Err(e) => return Err(e),
        };

        // Reset PTL retry counter on success
        ptl_retries = 0;

        // Record usage against the active model (may differ from `config.model`
        // if context was upgraded to a larger-context variant).
        cost_tracker.add_usage(active_model.as_str(), &total_usage);
        conversation.record_usage(total_usage.clone());
        let _ = event_tx
            .send(Event::MessageEnd { usage: total_usage })
            .await;

        // Handle max_tokens truncation: increase output cap and retry
        if is_max_tokens_stop(stop_reason.as_deref())
            && output_token_retries < MAX_OUTPUT_TOKEN_RETRIES
        {
            output_token_retries += 1;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let new_max = (f64::from(effective_max_tokens) * OUTPUT_TOKEN_SCALE_FACTOR) as u32;
            let _ = event_tx
                .send(Event::Error {
                    message: format!(
                        "Output truncated (max_tokens), retrying with {new_max} tokens \
                         ({output_token_retries}/{MAX_OUTPUT_TOKEN_RETRIES})"
                    ),
                })
                .await;
            effective_max_tokens = new_max;
            continue;
        }
        // Reset on non-truncated success
        output_token_retries = 0;

        // PostSampling hook: allow hooks to rewrite the assistant text
        // before it's persisted or seen by downstream consumers. Only the
        // text is mutable — tool_use blocks stay verbatim so the tool
        // boundary contract can't be circumvented.
        if let Some(hooks) = config.hook_executor.as_deref() {
            let hook_ctx = HookContext {
                tool_name: String::new(),
                tool_input: String::new(),
                working_dir: Some(tool_ctx.working_dir.clone()),
                tool_output: Some(assistant_msg.text()),
                tool_exit_code: None,
                session_id: config.session_id.clone(),
            };
            if let Ok(hr) = hooks.run(HookTrigger::PostSampling, &hook_ctx).await
                && hr.action == crab_plugin::hook::HookAction::Modify
                && let Some(new_text) = hr.message
            {
                rewrite_assistant_text(&mut assistant_msg, &new_text);
            }
        }

        // Add assistant message to conversation
        let has_tool_use = assistant_msg.has_tool_use();
        if let Some(persister) = &config.session_persister {
            persister.persist_message(&assistant_msg);
        }
        conversation.push(assistant_msg.clone());

        // If no tool use, run stop hooks — continue if any returns Retry
        if !has_tool_use {
            if let Some(hooks) = config.hook_executor.as_deref() {
                let hook_ctx = HookContext {
                    tool_name: String::new(),
                    tool_input: String::new(),
                    working_dir: Some(tool_ctx.working_dir.clone()),
                    tool_output: Some(assistant_msg.text()),
                    tool_exit_code: None,
                    session_id: config.session_id.clone(),
                };
                if let Ok(hr) = hooks.run(HookTrigger::Stop, &hook_ctx).await
                    && hr.action == HookAction::Retry
                {
                    if let Some(msg) = hr.message {
                        conversation.push(Message::user(msg));
                    }
                    metrics.record(iter_span.finish(true));
                    continue;
                }
            }
            metrics.record(iter_span.finish(true));
            return Ok(());
        }

        // Extract tool calls and execute them (with hook integration + plan mode)
        let tool_results = crate::tool_orchestration::execute_tool_calls(
            &assistant_msg,
            executor,
            tool_ctx,
            &event_tx,
            &cancel,
            config.hook_executor.as_deref(),
            config.session_id.as_deref(),
            plan_mode,
        )
        .await?;

        // Update plan mode state based on tool calls in this turn
        for block in &assistant_msg.content {
            if let ContentBlock::ToolUse { name, .. } = block {
                match name.as_str() {
                    ENTER_PLAN_MODE_TOOL_NAME => plan_mode = true,
                    EXIT_PLAN_MODE_TOOL_NAME => plan_mode = false,
                    _ => {}
                }
            }
        }

        // Build tool result message and add to conversation
        let result_msg = crate::tool_orchestration::tool_results_message(tool_results);
        if let Some(persister) = &config.session_persister {
            persister.persist_message(&result_msg);
        }
        conversation.push(result_msg);
        metrics.record(iter_span.finish(true));
    }
}

/// Retry wrapper around `stream_response`. Retries on transient errors
/// (connection, timeout, rate limit) using the provided `RetryPolicy`.
async fn stream_with_retry(
    backend: &LlmBackend,
    req: MessageRequest<'_>,
    policy: &RetryPolicy,
    event_tx: &mpsc::Sender<Event>,
    cancel: &CancellationToken,
) -> crab_core::Result<(Message, TokenUsage, Option<String>)> {
    let mut attempt = 0u32;
    loop {
        let req_clone = req.clone();
        match stream_response(backend, req_clone, event_tx, cancel).await {
            Ok(result) => return Ok(result),
            Err(e) => {
                // Check if we should retry: only retry transient errors
                // and only if we haven't exceeded the retry limit
                let is_transient = is_transient_error(&e);
                if is_transient && attempt < policy.max_retries {
                    let delay = policy.delay_for_attempt(attempt);
                    let _ = event_tx
                        .send(Event::Error {
                            message: format!(
                                "Retrying after error (attempt {}/{}): {e}",
                                attempt + 1,
                                policy.max_retries
                            ),
                        })
                        .await;
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                } else {
                    return Err(e);
                }
            }
        }
    }
}

/// Check if a `crab_core::Error` represents a transient/retryable condition.
fn is_transient_error(err: &crab_core::Error) -> bool {
    let msg = err.to_string().to_lowercase();
    msg.contains("timeout")
        || msg.contains("timed out")
        || msg.contains("connection")
        || msg.contains("rate limit")
        || msg.contains("429")
        || msg.contains("529")
        || msg.contains("overloaded")
}

/// Check if an error specifically indicates an overloaded/rate-limited model
/// (suitable for model fallback).
fn is_overloaded_error(err: &crab_core::Error) -> bool {
    let msg = err.to_string().to_lowercase();
    msg.contains("529")
        || msg.contains("overloaded")
        || msg.contains("rate limit")
        || msg.contains("429")
}

/// Check if an error indicates the prompt exceeded the model's context window.
fn is_prompt_too_long_error(err: &crab_core::Error) -> bool {
    let msg = err.to_string().to_lowercase();
    msg.contains("prompt is too long")
        || msg.contains("prompt too long")
        || msg.contains("too many tokens")
        || msg.contains("context length exceeded")
        || msg.contains("maximum context length")
        || msg.contains("input too long")
}

/// Check if the stop reason indicates the output was truncated at `max_tokens`.
/// Replace the first `Text` block in an assistant message with `new_text`.
///
/// `PostSampling` hooks may rewrite the assistant's narrative text but
/// never `tool_use` blocks or images. If the message has no text block,
/// one is prepended — otherwise all text blocks are collapsed into the
/// first, preserving order relative to `tool_use` blocks.
fn rewrite_assistant_text(msg: &mut crab_core::message::Message, new_text: &str) {
    use crab_core::message::ContentBlock;

    let mut wrote = false;
    msg.content.retain_mut(|block| {
        if let ContentBlock::Text { text } = block {
            if wrote {
                return false;
            }
            *text = new_text.to_string();
            wrote = true;
        }
        true
    });
    if !wrote {
        msg.content.insert(
            0,
            ContentBlock::Text {
                text: new_text.to_string(),
            },
        );
    }
}

fn is_max_tokens_stop(stop_reason: Option<&str>) -> bool {
    matches!(
        stop_reason,
        Some("max_tokens" | "length" | "max_output_tokens")
    )
}

/// Stream an LLM response, assembling the assistant message from SSE events.
///
/// Uses `StreamingToolParser` for incremental `tool_use` block parsing and
/// `StreamingUsage` for accurate token accumulation.
///
/// Returns the assembled message, total usage, and stop reason.
async fn stream_response(
    backend: &LlmBackend,
    req: MessageRequest<'_>,
    event_tx: &mpsc::Sender<Event>,
    cancel: &CancellationToken,
) -> crab_core::Result<(Message, TokenUsage, Option<String>)> {
    let mut stream = std::pin::pin!(backend.stream_message(req));

    // Use StreamingToolParser for incremental tool_use parsing
    let mut tool_parser = StreamingToolParser::new();
    // Use StreamingUsage for accurate token accumulation
    let mut usage_tracker = StreamingUsage::new();
    // Track thinking content blocks by index
    let mut thinking_blocks: std::collections::HashMap<usize, String> =
        std::collections::HashMap::new();
    // Track which block indices are thinking blocks
    while let Some(event) = stream.next().await {
        if cancel.is_cancelled() {
            break;
        }

        let event = event.map_err(|e| crab_core::Error::Other(format!("SSE stream error: {e}")))?;

        // Update usage tracker
        usage_tracker.update(&event);

        // Feed event to tool parser for incremental tool_use assembly
        tool_parser.process(&event);

        match &event {
            StreamEvent::MessageStart { id, .. } => {
                let _ = event_tx.send(Event::MessageStart { id: id.clone() }).await;
            }
            StreamEvent::ContentDelta { index, delta } => {
                let _ = event_tx
                    .send(Event::ContentDelta {
                        index: *index,
                        delta: delta.clone(),
                    })
                    .await;
            }
            StreamEvent::ThinkingDelta { index, delta } => {
                thinking_blocks.entry(*index).or_default().push_str(delta);
                let _ = event_tx
                    .send(Event::ThinkingDelta {
                        index: *index,
                        delta: delta.clone(),
                    })
                    .await;
            }
            StreamEvent::ContentBlockStop { index } => {
                let _ = event_tx
                    .send(Event::ContentBlockStop { index: *index })
                    .await;
            }
            StreamEvent::Error { message } => {
                let _ = event_tx
                    .send(Event::Error {
                        message: message.clone(),
                    })
                    .await;
                return Err(crab_core::Error::Other(format!(
                    "SSE stream error: {message}"
                )));
            }
            StreamEvent::ContentBlockStart { .. }
            | StreamEvent::MessageDelta { .. }
            | StreamEvent::MessageStop => {}
        }
    }

    // Extract stop reason before consuming usage_tracker
    let stop_reason = usage_tracker.stop_reason().map(String::from);

    // Assemble content blocks into a Message using the tool parser
    let mut content: Vec<ContentBlock> = Vec::new();

    // Add thinking blocks (sorted by index to preserve order)
    let mut thinking_indices_sorted: Vec<usize> = thinking_blocks.keys().copied().collect();
    thinking_indices_sorted.sort_unstable();
    for idx in thinking_indices_sorted {
        if let Some(thinking) = thinking_blocks.remove(&idx)
            && !thinking.is_empty()
        {
            content.push(ContentBlock::Thinking { thinking });
        }
    }

    // Add text content if any
    let text = tool_parser.text();
    if !text.is_empty() {
        content.push(ContentBlock::text(text));
    }

    // Add completed tool_use blocks from the streaming parser
    for acc in tool_parser.completed_tools() {
        content.push(ContentBlock::ToolUse {
            id: acc.id.clone(),
            name: acc.name.clone(),
            input: acc.parse_input(),
        });
    }

    // Add any in-progress tools that didn't get a ContentBlockStop
    for acc in tool_parser.in_progress_tools() {
        if let Some(input) = acc.try_parse_input() {
            content.push(ContentBlock::ToolUse {
                id: acc.id.clone(),
                name: acc.name.clone(),
                input,
            });
        }
    }

    let message = Message::new(Role::Assistant, content);
    let total_usage = usage_tracker.into_usage();

    Ok((message, total_usage, stop_reason))
}

/// Check context usage; try upgrading to a larger-context model variant
/// first, and only fall through to compaction if no upgrade is available.
///
/// On `NeedsUpgrade`: if `backend.try_upgrade_context(active_model)` returns
/// `Some(new_id)`, swap `active_model` + `conversation.context_window` and
/// emit `Event::ContextUpgraded`. If `None`, fall through to compaction as
/// if the state were `NeedsCompaction`.
///
/// On `NeedsCompaction`: unchanged — uses `compact_with_config` when a
/// client is present; otherwise falls back to truncation. Respects the
/// `AutoCompactState` circuit breaker.
#[allow(clippy::cast_precision_loss, clippy::too_many_arguments)]
async fn try_upgrade_or_compact(
    conversation: &mut Conversation,
    context_mgr: &ContextManager,
    backend: &LlmBackend,
    active_model: &mut ModelId,
    event_tx: &mpsc::Sender<Event>,
    client: Option<&dyn CompactionClient>,
    compaction_config: &CompactionConfig,
    compact_state: &mut AutoCompactState,
    hook_executor: Option<&std::sync::Arc<HookExecutor>>,
    session_id: Option<&str>,
) {
    let action = context_mgr.check(conversation);
    match action {
        ContextAction::NeedsUpgrade {
            used,
            limit,
            percent: _,
        } => {
            if let Some(new_id) = backend.try_upgrade_context(active_model.as_str()) {
                let old_window = conversation.context_window;
                let new_caps = backend.model_capabilities(&new_id);
                let new_window = u64::from(new_caps.context_window);
                // Only perform the swap if the new variant actually gives us
                // more room — otherwise it would not reduce usage percent.
                if new_window > old_window {
                    let from = active_model.as_str().to_string();
                    *active_model = ModelId::from(new_id.clone());
                    conversation.context_window = new_window;
                    let _ = event_tx
                        .send(Event::ContextUpgraded {
                            from,
                            to: new_id,
                            old_window,
                            new_window,
                        })
                        .await;
                    return;
                }
            }
            // No upgrade path available — emit a warning and let the next
            // turn either compact (once usage crosses the compact threshold)
            // or continue as-is.
            let _ = event_tx
                .send(Event::TokenWarning {
                    usage_pct: used as f32 / limit as f32,
                    used,
                    limit,
                })
                .await;
        }
        ContextAction::NeedsCompaction {
            used,
            limit,
            percent,
        } => {
            if compact_state.is_circuit_broken() {
                tracing::warn!("auto-compact circuit breaker tripped, skipping compaction");
                let _ = event_tx
                    .send(Event::TokenWarning {
                        usage_pct: used as f32 / limit as f32,
                        used,
                        limit,
                    })
                    .await;
                return;
            }

            if let Some(strategy) = CompactionStrategy::for_usage(percent) {
                let before_tokens = conversation.estimated_tokens();
                let strategy_name = format!("{strategy:?}");
                let _ = event_tx
                    .send(Event::CompactStart {
                        strategy: strategy_name,
                        before_tokens,
                    })
                    .await;

                let report =
                    run_compaction(conversation, client, compaction_config, strategy).await;

                match report {
                    Ok(r) => {
                        compact_state.record_success();
                        let _ = event_tx
                            .send(Event::CompactEnd {
                                after_tokens: r.tokens_after,
                                removed_messages: r.messages_removed(),
                            })
                            .await;
                        fire_compact_hook(hook_executor, session_id);
                    }
                    Err(e) => {
                        compact_state.record_failure();
                        tracing::warn!(error = %e, "compaction failed, falling back to truncation");
                        let budget = limit * 60 / 100;
                        let removed = conversation.inner.truncate_to_budget(budget);
                        let _ = event_tx
                            .send(Event::CompactEnd {
                                after_tokens: conversation.estimated_tokens(),
                                removed_messages: removed,
                            })
                            .await;
                        fire_compact_hook(hook_executor, session_id);
                    }
                }
            } else {
                let _ = event_tx
                    .send(Event::TokenWarning {
                        usage_pct: used as f32 / limit as f32,
                        used,
                        limit,
                    })
                    .await;
            }
        }
        ContextAction::Warning { used, limit, .. } => {
            let _ = event_tx
                .send(Event::TokenWarning {
                    usage_pct: used as f32 / limit as f32,
                    used,
                    limit,
                })
                .await;
        }
        ContextAction::Ok => {}
    }
}

/// Force-compact the conversation for PTL recovery.
///
/// Uses the full compaction pipeline with `Truncate` mode to aggressively
/// free space. Falls back to raw `truncate_to_budget` if compaction fails.
/// Unlike `check_and_compact`, this always compacts regardless of usage
/// thresholds — it is only called after a confirmed prompt-too-long error.
async fn force_compact(
    conversation: &mut Conversation,
    event_tx: &mpsc::Sender<Event>,
    client: Option<&dyn CompactionClient>,
    compaction_config: &CompactionConfig,
    hook_executor: Option<&std::sync::Arc<HookExecutor>>,
    session_id: Option<&str>,
) {
    let before_tokens = conversation.estimated_tokens();
    let _ = event_tx
        .send(Event::CompactStart {
            strategy: "ptl_recovery".into(),
            before_tokens,
        })
        .await;

    // Force truncation mode for PTL recovery
    let ptl_config = CompactionConfig {
        mode: CompactionMode::Truncate,
        ..compaction_config.clone()
    };
    let report = run_compaction(
        conversation,
        client,
        &ptl_config,
        CompactionStrategy::Truncate,
    )
    .await;

    match report {
        Ok(r) => {
            let _ = event_tx
                .send(Event::CompactEnd {
                    after_tokens: r.tokens_after,
                    removed_messages: r.messages_removed(),
                })
                .await;
            fire_compact_hook(hook_executor, session_id);
        }
        Err(e) => {
            tracing::warn!(error = %e, "PTL compaction failed, using raw truncation");
            let budget = conversation.context_window * 60 / 100;
            let removed = conversation.inner.truncate_to_budget(budget);
            let _ = event_tx
                .send(Event::CompactEnd {
                    after_tokens: conversation.estimated_tokens(),
                    removed_messages: removed,
                })
                .await;
            fire_compact_hook(hook_executor, session_id);
        }
    }
}

/// Fire Compact lifecycle hook in the background (fire-and-forget).
fn fire_compact_hook(
    hook_executor: Option<&std::sync::Arc<HookExecutor>>,
    session_id: Option<&str>,
) {
    let Some(hooks) = hook_executor.cloned() else {
        return;
    };
    let ctx = HookContext {
        tool_name: String::new(),
        tool_input: String::new(),
        working_dir: None,
        tool_output: None,
        tool_exit_code: None,
        session_id: session_id.map(String::from),
    };
    tokio::spawn(async move {
        if let Err(e) = hooks.run(HookTrigger::Compact, &ctx).await {
            tracing::warn!(error = %e, "compact hook failed");
        }
    });
}

/// Run the compaction pipeline. Uses `compact_with_config` when an LLM client
/// is available; otherwise applies a strategy-appropriate non-LLM fallback.
async fn run_compaction(
    conversation: &mut Conversation,
    client: Option<&dyn CompactionClient>,
    config: &CompactionConfig,
    strategy: CompactionStrategy,
) -> crab_core::Result<crab_session::CompactionReport> {
    if let Some(client) = client {
        compact_with_config(conversation, config, client).await
    } else {
        // No LLM client — apply non-LLM strategies only
        let tokens_before = conversation.estimated_tokens();
        let messages_before = conversation.len();

        match strategy {
            CompactionStrategy::SessionMemory { .. }
            | CompactionStrategy::Snip
            | CompactionStrategy::Microcompact
            | CompactionStrategy::Summarize
            | CompactionStrategy::Hybrid { .. } => {
                // Without an LLM client, snip is the best we can do for
                // levels 1-4. Levels 2-4 need LLM calls for summarization.
                let budget = conversation.context_window * 60 / 100;
                conversation.inner.truncate_to_budget(budget);
            }
            CompactionStrategy::Truncate => {
                let budget = conversation.context_window * 50 / 100;
                conversation.inner.truncate_to_budget(budget);
            }
            CompactionStrategy::SlidingWindow { .. } => {
                let budget = conversation.context_window * 60 / 100;
                conversation.inner.truncate_to_budget(budget);
            }
        }

        Ok(crab_session::CompactionReport {
            tokens_before,
            tokens_after: conversation.estimated_tokens(),
            messages_before,
            messages_after: conversation.len(),
            strategy_used: strategy,
        })
    }
}

/// Legacy — now delegates to [`crate::tool_orchestration::execute_tool_calls`].
/// Kept for test compatibility; will be removed in future cleanup.
#[allow(dead_code)]
#[allow(clippy::too_many_arguments)]
async fn execute_tool_calls(
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

    // Partition into read-only (concurrent) and write (sequential)
    let (reads, writes) = partition_tool_calls(&assistant_msg.content, registry);

    // Execute read-only tools concurrently
    // (hooks are not run for read-only tools to avoid serialization overhead)
    if !reads.is_empty() {
        let read_futures: Vec<_> = reads
            .iter()
            .map(|call| {
                let id = call.id.to_string();
                let name = call.name.to_string();
                let input = call.input.clone();
                let event_tx = event_tx.clone();
                async move {
                    let _ = event_tx
                        .send(Event::ToolUseStart {
                            id: id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                        })
                        .await;
                    let result = executor.execute(&name, input, ctx).await;
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

        let read_results = futures::future::join_all(read_futures).await;
        results.extend(read_results);
    }

    // Execute write tools sequentially (with hook support)
    for call in &writes {
        if cancel.is_cancelled() {
            // Generate synthetic tool_result for all remaining writes
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

        // ── Plan mode gate ─────────────────────────────────────────
        // In plan mode, block write tools (except ExitPlanMode itself)
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

        // ── PreToolUse hook ─────────────────────────────────────────
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

        // Use streaming execution for Bash to get real-time output
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

            // Forward deltas as ToolOutputDelta events
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

        // ── PostToolUse hook ────────────────────────────────────────
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
            let is_read = registry.get(name).is_some_and(|t| t.is_read_only());
            if is_read {
                reads.push(call);
            } else {
                writes.push(call);
            }
        }
    }
    (reads, writes)
}

/// Streaming tool executor -- starts tool execution as soon as
/// a `tool_use` block's JSON is fully parsed during SSE streaming.
pub struct StreamingToolExecutor {
    pub pending: Vec<tokio::task::JoinHandle<(String, crab_core::Result<ToolOutput>)>>,
}

impl StreamingToolExecutor {
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
        }
    }

    /// Spawn a tool execution as soon as its input JSON is complete.
    pub fn spawn(
        &mut self,
        _id: &str,
        name: String,
        input: serde_json::Value,
        ctx: ToolContext,
        tool_fn: impl FnOnce(
            String,
            serde_json::Value,
            ToolContext,
        )
            -> tokio::task::JoinHandle<(String, crab_core::Result<ToolOutput>)>,
    ) {
        let handle = tool_fn(name, input, ctx);
        self.pending.push(handle);
    }

    /// Collect all pending tool results after `message_stop`.
    pub async fn collect_all(&mut self) -> Vec<(String, crab_core::Result<ToolOutput>)> {
        let mut results = Vec::new();
        for handle in self.pending.drain(..) {
            results.push(handle.await.expect("tool task panicked"));
        }
        results
    }
}

impl Default for StreamingToolExecutor {
    fn default() -> Self {
        Self::new()
    }
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
    use crate::tool_orchestration::{partition_tool_calls, tool_results_message};
    use crab_core::message::ContentBlock;
    use crab_core::model::ModelId;
    use crab_core::query::QuerySource;

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

        match &msg.content[0] {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "tu_1");
                assert_eq!(content, "file contents");
                assert!(!is_error);
            }
            _ => panic!("expected ToolResult"),
        }

        match &msg.content[1] {
            ContentBlock::ToolResult {
                tool_use_id,
                is_error,
                ..
            } => {
                assert_eq!(tool_use_id, "tu_2");
                assert!(is_error);
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn partition_tool_calls_empty() {
        let registry = crab_tools::registry::ToolRegistry::new();
        let blocks: Vec<ContentBlock> = vec![];
        let (reads, writes) = partition_tool_calls(&blocks, &registry);
        assert!(reads.is_empty());
        assert!(writes.is_empty());
    }

    #[test]
    fn partition_tool_calls_unknown_tools_go_to_writes() {
        let registry = crab_tools::registry::ToolRegistry::new();
        let blocks = vec![ContentBlock::tool_use(
            "tu_1",
            "unknown_tool",
            serde_json::json!({}),
        )];
        let (reads, writes) = partition_tool_calls(&blocks, &registry);
        assert!(reads.is_empty());
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].name, "unknown_tool");
    }

    #[test]
    fn partition_tool_calls_skips_non_tool_blocks() {
        let registry = crab_tools::registry::ToolRegistry::new();
        let blocks = vec![
            ContentBlock::text("some text"),
            ContentBlock::tool_use("tu_1", "bash", serde_json::json!({})),
        ];
        let (reads, writes) = partition_tool_calls(&blocks, &registry);
        assert!(reads.is_empty());
        assert_eq!(writes.len(), 1);
    }

    #[test]
    fn streaming_tool_executor_new_is_empty() {
        let ste = StreamingToolExecutor::new();
        assert!(ste.pending.is_empty());
    }

    #[test]
    fn streaming_tool_executor_default() {
        let ste = StreamingToolExecutor::default();
        assert!(ste.pending.is_empty());
    }

    #[test]
    fn query_loop_config_construction() {
        let config = QueryConfig {
            model: ModelId::from("claude-sonnet-4-20250514"),
            max_tokens: 4096,
            temperature: Some(0.7),
            tool_schemas: vec![],
            cache_enabled: false,
            budget_tokens: None,
            retry_policy: None,
            hook_executor: None,
            session_id: None,
            effort: None,
            fallback_model: None,
            plan_model: None,
            source: QuerySource::Repl,
            compaction_client: None,
            compaction_config: crab_session::CompactionConfig::default(),
            session_persister: None,
        };
        assert_eq!(config.model.as_str(), "claude-sonnet-4-20250514");
        assert_eq!(config.max_tokens, 4096);
    }

    #[test]
    fn query_loop_config_with_retry_policy() {
        let policy = RetryPolicy::aggressive();
        let config = QueryConfig {
            model: ModelId::from("claude-sonnet-4-20250514"),
            max_tokens: 4096,
            temperature: None,
            tool_schemas: vec![],
            cache_enabled: false,
            budget_tokens: None,
            retry_policy: Some(policy),
            hook_executor: None,
            session_id: None,
            effort: None,
            fallback_model: None,
            plan_model: None,
            source: QuerySource::Repl,
            compaction_client: None,
            compaction_config: crab_session::CompactionConfig::default(),
            session_persister: None,
        };
        assert!(config.retry_policy.is_some());
        assert_eq!(config.retry_policy.unwrap().max_retries, 5);
    }

    #[test]
    fn tool_results_message_single_success() {
        let results = vec![("id1".to_string(), Ok(ToolOutput::success("ok")))];
        let msg = tool_results_message(results);
        assert_eq!(msg.role, Role::User);
        assert_eq!(msg.content.len(), 1);
        match &msg.content[0] {
            ContentBlock::ToolResult {
                is_error, content, ..
            } => {
                assert!(!is_error);
                assert_eq!(content, "ok");
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn tool_results_message_single_error() {
        let results = vec![(
            "id1".to_string(),
            Ok(ToolOutput::error("something went wrong")),
        )];
        let msg = tool_results_message(results);
        match &msg.content[0] {
            ContentBlock::ToolResult {
                is_error, content, ..
            } => {
                assert!(is_error);
                assert_eq!(content, "something went wrong");
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn tool_results_message_empty() {
        let results: Vec<(String, Result<ToolOutput, crab_core::Error>)> = vec![];
        let msg = tool_results_message(results);
        assert_eq!(msg.role, Role::User);
        assert!(msg.content.is_empty());
    }

    #[test]
    fn transient_error_timeout() {
        let err = crab_core::Error::Other("request timed out".into());
        assert!(is_transient_error(&err));
    }

    #[test]
    fn transient_error_connection() {
        let err = crab_core::Error::Other("connection refused".into());
        assert!(is_transient_error(&err));
    }

    #[test]
    fn transient_error_rate_limit() {
        let err = crab_core::Error::Other("SSE stream error: rate limit exceeded 429".into());
        assert!(is_transient_error(&err));
    }

    #[test]
    fn transient_error_overloaded() {
        let err = crab_core::Error::Other("server overloaded".into());
        assert!(is_transient_error(&err));
    }

    #[test]
    fn non_transient_error_json() {
        let err = crab_core::Error::Other("invalid JSON".into());
        assert!(!is_transient_error(&err));
    }

    #[test]
    fn non_transient_error_auth() {
        let err = crab_core::Error::Other("unauthorized: invalid API key".into());
        assert!(!is_transient_error(&err));
    }

    // ─── is_overloaded_error tests ───

    #[test]
    fn overloaded_error_529() {
        let err = crab_core::Error::Other("HTTP 529: model is overloaded".into());
        assert!(is_overloaded_error(&err));
    }

    #[test]
    fn overloaded_error_429() {
        let err = crab_core::Error::Other("rate limit exceeded 429".into());
        assert!(is_overloaded_error(&err));
    }

    #[test]
    fn overloaded_error_rate_limit_text() {
        let err = crab_core::Error::Other("Rate Limit exceeded".into());
        assert!(is_overloaded_error(&err));
    }

    #[test]
    fn overloaded_error_overloaded_text() {
        let err = crab_core::Error::Other("server overloaded, try again".into());
        assert!(is_overloaded_error(&err));
    }

    #[test]
    fn not_overloaded_error_auth() {
        let err = crab_core::Error::Other("unauthorized: invalid API key".into());
        assert!(!is_overloaded_error(&err));
    }

    #[test]
    fn not_overloaded_error_json() {
        let err = crab_core::Error::Other("invalid JSON response".into());
        assert!(!is_overloaded_error(&err));
    }

    // ─── fallback_model config tests ───

    #[test]
    fn query_loop_config_with_fallback_model() {
        let config = QueryConfig {
            model: ModelId::from("claude-opus-4-20250514"),
            max_tokens: 8192,
            temperature: None,
            tool_schemas: vec![],
            cache_enabled: false,
            budget_tokens: None,
            retry_policy: None,
            hook_executor: None,
            session_id: None,
            effort: None,
            fallback_model: Some(ModelId::from("claude-sonnet-4-20250514")),
            plan_model: None,
            source: QuerySource::Repl,
            compaction_client: None,
            compaction_config: crab_session::CompactionConfig::default(),
            session_persister: None,
        };
        assert_eq!(
            config.fallback_model.as_ref().unwrap().as_str(),
            "claude-sonnet-4-20250514"
        );
    }

    // ─── is_prompt_too_long_error tests ───

    #[test]
    fn ptl_error_prompt_is_too_long() {
        let err = crab_core::Error::Other(
            "SSE stream error: prompt is too long: 250000 tokens > 200000 maximum".into(),
        );
        assert!(is_prompt_too_long_error(&err));
    }

    #[test]
    fn ptl_error_context_length_exceeded() {
        let err = crab_core::Error::Other("This model's maximum context length exceeded".into());
        assert!(is_prompt_too_long_error(&err));
    }

    #[test]
    fn ptl_error_too_many_tokens() {
        let err = crab_core::Error::Other("too many tokens in the request".into());
        assert!(is_prompt_too_long_error(&err));
    }

    #[test]
    fn ptl_error_input_too_long() {
        let err = crab_core::Error::Other("input too long for this model".into());
        assert!(is_prompt_too_long_error(&err));
    }

    #[test]
    fn not_ptl_error_other() {
        let err = crab_core::Error::Other("invalid API key".into());
        assert!(!is_prompt_too_long_error(&err));
    }

    #[test]
    fn not_ptl_error_overloaded() {
        let err = crab_core::Error::Other("server overloaded".into());
        assert!(!is_prompt_too_long_error(&err));
    }

    // ─── is_max_tokens_stop tests ───

    #[test]
    fn max_tokens_stop_anthropic() {
        assert!(is_max_tokens_stop(Some("max_tokens")));
    }

    #[test]
    fn max_tokens_stop_openai() {
        assert!(is_max_tokens_stop(Some("length")));
    }

    #[test]
    fn max_tokens_stop_max_output() {
        assert!(is_max_tokens_stop(Some("max_output_tokens")));
    }

    #[test]
    fn max_tokens_stop_end_turn() {
        assert!(!is_max_tokens_stop(Some("end_turn")));
    }

    #[test]
    fn max_tokens_stop_tool_use() {
        assert!(!is_max_tokens_stop(Some("tool_use")));
    }

    #[test]
    fn max_tokens_stop_none() {
        assert!(!is_max_tokens_stop(None));
    }

    // ─── plan_model config tests ───

    #[test]
    fn query_config_with_plan_model() {
        let config = QueryConfig {
            model: ModelId::from("claude-sonnet-4-20250514"),
            max_tokens: 4096,
            temperature: None,
            tool_schemas: vec![],
            cache_enabled: false,
            budget_tokens: None,
            retry_policy: None,
            hook_executor: None,
            session_id: None,
            effort: None,
            fallback_model: None,
            plan_model: Some(ModelId::from("claude-opus-4-20250514")),
            source: QuerySource::Repl,
            compaction_client: None,
            compaction_config: crab_session::CompactionConfig::default(),
            session_persister: None,
        };
        assert_eq!(
            config.plan_model.as_ref().unwrap().as_str(),
            "claude-opus-4-20250514"
        );
    }

    // ─── recovery constants tests ───

    #[test]
    fn recovery_constants_reasonable() {
        assert!(MAX_PTL_RETRIES >= 1);
        assert!(MAX_PTL_RETRIES <= 5);
        assert!(MAX_OUTPUT_TOKEN_RETRIES >= 1);
        assert!(MAX_OUTPUT_TOKEN_RETRIES <= 5);
        assert!(OUTPUT_TOKEN_SCALE_FACTOR > 1.0);
        assert!(OUTPUT_TOKEN_SCALE_FACTOR <= 3.0);
    }

    // ─── context upgrade tests ───

    /// An `OpenAI`-compatible backend has no upgrade concept — `NeedsUpgrade`
    /// must fall through to a `TokenWarning` rather than swap the model.
    #[tokio::test]
    async fn upgrade_on_openai_backend_emits_warning_not_swap() {
        use crab_api::openai::OpenAiClient;
        let backend = LlmBackend::OpenAi(OpenAiClient::new("https://example.invalid", None));

        let mut conv = Conversation::new("s".into(), String::new(), 100);
        // Force usage into the [upgrade..compact) window.
        conv.push_user("x".repeat(260)); // ~65 tokens of ~100 = 65%
        let ctx_mgr = ContextManager {
            warn_threshold_percent: 30,
            upgrade_threshold_percent: 50,
            compact_threshold_percent: 90,
        };

        let mut active = ModelId::from("gpt-4o");
        let (tx, mut rx) = mpsc::channel::<Event>(16);
        let mut state = AutoCompactState::default();

        try_upgrade_or_compact(
            &mut conv,
            &ctx_mgr,
            &backend,
            &mut active,
            &tx,
            None,
            &CompactionConfig::default(),
            &mut state,
            None,
            None,
        )
        .await;

        // Model must not have been swapped, context window unchanged.
        assert_eq!(active.as_str(), "gpt-4o");
        assert_eq!(conv.context_window, 100);
        drop(tx);

        let mut saw_warning = false;
        let mut saw_upgrade = false;
        while let Some(ev) = rx.recv().await {
            match ev {
                Event::TokenWarning { .. } => saw_warning = true,
                Event::ContextUpgraded { .. } => saw_upgrade = true,
                _ => {}
            }
        }
        assert!(saw_warning, "expected TokenWarning for no-upgrade path");
        assert!(!saw_upgrade, "OpenAI backend must not emit ContextUpgraded");
    }

    /// Below the upgrade threshold, nothing should happen (no events, no swap).
    #[tokio::test]
    async fn upgrade_below_threshold_is_noop() {
        use crab_api::openai::OpenAiClient;
        let backend = LlmBackend::OpenAi(OpenAiClient::new("https://example.invalid", None));

        let mut conv = Conversation::new("s".into(), String::new(), 1_000_000);
        conv.push_user("tiny message");
        let ctx_mgr = ContextManager::default();

        let mut active = ModelId::from("claude-sonnet-4-5");
        let (tx, mut rx) = mpsc::channel::<Event>(16);
        let mut state = AutoCompactState::default();

        try_upgrade_or_compact(
            &mut conv,
            &ctx_mgr,
            &backend,
            &mut active,
            &tx,
            None,
            &CompactionConfig::default(),
            &mut state,
            None,
            None,
        )
        .await;

        drop(tx);
        assert_eq!(active.as_str(), "claude-sonnet-4-5");
        assert_eq!(conv.context_window, 1_000_000);
        assert!(
            rx.recv().await.is_none(),
            "no events expected at Ok usage level"
        );
    }
}
