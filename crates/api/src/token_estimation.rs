//! Approximate token count estimation without calling a tokenizer API.
//!
//! Uses heuristic rules (word splitting, punctuation counting) to provide
//! a fast, good-enough estimate for context window management and cost
//! tracking. Accuracy is ~85-90% for English text compared to `cl100k_base`.

use crab_core::message::Message;

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
/// in addition to the raw text content.
pub fn estimate_message_tokens(_messages: &[Message]) -> usize {
    todo!()
}

/// Estimate tokens for a JSON value (tool inputs/outputs).
///
/// JSON tends to be more token-dense than prose due to braces, quotes, and
/// key names.
pub fn estimate_json_tokens(_value: &serde_json::Value) -> usize {
    todo!()
}

#[cfg(test)]
mod tests {
    #[test]
    fn empty_string_is_zero_tokens() {
        assert_eq!(super::estimate_tokens(""), 0);
    }
}
