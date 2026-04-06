//! Session switcher — quick-switch popup for rapidly changing sessions.
//!
//! Similar to Ctrl+Tab in editors: shows recent sessions in a modal popup
//! with preview of last messages, and supports numbered shortcuts (Ctrl+1/2/3).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::theme::Theme;

// ─── Types ──────────────────────────────────────────────────────────────

/// A session entry in the switcher (lightweight, just for display).
#[derive(Debug, Clone)]
pub struct SwitcherEntry {
    /// Session ID.
    pub id: String,
    /// Session title.
    pub title: String,
    /// Whether this is the currently active session.
    pub active: bool,
    /// Last few messages for preview.
    pub preview_lines: Vec<String>,
    /// Message count.
    pub message_count: usize,
}

impl SwitcherEntry {
    /// Create a new switcher entry.
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            active: false,
            preview_lines: Vec::new(),
            message_count: 0,
        }
    }

    /// Set active state.
    #[must_use]
    pub fn with_active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Set preview lines.
    #[must_use]
    pub fn with_preview(mut self, lines: Vec<String>) -> Self {
        self.preview_lines = lines;
        self
    }

    /// Set message count.
    #[must_use]
    pub fn with_message_count(mut self, count: usize) -> Self {
        self.message_count = count;
        self
    }
}

/// Action resulting from the session switcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwitcherAction {
    /// Switch to the session with this ID.
    Switch(String),
    /// Dismissed without switching.
    Dismissed,
    /// Key was consumed (navigation).
    Consumed,
}

// ─── SessionSwitcher state ──────────────────────────────────────────────

/// Quick session switcher popup state.
pub struct SessionSwitcher {
    /// Whether the switcher is visible.
    visible: bool,
    /// Sessions ordered by most recent use.
    entries: Vec<SwitcherEntry>,
    /// Currently selected index.
    selected: usize,
    /// Whether to show preview lines.
    show_preview: bool,
}

impl SessionSwitcher {
    /// Create a new switcher (hidden by default).
    #[must_use]
    pub fn new() -> Self {
        Self {
            visible: false,
            entries: Vec::new(),
            selected: 0,
            show_preview: true,
        }
    }

    /// Show the switcher with the given entries.
    pub fn show(&mut self, entries: Vec<SwitcherEntry>) {
        self.entries = entries;
        self.visible = true;
        // Start on the second entry (first is current session, user wants to switch away)
        self.selected = usize::from(self.entries.len() > 1);
    }

    /// Hide the switcher.
    pub fn hide(&mut self) {
        self.visible = false;
    }

    /// Whether the switcher is visible.
    #[must_use]
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Get the entries.
    #[must_use]
    pub fn entries(&self) -> &[SwitcherEntry] {
        &self.entries
    }

    /// Number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Currently selected index.
    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Whether preview is shown.
    #[must_use]
    pub fn show_preview(&self) -> bool {
        self.show_preview
    }

    /// Toggle preview mode.
    pub fn toggle_preview(&mut self) {
        self.show_preview = !self.show_preview;
    }

    /// Get the selected entry.
    #[must_use]
    pub fn selected_entry(&self) -> Option<&SwitcherEntry> {
        self.entries.get(self.selected)
    }

    /// Move selection to the next entry (wrapping).
    pub fn select_next(&mut self) {
        if !self.entries.is_empty() {
            self.selected = (self.selected + 1) % self.entries.len();
        }
    }

    /// Move selection to the previous entry (wrapping).
    pub fn select_prev(&mut self) {
        if !self.entries.is_empty() {
            self.selected = if self.selected == 0 {
                self.entries.len() - 1
            } else {
                self.selected - 1
            };
        }
    }

    /// Select by number (0-indexed). Returns true if valid.
    pub fn select_by_number(&mut self, n: usize) -> bool {
        if n < self.entries.len() {
            self.selected = n;
            true
        } else {
            false
        }
    }

    /// Confirm the current selection.
    #[must_use]
    pub fn confirm(&mut self) -> SwitcherAction {
        if let Some(entry) = self.entries.get(self.selected) {
            let id = entry.id.clone();
            self.hide();
            SwitcherAction::Switch(id)
        } else {
            self.hide();
            SwitcherAction::Dismissed
        }
    }

    /// Dismiss the switcher.
    #[must_use]
    pub fn dismiss(&mut self) -> SwitcherAction {
        self.hide();
        SwitcherAction::Dismissed
    }
}

impl Default for SessionSwitcher {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Widget ─────────────────────────────────────────────────────────────

/// Widget for rendering the session switcher popup.
pub struct SessionSwitcherWidget<'a> {
    switcher: &'a SessionSwitcher,
    theme: &'a Theme,
}

impl<'a> SessionSwitcherWidget<'a> {
    #[must_use]
    pub fn new(switcher: &'a SessionSwitcher, theme: &'a Theme) -> Self {
        Self { switcher, theme }
    }
}

impl Widget for SessionSwitcherWidget<'_> {
    #[allow(clippy::cast_possible_truncation, clippy::too_many_lines)]
    fn render(self, area: Rect, buf: &mut Buffer) {
        if !self.switcher.is_visible() || area.height < 5 || area.width < 20 {
            return;
        }

        // Calculate popup dimensions
        let entries = self.switcher.entries();
        let preview_height: usize = if self.switcher.show_preview() { 3 } else { 0 };
        let content_height = entries.len() + preview_height + 2; // +2 for title + separator
        let popup_height = (content_height as u16).min(area.height - 2);
        let popup_width = (area.width * 3 / 5).max(30).min(area.width - 4);

        let popup_x = area.x + (area.width - popup_width) / 2;
        let popup_y = area.y + (area.height - popup_height) / 2;

        let border_style = Style::default().fg(self.theme.border);

        // Draw border
        // Top
        render_h_line(
            buf,
            popup_x,
            popup_y,
            popup_width,
            '\u{250c}',
            '\u{2500}',
            '\u{2510}',
            border_style,
        );
        // Bottom
        render_h_line(
            buf,
            popup_x,
            popup_y + popup_height - 1,
            popup_width,
            '\u{2514}',
            '\u{2500}',
            '\u{2518}',
            border_style,
        );
        // Sides
        for y in popup_y + 1..popup_y + popup_height - 1 {
            if let Some(cell) = buf.cell_mut((popup_x, y)) {
                cell.set_char('\u{2502}');
                cell.set_style(border_style);
            }
            // Clear inner
            for x in popup_x + 1..popup_x + popup_width - 1 {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(' ');
                    cell.set_style(Style::default().bg(self.theme.bg));
                }
            }
            if let Some(cell) = buf.cell_mut((popup_x + popup_width - 1, y)) {
                cell.set_char('\u{2502}');
                cell.set_style(border_style);
            }
        }

        // Title
        let title = " Switch Session ";
        let title_style = Style::default()
            .fg(self.theme.heading)
            .add_modifier(Modifier::BOLD);
        let title_x = popup_x + (popup_width - title.len() as u16) / 2;
        let title_line = Line::from(Span::styled(title, title_style));
        Widget::render(
            title_line,
            Rect::new(title_x, popup_y, title.len() as u16, 1),
            buf,
        );

        // Session entries
        let inner_width = popup_width - 2;
        let mut y = popup_y + 1;

        for (idx, entry) in entries.iter().enumerate() {
            if y >= popup_y + popup_height - 1 {
                break;
            }

            let is_selected = idx == self.switcher.selected();
            let bg = if is_selected {
                self.theme.border
            } else {
                self.theme.bg
            };

            let number = if idx < 9 {
                format!(" {} ", idx + 1)
            } else {
                "   ".to_string()
            };

            let indicator = if entry.active { "\u{25cf}" } else { " " };

            let title_style = if entry.active {
                Style::default()
                    .fg(self.theme.success)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(self.theme.fg).bg(bg)
            };

            let spans = vec![
                Span::styled(&number, Style::default().fg(self.theme.muted).bg(bg)),
                Span::styled(indicator, Style::default().fg(self.theme.success).bg(bg)),
                Span::styled(" ", Style::default().bg(bg)),
                Span::styled(&entry.title, title_style),
                Span::styled(
                    format!(" ({})", entry.message_count),
                    Style::default().fg(self.theme.muted).bg(bg),
                ),
            ];

            let line = Line::from(spans);
            Widget::render(line, Rect::new(popup_x + 1, y, inner_width, 1), buf);
            y += 1;
        }

        // Preview section
        if self.switcher.show_preview()
            && let Some(entry) = self.switcher.selected_entry()
            && !entry.preview_lines.is_empty()
            && y < popup_y + popup_height - 1
        {
            // Separator
            render_h_line(
                buf,
                popup_x,
                y,
                popup_width,
                '\u{251c}',
                '\u{2500}',
                '\u{2524}',
                border_style,
            );
            y += 1;

            // Preview lines
            let preview_style = Style::default().fg(self.theme.muted);
            for pline in entry.preview_lines.iter().take(3) {
                if y >= popup_y + popup_height - 1 {
                    break;
                }
                let truncated: String = pline.chars().take(inner_width as usize - 1).collect();
                let line = Line::from(Span::styled(format!(" {truncated}"), preview_style));
                Widget::render(line, Rect::new(popup_x + 1, y, inner_width, 1), buf);
                y += 1;
            }
        }
    }
}

/// Render a horizontal line with corner/fill characters.
#[allow(clippy::too_many_arguments)]
fn render_h_line(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    left: char,
    fill: char,
    right: char,
    style: Style,
) {
    if let Some(cell) = buf.cell_mut((x, y)) {
        cell.set_char(left);
        cell.set_style(style);
    }
    for xi in x + 1..x + width - 1 {
        if let Some(cell) = buf.cell_mut((xi, y)) {
            cell.set_char(fill);
            cell.set_style(style);
        }
    }
    if width > 1
        && let Some(cell) = buf.cell_mut((x + width - 1, y))
    {
        cell.set_char(right);
        cell.set_style(style);
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entries() -> Vec<SwitcherEntry> {
        vec![
            SwitcherEntry::new("s1", "Current Session")
                .with_active(true)
                .with_message_count(10)
                .with_preview(vec!["user: hello".into(), "ai: hi there".into()]),
            SwitcherEntry::new("s2", "Debug Session")
                .with_message_count(5)
                .with_preview(vec!["user: fix bug".into()]),
            SwitcherEntry::new("s3", "Old Session").with_message_count(20),
        ]
    }

    #[test]
    fn switcher_entry_new() {
        let entry = SwitcherEntry::new("id1", "Title");
        assert_eq!(entry.id, "id1");
        assert_eq!(entry.title, "Title");
        assert!(!entry.active);
        assert!(entry.preview_lines.is_empty());
        assert_eq!(entry.message_count, 0);
    }

    #[test]
    fn switcher_entry_builders() {
        let entry = SwitcherEntry::new("id", "T")
            .with_active(true)
            .with_message_count(5)
            .with_preview(vec!["line1".into()]);
        assert!(entry.active);
        assert_eq!(entry.message_count, 5);
        assert_eq!(entry.preview_lines.len(), 1);
    }

    #[test]
    fn switcher_new() {
        let sw = SessionSwitcher::new();
        assert!(!sw.is_visible());
        assert!(sw.is_empty());
        assert_eq!(sw.len(), 0);
        assert_eq!(sw.selected(), 0);
        assert!(sw.show_preview());
    }

    #[test]
    fn switcher_default() {
        let sw = SessionSwitcher::default();
        assert!(!sw.is_visible());
    }

    #[test]
    fn switcher_show_hide() {
        let mut sw = SessionSwitcher::new();
        sw.show(sample_entries());
        assert!(sw.is_visible());
        assert_eq!(sw.len(), 3);
        // Should start on index 1 (skip current session)
        assert_eq!(sw.selected(), 1);

        sw.hide();
        assert!(!sw.is_visible());
    }

    #[test]
    fn switcher_show_single_entry() {
        let mut sw = SessionSwitcher::new();
        sw.show(vec![SwitcherEntry::new("s1", "Only one").with_active(true)]);
        assert_eq!(sw.selected(), 0); // only one entry
    }

    #[test]
    fn switcher_navigation_wraps() {
        let mut sw = SessionSwitcher::new();
        sw.show(sample_entries());

        assert_eq!(sw.selected(), 1);
        sw.select_next();
        assert_eq!(sw.selected(), 2);
        sw.select_next(); // wrap to 0
        assert_eq!(sw.selected(), 0);

        sw.select_prev(); // wrap to 2
        assert_eq!(sw.selected(), 2);
        sw.select_prev();
        assert_eq!(sw.selected(), 1);
    }

    #[test]
    fn switcher_select_by_number() {
        let mut sw = SessionSwitcher::new();
        sw.show(sample_entries());

        assert!(sw.select_by_number(0));
        assert_eq!(sw.selected(), 0);

        assert!(sw.select_by_number(2));
        assert_eq!(sw.selected(), 2);

        assert!(!sw.select_by_number(10)); // out of range
        assert_eq!(sw.selected(), 2); // unchanged
    }

    #[test]
    fn switcher_confirm() {
        let mut sw = SessionSwitcher::new();
        sw.show(sample_entries());
        // selected is 1 = "Debug Session"
        let action = sw.confirm();
        assert_eq!(action, SwitcherAction::Switch("s2".into()));
        assert!(!sw.is_visible());
    }

    #[test]
    fn switcher_dismiss() {
        let mut sw = SessionSwitcher::new();
        sw.show(sample_entries());
        let action = sw.dismiss();
        assert_eq!(action, SwitcherAction::Dismissed);
        assert!(!sw.is_visible());
    }

    #[test]
    fn switcher_confirm_empty() {
        let mut sw = SessionSwitcher::new();
        sw.show(vec![]);
        let action = sw.confirm();
        assert_eq!(action, SwitcherAction::Dismissed);
    }

    #[test]
    fn switcher_toggle_preview() {
        let mut sw = SessionSwitcher::new();
        assert!(sw.show_preview());
        sw.toggle_preview();
        assert!(!sw.show_preview());
        sw.toggle_preview();
        assert!(sw.show_preview());
    }

    #[test]
    fn switcher_selected_entry() {
        let mut sw = SessionSwitcher::new();
        assert!(sw.selected_entry().is_none());

        sw.show(sample_entries());
        let entry = sw.selected_entry().unwrap();
        assert_eq!(entry.id, "s2"); // selected = 1
    }

    #[test]
    fn widget_renders_hidden() {
        let sw = SessionSwitcher::new();
        let theme = Theme::dark();
        let widget = SessionSwitcherWidget::new(&sw, &theme);
        let area = Rect::new(0, 0, 60, 20);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);
        // Nothing should be rendered
    }

    #[test]
    fn widget_renders_visible() {
        let mut sw = SessionSwitcher::new();
        sw.show(sample_entries());
        let theme = Theme::dark();
        let widget = SessionSwitcherWidget::new(&sw, &theme);
        let area = Rect::new(0, 0, 60, 20);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);

        // Check that the popup contains session titles
        let mut found_switch = false;
        let mut found_debug = false;
        for y in 0..area.height {
            let row: String = (0..area.width)
                .map(|x| buf.cell((x, y)).unwrap().symbol().to_string())
                .collect();
            if row.contains("Switch Session") {
                found_switch = true;
            }
            if row.contains("Debug Session") {
                found_debug = true;
            }
        }
        assert!(found_switch, "Should show 'Switch Session' title");
        assert!(found_debug, "Should show 'Debug Session' entry");
    }

    #[test]
    fn widget_small_area() {
        let mut sw = SessionSwitcher::new();
        sw.show(sample_entries());
        let theme = Theme::dark();
        let widget = SessionSwitcherWidget::new(&sw, &theme);
        let area = Rect::new(0, 0, 15, 3);
        let mut buf = Buffer::empty(area);
        // Should not panic
        Widget::render(widget, area, &mut buf);
    }
}
