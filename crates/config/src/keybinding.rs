//! Keybinding configuration system.
//!
//! Provides a schema for defining, parsing, and resolving keyboard shortcuts
//! across different application contexts. Supports modifier keys, chords
//! (multi-keystroke sequences), and user-overridable bindings loaded from JSON.
//!
//! Maps to CCB keybindings/ (13 files, 2617 LOC).

use std::collections::HashMap;
use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Error returned when parsing a keystroke string fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub input: String,
    pub reason: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid keystroke '{}': {}", self.input, self.reason)
    }
}

impl std::error::Error for ParseError {}

// ---------------------------------------------------------------------------
// Contexts
// ---------------------------------------------------------------------------

/// Contexts where keybindings apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeybindingContext {
    Global,
    Chat,
    Autocomplete,
    Confirmation,
    Help,
    Transcript,
    Permission,
    Search,
    FilePicker,
    SessionList,
    CommandPalette,
    DiffView,
    CodeBlock,
    SettingsEditor,
    ModelSelector,
    CompactView,
    ImagePreview,
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

/// An action that can be triggered by a keybinding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeybindingAction {
    AppInterrupt,
    ChatSubmit,
    ChatNewline,
    ScrollUp,
    ScrollDown,
    ScrollToTop,
    ScrollToBottom,
    PageUp,
    PageDown,
    Cancel,
    PermissionAccept,
    PermissionDeny,
    NewSession,
    NextSession,
    PrevSession,
    ToggleSidebar,
    ToggleFold,
    CopyCodeBlock,
    Search,
    SearchNext,
    SearchPrev,
    TabComplete,
    TabCompleteNext,
    TabCompletePrev,
    Quit,
    CommandPalette,
    ToggleCompact,
    Retry,
    SelectModel,
    ToggleFastMode,
}

// ---------------------------------------------------------------------------
// Key primitives
// ---------------------------------------------------------------------------

/// Modifier key flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

impl Modifiers {
    pub const NONE: Self = Self {
        ctrl: false,
        alt: false,
        shift: false,
        meta: false,
    };
}

/// A named key (non-character keys, plus a `Char` variant for printable keys).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Key {
    Char(char),
    Enter,
    Tab,
    Backspace,
    Delete,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Space,
    F(u8),
}

/// A single parsed keystroke, e.g. "ctrl+c" or "shift+enter".
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Keystroke {
    pub modifiers: Modifiers,
    pub key: Key,
}

/// A chord is a sequence of keystrokes, e.g. "ctrl+k ctrl+c".
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Chord(pub Vec<Keystroke>);

impl Chord {
    pub fn single(keystroke: Keystroke) -> Self {
        Self(vec![keystroke])
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Parse a keystroke string like "ctrl+shift+a" or "enter".
///
/// Format: `[modifier+]*key` where modifiers are `ctrl`, `alt`, `shift`, `meta`
/// and key is a named key or a single character.
pub fn parse_keystroke(input: &str) -> Result<Keystroke, ParseError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(ParseError {
            input: input.to_string(),
            reason: "empty keystroke".to_string(),
        });
    }

    let parts: Vec<&str> = input.split('+').collect();
    let mut modifiers = Modifiers::NONE;

    // All parts except the last are modifiers; the last is the key
    for &part in &parts[..parts.len() - 1] {
        match part.to_lowercase().as_str() {
            "ctrl" | "control" => modifiers.ctrl = true,
            "alt" | "opt" | "option" => modifiers.alt = true,
            "shift" => modifiers.shift = true,
            "meta" | "cmd" | "command" | "super" | "win" => modifiers.meta = true,
            other => {
                return Err(ParseError {
                    input: input.to_string(),
                    reason: format!("unknown modifier '{other}'"),
                });
            }
        }
    }

    let key_str = parts.last().unwrap().to_lowercase();
    let key = parse_key_name(&key_str).ok_or_else(|| ParseError {
        input: input.to_string(),
        reason: format!("unknown key '{key_str}'"),
    })?;

    Ok(Keystroke { modifiers, key })
}

/// Parse a key name string to a Key enum value.
fn parse_key_name(name: &str) -> Option<Key> {
    match name {
        "enter" | "return" => Some(Key::Enter),
        "tab" => Some(Key::Tab),
        "backspace" | "bs" => Some(Key::Backspace),
        "delete" | "del" => Some(Key::Delete),
        "escape" | "esc" => Some(Key::Escape),
        "up" | "↑" => Some(Key::Up),
        "down" | "↓" => Some(Key::Down),
        "left" | "←" => Some(Key::Left),
        "right" | "→" => Some(Key::Right),
        "home" => Some(Key::Home),
        "end" => Some(Key::End),
        "pageup" | "pgup" => Some(Key::PageUp),
        "pagedown" | "pgdn" => Some(Key::PageDown),
        "space" => Some(Key::Space),
        s if s.len() == 1 => Some(Key::Char(s.chars().next().unwrap())),
        s if s.starts_with('f') => {
            let num: u8 = s[1..].parse().ok()?;
            if (1..=24).contains(&num) {
                Some(Key::F(num))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Parse a chord string like "ctrl+k ctrl+c" (space-separated keystrokes).
pub fn parse_chord(input: &str) -> Result<Chord, ParseError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(ParseError {
            input: input.to_string(),
            reason: "empty chord".to_string(),
        });
    }

    let keystrokes: Result<Vec<Keystroke>, ParseError> =
        input.split_whitespace().map(parse_keystroke).collect();

    Ok(Chord(keystrokes?))
}

// ---------------------------------------------------------------------------
// Binding + Resolver
// ---------------------------------------------------------------------------

/// A single keybinding entry: chord → action, scoped to a context.
#[derive(Debug, Clone)]
pub struct Binding {
    pub chord: Chord,
    pub action: KeybindingAction,
}

/// Keybinding resolver: given a context and keystroke, find the matching action.
///
/// Maintains a map of `(context, chord) → action`. On lookup, checks the
/// specific context first, then falls back to `KeybindingContext::Global`.
pub struct KeybindingResolver {
    bindings: HashMap<KeybindingContext, Vec<Binding>>,
}

impl KeybindingResolver {
    /// Create a resolver loaded with sensible defaults for all contexts.
    pub fn with_defaults() -> Self {
        let mut resolver = Self {
            bindings: HashMap::new(),
        };

        // Global defaults
        resolver.set_binding(
            KeybindingContext::Global,
            Chord::single(Keystroke {
                modifiers: Modifiers {
                    ctrl: true,
                    ..Modifiers::NONE
                },
                key: Key::Char('c'),
            }),
            KeybindingAction::AppInterrupt,
        );
        resolver.set_binding(
            KeybindingContext::Global,
            Chord::single(Keystroke {
                modifiers: Modifiers::NONE,
                key: Key::Escape,
            }),
            KeybindingAction::Cancel,
        );

        // Chat defaults
        resolver.set_binding(
            KeybindingContext::Chat,
            Chord::single(Keystroke {
                modifiers: Modifiers::NONE,
                key: Key::Enter,
            }),
            KeybindingAction::ChatSubmit,
        );
        resolver.set_binding(
            KeybindingContext::Chat,
            Chord::single(Keystroke {
                modifiers: Modifiers {
                    shift: true,
                    ..Modifiers::NONE
                },
                key: Key::Enter,
            }),
            KeybindingAction::ChatNewline,
        );
        resolver.set_binding(
            KeybindingContext::Chat,
            Chord::single(Keystroke {
                modifiers: Modifiers::NONE,
                key: Key::Tab,
            }),
            KeybindingAction::TabComplete,
        );

        // Permission defaults
        resolver.set_binding(
            KeybindingContext::Permission,
            Chord::single(Keystroke {
                modifiers: Modifiers::NONE,
                key: Key::Char('y'),
            }),
            KeybindingAction::PermissionAccept,
        );
        resolver.set_binding(
            KeybindingContext::Permission,
            Chord::single(Keystroke {
                modifiers: Modifiers::NONE,
                key: Key::Char('n'),
            }),
            KeybindingAction::PermissionDeny,
        );

        // Transcript scrolling
        resolver.set_binding(
            KeybindingContext::Transcript,
            Chord::single(Keystroke {
                modifiers: Modifiers::NONE,
                key: Key::Up,
            }),
            KeybindingAction::ScrollUp,
        );
        resolver.set_binding(
            KeybindingContext::Transcript,
            Chord::single(Keystroke {
                modifiers: Modifiers::NONE,
                key: Key::Down,
            }),
            KeybindingAction::ScrollDown,
        );

        resolver
    }

    /// Load user-defined keybinding overrides from a JSON file.
    ///
    /// User bindings are merged on top of defaults — any matching
    /// `(context, chord)` pair replaces the default action.
    pub fn load_user_bindings(path: &Path) -> Result<Self, crab_core::Error> {
        let mut resolver = Self::with_defaults();

        let content = std::fs::read_to_string(path)?;
        let entries: Vec<KeybindingConfigEntry> = serde_json::from_str(&content)
            .map_err(|e| crab_core::Error::Config(format!("invalid keybindings JSON: {e}")))?;

        for entry in entries {
            if let Ok(chord) = parse_chord(&entry.chord) {
                resolver.set_binding(entry.context, chord, entry.action);
            }
        }

        Ok(resolver)
    }

    /// Resolve a single keystroke in the given context.
    ///
    /// Checks context-specific bindings first, then falls back to `Global`.
    /// Only matches single-keystroke chords.
    pub fn resolve(&self, ctx: KeybindingContext, key: &Keystroke) -> Option<KeybindingAction> {
        let single = Chord::single(key.clone());

        // Check context-specific bindings first (last-wins)
        if let Some(bindings) = self.bindings.get(&ctx)
            && let Some(binding) = bindings.iter().rev().find(|b| b.chord == single)
        {
            return Some(binding.action);
        }

        // Fall back to Global
        if ctx != KeybindingContext::Global
            && let Some(bindings) = self.bindings.get(&KeybindingContext::Global)
            && let Some(binding) = bindings.iter().rev().find(|b| b.chord == single)
        {
            return Some(binding.action);
        }

        None
    }

    /// Resolve a full chord in the given context.
    pub fn resolve_chord(&self, ctx: KeybindingContext, chord: &Chord) -> Option<KeybindingAction> {
        // Check context-specific (last-wins)
        if let Some(bindings) = self.bindings.get(&ctx)
            && let Some(binding) = bindings.iter().rev().find(|b| &b.chord == chord)
        {
            return Some(binding.action);
        }

        // Fall back to Global
        if ctx != KeybindingContext::Global
            && let Some(bindings) = self.bindings.get(&KeybindingContext::Global)
            && let Some(binding) = bindings.iter().rev().find(|b| &b.chord == chord)
        {
            return Some(binding.action);
        }

        None
    }

    /// Return all bindings for a given context (including global fallbacks).
    pub fn bindings_for_context(&self, ctx: KeybindingContext) -> Vec<&Binding> {
        let mut result: Vec<&Binding> = Vec::new();

        // Add global bindings first
        if let Some(global) = self.bindings.get(&KeybindingContext::Global) {
            result.extend(global.iter());
        }

        // Add context-specific (may override global via last-wins)
        if ctx != KeybindingContext::Global
            && let Some(specific) = self.bindings.get(&ctx)
        {
            result.extend(specific.iter());
        }

        result
    }

    /// Register or override a binding.
    pub fn set_binding(&mut self, ctx: KeybindingContext, chord: Chord, action: KeybindingAction) {
        self.bindings
            .entry(ctx)
            .or_default()
            .push(Binding { chord, action });
    }
}

impl Default for KeybindingResolver {
    fn default() -> Self {
        Self::with_defaults()
    }
}

impl fmt::Debug for KeybindingResolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeybindingResolver")
            .field("context_count", &self.bindings.len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Serializable config types (for JSON I/O)
// ---------------------------------------------------------------------------

/// JSON-serializable keybinding entry used in config files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeybindingConfigEntry {
    #[serde(default = "default_context")]
    pub context: KeybindingContext,
    pub chord: String,
    pub action: KeybindingAction,
}

fn default_context() -> KeybindingContext {
    KeybindingContext::Global
}

/// Validate a keybinding config entry.
pub fn validate_config_entry(entry: &KeybindingConfigEntry) -> Result<(), ParseError> {
    parse_chord(&entry.chord)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_error_display() {
        let err = ParseError {
            input: "ctrl+???".into(),
            reason: "unknown key".into(),
        };
        assert!(err.to_string().contains("ctrl+???"));
    }

    #[test]
    fn chord_single() {
        let ks = Keystroke {
            modifiers: Modifiers::NONE,
            key: Key::Enter,
        };
        let chord = Chord::single(ks.clone());
        assert_eq!(chord.0.len(), 1);
        assert_eq!(chord.0[0], ks);
    }

    #[test]
    fn modifiers_none() {
        let m = Modifiers::NONE;
        assert!(!m.ctrl);
        assert!(!m.alt);
        assert!(!m.shift);
        assert!(!m.meta);
    }

    #[test]
    fn context_serde_roundtrip() {
        let ctx = KeybindingContext::Autocomplete;
        let json = serde_json::to_string(&ctx).unwrap();
        let parsed: KeybindingContext = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ctx);
    }

    #[test]
    fn action_serde_roundtrip() {
        let action = KeybindingAction::ChatSubmit;
        let json = serde_json::to_string(&action).unwrap();
        let parsed: KeybindingAction = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, action);
    }

    #[test]
    fn config_entry_serde() {
        let json = r#"{"chord":"ctrl+c","action":"app_interrupt"}"#;
        let entry: KeybindingConfigEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.context, KeybindingContext::Global);
        assert_eq!(entry.chord, "ctrl+c");
        assert_eq!(entry.action, KeybindingAction::AppInterrupt);
    }

    // ── Keystroke parsing ─────────────────────────────────────────────

    #[test]
    fn parse_single_char() {
        let ks = parse_keystroke("a").unwrap();
        assert_eq!(ks.key, Key::Char('a'));
        assert_eq!(ks.modifiers, Modifiers::NONE);
    }

    #[test]
    fn parse_ctrl_c() {
        let ks = parse_keystroke("ctrl+c").unwrap();
        assert!(ks.modifiers.ctrl);
        assert_eq!(ks.key, Key::Char('c'));
    }

    #[test]
    fn parse_ctrl_shift_a() {
        let ks = parse_keystroke("ctrl+shift+a").unwrap();
        assert!(ks.modifiers.ctrl);
        assert!(ks.modifiers.shift);
        assert_eq!(ks.key, Key::Char('a'));
    }

    #[test]
    fn parse_enter() {
        let ks = parse_keystroke("enter").unwrap();
        assert_eq!(ks.key, Key::Enter);
    }

    #[test]
    fn parse_escape() {
        let ks = parse_keystroke("esc").unwrap();
        assert_eq!(ks.key, Key::Escape);
    }

    #[test]
    fn parse_f_key() {
        let ks = parse_keystroke("f12").unwrap();
        assert_eq!(ks.key, Key::F(12));
    }

    #[test]
    fn parse_alt_tab() {
        let ks = parse_keystroke("alt+tab").unwrap();
        assert!(ks.modifiers.alt);
        assert_eq!(ks.key, Key::Tab);
    }

    #[test]
    fn parse_meta_modifier() {
        let ks = parse_keystroke("cmd+k").unwrap();
        assert!(ks.modifiers.meta);
        assert_eq!(ks.key, Key::Char('k'));
    }

    #[test]
    fn parse_empty_fails() {
        assert!(parse_keystroke("").is_err());
    }

    #[test]
    fn parse_unknown_modifier_fails() {
        assert!(parse_keystroke("hyper+a").is_err());
    }

    #[test]
    fn parse_unknown_key_fails() {
        assert!(parse_keystroke("ctrl+???").is_err());
    }

    // ── Chord parsing ─────────────────────────────────────────────────

    #[test]
    fn parse_chord_single() {
        let chord = parse_chord("ctrl+c").unwrap();
        assert_eq!(chord.0.len(), 1);
    }

    #[test]
    fn parse_chord_multi() {
        let chord = parse_chord("ctrl+k ctrl+c").unwrap();
        assert_eq!(chord.0.len(), 2);
        assert!(chord.0[0].modifiers.ctrl);
        assert_eq!(chord.0[0].key, Key::Char('k'));
        assert!(chord.0[1].modifiers.ctrl);
        assert_eq!(chord.0[1].key, Key::Char('c'));
    }

    #[test]
    fn parse_chord_empty_fails() {
        assert!(parse_chord("").is_err());
    }

    // ── Resolver ──────────────────────────────────────────────────────

    #[test]
    fn resolver_defaults_have_ctrl_c() {
        let resolver = KeybindingResolver::with_defaults();
        let ks = Keystroke {
            modifiers: Modifiers {
                ctrl: true,
                ..Modifiers::NONE
            },
            key: Key::Char('c'),
        };
        let action = resolver.resolve(KeybindingContext::Chat, &ks);
        assert_eq!(action, Some(KeybindingAction::AppInterrupt));
    }

    #[test]
    fn resolver_context_specific() {
        let resolver = KeybindingResolver::with_defaults();
        let enter = Keystroke {
            modifiers: Modifiers::NONE,
            key: Key::Enter,
        };
        // Enter in Chat → ChatSubmit
        assert_eq!(
            resolver.resolve(KeybindingContext::Chat, &enter),
            Some(KeybindingAction::ChatSubmit)
        );
        // Enter in Help → None (no default)
        assert_eq!(resolver.resolve(KeybindingContext::Help, &enter), None);
    }

    #[test]
    fn resolver_set_override() {
        let mut resolver = KeybindingResolver::with_defaults();
        let enter = Keystroke {
            modifiers: Modifiers::NONE,
            key: Key::Enter,
        };
        // Override Enter in Chat to be Cancel
        resolver.set_binding(
            KeybindingContext::Chat,
            Chord::single(enter.clone()),
            KeybindingAction::Cancel,
        );
        // Last-wins: should now be Cancel
        assert_eq!(
            resolver.resolve(KeybindingContext::Chat, &enter),
            Some(KeybindingAction::Cancel)
        );
    }

    #[test]
    fn resolver_bindings_for_context() {
        let resolver = KeybindingResolver::with_defaults();
        let bindings = resolver.bindings_for_context(KeybindingContext::Chat);
        assert!(!bindings.is_empty());
    }

    #[test]
    fn validate_config_entry_valid() {
        let entry = KeybindingConfigEntry {
            context: KeybindingContext::Global,
            chord: "ctrl+c".into(),
            action: KeybindingAction::AppInterrupt,
        };
        assert!(validate_config_entry(&entry).is_ok());
    }

    #[test]
    fn validate_config_entry_invalid_chord() {
        let entry = KeybindingConfigEntry {
            context: KeybindingContext::Global,
            chord: "hyper+???".into(),
            action: KeybindingAction::Quit,
        };
        assert!(validate_config_entry(&entry).is_err());
    }
}
