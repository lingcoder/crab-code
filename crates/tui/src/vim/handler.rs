use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::mode::VimMode;
use super::motion::{CursorPos, Motion};
use crate::components::input::InputBox;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VimAction {
    Consumed,
    Submit,
    Ignored,
}

pub struct VimHandler {
    mode: VimMode,
    enabled: bool,
}

impl VimHandler {
    #[must_use]
    pub fn new() -> Self {
        Self {
            mode: VimMode::Normal,
            enabled: false,
        }
    }

    #[must_use]
    pub const fn mode(&self) -> VimMode {
        self.mode
    }

    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn toggle(&mut self) {
        self.enabled = !self.enabled;
        if self.enabled {
            self.mode = VimMode::Normal;
        } else {
            self.mode = VimMode::Insert;
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.mode = VimMode::Insert;
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent, input: &mut InputBox) -> VimAction {
        if !self.enabled {
            if input.handle_key(key) {
                return VimAction::Consumed;
            }
            return VimAction::Ignored;
        }

        match self.mode {
            VimMode::Normal => self.handle_normal(key, input),
            VimMode::Insert => self.handle_insert(key, input),
            VimMode::Visual | VimMode::Command => {
                if key.code == KeyCode::Esc {
                    self.mode = VimMode::Normal;
                    VimAction::Consumed
                } else {
                    VimAction::Ignored
                }
            }
        }
    }

    fn handle_normal(&mut self, key: KeyEvent, input: &mut InputBox) -> VimAction {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return VimAction::Ignored;
        }

        match key.code {
            KeyCode::Char('i') => {
                self.mode = VimMode::Insert;
                VimAction::Consumed
            }
            KeyCode::Char('a') => {
                apply_motion(input, Motion::Right);
                self.mode = VimMode::Insert;
                VimAction::Consumed
            }
            KeyCode::Char('o') => {
                let (row, _) = input.cursor();
                let lines = collect_lines(input);
                let end_col = lines.get(row).map_or(0, String::len);
                input.set_cursor_pos(row, end_col);
                input.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
                self.mode = VimMode::Insert;
                VimAction::Consumed
            }
            KeyCode::Char('A') => {
                let (row, _) = input.cursor();
                let lines = collect_lines(input);
                let end_col = lines.get(row).map_or(0, String::len);
                input.set_cursor_pos(row, end_col);
                self.mode = VimMode::Insert;
                VimAction::Consumed
            }
            KeyCode::Char('I') => {
                apply_motion(input, Motion::FirstNonBlank);
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

            KeyCode::Char('h') | KeyCode::Left => {
                apply_motion(input, Motion::Left);
                VimAction::Consumed
            }
            KeyCode::Char('j') | KeyCode::Down => {
                apply_motion(input, Motion::Down);
                VimAction::Consumed
            }
            KeyCode::Char('k') | KeyCode::Up => {
                apply_motion(input, Motion::Up);
                VimAction::Consumed
            }
            KeyCode::Char('l') | KeyCode::Right => {
                apply_motion(input, Motion::Right);
                VimAction::Consumed
            }
            KeyCode::Char('0') => {
                apply_motion(input, Motion::LineStart);
                VimAction::Consumed
            }
            KeyCode::Char('$') => {
                apply_motion(input, Motion::LineEnd);
                VimAction::Consumed
            }
            KeyCode::Char('^') => {
                apply_motion(input, Motion::FirstNonBlank);
                VimAction::Consumed
            }
            KeyCode::Char('w') => {
                apply_motion(input, Motion::WordForward);
                VimAction::Consumed
            }
            KeyCode::Char('b') => {
                apply_motion(input, Motion::WordBackward);
                VimAction::Consumed
            }
            KeyCode::Char('G') => {
                apply_motion(input, Motion::BufferBottom);
                VimAction::Consumed
            }

            KeyCode::Enter => VimAction::Submit,

            _ => VimAction::Ignored,
        }
    }

    fn handle_insert(&mut self, key: KeyEvent, input: &mut InputBox) -> VimAction {
        match key.code {
            KeyCode::Esc => {
                self.mode = VimMode::Normal;
                apply_motion(input, Motion::Left);
                VimAction::Consumed
            }
            _ => {
                if input.handle_key(key) {
                    VimAction::Consumed
                } else {
                    VimAction::Ignored
                }
            }
        }
    }
}

impl Default for VimHandler {
    fn default() -> Self {
        Self::new()
    }
}

fn apply_motion(input: &mut InputBox, motion: Motion) {
    let (row, col) = input.cursor();
    let lines = collect_lines(input);
    let line_refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let new_pos = motion.apply(CursorPos { row, col }, &line_refs);
    input.set_cursor_pos(new_pos.row, new_pos.col);
}

fn collect_lines(input: &InputBox) -> Vec<String> {
    input.text().lines().map(String::from).collect()
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

    fn make() -> (VimHandler, InputBox) {
        let mut vh = VimHandler::new();
        vh.set_enabled(true);
        (vh, InputBox::new())
    }

    #[test]
    fn starts_in_normal_mode() {
        let (vh, _) = make();
        assert_eq!(vh.mode(), VimMode::Normal);
    }

    #[test]
    fn i_enters_insert() {
        let (mut vh, mut input) = make();
        assert_eq!(vh.handle_key(key(KeyCode::Char('i')), &mut input), VimAction::Consumed);
        assert_eq!(vh.mode(), VimMode::Insert);
    }

    #[test]
    fn esc_returns_to_normal() {
        let (mut vh, mut input) = make();
        vh.handle_key(key(KeyCode::Char('i')), &mut input);
        assert_eq!(vh.mode(), VimMode::Insert);
        vh.handle_key(key(KeyCode::Esc), &mut input);
        assert_eq!(vh.mode(), VimMode::Normal);
    }

    #[test]
    fn insert_mode_passes_chars_to_input() {
        let (mut vh, mut input) = make();
        vh.handle_key(key(KeyCode::Char('i')), &mut input);
        vh.handle_key(key(KeyCode::Char('h')), &mut input);
        vh.handle_key(key(KeyCode::Char('i')), &mut input);
        assert_eq!(input.text(), "hi");
    }

    #[test]
    fn normal_mode_h_moves_left() {
        let (mut vh, mut input) = make();
        input.set_text("hello");
        vh.handle_key(key(KeyCode::Char('h')), &mut input);
        let (_, col) = input.cursor();
        assert!(col < 5);
    }

    #[test]
    fn a_enters_insert_after_cursor() {
        let (mut vh, mut input) = make();
        input.set_text("ab");
        input.set_cursor_pos(0, 0);
        vh.handle_key(key(KeyCode::Char('a')), &mut input);
        assert_eq!(vh.mode(), VimMode::Insert);
    }

    #[test]
    fn enter_in_normal_submits() {
        let (mut vh, mut input) = make();
        input.set_text("hello");
        assert_eq!(vh.handle_key(key(KeyCode::Enter), &mut input), VimAction::Submit);
    }

    #[test]
    fn v_enters_visual() {
        let (mut vh, mut input) = make();
        vh.handle_key(key(KeyCode::Char('v')), &mut input);
        assert_eq!(vh.mode(), VimMode::Visual);
    }

    #[test]
    fn colon_enters_command() {
        let (mut vh, mut input) = make();
        vh.handle_key(key(KeyCode::Char(':')), &mut input);
        assert_eq!(vh.mode(), VimMode::Command);
    }

    #[test]
    fn esc_from_visual_returns_to_normal() {
        let (mut vh, mut input) = make();
        vh.handle_key(key(KeyCode::Char('v')), &mut input);
        vh.handle_key(key(KeyCode::Esc), &mut input);
        assert_eq!(vh.mode(), VimMode::Normal);
    }

    #[test]
    fn disabled_passes_through() {
        let mut vh = VimHandler::new();
        let mut input = InputBox::new();
        vh.handle_key(key(KeyCode::Char('x')), &mut input);
        assert_eq!(input.text(), "x");
    }

    #[test]
    fn ctrl_keys_ignored_in_normal() {
        let (mut vh, mut input) = make();
        assert_eq!(
            vh.handle_key(ctrl_key(KeyCode::Char('c')), &mut input),
            VimAction::Ignored
        );
    }

    #[test]
    fn toggle_enabled() {
        let mut vh = VimHandler::new();
        assert!(!vh.is_enabled());
        vh.toggle();
        assert!(vh.is_enabled());
        assert_eq!(vh.mode(), VimMode::Normal);
        vh.toggle();
        assert!(!vh.is_enabled());
        assert_eq!(vh.mode(), VimMode::Insert);
    }

    #[test]
    fn o_opens_line_below() {
        let (mut vh, mut input) = make();
        input.set_text("line1");
        input.set_cursor_pos(0, 0);
        vh.handle_key(key(KeyCode::Char('o')), &mut input);
        assert_eq!(vh.mode(), VimMode::Insert);
        assert_eq!(input.line_count(), 2);
    }

    #[test]
    fn j_k_navigate_lines() {
        let (mut vh, mut input) = make();
        input.set_text("aaa\nbbb\nccc");
        input.set_cursor_pos(0, 0);
        vh.handle_key(key(KeyCode::Char('j')), &mut input);
        assert_eq!(input.cursor().0, 1);
        vh.handle_key(key(KeyCode::Char('k')), &mut input);
        assert_eq!(input.cursor().0, 0);
    }
}
