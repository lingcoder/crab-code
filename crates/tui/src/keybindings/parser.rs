//! String → `KeyChord` / `Sequence` parser.
//!
//! Supported syntax:
//!
//! - Single key: `"a"`, `"tab"`, `"esc"`, `"f10"`, `"space"`
//! - Modifier combo: `"ctrl+k"`, `"ctrl+shift+p"`, `"alt+enter"`
//! - Chord sequence: `"ctrl+k ctrl+s"` (space-separated chords)

use crossterm::event::{KeyCode, KeyModifiers};

use super::types::{KeyChord, Sequence};

/// Parse a single key name into a `KeyCode`.
pub fn parse_key_code(key: &str) -> Option<KeyCode> {
    match key.to_lowercase().as_str() {
        "tab" => Some(KeyCode::Tab),
        "backtab" | "shift+tab" => Some(KeyCode::BackTab),
        "enter" | "return" => Some(KeyCode::Enter),
        "esc" | "escape" => Some(KeyCode::Esc),
        "backspace" | "bs" => Some(KeyCode::Backspace),
        "delete" | "del" => Some(KeyCode::Delete),
        "up" => Some(KeyCode::Up),
        "down" => Some(KeyCode::Down),
        "left" => Some(KeyCode::Left),
        "right" => Some(KeyCode::Right),
        "home" => Some(KeyCode::Home),
        "end" => Some(KeyCode::End),
        "pageup" | "pgup" => Some(KeyCode::PageUp),
        "pagedown" | "pgdown" | "pgdn" => Some(KeyCode::PageDown),
        "space" => Some(KeyCode::Char(' ')),
        s if s.len() == 1 => s.chars().next().map(KeyCode::Char),
        s if s.starts_with('f') && s.len() >= 2 => s[1..].parse::<u8>().ok().map(KeyCode::F),
        _ => None,
    }
}

/// Parse a chord like `"ctrl+shift+k"` into a `KeyChord`.
pub fn parse_chord(input: &str) -> Option<KeyChord> {
    let mut modifiers = KeyModifiers::empty();
    let parts: Vec<&str> = input.split('+').map(str::trim).collect();
    if parts.is_empty() {
        return None;
    }

    let (&key_part, mod_parts) = parts.split_last()?;
    for m in mod_parts {
        match m.to_lowercase().as_str() {
            "ctrl" | "control" | "c" => modifiers |= KeyModifiers::CONTROL,
            "alt" | "option" | "meta" | "a" => modifiers |= KeyModifiers::ALT,
            "shift" | "s" => modifiers |= KeyModifiers::SHIFT,
            "super" | "cmd" | "win" => modifiers |= KeyModifiers::SUPER,
            _ => return None,
        }
    }

    let code = parse_key_code(key_part)?;
    Some(KeyChord::new(code, modifiers))
}

/// Parse a full sequence like `"ctrl+k ctrl+s"` into a `Sequence`.
pub fn parse_sequence(input: &str) -> Option<Sequence> {
    let chords: Option<Vec<KeyChord>> = input.split_whitespace().map(parse_chord).collect();
    let chords = chords?;
    if chords.is_empty() {
        None
    } else {
        Some(Sequence(chords))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_keys() {
        assert_eq!(parse_key_code("a"), Some(KeyCode::Char('a')));
        assert_eq!(parse_key_code("Enter"), Some(KeyCode::Enter));
        assert_eq!(parse_key_code("ESC"), Some(KeyCode::Esc));
        assert_eq!(parse_key_code("space"), Some(KeyCode::Char(' ')));
        assert_eq!(parse_key_code("f1"), Some(KeyCode::F(1)));
        assert_eq!(parse_key_code("f12"), Some(KeyCode::F(12)));
        assert_eq!(parse_key_code("pageup"), Some(KeyCode::PageUp));
        assert_eq!(parse_key_code("pgdn"), Some(KeyCode::PageDown));
    }

    #[test]
    fn parses_ctrl_chord() {
        let chord = parse_chord("ctrl+k").unwrap();
        assert_eq!(chord.code, KeyCode::Char('k'));
        assert_eq!(chord.modifiers, KeyModifiers::CONTROL);
    }

    #[test]
    fn parses_multi_modifier_chord() {
        let chord = parse_chord("ctrl+shift+p").unwrap();
        assert_eq!(chord.code, KeyCode::Char('p'));
        assert_eq!(chord.modifiers, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
    }

    #[test]
    fn parses_alt_enter() {
        let chord = parse_chord("alt+enter").unwrap();
        assert_eq!(chord.code, KeyCode::Enter);
        assert_eq!(chord.modifiers, KeyModifiers::ALT);
    }

    #[test]
    fn rejects_unknown_modifier() {
        assert!(parse_chord("hyper+k").is_none());
    }

    #[test]
    fn parses_two_chord_sequence() {
        let seq = parse_sequence("ctrl+k ctrl+s").unwrap();
        assert_eq!(seq.len(), 2);
        assert_eq!(seq.0[0], KeyChord::ctrl(KeyCode::Char('k')));
        assert_eq!(seq.0[1], KeyChord::ctrl(KeyCode::Char('s')));
    }

    #[test]
    fn parses_single_chord_sequence() {
        let seq = parse_sequence("ctrl+c").unwrap();
        assert_eq!(seq.len(), 1);
    }

    #[test]
    fn rejects_empty_sequence() {
        assert!(parse_sequence("").is_none());
        assert!(parse_sequence("   ").is_none());
    }

    #[test]
    fn rejects_invalid_key() {
        assert!(parse_chord("ctrl+").is_none());
        assert!(parse_chord("ctrl+foobar").is_none());
    }
}
