use std::borrow::Cow;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crab_api::LlmBackend;
use crab_api::capabilities::StreamingUsage;
use crab_api::error::ApiError;
use crab_api::error_classifier::{ErrorCategory, classify_error};
use crab_api::rate_limit::RetryPolicy;
use crab_api::streaming::StreamingToolParser;
use crab_api::types::{CacheBreakpoint, MessageRequest, StreamEvent};
use crab_core::error::ApiErrorKind;
use crab_core::event::Event;
use crab_core::message::{ContentBlock, Message, Role};
use crab_core::model::{ModelId, TokenUsage};
use crab_core::tool::{ToolContext, ToolOutput};
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

/// Mutable per-run recovery counters and live model state for the agent loop.
///
/// Grouping these makes the points where each counter is reset explicit and
/// auditable instead of scattering ~9 loose `let mut` bindings through the
/// loop body.
struct LoopState {
    turn_index: usize,
    plan_mode: bool,
    ptl_retries: u32,
    output_token_retries: u32,
    effective_max_tokens: u32,
    compact_state: AutoCompactState,
    /// Live model — may be swapped to a larger-context variant before
    /// compaction.
    active_model: ModelId,
    /// Tokens summed across every LLM iteration of this run, emitted once via
    /// `Event::MessageEnd` when the run truly completes.
    accumulated_usage: TokenUsage,
}

impl LoopState {
    fn new(config: &QueryConfig) -> Self {
        Self {
            turn_index: 0,
            plan_mode: false,
            ptl_retries: 0,
            output_token_retries: 0,
            effective_max_tokens: config.max_tokens,
            compact_state: AutoCompactState::default(),
            active_model: config.model.clone(),
            accumulated_usage: TokenUsage::default(),
        }
    }

    /// Fold one API response's usage into the run total.
    fn accumulate(&mut self, usage: &TokenUsage) {
        self.accumulated_usage.input_tokens = self
            .accumulated_usage
            .input_tokens
            .saturating_add(usage.input_tokens);
        self.accumulated_usage.output_tokens = self
            .accumulated_usage
            .output_tokens
            .saturating_add(usage.output_tokens);
        self.accumulated_usage.cache_read_tokens = self
            .accumulated_usage
            .cache_read_tokens
            .saturating_add(usage.cache_read_tokens);
        self.accumulated_usage.cache_creation_tokens = self
            .accumulated_usage
            .cache_creation_tokens
            .saturating_add(usage.cache_creation_tokens);
    }
}

/// Everything assembled from one streamed assistant turn.
struct StreamOutcome {
    message: Message,
    usage: TokenUsage,
    stop_reason: Option<String>,
    /// `(tool_use_id, output)` for tools executed eagerly during streaming.
    inline_results: Vec<(String, ToolOutput)>,
    /// `(tool_use_id, error_message)` for `tool_use` blocks whose input JSON
    /// failed to parse. These are never executed; the loop turns each into an
    /// error tool result.
    malformed_inputs: Vec<(String, String)>,
}

/// Map an `ApiError` into a structured `crab_core::Error::Api`, preserving the
/// HTTP status, a coarse `kind` for retry decisions, and any server-provided
/// retry delay. Classification reuses `api::error_classifier` for status codes.
fn map_api_error(err: &ApiError) -> crab_core::Error {
    match err {
        ApiError::Api { status, message } => {
            // 429/529 responses are surfaced upstream as `ApiError::RateLimited`
            // with the parsed `Retry-After` header, so an `Api` variant here
            // carries no retry hint of its own.
            let kind = api_kind_from_category(classify_error(*status, message), message);
            crab_core::Error::Api {
                status: Some(*status),
                kind,
                retry_after: None,
                message: message.clone(),
            }
        }
        ApiError::RateLimited { retry_after_ms } => crab_core::Error::Api {
            status: Some(429),
            kind: ApiErrorKind::RateLimited,
            retry_after: Some(Duration::from_millis(*retry_after_ms)),
            message: format!("rate limited, retry after {retry_after_ms}ms"),
        },
        ApiError::Timeout => crab_core::Error::api(ApiErrorKind::Timeout, "request timed out"),
        ApiError::Http(e) => {
            let kind = if e.is_timeout() {
                ApiErrorKind::Timeout
            } else if e.is_connect() {
                ApiErrorKind::Network
            } else {
                ApiErrorKind::Unknown
            };
            crab_core::Error::api(kind, e.to_string())
        }
        ApiError::Sse(message) => {
            // SSE transport errors carry no status; classify from the text and
            // default to a retryable network failure when ambiguous.
            let category = classify_error(0, message);
            let kind = match category {
                ErrorCategory::Unknown => ApiErrorKind::Network,
                other => api_kind_from_category(other, message),
            };
            crab_core::Error::api(kind, format!("SSE stream error: {message}"))
        }
        ApiError::Json(e) => crab_core::Error::api(ApiErrorKind::Unknown, e.to_string()),
        ApiError::Common(inner) => crab_core::Error::Other(inner.to_string()),
    }
}

/// Translate an `error_classifier::ErrorCategory` into the core `ApiErrorKind`,
/// upgrading to `PromptTooLong` when the message text says so (the provider
/// returns 400 for an over-long prompt, which is otherwise non-retryable).
fn api_kind_from_category(category: ErrorCategory, message: &str) -> ApiErrorKind {
    if message_is_prompt_too_long(message) {
        return ApiErrorKind::PromptTooLong;
    }
    match category {
        ErrorCategory::Transient => ApiErrorKind::Transient,
        ErrorCategory::RateLimit => ApiErrorKind::RateLimited,
        ErrorCategory::Auth => ApiErrorKind::Auth,
        ErrorCategory::InvalidRequest | ErrorCategory::ServerError => ApiErrorKind::InvalidRequest,
        ErrorCategory::NetworkError => ApiErrorKind::Network,
        ErrorCategory::Timeout => ApiErrorKind::Timeout,
        ErrorCategory::Unknown => ApiErrorKind::Unknown,
    }
}

/// Whether a provider error message describes an over-long prompt.
fn message_is_prompt_too_long(message: &str) -> bool {
    let msg = message.to_lowercase();
    msg.contains("prompt is too long")
        || msg.contains("prompt too long")
        || msg.contains("too many tokens")
        || msg.contains("context length exceeded")
        || msg.contains("maximum context length")
        || msg.contains("input too long")
}

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
    let context_mgr = ContextManager::default();
    let retry_policy = config.retry_policy.clone().unwrap_or_default();
    let mut state = LoopState::new(config);
    // Tracks which nested AGENTS.md files have been injected this query,
    // preventing re-injection across tool batches within the same session.
    let mut loaded_nested_paths: HashSet<PathBuf> = HashSet::new();

    loop {
        if cancel.is_cancelled() {
            let _ = event_tx
                .send(Event::MessageEnd {
                    usage: state.accumulated_usage.clone(),
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
            &mut state.active_model,
            &event_tx,
            config.compaction_client.as_deref(),
            &config.compaction_config,
            &mut state.compact_state,
            config.hook_executor.as_ref(),
            config.session_id.as_deref(),
        )
        .await;

        // Emit turn start
        let _ = event_tx
            .send(Event::TurnStart {
                turn_index: state.turn_index,
            })
            .await;
        let iter_span = crab_telemetry::metrics::agent_loop_span(state.turn_index as u32);
        state.turn_index += 1;

        // Build cache breakpoints
        let cache_breakpoints = if config.cache_enabled {
            vec![CacheBreakpoint::System, CacheBreakpoint::Tools]
        } else {
            vec![]
        };

        // Select model: use plan_model when in plan mode (if configured),
        // otherwise the live `active_model` (possibly upgraded from config.model).
        let effective_model = if state.plan_mode {
            config
                .plan_model
                .as_ref()
                .unwrap_or(&state.active_model)
                .clone()
        } else {
            state.active_model.clone()
        };

        // Build the API request from conversation state
        let req = MessageRequest {
            model: effective_model,
            messages: Cow::Borrowed(conversation.messages()),
            system: Some(conversation.system_prompt.clone()),
            max_tokens: state.effective_max_tokens,
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
        let mut outcome = match stream_with_retry(
            backend,
            req.clone(),
            &retry_policy,
            &event_tx,
            &cancel,
            Some(executor),
            Some(tool_ctx),
            config.hook_executor.as_deref(),
        )
        .await
        {
            Ok(result) => result,
            Err(e) if is_prompt_too_long_error(&e) && state.ptl_retries < MAX_PTL_RETRIES => {
                state.ptl_retries += 1;
                let _ = event_tx
                    .send(Event::Error {
                        message: format!(
                            "Prompt too long, compacting and retrying ({}/{MAX_PTL_RETRIES})",
                            state.ptl_retries
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
                    config.hook_executor.as_deref(),
                )
                .await?
            }
            Err(e) => return Err(e),
        };

        // Reset PTL retry counter on success
        state.ptl_retries = 0;

        // Record usage against the active model (may differ from `config.model`
        // if context was upgraded to a larger-context variant).
        cost_tracker.add_usage(state.active_model.as_str(), &outcome.usage);
        conversation.record_usage(outcome.usage.clone());
        // Accumulate this iteration's tokens into the task total. We deliberately
        // do NOT emit `Event::MessageEnd` here on every iteration — the TUI
        // treats `MessageEnd` as the signal that the whole agent task is done
        // (state -> Idle, spinner stops, streaming ratchet resets). Emitting
        // mid-cycle (e.g. between tool_use turns or during max-tokens
        // continuation) leaves the TUI Idle while content is still arriving,
        // making subsequent turns appear frozen mid-response.
        state.accumulate(&outcome.usage);

        // Handle max_tokens truncation with escalation + continuation
        if is_max_tokens_stop(outcome.stop_reason.as_deref())
            && state.output_token_retries < MAX_OUTPUT_TOKEN_RETRIES
        {
            state.output_token_retries += 1;

            // First attempt: escalate token cap without injecting a message.
            // The model retries with a higher ceiling, no conversation change.
            if state.output_token_retries == 1 && state.effective_max_tokens < ESCALATED_MAX_TOKENS
            {
                state.effective_max_tokens = ESCALATED_MAX_TOKENS;
                let _ = event_tx
                    .send(Event::Error {
                        message: format!(
                            "Output truncated, escalating to {ESCALATED_MAX_TOKENS} tokens \
                             ({}/{MAX_OUTPUT_TOKEN_RETRIES})",
                            state.output_token_retries
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
                         ({}/{MAX_OUTPUT_TOKEN_RETRIES})",
                        state.output_token_retries
                    ),
                })
                .await;
            if let Some(persister) = &config.session_persister {
                persister.persist_message(&outcome.message);
            }
            conversation.push(outcome.message);
            conversation.push(Message::user(MAX_OUTPUT_TOKENS_RECOVERY));
            continue;
        }
        // Reset on non-truncated success
        state.output_token_retries = 0;

        // PostSampling hook: allow hooks to rewrite the assistant text
        // before it's persisted or seen by downstream consumers. Only the
        // text is mutable — tool_use blocks stay verbatim so the tool
        // boundary contract can't be circumvented.
        if let Some(hooks) = config.hook_executor.as_deref() {
            let hook_ctx = HookContext {
                tool_name: String::new(),
                tool_input: String::new(),
                working_dir: Some(tool_ctx.working_dir.clone()),
                tool_output: Some(outcome.message.text()),
                tool_exit_code: None,
                session_id: config.session_id.clone(),
            };
            if let Ok(hr) = hooks.run(HookTrigger::PostSampling, &hook_ctx).await
                && hr.action == HookAction::Modify
                && let Some(new_text) = hr.message
            {
                rewrite_assistant_text(&mut outcome.message, &new_text);
            }
        }

        // Add assistant message to conversation
        let has_tool_use = outcome.message.has_tool_use();
        if let Some(persister) = &config.session_persister {
            persister.persist_message(&outcome.message);
        }
        conversation.push(outcome.message.clone());

        // If no tool use, run stop hooks — continue if any returns Retry
        if !has_tool_use {
            if let Some(hooks) = config.hook_executor.as_deref() {
                let hook_ctx = HookContext {
                    tool_name: String::new(),
                    tool_input: String::new(),
                    working_dir: Some(tool_ctx.working_dir.clone()),
                    tool_output: Some(outcome.message.text()),
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
                    usage: state.accumulated_usage.clone(),
                })
                .await;
            return Ok(());
        }

        // Apply plan-mode transitions BEFORE executing the batch so an
        // EnterPlanMode call gates later write tools in the *same* assistant
        // message. ExitPlanMode opens the gate for subsequent tools; the
        // EnterPlanMode/ExitPlanMode calls themselves are not write-gated.
        let plan_mode_for_batch =
            apply_plan_mode_transitions(&outcome.message, &mut state.plan_mode);

        // Execute remaining tool calls (skipping any already handled inline
        // during streaming, and any whose input failed to parse).
        let malformed_ids: HashSet<String> = outcome
            .malformed_inputs
            .iter()
            .map(|(id, _)| id.clone())
            .collect();
        let mut skip_ids: HashSet<String> = outcome
            .inline_results
            .iter()
            .map(|(id, _)| id.clone())
            .collect();
        skip_ids.extend(malformed_ids.iter().cloned());

        let batch_results = crate::tool_orchestration::execute_tool_calls(
            &outcome.message,
            executor,
            tool_ctx,
            &event_tx,
            &cancel,
            config.hook_executor.as_deref(),
            config.session_id.as_deref(),
            plan_mode_for_batch,
            &skip_ids,
        )
        .await?;

        // Merge inline, batch, and malformed-input results, then enforce the
        // invariant that every tool_use in the assistant message has exactly
        // one tool_result in the next user message.
        let result_msg = build_tool_result_message(
            &outcome.message,
            std::mem::take(&mut outcome.inline_results),
            batch_results,
            std::mem::take(&mut outcome.malformed_inputs),
        );
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

/// Apply this turn's plan-mode tool calls to `plan_mode` (the persistent state
/// carried into the next turn) and return the plan-mode value that must gate
/// *this* batch's write tools.
///
/// Returning a separate batch value closes the same-batch bypass: if an
/// assistant message contains `EnterPlanMode` alongside a write tool, the
/// write is gated even though `plan_mode` was off when the turn began. The
/// batch is gated whenever plan mode is active on entry or any `EnterPlanMode`
/// appears in the message and is not later cancelled by an `ExitPlanMode`.
fn apply_plan_mode_transitions(assistant_msg: &Message, plan_mode: &mut bool) -> bool {
    let entry = *plan_mode;
    let mut running = entry;
    let mut batch_gated = entry;
    for block in &assistant_msg.content {
        if let ContentBlock::ToolUse { name, .. } = block {
            match name.as_str() {
                ENTER_PLAN_MODE_TOOL_NAME => {
                    running = true;
                    batch_gated = true;
                }
                EXIT_PLAN_MODE_TOOL_NAME => running = false,
                _ => {}
            }
        }
    }
    *plan_mode = running;
    batch_gated
}

/// Assemble the tool-result `Message` for an assistant turn, merging results
/// from every source and guaranteeing the API invariant that each `tool_use`
/// block maps to exactly one `tool_result`.
///
/// Sources, in precedence order: tools executed eagerly during streaming
/// (`inline_results`), tools run in the post-stream batch (`batch_results`),
/// and `tool_use` blocks whose input JSON failed to parse (`malformed_inputs`,
/// reported as errors). Any `tool_use` still missing a result after the union
/// is backfilled with a synthetic error so the next request never carries an
/// orphan `tool_use`.
fn build_tool_result_message(
    assistant_msg: &Message,
    inline_results: Vec<(String, ToolOutput)>,
    batch_results: Vec<(String, crab_core::Result<ToolOutput>)>,
    malformed_inputs: Vec<(String, String)>,
) -> Message {
    use std::collections::HashMap;

    let mut by_id: HashMap<String, ContentBlock> = HashMap::new();
    for (id, output) in inline_results {
        by_id.insert(
            id.clone(),
            ContentBlock::tool_result(id, output.text(), output.is_error),
        );
    }
    for (id, result) in batch_results {
        let (text, is_error) = match result {
            Ok(output) => (output.text(), output.is_error),
            Err(e) => (e.to_string(), true),
        };
        by_id.insert(id.clone(), ContentBlock::tool_result(id, text, is_error));
    }
    for (id, message) in malformed_inputs {
        by_id.insert(id.clone(), ContentBlock::tool_result(id, message, true));
    }

    // Emit one tool_result per tool_use, in the assistant message's order.
    // Backfill any gap so the assistant turn can never leave an orphan.
    let mut content = Vec::new();
    for (id, _name, _input) in assistant_msg.tool_uses() {
        let block = by_id.remove(id).unwrap_or_else(|| {
            debug_assert!(
                false,
                "tool_use {id} had no tool_result from any source; backfilling error"
            );
            ContentBlock::tool_result(
                id,
                "tool produced no result (internal error)".to_string(),
                true,
            )
        });
        content.push(block);
    }

    Message::new(Role::User, content)
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
    hook_executor: Option<&HookExecutor>,
) -> crab_core::Result<StreamOutcome> {
    let mut attempt = 0u32;
    loop {
        let req_clone = req.clone();
        let inline_ctx = match (executor, tool_ctx) {
            (Some(exec), Some(tc)) => Some(InlineExecCtx {
                registry: exec.registry_arc(),
                tool_ctx: tc,
                hook_executor,
            }),
            _ => None,
        };
        match stream_response(backend, req_clone, event_tx, cancel, inline_ctx).await {
            Ok(result) => return Ok(result),
            Err(e) => {
                // Retry only retryable conditions, and only within the limit.
                if is_transient_error(&e) && attempt < policy.max_retries {
                    // Honor a server-provided Retry-After when present;
                    // otherwise fall back to the policy's exponential backoff.
                    let delay = e
                        .retry_after()
                        .unwrap_or_else(|| policy.delay_for_attempt(attempt));
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

/// Whether an error represents a transient/retryable condition.
///
/// Structured `Api` errors are classified by their `kind`. A narrow string
/// fallback covers genuinely unstructured transport errors (`Error::Other`)
/// that never passed through `map_api_error`.
fn is_transient_error(err: &crab_core::Error) -> bool {
    if let Some(kind) = err.api_kind() {
        return kind.is_retryable();
    }
    let msg = err.to_string().to_lowercase();
    msg.contains("timeout")
        || msg.contains("timed out")
        || msg.contains("connection")
        || msg.contains("rate limit")
        || msg.contains("429")
        || msg.contains("529")
        || msg.contains("overloaded")
}

/// Whether an error indicates an overloaded/rate-limited upstream, the trigger
/// for falling back to a secondary model.
fn is_overloaded_error(err: &crab_core::Error) -> bool {
    if let Some(kind) = err.api_kind() {
        return kind.is_overloaded();
    }
    let msg = err.to_string().to_lowercase();
    msg.contains("529")
        || msg.contains("overloaded")
        || msg.contains("rate limit")
        || msg.contains("429")
}

/// Whether an error indicates the prompt exceeded the model's context window.
fn is_prompt_too_long_error(err: &crab_core::Error) -> bool {
    if let Some(kind) = err.api_kind() {
        return kind == ApiErrorKind::PromptTooLong;
    }
    message_is_prompt_too_long(&err.to_string())
}

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

/// Whether the stop reason indicates the output was truncated at `max_tokens`.
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
    /// Hooks configured for this run, so the eager path can defer any tool
    /// that has a `PreToolUse` hook to the batch (which runs it before exec).
    hook_executor: Option<&'a HookExecutor>,
}

/// Stream an LLM response, assembling the assistant message from SSE events.
///
/// Uses `StreamingToolParser` for incremental `tool_use` block parsing and
/// `StreamingUsage` for accurate token accumulation.
///
/// When `inline_ctx` is provided, concurrency-safe tools are spawned as soon
/// as their JSON input is fully parsed (before the stream ends); their results
/// are returned in `StreamOutcome::inline_results`. `tool_use` blocks whose
/// JSON failed to parse are returned in `StreamOutcome::malformed_inputs`.
///
/// A stream that ends without a terminal `MessageStop` is treated as a
/// truncated transport failure (retryable `Api` error), not a complete
/// response — its partial message is discarded and its tools are never run.
async fn stream_response(
    backend: &LlmBackend,
    req: MessageRequest<'_>,
    event_tx: &mpsc::Sender<Event>,
    cancel: &CancellationToken,
    inline_ctx: Option<InlineExecCtx<'_>>,
) -> crab_core::Result<StreamOutcome> {
    let mut stream = std::pin::pin!(backend.stream_message(req));

    let mut tool_parser = StreamingToolParser::new();
    let mut usage_tracker = StreamingUsage::new();
    let mut thinking_blocks: std::collections::HashMap<usize, String> =
        std::collections::HashMap::new();
    let mut saw_message_stop = false;

    let mut streaming_executor = inline_ctx
        .as_ref()
        .map(|_| crate::streaming::StreamingToolExecutor::new(cancel.child_token()));

    while let Some(event) = stream.next().await {
        if cancel.is_cancelled() {
            break;
        }

        let event = match event {
            Ok(ev) => ev,
            Err(api_err) => {
                // Abort any inline tasks spawned during this doomed attempt so
                // they cannot emit stale ToolResult events against the retry.
                if let Some(se) = &mut streaming_executor {
                    se.abort_all();
                }
                return Err(map_api_error(&api_err));
            }
        };

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
                ictx.hook_executor,
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
            StreamEvent::MessageStop => {
                saw_message_stop = true;
            }
            StreamEvent::Error { message } => {
                let _ = event_tx
                    .send(Event::Error {
                        message: message.clone(),
                    })
                    .await;
                if let Some(se) = &mut streaming_executor {
                    se.abort_all();
                }
                let category = classify_error(0, message);
                let kind = match category {
                    ErrorCategory::Unknown => ApiErrorKind::Network,
                    other => api_kind_from_category(other, message),
                };
                return Err(crab_core::Error::api(
                    kind,
                    format!("SSE stream error: {message}"),
                ));
            }
            StreamEvent::ContentBlockStart { .. }
            | StreamEvent::MessageDelta { .. }
            | StreamEvent::Raw { .. } => {}
        }
    }

    // A clean EOF without MessageStop means the connection closed mid-message
    // (e.g. a proxy idle-timeout). Discard the partial turn and signal a
    // retryable transport failure so the partial tools are never executed.
    // A cancellation request is the one legitimate early exit.
    if !saw_message_stop && !cancel.is_cancelled() {
        if let Some(mut se) = streaming_executor {
            se.abort_all();
        }
        return Err(crab_core::Error::api(
            ApiErrorKind::Network,
            "stream ended without MessageStop (connection closed mid-response)",
        ));
    }

    // Extract stop reason before consuming usage_tracker
    let stop_reason = usage_tracker.stop_reason().map(String::from);

    // Assemble content blocks into a Message using the tool parser
    let mut content: Vec<ContentBlock> = Vec::new();
    let mut malformed_inputs: Vec<(String, String)> = Vec::new();

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

    // Add completed tool_use blocks. A block whose input JSON failed to parse
    // still appears in the assistant message (so the API contract holds), but
    // carries an empty input and is recorded as malformed so the loop reports
    // an error tool_result instead of executing it.
    for acc in tool_parser.completed_tools() {
        let input = match acc.parse_input_checked() {
            Ok(value) => value,
            Err(err) => {
                malformed_inputs.push((acc.id.clone(), err));
                serde_json::Value::Object(serde_json::Map::new())
            }
        };
        content.push(ContentBlock::ToolUse {
            id: acc.id.clone(),
            name: acc.name.clone(),
            input,
        });
    }

    // Add any in-progress tools that didn't get a ContentBlockStop, but only
    // when their buffer parses cleanly — a half-streamed tool with broken JSON
    // is dropped here (the missing MessageStop check above already rejects the
    // turn when the whole stream was truncated).
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

    let inline_results = match streaming_executor {
        Some(mut se) => se.collect_all().await,
        None => Vec::new(),
    };

    Ok(StreamOutcome {
        message,
        usage: total_usage,
        stop_reason,
        inline_results,
        malformed_inputs,
    })
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

    // ─── plan-mode same-batch gate (item 4) ───

    fn tool_use(id: &str, name: &str) -> ContentBlock {
        ContentBlock::tool_use(id, name, serde_json::json!({}))
    }

    #[test]
    fn enter_plan_mode_gates_writes_in_same_batch() {
        // EnterPlanMode followed by a write in the same assistant message: the
        // batch must be gated even though plan mode was off on entry.
        let msg = Message::new(
            Role::Assistant,
            vec![
                tool_use("tu_1", ENTER_PLAN_MODE_TOOL_NAME),
                tool_use("tu_2", "Write"),
            ],
        );
        let mut plan_mode = false;
        let batch_gated = apply_plan_mode_transitions(&msg, &mut plan_mode);
        assert!(batch_gated, "EnterPlanMode must gate the same batch");
        assert!(plan_mode, "plan mode persists into the next turn");
    }

    #[test]
    fn exit_plan_mode_opens_gate_for_next_turn() {
        let msg = Message::new(
            Role::Assistant,
            vec![tool_use("tu_1", EXIT_PLAN_MODE_TOOL_NAME)],
        );
        let mut plan_mode = true;
        let batch_gated = apply_plan_mode_transitions(&msg, &mut plan_mode);
        // Entry was plan mode, so this batch is still gated; the gate opens for
        // subsequent turns.
        assert!(batch_gated);
        assert!(!plan_mode, "ExitPlanMode clears plan mode for next turn");
    }

    #[test]
    fn no_plan_tools_preserves_state() {
        let msg = Message::new(Role::Assistant, vec![tool_use("tu_1", "Read")]);
        let mut plan_mode = false;
        assert!(!apply_plan_mode_transitions(&msg, &mut plan_mode));
        assert!(!plan_mode);

        let mut plan_mode = true;
        assert!(apply_plan_mode_transitions(&msg, &mut plan_mode));
        assert!(plan_mode);
    }

    // ─── tool result merge + invariant (items 1, 3) ───

    fn assistant_with_tools(ids: &[(&str, &str)]) -> Message {
        Message::new(
            Role::Assistant,
            ids.iter().map(|(id, name)| tool_use(id, name)).collect(),
        )
    }

    fn result_for<'a>(msg: &'a Message, id: &str) -> &'a str {
        msg.content
            .iter()
            .find_map(|b| match b {
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } if tool_use_id == id => Some(content.as_str()),
                _ => None,
            })
            .expect("result present for id")
    }

    #[test]
    fn build_tool_result_merges_inline_and_batch() {
        // tu_1 ran inline during streaming, tu_2 ran in the batch. Both results
        // must appear in the merged message, in assistant-message order.
        let msg = assistant_with_tools(&[("tu_1", "Read"), ("tu_2", "Bash")]);
        let inline = vec![("tu_1".to_string(), ToolOutput::success("inline output"))];
        let batch = vec![("tu_2".to_string(), Ok(ToolOutput::success("batch output")))];
        let out = build_tool_result_message(&msg, inline, batch, Vec::new());
        assert_eq!(out.content.len(), 2, "one result per tool_use");
        assert_eq!(result_for(&out, "tu_1"), "inline output");
        assert_eq!(result_for(&out, "tu_2"), "batch output");
    }

    #[test]
    fn build_tool_result_reports_malformed_input_as_error() {
        let msg = assistant_with_tools(&[("tu_1", "Read")]);
        let malformed = vec![("tu_1".to_string(), "bad json: trailing comma".to_string())];
        let out = build_tool_result_message(&msg, Vec::new(), Vec::new(), malformed);
        assert_eq!(out.content.len(), 1);
        match &out.content[0] {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "tu_1");
                assert!(*is_error, "malformed input is an error result");
                assert!(content.contains("bad json"));
            }
            _ => panic!("expected ToolResult"),
        }
    }

    /// A `tool_use` missing a result from every source is a programming error:
    /// in debug builds the `debug_assert` fires to surface it. The release-only
    /// backfill that keeps the API contract intact is covered by
    /// `build_tool_result_backfills_missing_in_release`.
    #[test]
    #[should_panic(expected = "had no tool_result")]
    #[cfg(debug_assertions)]
    fn build_tool_result_debug_asserts_on_missing() {
        let msg = assistant_with_tools(&[("tu_1", "Read"), ("tu_2", "Bash")]);
        let inline = vec![("tu_1".to_string(), ToolOutput::success("ok"))];
        let _ = build_tool_result_message(&msg, inline, Vec::new(), Vec::new());
    }

    /// In release builds (no `debug_assert`), a missing result is backfilled
    /// with a synthetic error so the assistant turn never carries an orphan.
    #[test]
    #[cfg(not(debug_assertions))]
    fn build_tool_result_backfills_missing_in_release() {
        let msg = assistant_with_tools(&[("tu_1", "Read"), ("tu_2", "Bash")]);
        let inline = vec![("tu_1".to_string(), ToolOutput::success("ok"))];
        let out = build_tool_result_message(&msg, inline, Vec::new(), Vec::new());
        assert_eq!(
            out.content.len(),
            2,
            "every tool_use gets exactly one result"
        );
        match out.content.iter().find(
            |b| matches!(b, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "tu_2"),
        ) {
            Some(ContentBlock::ToolResult { is_error, .. }) => {
                assert!(*is_error, "backfilled result is an error");
            }
            _ => panic!("tu_2 must have a backfilled result"),
        }
    }

    #[test]
    fn build_tool_result_preserves_assistant_order() {
        let msg = assistant_with_tools(&[("a", "Read"), ("b", "Read"), ("c", "Read")]);
        // Provide results out of order; output must follow the message order.
        let batch = vec![
            ("c".to_string(), Ok(ToolOutput::success("C"))),
            ("a".to_string(), Ok(ToolOutput::success("A"))),
            ("b".to_string(), Ok(ToolOutput::success("B"))),
        ];
        let out = build_tool_result_message(&msg, Vec::new(), batch, Vec::new());
        let ids: Vec<&str> = out
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(ids, vec!["a", "b", "c"]);
    }

    // ─── structured error mapping (item 5) ───

    #[test]
    fn map_api_error_rate_limited_preserves_retry_after() {
        let err = map_api_error(&ApiError::RateLimited {
            retry_after_ms: 3_000,
        });
        assert_eq!(err.api_kind(), Some(ApiErrorKind::RateLimited));
        assert_eq!(err.retry_after(), Some(Duration::from_millis(3_000)));
        assert!(is_transient_error(&err));
        assert!(is_overloaded_error(&err));
    }

    #[test]
    fn map_api_error_classifies_status() {
        let err = map_api_error(&ApiError::Api {
            status: 500,
            message: "internal".into(),
        });
        assert_eq!(err.api_kind(), Some(ApiErrorKind::Transient));
        assert!(is_transient_error(&err));
        assert!(!is_overloaded_error(&err));

        let auth = map_api_error(&ApiError::Api {
            status: 401,
            message: "bad key".into(),
        });
        assert_eq!(auth.api_kind(), Some(ApiErrorKind::Auth));
        assert!(!is_transient_error(&auth));
    }

    #[test]
    fn map_api_error_detects_prompt_too_long() {
        let err = map_api_error(&ApiError::Api {
            status: 400,
            message: "prompt is too long: 250000 tokens > 200000 maximum".into(),
        });
        assert_eq!(err.api_kind(), Some(ApiErrorKind::PromptTooLong));
        assert!(is_prompt_too_long_error(&err));
        // PromptTooLong is not a plain transient retry — it needs compaction.
        assert!(!is_transient_error(&err));
    }

    #[test]
    fn map_api_error_sse_is_retryable_network() {
        let err = map_api_error(&ApiError::Sse("unexpected end of stream".into()));
        assert_eq!(err.api_kind(), Some(ApiErrorKind::Network));
        assert!(is_transient_error(&err));
    }

    #[test]
    fn map_api_error_timeout() {
        let err = map_api_error(&ApiError::Timeout);
        assert_eq!(err.api_kind(), Some(ApiErrorKind::Timeout));
        assert!(is_transient_error(&err));
    }

    #[test]
    fn string_fallback_still_classifies_unstructured_errors() {
        // Errors that never passed through map_api_error (Error::Other) still
        // get a narrow substring classification.
        assert!(is_transient_error(&crab_core::Error::Other(
            "connection reset by peer".into()
        )));
        assert!(is_overloaded_error(&crab_core::Error::Other(
            "model is overloaded (529)".into()
        )));
        assert!(is_prompt_too_long_error(&crab_core::Error::Other(
            "context length exceeded".into()
        )));
    }
}
