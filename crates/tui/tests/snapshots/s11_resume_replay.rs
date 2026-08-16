//! Resume replay: a saved tool session rebuilt through `load_session_messages`
//! renders the same call/result cells a live turn would produce — no empty
//! `❯` shell for the `tool_result` user message.

use crab_agents::Conversation;
use crab_core::message::{ContentBlock, Message, Role};
use crab_tui::app::App;
use crab_tui::history::group_messages;

use super::helpers::{assert_snapshot, render_lines_to_text};

#[test]
fn s11_resume_tool_session() {
    let mut conv = Conversation::new("resume-snap".into(), String::new(), 100_000);
    conv.push_user("read the config");
    conv.push(Message::new(
        Role::Assistant,
        vec![
            ContentBlock::text("Let me read it."),
            ContentBlock::tool_use("t1", "Read", serde_json::json!({"path": "Cargo.toml"})),
        ],
    ));
    conv.push(Message::tool_result(
        "t1",
        "[workspace]\nresolver = \"2\"",
        false,
    ));
    conv.push_assistant("It uses resolver 2.");

    let mut app = App::new("test-model");
    app.load_session_messages(&conv);

    let cells = group_messages(&app.messages);
    let mut lines = Vec::new();
    for cell in &cells {
        lines.extend(cell.display_lines(80));
    }
    let text = render_lines_to_text(&lines, 80, 24);
    assert_snapshot("s11_resume_tool_session", &text);
}
