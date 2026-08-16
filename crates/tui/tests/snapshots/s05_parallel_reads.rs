//! Three parallel Read tool calls that trigger the collapsed-run cell path.

use crab_core::tool::CollapsedGroupLabel;
use crab_tui::app::{ChatMessage, ToolCallStatus};
use crab_tui::history::HistoryCell;
use crab_tui::history::cells::collapsed_read_search::CollapsedReadSearchCell;

use super::helpers::{assert_snapshot, render_lines_to_text};

fn read_label() -> CollapsedGroupLabel {
    CollapsedGroupLabel {
        active_verb: "Reading",
        past_verb: "Read",
        noun_singular: "file",
        noun_plural: "files",
    }
}

fn read_use(path: &str) -> ChatMessage {
    ChatMessage::ToolUse {
        id: format!("tu_{path}"),
        name: "Read".into(),
        summary: Some(format!("Read ({path})")),
        color: None,
        is_read_only: true,
        status: ToolCallStatus::Success,
        collapsed_label: Some(read_label()),
    }
}

fn read_result(text: &str) -> ChatMessage {
    ChatMessage::ToolResult {
        tool_name: "Read".into(),
        output: text.into(),
        is_error: false,
        display: None,
        collapsed: true,
        is_read_only: true,
    }
}

#[test]
fn s05_three_parallel_reads_done() {
    let msgs = vec![
        read_use("a.rs"),
        read_use("b.rs"),
        read_use("c.rs"),
        read_result("a"),
        read_result("b"),
        read_result("c"),
    ];
    let cell = CollapsedReadSearchCell::from_messages(&msgs);
    let lines = cell.display_lines(80);
    let text = render_lines_to_text(&lines, 80, 24);
    assert_snapshot("s05_three_parallel_reads_done", &text);
}

#[test]
fn s05_three_parallel_reads_pending() {
    let msgs = vec![read_use("a.rs"), read_use("b.rs"), read_use("c.rs")];
    let cell = CollapsedReadSearchCell::from_messages(&msgs);
    let lines = cell.display_lines(80);
    let text = render_lines_to_text(&lines, 80, 24);
    assert_snapshot("s05_three_parallel_reads_pending", &text);
}
