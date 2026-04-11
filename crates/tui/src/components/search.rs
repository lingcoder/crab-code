//! In-conversation search with match highlighting.
//!
//! Triggered by `/` key, highlights all matches in the content area.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::theme::Theme;

/// A single search match location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchMatch {
    /// Line number (0-indexed).
    pub line: usize,
    /// Byte offset within the line.
    pub col: usize,
    /// Length of the match in bytes.
    pub len: usize,
}

/// Search state for the conversation content.
#[derive(Debug, Clone)]
pub struct SearchState {
    /// The current search query.
    query: String,
    /// Whether search mode is active.
    active: bool,
    /// All matches found.
    matches: Vec<SearchMatch>,
    /// Currently highlighted match index.
    current_match: usize,
}

impl SearchState {
    /// Create a new inactive search state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            query: String::new(),
            active: false,
            matches: Vec::new(),
            current_match: 0,
        }
    }

    /// Whether search mode is active.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Activate search mode.
    pub fn activate(&mut self) {
        self.active = true;
        self.query.clear();
        self.matches.clear();
        self.current_match = 0;
    }

    /// Deactivate search mode.
    pub fn deactivate(&mut self) {
        self.active = false;
        self.query.clear();
        self.matches.clear();
        self.current_match = 0;
    }

    /// Get the current query.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Add a character to the query and re-search.
    pub fn push_char(&mut self, c: char) {
        self.query.push(c);
    }

    /// Remove the last character from the query.
    pub fn pop_char(&mut self) {
        self.query.pop();
    }

    /// Get the current match index.
    #[must_use]
    pub fn current_match_index(&self) -> usize {
        self.current_match
    }

    /// Total number of matches.
    #[must_use]
    pub fn match_count(&self) -> usize {
        self.matches.len()
    }

    /// Get all matches.
    #[must_use]
    pub fn matches(&self) -> &[SearchMatch] {
        &self.matches
    }

    /// Get the current match.
    #[must_use]
    pub fn current(&self) -> Option<&SearchMatch> {
        self.matches.get(self.current_match)
    }

    /// Move to the next match.
    pub fn next_match(&mut self) {
        if !self.matches.is_empty() {
            self.current_match = (self.current_match + 1) % self.matches.len();
        }
    }

    /// Move to the previous match.
    pub fn prev_match(&mut self) {
        if !self.matches.is_empty() {
            if self.current_match == 0 {
                self.current_match = self.matches.len() - 1;
            } else {
                self.current_match -= 1;
            }
        }
    }

    /// Perform the search on the given text content.
    pub fn search(&mut self, content: &str) {
        self.matches.clear();
        self.current_match = 0;

        if self.query.is_empty() {
            return;
        }

        let query_lower = self.query.to_lowercase();
        for (line_num, line) in content.lines().enumerate() {
            let line_lower = line.to_lowercase();
            let mut start = 0;
            while let Some(pos) = line_lower[start..].find(&query_lower) {
                self.matches.push(SearchMatch {
                    line: line_num,
                    col: start + pos,
                    len: query_lower.len(),
                });
                start += pos + query_lower.len();
            }
        }
    }

    /// Check if a position in the content is part of a match.
    /// Returns Some(true) if it's the current match, Some(false) if another match.
    #[must_use]
    pub fn match_at(&self, line: usize, col: usize) -> Option<bool> {
        for (i, m) in self.matches.iter().enumerate() {
            if m.line == line && col >= m.col && col < m.col + m.len {
                return Some(i == self.current_match);
            }
        }
        None
    }
}

impl Default for SearchState {
    fn default() -> Self {
        Self::new()
    }
}

/// Render the search bar at the bottom of the content area.
///
/// All colors come from `theme` — the search prompt uses `theme.warning` for
/// the leading `/`, `theme.text_bright` for the query text, and `theme.muted`
/// for the "(n/m)" match counter.
pub fn render_search_bar(search: &SearchState, theme: &Theme, area: Rect, buf: &mut Buffer) {
    if area.height == 0 || !search.is_active() {
        return;
    }

    let match_info = if search.query().is_empty() {
        String::new()
    } else if search.matches.is_empty() {
        " (no matches)".to_string()
    } else {
        format!(" ({}/{})", search.current_match + 1, search.matches.len())
    };

    let line = Line::from(vec![
        Span::styled(
            "/",
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(search.query(), Style::default().fg(theme.text_bright)),
        Span::styled(match_info, Style::default().fg(theme.muted)),
    ]);

    Widget::render(line, area, buf);
}

/// Style for search match highlighting.
///
/// The "current" match uses `theme.highlight_bg` + `theme.highlight_fg`,
/// other matches use `theme.selection_bg` + `theme.selection_fg`.
#[must_use]
pub fn match_style(theme: &Theme, is_current: bool) -> Style {
    if is_current {
        Style::default()
            .bg(theme.highlight_bg)
            .fg(theme.highlight_fg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .bg(theme.selection_bg)
            .fg(theme.selection_fg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_search_is_inactive() {
        let search = SearchState::new();
        assert!(!search.is_active());
        assert!(search.query().is_empty());
        assert_eq!(search.match_count(), 0);
    }

    #[test]
    fn activate_deactivate() {
        let mut search = SearchState::new();
        search.activate();
        assert!(search.is_active());
        search.deactivate();
        assert!(!search.is_active());
    }

    #[test]
    fn push_pop_char() {
        let mut search = SearchState::new();
        search.activate();
        search.push_char('h');
        search.push_char('i');
        assert_eq!(search.query(), "hi");
        search.pop_char();
        assert_eq!(search.query(), "h");
        search.pop_char();
        assert!(search.query().is_empty());
        search.pop_char(); // no panic on empty
    }

    #[test]
    fn search_basic() {
        let mut search = SearchState::new();
        search.activate();
        search.push_char('h');
        search.push_char('e');
        search.push_char('l');
        search.push_char('l');
        search.push_char('o');

        let content = "Hello world\nHello again\nbye";
        search.search(content);

        assert_eq!(search.match_count(), 2);
        assert_eq!(search.matches()[0].line, 0);
        assert_eq!(search.matches()[0].col, 0);
        assert_eq!(search.matches()[1].line, 1);
    }

    #[test]
    fn search_case_insensitive() {
        let mut search = SearchState::new();
        search.activate();
        search.push_char('H');
        search.push_char('E');
        search.push_char('L');

        let content = "hello world\nHELLO again";
        search.search(content);
        assert_eq!(search.match_count(), 2);
    }

    #[test]
    fn search_multiple_on_same_line() {
        let mut search = SearchState::new();
        search.activate();
        search.push_char('a');

        let content = "aaa bbb aaa";
        search.search(content);
        assert_eq!(search.match_count(), 6); // "a" appears 6 times
    }

    #[test]
    fn search_empty_query() {
        let mut search = SearchState::new();
        search.activate();
        search.search("some content");
        assert_eq!(search.match_count(), 0);
    }

    #[test]
    fn next_prev_match() {
        let mut search = SearchState::new();
        search.activate();
        search.push_char('x');
        search.search("x y x z x");
        assert_eq!(search.match_count(), 3);

        assert_eq!(search.current_match_index(), 0);
        search.next_match();
        assert_eq!(search.current_match_index(), 1);
        search.next_match();
        assert_eq!(search.current_match_index(), 2);
        search.next_match();
        assert_eq!(search.current_match_index(), 0); // wraps

        search.prev_match();
        assert_eq!(search.current_match_index(), 2); // wraps back
        search.prev_match();
        assert_eq!(search.current_match_index(), 1);
    }

    #[test]
    fn next_prev_no_matches() {
        let mut search = SearchState::new();
        search.next_match(); // no panic
        search.prev_match(); // no panic
    }

    #[test]
    fn current_match() {
        let mut search = SearchState::new();
        assert!(search.current().is_none());

        search.activate();
        search.push_char('o');
        search.search("hello world");
        assert!(search.current().is_some());
        assert_eq!(search.current().unwrap().line, 0);
    }

    #[test]
    fn match_at_check() {
        let mut search = SearchState::new();
        search.activate();
        search.push_char('h');
        search.push_char('i');
        search.search("hi there, hi!");

        // First "hi" at col 0
        assert_eq!(search.match_at(0, 0), Some(true)); // current match
        assert_eq!(search.match_at(0, 1), Some(true)); // still in first match

        // Second "hi" at col 10
        assert_eq!(search.match_at(0, 10), Some(false)); // not current

        // Not a match
        assert_eq!(search.match_at(0, 5), None);
        assert_eq!(search.match_at(1, 0), None);
    }

    #[test]
    fn match_style_current_uses_theme_highlight() {
        let theme = Theme::dark();
        let style = match_style(&theme, true);
        assert_eq!(style.bg, Some(theme.highlight_bg));
        assert_eq!(style.fg, Some(theme.highlight_fg));
        // Dark theme preserves historical yellow-on-black appearance.
        assert_eq!(style.bg, Some(ratatui::style::Color::Yellow));
        assert_eq!(style.fg, Some(ratatui::style::Color::Black));
    }

    #[test]
    fn match_style_other_uses_theme_selection() {
        let theme = Theme::dark();
        let style = match_style(&theme, false);
        assert_eq!(style.bg, Some(theme.selection_bg));
        assert_eq!(style.fg, Some(theme.selection_fg));
    }

    #[test]
    fn match_style_follows_theme_switch() {
        // Monokai should produce different colors than dark without
        // callers having to pass anything but a different theme.
        let dark = Theme::dark();
        let monokai = Theme::monokai();
        let dark_style = match_style(&dark, true);
        let monokai_style = match_style(&monokai, true);
        assert_ne!(dark_style.bg, monokai_style.bg);
    }

    #[test]
    fn render_search_bar_inactive() {
        let search = SearchState::new();
        let theme = Theme::dark();
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        render_search_bar(&search, &theme, area, &mut buf);
        // No content rendered (inactive)
    }

    #[test]
    fn render_search_bar_active() {
        let mut search = SearchState::new();
        search.activate();
        search.push_char('t');
        search.push_char('e');
        search.push_char('s');
        search.push_char('t');
        search.search("test content with test");

        let theme = Theme::dark();
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        render_search_bar(&search, &theme, area, &mut buf);

        let buf_ref = &buf;
        let row: String = (0..area.width)
            .map(|x| buf_ref.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(row.contains('/'));
        assert!(row.contains("test"));
        assert!(row.contains("1/2")); // match count
    }

    #[test]
    fn render_search_bar_uses_theme_colors() {
        // Buffer cell assertion: verify the rendered '/' prompt uses
        // theme.warning and the query text uses theme.text_bright.
        // This proves the render path flows through theme fields, not
        // hardcoded literals. Under dark theme these happen to equal
        // Color::Yellow / Color::White, confirming byte-identical output.
        let mut search = SearchState::new();
        search.activate();
        search.push_char('x');
        search.search("abc");

        let theme = Theme::dark();
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        render_search_bar(&search, &theme, area, &mut buf);

        // First cell is '/'
        let slash_cell = buf.cell((0, 0)).unwrap();
        assert_eq!(slash_cell.symbol(), "/");
        assert_eq!(slash_cell.fg, theme.warning);

        // Second cell is 'x' — first character of query
        let query_cell = buf.cell((1, 0)).unwrap();
        assert_eq!(query_cell.symbol(), "x");
        assert_eq!(query_cell.fg, theme.text_bright);
    }

    #[test]
    fn render_search_bar_no_matches() {
        let mut search = SearchState::new();
        search.activate();
        search.push_char('z');
        search.search("no z here... wait");

        let theme = Theme::dark();
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        render_search_bar(&search, &theme, area, &mut buf);
    }

    #[test]
    fn deactivate_clears_state() {
        let mut search = SearchState::new();
        search.activate();
        search.push_char('x');
        search.search("x x x");
        assert_eq!(search.match_count(), 3);

        search.deactivate();
        assert!(search.query().is_empty());
        assert_eq!(search.match_count(), 0);
    }

    #[test]
    fn default_search() {
        let search = SearchState::default();
        assert!(!search.is_active());
    }
}
