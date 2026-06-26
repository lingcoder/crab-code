//! Multi-line text input component with cursor movement and history.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
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
    /// Undo stack: `(lines, cursor_row, cursor_col)`.
    undo_stack: Vec<(Vec<String>, usize, usize)>,
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
            undo_stack: Vec::new(),
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

    /// Save current state to the undo stack.
    fn save_undo(&mut self) {
        // Limit undo stack to 50 entries
        if self.undo_stack.len() >= 50 {
            self.undo_stack.remove(0);
        }
        self.undo_stack
            .push((self.lines.clone(), self.cursor_row, self.cursor_col));
    }

    /// Undo the last edit.
    pub fn undo(&mut self) {
        if let Some((lines, row, col)) = self.undo_stack.pop() {
            self.lines = lines;
            self.cursor_row = row;
            self.cursor_col = col;
        }
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
        // Save undo state before plain text-modifying keys. Control/alt edit
        // combos checkpoint themselves inside their own arms.
        let plain_or_shift = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        match key.code {
            KeyCode::Char(_) | KeyCode::Backspace | KeyCode::Delete | KeyCode::Enter
                if plain_or_shift =>
            {
                self.save_undo();
            }
            _ => {}
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            // Readline cursor movement (no undo checkpoint — no mutation).
            KeyCode::Char('a') if ctrl => {
                self.cursor_col = 0;
                true
            }
            KeyCode::Char('e') if ctrl => {
                self.cursor_col = self.current_line().len();
                true
            }
            KeyCode::Char('b') if alt => {
                self.move_word_left();
                true
            }
            KeyCode::Char('f') if alt => {
                self.move_word_right();
                true
            }
            // Readline kill ops (checkpoint here since the guard above skips
            // control/alt combos).
            KeyCode::Char('w') if ctrl => {
                self.save_undo();
                self.exit_history_browse();
                self.kill_word_backward();
                true
            }
            KeyCode::Char('d') if alt => {
                self.save_undo();
                self.exit_history_browse();
                self.kill_word_forward();
                true
            }
            KeyCode::Char('u') if ctrl => {
                self.save_undo();
                self.exit_history_browse();
                self.kill_to_line_start();
                true
            }
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

    /// Bulk-insert `text` at the current cursor position.
    ///
    /// Differs from character-by-character `insert_char`: `\r\n` and bare `\r`
    /// are normalized to `\n`, and embedded newlines split the input into
    /// additional lines. The cursor lands at the end of the inserted region.
    pub fn insert_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.save_undo();
        self.exit_history_browse();

        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");

        let col = self.cursor_col.min(self.lines[self.cursor_row].len());
        let tail = self.lines[self.cursor_row].split_off(col);
        let mut segments = normalized.split('\n');

        // First segment appends onto the current line.
        let first = segments.next().unwrap_or("");
        self.lines[self.cursor_row].push_str(first);

        let mut last_len_before_tail = self.lines[self.cursor_row].len();
        for seg in segments {
            self.cursor_row += 1;
            self.lines.insert(self.cursor_row, seg.to_string());
            last_len_before_tail = seg.len();
        }

        self.cursor_col = last_len_before_tail;
        self.lines[self.cursor_row].push_str(&tail);
    }

    /// Delete the half-open range `[(start_row,start_col), (end_row,end_col))`
    /// and return the removed text. The cursor lands at the range start.
    pub fn delete_range(
        &mut self,
        start_row: usize,
        start_col: usize,
        end_row: usize,
        end_col: usize,
    ) -> String {
        if start_row >= self.lines.len() {
            return String::new();
        }
        self.save_undo();
        let removed = self.slice_range(start_row, start_col, end_row, end_col);
        if start_row == end_row {
            let line = &mut self.lines[start_row];
            let s = start_col.min(line.len());
            let e = end_col.min(line.len());
            line.replace_range(s..e.max(s), "");
        } else {
            let er = end_row.min(self.lines.len() - 1);
            let s = start_col.min(self.lines[start_row].len());
            let e = end_col.min(self.lines[er].len());
            let tail = self.lines[er][e..].to_string();
            self.lines[start_row].truncate(s);
            self.lines[start_row].push_str(&tail);
            self.lines.drain(start_row + 1..=er);
        }
        self.cursor_row = start_row;
        self.cursor_col = start_col.min(self.lines[start_row].len());
        removed
    }

    /// Read (without removing) the half-open range
    /// `[(start_row,start_col), (end_row,end_col))`.
    #[must_use]
    pub fn slice_range(
        &self,
        start_row: usize,
        start_col: usize,
        end_row: usize,
        end_col: usize,
    ) -> String {
        if start_row == end_row {
            let line = &self.lines[start_row];
            let s = start_col.min(line.len());
            let e = end_col.min(line.len());
            return line[s..e.max(s)].to_string();
        }
        let mut out = String::new();
        let s = start_col.min(self.lines[start_row].len());
        out.push_str(&self.lines[start_row][s..]);
        for row in (start_row + 1)..end_row.min(self.lines.len()) {
            out.push('\n');
            out.push_str(&self.lines[row]);
        }
        if end_row < self.lines.len() {
            out.push('\n');
            let e = end_col.min(self.lines[end_row].len());
            out.push_str(&self.lines[end_row][..e]);
        }
        out
    }

    /// Delete whole lines `[first, last]` (inclusive) and return them joined by
    /// newlines. Always leaves at least one (possibly empty) line.
    pub fn delete_lines(&mut self, first: usize, last: usize) -> String {
        let last = last.min(self.lines.len().saturating_sub(1));
        if first > last {
            return String::new();
        }
        self.save_undo();
        let removed: Vec<String> = self.lines.drain(first..=last).collect();
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_row = first.min(self.lines.len() - 1);
        self.cursor_col = 0;
        removed.join("\n")
    }

    /// Read (without removing) whole lines `[first, last]` (inclusive).
    #[must_use]
    pub fn slice_lines(&self, first: usize, last: usize) -> String {
        let last = last.min(self.lines.len().saturating_sub(1));
        if first > last {
            return String::new();
        }
        self.lines[first..=last].join("\n")
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

    /// Clamp `col` to the line length and snap it down to the nearest UTF-8
    /// char boundary, so vertical motion never strands the cursor mid-codepoint.
    fn snap_col_to_boundary(&self, row: usize, col: usize) -> usize {
        let line = &self.lines[row];
        let mut c = col.min(line.len());
        while c > 0 && !line.is_char_boundary(c) {
            c -= 1;
        }
        c
    }

    fn move_up(&mut self) {
        if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.snap_col_to_boundary(self.cursor_row, self.cursor_col);
        }
    }

    fn move_down(&mut self) {
        if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = self.snap_col_to_boundary(self.cursor_row, self.cursor_col);
        }
    }

    /// Byte offset of the start of the word at or before the cursor on the
    /// current line. Skips trailing whitespace, then the word characters.
    fn word_start_offset(&self) -> usize {
        let line = self.current_line();
        let bytes = line.as_bytes();
        let mut idx = self.cursor_col.min(line.len());
        while idx > 0 && bytes[idx - 1].is_ascii_whitespace() {
            idx -= 1;
        }
        while idx > 0 && !bytes[idx - 1].is_ascii_whitespace() {
            idx -= 1;
        }
        idx
    }

    /// Byte offset of the end of the word at or after the cursor on the
    /// current line. Skips leading whitespace, then the word characters.
    fn word_end_offset(&self) -> usize {
        let line = self.current_line();
        let bytes = line.as_bytes();
        let len = line.len();
        let mut idx = self.cursor_col.min(len);
        while idx < len && bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }
        while idx < len && !bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }
        idx
    }

    fn move_word_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col = self.word_start_offset();
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.lines[self.cursor_row].len();
        }
    }

    fn move_word_right(&mut self) {
        if self.cursor_col < self.current_line().len() {
            self.cursor_col = self.word_end_offset();
        } else if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = 0;
        }
    }

    fn kill_word_backward(&mut self) {
        let start = self.word_start_offset();
        if start < self.cursor_col {
            self.lines[self.cursor_row].replace_range(start..self.cursor_col, "");
            self.cursor_col = start;
        }
    }

    fn kill_word_forward(&mut self) {
        let end = self.word_end_offset();
        if end > self.cursor_col {
            self.lines[self.cursor_row].replace_range(self.cursor_col..end, "");
        }
    }

    fn kill_to_line_start(&mut self) {
        if self.cursor_col > 0 {
            self.lines[self.cursor_row].replace_range(..self.cursor_col, "");
            self.cursor_col = 0;
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
            let display = Line::from(line.as_str());
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
    fn vertical_move_onto_cjk_line_does_not_panic() {
        // A byte column carried down from an ASCII line can land mid-codepoint
        // on a CJK line; snapping to a char boundary must keep Delete safe.
        let mut input = InputBox::new();
        input.set_text("abcdef\n你好世界");
        input.cursor_row = 0;
        input.cursor_col = 5;
        input.handle_key(key(KeyCode::Down));
        // Cursor snaps back to the nearest boundary (start of 好 at byte 3).
        assert_eq!(input.cursor(), (1, 3));
        input.handle_key(key(KeyCode::Delete));
        assert_eq!(input.text(), "abcdef\n你世界");
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
    fn renders_empty_when_no_text() {
        // Placeholder moved to app.rs render_input_with_prompt()
        let input = InputBox::new();
        let area = Rect::new(0, 0, 30, 1);
        let mut buf = Buffer::empty(area);
        Widget::render(&input, area, &mut buf);

        let content: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        // InputBox itself no longer renders placeholder
        assert!(!content.contains("Type a message"));
    }

    #[test]
    fn insert_text_single_line() {
        let mut input = InputBox::new();
        input.insert_text("hello");
        assert_eq!(input.text(), "hello");
        assert_eq!(input.cursor(), (0, 5));
    }

    #[test]
    fn insert_text_multi_line() {
        let mut input = InputBox::new();
        input.insert_text("line1\nline2\nline3");
        assert_eq!(input.text(), "line1\nline2\nline3");
        assert_eq!(input.line_count(), 3);
        assert_eq!(input.cursor(), (2, 5));
    }

    #[test]
    fn insert_text_empty_noop() {
        let mut input = InputBox::new();
        input.set_text("abc");
        input.insert_text("");
        assert_eq!(input.text(), "abc");
        assert_eq!(input.cursor(), (0, 3));
    }

    #[test]
    fn insert_text_normalizes_crlf() {
        let mut input = InputBox::new();
        input.insert_text("a\r\nb\r\nc");
        assert_eq!(input.text(), "a\nb\nc");
        assert_eq!(input.line_count(), 3);
        assert_eq!(input.cursor(), (2, 1));
    }

    #[test]
    fn insert_text_normalizes_bare_cr() {
        let mut input = InputBox::new();
        input.insert_text("a\rb");
        assert_eq!(input.text(), "a\nb");
        assert_eq!(input.line_count(), 2);
    }

    #[test]
    fn insert_text_preserves_tail_after_cursor() {
        let mut input = InputBox::new();
        input.set_text("AB");
        input.set_cursor_pos(0, 1);
        input.insert_text("xy");
        assert_eq!(input.text(), "AxyB");
        assert_eq!(input.cursor(), (0, 3));
    }

    #[test]
    fn insert_text_multi_line_splits_tail() {
        let mut input = InputBox::new();
        input.set_text("ABCD");
        input.set_cursor_pos(0, 2);
        input.insert_text("x\ny");
        assert_eq!(input.text(), "ABx\nyCD");
        assert_eq!(input.cursor(), (1, 1));
    }

    #[test]
    fn insert_text_supports_undo() {
        let mut input = InputBox::new();
        input.set_text("hi");
        input.set_cursor_pos(0, 2);
        input.insert_text(" there");
        assert_eq!(input.text(), "hi there");
        input.undo();
        assert_eq!(input.text(), "hi");
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

    // ── Readline editing keys ──

    #[test]
    fn ctrl_a_moves_to_line_start() {
        let mut input = InputBox::new();
        input.set_text("hello");
        input.handle_key(key_with(KeyCode::Char('a'), KeyModifiers::CONTROL));
        assert_eq!(input.cursor(), (0, 0));
    }

    #[test]
    fn ctrl_e_moves_to_line_end() {
        let mut input = InputBox::new();
        input.set_text("hello");
        input.set_cursor_pos(0, 0);
        input.handle_key(key_with(KeyCode::Char('e'), KeyModifiers::CONTROL));
        assert_eq!(input.cursor(), (0, 5));
    }

    #[test]
    fn alt_b_moves_word_left() {
        let mut input = InputBox::new();
        input.set_text("foo bar");
        input.handle_key(key_with(KeyCode::Char('b'), KeyModifiers::ALT));
        assert_eq!(input.cursor(), (0, 4));
        input.handle_key(key_with(KeyCode::Char('b'), KeyModifiers::ALT));
        assert_eq!(input.cursor(), (0, 0));
    }

    #[test]
    fn alt_f_moves_word_right() {
        let mut input = InputBox::new();
        input.set_text("foo bar");
        input.set_cursor_pos(0, 0);
        input.handle_key(key_with(KeyCode::Char('f'), KeyModifiers::ALT));
        assert_eq!(input.cursor(), (0, 3));
        input.handle_key(key_with(KeyCode::Char('f'), KeyModifiers::ALT));
        assert_eq!(input.cursor(), (0, 7));
    }

    #[test]
    fn ctrl_w_kills_word_backward() {
        let mut input = InputBox::new();
        input.set_text("foo bar");
        input.handle_key(key_with(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(input.text(), "foo ");
        assert_eq!(input.cursor(), (0, 4));
    }

    #[test]
    fn alt_d_kills_word_forward() {
        let mut input = InputBox::new();
        input.set_text("foo bar");
        input.set_cursor_pos(0, 0);
        input.handle_key(key_with(KeyCode::Char('d'), KeyModifiers::ALT));
        assert_eq!(input.text(), " bar");
        assert_eq!(input.cursor(), (0, 0));
    }

    #[test]
    fn ctrl_u_kills_to_line_start() {
        let mut input = InputBox::new();
        input.set_text("foo bar");
        input.set_cursor_pos(0, 4);
        input.handle_key(key_with(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(input.text(), "bar");
        assert_eq!(input.cursor(), (0, 0));
    }

    #[test]
    fn ctrl_w_supports_single_undo() {
        let mut input = InputBox::new();
        input.set_text("foo bar");
        input.set_cursor_pos(0, 7);
        input.handle_key(key_with(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(input.text(), "foo ");
        input.undo();
        assert_eq!(input.text(), "foo bar");
    }

    #[test]
    fn ctrl_combos_do_not_insert_literal() {
        let mut input = InputBox::new();
        input.handle_key(key_with(KeyCode::Char('a'), KeyModifiers::CONTROL));
        assert!(input.is_empty());
    }

    // ── Range helpers (used by vim operators) ──

    #[test]
    fn delete_range_same_line() {
        let mut input = InputBox::new();
        input.set_text("hello world");
        let removed = input.delete_range(0, 0, 0, 6);
        assert_eq!(removed, "hello ");
        assert_eq!(input.text(), "world");
        assert_eq!(input.cursor(), (0, 0));
    }

    #[test]
    fn delete_range_cross_line_merges() {
        let mut input = InputBox::new();
        input.set_text("abc\ndef\nghi");
        let removed = input.delete_range(0, 1, 2, 1);
        assert_eq!(input.text(), "ahi");
        assert!(removed.contains("bc"));
    }

    #[test]
    fn slice_range_does_not_mutate() {
        let mut input = InputBox::new();
        input.set_text("hello");
        assert_eq!(input.slice_range(0, 0, 0, 3), "hel");
        assert_eq!(input.text(), "hello");
    }

    #[test]
    fn delete_lines_inclusive() {
        let mut input = InputBox::new();
        input.set_text("a\nb\nc");
        let removed = input.delete_lines(0, 1);
        assert_eq!(removed, "a\nb");
        assert_eq!(input.text(), "c");
        assert_eq!(input.cursor(), (0, 0));
    }

    #[test]
    fn delete_lines_all_keeps_empty() {
        let mut input = InputBox::new();
        input.set_text("a\nb");
        input.delete_lines(0, 1);
        assert!(input.is_empty());
        assert_eq!(input.line_count(), 1);
    }

    #[test]
    fn delete_range_supports_undo() {
        let mut input = InputBox::new();
        input.set_text("hello");
        input.delete_range(0, 0, 0, 2);
        assert_eq!(input.text(), "llo");
        input.undo();
        assert_eq!(input.text(), "hello");
    }
}
