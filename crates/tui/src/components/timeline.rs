//! Session timeline — vertical timeline displaying session events with relative timestamps.
//!
//! Shows user messages, assistant responses, tool executions, errors,
//! and system events in chronological order with duration and fold support.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::theme::Theme;

// ─── Types ──────────────────────────────────────────────────────────────

/// The type of event on the timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventType {
    UserMessage,
    AssistantMessage,
    ToolExecution,
    Error,
    SystemEvent,
}

impl std::fmt::Display for EventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UserMessage => write!(f, "User"),
            Self::AssistantMessage => write!(f, "Assistant"),
            Self::ToolExecution => write!(f, "Tool"),
            Self::Error => write!(f, "Error"),
            Self::SystemEvent => write!(f, "System"),
        }
    }
}

impl EventType {
    /// Icon character for this event type.
    #[must_use]
    pub fn icon(&self) -> &'static str {
        match self {
            Self::UserMessage => ">",
            Self::AssistantMessage => "<",
            Self::ToolExecution => "*",
            Self::Error => "!",
            Self::SystemEvent => "#",
        }
    }
}

/// A single entry on the timeline.
#[derive(Debug, Clone)]
pub struct TimelineEntry {
    /// Timestamp in milliseconds since session start.
    pub timestamp_ms: u64,
    /// Type of event.
    pub event_type: EventType,
    /// Display label for the event.
    pub label: String,
    /// Duration in milliseconds (for tool executions, etc.).
    pub duration_ms: Option<u64>,
    /// Optional detail text (tool output, error message, etc.).
    pub detail: Option<String>,
    /// Whether the detail section is folded (collapsed).
    pub folded: bool,
}

impl TimelineEntry {
    /// Create a new timeline entry.
    pub fn new(timestamp_ms: u64, event_type: EventType, label: impl Into<String>) -> Self {
        Self {
            timestamp_ms,
            event_type,
            label: label.into(),
            duration_ms: None,
            detail: None,
            folded: true,
        }
    }

    /// Set the duration.
    #[must_use]
    pub fn with_duration(mut self, duration_ms: u64) -> Self {
        self.duration_ms = Some(duration_ms);
        self
    }

    /// Set the detail text.
    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Set the folded state.
    #[must_use]
    pub fn with_folded(mut self, folded: bool) -> Self {
        self.folded = folded;
        self
    }

    /// Whether this entry has expandable detail.
    #[must_use]
    pub fn has_detail(&self) -> bool {
        self.detail.is_some()
    }

    /// Toggle fold state.
    pub fn toggle_fold(&mut self) {
        self.folded = !self.folded;
    }

    /// Number of visible lines this entry takes up.
    #[must_use]
    pub fn visible_lines(&self) -> usize {
        let mut lines = 1; // main line
        if !self.folded
            && let Some(detail) = &self.detail
        {
            lines += detail.lines().count().max(1);
        }
        lines
    }
}

/// Format milliseconds as a relative time string.
#[must_use]
pub fn format_relative_time(ms: u64) -> String {
    let secs = ms / 1000;
    if secs < 1 {
        return "just now".to_string();
    }
    if secs < 60 {
        return format!("{secs}s ago");
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{mins}m ago");
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("{hours}h ago");
    }
    let days = hours / 24;
    format!("{days}d ago")
}

/// Format milliseconds as a duration string.
#[must_use]
pub fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        return format!("{ms}ms");
    }
    let secs = ms / 1000;
    let remaining_ms = ms % 1000;
    if secs < 60 {
        if remaining_ms > 0 {
            return format!("{secs}.{:01}s", remaining_ms / 100);
        }
        return format!("{secs}s");
    }
    let mins = secs / 60;
    let remaining_secs = secs % 60;
    format!("{mins}m {remaining_secs}s")
}

// ─── Timeline state ─────────────────────────────────────────────────────

/// The timeline state managing entries and navigation.
pub struct Timeline {
    /// All entries in chronological order.
    entries: Vec<TimelineEntry>,
    /// Currently selected entry index.
    selected: usize,
    /// Scroll offset for the view.
    scroll_offset: usize,
    /// Current session time in milliseconds (for relative time calculation).
    current_time_ms: u64,
}

impl Timeline {
    /// Create a new empty timeline.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            selected: 0,
            scroll_offset: 0,
            current_time_ms: 0,
        }
    }

    /// Add an entry to the timeline.
    pub fn push(&mut self, entry: TimelineEntry) {
        self.entries.push(entry);
    }

    /// Number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the timeline is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get all entries.
    #[must_use]
    pub fn entries(&self) -> &[TimelineEntry] {
        &self.entries
    }

    /// Get the selected index.
    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Get the scroll offset.
    #[must_use]
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Get the current session time.
    #[must_use]
    pub fn current_time_ms(&self) -> u64 {
        self.current_time_ms
    }

    /// Set the current session time (for relative timestamps).
    pub fn set_current_time(&mut self, ms: u64) {
        self.current_time_ms = ms;
    }

    /// Move selection up.
    pub fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Move selection down.
    pub fn select_next(&mut self) {
        if !self.entries.is_empty() && self.selected < self.entries.len() - 1 {
            self.selected += 1;
        }
    }

    /// Toggle fold on the selected entry.
    pub fn toggle_fold(&mut self) {
        if let Some(entry) = self.entries.get_mut(self.selected) {
            entry.toggle_fold();
        }
    }

    /// Get the selected entry.
    #[must_use]
    pub fn selected_entry(&self) -> Option<&TimelineEntry> {
        self.entries.get(self.selected)
    }

    /// Total visible lines (accounting for expanded details).
    #[must_use]
    pub fn total_visible_lines(&self) -> usize {
        self.entries.iter().map(TimelineEntry::visible_lines).sum()
    }

    /// Adjust scroll for a given viewport height.
    pub fn adjust_scroll(&mut self, viewport_height: usize) {
        if viewport_height == 0 {
            return;
        }
        // Calculate the line offset of the selected entry
        let mut line_offset = 0;
        for (i, entry) in self.entries.iter().enumerate() {
            if i == self.selected {
                break;
            }
            line_offset += entry.visible_lines();
        }
        if line_offset < self.scroll_offset {
            self.scroll_offset = line_offset;
        } else if line_offset >= self.scroll_offset + viewport_height {
            self.scroll_offset = line_offset - viewport_height + 1;
        }
    }

    /// Clear all entries.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.selected = 0;
        self.scroll_offset = 0;
    }
}

impl Default for Timeline {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Widget ─────────────────────────────────────────────────────────────

/// Widget for rendering the timeline.
pub struct TimelineWidget<'a> {
    timeline: &'a Timeline,
    theme: &'a Theme,
}

impl<'a> TimelineWidget<'a> {
    #[must_use]
    pub fn new(timeline: &'a Timeline, theme: &'a Theme) -> Self {
        Self { timeline, theme }
    }

    /// Get the color for an event type.
    fn event_color(&self, event_type: EventType) -> Color {
        match event_type {
            EventType::UserMessage => self.theme.heading,
            EventType::AssistantMessage => self.theme.success,
            EventType::ToolExecution => self.theme.warning,
            EventType::Error => self.theme.error,
            EventType::SystemEvent => self.theme.muted,
        }
    }
}

impl Widget for TimelineWidget<'_> {
    #[allow(clippy::cast_possible_truncation)]
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width < 15 {
            return;
        }

        let now = self.timeline.current_time_ms();
        let scroll = self.timeline.scroll_offset();
        let mut current_line: usize = 0;
        let visible_height = area.height as usize;

        for (idx, entry) in self.timeline.entries().iter().enumerate() {
            let entry_lines = entry.visible_lines();

            // Skip entries above scroll
            if current_line + entry_lines <= scroll {
                current_line += entry_lines;
                continue;
            }

            // Stop if we're past the visible area
            if current_line >= scroll + visible_height {
                break;
            }

            let is_selected = idx == self.timeline.selected();
            let event_color = self.event_color(entry.event_type);

            // ─── Main line ───
            let vis_y = current_line.saturating_sub(scroll);
            if vis_y < visible_height {
                let y = area.y + vis_y as u16;
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

                // Build the line: [time] icon label (duration)
                let relative = format_relative_time(now.saturating_sub(entry.timestamp_ms));
                let time_str = format!("{relative:>8}");

                let fold_indicator = if entry.has_detail() {
                    if entry.folded { "+ " } else { "- " }
                } else {
                    "  "
                };

                let mut spans = vec![
                    Span::styled(&time_str, Style::default().fg(self.theme.muted).bg(bg)),
                    Span::styled(" | ", Style::default().fg(self.theme.border).bg(bg)),
                    Span::styled(
                        entry.event_type.icon(),
                        Style::default()
                            .fg(event_color)
                            .bg(bg)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" ", Style::default().bg(bg)),
                    Span::styled(fold_indicator, Style::default().fg(self.theme.muted).bg(bg)),
                    Span::styled(&entry.label, Style::default().fg(self.theme.fg).bg(bg)),
                ];

                if let Some(dur) = entry.duration_ms {
                    spans.push(Span::styled(
                        format!(" ({})", format_duration(dur)),
                        Style::default().fg(self.theme.muted).bg(bg),
                    ));
                }

                let line = Line::from(spans);
                Widget::render(line, Rect::new(area.x, y, area.width, 1), buf);
            }

            // ─── Detail lines ───
            if !entry.folded
                && let Some(detail) = &entry.detail
            {
                for (di, detail_line) in detail.lines().enumerate() {
                    let detail_vis_y = current_line + 1 + di;
                    let detail_vis_y = detail_vis_y.saturating_sub(scroll);
                    if detail_vis_y >= visible_height {
                        break;
                    }
                    let y = area.y + detail_vis_y as u16;

                    // Clear
                    for x in area.x..area.x + area.width {
                        if let Some(cell) = buf.cell_mut((x, y)) {
                            cell.set_char(' ');
                            cell.set_style(Style::default().bg(self.theme.bg));
                        }
                    }

                    let indent = "           | ";
                    let detail_style = Style::default().fg(self.theme.muted);
                    let line = Line::from(vec![
                        Span::styled(indent, Style::default().fg(self.theme.border)),
                        Span::styled(detail_line, detail_style),
                    ]);
                    Widget::render(line, Rect::new(area.x, y, area.width, 1), buf);
                }
            }

            current_line += entry_lines;
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_type_display() {
        assert_eq!(EventType::UserMessage.to_string(), "User");
        assert_eq!(EventType::AssistantMessage.to_string(), "Assistant");
        assert_eq!(EventType::ToolExecution.to_string(), "Tool");
        assert_eq!(EventType::Error.to_string(), "Error");
        assert_eq!(EventType::SystemEvent.to_string(), "System");
    }

    #[test]
    fn event_type_icon() {
        assert_eq!(EventType::UserMessage.icon(), ">");
        assert_eq!(EventType::Error.icon(), "!");
    }

    #[test]
    fn timeline_entry_new() {
        let entry = TimelineEntry::new(1000, EventType::UserMessage, "Hello");
        assert_eq!(entry.timestamp_ms, 1000);
        assert_eq!(entry.event_type, EventType::UserMessage);
        assert_eq!(entry.label, "Hello");
        assert!(entry.duration_ms.is_none());
        assert!(entry.detail.is_none());
        assert!(entry.folded);
    }

    #[test]
    fn timeline_entry_builders() {
        let entry = TimelineEntry::new(0, EventType::ToolExecution, "bash")
            .with_duration(1500)
            .with_detail("ls -la\ntotal 42")
            .with_folded(false);
        assert_eq!(entry.duration_ms, Some(1500));
        assert!(entry.has_detail());
        assert!(!entry.folded);
    }

    #[test]
    fn timeline_entry_visible_lines_folded() {
        let entry = TimelineEntry::new(0, EventType::ToolExecution, "test")
            .with_detail("line1\nline2\nline3");
        assert_eq!(entry.visible_lines(), 1); // folded = only main line
    }

    #[test]
    fn timeline_entry_visible_lines_unfolded() {
        let entry = TimelineEntry::new(0, EventType::ToolExecution, "test")
            .with_detail("line1\nline2\nline3")
            .with_folded(false);
        assert_eq!(entry.visible_lines(), 4); // 1 main + 3 detail
    }

    #[test]
    fn timeline_entry_visible_lines_no_detail() {
        let entry = TimelineEntry::new(0, EventType::UserMessage, "test").with_folded(false);
        assert_eq!(entry.visible_lines(), 1);
    }

    #[test]
    fn timeline_entry_toggle_fold() {
        let mut entry =
            TimelineEntry::new(0, EventType::ToolExecution, "test").with_detail("detail");
        assert!(entry.folded);
        entry.toggle_fold();
        assert!(!entry.folded);
        entry.toggle_fold();
        assert!(entry.folded);
    }

    #[test]
    fn format_relative_time_just_now() {
        assert_eq!(format_relative_time(500), "just now");
        assert_eq!(format_relative_time(0), "just now");
    }

    #[test]
    fn format_relative_time_seconds() {
        assert_eq!(format_relative_time(5000), "5s ago");
        assert_eq!(format_relative_time(30_000), "30s ago");
    }

    #[test]
    fn format_relative_time_minutes() {
        assert_eq!(format_relative_time(60_000), "1m ago");
        assert_eq!(format_relative_time(300_000), "5m ago");
    }

    #[test]
    fn format_relative_time_hours() {
        assert_eq!(format_relative_time(3_600_000), "1h ago");
        assert_eq!(format_relative_time(7_200_000), "2h ago");
    }

    #[test]
    fn format_relative_time_days() {
        assert_eq!(format_relative_time(86_400_000), "1d ago");
    }

    #[test]
    fn format_duration_ms() {
        assert_eq!(format_duration(500), "500ms");
        assert_eq!(format_duration(0), "0ms");
    }

    #[test]
    fn format_duration_seconds() {
        assert_eq!(format_duration(1000), "1s");
        assert_eq!(format_duration(2500), "2.5s");
        assert_eq!(format_duration(10_000), "10s");
    }

    #[test]
    fn format_duration_minutes() {
        assert_eq!(format_duration(60_000), "1m 0s");
        assert_eq!(format_duration(90_000), "1m 30s");
        assert_eq!(format_duration(150_000), "2m 30s");
    }

    #[test]
    fn timeline_new() {
        let tl = Timeline::new();
        assert!(tl.is_empty());
        assert_eq!(tl.len(), 0);
        assert_eq!(tl.selected(), 0);
        assert_eq!(tl.current_time_ms(), 0);
    }

    #[test]
    fn timeline_default() {
        let tl = Timeline::default();
        assert!(tl.is_empty());
    }

    #[test]
    fn timeline_push() {
        let mut tl = Timeline::new();
        tl.push(TimelineEntry::new(0, EventType::UserMessage, "Hello"));
        tl.push(TimelineEntry::new(1000, EventType::AssistantMessage, "Hi"));
        assert_eq!(tl.len(), 2);
        assert!(!tl.is_empty());
    }

    #[test]
    fn timeline_navigation() {
        let mut tl = Timeline::new();
        tl.push(TimelineEntry::new(0, EventType::UserMessage, "A"));
        tl.push(TimelineEntry::new(1000, EventType::AssistantMessage, "B"));
        tl.push(TimelineEntry::new(2000, EventType::ToolExecution, "C"));

        assert_eq!(tl.selected(), 0);
        tl.select_next();
        assert_eq!(tl.selected(), 1);
        tl.select_next();
        assert_eq!(tl.selected(), 2);
        tl.select_next(); // clamp
        assert_eq!(tl.selected(), 2);

        tl.select_prev();
        assert_eq!(tl.selected(), 1);
        tl.select_prev();
        assert_eq!(tl.selected(), 0);
        tl.select_prev(); // clamp
        assert_eq!(tl.selected(), 0);
    }

    #[test]
    fn timeline_toggle_fold() {
        let mut tl = Timeline::new();
        tl.push(TimelineEntry::new(0, EventType::ToolExecution, "bash").with_detail("output"));
        assert!(tl.selected_entry().unwrap().folded);

        tl.toggle_fold();
        assert!(!tl.entries()[0].folded);

        tl.toggle_fold();
        assert!(tl.entries()[0].folded);
    }

    #[test]
    fn timeline_total_visible_lines() {
        let mut tl = Timeline::new();
        tl.push(TimelineEntry::new(0, EventType::UserMessage, "A"));
        tl.push(
            TimelineEntry::new(1000, EventType::ToolExecution, "B")
                .with_detail("line1\nline2")
                .with_folded(false),
        );
        // A = 1 line, B = 1 + 2 = 3 lines
        assert_eq!(tl.total_visible_lines(), 4);
    }

    #[test]
    fn timeline_set_current_time() {
        let mut tl = Timeline::new();
        tl.set_current_time(5000);
        assert_eq!(tl.current_time_ms(), 5000);
    }

    #[test]
    fn timeline_clear() {
        let mut tl = Timeline::new();
        tl.push(TimelineEntry::new(0, EventType::UserMessage, "A"));
        tl.select_next();
        tl.clear();
        assert!(tl.is_empty());
        assert_eq!(tl.selected(), 0);
    }

    #[test]
    fn timeline_adjust_scroll() {
        let mut tl = Timeline::new();
        for i in 0..20 {
            tl.push(TimelineEntry::new(
                i * 1000,
                EventType::UserMessage,
                format!("msg {i}"),
            ));
        }
        // Select entry 15
        for _ in 0..15 {
            tl.select_next();
        }
        tl.adjust_scroll(5);
        // Selected (15) should be within scroll_offset..scroll_offset+5
        assert!(tl.scroll_offset() <= 15);
        assert!(tl.scroll_offset() + 5 > 15);
    }

    #[test]
    fn timeline_selected_entry() {
        let mut tl = Timeline::new();
        assert!(tl.selected_entry().is_none());

        tl.push(TimelineEntry::new(0, EventType::UserMessage, "first"));
        assert_eq!(tl.selected_entry().unwrap().label, "first");
    }

    #[test]
    fn widget_renders() {
        let mut tl = Timeline::new();
        tl.set_current_time(10_000);
        tl.push(TimelineEntry::new(0, EventType::UserMessage, "Hello world"));
        tl.push(TimelineEntry::new(5000, EventType::ToolExecution, "bash ls").with_duration(1200));

        let theme = Theme::dark();
        let widget = TimelineWidget::new(&tl, &theme);
        let area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);

        let row0: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(row0.contains("Hello world"));

        let row1: String = (0..area.width)
            .map(|x| buf.cell((x, 1)).unwrap().symbol().to_string())
            .collect();
        assert!(row1.contains("bash ls"));
    }

    #[test]
    fn widget_renders_empty() {
        let tl = Timeline::new();
        let theme = Theme::dark();
        let widget = TimelineWidget::new(&tl, &theme);
        let area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);
    }

    #[test]
    fn widget_renders_with_detail() {
        let mut tl = Timeline::new();
        tl.set_current_time(5000);
        tl.push(
            TimelineEntry::new(0, EventType::ToolExecution, "bash")
                .with_detail("output line 1\noutput line 2")
                .with_folded(false),
        );

        let theme = Theme::dark();
        let widget = TimelineWidget::new(&tl, &theme);
        let area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);

        // Detail should appear on lines 1 and 2
        let row1: String = (0..area.width)
            .map(|x| buf.cell((x, 1)).unwrap().symbol().to_string())
            .collect();
        assert!(row1.contains("output line 1"));
    }

    #[test]
    fn widget_small_area() {
        let mut tl = Timeline::new();
        tl.push(TimelineEntry::new(0, EventType::UserMessage, "test"));
        let theme = Theme::dark();
        let widget = TimelineWidget::new(&tl, &theme);
        let area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(area);
        // Should not panic even with narrow area
        Widget::render(widget, area, &mut buf);
    }
}
