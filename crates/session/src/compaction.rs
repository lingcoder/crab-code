use crab_core::message::{ContentBlock, Message, Role};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;

use crate::conversation::Conversation;

/// Token threshold above which a tool result is considered "large" for snipping.
const SNIP_TOKEN_THRESHOLD: u64 = 200;

// ── Configurable compaction mode ──────────────────────────────────────

/// High-level compaction mode that can be selected in settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionMode {
    /// Use an LLM to summarize old messages into a compact summary.
    Summarize,
    /// Truncate old messages to stay within budget (fast, no LLM needed).
    Truncate,
    /// Keep a sliding window of recent turns; drop the oldest.
    SlidingWindow {
        /// Number of recent turns to keep.
        window_size: usize,
    },
    /// Automatic multi-level strategy (default): picks the best approach
    /// based on context usage percentage.
    #[default]
    Auto,
}

// ── Compaction trigger ────────────────────────────────────────────────

/// Conditions that trigger compaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CompactionTrigger {
    /// Token usage percentage at which to trigger (0-100). Default: 80.
    pub token_threshold_percent: u8,
    /// Maximum number of messages before triggering (0 = disabled). Default: 0.
    pub max_messages: usize,
}

impl Default for CompactionTrigger {
    fn default() -> Self {
        Self {
            token_threshold_percent: 80,
            max_messages: 0,
        }
    }
}

impl CompactionTrigger {
    /// Check whether compaction should be triggered for the given conversation.
    pub fn should_compact(&self, conversation: &Conversation) -> bool {
        // Token threshold check
        if conversation.context_window > 0 {
            let used = conversation.estimated_tokens();
            #[allow(clippy::cast_possible_truncation)]
            let percent = (used * 100 / conversation.context_window) as u8;
            if percent >= self.token_threshold_percent {
                return true;
            }
        }
        // Message count check
        if self.max_messages > 0 && conversation.len() > self.max_messages {
            return true;
        }
        false
    }
}

// ── Compaction config (combines mode + trigger) ───────────────────────

/// Full compaction configuration, suitable for serialization in settings.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionConfig {
    /// Which compaction mode to use.
    #[serde(default)]
    pub mode: CompactionMode,
    /// When to trigger compaction.
    #[serde(default)]
    pub trigger: CompactionTrigger,
    /// Whether to preserve system messages from compaction.
    #[serde(default = "default_true")]
    pub preserve_system_messages: bool,
    /// Whether to preserve tool error results from compaction.
    #[serde(default = "default_true")]
    pub preserve_tool_errors: bool,
    /// Number of recent turns always preserved (regardless of strategy).
    #[serde(default = "default_preserve_recent")]
    pub preserve_recent_turns: usize,
}

fn default_true() -> bool {
    true
}

fn default_preserve_recent() -> usize {
    2
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            mode: CompactionMode::default(),
            trigger: CompactionTrigger::default(),
            preserve_system_messages: true,
            preserve_tool_errors: true,
            preserve_recent_turns: 2,
        }
    }
}

// ── Compaction report ─────────────────────────────────────────────────

/// Report produced after compaction, showing before/after token estimates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionReport {
    /// Estimated tokens before compaction.
    pub tokens_before: u64,
    /// Estimated tokens after compaction.
    pub tokens_after: u64,
    /// Number of messages before compaction.
    pub messages_before: usize,
    /// Number of messages after compaction.
    pub messages_after: usize,
    /// The strategy that was actually applied.
    pub strategy_used: CompactionStrategy,
}

impl CompactionReport {
    /// Tokens saved by compaction.
    pub fn tokens_saved(&self) -> u64 {
        self.tokens_before.saturating_sub(self.tokens_after)
    }

    /// Messages removed by compaction.
    pub fn messages_removed(&self) -> usize {
        self.messages_before.saturating_sub(self.messages_after)
    }

    /// Compression ratio as a percentage (0-100). 0 = no savings, 100 = all removed.
    pub fn compression_percent(&self) -> u8 {
        if self.tokens_before == 0 {
            return 0;
        }
        #[allow(clippy::cast_possible_truncation)]
        let pct = (self.tokens_saved() * 100 / self.tokens_before) as u8;
        pct
    }
}

// ── Internal 5-level strategy (used by Auto mode) ─────────────────────

/// 5-level compaction strategy, triggered by context usage thresholds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompactionStrategy {
    /// Level 1 (70-80%): Trim old tool output, keep only summary lines.
    Snip,
    /// Level 2 (80-85%): Replace large results (>500 tokens) with AI summary.
    Microcompact,
    /// Level 3 (85-90%): Summarize old messages via small model.
    Summarize,
    /// Level 4 (90-95%): Keep recent N turns + summarize the rest.
    Hybrid { keep_recent: usize },
    /// Level 5 (>95%): Emergency truncation via `Conversation::truncate_to_budget`.
    Truncate,
    /// Sliding window: keep only the most recent N turns.
    SlidingWindow { window_size: usize },
}

impl CompactionStrategy {
    /// Select the appropriate strategy based on context usage percentage.
    pub fn for_usage(percent: u8) -> Option<Self> {
        match percent {
            0..70 => None,
            70..80 => Some(Self::Snip),
            80..85 => Some(Self::Microcompact),
            85..90 => Some(Self::Summarize),
            90..95 => Some(Self::Hybrid { keep_recent: 3 }),
            _ => Some(Self::Truncate),
        }
    }
}

// ── Message importance ────────────────────────────────────────────────

/// Determine whether a message is "important" and should be preserved
/// during compaction.
fn is_important_message(msg: &Message, config: &CompactionConfig) -> bool {
    // System messages are always important if configured
    if config.preserve_system_messages && msg.role == Role::System {
        return true;
    }

    // Tool error results are important if configured
    if config.preserve_tool_errors {
        for block in &msg.content {
            if let ContentBlock::ToolResult { is_error, .. } = block
                && *is_error
            {
                return true;
            }
        }
    }

    false
}

// ── Compaction client trait ───────────────────────────────────────────

/// Abstraction for the LLM client used during compaction.
/// Decouples compaction logic from a specific API backend.
pub trait CompactionClient: Send + Sync {
    fn summarize(
        &self,
        messages: &[Message],
        instruction: &str,
    ) -> Pin<Box<dyn Future<Output = crab_common::Result<String>> + Send + '_>>;
}

// ── Main compaction entry point ───────────────────────────────────────

/// Apply compaction to a conversation using the given config.
/// Returns a report describing what was done.
pub async fn compact_with_config(
    conversation: &mut Conversation,
    config: &CompactionConfig,
    client: &dyn CompactionClient,
) -> crab_common::Result<CompactionReport> {
    let tokens_before = conversation.estimated_tokens();
    let messages_before = conversation.len();

    // Determine strategy based on mode
    let strategy = match &config.mode {
        CompactionMode::Auto => {
            if conversation.context_window == 0 {
                CompactionStrategy::Truncate
            } else {
                #[allow(clippy::cast_possible_truncation)]
                let percent = (tokens_before * 100 / conversation.context_window) as u8;
                CompactionStrategy::for_usage(percent).unwrap_or(CompactionStrategy::Snip)
            }
        }
        CompactionMode::Summarize => CompactionStrategy::Summarize,
        CompactionMode::Truncate => CompactionStrategy::Truncate,
        CompactionMode::SlidingWindow { window_size } => CompactionStrategy::SlidingWindow {
            window_size: *window_size,
        },
    };

    // Apply the strategy
    apply_strategy(conversation, &strategy, config, client).await?;

    let tokens_after = conversation.estimated_tokens();
    let messages_after = conversation.len();

    Ok(CompactionReport {
        tokens_before,
        tokens_after,
        messages_before,
        messages_after,
        strategy_used: strategy,
    })
}

/// Legacy entry point: apply a compaction strategy directly.
pub async fn compact(
    conversation: &mut Conversation,
    strategy: CompactionStrategy,
    client: &dyn CompactionClient,
) -> crab_common::Result<()> {
    let config = CompactionConfig::default();
    apply_strategy(conversation, &strategy, &config, client).await
}

/// Apply a specific compaction strategy.
async fn apply_strategy(
    conversation: &mut Conversation,
    strategy: &CompactionStrategy,
    config: &CompactionConfig,
    _client: &dyn CompactionClient,
) -> crab_common::Result<()> {
    match strategy {
        CompactionStrategy::Snip => {
            snip_large_tool_results(conversation, config);
            Ok(())
        }
        CompactionStrategy::Truncate => {
            let budget = conversation.context_window * 50 / 100;
            truncate_preserving_important(conversation, budget, config);
            Ok(())
        }
        CompactionStrategy::SlidingWindow { window_size } => {
            sliding_window(conversation, *window_size, config);
            Ok(())
        }
        CompactionStrategy::Microcompact => {
            // Level 2: Summarize large tool results (>500 tokens) via LLM,
            // then snip anything remaining that is still large.
            summarize_large_tool_results(conversation, config, _client).await?;
            snip_large_tool_results(conversation, config);
            Ok(())
        }
        CompactionStrategy::Summarize => {
            // Level 3: Summarize all old messages via LLM into a single
            // system-level recap, keeping only recent turns.
            summarize_old_messages(conversation, config, _client).await
        }
        CompactionStrategy::Hybrid { keep_recent } => {
            // Level 4: Keep recent N turns verbatim, summarize the rest.
            let keep = *keep_recent;
            summarize_old_messages_keeping(conversation, config, _client, keep).await
        }
    }
}

// ── Strategy implementations ──────────────────────────────────────────

/// Level 1 compaction: replace large tool results with a truncated snippet.
fn snip_large_tool_results(conversation: &mut Conversation, config: &CompactionConfig) {
    let turn_count = conversation.turn_count();
    let preserve_turns = turn_count.saturating_sub(config.preserve_recent_turns);

    let messages = conversation.inner.messages().to_vec();
    let mut snipped = Vec::with_capacity(messages.len());
    let mut current_turn = 0usize;

    for msg in messages {
        if msg.role == Role::User {
            current_turn += 1;
        }

        if current_turn <= preserve_turns && !is_important_message(&msg, config) {
            // In old turns: snip large tool results
            let new_content: Vec<ContentBlock> = msg
                .content
                .into_iter()
                .map(|block| {
                    if let ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } = &block
                    {
                        let estimated = content.len() as u64 / 4; // rough token estimate
                        if estimated > SNIP_TOKEN_THRESHOLD {
                            let preview: String = content.chars().take(200).collect();
                            return ContentBlock::ToolResult {
                                tool_use_id: tool_use_id.clone(),
                                content: format!("{preview}... [snipped, was ~{estimated} tokens]"),
                                is_error: *is_error,
                            };
                        }
                    }
                    block
                })
                .collect();
            snipped.push(Message::new(msg.role, new_content));
        } else {
            snipped.push(msg);
        }
    }

    conversation.inner.clear();
    for msg in snipped {
        conversation.inner.push(msg);
    }
}

/// Truncate messages from the beginning, but preserve important messages.
fn truncate_preserving_important(
    conversation: &mut Conversation,
    budget: u64,
    config: &CompactionConfig,
) {
    if conversation.estimated_tokens() <= budget {
        return;
    }

    let messages = conversation.inner.messages().to_vec();
    let total = messages.len();

    // Determine which messages are in recent turns (protected)
    let turn_count = conversation.turn_count();
    let preserve_boundary = turn_count.saturating_sub(config.preserve_recent_turns);
    let mut current_turn = 0usize;

    // Mark each message as removable or not
    let mut removable = vec![false; total];
    for (i, msg) in messages.iter().enumerate() {
        if msg.role == Role::User {
            current_turn += 1;
        }
        // Only mark old-turn, non-important messages as removable
        if current_turn <= preserve_boundary && !is_important_message(msg, config) {
            removable[i] = true;
        }
    }

    // Remove messages from the beginning until we fit within budget
    let mut kept = Vec::new();

    // First pass: collect all non-removable messages to see minimum cost
    let non_removable: Vec<(usize, &Message)> = messages
        .iter()
        .enumerate()
        .filter(|(i, _)| !removable[*i])
        .collect();

    let non_removable_tokens: u64 = non_removable
        .iter()
        .map(|(_, msg)| msg.estimated_tokens())
        .sum();

    if non_removable_tokens <= budget {
        // We have room for some removable messages too — keep from the end
        let remaining_budget = budget - non_removable_tokens;
        let removable_msgs: Vec<(usize, &Message)> = messages
            .iter()
            .enumerate()
            .filter(|(i, _)| removable[*i])
            .collect();

        // Keep removable messages from the end (most recent first)
        let mut kept_removable_tokens = 0u64;
        let mut kept_removable_indices = Vec::new();
        for &(idx, msg) in removable_msgs.iter().rev() {
            let msg_tokens = msg.estimated_tokens();
            if kept_removable_tokens + msg_tokens <= remaining_budget {
                kept_removable_indices.push(idx);
                kept_removable_tokens += msg_tokens;
            }
        }
        kept_removable_indices.sort_unstable();

        // Merge: non-removable + kept-removable, in original order
        let mut keep_set: Vec<usize> = non_removable.iter().map(|(i, _)| *i).collect();
        keep_set.extend_from_slice(&kept_removable_indices);
        keep_set.sort_unstable();

        for idx in keep_set {
            kept.push(messages[idx].clone());
        }
    } else {
        // Even non-removable messages exceed budget.
        // Always keep important messages; fill remaining budget from the end.
        let mut keep_set = Vec::new();
        let mut budget_used = 0u64;

        // First pass: always keep important messages (system, errors)
        for (i, msg) in messages.iter().enumerate() {
            if is_important_message(msg, config) {
                keep_set.push(i);
                budget_used += msg.estimated_tokens();
            }
        }

        // Second pass: add non-important, non-removable from the end
        let others: Vec<usize> = (0..total)
            .filter(|i| !removable[*i] && !is_important_message(&messages[*i], config))
            .collect();
        for &idx in others.iter().rev() {
            let msg_tokens = messages[idx].estimated_tokens();
            if budget_used + msg_tokens > budget {
                break;
            }
            keep_set.push(idx);
            budget_used += msg_tokens;
        }

        keep_set.sort_unstable();
        for idx in keep_set {
            kept.push(messages[idx].clone());
        }
    }

    conversation.inner.clear();
    for msg in kept {
        conversation.inner.push(msg);
    }
}

/// Level 2 compaction: replace large tool results with LLM-generated summaries.
async fn summarize_large_tool_results(
    conversation: &mut Conversation,
    config: &CompactionConfig,
    client: &dyn CompactionClient,
) -> crab_common::Result<()> {
    let turn_count = conversation.turn_count();
    let preserve_turns = turn_count.saturating_sub(config.preserve_recent_turns);

    let messages = conversation.inner.messages().to_vec();
    let mut updated = Vec::with_capacity(messages.len());
    let mut current_turn = 0usize;

    for msg in messages {
        if msg.role == Role::User {
            current_turn += 1;
        }

        if current_turn <= preserve_turns && !is_important_message(&msg, config) {
            let mut new_content = Vec::with_capacity(msg.content.len());
            for block in msg.content {
                if let ContentBlock::ToolResult {
                    ref tool_use_id,
                    ref content,
                    is_error,
                } = block
                {
                    let estimated = content.len() as u64 / 4;
                    if estimated > SNIP_TOKEN_THRESHOLD {
                        // Ask LLM to summarize this tool result
                        let summary_msg = Message::new(
                            Role::User,
                            vec![ContentBlock::text(content.clone())],
                        );
                        let summary = client
                            .summarize(
                                &[summary_msg],
                                "Summarize this tool output in 1-2 sentences. Keep key facts.",
                            )
                            .await
                            .unwrap_or_else(|_| {
                                // Fallback to simple truncation if LLM fails
                                let preview: String = content.chars().take(200).collect();
                                format!("{preview}... [summarization failed, truncated]")
                            });
                        new_content.push(ContentBlock::ToolResult {
                            tool_use_id: tool_use_id.clone(),
                            content: format!("[summary] {summary}"),
                            is_error,
                        });
                        continue;
                    }
                }
                new_content.push(block);
            }
            updated.push(Message::new(msg.role, new_content));
        } else {
            updated.push(msg);
        }
    }

    conversation.inner.clear();
    for msg in updated {
        conversation.inner.push(msg);
    }
    Ok(())
}

/// Level 3 compaction: summarize all old messages into a single recap.
async fn summarize_old_messages(
    conversation: &mut Conversation,
    config: &CompactionConfig,
    client: &dyn CompactionClient,
) -> crab_common::Result<()> {
    summarize_old_messages_keeping(conversation, config, client, config.preserve_recent_turns)
        .await
}

/// Level 3/4 compaction: summarize old messages, keeping `keep_recent` turns verbatim.
async fn summarize_old_messages_keeping(
    conversation: &mut Conversation,
    config: &CompactionConfig,
    client: &dyn CompactionClient,
    keep_recent: usize,
) -> crab_common::Result<()> {
    let turn_count = conversation.turn_count();
    if turn_count <= keep_recent {
        return Ok(()); // nothing old to summarize
    }

    let messages = conversation.inner.messages().to_vec();
    let preserve_boundary = turn_count.saturating_sub(keep_recent);

    // Split messages into old (to summarize) and recent (to keep)
    let mut old_msgs = Vec::new();
    let mut recent_msgs = Vec::new();
    let mut important_msgs = Vec::new();
    let mut current_turn = 0usize;

    for msg in &messages {
        if msg.role == Role::User {
            current_turn += 1;
        }
        if current_turn <= preserve_boundary {
            if is_important_message(msg, config) {
                important_msgs.push(msg.clone());
            } else {
                old_msgs.push(msg.clone());
            }
        } else {
            recent_msgs.push(msg.clone());
        }
    }

    if old_msgs.is_empty() {
        return Ok(()); // nothing to summarize
    }

    // Ask LLM to produce a summary of old messages
    let summary = client
        .summarize(
            &old_msgs,
            "Summarize the preceding conversation concisely. \
             Preserve key decisions, file paths, function names, and outcomes. \
             Omit routine acknowledgements.",
        )
        .await
        .unwrap_or_else(|_| {
            // Fallback: just count what was removed
            format!(
                "[compaction: {} old messages removed, {} turns summarized]",
                old_msgs.len(),
                preserve_boundary
            )
        });

    // Rebuild conversation: important + summary + recent
    conversation.inner.clear();
    for msg in important_msgs {
        conversation.inner.push(msg);
    }
    // Insert the summary as a system message so the model has context
    conversation.inner.push(Message::new(
        Role::User,
        vec![ContentBlock::text(format!(
            "[Context compacted — summary of earlier conversation]\n{summary}"
        ))],
    ));
    conversation.inner.push(Message::new(
        Role::Assistant,
        vec![ContentBlock::text(
            "Understood, I have the context from the summary above.".to_owned(),
        )],
    ));
    for msg in recent_msgs {
        conversation.inner.push(msg);
    }

    Ok(())
}

/// Sliding window compaction: keep only the most recent N turns.
fn sliding_window(conversation: &mut Conversation, window_size: usize, config: &CompactionConfig) {
    let turn_count = conversation.turn_count();
    if turn_count <= window_size {
        return; // nothing to drop
    }

    let messages = conversation.inner.messages().to_vec();
    let keep_from_turn = turn_count - window_size;

    let mut kept = Vec::new();
    let mut current_turn = 0usize;

    for msg in &messages {
        if msg.role == Role::User {
            current_turn += 1;
        }

        // Keep if in the window OR if important
        if current_turn > keep_from_turn || is_important_message(msg, config) {
            kept.push(msg.clone());
        }
    }

    conversation.inner.clear();
    for msg in kept {
        conversation.inner.push(msg);
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Test helpers ───────────────────────────────────────────────

    struct DummyClient;
    impl CompactionClient for DummyClient {
        fn summarize(
            &self,
            _messages: &[Message],
            _instruction: &str,
        ) -> Pin<Box<dyn Future<Output = crab_common::Result<String>> + Send + '_>> {
            Box::pin(async { Ok("summary".into()) })
        }
    }

    fn make_conv(context_window: u64) -> Conversation {
        Conversation::new("s".into(), String::new(), context_window)
    }

    fn fill_turns(conv: &mut Conversation, n: usize) {
        for i in 0..n {
            conv.push_user(format!("message {i}"));
            conv.push(Message::new(
                Role::Assistant,
                vec![ContentBlock::text(format!("reply {i}"))],
            ));
        }
    }

    // ── CompactionMode serde ──────────────────────────────────────

    #[test]
    fn compaction_mode_default_is_auto() {
        assert_eq!(CompactionMode::default(), CompactionMode::Auto);
    }

    #[test]
    fn compaction_mode_serde_roundtrip() {
        let modes = vec![
            CompactionMode::Summarize,
            CompactionMode::Truncate,
            CompactionMode::SlidingWindow { window_size: 5 },
            CompactionMode::Auto,
        ];
        for mode in &modes {
            let json = serde_json::to_string(mode).unwrap();
            let back: CompactionMode = serde_json::from_str(&json).unwrap();
            assert_eq!(&back, mode);
        }
    }

    // ── CompactionTrigger ─────────────────────────────────────────

    #[test]
    fn trigger_default_values() {
        let trigger = CompactionTrigger::default();
        assert_eq!(trigger.token_threshold_percent, 80);
        assert_eq!(trigger.max_messages, 0);
    }

    #[test]
    fn trigger_token_threshold() {
        let trigger = CompactionTrigger {
            token_threshold_percent: 50,
            max_messages: 0,
        };
        let mut conv = make_conv(100);
        let big_text = "x".repeat(300); // ~75 tokens >> 50% of 100
        conv.push_user(&big_text);
        assert!(trigger.should_compact(&conv));
    }

    #[test]
    fn trigger_message_count() {
        let trigger = CompactionTrigger {
            token_threshold_percent: 99,
            max_messages: 5,
        };
        let mut conv = make_conv(1_000_000);
        fill_turns(&mut conv, 3); // 6 messages > max_messages=5
        assert!(trigger.should_compact(&conv));
    }

    #[test]
    fn trigger_not_reached() {
        let trigger = CompactionTrigger {
            token_threshold_percent: 99,
            max_messages: 100,
        };
        let mut conv = make_conv(1_000_000);
        conv.push_user("small");
        assert!(!trigger.should_compact(&conv));
    }

    #[test]
    fn trigger_zero_context_window_only_checks_messages() {
        let trigger = CompactionTrigger {
            token_threshold_percent: 50,
            max_messages: 2,
        };
        let mut conv = make_conv(0);
        fill_turns(&mut conv, 2); // 4 messages > 2
        assert!(trigger.should_compact(&conv));
    }

    // ── CompactionConfig serde ────────────────────────────────────

    #[test]
    fn config_default_values() {
        let config = CompactionConfig::default();
        assert_eq!(config.mode, CompactionMode::Auto);
        assert_eq!(config.trigger.token_threshold_percent, 80);
        assert!(config.preserve_system_messages);
        assert!(config.preserve_tool_errors);
        assert_eq!(config.preserve_recent_turns, 2);
    }

    #[test]
    fn config_serde_roundtrip() {
        let config = CompactionConfig {
            mode: CompactionMode::SlidingWindow { window_size: 10 },
            trigger: CompactionTrigger {
                token_threshold_percent: 70,
                max_messages: 50,
            },
            preserve_system_messages: false,
            preserve_tool_errors: true,
            preserve_recent_turns: 3,
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: CompactionConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.mode, config.mode);
        assert_eq!(
            back.trigger.token_threshold_percent,
            config.trigger.token_threshold_percent
        );
        assert_eq!(back.trigger.max_messages, config.trigger.max_messages);
        assert!(!back.preserve_system_messages);
        assert_eq!(back.preserve_recent_turns, 3);
    }

    #[test]
    fn config_deserialize_with_defaults() {
        // Minimal JSON — all fields should get defaults
        let json = r"{}";
        let config: CompactionConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.mode, CompactionMode::Auto);
        assert!(config.preserve_system_messages);
        assert_eq!(config.preserve_recent_turns, 2);
    }

    // ── CompactionReport ──────────────────────────────────────────

    #[test]
    fn report_tokens_saved() {
        let report = CompactionReport {
            tokens_before: 1000,
            tokens_after: 400,
            messages_before: 20,
            messages_after: 8,
            strategy_used: CompactionStrategy::Truncate,
        };
        assert_eq!(report.tokens_saved(), 600);
        assert_eq!(report.messages_removed(), 12);
        assert_eq!(report.compression_percent(), 60);
    }

    #[test]
    fn report_no_savings() {
        let report = CompactionReport {
            tokens_before: 100,
            tokens_after: 100,
            messages_before: 5,
            messages_after: 5,
            strategy_used: CompactionStrategy::Snip,
        };
        assert_eq!(report.tokens_saved(), 0);
        assert_eq!(report.messages_removed(), 0);
        assert_eq!(report.compression_percent(), 0);
    }

    #[test]
    fn report_zero_before() {
        let report = CompactionReport {
            tokens_before: 0,
            tokens_after: 0,
            messages_before: 0,
            messages_after: 0,
            strategy_used: CompactionStrategy::Snip,
        };
        assert_eq!(report.compression_percent(), 0);
    }

    // ── Strategy selection (Auto mode) ────────────────────────────

    #[test]
    fn strategy_for_usage_levels() {
        assert!(CompactionStrategy::for_usage(50).is_none());
        assert_eq!(
            CompactionStrategy::for_usage(75),
            Some(CompactionStrategy::Snip)
        );
        assert_eq!(
            CompactionStrategy::for_usage(82),
            Some(CompactionStrategy::Microcompact)
        );
        assert_eq!(
            CompactionStrategy::for_usage(87),
            Some(CompactionStrategy::Summarize)
        );
        assert_eq!(
            CompactionStrategy::for_usage(92),
            Some(CompactionStrategy::Hybrid { keep_recent: 3 })
        );
        assert_eq!(
            CompactionStrategy::for_usage(96),
            Some(CompactionStrategy::Truncate)
        );
    }

    #[test]
    fn strategy_for_usage_boundary_values() {
        assert!(CompactionStrategy::for_usage(0).is_none());
        assert!(CompactionStrategy::for_usage(69).is_none());
        assert_eq!(
            CompactionStrategy::for_usage(70),
            Some(CompactionStrategy::Snip)
        );
        assert_eq!(
            CompactionStrategy::for_usage(79),
            Some(CompactionStrategy::Snip)
        );
        assert_eq!(
            CompactionStrategy::for_usage(80),
            Some(CompactionStrategy::Microcompact)
        );
        assert_eq!(
            CompactionStrategy::for_usage(85),
            Some(CompactionStrategy::Summarize)
        );
        assert_eq!(
            CompactionStrategy::for_usage(90),
            Some(CompactionStrategy::Hybrid { keep_recent: 3 })
        );
        assert_eq!(
            CompactionStrategy::for_usage(95),
            Some(CompactionStrategy::Truncate)
        );
        assert_eq!(
            CompactionStrategy::for_usage(100),
            Some(CompactionStrategy::Truncate)
        );
        assert_eq!(
            CompactionStrategy::for_usage(255),
            Some(CompactionStrategy::Truncate)
        );
    }

    #[test]
    fn strategy_equality() {
        assert_eq!(CompactionStrategy::Snip, CompactionStrategy::Snip);
        assert_ne!(CompactionStrategy::Snip, CompactionStrategy::Truncate);
        assert_eq!(
            CompactionStrategy::Hybrid { keep_recent: 3 },
            CompactionStrategy::Hybrid { keep_recent: 3 }
        );
        assert_ne!(
            CompactionStrategy::Hybrid { keep_recent: 3 },
            CompactionStrategy::Hybrid { keep_recent: 5 }
        );
    }

    // ── Message importance ────────────────────────────────────────

    #[test]
    fn system_message_is_important() {
        let config = CompactionConfig::default();
        let msg = Message::system("You are helpful.");
        assert!(is_important_message(&msg, &config));
    }

    #[test]
    fn system_message_not_important_when_disabled() {
        let config = CompactionConfig {
            preserve_system_messages: false,
            ..Default::default()
        };
        let msg = Message::system("You are helpful.");
        assert!(!is_important_message(&msg, &config));
    }

    #[test]
    fn tool_error_is_important() {
        let config = CompactionConfig::default();
        let msg = Message::tool_result("tc_1", "command failed", true);
        assert!(is_important_message(&msg, &config));
    }

    #[test]
    fn tool_error_not_important_when_disabled() {
        let config = CompactionConfig {
            preserve_tool_errors: false,
            ..Default::default()
        };
        let msg = Message::tool_result("tc_1", "command failed", true);
        assert!(!is_important_message(&msg, &config));
    }

    #[test]
    fn normal_user_message_not_important() {
        let config = CompactionConfig::default();
        let msg = Message::user("hello");
        assert!(!is_important_message(&msg, &config));
    }

    #[test]
    fn normal_tool_result_not_important() {
        let config = CompactionConfig::default();
        let msg = Message::tool_result("tc_1", "file contents here", false);
        assert!(!is_important_message(&msg, &config));
    }

    // ── Snip strategy ─────────────────────────────────────────────

    #[test]
    fn snip_removes_large_tool_results() {
        let mut conv = make_conv(100_000);

        // Turn 1 (old): user + assistant + large tool result
        conv.push_user("Do something");
        conv.push(Message::new(
            Role::Assistant,
            vec![ContentBlock::text("Sure")],
        ));
        let large_content = "x".repeat(2000); // ~500 tokens, > SNIP_TOKEN_THRESHOLD
        conv.push_tool_result("tc_1", &large_content, false);

        // Turn 2: user + assistant (preserved)
        conv.push_user("And this?");
        conv.push(Message::new(
            Role::Assistant,
            vec![ContentBlock::text("Done")],
        ));

        // Turn 3 (recent): user + large tool result (should NOT be snipped)
        conv.push_user("One more");
        conv.push_tool_result("tc_2", &large_content, false);

        let config = CompactionConfig::default();
        snip_large_tool_results(&mut conv, &config);

        let msgs = conv.messages();
        if let ContentBlock::ToolResult { content, .. } = &msgs[2].content[0] {
            assert!(content.contains("[snipped"));
            assert!(content.len() < 500);
        }

        // Recent turn's tool result should be preserved
        if let ContentBlock::ToolResult { content, .. } = &msgs[6].content[0] {
            assert_eq!(content.len(), 2000);
        }
    }

    #[test]
    fn snip_preserves_small_results() {
        let mut conv = make_conv(100_000);
        conv.push_user("Do something");
        conv.push_tool_result("tc_1", "small result", false);
        conv.push_user("Next");
        conv.push(Message::new(
            Role::Assistant,
            vec![ContentBlock::text("ok")],
        ));
        conv.push_user("Last");

        let config = CompactionConfig::default();
        snip_large_tool_results(&mut conv, &config);

        let msgs = conv.messages();
        if let ContentBlock::ToolResult { content, .. } = &msgs[1].content[0] {
            assert_eq!(content, "small result");
        }
    }

    #[test]
    fn snip_empty_conversation() {
        let mut conv = make_conv(100_000);
        let config = CompactionConfig::default();
        snip_large_tool_results(&mut conv, &config);
        assert!(conv.is_empty());
    }

    #[test]
    fn snip_single_turn_preserves_everything() {
        let mut conv = make_conv(100_000);
        let large_content = "x".repeat(2000);
        conv.push_user("hello");
        conv.push_tool_result("tc_1", &large_content, false);

        let config = CompactionConfig::default();
        snip_large_tool_results(&mut conv, &config);

        let msgs = conv.messages();
        if let ContentBlock::ToolResult { content, .. } = &msgs[1].content[0] {
            assert_eq!(content.len(), 2000);
        }
    }

    #[test]
    fn snip_preserves_error_tool_results() {
        let mut conv = make_conv(100_000);

        // Turn 1 (old)
        conv.push_user("Do something");
        let large_error = "E".repeat(2000);
        conv.push_tool_result("tc_1", &large_error, true);

        // Turn 2 (recent)
        conv.push_user("Next");
        conv.push(Message::new(
            Role::Assistant,
            vec![ContentBlock::text("ok")],
        ));

        // Turn 3 (recent)
        conv.push_user("Last");

        let config = CompactionConfig::default();
        snip_large_tool_results(&mut conv, &config);

        // Error tool result should be preserved (important message)
        let msgs = conv.messages();
        if let ContentBlock::ToolResult {
            content, is_error, ..
        } = &msgs[1].content[0]
        {
            assert_eq!(content.len(), 2000); // NOT snipped
            assert!(*is_error);
        }
    }

    // ── Sliding window strategy ───────────────────────────────────

    #[test]
    fn sliding_window_keeps_recent_turns() {
        let mut conv = make_conv(100_000);
        fill_turns(&mut conv, 10); // 10 turns, 20 messages
        assert_eq!(conv.len(), 20);

        let config = CompactionConfig::default();
        sliding_window(&mut conv, 3, &config);

        // Should keep only last 3 turns = 6 messages
        assert_eq!(conv.len(), 6);
    }

    #[test]
    fn sliding_window_noop_when_within_window() {
        let mut conv = make_conv(100_000);
        fill_turns(&mut conv, 3);
        assert_eq!(conv.len(), 6);

        let config = CompactionConfig::default();
        sliding_window(&mut conv, 5, &config);

        assert_eq!(conv.len(), 6); // no change
    }

    #[test]
    fn sliding_window_preserves_system_messages() {
        let mut conv = make_conv(100_000);

        // System message in turn 1
        conv.push(Message::system("Important context"));
        fill_turns(&mut conv, 5);
        let total_before = conv.len();

        let config = CompactionConfig::default();
        sliding_window(&mut conv, 2, &config);

        // System message should be preserved even though it's old
        let msgs = conv.messages();
        let has_system = msgs.iter().any(|m| m.role == Role::System);
        assert!(has_system);
        assert!(conv.len() < total_before);
    }

    // ── Truncate with importance preservation ─────────────────────

    #[tokio::test]
    async fn compact_truncate_reduces_messages() {
        let mut conv = make_conv(1000);
        fill_turns(&mut conv, 20);
        let original_len = conv.len();

        compact(&mut conv, CompactionStrategy::Truncate, &DummyClient)
            .await
            .unwrap();

        assert!(conv.len() <= original_len);
    }

    #[tokio::test]
    async fn compact_with_config_returns_report() {
        let mut conv = make_conv(100);
        let big_text = "x".repeat(500);
        conv.push_user(&big_text);
        conv.push(Message::new(
            Role::Assistant,
            vec![ContentBlock::text(&big_text)],
        ));
        conv.push_user("latest question");

        let config = CompactionConfig {
            mode: CompactionMode::Truncate,
            ..Default::default()
        };

        let report = compact_with_config(&mut conv, &config, &DummyClient)
            .await
            .unwrap();

        assert!(report.tokens_before >= report.tokens_after);
        assert_eq!(report.strategy_used, CompactionStrategy::Truncate);
    }

    #[tokio::test]
    async fn compact_sliding_window_via_config() {
        let mut conv = make_conv(100_000);
        fill_turns(&mut conv, 10);
        assert_eq!(conv.len(), 20);

        let config = CompactionConfig {
            mode: CompactionMode::SlidingWindow { window_size: 3 },
            ..Default::default()
        };

        let report = compact_with_config(&mut conv, &config, &DummyClient)
            .await
            .unwrap();

        assert_eq!(conv.len(), 6);
        assert_eq!(report.messages_before, 20);
        assert_eq!(report.messages_after, 6);
        assert_eq!(
            report.strategy_used,
            CompactionStrategy::SlidingWindow { window_size: 3 }
        );
    }

    #[tokio::test]
    async fn compact_auto_selects_strategy() {
        let mut conv = make_conv(100);
        // Fill to >95% to trigger Truncate
        let big_text = "x".repeat(1000);
        conv.push_user(&big_text);
        conv.push(Message::new(
            Role::Assistant,
            vec![ContentBlock::text(&big_text)],
        ));

        let config = CompactionConfig {
            mode: CompactionMode::Auto,
            ..Default::default()
        };

        let report = compact_with_config(&mut conv, &config, &DummyClient)
            .await
            .unwrap();

        // Should have used Truncate (emergency) since usage > 95%
        assert_eq!(report.strategy_used, CompactionStrategy::Truncate);
    }

    // ── Truncate preserves important messages ─────────────────────

    #[test]
    fn truncate_preserves_system_messages() {
        let mut conv = make_conv(200);

        conv.push(Message::system("Critical instruction"));
        let big = "x".repeat(500);
        conv.push_user(&big);
        conv.push(Message::new(
            Role::Assistant,
            vec![ContentBlock::text(&big)],
        ));
        // Recent turn
        conv.push_user("latest");
        conv.push(Message::new(
            Role::Assistant,
            vec![ContentBlock::text("ok")],
        ));

        let config = CompactionConfig::default();
        truncate_preserving_important(&mut conv, 50, &config);

        let msgs = conv.messages();
        let has_system = msgs.iter().any(|m| m.role == Role::System);
        assert!(has_system, "System message should be preserved");
    }

    #[test]
    fn truncate_preserves_tool_errors() {
        let mut conv = make_conv(200);

        conv.push_user("first");
        conv.push_tool_result("tc_1", "command failed with exit code 1", true);

        let big = "x".repeat(500);
        conv.push_user(&big);
        conv.push(Message::new(
            Role::Assistant,
            vec![ContentBlock::text(&big)],
        ));

        // Recent turns
        conv.push_user("latest");
        conv.push(Message::new(
            Role::Assistant,
            vec![ContentBlock::text("ok")],
        ));
        conv.push_user("final");

        let config = CompactionConfig::default();
        truncate_preserving_important(&mut conv, 50, &config);

        let msgs = conv.messages();
        let has_error = msgs.iter().any(|m| {
            m.content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolResult { is_error: true, .. }))
        });
        assert!(has_error, "Tool error should be preserved");
    }

    // ── Edge cases ────────────────────────────────────────────────

    #[test]
    fn sliding_window_empty_conversation() {
        let mut conv = make_conv(100_000);
        let config = CompactionConfig::default();
        sliding_window(&mut conv, 5, &config);
        assert!(conv.is_empty());
    }

    #[test]
    fn truncate_empty_conversation() {
        let mut conv = make_conv(100_000);
        let config = CompactionConfig::default();
        truncate_preserving_important(&mut conv, 1000, &config);
        assert!(conv.is_empty());
    }

    #[test]
    fn truncate_within_budget_is_noop() {
        let mut conv = make_conv(100_000);
        conv.push_user("hello");
        conv.push_assistant("hi");
        let before = conv.len();

        let config = CompactionConfig::default();
        truncate_preserving_important(&mut conv, 100_000, &config);

        assert_eq!(conv.len(), before);
    }
}
