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
        // "你好世界" = 8 columns (each char is 2)
        // max_width=5 can fit "你好" (4) but not "你好世" (6)
        assert_eq!(truncate_to_width("你好世界", 5), "你好");
    }

    #[test]
    fn truncate_zero_width() {
        assert_eq!(truncate_to_width("hello", 0), "");
    }

    #[test]
    fn truncate_mixed_ascii_cjk() {
        // "a你b" = 1 + 2 + 1 = 4 columns
        assert_eq!(truncate_to_width("a你b", 3), "a你");
        assert_eq!(truncate_to_width("a你b", 4), "a你b");
    }

    #[test]
    fn truncate_empty() {
        assert_eq!(truncate_to_width("", 5), "");
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
