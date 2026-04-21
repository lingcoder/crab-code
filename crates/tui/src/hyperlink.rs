//! OSC 8 terminal hyperlink support.
//!
//! Wraps text in OSC 8 escape sequences so terminals that support it
//! render clickable hyperlinks. Falls back to plain text on terminals
//! that don't support the protocol.

use crate::terminal_notify::TerminalKind;

/// Check if the current terminal supports OSC 8 hyperlinks.
#[must_use]
pub fn supports_hyperlinks() -> bool {
    match TerminalKind::detect() {
        TerminalKind::ITerm2
        | TerminalKind::Kitty
        | TerminalKind::WezTerm
        | TerminalKind::Ghostty
        | TerminalKind::VsCode => true,
        TerminalKind::Unknown => false,
    }
}

/// Wrap display text in an OSC 8 hyperlink escape sequence.
///
/// Format: `ESC]8;;url ST text ESC]8;; ST`
/// where ST (String Terminator) is `ESC\`.
#[must_use]
pub fn wrap_hyperlink(url: &str, text: &str) -> String {
    format!("\x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\")
}

/// Format a link for terminal display.
///
/// If the terminal supports OSC 8, returns wrapped hyperlink text.
/// Otherwise returns `text (url)` format as fallback.
#[must_use]
pub fn format_link(url: &str, text: &str) -> String {
    if supports_hyperlinks() {
        wrap_hyperlink(url, text)
    } else if url.is_empty() || text == url {
        text.to_string()
    } else {
        format!("{text} ({url})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_hyperlink_format() {
        let result = wrap_hyperlink("https://example.com", "click");
        assert_eq!(
            result,
            "\x1b]8;;https://example.com\x1b\\click\x1b]8;;\x1b\\"
        );
    }

    #[test]
    fn wrap_hyperlink_empty_url() {
        let result = wrap_hyperlink("", "text");
        assert_eq!(result, "\x1b]8;;\x1b\\text\x1b]8;;\x1b\\");
    }

    #[test]
    fn format_link_fallback_when_different() {
        // In test environment, TERM_PROGRAM is likely unset → Unknown → fallback
        let result = format_link("https://example.com", "click");
        // Either OSC 8 or fallback format depending on env
        assert!(
            result.contains("click"),
            "result should contain display text"
        );
    }

    #[test]
    fn format_link_same_url_and_text() {
        let url = "https://example.com";
        let result = format_link(url, url);
        assert!(result.contains("https://example.com"));
        // Should NOT duplicate as "url (url)" — just show once
        assert!(!result.contains("(https://example.com)"));
    }
}
