//! Core keybinding types: `Action`, `KeyContext`, `KeyChord`.

use crossterm::event::{KeyCode, KeyModifiers};
use serde::{Deserialize, Serialize};

/// A discrete UI action that can be bound to a key or chord.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Quit,
    Submit,
    NewLine,
    NewSession,
    NextSession,
    PrevSession,
    ToggleSidebar,
    Cancel,
    ScrollUp,
    ScrollDown,
    ScrollHome,
    ScrollEnd,
    PermissionAllow,
    PermissionDeny,
    ToggleFold,
    FoldOutput,
    UnfoldOutput,
    CopyCodeBlock,
    Search,
    SearchNext,
    SearchPrev,
    TabComplete,
    TabCompleteNext,
    TabCompletePrev,
    HistorySearch,
    ExternalEditor,
    Stash,
    ToggleTodos,
    ToggleTranscript,
    Redraw,
    KillAgents,
    CycleMode,
    ModelPicker,
    ImagePaste,
    Undo,
    Redo,
    OpenTaskList,
    OpenGlobalSearch,
    OpenCommandPalette,
    OpenHelp,
    EnterSelectionMode,
    ExitSelectionMode,
    SelectionCopy,
    OpenAgentDetail,
    ClearScreen,
}

/// The focus / overlay context a binding applies to.
///
/// Contexts are checked from innermost-focused to outermost; the first
/// matching binding wins. `Global` is always the outermost fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyContext {
    Global,
    Chat,
    Input,
    Search,
    HistorySearch,
    GlobalSearch,
    Permission,
    CommandPalette,
    SelectionMode,
    FileDialog,
    TaskList,
    Transcript,
    ScrollBox,
    Help,
    DragDrop,
    ModelPicker,
    OutputFold,
    Diff,
    AgentDetail,
    Sidebar,
    Autocomplete,
    VimNormal,
    VimVisual,
}

/// A single key event (modifier set + key code).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyChord {
    pub const fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self { code, modifiers }
    }

    pub const fn ctrl(code: KeyCode) -> Self {
        Self::new(code, KeyModifiers::CONTROL)
    }

    pub const fn alt(code: KeyCode) -> Self {
        Self::new(code, KeyModifiers::ALT)
    }

    pub const fn plain(code: KeyCode) -> Self {
        Self::new(code, KeyModifiers::NONE)
    }
}

/// A chord sequence (one or more `KeyChord` pressed in order).
///
/// Single-key bindings use a `Sequence` of length 1. Multi-key bindings
/// like `Ctrl+K Ctrl+S` use length >= 2.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Sequence(pub Vec<KeyChord>);

impl Sequence {
    pub fn single(chord: KeyChord) -> Self {
        Self(vec![chord])
    }

    pub fn of(chords: Vec<KeyChord>) -> Self {
        Self(chords)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn starts_with(&self, prefix: &[KeyChord]) -> bool {
        self.0.len() >= prefix.len() && self.0[..prefix.len()] == *prefix
    }
}

/// Legacy single-key combo alias, retained for the simple resolver API.
pub type KeyCombo = KeyChord;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequence_starts_with_handles_prefix() {
        let seq = Sequence(vec![
            KeyChord::ctrl(KeyCode::Char('k')),
            KeyChord::ctrl(KeyCode::Char('s')),
        ]);
        assert!(seq.starts_with(&[KeyChord::ctrl(KeyCode::Char('k'))]));
        assert!(!seq.starts_with(&[KeyChord::ctrl(KeyCode::Char('x'))]));
        assert!(seq.starts_with(&[]));
    }

    #[test]
    fn chord_constructors() {
        assert_eq!(
            KeyChord::ctrl(KeyCode::Char('c')),
            KeyChord::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
        );
        assert_eq!(
            KeyChord::alt(KeyCode::Char('p')),
            KeyChord::new(KeyCode::Char('p'), KeyModifiers::ALT)
        );
        assert_eq!(
            KeyChord::plain(KeyCode::Tab),
            KeyChord::new(KeyCode::Tab, KeyModifiers::NONE)
        );
    }

    #[test]
    fn action_roundtrip_json() {
        for action in [
            Action::Quit,
            Action::OpenGlobalSearch,
            Action::EnterSelectionMode,
        ] {
            let json = serde_json::to_string(&action).unwrap();
            let back: Action = serde_json::from_str(&json).unwrap();
            assert_eq!(action, back);
        }
    }

    #[test]
    fn key_context_roundtrip_json() {
        for ctx in [
            KeyContext::Global,
            KeyContext::Chat,
            KeyContext::Permission,
            KeyContext::AgentDetail,
        ] {
            let json = serde_json::to_string(&ctx).unwrap();
            let back: KeyContext = serde_json::from_str(&json).unwrap();
            assert_eq!(ctx, back);
        }
    }
}
