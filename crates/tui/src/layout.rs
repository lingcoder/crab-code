//! TUI layout — splits the terminal into distinct areas.

use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// Default sidebar width in columns.
pub const DEFAULT_SIDEBAR_WIDTH: u16 = 24;

/// Named areas of the TUI layout.
pub struct AppLayout {
    /// Top bar (title, model name, token count).
    pub top_bar: Rect,
    /// Optional sidebar (session list). `None` when sidebar is hidden.
    pub sidebar: Option<Rect>,
    /// Main content area (conversation messages, tool output).
    pub content: Rect,
    /// Spinner / status line between content and input.
    pub status: Rect,
    /// Text input area at the bottom.
    pub input: Rect,
    /// Bottom status bar (mode, cost, shortcuts).
    pub bottom_bar: Rect,
}

impl AppLayout {
    /// Compute the layout for the given terminal area.
    ///
    /// Layout (top to bottom):
    /// - Top bar: 1 line
    /// - [Sidebar | Content]: fills remaining space (sidebar optional)
    /// - Status line: 1 line (spinner / progress)
    /// - Input: `input_height` lines (minimum 1)
    /// - Bottom bar: 1 line
    #[must_use]
    pub fn compute(area: Rect, input_height: u16) -> Self {
        Self::compute_with_sidebar(area, input_height, false, DEFAULT_SIDEBAR_WIDTH)
    }

    /// Compute layout with optional sidebar panel.
    #[must_use]
    pub fn compute_with_sidebar(
        area: Rect,
        input_height: u16,
        show_sidebar: bool,
        sidebar_width: u16,
    ) -> Self {
        let input_h = input_height.max(1).min(area.height.saturating_sub(4));

        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),       // top bar
                Constraint::Min(1),          // main area (sidebar + content)
                Constraint::Length(1),       // status
                Constraint::Length(input_h), // input
                Constraint::Length(1),       // bottom bar
            ])
            .split(area);

        let (sidebar, content) = if show_sidebar && area.width > sidebar_width + 20 {
            let horizontal = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(sidebar_width), Constraint::Min(1)])
                .split(vertical[1]);
            (Some(horizontal[0]), horizontal[1])
        } else {
            (None, vertical[1])
        };

        Self {
            top_bar: vertical[0],
            sidebar,
            content,
            status: vertical[2],
            input: vertical[3],
            bottom_bar: vertical[4],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_basic_dimensions() {
        let area = Rect::new(0, 0, 120, 40);
        let layout = AppLayout::compute(area, 3);

        assert_eq!(layout.top_bar.height, 1);
        assert_eq!(layout.status.height, 1);
        assert_eq!(layout.input.height, 3);
        assert_eq!(layout.bottom_bar.height, 1);
        // content gets the rest: 40 - 1 - 1 - 3 - 1 = 34
        assert_eq!(layout.content.height, 34);
        assert!(layout.sidebar.is_none());
    }

    #[test]
    fn layout_full_width() {
        let area = Rect::new(0, 0, 80, 24);
        let layout = AppLayout::compute(area, 1);

        assert_eq!(layout.top_bar.width, 80);
        assert_eq!(layout.content.width, 80);
        assert_eq!(layout.status.width, 80);
        assert_eq!(layout.input.width, 80);
        assert_eq!(layout.bottom_bar.width, 80);
    }

    #[test]
    fn layout_input_height_clamped() {
        let area = Rect::new(0, 0, 80, 10);
        // Request 100 lines of input — should be clamped
        let layout = AppLayout::compute(area, 100);
        // max input = 10 - 4 = 6
        assert_eq!(layout.input.height, 6);
    }

    #[test]
    fn layout_minimum_input_height() {
        let area = Rect::new(0, 0, 80, 24);
        let layout = AppLayout::compute(area, 0);
        assert_eq!(layout.input.height, 1);
    }

    #[test]
    fn layout_y_positions_are_contiguous() {
        let area = Rect::new(0, 0, 80, 30);
        let layout = AppLayout::compute(area, 2);

        assert_eq!(layout.top_bar.y, 0);
        assert_eq!(layout.content.y, layout.top_bar.y + layout.top_bar.height);
        assert_eq!(layout.status.y, layout.content.y + layout.content.height);
        assert_eq!(layout.input.y, layout.status.y + layout.status.height);
        assert_eq!(layout.bottom_bar.y, layout.input.y + layout.input.height);
    }

    #[test]
    fn layout_total_height_matches_area() {
        let area = Rect::new(0, 0, 100, 50);
        let layout = AppLayout::compute(area, 4);

        let total = layout.top_bar.height
            + layout.content.height
            + layout.status.height
            + layout.input.height
            + layout.bottom_bar.height;
        assert_eq!(total, area.height);
    }

    #[test]
    fn layout_small_terminal() {
        let area = Rect::new(0, 0, 40, 5);
        let layout = AppLayout::compute(area, 1);
        // 1 + content + 1 + 1 + 1 = 5 => content = 1
        assert_eq!(layout.content.height, 1);
        assert_eq!(layout.input.height, 1);
    }

    #[test]
    fn layout_with_sidebar() {
        let area = Rect::new(0, 0, 120, 40);
        let layout = AppLayout::compute_with_sidebar(area, 3, true, 24);

        assert!(layout.sidebar.is_some());
        let sidebar = layout.sidebar.unwrap();
        assert_eq!(sidebar.width, 24);
        // Content is narrower by sidebar width
        assert_eq!(sidebar.width + layout.content.width, 120);
        // Both have same height (the main area row)
        assert_eq!(sidebar.height, layout.content.height);
    }

    #[test]
    fn layout_sidebar_hidden_when_requested() {
        let area = Rect::new(0, 0, 120, 40);
        let layout = AppLayout::compute_with_sidebar(area, 3, false, 24);
        assert!(layout.sidebar.is_none());
        assert_eq!(layout.content.width, 120);
    }

    #[test]
    fn layout_sidebar_hidden_on_narrow_terminal() {
        // Terminal too narrow for sidebar + 20 cols of content
        let area = Rect::new(0, 0, 40, 24);
        let layout = AppLayout::compute_with_sidebar(area, 1, true, 24);
        // 40 <= 24 + 20, so sidebar should be hidden
        assert!(layout.sidebar.is_none());
        assert_eq!(layout.content.width, 40);
    }

    #[test]
    fn layout_sidebar_y_matches_content() {
        let area = Rect::new(0, 0, 100, 30);
        let layout = AppLayout::compute_with_sidebar(area, 2, true, 24);

        let sidebar = layout.sidebar.unwrap();
        assert_eq!(sidebar.y, layout.content.y);
        assert_eq!(sidebar.height, layout.content.height);
    }

    #[test]
    fn layout_default_sidebar_width() {
        assert_eq!(DEFAULT_SIDEBAR_WIDTH, 24);
    }
}
