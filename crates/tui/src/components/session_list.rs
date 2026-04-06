//! Session list — sortable, searchable list of all saved sessions.
//!
//! Displays sessions with metadata (creation time, message count, last activity)
//! and supports sorting, filtering, deletion, and archival.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::theme::Theme;

// ─── Types ──────────────────────────────────────────────────────────────

/// A session entry with full metadata.
#[derive(Debug, Clone)]
pub struct SessionListEntry {
    /// Unique session identifier.
    pub id: String,
    /// User-visible title.
    pub title: String,
    /// Creation timestamp (epoch seconds).
    pub created_at: u64,
    /// Number of messages in the session.
    pub message_count: usize,
    /// Last activity timestamp (epoch seconds).
    pub last_activity: u64,
    /// Whether this session is archived.
    pub archived: bool,
    /// Whether this is the currently active session.
    pub active: bool,
}

impl SessionListEntry {
    /// Create a new session entry.
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        created_at: u64,
        last_activity: u64,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            created_at,
            message_count: 0,
            last_activity,
            archived: false,
            active: false,
        }
    }

    /// Set message count.
    #[must_use]
    pub fn with_message_count(mut self, count: usize) -> Self {
        self.message_count = count;
        self
    }

    /// Set archived state.
    #[must_use]
    pub fn with_archived(mut self, archived: bool) -> Self {
        self.archived = archived;
        self
    }

    /// Set active state.
    #[must_use]
    pub fn with_active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }
}

/// Sort order for the session list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    /// Most recently active first.
    RecentActivity,
    /// Most recently created first.
    RecentCreated,
    /// Alphabetical by title.
    Alphabetical,
    /// Most messages first.
    MessageCount,
}

impl std::fmt::Display for SortOrder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RecentActivity => write!(f, "Recent Activity"),
            Self::RecentCreated => write!(f, "Recent Created"),
            Self::Alphabetical => write!(f, "Alphabetical"),
            Self::MessageCount => write!(f, "Message Count"),
        }
    }
}

/// Action resulting from a session list interaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionListAction {
    /// Switch to the selected session.
    Switch(String),
    /// Delete the selected session.
    Delete(String),
    /// Archive the selected session.
    Archive(String),
    /// No action (key consumed).
    Consumed,
    /// Dismissed the list.
    Dismissed,
}

// ─── Format helpers ─────────────────────────────────────────────────────

/// Format an epoch timestamp as a relative time string.
#[must_use]
pub fn format_relative_epoch(timestamp: u64, now: u64) -> String {
    let diff = now.saturating_sub(timestamp);
    if diff < 60 {
        return "just now".to_string();
    }
    let mins = diff / 60;
    if mins < 60 {
        return format!("{mins}m ago");
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("{hours}h ago");
    }
    let days = hours / 24;
    if days < 30 {
        return format!("{days}d ago");
    }
    let months = days / 30;
    format!("{months}mo ago")
}

// ─── SessionList state ──────────────────────────────────────────────────

/// Session list state with sorting, filtering, and navigation.
pub struct SessionList {
    /// All session entries.
    entries: Vec<SessionListEntry>,
    /// Indices into entries after filtering and sorting.
    visible_indices: Vec<usize>,
    /// Currently selected position in `visible_indices`.
    selected: usize,
    /// Scroll offset.
    scroll_offset: usize,
    /// Current sort order.
    sort_order: SortOrder,
    /// Current search filter query.
    filter_query: String,
    /// Whether the filter input is active.
    filter_active: bool,
    /// Whether to show archived sessions.
    show_archived: bool,
    /// Current time for relative timestamps (epoch seconds).
    current_time: u64,
}

impl SessionList {
    /// Create a new session list.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            visible_indices: Vec::new(),
            selected: 0,
            scroll_offset: 0,
            sort_order: SortOrder::RecentActivity,
            filter_query: String::new(),
            filter_active: false,
            show_archived: false,
            current_time: 0,
        }
    }

    /// Set the session entries and rebuild the visible list.
    pub fn set_entries(&mut self, entries: Vec<SessionListEntry>) {
        self.entries = entries;
        self.rebuild_visible();
    }

    /// Set the current time (for relative timestamps).
    pub fn set_current_time(&mut self, time: u64) {
        self.current_time = time;
    }

    /// Get the current time.
    #[must_use]
    pub fn current_time(&self) -> u64 {
        self.current_time
    }

    /// Total number of entries (before filtering).
    #[must_use]
    pub fn total_count(&self) -> usize {
        self.entries.len()
    }

    /// Number of visible entries (after filtering).
    #[must_use]
    pub fn visible_count(&self) -> usize {
        self.visible_indices.len()
    }

    /// Current selected index in visible list.
    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Scroll offset.
    #[must_use]
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Current sort order.
    #[must_use]
    pub fn sort_order(&self) -> SortOrder {
        self.sort_order
    }

    /// Get the filter query.
    #[must_use]
    pub fn filter_query(&self) -> &str {
        &self.filter_query
    }

    /// Whether filter input is active.
    #[must_use]
    pub fn filter_active(&self) -> bool {
        self.filter_active
    }

    /// Whether archived sessions are shown.
    #[must_use]
    pub fn show_archived(&self) -> bool {
        self.show_archived
    }

    /// Get the currently selected entry.
    #[must_use]
    pub fn selected_entry(&self) -> Option<&SessionListEntry> {
        self.visible_indices
            .get(self.selected)
            .and_then(|&idx| self.entries.get(idx))
    }

    /// Get visible entries.
    #[must_use]
    pub fn visible_entries(&self) -> Vec<&SessionListEntry> {
        self.visible_indices
            .iter()
            .filter_map(|&idx| self.entries.get(idx))
            .collect()
    }

    // ─── Navigation ───

    /// Move selection up.
    pub fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
        self.adjust_scroll();
    }

    /// Move selection down.
    pub fn select_next(&mut self) {
        if !self.visible_indices.is_empty() && self.selected < self.visible_indices.len() - 1 {
            self.selected += 1;
        }
        self.adjust_scroll();
    }

    /// Confirm selection — returns the session ID.
    #[must_use]
    pub fn confirm(&self) -> Option<SessionListAction> {
        self.selected_entry()
            .map(|e| SessionListAction::Switch(e.id.clone()))
    }

    /// Delete the selected session.
    #[must_use]
    pub fn delete_selected(&self) -> Option<SessionListAction> {
        self.selected_entry()
            .map(|e| SessionListAction::Delete(e.id.clone()))
    }

    /// Archive the selected session.
    #[must_use]
    pub fn archive_selected(&self) -> Option<SessionListAction> {
        self.selected_entry()
            .map(|e| SessionListAction::Archive(e.id.clone()))
    }

    // ─── Sorting ───

    /// Set the sort order and rebuild.
    pub fn set_sort_order(&mut self, order: SortOrder) {
        self.sort_order = order;
        self.rebuild_visible();
    }

    /// Cycle to the next sort order.
    pub fn cycle_sort(&mut self) {
        self.sort_order = match self.sort_order {
            SortOrder::RecentActivity => SortOrder::RecentCreated,
            SortOrder::RecentCreated => SortOrder::Alphabetical,
            SortOrder::Alphabetical => SortOrder::MessageCount,
            SortOrder::MessageCount => SortOrder::RecentActivity,
        };
        self.rebuild_visible();
    }

    // ─── Filtering ───

    /// Start the filter input.
    pub fn start_filter(&mut self) {
        self.filter_active = true;
        self.filter_query.clear();
    }

    /// Stop the filter input.
    pub fn stop_filter(&mut self) {
        self.filter_active = false;
        self.filter_query.clear();
        self.rebuild_visible();
    }

    /// Type a character into the filter.
    pub fn filter_type_char(&mut self, ch: char) {
        self.filter_query.push(ch);
        self.rebuild_visible();
    }

    /// Delete the last character from the filter.
    pub fn filter_backspace(&mut self) {
        self.filter_query.pop();
        self.rebuild_visible();
    }

    /// Toggle showing archived sessions.
    pub fn toggle_archived(&mut self) {
        self.show_archived = !self.show_archived;
        self.rebuild_visible();
    }

    // ─── Internal ───

    /// Rebuild the visible indices based on filter and sort.
    fn rebuild_visible(&mut self) {
        let query = self.filter_query.to_lowercase();
        let show_archived = self.show_archived;

        // Filter
        let mut indices: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                if !show_archived && e.archived {
                    return false;
                }
                if query.is_empty() {
                    return true;
                }
                e.title.to_lowercase().contains(&query) || e.id.to_lowercase().contains(&query)
            })
            .map(|(i, _)| i)
            .collect();

        // Sort
        let entries = &self.entries;
        match self.sort_order {
            SortOrder::RecentActivity => {
                indices.sort_by(|&a, &b| entries[b].last_activity.cmp(&entries[a].last_activity));
            }
            SortOrder::RecentCreated => {
                indices.sort_by(|&a, &b| entries[b].created_at.cmp(&entries[a].created_at));
            }
            SortOrder::Alphabetical => {
                indices.sort_by(|&a, &b| {
                    entries[a]
                        .title
                        .to_lowercase()
                        .cmp(&entries[b].title.to_lowercase())
                });
            }
            SortOrder::MessageCount => {
                indices.sort_by(|&a, &b| entries[b].message_count.cmp(&entries[a].message_count));
            }
        }

        self.visible_indices = indices;
        if self.selected >= self.visible_indices.len() && !self.visible_indices.is_empty() {
            self.selected = self.visible_indices.len() - 1;
        }
        if self.visible_indices.is_empty() {
            self.selected = 0;
        }
    }

    /// Adjust scroll to keep selected in view.
    #[allow(clippy::unused_self)]
    fn adjust_scroll(&self) {
        // Scroll adjustment is applied at render time
    }

    /// Adjust scroll for a given viewport height.
    pub fn adjust_scroll_for_height(&mut self, height: usize) {
        if height == 0 {
            return;
        }
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + height {
            self.scroll_offset = self.selected - height + 1;
        }
    }
}

impl Default for SessionList {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Widget ─────────────────────────────────────────────────────────────

/// Widget for rendering the session list.
pub struct SessionListWidget<'a> {
    list: &'a SessionList,
    theme: &'a Theme,
}

impl<'a> SessionListWidget<'a> {
    #[must_use]
    pub fn new(list: &'a SessionList, theme: &'a Theme) -> Self {
        Self { list, theme }
    }
}

impl Widget for SessionListWidget<'_> {
    #[allow(clippy::cast_possible_truncation)]
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 2 || area.width < 15 {
            return;
        }

        let now = self.list.current_time();

        // Header: sort order + count
        let header_y = area.y;
        let header_text = format!(
            " {} ({}/{})",
            self.list.sort_order(),
            self.list.visible_count(),
            self.list.total_count(),
        );
        let header_style = Style::default()
            .fg(self.theme.muted)
            .add_modifier(Modifier::ITALIC);
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut((x, header_y)) {
                cell.set_char(' ');
                cell.set_style(Style::default().bg(self.theme.bg));
            }
        }
        let header_line = Line::from(Span::styled(header_text, header_style));
        Widget::render(header_line, Rect::new(area.x, header_y, area.width, 1), buf);

        // Filter line (if active)
        let filter_lines: u16 = u16::from(self.list.filter_active());
        if self.list.filter_active() {
            let filter_y = area.y + 1;
            let _filter_text = format!(" / {}", self.list.filter_query());
            let filter_style = Style::default().fg(self.theme.fg);
            for x in area.x..area.x + area.width {
                if let Some(cell) = buf.cell_mut((x, filter_y)) {
                    cell.set_char(' ');
                    cell.set_style(filter_style);
                }
            }
            let filter_line = Line::from(vec![
                Span::styled(" / ", Style::default().fg(self.theme.warning)),
                Span::styled(self.list.filter_query(), filter_style),
            ]);
            Widget::render(filter_line, Rect::new(area.x, filter_y, area.width, 1), buf);
        }

        // List body
        let body_y = area.y + 1 + filter_lines;
        let body_height = area.height.saturating_sub(1 + filter_lines) as usize;
        let scroll = self.list.scroll_offset();
        let entries = self.list.visible_entries();

        for (vi, entry) in entries.iter().skip(scroll).take(body_height).enumerate() {
            let y = body_y + vi as u16;
            let idx = scroll + vi;
            let is_selected = idx == self.list.selected();

            let bg = if is_selected {
                self.theme.border
            } else {
                self.theme.bg
            };

            // Clear line
            for x in area.x..area.x + area.width {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(' ');
                    cell.set_style(Style::default().bg(bg));
                }
            }

            let indicator = if entry.active {
                "\u{25cf} " // ●
            } else {
                "  "
            };

            let title_style = if entry.active {
                Style::default()
                    .fg(self.theme.success)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD)
            } else if entry.archived {
                Style::default()
                    .fg(self.theme.muted)
                    .bg(bg)
                    .add_modifier(Modifier::ITALIC)
            } else {
                Style::default().fg(self.theme.fg).bg(bg)
            };

            let time_str = format_relative_epoch(entry.last_activity, now);
            let meta = format!(" ({}, {})", entry.message_count, time_str);

            let spans = vec![
                Span::styled(indicator, title_style),
                Span::styled(&entry.title, title_style),
                Span::styled(meta, Style::default().fg(self.theme.muted).bg(bg)),
            ];

            let line = Line::from(spans);
            Widget::render(line, Rect::new(area.x, y, area.width, 1), buf);
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entries() -> Vec<SessionListEntry> {
        vec![
            SessionListEntry::new("s1", "Build feature A", 1000, 5000)
                .with_message_count(10)
                .with_active(true),
            SessionListEntry::new("s2", "Debug crash", 2000, 4000).with_message_count(25),
            SessionListEntry::new("s3", "Refactor utils", 3000, 3000).with_message_count(5),
            SessionListEntry::new("s4", "Archived task", 500, 500)
                .with_message_count(3)
                .with_archived(true),
        ]
    }

    #[test]
    fn session_list_entry_new() {
        let entry = SessionListEntry::new("id1", "My Session", 100, 200);
        assert_eq!(entry.id, "id1");
        assert_eq!(entry.title, "My Session");
        assert_eq!(entry.created_at, 100);
        assert_eq!(entry.last_activity, 200);
        assert_eq!(entry.message_count, 0);
        assert!(!entry.archived);
        assert!(!entry.active);
    }

    #[test]
    fn session_list_entry_builders() {
        let entry = SessionListEntry::new("id1", "Test", 0, 0)
            .with_message_count(42)
            .with_archived(true)
            .with_active(true);
        assert_eq!(entry.message_count, 42);
        assert!(entry.archived);
        assert!(entry.active);
    }

    #[test]
    fn sort_order_display() {
        assert_eq!(SortOrder::RecentActivity.to_string(), "Recent Activity");
        assert_eq!(SortOrder::Alphabetical.to_string(), "Alphabetical");
        assert_eq!(SortOrder::MessageCount.to_string(), "Message Count");
    }

    #[test]
    fn format_relative_epoch_values() {
        assert_eq!(format_relative_epoch(100, 100), "just now");
        assert_eq!(format_relative_epoch(100, 130), "just now");
        assert_eq!(format_relative_epoch(100, 220), "2m ago");
        assert_eq!(format_relative_epoch(100, 3700), "1h ago");
        assert_eq!(format_relative_epoch(100, 86500), "1d ago");
        assert_eq!(format_relative_epoch(0, 2_700_000), "1mo ago");
    }

    #[test]
    fn session_list_new() {
        let list = SessionList::new();
        assert_eq!(list.total_count(), 0);
        assert_eq!(list.visible_count(), 0);
        assert_eq!(list.selected(), 0);
        assert_eq!(list.sort_order(), SortOrder::RecentActivity);
        assert!(!list.filter_active());
        assert!(!list.show_archived());
    }

    #[test]
    fn session_list_default() {
        let list = SessionList::default();
        assert_eq!(list.total_count(), 0);
    }

    #[test]
    fn session_list_set_entries() {
        let mut list = SessionList::new();
        list.set_entries(sample_entries());
        assert_eq!(list.total_count(), 4);
        // Archived entry hidden by default
        assert_eq!(list.visible_count(), 3);
    }

    #[test]
    fn session_list_sort_recent_activity() {
        let mut list = SessionList::new();
        list.set_entries(sample_entries());
        list.set_sort_order(SortOrder::RecentActivity);
        let visible = list.visible_entries();
        // s1 (5000) > s2 (4000) > s3 (3000)
        assert_eq!(visible[0].id, "s1");
        assert_eq!(visible[1].id, "s2");
        assert_eq!(visible[2].id, "s3");
    }

    #[test]
    fn session_list_sort_alphabetical() {
        let mut list = SessionList::new();
        list.set_entries(sample_entries());
        list.set_sort_order(SortOrder::Alphabetical);
        let visible = list.visible_entries();
        assert_eq!(visible[0].title, "Build feature A");
        assert_eq!(visible[1].title, "Debug crash");
        assert_eq!(visible[2].title, "Refactor utils");
    }

    #[test]
    fn session_list_sort_message_count() {
        let mut list = SessionList::new();
        list.set_entries(sample_entries());
        list.set_sort_order(SortOrder::MessageCount);
        let visible = list.visible_entries();
        assert_eq!(visible[0].message_count, 25);
        assert_eq!(visible[1].message_count, 10);
        assert_eq!(visible[2].message_count, 5);
    }

    #[test]
    fn session_list_cycle_sort() {
        let mut list = SessionList::new();
        assert_eq!(list.sort_order(), SortOrder::RecentActivity);
        list.cycle_sort();
        assert_eq!(list.sort_order(), SortOrder::RecentCreated);
        list.cycle_sort();
        assert_eq!(list.sort_order(), SortOrder::Alphabetical);
        list.cycle_sort();
        assert_eq!(list.sort_order(), SortOrder::MessageCount);
        list.cycle_sort();
        assert_eq!(list.sort_order(), SortOrder::RecentActivity);
    }

    #[test]
    fn session_list_navigation() {
        let mut list = SessionList::new();
        list.set_entries(sample_entries());

        assert_eq!(list.selected(), 0);
        list.select_next();
        assert_eq!(list.selected(), 1);
        list.select_next();
        assert_eq!(list.selected(), 2);
        list.select_next(); // clamp
        assert_eq!(list.selected(), 2);

        list.select_prev();
        assert_eq!(list.selected(), 1);
        list.select_prev();
        assert_eq!(list.selected(), 0);
        list.select_prev(); // clamp
        assert_eq!(list.selected(), 0);
    }

    #[test]
    fn session_list_selected_entry() {
        let mut list = SessionList::new();
        list.set_entries(sample_entries());
        let entry = list.selected_entry().unwrap();
        assert_eq!(entry.id, "s1"); // sorted by recent activity
    }

    #[test]
    fn session_list_confirm() {
        let mut list = SessionList::new();
        list.set_entries(sample_entries());
        let action = list.confirm().unwrap();
        assert_eq!(action, SessionListAction::Switch("s1".into()));
    }

    #[test]
    fn session_list_delete() {
        let mut list = SessionList::new();
        list.set_entries(sample_entries());
        let action = list.delete_selected().unwrap();
        assert_eq!(action, SessionListAction::Delete("s1".into()));
    }

    #[test]
    fn session_list_archive() {
        let mut list = SessionList::new();
        list.set_entries(sample_entries());
        let action = list.archive_selected().unwrap();
        assert_eq!(action, SessionListAction::Archive("s1".into()));
    }

    #[test]
    fn session_list_filter() {
        let mut list = SessionList::new();
        list.set_entries(sample_entries());
        assert_eq!(list.visible_count(), 3);

        list.start_filter();
        assert!(list.filter_active());

        list.filter_type_char('d');
        list.filter_type_char('e');
        // "de" matches "Debug crash"
        assert_eq!(list.visible_count(), 1);
        assert_eq!(list.selected_entry().unwrap().title, "Debug crash");

        list.filter_backspace();
        // "d" matches "Debug crash" and "Build feature A" (contains 'd')
        assert_eq!(list.visible_count(), 2);

        list.stop_filter();
        assert!(!list.filter_active());
        assert_eq!(list.visible_count(), 3); // restored
    }

    #[test]
    fn session_list_toggle_archived() {
        let mut list = SessionList::new();
        list.set_entries(sample_entries());
        assert_eq!(list.visible_count(), 3); // archived hidden

        list.toggle_archived();
        assert!(list.show_archived());
        assert_eq!(list.visible_count(), 4); // archived shown

        list.toggle_archived();
        assert!(!list.show_archived());
        assert_eq!(list.visible_count(), 3);
    }

    #[test]
    fn session_list_set_current_time() {
        let mut list = SessionList::new();
        list.set_current_time(10_000);
        assert_eq!(list.current_time(), 10_000);
    }

    #[test]
    fn session_list_empty_confirm() {
        let list = SessionList::new();
        assert!(list.confirm().is_none());
    }

    #[test]
    fn session_list_adjust_scroll() {
        let mut list = SessionList::new();
        let entries: Vec<_> = (0..20)
            .map(|i| SessionListEntry::new(format!("s{i}"), format!("Session {i}"), i, i))
            .collect();
        list.set_entries(entries);
        for _ in 0..15 {
            list.select_next();
        }
        list.adjust_scroll_for_height(5);
        assert!(list.scroll_offset() + 5 > list.selected());
    }

    #[test]
    fn widget_renders() {
        let mut list = SessionList::new();
        list.set_entries(sample_entries());
        list.set_current_time(10_000);
        let theme = Theme::dark();
        let widget = SessionListWidget::new(&list, &theme);
        let area = Rect::new(0, 0, 50, 10);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);

        // Header should show sort order and count
        let header: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(header.contains("Recent Activity"));
        assert!(header.contains("3/4"));

        // First entry should be visible
        let row1: String = (0..area.width)
            .map(|x| buf.cell((x, 1)).unwrap().symbol().to_string())
            .collect();
        assert!(row1.contains("Build feature A"));
    }

    #[test]
    fn widget_renders_empty() {
        let list = SessionList::new();
        let theme = Theme::dark();
        let widget = SessionListWidget::new(&list, &theme);
        let area = Rect::new(0, 0, 50, 10);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);
    }

    #[test]
    fn widget_small_area() {
        let mut list = SessionList::new();
        list.set_entries(sample_entries());
        let theme = Theme::dark();
        let widget = SessionListWidget::new(&list, &theme);
        let area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);
    }
}
