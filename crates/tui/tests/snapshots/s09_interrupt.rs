//! Streaming assistant interrupted mid-output: a partial assistant cell
//! followed by a system `[interrupted]` notice.

use crab_tui::history::HistoryCell;
use crab_tui::history::cells::assistant::AssistantCell;
use crab_tui::history::cells::system::SystemCell;

use super::helpers::{assert_snapshot, render_lines_to_text};

#[test]
fn s09_partial_assistant_then_interrupted() {
    let asst = AssistantCell::new("Let me start by reading the");
    let sys = SystemCell::new("Interrupted \u{00b7} What should Crab do instead?");

    let mut lines = Vec::new();
    lines.extend(asst.display_lines(80));
    lines.extend(sys.display_lines(80));

    let text = render_lines_to_text(&lines, 80, 24);
    assert_snapshot("s09_partial_assistant_then_interrupted", &text);
}
