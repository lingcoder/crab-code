//! OS-level terminal notification protocol.
//!
//! Detects the terminal emulator and sends native notifications via OSC
//! escape sequences when supported. Falls back to BEL for unsupported
//! terminals.

use std::io::Write;

/// Known terminal emulators with notification support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalKind {
    ITerm2,
    Kitty,
    WezTerm,
    Ghostty,
    VsCode,
    Unknown,
}

impl TerminalKind {
    /// Detect the current terminal from environment variables.
    #[must_use]
    pub fn detect() -> Self {
        if let Ok(program) = std::env::var("TERM_PROGRAM") {
            match program.as_str() {
                "iTerm.app" => return Self::ITerm2,
                "WezTerm" => return Self::WezTerm,
                "ghostty" => return Self::Ghostty,
                "vscode" => return Self::VsCode,
                _ => {}
            }
        }
        if std::env::var("KITTY_PID").is_ok() || std::env::var("KITTY_WINDOW_ID").is_ok() {
            return Self::Kitty;
        }
        Self::Unknown
    }

    /// Whether this terminal supports native notifications.
    #[must_use]
    pub fn supports_notification(self) -> bool {
        !matches!(self, Self::Unknown)
    }
}

/// Send a terminal notification if the terminal supports it.
///
/// - `iTerm2`: OSC 9 (growl-style notification)
/// - `Kitty`: OSC 99 (desktop notification protocol)
/// - `WezTerm`: OSC 9 (same as `iTerm2`)
/// - `Ghostty`: OSC 777 (rxvt notification)
/// - VS Code: BEL (terminal integrated notification)
/// - Unknown: BEL fallback
pub fn notify(title: &str, body: &str) {
    let kind = TerminalKind::detect();
    let sequence = format_notification(kind, title, body);
    let _ = std::io::stdout().write_all(sequence.as_bytes());
    let _ = std::io::stdout().flush();
}

/// Format the notification escape sequence for a given terminal.
#[must_use]
pub fn format_notification(kind: TerminalKind, title: &str, body: &str) -> String {
    match kind {
        TerminalKind::ITerm2 | TerminalKind::WezTerm => {
            format!("\x1b]9;{body}\x07")
        }
        TerminalKind::Kitty => {
            format!("\x1b]99;i=1:d=0;{title}\x1b\\")
        }
        TerminalKind::Ghostty => {
            format!("\x1b]777;notify;{title};{body}\x1b\\")
        }
        TerminalKind::VsCode | TerminalKind::Unknown => {
            "\x07".to_string()
        }
    }
}

/// Send a BEL character (audible/visual bell).
pub fn bell() {
    let _ = std::io::stdout().write_all(b"\x07");
    let _ = std::io::stdout().flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iterm2_format() {
        let s = format_notification(TerminalKind::ITerm2, "Crab", "Done");
        assert_eq!(s, "\x1b]9;Done\x07");
    }

    #[test]
    fn kitty_format() {
        let s = format_notification(TerminalKind::Kitty, "Crab", "Done");
        assert_eq!(s, "\x1b]99;i=1:d=0;Crab\x1b\\");
    }

    #[test]
    fn wezterm_format() {
        let s = format_notification(TerminalKind::WezTerm, "Crab", "Done");
        assert_eq!(s, "\x1b]9;Done\x07");
    }

    #[test]
    fn ghostty_format() {
        let s = format_notification(TerminalKind::Ghostty, "Crab", "Done");
        assert_eq!(s, "\x1b]777;notify;Crab;Done\x1b\\");
    }

    #[test]
    fn unknown_falls_back_to_bel() {
        let s = format_notification(TerminalKind::Unknown, "Crab", "Done");
        assert_eq!(s, "\x07");
    }

    #[test]
    fn vscode_uses_bel() {
        let s = format_notification(TerminalKind::VsCode, "Crab", "Done");
        assert_eq!(s, "\x07");
    }

    #[test]
    fn supports_notification_known_terminals() {
        assert!(TerminalKind::ITerm2.supports_notification());
        assert!(TerminalKind::Kitty.supports_notification());
        assert!(TerminalKind::WezTerm.supports_notification());
        assert!(TerminalKind::Ghostty.supports_notification());
        assert!(TerminalKind::VsCode.supports_notification());
        assert!(!TerminalKind::Unknown.supports_notification());
    }
}
