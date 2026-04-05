//! Collapsible tool output display.
//!
//! Long tool outputs are collapsed by default with a summary line.
//! The user can toggle expansion with Enter.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

/// Maximum number of lines to show before collapsing.
const COLLAPSE_THRESHOLD: usize = 5;
/// Number of preview lines to show when collapsed.
const PREVIEW_LINES: usize = 3;

/// A single tool output entry with collapse state.
#[derive(Debug, Clone)]
pub struct ToolOutputEntry {
    /// Tool name.
    pub tool_name: String,
    /// Full output text.
    pub output: String,
    /// Whether this output is an error.
    pub is_error: bool,
    /// Whether the output is currently expanded.
    pub expanded: bool,
}

impl ToolOutputEntry {
    /// Create a new tool output entry.
    pub fn new(tool_name: impl Into<String>, output: impl Into<String>, is_error: bool) -> Self {
        Self {
            tool_name: tool_name.into(),
            output: output.into(),
            is_error,
            expanded: false,
        }
    }

    /// Whether this entry should be collapsible (output exceeds threshold).
    #[must_use]
    pub fn is_collapsible(&self) -> bool {
        self.output.lines().count() > COLLAPSE_THRESHOLD
    }

    /// Toggle expanded/collapsed state.
    pub fn toggle(&mut self) {
        self.expanded = !self.expanded;
    }

    /// Get the lines to display based on current state.
    #[must_use]
    pub fn visible_lines(&self) -> Vec<&str> {
        let lines: Vec<&str> = self.output.lines().collect();
        if !self.is_collapsible() || self.expanded {
            lines
        } else {
            lines.into_iter().take(PREVIEW_LINES).collect()
        }
    }

    /// Number of lines this entry occupies when rendered.
    /// Includes header line and optional "... N more lines" footer.
    #[must_use]
    pub fn render_height(&self) -> usize {
        let content_lines = self.visible_lines().len();
        let header = 1; // "[tool_name] ..." header
        let footer = usize::from(self.is_collapsible() && !self.expanded);
        header + content_lines + footer
    }
}

/// Manages a list of tool outputs with selection for fold/unfold.
#[derive(Debug, Clone)]
pub struct ToolOutputList {
    /// All tool output entries.
    entries: Vec<ToolOutputEntry>,
    /// Currently selected entry index (for toggle).
    selected: Option<usize>,
}

impl ToolOutputList {
    /// Create an empty list.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            selected: None,
        }
    }

    /// Add a tool output entry.
    pub fn push(&mut self, entry: ToolOutputEntry) {
        self.entries.push(entry);
        // Auto-select the latest entry
        self.selected = Some(self.entries.len() - 1);
    }

    /// Get all entries.
    #[must_use]
    pub fn entries(&self) -> &[ToolOutputEntry] {
        &self.entries
    }

    /// Get mutable reference to all entries.
    pub fn entries_mut(&mut self) -> &mut [ToolOutputEntry] {
        &mut self.entries
    }

    /// Number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the list is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Currently selected index.
    #[must_use]
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// Select the next collapsible entry.
    pub fn select_next(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        let start = self.selected.map_or(0, |i| i + 1);
        for i in start..self.entries.len() {
            if self.entries[i].is_collapsible() {
                self.selected = Some(i);
                return;
            }
        }
    }

    /// Select the previous collapsible entry.
    pub fn select_prev(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        let start = self.selected.unwrap_or(self.entries.len());
        for i in (0..start).rev() {
            if self.entries[i].is_collapsible() {
                self.selected = Some(i);
                return;
            }
        }
    }

    /// Toggle the currently selected entry.
    pub fn toggle_selected(&mut self) {
        if let Some(idx) = self.selected
            && let Some(entry) = self.entries.get_mut(idx)
        {
            entry.toggle();
        }
    }

    /// Clear all entries.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.selected = None;
    }
}

impl Default for ToolOutputList {
    fn default() -> Self {
        Self::new()
    }
}

/// Render a single tool output entry.
pub fn render_tool_output(
    entry: &ToolOutputEntry,
    is_selected: bool,
    area: Rect,
    buf: &mut Buffer,
) {
    if area.height == 0 {
        return;
    }

    let mut y = area.y;

    // Header line
    let header_style = if entry.is_error {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::Blue)
            .add_modifier(Modifier::BOLD)
    };

    let selection_indicator = if is_selected { ">" } else { " " };
    let collapse_indicator = if entry.is_collapsible() {
        if entry.expanded { "[-]" } else { "[+]" }
    } else {
        "   "
    };

    let prefix = if entry.is_error {
        "tool error"
    } else {
        &entry.tool_name
    };

    let header = Line::from(vec![
        Span::styled(selection_indicator, Style::default().fg(Color::Yellow)),
        Span::raw(" "),
        Span::styled(collapse_indicator, Style::default().fg(Color::DarkGray)),
        Span::raw(" "),
        Span::styled(format!("[{prefix}]"), header_style),
    ]);

    let header_area = Rect {
        x: area.x,
        y,
        width: area.width,
        height: 1,
    };
    Widget::render(header, header_area, buf);
    y += 1;

    // Content lines
    let visible = entry.visible_lines();
    for line_text in &visible {
        if y >= area.y + area.height {
            break;
        }
        let content_style = if entry.is_error {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::Gray)
        };
        let line = Line::from(Span::styled(format!("  {line_text}"), content_style));
        let line_area = Rect {
            x: area.x,
            y,
            width: area.width,
            height: 1,
        };
        Widget::render(line, line_area, buf);
        y += 1;
    }

    // Footer (collapsed indicator)
    if entry.is_collapsible() && !entry.expanded && y < area.y + area.height {
        let total_lines = entry.output.lines().count();
        let hidden = total_lines.saturating_sub(PREVIEW_LINES);
        let footer = Line::from(Span::styled(
            format!("  ... {hidden} more lines (Enter to expand)"),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        ));
        let footer_area = Rect {
            x: area.x,
            y,
            width: area.width,
            height: 1,
        };
        Widget::render(footer, footer_area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_short_output_not_collapsible() {
        let entry = ToolOutputEntry::new("bash", "line1\nline2\nline3", false);
        assert!(!entry.is_collapsible());
        assert_eq!(entry.visible_lines().len(), 3);
    }

    #[test]
    fn entry_long_output_is_collapsible() {
        let output = (0..20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let entry = ToolOutputEntry::new("bash", output, false);
        assert!(entry.is_collapsible());
        // Collapsed: shows only PREVIEW_LINES
        assert_eq!(entry.visible_lines().len(), PREVIEW_LINES);
    }

    #[test]
    fn entry_toggle_expands() {
        let output = (0..20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut entry = ToolOutputEntry::new("bash", output, false);
        assert!(!entry.expanded);
        entry.toggle();
        assert!(entry.expanded);
        assert_eq!(entry.visible_lines().len(), 20);
        entry.toggle();
        assert!(!entry.expanded);
        assert_eq!(entry.visible_lines().len(), PREVIEW_LINES);
    }

    #[test]
    fn entry_render_height_short() {
        let entry = ToolOutputEntry::new("read", "ok", false);
        // header(1) + content(1) + no footer = 2
        assert_eq!(entry.render_height(), 2);
    }

    #[test]
    fn entry_render_height_collapsible() {
        let output = (0..20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let entry = ToolOutputEntry::new("bash", output, false);
        // header(1) + preview(3) + footer(1) = 5
        assert_eq!(entry.render_height(), 1 + PREVIEW_LINES + 1);
    }

    #[test]
    fn entry_render_height_expanded() {
        let output = (0..20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut entry = ToolOutputEntry::new("bash", output, false);
        entry.toggle();
        // header(1) + all 20 lines + no footer = 21
        assert_eq!(entry.render_height(), 21);
    }

    #[test]
    fn entry_error_flag() {
        let entry = ToolOutputEntry::new("bash", "command not found", true);
        assert!(entry.is_error);
    }

    #[test]
    fn list_new_is_empty() {
        let list = ToolOutputList::new();
        assert!(list.is_empty());
        assert_eq!(list.len(), 0);
        assert!(list.selected().is_none());
    }

    #[test]
    fn list_push_and_auto_select() {
        let mut list = ToolOutputList::new();
        list.push(ToolOutputEntry::new("bash", "ok", false));
        assert_eq!(list.len(), 1);
        assert_eq!(list.selected(), Some(0));
    }

    #[test]
    fn list_toggle_selected() {
        let output = (0..20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut list = ToolOutputList::new();
        list.push(ToolOutputEntry::new("bash", output, false));
        assert!(!list.entries()[0].expanded);
        list.toggle_selected();
        assert!(list.entries()[0].expanded);
    }

    #[test]
    fn list_select_next_prev() {
        let short = "ok";
        let long = (0..20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");

        let mut list = ToolOutputList::new();
        list.push(ToolOutputEntry::new("read", short, false)); // idx 0, not collapsible
        list.push(ToolOutputEntry::new("bash", long.clone(), false)); // idx 1, collapsible
        list.push(ToolOutputEntry::new("grep", long, false)); // idx 2, collapsible

        list.selected = None;
        list.select_next();
        assert_eq!(list.selected(), Some(1)); // first collapsible

        list.select_next();
        assert_eq!(list.selected(), Some(2)); // second collapsible

        list.select_prev();
        assert_eq!(list.selected(), Some(1)); // back to first
    }

    #[test]
    fn list_clear() {
        let mut list = ToolOutputList::new();
        list.push(ToolOutputEntry::new("bash", "ok", false));
        list.clear();
        assert!(list.is_empty());
        assert!(list.selected().is_none());
    }

    #[test]
    fn render_tool_output_no_panic() {
        let entry = ToolOutputEntry::new("bash", "hello\nworld", false);
        let area = Rect::new(0, 0, 80, 10);
        let mut buf = Buffer::empty(area);
        render_tool_output(&entry, false, area, &mut buf);
    }

    #[test]
    fn render_tool_output_selected() {
        let entry = ToolOutputEntry::new("bash", "hello", false);
        let area = Rect::new(0, 0, 80, 5);
        let mut buf = Buffer::empty(area);
        render_tool_output(&entry, true, area, &mut buf);

        // Check selection indicator ">" is present
        let first_cell = buf.cell((0, 0)).unwrap().symbol().to_string();
        assert_eq!(first_cell, ">");
    }

    #[test]
    fn render_tool_output_error() {
        let entry = ToolOutputEntry::new("bash", "not found", true);
        let area = Rect::new(0, 0, 80, 5);
        let mut buf = Buffer::empty(area);
        render_tool_output(&entry, false, area, &mut buf);
    }

    #[test]
    fn render_tool_output_collapsed_footer() {
        let output = (0..20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let entry = ToolOutputEntry::new("bash", output, false);
        let area = Rect::new(0, 0, 80, 10);
        let mut buf = Buffer::empty(area);
        render_tool_output(&entry, false, area, &mut buf);

        // Footer should be at y = 1 (header) + 3 (preview) = 4
        let buf_ref = &buf;
        let footer_row: String = (0..area.width)
            .map(|x| buf_ref.cell((x, 4)).unwrap().symbol().to_string())
            .collect();
        assert!(footer_row.contains("more lines"));
    }

    #[test]
    fn render_zero_height_no_panic() {
        let entry = ToolOutputEntry::new("bash", "hello", false);
        let area = Rect::new(0, 0, 80, 0);
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 1));
        render_tool_output(&entry, false, area, &mut buf);
    }

    #[test]
    fn default_list() {
        let list = ToolOutputList::default();
        assert!(list.is_empty());
    }
}
