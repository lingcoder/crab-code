use std::borrow::Cow;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use crab_api::LlmBackend;
use crab_api::capabilities::StreamingUsage;
use crab_api::rate_limit::RetryPolicy;
use crab_api::streaming::StreamingToolParser;
use crab_api::types::{CacheBreakpoint, MessageRequest, StreamEvent};
use crab_core::event::Event;
use crab_core::message::{ContentBlock, Message, Role};
use crab_core::model::{ModelId, TokenUsage};
use crab_core::tool::ToolContext;
use crab_hooks::{HookAction, HookContext, HookExecutor, HookTrigger};
use crab_session::{
    AutoCompactState, CompactionClient, CompactionConfig, CompactionMode, CompactionStrategy,
    ContextAction, ContextManager, Conversation, CostAccumulator, compact_with_config,
};

use crab_tools::builtin::plan_mode::{ENTER_PLAN_MODE_TOOL_NAME, EXIT_PLAN_MODE_TOOL_NAME};
use crab_tools::executor::ToolExecutor;
use crab_tools::registry::ToolRegistry;
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::QueryConfig;

const MAX_PTL_RETRIES: u32 = 3;
const MAX_OUTPUT_TOKEN_RETRIES: u32 = 3;
const ESCALATED_MAX_TOKENS: u32 = 64_000;
const MAX_OUTPUT_TOKENS_RECOVERY: &str = "Output token limit hit. Resume directly — no apology, no recap. \
     Pick up mid-thought if that is where the cut happened. \
     Break remaining work into smaller pieces.";

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
    // Tracks which nested AGENTS.md files have been injected this query,
    // preventing re-injection across tool batches within the same session.
    let mut loaded_nested_paths: HashSet<PathBuf> = HashSet::new();
    // Tokens summed across every LLM iteration of this run. We send a single
    // `Event::MessageEnd` with this total when the run truly completes, so the
    // TUI does not flicker into Idle between tool-use turns.
    let mut accumulated_usage = crab_core::model::TokenUsage::default();

    loop {
        if cancel.is_cancelled() {
            let _ = event_tx
                .send(Event::MessageEnd {
                    usage: accumulated_usage.clone(),
                })
                .await;
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
        let (mut assistant_msg, total_usage, stop_reason, inline_ids) = match stream_with_retry(
            backend,
            req.clone(),
            &retry_policy,
            &event_tx,
            &cancel,
            Some(executor),
            Some(tool_ctx),
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
                // Try stripping image blocks first (cheap, no LLM needed)
                let stripped = strip_images(conversation);
                if stripped > 0 {
                    tracing::info!(stripped, "stripped image blocks for PTL recovery");
                    continue;
                }
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
                let _ = event_tx
                    .send(Event::StreamAborted {
                        reason: format!(
                            "Primary model overloaded, falling back to {}",
                            fallback.as_str()
                        ),
                    })
                    .await;
                let fallback_req = MessageRequest {
                    model: fallback.clone(),
                    ..req
                };
                stream_with_retry(
                    backend,
                    fallback_req,
                    &retry_policy,
                    &event_tx,
                    &cancel,
                    Some(executor),
                    Some(tool_ctx),
                )
                .await?
            }
            Err(e) => return Err(e),
        };

        // Reset PTL retry counter on success
        ptl_retries = 0;

        // Record usage against the active model (may differ from `config.model`
        // if context was upgraded to a larger-context variant).
        cost_tracker.add_usage(active_model.as_str(), &total_usage);
        conversation.record_usage(total_usage.clone());
        // Accumulate this iteration's tokens into the task total. We deliberately
        // do NOT emit `Event::MessageEnd` here on every iteration — the TUI
        // treats `MessageEnd` as the signal that the whole agent task is done
        // (state -> Idle, spinner stops, streaming ratchet resets). Emitting
        // mid-cycle (e.g. between tool_use turns or during max-tokens
        // continuation) leaves the TUI Idle while content is still arriving,
        // making subsequent turns appear frozen mid-response.
        accumulated_usage.input_tokens = accumulated_usage
            .input_tokens
            .saturating_add(total_usage.input_tokens);
        accumulated_usage.output_tokens = accumulated_usage
            .output_tokens
            .saturating_add(total_usage.output_tokens);
        accumulated_usage.cache_read_tokens = accumulated_usage
            .cache_read_tokens
            .saturating_add(total_usage.cache_read_tokens);
        accumulated_usage.cache_creation_tokens = accumulated_usage
            .cache_creation_tokens
            .saturating_add(total_usage.cache_creation_tokens);

        // Handle max_tokens truncation with escalation + continuation
        if is_max_tokens_stop(stop_reason.as_deref())
            && output_token_retries < MAX_OUTPUT_TOKEN_RETRIES
        {
            output_token_retries += 1;

            // First attempt: escalate token cap without injecting a message.
            // The model retries with a higher ceiling, no conversation change.
            if output_token_retries == 1 && effective_max_tokens < ESCALATED_MAX_TOKENS {
                effective_max_tokens = ESCALATED_MAX_TOKENS;
                let _ = event_tx
                    .send(Event::Error {
                        message: format!(
                            "Output truncated, escalating to {ESCALATED_MAX_TOKENS} tokens \
                             ({output_token_retries}/{MAX_OUTPUT_TOKEN_RETRIES})"
                        ),
                    })
                    .await;
                continue;
            }

            // Subsequent: keep truncated assistant message, inject continuation
            let _ = event_tx
                .send(Event::Error {
                    message: format!(
                        "Output truncated, injecting continuation \
                         ({output_token_retries}/{MAX_OUTPUT_TOKEN_RETRIES})"
                    ),
                })
                .await;
            if let Some(persister) = &config.session_persister {
                persister.persist_message(&assistant_msg);
            }
            conversation.push(assistant_msg);
            conversation.push(Message::user(MAX_OUTPUT_TOKENS_RECOVERY));
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
                && hr.action == HookAction::Modify
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
            let _ = event_tx
                .send(Event::MessageEnd {
                    usage: accumulated_usage.clone(),
                })
                .await;
            return Ok(());
        }

        // Execute remaining tool calls (skipping any already handled inline
        // during streaming).
        let tool_results = crate::tool_orchestration::execute_tool_calls(
            &assistant_msg,
            executor,
            tool_ctx,
            &event_tx,
            &cancel,
            config.hook_executor.as_deref(),
            config.session_id.as_deref(),
            plan_mode,
            &inline_ids,
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

        // Inject nested AGENTS.md content for files accessed in this tool batch.
        // File-reading tools (Read, Edit, Write) add paths to the trigger set
        // during execution; we drain it here and collect matching instructions.
        let triggers: Vec<PathBuf> = {
            let mut guard = tool_ctx.nested_memory_triggers.lock().await;
            guard.drain().collect()
        };
        if !triggers.is_empty() {
            let nested = crab_memory::agents_md::collect_nested_agents_md(
                &tool_ctx.working_dir,
                &triggers,
                &mut loaded_nested_paths,
                &crab_config::config::global_config_dir(),
            );
            if !nested.is_empty() {
                let mut block = String::from(
                    "# Nested Project Instructions\n\
                     (Loaded for files accessed in this turn)\n\n",
                );
                for md in &nested {
                    use std::fmt::Write;
                    let _ = writeln!(
                        block,
                        "<!-- source: {} -->\n{}\n",
                        md.source.label(),
                        md.content
                    );
                }
                let nested_msg = Message::system(block);
                if let Some(persister) = &config.session_persister {
                    persister.persist_message(&nested_msg);
                }
                conversation.push(nested_msg);
            }
        }

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
    executor: Option<&ToolExecutor>,
    tool_ctx: Option<&ToolContext>,
) -> crab_core::Result<(Message, TokenUsage, Option<String>, HashSet<String>)> {
    let mut attempt = 0u32;
    loop {
        let req_clone = req.clone();
        let inline_ctx = match (executor, tool_ctx) {
            (Some(exec), Some(tc)) => Some(InlineExecCtx {
                registry: exec.registry_arc(),
                tool_ctx: tc,
            }),
            _ => None,
        };
        match stream_response(backend, req_clone, event_tx, cancel, inline_ctx).await {
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

/// Optional context for inline tool execution during streaming.
struct InlineExecCtx<'a> {
    registry: Arc<ToolRegistry>,
    tool_ctx: &'a ToolContext,
}

/// Stream an LLM response, assembling the assistant message from SSE events.
///
/// Uses `StreamingToolParser` for incremental `tool_use` block parsing and
/// `StreamingUsage` for accurate token accumulation.
///
/// When `inline_ctx` is provided, concurrency-safe tools are spawned as soon
/// as their JSON input is fully parsed (before the stream ends). The returned
/// `HashSet` contains the IDs of tool blocks already executed inline; the
/// caller should skip these when running the post-stream batch executor.
async fn stream_response(
    backend: &LlmBackend,
    req: MessageRequest<'_>,
    event_tx: &mpsc::Sender<Event>,
    cancel: &CancellationToken,
    inline_ctx: Option<InlineExecCtx<'_>>,
) -> crab_core::Result<(Message, TokenUsage, Option<String>, HashSet<String>)> {
    let mut stream = std::pin::pin!(backend.stream_message(req));

    let mut tool_parser = StreamingToolParser::new();
    let mut usage_tracker = StreamingUsage::new();
    let mut thinking_blocks: std::collections::HashMap<usize, String> =
        std::collections::HashMap::new();

    let mut streaming_executor = inline_ctx
        .as_ref()
        .map(|_| crate::streaming::StreamingToolExecutor::new(cancel.child_token()));

    while let Some(event) = stream.next().await {
        if cancel.is_cancelled() {
            break;
        }

        let event = event.map_err(|e| crab_core::Error::Other(format!("SSE stream error: {e}")))?;

        // Update usage tracker
        usage_tracker.update(&event);

        if let Some(completed) = tool_parser.process(&event)
            && let Some(se) = &mut streaming_executor
            && let Some(ictx) = &inline_ctx
        {
            se.spawn_if_eligible(
                &completed,
                ictx.registry.clone(),
                ictx.tool_ctx.clone(),
                event_tx.clone(),
            );
        }

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
            | StreamEvent::MessageStop
            | StreamEvent::Raw { .. } => {}
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

    let inline_ids = match streaming_executor {
        Some(mut se) => {
            se.collect_all().await;
            se.spawned_ids().clone()
        }
        None => HashSet::new(),
    };

    Ok((message, total_usage, stop_reason, inline_ids))
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

    // Summarize-first: when a client is available, try semantic summarization
    // before falling back to truncation. Preserves more context per token.
    if client.is_some() {
        let summarize_config = CompactionConfig {
            mode: CompactionMode::Summarize,
            ..compaction_config.clone()
        };
        if let Ok(r) = run_compaction(
            conversation,
            client,
            &summarize_config,
            CompactionStrategy::Summarize,
        )
        .await
            && r.tokens_saved() > 0
        {
            let _ = event_tx
                .send(Event::CompactEnd {
                    after_tokens: r.tokens_after,
                    removed_messages: r.messages_removed(),
                })
                .await;
            fire_compact_hook(hook_executor, session_id);
            return;
        }
    }

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

/// Strip all image content blocks from the conversation to free token budget.
/// Returns the number of image blocks removed.
fn strip_images(conversation: &mut Conversation) -> usize {
    let mut stripped = 0;
    for msg in conversation.messages_mut() {
        msg.content.retain(|block| {
            if block.is_image() {
                stripped += 1;
                false
            } else {
                true
            }
        });
    }
    stripped
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_orchestration::{partition_tool_calls, tool_results_message};
    use crab_core::message::ContentBlock;
    use crab_core::model::ModelId;
    use crab_core::query::QuerySource;
    use crab_core::tool::ToolOutput;

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
        let ste = crate::streaming::StreamingToolExecutor::new(
            tokio_util::sync::CancellationToken::new(),
        );
        assert!(ste.is_empty());
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
    #[allow(clippy::assertions_on_constants)]
    fn recovery_constants_reasonable() {
        assert!(MAX_PTL_RETRIES >= 1);
        assert!(MAX_PTL_RETRIES <= 5);
        assert!(MAX_OUTPUT_TOKEN_RETRIES >= 1);
        assert!(MAX_OUTPUT_TOKEN_RETRIES <= 5);
        assert!(ESCALATED_MAX_TOKENS >= 16_000);
        assert!(!MAX_OUTPUT_TOKENS_RECOVERY.is_empty());
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
