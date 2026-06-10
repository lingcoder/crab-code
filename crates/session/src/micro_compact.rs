//! Micro-compaction: replace individual large tool results with summaries.
//!
//! Unlike full conversation compaction (which re-summarizes the entire history),
//! micro-compaction targets individual tool results that exceed a token threshold.
//! This preserves the conversation structure while reducing token usage for
//! bloated tool outputs (e.g., large file reads, verbose command output).
//!
//! Maps to Claude Code's `microCompact.ts`.
//!
//! # Strategy
//!
//! 1. Scan messages for `ToolResult` content blocks.
//! 2. Estimate tokens for each result (chars / 4 heuristic).
//! 3. If a result exceeds `max_tool_result_tokens`, replace it with a
//!    summary targeting `summary_target_tokens`.
//! 4. Return the modified message list and statistics.
//!
//! # Relationship to `compaction.rs`
//!
//! The `CompactionStrategy::Microcompact` level in `compaction.rs` delegates
//! to this module for the actual per-result replacement logic.

use crab_core::message::{ContentBlock, Message};

// ─── Configuration ──────────────────────────────────────────────────────

/// Configuration for micro-compaction.
#[derive(Debug, Clone)]
pub struct MicroCompactConfig {
    /// Maximum token count for a single tool result before it is compacted.
    /// Results with estimated tokens above this threshold are summarized.
    pub max_tool_result_tokens: usize,
    /// Target token count for the summary that replaces a compacted result.
    pub summary_target_tokens: usize,
}

impl Default for MicroCompactConfig {
    fn default() -> Self {
        Self {
            max_tool_result_tokens: 500,
            summary_target_tokens: 100,
        }
    }
}

// ─── Result ─────────────────────────────────────────────────────────────

/// Result of micro-compaction applied to a conversation.
#[derive(Debug, Clone)]
pub struct MicroCompactResult {
    /// The messages after compaction (with large tool results replaced).
    pub messages: Vec<Message>,
    /// Number of tool results that were compacted.
    pub compacted_count: usize,
    /// Estimated total tokens saved by compaction.
    pub tokens_saved: usize,
}

// ─── Public API ─────────────────────────────────────────────────────────

/// Micro-compact: replace individual large tool results with summaries,
/// without re-summarizing the entire conversation.
///
/// Scans all messages for `ToolResult` content blocks that exceed the
/// configured token threshold and replaces their content with brief
/// summaries.
///
/// Messages that do not contain tool results are passed through unchanged.
///
/// # Arguments
///
/// * `messages` — the conversation messages to scan
/// * `config` — micro-compaction thresholds
///
/// # Returns
///
/// A [`MicroCompactResult`] with the (possibly modified) messages and
/// statistics about what was compacted.
pub fn micro_compact(messages: &[Message], config: &MicroCompactConfig) -> MicroCompactResult {
    let mut result_messages = Vec::with_capacity(messages.len());
    let mut compacted_count = 0;
    let mut tokens_saved = 0;

    for msg in messages {
        let mut new_content = Vec::with_capacity(msg.content.len());
        let mut message_modified = false;

        for block in &msg.content {
            match block {
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } if !is_error && should_compact_result(content, config.max_tool_result_tokens) => {
                    let original_tokens = estimate_tokens(content);
                    let summary =
                        summarize_tool_result("tool", content, config.summary_target_tokens);
                    let summary_tokens = estimate_tokens(&summary);

                    tokens_saved += original_tokens.saturating_sub(summary_tokens);
                    compacted_count += 1;
                    message_modified = true;

                    new_content.push(ContentBlock::ToolResult {
                        tool_use_id: tool_use_id.clone(),
                        content: summary,
                        is_error: *is_error,
                    });
                }
                _ => {
                    new_content.push(block.clone());
                }
            }
        }

        if message_modified {
            result_messages.push(Message::new(msg.role, new_content));
        } else {
            result_messages.push(msg.clone());
        }
    }

    MicroCompactResult {
        messages: result_messages,
        compacted_count,
        tokens_saved,
    }
}

/// Check if a specific tool result exceeds the token threshold.
///
/// Uses a rough `chars / 4` heuristic for token estimation.
#[must_use]
pub fn should_compact_result(result: &str, max_tokens: usize) -> bool {
    estimate_tokens(result) > max_tokens
}

/// Generate a brief summary of a tool result.
///
/// Uses heuristics to preserve the most useful parts of the output:
/// - **Error messages**: kept in full (errors are always relevant).
/// - **JSON results**: structure is preserved; large string values are truncated.
/// - **Line-based output**: first N + last N lines with an omission marker.
///
/// # Arguments
///
/// * `tool_name` — name of the tool that produced the result
/// * `result` — the original tool result text
/// * `target_tokens` — target summary length in tokens
pub fn summarize_tool_result(tool_name: &str, result: &str, target_tokens: usize) -> String {
    let original_tokens = estimate_tokens(result);
    let target_chars = target_tokens * 4; // reverse the heuristic

    if result.len() <= target_chars {
        return result.to_string();
    }

    // Try JSON-aware summarization first.
    if let Some(json_summary) = try_summarize_json(result, target_chars) {
        return format!(
            "[micro-compacted: {tool_name} JSON output ({original_tokens} tokens -> ~{target_tokens} tokens)]\n\
             {json_summary}"
        );
    }

    // Line-based: extract first and last lines.
    let lines: Vec<&str> = result.lines().collect();
    let total_lines = lines.len();
    let target_lines = (target_chars / 80).max(6); // rough lines-to-show estimate
    let head_lines = target_lines * 2 / 3;
    let tail_lines = target_lines / 3;

    if total_lines <= head_lines + tail_lines + 2 {
        // Not enough lines to meaningfully truncate; fall back to char-level.
        let head = safe_truncate(result, target_chars * 2 / 3);
        let tail = safe_tail(result, target_chars / 3);
        return format!(
            "[micro-compacted: {tool_name} output ({original_tokens} tokens -> ~{target_tokens} tokens)]\n\
             {head}\n\
             [...{} tokens omitted...]\n\
             {tail}",
            original_tokens.saturating_sub(target_tokens)
        );
    }

    let omitted = total_lines - head_lines - tail_lines;
    let head = lines[..head_lines].join("\n");
    let tail = lines[total_lines - tail_lines..].join("\n");

    format!(
        "[micro-compacted: {tool_name} output ({total_lines} lines, {original_tokens} tokens -> ~{target_tokens} tokens)]\n\
         {head}\n\
         [... {omitted} lines omitted ...]\n\
         {tail}"
    )
}

/// Try to produce a JSON-aware summary by truncating large string values.
///
/// Returns `None` if the input is not valid JSON or is not an object/array.
fn try_summarize_json(result: &str, target_chars: usize) -> Option<String> {
    let trimmed = result.trim();
    if !(trimmed.starts_with('{') || trimmed.starts_with('[')) {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    let truncated = truncate_json_values(&value, target_chars / 4);
    let output = serde_json::to_string_pretty(&truncated).ok()?;
    Some(output)
}

/// Recursively truncate large string values in a JSON tree.
///
/// Strings longer than `max_str_len` are replaced with a prefix + marker.
fn truncate_json_values(value: &serde_json::Value, max_str_len: usize) -> serde_json::Value {
    match value {
        serde_json::Value::String(s) => {
            if s.len() > max_str_len {
                let prefix = safe_truncate(s, max_str_len);
                serde_json::Value::String(format!(
                    "{prefix}...[truncated: {} chars]",
                    s.len() - prefix.len()
                ))
            } else {
                value.clone()
            }
        }
        serde_json::Value::Array(arr) => {
            let truncated: Vec<_> = arr
                .iter()
                .map(|v| truncate_json_values(v, max_str_len))
                .collect();
            serde_json::Value::Array(truncated)
        }
        serde_json::Value::Object(map) => {
            let truncated: serde_json::Map<_, _> = map
                .iter()
                .map(|(k, v)| (k.clone(), truncate_json_values(v, max_str_len)))
                .collect();
            serde_json::Value::Object(truncated)
        }
        // Numbers, bools, null — pass through unchanged.
        _ => value.clone(),
    }
}

// ─── Internal helpers ───────────────────────────────────────────────────

/// Rough token estimate: chars / 4.
fn estimate_tokens(text: &str) -> usize {
    text.len() / 4
}

/// Safely truncate a string to approximately `max_chars` at a char boundary.
fn safe_truncate(s: &str, max_chars: usize) -> &str {
    if s.len() <= max_chars {
        return s;
    }
    // Find the last char boundary at or before max_chars.
    let mut end = max_chars;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Safely get the last `max_chars` of a string at a char boundary.
fn safe_tail(s: &str, max_chars: usize) -> &str {
    if s.len() <= max_chars {
        return s;
    }
    let start = s.len() - max_chars;
    let mut adjusted = start;
    while adjusted < s.len() && !s.is_char_boundary(adjusted) {
        adjusted += 1;
    }
    &s[adjusted..]
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::message::Role;

    fn make_tool_result_msg(tool_use_id: &str, content: &str, is_error: bool) -> Message {
        Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content: content.to_string(),
                is_error,
            }],
        )
    }

    fn make_text_msg(role: Role, text: &str) -> Message {
        Message::new(role, vec![ContentBlock::text(text)])
    }

    // ── Config ─────────────────────────────────────────────────────

    #[test]
    fn default_config() {
        let config = MicroCompactConfig::default();
        assert_eq!(config.max_tool_result_tokens, 500);
        assert_eq!(config.summary_target_tokens, 100);
    }

    // ── should_compact_result ──────────────────────────────────────

    #[test]
    fn short_result_not_compacted() {
        assert!(!should_compact_result("short text", 100));
    }

    #[test]
    fn long_result_is_compacted() {
        let long_text = "x".repeat(2004); // 2004 chars / 4 = 501 tokens > 500
        assert!(should_compact_result(&long_text, 500));
    }

    #[test]
    fn result_at_threshold_not_compacted() {
        let text = "x".repeat(2000); // exactly 500 tokens
        assert!(!should_compact_result(&text, 500));
    }

    // ── summarize_tool_result ──────────────────────────────────────

    #[test]
    fn summarize_short_result_unchanged() {
        let result = "short output";
        let summary = summarize_tool_result("bash", result, 100);
        assert_eq!(summary, result);
    }

    #[test]
    fn summarize_long_result_truncated() {
        let long_result = "x".repeat(10000);
        let summary = summarize_tool_result("bash", &long_result, 50);
        assert!(summary.contains("[micro-compacted:"));
        assert!(summary.contains("omitted"));
        assert!(summary.len() < long_result.len());
    }

    // ── micro_compact ──────────────────────────────────────────────

    #[test]
    fn compact_empty_messages() {
        let result = micro_compact(&[], &MicroCompactConfig::default());
        assert!(result.messages.is_empty());
        assert_eq!(result.compacted_count, 0);
        assert_eq!(result.tokens_saved, 0);
    }

    #[test]
    fn compact_no_tool_results() {
        let messages = vec![
            make_text_msg(Role::User, "hello"),
            make_text_msg(Role::Assistant, "hi there"),
        ];
        let result = micro_compact(&messages, &MicroCompactConfig::default());
        assert_eq!(result.messages.len(), 2);
        assert_eq!(result.compacted_count, 0);
    }

    #[test]
    fn compact_small_tool_results_unchanged() {
        let messages = vec![make_tool_result_msg("id1", "short output", false)];
        let config = MicroCompactConfig::default();
        let result = micro_compact(&messages, &config);
        assert_eq!(result.compacted_count, 0);

        match &result.messages[0].content[0] {
            ContentBlock::ToolResult { content, .. } => {
                assert_eq!(content, "short output");
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn compact_large_tool_result() {
        let large_output = "x".repeat(10000); // ~2500 tokens, way above default 500
        let messages = vec![make_tool_result_msg("id1", &large_output, false)];
        let config = MicroCompactConfig::default();
        let result = micro_compact(&messages, &config);

        assert_eq!(result.compacted_count, 1);
        assert!(result.tokens_saved > 0);

        match &result.messages[0].content[0] {
            ContentBlock::ToolResult { content, .. } => {
                assert!(content.contains("[micro-compacted:"));
                assert!(content.len() < large_output.len());
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn compact_preserves_error_results() {
        let large_error = "e".repeat(10000);
        let messages = vec![make_tool_result_msg("id1", &large_error, true)];
        let config = MicroCompactConfig::default();
        let result = micro_compact(&messages, &config);

        // Error results should NOT be compacted.
        assert_eq!(result.compacted_count, 0);
        match &result.messages[0].content[0] {
            ContentBlock::ToolResult { content, .. } => {
                assert_eq!(content.len(), large_error.len());
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn compact_mixed_messages() {
        let large = "x".repeat(10000);
        let messages = vec![
            make_text_msg(Role::User, "do something"),
            make_text_msg(Role::Assistant, "calling tool..."),
            make_tool_result_msg("id1", &large, false),
            make_tool_result_msg("id2", "small", false),
            make_text_msg(Role::Assistant, "done"),
        ];

        let config = MicroCompactConfig::default();
        let result = micro_compact(&messages, &config);

        assert_eq!(result.messages.len(), 5);
        assert_eq!(result.compacted_count, 1); // only the large one
    }

    // ── Helper tests ───────────────────────────────────────────────

    #[test]
    fn estimate_tokens_basic() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("1234"), 1);
        assert_eq!(estimate_tokens("12345678"), 2);
    }

    #[test]
    fn safe_truncate_within_bounds() {
        assert_eq!(safe_truncate("hello", 10), "hello");
    }

    #[test]
    fn safe_truncate_at_boundary() {
        assert_eq!(safe_truncate("hello world", 5), "hello");
    }

    #[test]
    fn safe_tail_within_bounds() {
        assert_eq!(safe_tail("hello", 10), "hello");
    }

    #[test]
    fn safe_tail_at_boundary() {
        assert_eq!(safe_tail("hello world", 5), "world");
    }

    // ── JSON-aware summarization ──────────────────────────────────

    #[test]
    fn summarize_json_result_truncates_long_strings() {
        let big = "x".repeat(5000);
        let json_str = serde_json::to_string(&serde_json::json!({
            "name": "test",
            "data": big
        }))
        .unwrap();
        let summary = summarize_tool_result("bash", &json_str, 50);
        assert!(summary.contains("micro-compacted"));
        assert!(summary.len() < json_str.len());
        assert!(summary.contains("name"));
    }

    #[test]
    fn summarize_non_json_falls_back_to_line_based() {
        let lines = (0..200)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let summary = summarize_tool_result("bash", &lines, 20);
        assert!(summary.contains("micro-compacted"));
        assert!(summary.contains("lines omitted"));
    }

    #[test]
    fn try_summarize_json_returns_none_for_plain_text() {
        assert!(try_summarize_json("not json at all", 100).is_none());
    }

    #[test]
    fn try_summarize_json_returns_none_for_invalid_json() {
        assert!(try_summarize_json("{invalid json!!!}", 100).is_none());
    }

    #[test]
    fn try_summarize_json_works_for_object() {
        let json = r#"{"key": "value", "num": 42}"#;
        let result = try_summarize_json(json, 1000).unwrap();
        assert!(result.contains("key"));
        assert!(result.contains("value"));
    }

    #[test]
    fn truncate_json_values_preserves_structure() {
        let value = serde_json::json!({
            "short": "ok",
            "long": "x".repeat(1000),
            "nested": {
                "inner": "y".repeat(500)
            }
        });
        let truncated = truncate_json_values(&value, 50);
        let obj = truncated.as_object().unwrap();
        assert_eq!(obj["short"], "ok");
        assert!(obj["long"].as_str().unwrap().contains("truncated"));
        let nested = obj["nested"].as_object().unwrap();
        assert!(nested["inner"].as_str().unwrap().contains("truncated"));
    }

    #[test]
    fn truncate_json_values_preserves_non_strings() {
        let value = serde_json::json!({
            "num": 42,
            "bool": true,
            "null_val": null
        });
        let truncated = truncate_json_values(&value, 5);
        assert_eq!(truncated["num"], 42);
        assert_eq!(truncated["bool"], true);
        assert!(truncated["null_val"].is_null());
    }
}
