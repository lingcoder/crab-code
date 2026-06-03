//! Bash tool call with output long enough to trigger the collapsed
//! `ToolResultCell` path with the Ctrl+O hint.

use crab_core::tool::ToolDisplayStyle;
use crab_tui::app::ToolCallStatus;
use crab_tui::history::HistoryCell;
use crab_tui::history::cells::tool_call::ToolCallCell;
use crab_tui::history::cells::tool_result::ToolResultCell;

use super::helpers::{assert_snapshot, render_lines_to_text};

#[test]
fn s06_bash_long_output_collapsed() {
    let call = ToolCallCell::new(
        "Bash",
        Some("Run (find . -name '*.rs')".into()),
        Some(ToolDisplayStyle::Highlight),
        ToolCallStatus::Success,
    );
    let body = (0..30)
        .map(|i| format!("./crates/foo/src/file_{i}.rs"))
        .collect::<Vec<_>>()
        .join("\n");
    let result = ToolResultCell::new("Bash", body, false, None, true);

    let mut lines = call.display_lines(100);
    lines.extend(result.display_lines(100));
    let text = render_lines_to_text(&lines, 100, 28);
    assert_snapshot("s06_bash_long_output_collapsed", &text);
}
