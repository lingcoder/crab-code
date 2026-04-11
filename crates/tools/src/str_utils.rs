//! Small string utilities shared by tool implementations.
//!
//! These exist because many tools produce user-visible one-line summaries
//! (`format_use_summary`) with length caps. Slicing by byte index on a
//! multi-byte UTF-8 string panics when the index lands in the middle of a
//! codepoint — a real hazard when the input can come from web responses,
//! user shell commands, or LLM-generated text.

/// Truncate `s` to at most `max_chars` Unicode scalar values, appending
/// `ellipsis` if any characters were dropped.
///
/// "Characters" here means codepoints (what `str::chars()` yields), not
/// grapheme clusters — a flag emoji or combining sequence may still span
/// multiple counted chars. That's fine for "short summary" truncation: the
/// goal is to avoid byte-slice panics and keep output bounded, not to be
/// grapheme-accurate.
///
/// If `s` already fits, it's returned unchanged (no ellipsis).
///
/// # Examples
///
/// ```
/// use crab_tools::str_utils::truncate_chars;
///
/// assert_eq!(truncate_chars("hello", 10, "…"), "hello");
/// assert_eq!(truncate_chars("hello world", 5, "…"), "hello…");
/// // Multi-byte: each Chinese character is 3 bytes but 1 char.
/// assert_eq!(truncate_chars("你好世界", 2, "…"), "你好…");
/// ```
#[must_use]
pub fn truncate_chars(s: &str, max_chars: usize, ellipsis: &str) -> String {
    // Fast path: count up to max_chars+1 to decide if we need to truncate.
    // `chars().count()` walks the whole string; skip that by checking
    // `nth(max_chars)` which stops early.
    if s.chars().nth(max_chars).is_none() {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars).collect();
    out.push_str(ellipsis);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_shorter_than_limit_unchanged() {
        assert_eq!(truncate_chars("hi", 10, "…"), "hi");
    }

    #[test]
    fn ascii_exactly_at_limit_unchanged() {
        assert_eq!(truncate_chars("hello", 5, "…"), "hello");
    }

    #[test]
    fn ascii_longer_than_limit_truncated_with_ellipsis() {
        assert_eq!(truncate_chars("hello world", 5, "…"), "hello…");
    }

    #[test]
    fn multibyte_counts_by_chars_not_bytes() {
        // "你好世界" is 4 chars, 12 bytes. Byte-slicing at [..3] would panic;
        // char-based truncation at 2 yields "你好…".
        assert_eq!(truncate_chars("你好世界", 2, "…"), "你好…");
    }

    #[test]
    fn multibyte_fits_within_limit_unchanged() {
        assert_eq!(truncate_chars("你好世界", 10, "…"), "你好世界");
    }

    #[test]
    fn mixed_ascii_and_multibyte() {
        // 6 chars: "a", "b", "c", "你", "好", "d"
        assert_eq!(truncate_chars("abc你好d", 4, "…"), "abc你…");
    }

    #[test]
    fn empty_input_unchanged() {
        assert_eq!(truncate_chars("", 10, "…"), "");
        assert_eq!(truncate_chars("", 0, "…"), "");
    }

    #[test]
    fn zero_limit_non_empty_input_yields_ellipsis_only() {
        assert_eq!(truncate_chars("hello", 0, "…"), "…");
    }

    #[test]
    fn custom_ellipsis_suffix() {
        assert_eq!(truncate_chars("abcdef", 3, "[...]"), "abc[...]");
    }

    #[test]
    fn empty_ellipsis_truncates_silently() {
        assert_eq!(truncate_chars("abcdef", 3, ""), "abc");
    }

    #[test]
    fn emoji_counted_as_chars() {
        // Each emoji here is a single codepoint (not a ZWJ sequence).
        // "🦀🦀🦀" is 3 chars, 12 bytes.
        assert_eq!(truncate_chars("🦀🦀🦀🦀", 2, "…"), "🦀🦀…");
    }
}
