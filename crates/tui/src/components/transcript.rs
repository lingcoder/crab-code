//! Transcript cells and virtual scroller — renders only visible messages.
//!
//! Each message becomes a `HistoryCell` that knows how to measure and render
//! itself. The `VirtualScroller` maintains a height cache and only renders
//! cells within the viewport.

use std::collections::HashMap;
use std::sync::Arc;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::app::ChatMessage;
use crate::components::text_utils::strip_trailing_tool_json;
use crate::traits::Renderable;

/// Terra cotta color.
const CRAB_COLOR: Color = Color::Rgb(218, 119, 86);

/// A single unit in the transcript (user message, assistant response, tool call, etc.).
pub trait HistoryCell: Send + Sync {
    /// Unique identifier for this cell.
    fn id(&self) -> &str;

    /// Render this cell into styled lines for the given width.
    fn display_lines(&self, width: u16) -> Vec<Line<'static>>;

    /// Height in rows for the given terminal width.
    fn height(&self, width: u16) -> u16 {
        #[allow(clippy::cast_possible_truncation)]
        let h = self.display_lines(width).len() as u16;
        h
    }

    /// Whether this cell is currently being streamed (active).
    fn is_active(&self) -> bool {
        false
    }
}

// ─── Concrete cell implementations ──────────────────────────────────

/// User input cell.
pub struct UserCell {
    pub id: String,
    pub text: String,
}

impl HistoryCell for UserCell {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        vec![
            Line::from(vec![
                Span::styled(
                    "❯ ",
                    Style::default().fg(CRAB_COLOR).add_modifier(Modifier::BOLD),
                ),
                Span::styled(self.text.clone(), Style::default().fg(Color::White)),
            ]),
            Line::default(),
        ]
    }
}

/// Assistant response cell — supports streaming animation.
pub struct AssistantCell {
    pub id: String,
    pub text: String,
    /// Characters revealed so far (for streaming animation).
    /// When `revealed_chars >= text.len()`, animation is complete.
    pub revealed_chars: usize,
    /// Whether this cell is actively streaming.
    pub active: bool,
}

impl AssistantCell {
    /// Advance the reveal animation by `n` characters.
    pub fn advance_reveal(&mut self, n: usize) {
        self.revealed_chars = (self.revealed_chars + n).min(self.text.len());
    }

    /// Whether the reveal animation is complete.
    pub fn is_fully_revealed(&self) -> bool {
        self.revealed_chars >= self.text.len()
    }

    fn visible_text(&self) -> &str {
        if self.revealed_chars >= self.text.len() {
            &self.text
        } else {
            // Find a char boundary at or before revealed_chars
            let end = self.text.floor_char_boundary(self.revealed_chars);
            &self.text[..end]
        }
    }
}

impl HistoryCell for AssistantCell {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        let visible = self.visible_text();
        if visible.is_empty() {
            return vec![];
        }

        let clean_text = strip_trailing_tool_json(visible);
        if clean_text.is_empty() {
            return vec![];
        }

        let mut lines = Vec::new();
        for (i, line) in clean_text.lines().enumerate() {
            if i == 0 {
                lines.push(Line::from(vec![
                    Span::styled("● ", Style::default().fg(CRAB_COLOR)),
                    Span::styled(line.to_string(), Style::default().fg(Color::White)),
                ]));
            } else {
                lines.push(Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(Color::White),
                )));
            }
        }
        lines.push(Line::default());
        lines
    }

    fn is_active(&self) -> bool {
        self.active
    }
}

/// Tool invocation cell.
pub struct ToolUseCell {
    pub id: String,
    pub name: String,
}

impl HistoryCell for ToolUseCell {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        vec![Line::from(Span::styled(
            format!("● {}", self.name),
            Style::default().fg(Color::DarkGray),
        ))]
    }
}

/// Tool result cell.
pub struct ToolResultCell {
    pub id: String,
    pub tool_name: String,
    pub output: String,
    pub is_error: bool,
}

impl HistoryCell for ToolResultCell {
    fn id(&self) -> &str {
        &self.id
    }

    #[allow(clippy::cast_possible_truncation)]
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        let style = if self.is_error {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let lines: Vec<&str> = self.output.lines().collect();
        let show = lines.len().min(10);
        let mut result = Vec::new();
        for line in &lines[..show] {
            result.push(Line::from(Span::styled(format!("  {line}"), style)));
        }
        if lines.len() > 10 {
            result.push(Line::from(Span::styled(
                format!("  ... ({} more lines)", lines.len() - 10),
                Style::default().fg(Color::DarkGray),
            )));
        }
        result.push(Line::default());
        result
    }
}

/// System message cell.
pub struct SystemCell {
    pub id: String,
    pub text: String,
}

impl HistoryCell for SystemCell {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        vec![Line::from(Span::styled(
            self.text.clone(),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        ))]
    }
}

// ─── Virtual Scroller ──────────────────────────────────────────────

/// Virtual scroller — only renders cells visible in the viewport.
///
/// Maintains a height cache for O(1) lookups and uses cumulative heights
/// for binary search to find visible range.
pub struct VirtualScroller {
    /// All transcript cells.
    pub cells: Vec<Arc<dyn HistoryCell>>,
    /// Height cache: `(cell_index, width) → height`.
    height_cache: HashMap<(usize, u16), u16>,
    /// Scroll offset from bottom (in lines).
    pub scroll_offset: usize,
    /// Whether to auto-follow the bottom (sticky scroll).
    pub sticky: bool,
    /// Counter for generating unique cell IDs.
    next_id: usize,
}

impl VirtualScroller {
    /// Create a new empty scroller.
    pub fn new() -> Self {
        Self {
            cells: Vec::new(),
            height_cache: HashMap::new(),
            scroll_offset: 0,
            sticky: true,
            next_id: 0,
        }
    }

    /// Generate a unique cell ID.
    pub fn next_id(&mut self) -> String {
        self.next_id += 1;
        format!("cell_{}", self.next_id)
    }

    /// Add a cell to the transcript.
    pub fn push(&mut self, cell: Arc<dyn HistoryCell>) {
        self.cells.push(cell);
        // If sticky, stay at bottom
        if self.sticky {
            self.scroll_offset = 0;
        }
    }

    /// Get the last cell (e.g., to append to an active assistant cell).
    pub fn last_cell(&self) -> Option<&Arc<dyn HistoryCell>> {
        self.cells.last()
    }

    /// Get height for a cell at the given width, using cache.
    #[allow(dead_code)]
    fn cell_height(&mut self, index: usize, width: u16) -> u16 {
        if let Some(&h) = self.height_cache.get(&(index, width)) {
            return h;
        }
        let h = self.cells[index].height(width);
        self.height_cache.insert((index, width), h);
        h
    }

    /// Invalidate the height cache (e.g., on terminal resize or cell mutation).
    pub fn invalidate_cache(&mut self) {
        self.height_cache.clear();
    }

    /// Invalidate cache for a specific cell (e.g., when streaming content).
    pub fn invalidate_cell(&mut self, index: usize) {
        self.height_cache.retain(|&(i, _), _| i != index);
    }

    /// Total height of all cells for the given width.
    #[allow(dead_code)]
    fn total_height(&mut self, width: u16) -> usize {
        let mut total: usize = 0;
        for i in 0..self.cells.len() {
            total += self.cell_height(i, width) as usize;
        }
        total
    }

    /// Scroll up by N lines.
    pub fn scroll_up(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_add(n);
        self.sticky = false;
    }

    /// Scroll down by N lines.
    pub fn scroll_down(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
        if self.scroll_offset == 0 {
            self.sticky = true;
        }
    }

    /// Scroll to bottom.
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
        self.sticky = true;
    }

    /// Convert from `ChatMessage` list (migration path from Phase 1).
    pub fn from_messages(messages: &[ChatMessage]) -> Self {
        let mut scroller = Self::new();
        for msg in messages {
            let id = scroller.next_id();
            let cell: Arc<dyn HistoryCell> = match msg {
                ChatMessage::User { text } => Arc::new(UserCell {
                    id,
                    text: text.clone(),
                }),
                ChatMessage::Assistant { text } => Arc::new(AssistantCell {
                    id,
                    text: text.clone(),
                    revealed_chars: text.len(),
                    active: false,
                }),
                ChatMessage::ToolUse { name, .. } => Arc::new(ToolUseCell {
                    id,
                    name: name.clone(),
                }),
                ChatMessage::ToolResult {
                    tool_name,
                    output,
                    is_error,
                    ..
                } => Arc::new(ToolResultCell {
                    id,
                    tool_name: tool_name.clone(),
                    output: output.clone(),
                    is_error: *is_error,
                }),
                ChatMessage::System { text } => Arc::new(SystemCell {
                    id,
                    text: text.clone(),
                }),
            };
            scroller.push(cell);
        }
        scroller
    }
}

impl Default for VirtualScroller {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderable for VirtualScroller {
    #[allow(clippy::cast_possible_truncation)]
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || self.cells.is_empty() {
            return;
        }

        // Collect all display lines from all cells
        // (In a fully optimized version, we'd only render visible cells via
        // cumulative height binary search. For now, this is correct and handles
        // the common case. Optimization comes in Phase 8.)
        let mut all_lines: Vec<Line<'static>> = Vec::new();
        for cell in &self.cells {
            all_lines.extend(cell.display_lines(area.width));
        }

        // Render with scroll (same as MessageList for now)
        let visible = area.height as usize;
        let end = all_lines.len().saturating_sub(self.scroll_offset);
        let start = end.saturating_sub(visible);

        for (i, line) in all_lines
            .iter()
            .skip(start)
            .take(visible.min(end.saturating_sub(start)))
            .enumerate()
        {
            let y = area.y + i as u16;
            Widget::render(
                line.clone(),
                Rect {
                    x: area.x,
                    y,
                    width: area.width,
                    height: 1,
                },
                buf,
            );
        }
    }

    fn desired_height(&self, _width: u16) -> u16 {
        0 // flex item
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_cell_display() {
        let cell = UserCell {
            id: "1".into(),
            text: "hello".into(),
        };
        let lines = cell.display_lines(80);
        assert_eq!(lines.len(), 2); // text + blank
    }

    #[test]
    fn assistant_cell_streaming() {
        let mut cell = AssistantCell {
            id: "2".into(),
            text: "Hello world".into(),
            revealed_chars: 0,
            active: true,
        };
        assert!(cell.is_active());
        assert!(!cell.is_fully_revealed());

        // Reveal 5 chars
        cell.advance_reveal(5);
        assert_eq!(cell.visible_text(), "Hello");

        // Reveal all
        cell.advance_reveal(100);
        assert!(cell.is_fully_revealed());
        assert_eq!(cell.visible_text(), "Hello world");
    }

    #[test]
    fn tool_result_truncation() {
        let output = (0..20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let cell = ToolResultCell {
            id: "3".into(),
            tool_name: "bash".into(),
            output,
            is_error: false,
        };
        let lines = cell.display_lines(80);
        // 10 shown + 1 "more" + 1 blank = 12
        assert_eq!(lines.len(), 12);
    }

    #[test]
    fn virtual_scroller_push_and_render() {
        let mut scroller = VirtualScroller::new();
        let id = scroller.next_id();
        scroller.push(Arc::new(UserCell {
            id,
            text: "test".into(),
        }));
        assert_eq!(scroller.cells.len(), 1);

        let area = Rect::new(0, 0, 80, 20);
        let mut buf = Buffer::empty(area);
        scroller.render(area, &mut buf);
    }

    #[test]
    fn virtual_scroller_scroll() {
        let mut scroller = VirtualScroller::new();
        assert!(scroller.sticky);

        scroller.scroll_up(10);
        assert!(!scroller.sticky);
        assert_eq!(scroller.scroll_offset, 10);

        scroller.scroll_down(5);
        assert_eq!(scroller.scroll_offset, 5);

        scroller.scroll_to_bottom();
        assert!(scroller.sticky);
        assert_eq!(scroller.scroll_offset, 0);
    }

    #[test]
    fn virtual_scroller_from_messages() {
        let messages = vec![
            ChatMessage::User { text: "hi".into() },
            ChatMessage::Assistant {
                text: "hello".into(),
            },
            ChatMessage::ToolUse {
                name: "bash".into(),
                summary: None,
            },
            ChatMessage::ToolResult {
                tool_name: "bash".into(),
                output: "ok".into(),
                is_error: false,
                display: None,
            },
            ChatMessage::System {
                text: "saved".into(),
            },
        ];
        let scroller = VirtualScroller::from_messages(&messages);
        assert_eq!(scroller.cells.len(), 5);
    }

    #[test]
    fn height_cache_works() {
        let mut scroller = VirtualScroller::new();
        let id = scroller.next_id();
        scroller.push(Arc::new(UserCell {
            id,
            text: "test".into(),
        }));
        let h1 = scroller.cell_height(0, 80);
        let h2 = scroller.cell_height(0, 80);
        assert_eq!(h1, h2);

        scroller.invalidate_cache();
        let h3 = scroller.cell_height(0, 80);
        assert_eq!(h1, h3);
    }
}
