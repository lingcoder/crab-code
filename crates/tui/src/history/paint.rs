//! Bottom-anchored painter for the inline-viewport content area.
//!
//! Iterates [`ChatMessage`]s, renders each via the cell adapter, and paints
//! the resulting lines starting at the bottom of `area` and growing
//! upward. Excess content (more lines than `area.height`) is clipped from
//! the top — those rows belong in scrollback and the drain pipeline is
//! expected to push them out before this is called.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

use crate::app::ChatMessage;
use crate::history::group_messages;

pub fn paint_messages_bottom_up(messages: &[ChatMessage], area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 || messages.is_empty() {
        return;
    }
    let mut all_lines = Vec::new();
    // Group consecutive read-only tool runs into a single collapsed cell so the
    // live viewport shows "Read N files" instead of N separate rows.
    for cell in group_messages(messages) {
        all_lines.extend(cell.display_lines(area.width));
    }
    let visible = area.height as usize;
    let total = all_lines.len();
    let start = total.saturating_sub(visible);
    let to_paint = total - start;
    let top_pad = visible - to_paint;
    for (i, line) in all_lines[start..].iter().enumerate() {
        let y = area.y + (top_pad + i) as u16;
        if y >= area.y + area.height {
            break;
        }
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

pub fn messages_total_lines(messages: &[ChatMessage], width: u16) -> usize {
    if width == 0 {
        return 0;
    }
    // Must group the same way `paint_messages_bottom_up` does, or the scroll
    // math diverges from what's actually painted.
    group_messages(messages)
        .iter()
        .map(|cell| cell.desired_height(width) as usize)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ToolCallStatus;

    fn read_use() -> ChatMessage {
        ChatMessage::ToolUse {
            id: "read_tu".into(),
            name: "Read".into(),
            summary: Some("Read (a.rs)".into()),
            color: None,
            is_read_only: true,
            status: ToolCallStatus::Success,
            collapsed_label: Some(crab_core::tool::CollapsedGroupLabel {
                active_verb: "Reading",
                past_verb: "Read",
                noun_singular: "file",
                noun_plural: "files",
            }),
        }
    }

    fn read_result() -> ChatMessage {
        ChatMessage::ToolResult {
            tool_name: "Read".into(),
            output: "ok".into(),
            is_error: false,
            display: None,
            collapsed: true,
            is_read_only: true,
        }
    }

    #[test]
    fn total_lines_collapses_read_runs() {
        // A 3-read run renders as one compact collapsed cell, so the scroll
        // math counts far fewer lines than six separate tool cells would.
        let run = vec![
            read_use(),
            read_use(),
            read_use(),
            read_result(),
            read_result(),
            read_result(),
        ];
        let grouped = messages_total_lines(&run, 80);
        let ungrouped: usize = run
            .iter()
            .map(|m| crate::history::cell_from_chat_message(m).desired_height(80) as usize)
            .sum();
        assert!(
            grouped < ungrouped,
            "grouped run ({grouped}) should be shorter than ungrouped ({ungrouped})"
        );
    }
}
