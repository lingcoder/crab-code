//! The [`VimHandler`] that wraps an [`InputBox`] and intercepts key
//! events to implement vim-like navigation and mode transitions.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::mode::VimMode;
use super::motion::{CursorPos, Motion};
use crate::components::input::InputBox;

/// Result of handling a key event in vim mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VimAction {
    /// Key was consumed, no further action needed.
    Consumed,
    /// The user wants to submit the current input (Enter in normal mode).
    Submit,
    /// Key was not handled — pass through to default handling.
    Ignored,
}

/// Vim-style key handler wrapping an [`InputBox`].
///
/// Maintains the current vim mode and translates key events into
/// `InputBox` operations or mode transitions.
pub struct VimHandler {
    /// The wrapped input box.
    pub input: InputBox,
    /// Current vim mode.
    mode: VimMode,
    /// Whether vim mode is enabled (false = pass-through to `InputBox`).
    enabled: bool,
}

impl VimHandler {
    /// Create a new handler wrapping a fresh `InputBox`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            input: InputBox::new(),
            mode: VimMode::Normal,
            enabled: true,
        }
    }

    /// Current vim mode.
    #[must_use]
    pub const fn mode(&self) -> VimMode {
        self.mode
    }

    /// Whether vim mode is enabled.
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Toggle vim mode on/off.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.mode = VimMode::Insert; // disable = always insert
        }
    }

    /// Handle a key event. Returns the resulting action.
    pub fn handle_key(&mut self, key: KeyEvent) -> VimAction {
        if !self.enabled {
            // Pass everything to InputBox
            if self.input.handle_key(key) {
                return VimAction::Consumed;
            }
            return VimAction::Ignored;
        }

        match self.mode {
            VimMode::Normal => self.handle_normal(key),
            VimMode::Insert => self.handle_insert(key),
            VimMode::Visual | VimMode::Command => {
                // Visual and Command are stubs — Esc returns to Normal
                if key.code == KeyCode::Esc {
                    self.mode = VimMode::Normal;
                    VimAction::Consumed
                } else {
                    VimAction::Ignored
                }
            }
        }
    }

    fn handle_normal(&mut self, key: KeyEvent) -> VimAction {
        // Ctrl+key should not be intercepted by vim
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return VimAction::Ignored;
        }

        match key.code {
            // ─── Mode transitions ───
            KeyCode::Char('i') => {
                self.mode = VimMode::Insert;
                VimAction::Consumed
            }
            KeyCode::Char('a') => {
                // Move right one then enter insert
                self.apply_motion(Motion::Right);
                self.mode = VimMode::Insert;
                VimAction::Consumed
            }
            KeyCode::Char('o') => {
                // Open line below: move to end of line, insert newline, enter insert
                let (row, _) = self.input.cursor();
                let lines = self.collect_lines();
                let end_col = lines.get(row).map_or(0, String::len);
                self.set_cursor(row, end_col);
                // Insert a newline via InputBox
                self.input
                    .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
                self.mode = VimMode::Insert;
                VimAction::Consumed
            }
            KeyCode::Char('A') => {
                // Append at end of line
                let (row, _) = self.input.cursor();
                let lines = self.collect_lines();
                let end_col = lines.get(row).map_or(0, String::len);
                self.set_cursor(row, end_col);
                self.mode = VimMode::Insert;
                VimAction::Consumed
            }
            KeyCode::Char('I') => {
                // Insert at first non-blank
                self.apply_motion(Motion::FirstNonBlank);
                self.mode = VimMode::Insert;
                VimAction::Consumed
            }
            KeyCode::Char('v') => {
                self.mode = VimMode::Visual;
                VimAction::Consumed
            }
            KeyCode::Char(':') => {
                self.mode = VimMode::Command;
                VimAction::Consumed
            }

            // ─── Navigation ───
            KeyCode::Char('h') | KeyCode::Left => {
                self.apply_motion(Motion::Left);
                VimAction::Consumed
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.apply_motion(Motion::Down);
                VimAction::Consumed
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.apply_motion(Motion::Up);
                VimAction::Consumed
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.apply_motion(Motion::Right);
                VimAction::Consumed
            }
            KeyCode::Char('0') => {
                self.apply_motion(Motion::LineStart);
                VimAction::Consumed
            }
            KeyCode::Char('$') => {
                self.apply_motion(Motion::LineEnd);
                VimAction::Consumed
            }
            KeyCode::Char('^') => {
                self.apply_motion(Motion::FirstNonBlank);
                VimAction::Consumed
            }
            KeyCode::Char('w') => {
                self.apply_motion(Motion::WordForward);
                VimAction::Consumed
            }
            KeyCode::Char('b') => {
                self.apply_motion(Motion::WordBackward);
                VimAction::Consumed
            }
            KeyCode::Char('G') => {
                self.apply_motion(Motion::BufferBottom);
                VimAction::Consumed
            }

            // ─── Submit ───
            KeyCode::Enter => VimAction::Submit,

            _ => VimAction::Ignored,
        }
    }

    fn handle_insert(&mut self, key: KeyEvent) -> VimAction {
        match key.code {
            KeyCode::Esc => {
                self.mode = VimMode::Normal;
                // In vim, cursor moves left one on Esc from insert
                self.apply_motion(Motion::Left);
                VimAction::Consumed
            }
            _ => {
                if self.input.handle_key(key) {
                    VimAction::Consumed
                } else {
                    VimAction::Ignored
                }
            }
        }
    }

    fn apply_motion(&mut self, motion: Motion) {
        let (row, col) = self.input.cursor();
        let lines = self.collect_lines();
        let line_refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let new_pos = motion.apply(CursorPos { row, col }, &line_refs);
        self.set_cursor(new_pos.row, new_pos.col);
    }

    fn collect_lines(&self) -> Vec<String> {
        self.input.text().lines().map(String::from).collect()
    }

    fn set_cursor(&mut self, row: usize, col: usize) {
        // Roundtrip through set_text puts cursor at the end of the last
        // line; set_cursor_pos then moves it to the target.
        let text = self.input.text();
        self.input.set_text(&text);
        self.input.set_cursor_pos(row, col);
    }
}

impl Default for VimHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl_key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    #[test]
    fn starts_in_normal_mode() {
        let vh = VimHandler::new();
        assert_eq!(vh.mode(), VimMode::Normal);
    }

    #[test]
    fn i_enters_insert() {
        let mut vh = VimHandler::new();
        assert_eq!(vh.handle_key(key(KeyCode::Char('i'))), VimAction::Consumed);
        assert_eq!(vh.mode(), VimMode::Insert);
    }

    #[test]
    fn esc_returns_to_normal() {
        let mut vh = VimHandler::new();
        vh.handle_key(key(KeyCode::Char('i')));
        assert_eq!(vh.mode(), VimMode::Insert);
        vh.handle_key(key(KeyCode::Esc));
        assert_eq!(vh.mode(), VimMode::Normal);
    }

    #[test]
    fn insert_mode_passes_chars_to_input() {
        let mut vh = VimHandler::new();
        vh.handle_key(key(KeyCode::Char('i')));
        vh.handle_key(key(KeyCode::Char('h')));
        vh.handle_key(key(KeyCode::Char('i')));
        assert_eq!(vh.input.text(), "hi");
    }

    #[test]
    fn normal_mode_h_moves_left() {
        let mut vh = VimHandler::new();
        vh.input.set_text("hello");
        vh.handle_key(key(KeyCode::Char('h')));
        let (_, col) = vh.input.cursor();
        assert!(col < 5);
    }

    #[test]
    fn a_enters_insert_after_cursor() {
        let mut vh = VimHandler::new();
        vh.input.set_text("ab");
        vh.input.set_cursor_pos(0, 0);
        vh.handle_key(key(KeyCode::Char('a')));
        assert_eq!(vh.mode(), VimMode::Insert);
    }

    #[test]
    fn enter_in_normal_submits() {
        let mut vh = VimHandler::new();
        vh.input.set_text("hello");
        assert_eq!(vh.handle_key(key(KeyCode::Enter)), VimAction::Submit);
    }

    #[test]
    fn v_enters_visual() {
        let mut vh = VimHandler::new();
        vh.handle_key(key(KeyCode::Char('v')));
        assert_eq!(vh.mode(), VimMode::Visual);
    }

    #[test]
    fn colon_enters_command() {
        let mut vh = VimHandler::new();
        vh.handle_key(key(KeyCode::Char(':')));
        assert_eq!(vh.mode(), VimMode::Command);
    }

    #[test]
    fn esc_from_visual_returns_to_normal() {
        let mut vh = VimHandler::new();
        vh.handle_key(key(KeyCode::Char('v')));
        vh.handle_key(key(KeyCode::Esc));
        assert_eq!(vh.mode(), VimMode::Normal);
    }

    #[test]
    fn disabled_passes_through() {
        let mut vh = VimHandler::new();
        vh.set_enabled(false);
        vh.handle_key(key(KeyCode::Char('x')));
        assert_eq!(vh.input.text(), "x");
    }

    #[test]
    fn ctrl_keys_ignored_in_normal() {
        let mut vh = VimHandler::new();
        assert_eq!(
            vh.handle_key(ctrl_key(KeyCode::Char('c'))),
            VimAction::Ignored
        );
    }

    #[test]
    fn toggle_enabled() {
        let mut vh = VimHandler::new();
        assert!(vh.is_enabled());
        vh.set_enabled(false);
        assert!(!vh.is_enabled());
        assert_eq!(vh.mode(), VimMode::Insert);
    }

    #[test]
    fn o_opens_line_below() {
        let mut vh = VimHandler::new();
        vh.input.set_text("line1");
        vh.input.set_cursor_pos(0, 0);
        vh.handle_key(key(KeyCode::Char('o')));
        assert_eq!(vh.mode(), VimMode::Insert);
        assert_eq!(vh.input.line_count(), 2);
    }

    #[test]
    fn j_k_navigate_lines() {
        let mut vh = VimHandler::new();
        vh.input.set_text("aaa\nbbb\nccc");
        vh.input.set_cursor_pos(0, 0);
        vh.handle_key(key(KeyCode::Char('j'))); // down
        assert_eq!(vh.input.cursor().0, 1);
        vh.handle_key(key(KeyCode::Char('k'))); // up
        assert_eq!(vh.input.cursor().0, 0);
    }
}
