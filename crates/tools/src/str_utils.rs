//! Small string utilities shared by tool implementations.
//!
//! These exist because many tools produce user-visible one-line summaries
//! (`format_use_summary`) with length caps. Slicing by byte index on a
//! multi-byte UTF-8 string panics when the index lands in the middle of a
//! codepoint — a real hazard when the input can come from web responses,
//! user shell commands, or LLM-generated text.

pub use crab_utils::text::truncate_chars;

/// Truncate large text keeping the head (2/3) and tail (1/3) with an
/// omission marker between them.
///
/// Returns the original text unchanged when its character count is at or
/// below `max_chars`. The character count is by Unicode scalar values
/// (`str::chars()`), matching [`truncate_chars`].
///
/// Output format: `"{head}\n\n[...{omitted} characters omitted...]\n\n{tail}"`.
///
/// # Examples
///
/// ```
/// use crab_tools::str_utils::truncate_with_preview;
///
/// assert_eq!(truncate_with_preview("hello", 10), "hello");
/// let big: String = "x".repeat(1000);
/// let out = truncate_with_preview(&big, 60);
/// assert!(out.contains("characters omitted"));
/// assert!(out.len() < big.len());
/// ```
#[must_use]
pub fn truncate_with_preview(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_string();
    }
    let head_chars = max_chars * 2 / 3;
    let tail_chars = max_chars - head_chars;
    let omitted = total.saturating_sub(head_chars + tail_chars);

    let head: String = text.chars().take(head_chars).collect();
    let tail: String = text.chars().skip(total - tail_chars).collect();

    format!("{head}\n\n[...{omitted} characters omitted...]\n\n{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_short_text_unchanged() {
        assert_eq!(truncate_with_preview("hello", 100), "hello");
    }

    #[test]
    fn preview_at_limit_unchanged() {
        let s = "abcdef";
        assert_eq!(truncate_with_preview(s, 6), s);
    }

    #[test]
    fn preview_truncates_long_text_with_marker() {
        let big: String = "x".repeat(1000);
        let out = truncate_with_preview(&big, 60);
        assert!(out.contains("characters omitted"));
        // head=40, tail=20, omitted=940
        assert!(out.contains("940 characters omitted"));
        // Original was 1000 'x'; output should be much shorter
        assert!(out.chars().count() < big.chars().count());
    }

    #[test]
    fn preview_keeps_head_two_thirds_and_tail_one_third() {
        // 90 unique chars: 'a' x 60 + 'b' x 30
        let s: String = std::iter::repeat_n('a', 60)
            .chain(std::iter::repeat_n('b', 30))
            .collect();
        // max=30 → head=20 'a', tail=10 'b', omitted=60
        let out = truncate_with_preview(&s, 30);
        assert!(out.starts_with(&"a".repeat(20)));
        assert!(out.ends_with(&"b".repeat(10)));
        assert!(out.contains("60 characters omitted"));
    }

    #[test]
    fn preview_multibyte_safe() {
        // 100 Chinese chars (each 3 bytes), max=30 → keep 30 chars total
        let s: String = "你".repeat(100);
        let out = truncate_with_preview(&s, 30);
        assert!(out.contains("70 characters omitted"));
        // Output is valid UTF-8 (no panics from byte-slicing inside codepoints)
        assert!(
            out.chars()
                .all(|c| c == '你' || !c.is_alphabetic() || c.is_ascii())
        );
    }
}
