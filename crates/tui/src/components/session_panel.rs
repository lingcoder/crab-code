//! Session list sidebar panel for multi-session management.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Widget};

/// An entry in the session list.
#[derive(Debug, Clone)]
pub struct SessionEntry {
    /// Session identifier.
    pub id: String,
    /// Short display label (truncated ID or user-given name).
    pub label: String,
    /// Whether this session is the currently active one.
    pub active: bool,
    /// Number of messages in this session.
    pub message_count: usize,
}

impl SessionEntry {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            active: false,
            message_count: 0,
        }
    }

    #[must_use]
    pub fn with_active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    #[must_use]
    pub fn with_message_count(mut self, count: usize) -> Self {
        self.message_count = count;
        self
    }
}

/// The session sidebar panel state.
pub struct SessionPanel {
    sessions: Vec<SessionEntry>,
    selected: usize,
    /// Scroll offset for long session lists.
    scroll_offset: usize,
}

impl SessionPanel {
    #[must_use]
    pub fn new() -> Self {
        Self {
            sessions: Vec::new(),
            selected: 0,
            scroll_offset: 0,
        }
    }

    /// Replace the session list.
    pub fn set_sessions(&mut self, sessions: Vec<SessionEntry>) {
        self.sessions = sessions;
        if self.selected >= self.sessions.len() && !self.sessions.is_empty() {
            self.selected = self.sessions.len() - 1;
        }
    }

    /// Add a session entry.
    pub fn push(&mut self, entry: SessionEntry) {
        self.sessions.push(entry);
    }

    /// Number of sessions.
    #[must_use]
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// Whether empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    /// Currently selected index.
    #[must_use]
    pub const fn selected(&self) -> usize {
        self.selected
    }

    /// Get the session entry at the selected index.
    #[must_use]
    pub fn selected_entry(&self) -> Option<&SessionEntry> {
        self.sessions.get(self.selected)
    }

    /// Move selection up.
    pub fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            if self.selected < self.scroll_offset {
                self.scroll_offset = self.selected;
            }
        }
    }

    /// Move selection down.
    pub fn select_next(&mut self) {
        if !self.sessions.is_empty() && self.selected < self.sessions.len() - 1 {
            self.selected += 1;
        }
    }

    /// Select a session by its ID. Returns true if found.
    pub fn select_by_id(&mut self, id: &str) -> bool {
        if let Some(idx) = self.sessions.iter().position(|s| s.id == id) {
            self.selected = idx;
            true
        } else {
            false
        }
    }

    /// Mark a session as active (and deactivate all others).
    pub fn set_active(&mut self, id: &str) {
        for session in &mut self.sessions {
            session.active = session.id == id;
        }
    }

    /// Adjust scroll for a given visible height.
    pub fn adjust_scroll(&mut self, visible_height: usize) {
        if visible_height == 0 {
            return;
        }
        if self.selected >= self.scroll_offset + visible_height {
            self.scroll_offset = self.selected - visible_height + 1;
        }
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        }
    }
}

impl Default for SessionPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for &SessionPanel {
    #[allow(clippy::cast_possible_truncation)]
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .title(" Sessions ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));

        let inner = block.inner(area);
        Widget::render(block, area, buf);

        if inner.height == 0 || inner.width == 0 {
            return;
        }

        if self.sessions.is_empty() {
            let msg = Line::from(Span::styled(
                "No sessions",
                Style::default().fg(Color::DarkGray),
            ));
            Widget::render(msg, inner, buf);
            return;
        }

        // Compute scroll
        let visible = inner.height as usize;
        let scroll = if self.selected >= self.scroll_offset + visible {
            self.selected - visible + 1
        } else if self.selected < self.scroll_offset {
            self.selected
        } else {
            self.scroll_offset
        };

        for (i, session) in self.sessions.iter().skip(scroll).take(visible).enumerate() {
            let y = inner.y + i as u16;
            let is_selected = scroll + i == self.selected;

            let indicator = if session.active {
                "●"
            } else if is_selected {
                "▸"
            } else {
                " "
            };

            let label_style = if is_selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else if session.active {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::White)
            };

            // Truncate label to fit
            let max_label_width = (inner.width as usize).saturating_sub(5);
            let label = if session.label.len() > max_label_width {
                format!("{}…", &session.label[..max_label_width.saturating_sub(1)])
            } else {
                session.label.clone()
            };

            let count_str = if session.message_count > 0 {
                format!(" {}", session.message_count)
            } else {
                String::new()
            };

            let line = Line::from(vec![
                Span::styled(format!("{indicator} "), label_style),
                Span::styled(label, label_style),
                Span::styled(count_str, Style::default().fg(Color::DarkGray)),
            ]);

            let line_area = Rect {
                x: inner.x,
                y,
                width: inner.width,
                height: 1,
            };
            Widget::render(line, line_area, buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_sessions() -> Vec<SessionEntry> {
        vec![
            SessionEntry::new("sess_001", "Session 1")
                .with_active(true)
                .with_message_count(5),
            SessionEntry::new("sess_002", "Session 2").with_message_count(12),
            SessionEntry::new("sess_003", "Session 3"),
        ]
    }

    #[test]
    fn new_is_empty() {
        let panel = SessionPanel::new();
        assert!(panel.is_empty());
        assert_eq!(panel.len(), 0);
        assert_eq!(panel.selected(), 0);
        assert!(panel.selected_entry().is_none());
    }

    #[test]
    fn set_sessions() {
        let mut panel = SessionPanel::new();
        panel.set_sessions(sample_sessions());
        assert_eq!(panel.len(), 3);
        assert!(!panel.is_empty());
        assert_eq!(panel.selected(), 0);
    }

    #[test]
    fn push_adds_entry() {
        let mut panel = SessionPanel::new();
        panel.push(SessionEntry::new("s1", "First"));
        panel.push(SessionEntry::new("s2", "Second"));
        assert_eq!(panel.len(), 2);
    }

    #[test]
    fn select_next_prev() {
        let mut panel = SessionPanel::new();
        panel.set_sessions(sample_sessions());

        panel.select_next();
        assert_eq!(panel.selected(), 1);

        panel.select_next();
        assert_eq!(panel.selected(), 2);

        // Stops at end
        panel.select_next();
        assert_eq!(panel.selected(), 2);

        panel.select_prev();
        assert_eq!(panel.selected(), 1);

        panel.select_prev();
        assert_eq!(panel.selected(), 0);

        // Stops at start
        panel.select_prev();
        assert_eq!(panel.selected(), 0);
    }

    #[test]
    fn select_by_id() {
        let mut panel = SessionPanel::new();
        panel.set_sessions(sample_sessions());

        assert!(panel.select_by_id("sess_002"));
        assert_eq!(panel.selected(), 1);

        assert!(!panel.select_by_id("nonexistent"));
        assert_eq!(panel.selected(), 1); // unchanged
    }

    #[test]
    fn set_active_marks_correct_session() {
        let mut panel = SessionPanel::new();
        panel.set_sessions(sample_sessions());

        panel.set_active("sess_002");
        assert!(!panel.sessions[0].active);
        assert!(panel.sessions[1].active);
        assert!(!panel.sessions[2].active);
    }

    #[test]
    fn selected_entry_returns_correct() {
        let mut panel = SessionPanel::new();
        panel.set_sessions(sample_sessions());

        let entry = panel.selected_entry().unwrap();
        assert_eq!(entry.id, "sess_001");

        panel.select_next();
        let entry = panel.selected_entry().unwrap();
        assert_eq!(entry.id, "sess_002");
    }

    #[test]
    fn selected_clamps_on_set_sessions() {
        let mut panel = SessionPanel::new();
        panel.set_sessions(sample_sessions());
        panel.select_next();
        panel.select_next();
        assert_eq!(panel.selected(), 2);

        // Shrink list
        panel.set_sessions(vec![SessionEntry::new("s1", "Only one")]);
        assert_eq!(panel.selected(), 0);
    }

    #[test]
    fn session_entry_builder() {
        let entry = SessionEntry::new("id", "label")
            .with_active(true)
            .with_message_count(42);
        assert_eq!(entry.id, "id");
        assert_eq!(entry.label, "label");
        assert!(entry.active);
        assert_eq!(entry.message_count, 42);
    }

    #[test]
    fn default_panel() {
        let panel = SessionPanel::default();
        assert!(panel.is_empty());
    }

    #[test]
    fn renders_without_panic() {
        let mut panel = SessionPanel::new();
        panel.set_sessions(sample_sessions());

        let area = Rect::new(0, 0, 25, 10);
        let mut buf = Buffer::empty(area);
        Widget::render(&panel, area, &mut buf);

        let buf_ref = &buf;
        let all_text: String = (0..area.height)
            .flat_map(|y| {
                (0..area.width).map(move |x| buf_ref.cell((x, y)).unwrap().symbol().to_string())
            })
            .collect();
        assert!(all_text.contains("Sessions"));
        assert!(all_text.contains("Session 1"));
    }

    #[test]
    fn renders_empty_state() {
        let panel = SessionPanel::new();
        let area = Rect::new(0, 0, 25, 5);
        let mut buf = Buffer::empty(area);
        Widget::render(&panel, area, &mut buf);

        let buf_ref = &buf;
        let all_text: String = (0..area.height)
            .flat_map(|y| {
                (0..area.width).map(move |x| buf_ref.cell((x, y)).unwrap().symbol().to_string())
            })
            .collect();
        assert!(all_text.contains("No sessions"));
    }

    #[test]
    fn adjust_scroll_basic() {
        let mut panel = SessionPanel::new();
        let mut entries = Vec::new();
        for i in 0..20 {
            entries.push(SessionEntry::new(format!("s{i}"), format!("Session {i}")));
        }
        panel.set_sessions(entries);

        // Select near bottom
        for _ in 0..15 {
            panel.select_next();
        }
        panel.adjust_scroll(5);
        assert!(panel.scroll_offset > 0);
    }
}
