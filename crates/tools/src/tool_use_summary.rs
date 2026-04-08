//! Tool result summarization for context compression.
//!
//! When tool results are large, this module provides functions to create
//! concise summaries that preserve the essential information while reducing
//! context window usage.

/// Default threshold in characters above which results should be summarized.
#[allow(dead_code)]
const DEFAULT_THRESHOLD: usize = 8000;

/// Check whether a tool result exceeds the given threshold and should be summarized.
///
/// Returns `true` if the result length exceeds `threshold`.
#[must_use]
pub fn should_summarize(result: &str, threshold: usize) -> bool {
    result.len() > threshold
}

/// Summarize a tool result for context compression.
///
/// The summarization strategy depends on the tool:
/// - For file-reading tools: keep first/last lines, elide middle
/// - For search tools: keep match counts and top results
/// - For shell tools: keep exit code, stderr, and tail of stdout
/// - Default: head + tail truncation with character count
///
/// # Arguments
/// - `tool_name`: Name of the tool that produced the result
/// - `result`: The full tool result text
/// - `max_tokens`: Approximate token budget for the summary
#[must_use]
pub fn summarize_tool_result(tool_name: &str, result: &str, max_tokens: usize) -> String {
    let _ = (tool_name, result, max_tokens);
    todo!("summarize_tool_result — implement per-tool summarization strategies")
}

/// Estimate the number of tokens in a string (rough approximation).
///
/// Uses the rule of thumb: ~4 characters per token for English text.
#[must_use]
pub fn estimate_tokens(text: &str) -> usize {
    text.len() / 4
}

/// Create a head+tail summary with an elision marker.
///
/// Keeps the first `head_chars` and last `tail_chars` characters,
/// replacing the middle with an elision message.
#[must_use]
pub fn head_tail_summary(text: &str, head_chars: usize, tail_chars: usize) -> String {
    let total = head_chars + tail_chars;
    if text.len() <= total {
        return text.to_owned();
    }
    let elided = text.len() - total;
    format!(
        "{}\n\n... [{elided} characters elided] ...\n\n{}",
        &text[..head_chars],
        &text[text.len() - tail_chars..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_summarize_short_text() {
        assert!(!should_summarize("short", DEFAULT_THRESHOLD));
    }

    #[test]
    fn should_summarize_long_text() {
        let long = "x".repeat(DEFAULT_THRESHOLD + 1);
        assert!(should_summarize(&long, DEFAULT_THRESHOLD));
    }

    #[test]
    fn should_summarize_exact_threshold() {
        let exact = "x".repeat(DEFAULT_THRESHOLD);
        assert!(!should_summarize(&exact, DEFAULT_THRESHOLD));
    }

    #[test]
    fn estimate_tokens_approximation() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("a".repeat(400).as_str()), 100);
    }

    #[test]
    fn head_tail_short_text_unchanged() {
        assert_eq!(head_tail_summary("hello", 10, 10), "hello");
    }

    #[test]
    fn head_tail_long_text_elides() {
        let text = "a".repeat(100);
        let result = head_tail_summary(&text, 20, 20);
        assert!(result.contains("[60 characters elided]"));
        assert!(result.starts_with(&"a".repeat(20)));
        assert!(result.ends_with(&"a".repeat(20)));
    }
}
