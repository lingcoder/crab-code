//! Message list component — paints the conversation transcript by
//! dispatching to per-message [`HistoryCell`] implementations.
//!
//! `ChatMessage` remains the persistence-facing enum used by `App`;
//! this renderer converts on the fly via
//! [`crate::history::cell_from_chat_message`]. When the rest of `App`
//! migrates to owning `Box<dyn HistoryCell>` directly, this module's
//! implementation only needs to drop the conversion step.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Widget;

use crate::app::ChatMessage;
use crate::history::cell_from_chat_message;
use crate::traits::Renderable;

/// Renders the structured message list with scroll support.
pub struct MessageList<'a> {
    pub messages: &'a [ChatMessage],
    pub scroll_offset: usize,
}

impl Renderable for MessageList<'_> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        render_messages(self.messages, self.scroll_offset, area, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        // Flex item — takes all available space
        0
    }
}

/// Flatten `messages` into Lines via the `HistoryCell` adapter and
/// paint the viewport.
#[allow(clippy::cast_possible_truncation)]
pub fn render_messages(
    messages: &[ChatMessage],
    scroll_offset: usize,
    area: Rect,
    buf: &mut Buffer,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let mut rendered_lines: Vec<Line<'static>> = Vec::new();
    for msg in messages {
        let cell = cell_from_chat_message(msg);
        rendered_lines.extend(cell.display_lines(area.width));
    }

    let visible = area.height as usize;
    let end = rendered_lines.len().saturating_sub(scroll_offset);
    let start = end.saturating_sub(visible);

    for (i, line) in rendered_lines
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_list_desired_height() {
        let ml = MessageList {
            messages: &[],
            scroll_offset: 0,
        };
        assert_eq!(ml.desired_height(80), 0);
    }

    #[test]
    fn render_empty_messages() {
        let area = Rect::new(0, 0, 80, 20);
        let mut buf = Buffer::empty(area);
        render_messages(&[], 0, area, &mut buf);
    }

    #[test]
    fn render_user_message() {
        let msgs = vec![ChatMessage::User {
            text: "hello".into(),
        }];
        let area = Rect::new(0, 0, 80, 20);
        let mut buf = Buffer::empty(area);
        render_messages(&msgs, 0, area, &mut buf);
    }

    #[test]
    fn render_all_variants_without_panic() {
        let msgs = vec![
            ChatMessage::User { text: "hi".into() },
            ChatMessage::Assistant {
                text: "**bold**".into(),
            },
            ChatMessage::ToolUse {
                name: "read".into(),
                summary: Some("src/lib.rs".into()),
            },
            ChatMessage::ToolResult {
                tool_name: "read".into(),
                output: "line1\nline2\nline3".into(),
                is_error: false,
                display: None,
            },
            ChatMessage::System {
                text: "note".into(),
            },
        ];
        let area = Rect::new(0, 0, 80, 40);
        let mut buf = Buffer::empty(area);
        render_messages(&msgs, 0, area, &mut buf);
    }
}
