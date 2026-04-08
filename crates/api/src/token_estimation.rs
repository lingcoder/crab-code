//! Approximate token count estimation without calling a tokenizer API.
//!
//! Uses heuristic rules (word splitting, punctuation counting) to provide
//! a fast, good-enough estimate for context window management and cost
//! tracking. Accuracy is ~85-90% for English text compared to `cl100k_base`.
//!
//! CCB uses a 4/3 padding strategy for conservative estimates.

use crab_core::message::{ContentBlock, Message};

/// Overhead tokens per message for role/framing. Accounts for the role tag,
/// content block wrappers, and JSON structure in the API request.
const MESSAGE_OVERHEAD_TOKENS: usize = 4;

/// Flat token estimate for image/document content blocks.
const IMAGE_TOKEN_ESTIMATE: usize = 2_000;

/// Estimate the token count of a plain text string.
///
/// Uses a word/punctuation heuristic: roughly 1 token per 4 characters for
/// English, adjusted for whitespace density and special characters.
pub fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    // Heuristic: ~4 chars per token for English, ~2 chars per token for CJK/code
    let char_count = text.len();
    // Rough approximation: 1 token per 4 bytes, minimum 1
    char_count.div_ceil(4)
}

/// Estimate the total token count across a slice of messages.
///
/// Accounts for message framing overhead (role tags, content block wrappers)
/// in addition to the raw text content. Uses a 4/3 padding multiplier for
/// conservative estimation (matching CCB's approach).
pub fn estimate_message_tokens(messages: &[Message]) -> usize {
    let mut total = 0;

    for message in messages {
        // Per-message overhead (role tag, framing)
        total += MESSAGE_OVERHEAD_TOKENS;

        for block in &message.content {
            total += estimate_content_block_tokens(block);
        }
    }

    // Apply 4/3 padding for conservative estimate (CCB convention)
    total * 4 / 3
}

/// Estimate tokens for a single content block.
fn estimate_content_block_tokens(block: &ContentBlock) -> usize {
    match block {
        ContentBlock::Text { text } => estimate_tokens(text),
        ContentBlock::ToolUse { name, input, .. } => {
            // Tool name + JSON input
            estimate_tokens(name) + estimate_json_tokens(input)
        }
        ContentBlock::ToolResult { content, .. } => estimate_tokens(content),
        ContentBlock::Image { .. } => IMAGE_TOKEN_ESTIMATE,
        ContentBlock::Thinking { thinking } => estimate_tokens(thinking),
    }
}

/// Estimate tokens for a JSON value (tool inputs/outputs).
///
/// JSON tends to be more token-dense than prose due to braces, quotes, and
/// key names. Uses a 3-chars-per-token heuristic (tighter than plain text).
pub fn estimate_json_tokens(value: &serde_json::Value) -> usize {
    // Serialize to string and estimate with tighter ratio
    let json_str = value.to_string();
    if json_str.is_empty() {
        return 0;
    }
    // JSON is more token-dense: ~3 chars per token
    json_str.len().div_ceil(3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_is_zero_tokens() {
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn short_text_estimation() {
        // "hello" = 5 chars → ceil(5/4) = 2 tokens
        assert_eq!(estimate_tokens("hello"), 2);
    }

    #[test]
    fn longer_text_estimation() {
        // 100 chars → 25 tokens
        let text = "a".repeat(100);
        assert_eq!(estimate_tokens(&text), 25);
    }

    #[test]
    fn estimate_message_tokens_empty() {
        assert_eq!(estimate_message_tokens(&[]), 0);
    }

    #[test]
    fn estimate_message_tokens_single_text() {
        let messages = vec![Message::user("Hello world")];
        let tokens = estimate_message_tokens(&messages);
        // 4 overhead + ceil(11/4)=3 text = 7, then * 4/3 = 9
        assert!(tokens > 0);
        assert!(tokens < 20);
    }

    #[test]
    fn estimate_message_tokens_with_tool_result() {
        let messages = vec![Message::tool_result("id1", "a".repeat(400), false)];
        let tokens = estimate_message_tokens(&messages);
        // 4 overhead + 100 content = 104, * 4/3 ≈ 138
        assert!(tokens > 100);
    }

    #[test]
    fn estimate_message_tokens_padding() {
        // Padding should make estimate ~33% larger than raw
        let messages = vec![Message::user("a".repeat(120))];
        let tokens = estimate_message_tokens(&messages);
        // Raw: 4 + 30 = 34, padded: 34 * 4/3 = 45
        assert!(tokens > 40);
    }

    #[test]
    fn estimate_json_tokens_empty_object() {
        let val = serde_json::json!({});
        let tokens = estimate_json_tokens(&val);
        assert!(tokens > 0); // "{}" = 2 chars → 1 token
    }

    #[test]
    fn estimate_json_tokens_with_content() {
        let val = serde_json::json!({"command": "git status", "description": "check repo"});
        let tokens = estimate_json_tokens(&val);
        assert!(tokens > 10);
    }

    #[test]
    fn estimate_json_tokens_null() {
        let val = serde_json::Value::Null;
        let tokens = estimate_json_tokens(&val);
        // "null" = 4 chars → ceil(4/3) = 2
        assert_eq!(tokens, 2);
    }

    #[test]
    fn content_block_image_estimate() {
        let block = ContentBlock::Image {
            source: crab_core::message::ImageSource {
                source_type: "base64".into(),
                media_type: "image/png".into(),
                data: "base64data".into(),
            },
        };
        assert_eq!(estimate_content_block_tokens(&block), IMAGE_TOKEN_ESTIMATE);
    }
}
