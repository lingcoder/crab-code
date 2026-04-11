//! Session sidebar — list sessions, switch, and show metadata.
//!
//! Renders a sidebar panel showing available sessions with their names,
//! last-active timestamps, and message counts. Supports keyboard
//! navigation and session switching.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Widget};

/// Metadata for a single session entry.
#[derive(Debug, Clone)]
pub struct SessionEntry {
    /// Unique session identifier.
    pub id: String,
    /// Human-readable session name.
    pub name: String,
    /// Last activity timestamp (ISO 8601 or relative).
    pub last_active: String,
    /// Number of messages in the session.
    pub message_count: usize,
}

impl SessionEntry {
    /// Create a new session entry.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        last_active: impl Into<String>,
        message_count: usize,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            last_active: last_active.into(),
            message_count,
        }
    }

    /// Format the entry for display in the sidebar.
    fn display_line(&self, is_selected: bool) -> Line<'_> {
        let name_style = if is_selected {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        let meta_style = Style::default().fg(Color::DarkGray);

        Line::from(vec![
            Span::styled(&self.name, name_style),
            Span::styled(format!(" ({} msgs)", self.message_count), meta_style),
        ])
    }
}

/// Session sidebar component state.
pub struct SessionSidebar {
    /// All available sessions.
    pub sessions: Vec<SessionEntry>,
    /// Currently selected index.
    pub selected: usize,
    /// Whether the sidebar is visible.
    pub visible: bool,
}

impl SessionSidebar {
    /// Create a new empty sidebar.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sessions: Vec::new(),
            selected: 0,
            visible: false,
        }
    }

    /// Toggle sidebar visibility.
    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    /// Move selection up.
    pub fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Move selection down.
    pub fn select_next(&mut self) {
        if !self.sessions.is_empty() && self.selected < self.sessions.len() - 1 {
            self.selected += 1;
        }
    }

    /// Get the currently selected session entry, if any.
    #[must_use]
    pub fn selected_session(&self) -> Option<&SessionEntry> {
        self.sessions.get(self.selected)
    }

    /// Set the session list, resetting selection if needed.
    pub fn set_sessions(&mut self, sessions: Vec<SessionEntry>) {
        self.sessions = sessions;
        if self.selected >= self.sessions.len() {
            self.selected = self.sessions.len().saturating_sub(1);
        }
    }

    /// Get the next session ID (wrapping around).
    pub fn next_session_id(&self) -> Option<String> {
        if self.sessions.is_empty() {
            return None;
        }
        let next = if self.selected + 1 < self.sessions.len() {
            self.selected + 1
        } else {
            0
        };
        Some(self.sessions[next].id.clone())
    }

    /// Get the previous session ID (wrapping around).
    pub fn prev_session_id(&self) -> Option<String> {
        if self.sessions.is_empty() {
            return None;
        }
        let prev = if self.selected > 0 {
            self.selected - 1
        } else {
            self.sessions.len() - 1
        };
        Some(self.sessions[prev].id.clone())
    }

    /// Default sidebar width.
    #[must_use]
    pub const fn width() -> u16 {
        30
    }
}

impl Widget for &SessionSidebar {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if !self.visible || area.width < 10 || area.height < 3 {
            return;
        }

        let block = Block::default()
            .title(" Sessions ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));

        let inner = block.inner(area);
        Widget::render(block, area, buf);

        if inner.height == 0 || inner.width < 5 {
            return;
        }

        for (i, session) in self.sessions.iter().enumerate() {
            if i as u16 >= inner.height {
                break;
            }
            let is_selected = i == self.selected;
            let line = session.display_line(is_selected);

            let row_area = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);

            if is_selected {
                // Highlight selected row background
                for x in row_area.x..row_area.x + row_area.width {
                    if let Some(cell) = buf.cell_mut((x, row_area.y)) {
                        cell.set_bg(Color::DarkGray);
                    }
                }
            }

            Widget::render(line, row_area, buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_sidebar_defaults() {
        let sb = SessionSidebar::new();
        assert!(sb.sessions.is_empty());
        assert_eq!(sb.selected, 0);
        assert!(!sb.visible);
    }

    #[test]
    fn toggle_visibility() {
        let mut sb = SessionSidebar::new();
        sb.toggle();
        assert!(sb.visible);
        sb.toggle();
        assert!(!sb.visible);
    }

    #[test]
    fn navigate_sessions() {
        let mut sb = SessionSidebar::new();
        sb.set_sessions(vec![
            SessionEntry::new("1", "Session 1", "now", 5),
            SessionEntry::new("2", "Session 2", "1h ago", 10),
            SessionEntry::new("3", "Session 3", "2h ago", 3),
        ]);

        assert_eq!(sb.selected, 0);
        sb.select_next();
        assert_eq!(sb.selected, 1);
        sb.select_next();
        assert_eq!(sb.selected, 2);
        // Clamp at end
        sb.select_next();
        assert_eq!(sb.selected, 2);
        sb.select_prev();
        assert_eq!(sb.selected, 1);
    }

    #[test]
    fn selected_session_returns_entry() {
        let mut sb = SessionSidebar::new();
        assert!(sb.selected_session().is_none());
        sb.set_sessions(vec![SessionEntry::new("1", "Test", "now", 1)]);
        let entry = sb.selected_session().unwrap();
        assert_eq!(entry.id, "1");
    }

    #[test]
    fn set_sessions_clamps_selection() {
        let mut sb = SessionSidebar::new();
        sb.selected = 5;
        sb.set_sessions(vec![SessionEntry::new("1", "S1", "now", 1)]);
        assert_eq!(sb.selected, 0);
    }

    #[test]
    fn renders_without_panic() {
        let mut sb = SessionSidebar::new();
        sb.visible = true;
        sb.set_sessions(vec![
            SessionEntry::new("1", "Session 1", "now", 5),
            SessionEntry::new("2", "Session 2", "1h ago", 10),
        ]);
        let area = Rect::new(0, 0, 30, 10);
        let mut buf = Buffer::empty(area);
        Widget::render(&sb, area, &mut buf);
    }

    #[test]
    fn hidden_sidebar_does_not_render() {
        let sb = SessionSidebar::new();
        let area = Rect::new(0, 0, 30, 10);
        let mut buf = Buffer::empty(area);
        Widget::render(&sb, area, &mut buf);
        // Should be a no-op when hidden
    }

    #[test]
    fn session_entry_display() {
        let entry = SessionEntry::new("1", "Test Session", "now", 42);
        let line = entry.display_line(false);
        assert!(!line.spans.is_empty());
    }
}
