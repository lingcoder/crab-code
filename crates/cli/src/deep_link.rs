//! Deep-link URL parsing for `crab-cli://` URLs.
//!
//! Allows external applications and web pages to launch `crab` with a
//! pre-configured action by opening a URL like:
//!
//! ```text
//! crab-cli://open-session/abc-123
//! crab-cli://run/ls -la
//! crab-cli://install-plugin/my-plugin
//! ```

/// Actions that can be triggered via a deep link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeepLinkAction {
    /// Resume or open an existing session by identifier.
    OpenSession {
        /// Session identifier.
        session_id: String,
    },
    /// Run a CLI command string.
    RunCommand {
        /// The command text to execute.
        command: String,
    },
    /// Install a plugin by name.
    InstallPlugin {
        /// Plugin identifier.
        plugin_name: String,
    },
}

/// The URL scheme used for deep links.
const SCHEME: &str = "crab-cli://";

/// Parse a `crab-cli://` URL into a [`DeepLinkAction`].
///
/// Returns `None` if the URL does not start with the expected scheme or
/// contains an unrecognised action.
///
/// # Examples
///
/// ```rust,no_run
/// use crab_cli::deep_link::parse_deep_link;
///
/// let action = parse_deep_link("crab-cli://open-session/sess-42");
/// ```
pub fn parse_deep_link(url: &str) -> Option<DeepLinkAction> {
    let body = url.strip_prefix(SCHEME)?;
    if body.is_empty() {
        return None;
    }

    // Split into action and argument at the first `/`
    let (action, argument) = match body.find('/') {
        Some(idx) => (&body[..idx], &body[idx + 1..]),
        None => (body, ""),
    };

    match action {
        "open-session" => {
            if argument.is_empty() {
                None
            } else {
                Some(DeepLinkAction::OpenSession {
                    session_id: percent_decode(argument),
                })
            }
        }
        "run" => {
            if argument.is_empty() {
                None
            } else {
                Some(DeepLinkAction::RunCommand {
                    command: percent_decode(argument),
                })
            }
        }
        "install-plugin" => {
            if argument.is_empty() {
                None
            } else {
                Some(DeepLinkAction::InstallPlugin {
                    plugin_name: percent_decode(argument),
                })
            }
        }
        _ => None,
    }
}

/// Minimal percent-decoding for URL arguments.
///
/// Decodes `%XX` sequences into the corresponding byte. Does not handle
/// full URI spec (no `+` as space, no charset awareness), but sufficient
/// for simple identifiers and commands.
fn percent_decode(input: &str) -> String {
    let mut bytes: Vec<u8> = Vec::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        if ch == '%' {
            let hi = chars.next();
            let lo = chars.next();
            if let (Some(h), Some(l)) = (hi, lo) {
                let hex = format!("{h}{l}");
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    bytes.push(byte);
                    continue;
                }
                // Malformed — just pass through
                bytes.push(b'%');
                let mut buf = [0u8; 4];
                bytes.extend_from_slice(h.encode_utf8(&mut buf).as_bytes());
                bytes.extend_from_slice(l.encode_utf8(&mut buf).as_bytes());
            } else {
                bytes.push(b'%');
            }
        } else {
            let mut buf = [0u8; 4];
            bytes.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
        }
    }
    // Decoded `%XX` bytes plus literal chars together form UTF-8.
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Return platform-specific instructions for registering the `crab-cli://`
/// URL scheme with the operating system.
///
/// This does not perform the registration itself — it returns a
/// human-readable message describing the steps.
pub fn register_url_scheme() -> String {
    if cfg!(target_os = "windows") {
        "To register the crab-cli:// URL scheme on Windows:\n\
         1. Open Registry Editor (regedit)\n\
         2. Create key HKEY_CLASSES_ROOT\\crab-cli\n\
         3. Set (Default) to \"URL:Crab Code Protocol\"\n\
         4. Add string value \"URL Protocol\" = \"\"\n\
         5. Create sub-key shell\\open\\command\n\
         6. Set (Default) to \"\\\"<path-to-crab.exe>\\\" \\\"%1\\\"\""
            .into()
    } else if cfg!(target_os = "macos") {
        "To register the crab-cli:// URL scheme on macOS:\n\
         1. Add a CFBundleURLTypes entry to your app's Info.plist\n\
         2. Set CFBundleURLSchemes to [\"crab-cli\"]\n\
         3. Rebuild or re-register with Launch Services"
            .into()
    } else {
        "To register the crab-cli:// URL scheme on Linux:\n\
         1. Create a .desktop file in ~/.local/share/applications/\n\
         2. Set MimeType=x-scheme-handler/crab-cli\n\
         3. Run: xdg-mime default crab-code.desktop x-scheme-handler/crab-cli"
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_open_session() {
        let action = parse_deep_link("crab-cli://open-session/my-sess-42");
        assert_eq!(
            action,
            Some(DeepLinkAction::OpenSession {
                session_id: "my-sess-42".into(),
            })
        );
    }

    #[test]
    fn parse_run_command() {
        let action = parse_deep_link("crab-cli://run/cargo%20test");
        assert_eq!(
            action,
            Some(DeepLinkAction::RunCommand {
                command: "cargo test".into(),
            })
        );
    }

    #[test]
    fn percent_decode_multibyte_utf8() {
        // %E4%B8%AD is the UTF-8 encoding of 中; bytes must be reassembled
        // into one char, not pushed as three Latin-1 code points.
        assert_eq!(percent_decode("%E4%B8%AD"), "中");
        assert_eq!(percent_decode("a%20b%E4%B8%ADc"), "a b中c");
    }

    #[test]
    fn parse_install_plugin() {
        let action = parse_deep_link("crab-cli://install-plugin/my-plugin");
        assert_eq!(
            action,
            Some(DeepLinkAction::InstallPlugin {
                plugin_name: "my-plugin".into(),
            })
        );
    }

    #[test]
    fn parse_missing_argument_returns_none() {
        assert!(parse_deep_link("crab-cli://open-session/").is_none());
        assert!(parse_deep_link("crab-cli://run/").is_none());
        assert!(parse_deep_link("crab-cli://install-plugin/").is_none());
    }

    #[test]
    fn parse_unknown_action_returns_none() {
        assert!(parse_deep_link("crab-cli://delete-everything/please").is_none());
    }

    #[test]
    fn parse_wrong_scheme_returns_none() {
        assert!(parse_deep_link("https://example.com/open-session/x").is_none());
        assert!(parse_deep_link("").is_none());
    }

    #[test]
    fn parse_empty_body_returns_none() {
        assert!(parse_deep_link("crab-cli://").is_none());
    }

    #[test]
    fn percent_decode_roundtrip() {
        assert_eq!(percent_decode("hello%20world"), "hello world");
        assert_eq!(percent_decode("no%2Fslash"), "no/slash");
        assert_eq!(percent_decode("plain"), "plain");
    }

    #[test]
    fn register_url_scheme_returns_non_empty() {
        let info = register_url_scheme();
        assert!(!info.is_empty());
        assert!(info.contains("crab-cli"));
    }
}
