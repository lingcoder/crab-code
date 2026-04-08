//! Configurable keybinding system for the TUI.
//!
//! Default bindings can be overridden via `~/.crab/keybindings.json`.

use std::collections::HashMap;
use std::path::Path;

use crossterm::event::{KeyCode, KeyModifiers};
use serde::{Deserialize, Serialize};

/// An action that can be bound to a key combination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Quit the application.
    Quit,
    /// Submit the current input.
    Submit,
    /// Insert a newline in the input.
    NewLine,
    /// Create a new session.
    NewSession,
    /// Switch to the next session.
    NextSession,
    /// Switch to the previous session.
    PrevSession,
    /// Toggle the session sidebar visibility.
    ToggleSidebar,
    /// Cancel the current operation.
    Cancel,
    /// Scroll content up.
    ScrollUp,
    /// Scroll content down.
    ScrollDown,
    /// Accept a permission prompt.
    PermissionAllow,
    /// Deny a permission prompt.
    PermissionDeny,
    /// Toggle fold/unfold of selected tool output.
    ToggleFold,
    /// Copy the focused code block to clipboard.
    CopyCodeBlock,
    /// Activate search mode.
    Search,
    /// Move to next search match.
    SearchNext,
    /// Move to previous search match.
    SearchPrev,
    /// Trigger tab completion.
    TabComplete,
    /// Cycle to next completion candidate.
    TabCompleteNext,
    /// Cycle to previous completion candidate.
    TabCompletePrev,
    /// Search through input history.
    HistorySearch,
    /// Open an external editor for the current input.
    ExternalEditor,
    /// Stash the current input for later retrieval.
    Stash,
    /// Toggle the to-do list panel.
    ToggleTodos,
    /// Toggle the transcript panel.
    ToggleTranscript,
    /// Force a full terminal redraw.
    Redraw,
    /// Kill all running agents.
    KillAgents,
    /// Cycle between prompt input modes.
    CycleMode,
    /// Open the model picker.
    ModelPicker,
    /// Paste an image from the clipboard.
    ImagePaste,
    /// Undo the last edit in the input box.
    Undo,
}

/// A key combination (modifier + key code).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyCombo {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyCombo {
    pub const fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self { code, modifiers }
    }
}

/// Serializable representation of a key combo for JSON config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyComboConfig {
    pub key: String,
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub shift: bool,
}

impl KeyComboConfig {
    fn to_key_combo(&self) -> Option<KeyCombo> {
        let code = parse_key_code(&self.key)?;
        let mut modifiers = KeyModifiers::empty();
        if self.ctrl {
            modifiers |= KeyModifiers::CONTROL;
        }
        if self.alt {
            modifiers |= KeyModifiers::ALT;
        }
        if self.shift {
            modifiers |= KeyModifiers::SHIFT;
        }
        Some(KeyCombo::new(code, modifiers))
    }
}

/// Serializable keybinding override entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeybindingEntry {
    pub action: Action,
    pub key: KeyComboConfig,
}

/// The complete keybinding map.
pub struct Keybindings {
    map: HashMap<KeyCombo, Action>,
}

impl Keybindings {
    /// Create default keybindings.
    #[must_use]
    pub fn defaults() -> Self {
        let mut map = HashMap::new();

        // Quit
        map.insert(
            KeyCombo::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Action::Quit,
        );
        map.insert(
            KeyCombo::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
            Action::Quit,
        );

        // Session management
        map.insert(
            KeyCombo::new(KeyCode::Char('n'), KeyModifiers::CONTROL),
            Action::NewSession,
        );
        map.insert(
            KeyCombo::new(KeyCode::Tab, KeyModifiers::CONTROL),
            Action::NextSession,
        );
        map.insert(
            KeyCombo::new(KeyCode::BackTab, KeyModifiers::CONTROL),
            Action::PrevSession,
        );

        // Sidebar toggle
        map.insert(
            KeyCombo::new(KeyCode::Char('b'), KeyModifiers::CONTROL),
            Action::ToggleSidebar,
        );

        // Scroll
        map.insert(
            KeyCombo::new(KeyCode::PageUp, KeyModifiers::empty()),
            Action::ScrollUp,
        );
        map.insert(
            KeyCombo::new(KeyCode::PageDown, KeyModifiers::empty()),
            Action::ScrollDown,
        );

        // History search
        map.insert(
            KeyCombo::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
            Action::HistorySearch,
        );

        // External editor
        map.insert(
            KeyCombo::new(KeyCode::Char('g'), KeyModifiers::CONTROL),
            Action::ExternalEditor,
        );

        // Stash current input
        map.insert(
            KeyCombo::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
            Action::Stash,
        );

        // Toggle to-do list panel
        map.insert(
            KeyCombo::new(KeyCode::Char('t'), KeyModifiers::CONTROL),
            Action::ToggleTodos,
        );

        // Toggle transcript panel
        map.insert(
            KeyCombo::new(KeyCode::Char('o'), KeyModifiers::CONTROL),
            Action::ToggleTranscript,
        );

        // Force terminal redraw
        map.insert(
            KeyCombo::new(KeyCode::Char('l'), KeyModifiers::CONTROL),
            Action::Redraw,
        );

        // Kill all agents (Ctrl+K as simplified chord)
        map.insert(
            KeyCombo::new(KeyCode::Char('k'), KeyModifiers::CONTROL),
            Action::KillAgents,
        );

        // Cycle prompt input mode
        map.insert(
            KeyCombo::new(KeyCode::BackTab, KeyModifiers::SHIFT),
            Action::CycleMode,
        );

        // Model picker
        map.insert(
            KeyCombo::new(KeyCode::Char('p'), KeyModifiers::ALT),
            Action::ModelPicker,
        );

        // Image paste (platform-dependent)
        #[cfg(target_os = "windows")]
        map.insert(
            KeyCombo::new(KeyCode::Char('v'), KeyModifiers::ALT),
            Action::ImagePaste,
        );
        #[cfg(not(target_os = "windows"))]
        map.insert(
            KeyCombo::new(KeyCode::Char('v'), KeyModifiers::CONTROL),
            Action::ImagePaste,
        );

        // Undo (Ctrl+Z primary, Ctrl+_ alias)
        map.insert(
            KeyCombo::new(KeyCode::Char('z'), KeyModifiers::CONTROL),
            Action::Undo,
        );
        map.insert(
            KeyCombo::new(KeyCode::Char('_'), KeyModifiers::CONTROL),
            Action::Undo,
        );

        Self { map }
    }

    /// Load keybinding overrides from a JSON file and merge them into defaults.
    ///
    /// File format:
    /// ```json
    /// [
    ///   { "action": "new_session", "key": { "key": "t", "ctrl": true } },
    ///   { "action": "toggle_sidebar", "key": { "key": "e", "ctrl": true } }
    /// ]
    /// ```
    pub fn load_from_file(path: &Path) -> Self {
        let mut bindings = Self::defaults();

        let Ok(content) = std::fs::read_to_string(path) else {
            return bindings;
        };

        let Ok(entries) = serde_json::from_str::<Vec<KeybindingEntry>>(&content) else {
            return bindings;
        };

        for entry in entries {
            if let Some(combo) = entry.key.to_key_combo() {
                bindings.map.insert(combo, entry.action);
            }
        }

        bindings
    }

    /// Look up the action bound to a key event.
    #[must_use]
    pub fn resolve(&self, code: KeyCode, modifiers: KeyModifiers) -> Option<Action> {
        self.map.get(&KeyCombo::new(code, modifiers)).copied()
    }

    /// Number of active bindings.
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether there are no bindings.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

impl Default for Keybindings {
    fn default() -> Self {
        Self::defaults()
    }
}

/// Parse a key name string into a `KeyCode`.
fn parse_key_code(key: &str) -> Option<KeyCode> {
    match key.to_lowercase().as_str() {
        "tab" => Some(KeyCode::Tab),
        "backtab" => Some(KeyCode::BackTab),
        "enter" => Some(KeyCode::Enter),
        "esc" | "escape" => Some(KeyCode::Esc),
        "backspace" => Some(KeyCode::Backspace),
        "delete" | "del" => Some(KeyCode::Delete),
        "up" => Some(KeyCode::Up),
        "down" => Some(KeyCode::Down),
        "left" => Some(KeyCode::Left),
        "right" => Some(KeyCode::Right),
        "home" => Some(KeyCode::Home),
        "end" => Some(KeyCode::End),
        "pageup" => Some(KeyCode::PageUp),
        "pagedown" => Some(KeyCode::PageDown),
        "space" => Some(KeyCode::Char(' ')),
        s if s.len() == 1 => s.chars().next().map(KeyCode::Char),
        s if s.starts_with('f') => s[1..].parse::<u8>().ok().map(KeyCode::F),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_has_quit_binding() {
        let kb = Keybindings::defaults();
        assert_eq!(
            kb.resolve(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Some(Action::Quit)
        );
        assert_eq!(
            kb.resolve(KeyCode::Char('d'), KeyModifiers::CONTROL),
            Some(Action::Quit)
        );
    }

    #[test]
    fn defaults_has_session_bindings() {
        let kb = Keybindings::defaults();
        assert_eq!(
            kb.resolve(KeyCode::Char('n'), KeyModifiers::CONTROL),
            Some(Action::NewSession)
        );
        assert_eq!(
            kb.resolve(KeyCode::Tab, KeyModifiers::CONTROL),
            Some(Action::NextSession)
        );
    }

    #[test]
    fn defaults_has_sidebar_toggle() {
        let kb = Keybindings::defaults();
        assert_eq!(
            kb.resolve(KeyCode::Char('b'), KeyModifiers::CONTROL),
            Some(Action::ToggleSidebar)
        );
    }

    #[test]
    fn defaults_has_scroll() {
        let kb = Keybindings::defaults();
        assert_eq!(
            kb.resolve(KeyCode::PageUp, KeyModifiers::empty()),
            Some(Action::ScrollUp)
        );
        assert_eq!(
            kb.resolve(KeyCode::PageDown, KeyModifiers::empty()),
            Some(Action::ScrollDown)
        );
    }

    #[test]
    fn unknown_key_returns_none() {
        let kb = Keybindings::defaults();
        assert_eq!(kb.resolve(KeyCode::Char('x'), KeyModifiers::empty()), None);
    }

    #[test]
    fn parse_key_code_basic() {
        assert_eq!(parse_key_code("a"), Some(KeyCode::Char('a')));
        assert_eq!(parse_key_code("tab"), Some(KeyCode::Tab));
        assert_eq!(parse_key_code("Enter"), Some(KeyCode::Enter));
        assert_eq!(parse_key_code("ESC"), Some(KeyCode::Esc));
        assert_eq!(parse_key_code("space"), Some(KeyCode::Char(' ')));
        assert_eq!(parse_key_code("f1"), Some(KeyCode::F(1)));
        assert_eq!(parse_key_code("f12"), Some(KeyCode::F(12)));
        assert_eq!(parse_key_code("pageup"), Some(KeyCode::PageUp));
        assert_eq!(parse_key_code("delete"), Some(KeyCode::Delete));
    }

    #[test]
    fn parse_key_code_unknown() {
        assert_eq!(parse_key_code("foobar"), None);
        assert_eq!(parse_key_code(""), None);
    }

    #[test]
    fn key_combo_config_to_combo() {
        let config = KeyComboConfig {
            key: "n".into(),
            ctrl: true,
            alt: false,
            shift: false,
        };
        let combo = config.to_key_combo().unwrap();
        assert_eq!(combo.code, KeyCode::Char('n'));
        assert_eq!(combo.modifiers, KeyModifiers::CONTROL);
    }

    #[test]
    fn key_combo_config_with_multiple_modifiers() {
        let config = KeyComboConfig {
            key: "s".into(),
            ctrl: true,
            alt: false,
            shift: true,
        };
        let combo = config.to_key_combo().unwrap();
        assert_eq!(combo.modifiers, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
    }

    #[test]
    fn load_from_nonexistent_file_returns_defaults() {
        let kb = Keybindings::load_from_file(Path::new("/nonexistent/path.json"));
        assert_eq!(
            kb.resolve(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Some(Action::Quit)
        );
    }

    #[test]
    fn keybinding_entry_serde_roundtrip() {
        let entry = KeybindingEntry {
            action: Action::NewSession,
            key: KeyComboConfig {
                key: "t".into(),
                ctrl: true,
                alt: false,
                shift: false,
            },
        };

        let json = serde_json::to_string(&entry).unwrap();
        let parsed: KeybindingEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.action, Action::NewSession);
        assert_eq!(parsed.key.key, "t");
        assert!(parsed.key.ctrl);
    }

    #[test]
    fn action_serde_all_variants() {
        let actions = vec![
            Action::Quit,
            Action::Submit,
            Action::NewLine,
            Action::NewSession,
            Action::NextSession,
            Action::PrevSession,
            Action::ToggleSidebar,
            Action::Cancel,
            Action::ScrollUp,
            Action::ScrollDown,
            Action::PermissionAllow,
            Action::PermissionDeny,
            Action::ToggleFold,
            Action::CopyCodeBlock,
            Action::Search,
            Action::SearchNext,
            Action::SearchPrev,
            Action::TabComplete,
            Action::TabCompleteNext,
            Action::TabCompletePrev,
            Action::HistorySearch,
            Action::ExternalEditor,
            Action::Stash,
            Action::ToggleTodos,
            Action::ToggleTranscript,
            Action::Redraw,
            Action::KillAgents,
            Action::CycleMode,
            Action::ModelPicker,
            Action::ImagePaste,
            Action::Undo,
        ];
        for action in actions {
            let json = serde_json::to_string(&action).unwrap();
            let parsed: Action = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, action);
        }
    }

    #[test]
    fn defaults_not_empty() {
        let kb = Keybindings::defaults();
        assert!(!kb.is_empty());
        assert!(kb.len() > 5);
    }

    #[test]
    fn default_trait() {
        let kb = Keybindings::default();
        assert_eq!(
            kb.resolve(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Some(Action::Quit)
        );
    }

    #[test]
    fn defaults_has_new_bindings() {
        let kb = Keybindings::defaults();
        assert_eq!(
            kb.resolve(KeyCode::Char('r'), KeyModifiers::CONTROL),
            Some(Action::HistorySearch)
        );
        assert_eq!(
            kb.resolve(KeyCode::Char('g'), KeyModifiers::CONTROL),
            Some(Action::ExternalEditor)
        );
        assert_eq!(
            kb.resolve(KeyCode::Char('s'), KeyModifiers::CONTROL),
            Some(Action::Stash)
        );
        assert_eq!(
            kb.resolve(KeyCode::Char('t'), KeyModifiers::CONTROL),
            Some(Action::ToggleTodos)
        );
        assert_eq!(
            kb.resolve(KeyCode::Char('o'), KeyModifiers::CONTROL),
            Some(Action::ToggleTranscript)
        );
        assert_eq!(
            kb.resolve(KeyCode::Char('l'), KeyModifiers::CONTROL),
            Some(Action::Redraw)
        );
        assert_eq!(
            kb.resolve(KeyCode::Char('k'), KeyModifiers::CONTROL),
            Some(Action::KillAgents)
        );
        assert_eq!(
            kb.resolve(KeyCode::BackTab, KeyModifiers::SHIFT),
            Some(Action::CycleMode)
        );
        assert_eq!(
            kb.resolve(KeyCode::Char('p'), KeyModifiers::ALT),
            Some(Action::ModelPicker)
        );
        assert_eq!(
            kb.resolve(KeyCode::Char('z'), KeyModifiers::CONTROL),
            Some(Action::Undo)
        );
        assert_eq!(
            kb.resolve(KeyCode::Char('_'), KeyModifiers::CONTROL),
            Some(Action::Undo)
        );
    }

    #[test]
    fn defaults_has_image_paste() {
        let kb = Keybindings::defaults();
        // Platform-dependent: on Windows it's Alt+V, on others it's Ctrl+V
        #[cfg(target_os = "windows")]
        assert_eq!(
            kb.resolve(KeyCode::Char('v'), KeyModifiers::ALT),
            Some(Action::ImagePaste)
        );
        #[cfg(not(target_os = "windows"))]
        assert_eq!(
            kb.resolve(KeyCode::Char('v'), KeyModifiers::CONTROL),
            Some(Action::ImagePaste)
        );
    }
}
