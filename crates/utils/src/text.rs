/// Returns the display width of a string, accounting for ANSI escapes and Unicode widths.
#[must_use]
pub fn display_width(s: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(strip_ansi(s).as_str())
}

/// Strips ANSI escape sequences from a string.
#[must_use]
pub fn strip_ansi(s: &str) -> String {
    let bytes = strip_ansi_escapes::strip(s);
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Truncates `s` to at most `max_chars` Unicode scalar values, appending
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
/// use crab_utils::text::truncate_chars;
///
/// assert_eq!(truncate_chars("hello", 10, "…"), "hello");
/// assert_eq!(truncate_chars("hello world", 5, "…"), "hello…");
/// ```
#[must_use]
pub fn truncate_chars(s: &str, max_chars: usize, ellipsis: &str) -> String {
    // Fast path: `nth(max_chars)` stops early instead of walking the whole
    // string the way `chars().count()` would.
    if s.chars().nth(max_chars).is_none() {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars).collect();
    out.push_str(ellipsis);
    out
}

/// Truncates `s` so the *result* — ellipsis included — is at most
/// `max_chars` codepoints.
///
/// Unlike [`truncate_chars`] (which keeps `max_chars` of content and then
/// appends the ellipsis), this enforces a total output budget. If
/// `max_chars` is smaller than the ellipsis itself, the bare ellipsis is
/// returned and exceeds the budget.
///
/// # Examples
///
/// ```
/// use crab_utils::text::truncate_chars_within;
///
/// assert_eq!(truncate_chars_within("hello world!", 8, "..."), "hello...");
/// assert_eq!(truncate_chars_within("abc", 3, "..."), "abc");
/// ```
#[must_use]
pub fn truncate_chars_within(s: &str, max_chars: usize, ellipsis: &str) -> String {
    if s.chars().nth(max_chars).is_none() {
        return s.to_string();
    }
    let keep = max_chars.saturating_sub(ellipsis.chars().count());
    truncate_chars(s, keep, ellipsis)
}

/// Truncates a string to fit within `max_width` display columns.
/// Handles CJK double-width characters correctly.
#[must_use]
pub fn truncate_to_width(s: &str, max_width: usize) -> String {
    let mut width = 0;
    let mut result = String::new();
    for ch in s.chars() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + w > max_width {
            break;
        }
        width += w;
        result.push(ch);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_width_ascii() {
        assert_eq!(display_width("hello"), 5);
    }

    #[test]
    fn display_width_empty() {
        assert_eq!(display_width(""), 0);
    }

    #[test]
    fn display_width_cjk() {
        // Each CJK character is 2 columns wide
        assert_eq!(display_width("你好"), 4);
        assert_eq!(display_width("ab你好cd"), 8);
    }

    #[test]
    fn display_width_with_ansi() {
        // ANSI escape codes should not count toward width
        assert_eq!(display_width("\x1b[31mred\x1b[0m"), 3);
        assert_eq!(display_width("\x1b[1;32mbold green\x1b[0m"), 10);
    }

    #[test]
    fn strip_ansi_removes_escapes() {
        assert_eq!(strip_ansi("\x1b[31mhello\x1b[0m"), "hello");
    }

    #[test]
    fn strip_ansi_no_escapes() {
        assert_eq!(strip_ansi("plain text"), "plain text");
    }

    #[test]
    fn strip_ansi_empty() {
        assert_eq!(strip_ansi(""), "");
    }

    #[test]
    fn truncate_ascii() {
        assert_eq!(truncate_to_width("hello world", 5), "hello");
    }

    #[test]
    fn truncate_exact_fit() {
        assert_eq!(truncate_to_width("hello", 5), "hello");
    }

    #[test]
    fn truncate_longer_than_string() {
        assert_eq!(truncate_to_width("hi", 10), "hi");
    }

    #[test]
    fn truncate_cjk_boundary() {
        // 4 CJK characters at 2 columns each = 8 total columns. With
        // max_width=5, the first 2 chars (4 cols) fit but adding a third
        // would overflow to 6, so truncation stops after 2.
        assert_eq!(truncate_to_width("你好世界", 5), "你好");
    }

    #[test]
    fn truncate_zero_width() {
        assert_eq!(truncate_to_width("hello", 0), "");
    }

    #[test]
    fn truncate_mixed_ascii_cjk() {
        // ASCII (1 col) + CJK (2 cols) + ASCII (1 col) = 4 total columns.
        // max_width=3 truncates after the CJK char; max_width=4 fits all.
        assert_eq!(truncate_to_width("a你b", 3), "a你");
        assert_eq!(truncate_to_width("a你b", 4), "a你b");
    }

    #[test]
    fn truncate_empty() {
        assert_eq!(truncate_to_width("", 5), "");
    }

    #[test]
    fn truncate_chars_ascii_within_limit_unchanged() {
        assert_eq!(truncate_chars("hi", 10, "…"), "hi");
        assert_eq!(truncate_chars("hello", 5, "…"), "hello");
    }

    #[test]
    fn truncate_chars_ascii_over_limit() {
        assert_eq!(truncate_chars("hello world", 5, "…"), "hello…");
    }

    #[test]
    fn truncate_chars_multibyte_counts_by_chars_not_bytes() {
        // 4-char CJK input occupies 12 bytes. Byte-slicing at [..3] would
        // panic; char-based truncation at 2 keeps the first 2 characters.
        assert_eq!(truncate_chars("你好世界", 2, "…"), "你好…");
        assert_eq!(truncate_chars("你好世界", 10, "…"), "你好世界");
    }

    #[test]
    fn truncate_chars_mixed_ascii_and_multibyte() {
        assert_eq!(truncate_chars("abc你好d", 4, "…"), "abc你…");
    }

    #[test]
    fn truncate_chars_empty_input_unchanged() {
        assert_eq!(truncate_chars("", 10, "…"), "");
        assert_eq!(truncate_chars("", 0, "…"), "");
    }

    #[test]
    fn truncate_chars_zero_limit_yields_ellipsis_only() {
        assert_eq!(truncate_chars("hello", 0, "…"), "…");
    }

    #[test]
    fn truncate_chars_custom_and_empty_ellipsis() {
        assert_eq!(truncate_chars("abcdef", 3, "[...]"), "abc[...]");
        assert_eq!(truncate_chars("abcdef", 3, ""), "abc");
    }

    #[test]
    fn truncate_chars_emoji_counted_as_chars() {
        // Each emoji here is a single codepoint (not a ZWJ sequence).
        assert_eq!(truncate_chars("🦀🦀🦀🦀", 2, "…"), "🦀🦀…");
    }

    #[test]
    fn truncate_chars_within_fits_unchanged() {
        assert_eq!(truncate_chars_within("abc", 3, "..."), "abc");
        assert_eq!(truncate_chars_within("hi", 10, "..."), "hi");
    }

    #[test]
    fn truncate_chars_within_total_budget_includes_ellipsis() {
        let out = truncate_chars_within("hello world!", 8, "...");
        assert_eq!(out, "hello...");
        assert_eq!(out.chars().count(), 8);
    }

    #[test]
    fn truncate_chars_within_multibyte() {
        let out = truncate_chars_within("你好世界你好世界", 5, "…");
        assert_eq!(out, "你好世界…");
        assert_eq!(out.chars().count(), 5);
    }

    #[test]
    fn truncate_chars_within_budget_smaller_than_ellipsis() {
        assert_eq!(truncate_chars_within("hello", 2, "..."), "...");
    }

    #[test]
    fn display_width_emoji() {
        // Emoji should have non-zero width
        let w = display_width("\u{1F600}"); // grinning face
        assert!(w > 0);
    }

    #[test]
    fn strip_ansi_nested_escapes() {
        let input = "\x1b[1m\x1b[31mbold red\x1b[0m normal";
        assert_eq!(strip_ansi(input), "bold red normal");
    }

    #[test]
    fn truncate_at_width_one() {
        assert_eq!(truncate_to_width("abc", 1), "a");
        // CJK char is width 2, doesn't fit in width 1
        assert_eq!(truncate_to_width("\u{4f60}\u{597d}", 1), "");
    }
}
