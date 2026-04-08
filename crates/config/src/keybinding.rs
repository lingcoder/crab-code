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
    /// The input string that failed to parse.
    pub input: String,
    /// Human-readable explanation.
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
///
/// The active context determines which bindings are checked when a key event
/// arrives. Contexts form a simple priority: the most specific context wins.
/// When no match is found in a specific context, `Global` is consulted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeybindingContext {
    /// Always-active bindings (e.g., quit, interrupt).
    Global,
    /// Main chat input area.
    Chat,
    /// Autocomplete popup is visible.
    Autocomplete,
    /// A confirmation dialog is focused.
    Confirmation,
    /// Help overlay is showing.
    Help,
    /// Scrollable transcript view.
    Transcript,
    /// Permission prompt is active.
    Permission,
    /// Search mode.
    Search,
    /// File picker / selector.
    FilePicker,
    /// Session sidebar.
    SessionList,
    /// Command palette.
    CommandPalette,
    /// Diff viewer.
    DiffView,
    /// Code block focus mode.
    CodeBlock,
    /// Settings editor.
    SettingsEditor,
    /// Model selector.
    ModelSelector,
    /// Compact mode.
    CompactView,
    /// Image preview.
    ImagePreview,
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

/// An action that can be triggered by a keybinding.
///
/// These are logical actions, decoupled from UI implementation. The TUI layer
/// maps each action to concrete behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeybindingAction {
    /// Send interrupt signal (like Ctrl+C).
    AppInterrupt,
    /// Submit the current chat input.
    ChatSubmit,
    /// Insert a newline in the chat input.
    ChatNewline,
    /// Scroll content up.
    ScrollUp,
    /// Scroll content down.
    ScrollDown,
    /// Scroll to the top.
    ScrollToTop,
    /// Scroll to the bottom.
    ScrollToBottom,
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
    /// Cancel the current operation.
    Cancel,
    /// Accept a permission request.
    PermissionAccept,
    /// Deny a permission request.
    PermissionDeny,
    /// Create a new session.
    NewSession,
    /// Switch to the next session.
    NextSession,
    /// Switch to the previous session.
    PrevSession,
    /// Toggle the session sidebar.
    ToggleSidebar,
    /// Toggle fold/unfold of selected tool output.
    ToggleFold,
    /// Copy focused code block to clipboard.
    CopyCodeBlock,
    /// Activate search mode.
    Search,
    /// Move to the next search match.
    SearchNext,
    /// Move to the previous search match.
    SearchPrev,
    /// Trigger tab completion.
    TabComplete,
    /// Next completion candidate.
    TabCompleteNext,
    /// Previous completion candidate.
    TabCompletePrev,
    /// Quit the application.
    Quit,
    /// Open command palette.
    CommandPalette,
    /// Toggle compact mode.
    ToggleCompact,
    /// Retry the last request.
    Retry,
    /// Open model selector.
    SelectModel,
    /// Toggle fast mode.
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
    /// No modifiers.
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
///
/// Most bindings are single-keystroke chords. Multi-keystroke chords require
/// the resolver to track partial matches.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Chord(pub Vec<Keystroke>);

impl Chord {
    /// Create a single-keystroke chord.
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
///
/// # Errors
///
/// Returns `ParseError` if the input is empty, contains unknown modifiers,
/// or has no recognizable key component.
pub fn parse_keystroke(_input: &str) -> Result<Keystroke, ParseError> {
    todo!()
}

/// Parse a chord string like "ctrl+k ctrl+c" (space-separated keystrokes).
///
/// # Errors
///
/// Returns `ParseError` if any keystroke in the sequence is invalid.
pub fn parse_chord(_input: &str) -> Result<Chord, ParseError> {
    todo!()
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
        todo!()
    }

    /// Load user-defined keybinding overrides from a JSON file.
    ///
    /// The file format is an array of objects:
    /// ```json
    /// [
    ///   {
    ///     "context": "global",
    ///     "chord": "ctrl+c",
    ///     "action": "app_interrupt"
    ///   }
    /// ]
    /// ```
    ///
    /// User bindings are merged on top of defaults — any matching
    /// `(context, chord)` pair replaces the default action.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or contains invalid JSON.
    pub fn load_user_bindings(_path: &Path) -> Result<Self, crab_common::Error> {
        todo!()
    }

    /// Resolve a single keystroke in the given context.
    ///
    /// Checks context-specific bindings first, then falls back to `Global`.
    /// Only matches single-keystroke chords; use `resolve_chord` for
    /// multi-keystroke sequences.
    pub fn resolve(&self, _ctx: KeybindingContext, _key: &Keystroke) -> Option<KeybindingAction> {
        todo!()
    }

    /// Resolve a full chord in the given context.
    pub fn resolve_chord(
        &self,
        _ctx: KeybindingContext,
        _chord: &Chord,
    ) -> Option<KeybindingAction> {
        todo!()
    }

    /// Return all bindings for a given context (including global fallbacks).
    pub fn bindings_for_context(&self, _ctx: KeybindingContext) -> Vec<&Binding> {
        todo!()
    }

    /// Register or override a binding.
    pub fn set_binding(
        &mut self,
        _ctx: KeybindingContext,
        _chord: Chord,
        _action: KeybindingAction,
    ) {
        todo!()
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
    /// Context scope (defaults to `Global` if omitted).
    #[serde(default = "default_context")]
    pub context: KeybindingContext,
    /// Chord string, e.g. "ctrl+c" or "ctrl+k ctrl+c".
    pub chord: String,
    /// The action to trigger.
    pub action: KeybindingAction,
}

fn default_context() -> KeybindingContext {
    KeybindingContext::Global
}

/// Validate a keybinding config entry.
///
/// Checks that the chord string is parseable and the action is known.
///
/// # Errors
///
/// Returns `ParseError` if the chord is invalid.
pub fn validate_config_entry(_entry: &KeybindingConfigEntry) -> Result<(), ParseError> {
    todo!()
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
}
