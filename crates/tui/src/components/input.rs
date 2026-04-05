//! Multi-line text input component with cursor movement and history.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

/// Multi-line text input box with cursor and history support.
pub struct InputBox {
    /// Lines of text (always at least one empty line).
    lines: Vec<String>,
    /// Cursor row (0-based, index into `lines`).
    cursor_row: usize,
    /// Cursor column (0-based byte offset within the current line).
    cursor_col: usize,
    /// Input history (most recent last).
    history: Vec<String>,
    /// Current position in history when browsing (None = not browsing).
    history_index: Option<usize>,
    /// Saved current input when entering history browse mode.
    saved_input: Option<String>,
}

impl InputBox {
    #[must_use]
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            cursor_row: 0,
            cursor_col: 0,
            history: Vec::new(),
            history_index: None,
            saved_input: None,
        }
    }

    /// Current text content (all lines joined with newlines).
    #[must_use]
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    /// Whether the input is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].is_empty()
    }

    /// Current cursor position (row, col).
    #[must_use]
    pub const fn cursor(&self) -> (usize, usize) {
        (self.cursor_row, self.cursor_col)
    }

    /// Number of lines.
    #[must_use]
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Submit the current input: return the text, push to history, and clear.
    pub fn submit(&mut self) -> String {
        let text = self.text();
        if !text.trim().is_empty() {
            self.history.push(text.clone());
        }
        self.clear();
        text
    }

    /// Clear the input box.
    pub fn clear(&mut self) {
        self.lines = vec![String::new()];
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.history_index = None;
        self.saved_input = None;
    }

    /// Handle a key event. Returns `true` if the event was consumed.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char(c) => {
                self.exit_history_browse();
                self.insert_char(c);
                true
            }
            KeyCode::Backspace => {
                self.exit_history_browse();
                self.backspace();
                true
            }
            KeyCode::Delete => {
                self.exit_history_browse();
                self.delete();
                true
            }
            KeyCode::Left => {
                self.move_left();
                true
            }
            KeyCode::Right => {
                self.move_right();
                true
            }
            KeyCode::Up => {
                if key.modifiers.contains(KeyModifiers::ALT) || self.lines.len() == 1 {
                    self.history_up();
                } else {
                    self.move_up();
                }
                true
            }
            KeyCode::Down => {
                if key.modifiers.contains(KeyModifiers::ALT) || self.lines.len() == 1 {
                    self.history_down();
                } else {
                    self.move_down();
                }
                true
            }
            KeyCode::Home => {
                self.cursor_col = 0;
                true
            }
            KeyCode::End => {
                self.cursor_col = self.current_line().len();
                true
            }
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.exit_history_browse();
                self.insert_newline();
                true
            }
            _ => false,
        }
    }

    /// Set the cursor position directly (row, col).
    ///
    /// Clamps to valid bounds within current content.
    pub fn set_cursor_pos(&mut self, row: usize, col: usize) {
        self.cursor_row = row.min(self.lines.len().saturating_sub(1));
        self.cursor_col = col.min(self.lines[self.cursor_row].len());
    }

    /// Set the input text programmatically (e.g., from history).
    pub fn set_text(&mut self, text: &str) {
        self.lines = text.lines().map(String::from).collect();
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_row = self.lines.len() - 1;
        self.cursor_col = self.lines[self.cursor_row].len();
    }

    // ── Internal helpers ──

    fn current_line(&self) -> &str {
        &self.lines[self.cursor_row]
    }

    fn insert_char(&mut self, c: char) {
        let col = self.cursor_col.min(self.lines[self.cursor_row].len());
        self.lines[self.cursor_row].insert(col, c);
        self.cursor_col = col + c.len_utf8();
    }

    fn insert_newline(&mut self) {
        let col = self.cursor_col.min(self.lines[self.cursor_row].len());
        let rest = self.lines[self.cursor_row][col..].to_string();
        self.lines[self.cursor_row].truncate(col);
        self.cursor_row += 1;
        self.lines.insert(self.cursor_row, rest);
        self.cursor_col = 0;
    }

    fn backspace(&mut self) {
        if self.cursor_col > 0 {
            let col = self.cursor_col.min(self.lines[self.cursor_row].len());
            // Find the byte boundary of the previous char
            let prev_boundary = self.lines[self.cursor_row][..col]
                .char_indices()
                .next_back()
                .map_or(0, |(i, _)| i);
            self.lines[self.cursor_row].remove(prev_boundary);
            self.cursor_col = prev_boundary;
        } else if self.cursor_row > 0 {
            // Merge with previous line
            let current = self.lines.remove(self.cursor_row);
            self.cursor_row -= 1;
            self.cursor_col = self.lines[self.cursor_row].len();
            self.lines[self.cursor_row].push_str(&current);
        }
    }

    fn delete(&mut self) {
        let line_len = self.lines[self.cursor_row].len();
        if self.cursor_col < line_len {
            self.lines[self.cursor_row].remove(self.cursor_col);
        } else if self.cursor_row + 1 < self.lines.len() {
            let next = self.lines.remove(self.cursor_row + 1);
            self.lines[self.cursor_row].push_str(&next);
        }
    }

    fn move_left(&mut self) {
        if self.cursor_col > 0 {
            let prev = self.lines[self.cursor_row][..self.cursor_col]
                .char_indices()
                .next_back()
                .map_or(0, |(i, _)| i);
            self.cursor_col = prev;
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.lines[self.cursor_row].len();
        }
    }

    fn move_right(&mut self) {
        let line_len = self.lines[self.cursor_row].len();
        if self.cursor_col < line_len {
            let next = self.lines[self.cursor_row][self.cursor_col..]
                .char_indices()
                .nth(1)
                .map_or(line_len, |(i, _)| self.cursor_col + i);
            self.cursor_col = next;
        } else if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = 0;
        }
    }

    fn move_up(&mut self) {
        if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.cursor_col.min(self.lines[self.cursor_row].len());
        }
    }

    fn move_down(&mut self) {
        if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = self.cursor_col.min(self.lines[self.cursor_row].len());
        }
    }

    fn history_up(&mut self) {
        if self.history.is_empty() {
            return;
        }
        match self.history_index {
            None => {
                self.saved_input = Some(self.text());
                let idx = self.history.len() - 1;
                self.history_index = Some(idx);
                self.set_text(&self.history[idx].clone());
            }
            Some(idx) if idx > 0 => {
                let new_idx = idx - 1;
                self.history_index = Some(new_idx);
                self.set_text(&self.history[new_idx].clone());
            }
            _ => {}
        }
    }

    fn history_down(&mut self) {
        match self.history_index {
            Some(idx) if idx + 1 < self.history.len() => {
                let new_idx = idx + 1;
                self.history_index = Some(new_idx);
                self.set_text(&self.history[new_idx].clone());
            }
            Some(_) => {
                self.history_index = None;
                if let Some(saved) = self.saved_input.take() {
                    self.set_text(&saved);
                }
            }
            None => {}
        }
    }

    fn exit_history_browse(&mut self) {
        self.history_index = None;
        self.saved_input = None;
    }
}

impl Default for InputBox {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for &InputBox {
    #[allow(clippy::cast_possible_truncation)]
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let visible_lines = area.height as usize;
        // Scroll so cursor row is visible
        let scroll_offset = if self.cursor_row >= visible_lines {
            self.cursor_row - visible_lines + 1
        } else {
            0
        };

        for (i, line) in self
            .lines
            .iter()
            .skip(scroll_offset)
            .take(visible_lines)
            .enumerate()
        {
            let y = area.y + i as u16;
            let display = if line.is_empty() && i == 0 && scroll_offset == 0 && self.is_empty() {
                Line::from(Span::styled(
                    "Type a message...",
                    Style::default().fg(Color::DarkGray),
                ))
            } else {
                Line::from(line.as_str())
            };
            let line_area = Rect {
                x: area.x,
                y,
                width: area.width,
                height: 1,
            };
            Widget::render(display, line_area, buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn key_with(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn new_is_empty() {
        let input = InputBox::new();
        assert!(input.is_empty());
        assert_eq!(input.text(), "");
        assert_eq!(input.cursor(), (0, 0));
        assert_eq!(input.line_count(), 1);
    }

    #[test]
    fn type_chars() {
        let mut input = InputBox::new();
        input.handle_key(key(KeyCode::Char('h')));
        input.handle_key(key(KeyCode::Char('i')));
        assert_eq!(input.text(), "hi");
        assert_eq!(input.cursor(), (0, 2));
        assert!(!input.is_empty());
    }

    #[test]
    fn backspace_removes_char() {
        let mut input = InputBox::new();
        input.handle_key(key(KeyCode::Char('a')));
        input.handle_key(key(KeyCode::Char('b')));
        input.handle_key(key(KeyCode::Backspace));
        assert_eq!(input.text(), "a");
        assert_eq!(input.cursor(), (0, 1));
    }

    #[test]
    fn backspace_on_empty_does_nothing() {
        let mut input = InputBox::new();
        input.handle_key(key(KeyCode::Backspace));
        assert!(input.is_empty());
    }

    #[test]
    fn delete_removes_char_ahead() {
        let mut input = InputBox::new();
        input.set_text("abc");
        input.cursor_col = 0;
        input.handle_key(key(KeyCode::Delete));
        assert_eq!(input.text(), "bc");
    }

    #[test]
    fn left_right_movement() {
        let mut input = InputBox::new();
        input.set_text("abc");
        assert_eq!(input.cursor(), (0, 3));

        input.handle_key(key(KeyCode::Left));
        assert_eq!(input.cursor(), (0, 2));

        input.handle_key(key(KeyCode::Left));
        assert_eq!(input.cursor(), (0, 1));

        input.handle_key(key(KeyCode::Right));
        assert_eq!(input.cursor(), (0, 2));
    }

    #[test]
    fn home_end_movement() {
        let mut input = InputBox::new();
        input.set_text("hello");

        input.handle_key(key(KeyCode::Home));
        assert_eq!(input.cursor(), (0, 0));

        input.handle_key(key(KeyCode::End));
        assert_eq!(input.cursor(), (0, 5));
    }

    #[test]
    fn shift_enter_creates_newline() {
        let mut input = InputBox::new();
        input.handle_key(key(KeyCode::Char('a')));
        input.handle_key(key_with(KeyCode::Enter, KeyModifiers::SHIFT));
        input.handle_key(key(KeyCode::Char('b')));
        assert_eq!(input.text(), "a\nb");
        assert_eq!(input.line_count(), 2);
        assert_eq!(input.cursor(), (1, 1));
    }

    #[test]
    fn up_down_in_multiline() {
        let mut input = InputBox::new();
        input.set_text("line1\nline2\nline3");
        // cursor at end of line3
        assert_eq!(input.cursor(), (2, 5));

        input.handle_key(key(KeyCode::Up));
        assert_eq!(input.cursor(), (1, 5));

        input.handle_key(key(KeyCode::Up));
        assert_eq!(input.cursor(), (0, 5));

        input.handle_key(key(KeyCode::Down));
        assert_eq!(input.cursor(), (1, 5));
    }

    #[test]
    fn submit_clears_and_returns_text() {
        let mut input = InputBox::new();
        input.set_text("hello world");
        let text = input.submit();
        assert_eq!(text, "hello world");
        assert!(input.is_empty());
    }

    #[test]
    fn submit_pushes_to_history() {
        let mut input = InputBox::new();
        input.set_text("command 1");
        input.submit();
        input.set_text("command 2");
        input.submit();
        assert_eq!(input.history.len(), 2);
    }

    #[test]
    fn history_up_down() {
        let mut input = InputBox::new();
        input.set_text("first");
        input.submit();
        input.set_text("second");
        input.submit();

        // Arrow up gets most recent
        input.handle_key(key(KeyCode::Up));
        assert_eq!(input.text(), "second");

        // Arrow up again gets older
        input.handle_key(key(KeyCode::Up));
        assert_eq!(input.text(), "first");

        // Arrow down goes back
        input.handle_key(key(KeyCode::Down));
        assert_eq!(input.text(), "second");

        // Arrow down again restores original input
        input.handle_key(key(KeyCode::Down));
        assert_eq!(input.text(), "");
    }

    #[test]
    fn history_preserves_current_input() {
        let mut input = InputBox::new();
        input.set_text("old");
        input.submit();

        input.set_text("current typing");
        input.handle_key(key(KeyCode::Up));
        assert_eq!(input.text(), "old");

        input.handle_key(key(KeyCode::Down));
        assert_eq!(input.text(), "current typing");
    }

    #[test]
    fn backspace_merges_lines() {
        let mut input = InputBox::new();
        input.set_text("ab\ncd");
        input.cursor_row = 1;
        input.cursor_col = 0;
        input.handle_key(key(KeyCode::Backspace));
        assert_eq!(input.text(), "abcd");
        assert_eq!(input.cursor(), (0, 2));
    }

    #[test]
    fn delete_merges_next_line() {
        let mut input = InputBox::new();
        input.set_text("ab\ncd");
        input.cursor_row = 0;
        input.cursor_col = 2;
        input.handle_key(key(KeyCode::Delete));
        assert_eq!(input.text(), "abcd");
    }

    #[test]
    fn submit_empty_does_not_add_history() {
        let mut input = InputBox::new();
        input.submit();
        assert!(input.history.is_empty());
    }

    #[test]
    fn renders_placeholder_when_empty() {
        let input = InputBox::new();
        let area = Rect::new(0, 0, 30, 1);
        let mut buf = Buffer::empty(area);
        Widget::render(&input, area, &mut buf);

        let content: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(content.contains("Type a message"));
    }

    #[test]
    fn renders_text_content() {
        let mut input = InputBox::new();
        input.set_text("hello");

        let area = Rect::new(0, 0, 30, 1);
        let mut buf = Buffer::empty(area);
        Widget::render(&input, area, &mut buf);

        let content: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(content.contains("hello"));
    }
}
