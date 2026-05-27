//! Integration test for the inline-viewport drain → scrollback flush flow.
//!
//! Builds a `vt100::Parser`-backed terminal, simulates a couple of finalized
//! cells getting drained via `App::drain_finalized_into_pending`, then runs
//! `insert_history_lines_with_mode` and verifies the rendered text shows up
//! above the inline viewport area in the simulated screen.

#![cfg(test)]

use ratatui::layout::Rect;

use crate::app::{App, AppState, ChatMessage};
use crate::custom_terminal::Terminal;
use crate::insert_history::insert_history_lines_with_mode;
use crate::terminal_detection::InsertHistoryMode;
use crate::test_backend::VT100Backend;

fn build_terminal(width: u16, height: u16) -> Terminal<VT100Backend> {
    Terminal::with_options(VT100Backend::new(width, height)).expect("terminal")
}

#[test]
fn finalized_user_message_lands_in_scrollback() {
    let width: u16 = 80;
    let height: u16 = 24;
    let mut term = build_terminal(width, height);
    // Anchor a small inline viewport at the bottom — chrome lives here,
    // history lands above.
    let viewport = Rect::new(0, height - 4, width, 4);
    term.set_viewport_area(viewport);

    let mut app = App::new("test-model");
    app.state = AppState::Idle;
    app.messages.push(ChatMessage::User {
        text: "hello from the test".into(),
    });

    app.drain_finalized_into_pending(width);
    assert!(
        app.messages.is_empty(),
        "finalized user message should be drained out"
    );
    let lines = app.pending_history.take();
    assert!(!lines.is_empty(), "drain should produce display lines");

    insert_history_lines_with_mode(&mut term, &lines, InsertHistoryMode::Standard)
        .expect("flush succeeds");

    let rows: Vec<String> = term.backend().vt100().screen().rows(0, width).collect();
    assert!(
        rows.iter().any(|r| r.contains("hello from the test")),
        "expected drained text in scrollback area, rows: {rows:?}"
    );
}

#[test]
fn streaming_tail_stays_in_viewport() {
    let width: u16 = 80;
    let height: u16 = 24;
    let mut term = build_terminal(width, height);
    term.set_viewport_area(Rect::new(0, height - 6, width, 6));

    let mut app = App::new("test-model");
    // First, a finalized user turn — drainable.
    app.messages.push(ChatMessage::User {
        text: "what's up?".into(),
    });
    // Then an assistant turn that's still streaming. App::Processing keeps
    // the tail anchored to the viewport.
    app.state = AppState::Processing;
    app.messages.push(ChatMessage::Assistant {
        streaming: false,
        committed_lines: 0,
        text: "thinking...".into(),
    });

    app.drain_finalized_into_pending(width);
    assert_eq!(
        app.messages.len(),
        1,
        "user message drained, assistant tail kept"
    );
    assert!(
        matches!(&app.messages[0], ChatMessage::Assistant { .. }),
        "remaining cell is the streaming assistant turn"
    );
    let lines = app.pending_history.take();
    assert!(
        !lines.is_empty(),
        "drained user turn should yield pending lines"
    );
    insert_history_lines_with_mode(&mut term, &lines, InsertHistoryMode::Standard).unwrap();

    let rows: Vec<String> = term.backend().vt100().screen().rows(0, width).collect();
    assert!(
        rows.iter().any(|r| r.contains("what's up?")),
        "user message reached scrollback"
    );
}

#[test]
fn fallback_mode_emits_history_above_viewport() {
    let width: u16 = 60;
    let height: u16 = 20;
    let mut term = build_terminal(width, height);
    term.set_viewport_area(Rect::new(0, height - 3, width, 3));

    let mut app = App::new("test-model");
    app.messages.push(ChatMessage::User {
        text: "fallback path".into(),
    });

    app.drain_finalized_into_pending(width);
    let lines = app.pending_history.take();
    insert_history_lines_with_mode(&mut term, &lines, InsertHistoryMode::Fallback).unwrap();

    let rows: Vec<String> = term.backend().vt100().screen().rows(0, width).collect();
    assert!(
        rows.iter().any(|r| r.contains("fallback path")),
        "fallback path also writes drained lines into scrollback area"
    );
}

#[test]
fn streaming_assistant_commits_complete_lines_only() {
    let width: u16 = 60;
    let mut app = App::new("test-model");
    app.state = AppState::Processing;
    app.messages.push(ChatMessage::Assistant {
        streaming: true,
        text: "first paragraph done.\n".into(),
        committed_lines: 0,
    });

    app.flush_streaming_assistant_lines(width);
    let after_first = app.pending_history.take();
    assert!(
        !after_first.is_empty(),
        "first complete line should be committed"
    );

    if let Some(ChatMessage::Assistant { text, .. }) = app.messages.last_mut() {
        text.push_str("\nsecond paragraph still typing");
    }
    app.flush_streaming_assistant_lines(width);
    assert!(
        app.pending_history.is_empty(),
        "incomplete tail must not be committed before its newline arrives"
    );

    if let Some(ChatMessage::Assistant { text, .. }) = app.messages.last_mut() {
        text.push('\n');
    }
    app.flush_streaming_assistant_lines(width);
    let after_second = app.pending_history.take();
    assert!(
        !after_second.is_empty(),
        "closing newline on a new paragraph triggers a commit"
    );
}

#[test]
fn streaming_holds_committed_lines_while_code_fence_open() {
    let width: u16 = 60;
    let mut app = App::new("test-model");
    app.state = AppState::Processing;
    app.messages.push(ChatMessage::Assistant {
        streaming: true,
        text: "intro line\n```rust\nfn main() {\n".into(),
        committed_lines: 0,
    });

    app.flush_streaming_assistant_lines(width);
    assert!(
        app.pending_history.is_empty(),
        "must wait for the closing fence before committing fenced lines"
    );

    if let Some(ChatMessage::Assistant { text, .. }) = app.messages.last_mut() {
        text.push_str("    println!(\"hi\");\n}\n```\n");
    }
    app.flush_streaming_assistant_lines(width);
    assert!(
        !app.pending_history.is_empty(),
        "once the fence closes, the whole block becomes committable"
    );
}

#[test]
fn streaming_holds_committed_lines_while_table_open() {
    let width: u16 = 80;
    let mut app = App::new("test-model");
    app.state = AppState::Processing;
    app.messages.push(ChatMessage::Assistant {
        streaming: true,
        text: "intro\n| Name | Age |\n| --- | --- |\n| Alice | 30 |\n".into(),
        committed_lines: 0,
    });

    app.flush_streaming_assistant_lines(width);
    // Only "intro" should commit; table rows are held back.
    let lines = app.pending_history.take();
    assert_eq!(lines.len(), 1, "only the pre-table line commits");

    // Now close the table with a blank line.
    if let Some(ChatMessage::Assistant { text, .. }) = app.messages.last_mut() {
        text.push_str("\nmore text after table\n");
    }
    app.flush_streaming_assistant_lines(width);
    let after_close = app.pending_history.take();
    assert!(
        !after_close.is_empty(),
        "once the table closes, held-back lines become committable"
    );
}

#[test]
fn single_newlines_in_prose_commit_line_by_line() {
    // Regression: SoftBreak used to collapse to a space, which made single-
    // newline-separated prose render as one giant paragraph and never reach
    // the per-line commit threshold (typewriter effect). Now that SoftBreak
    // breaks the line, each newline-terminated prose segment commits.
    let width: u16 = 60;
    let mut app = App::new("test-model");
    app.state = AppState::Processing;
    app.messages.push(ChatMessage::Assistant {
        text: "first line of prose.\n".into(),
        committed_lines: 0,
        streaming: true,
    });
    app.flush_streaming_assistant_lines(width);
    let after_first = app.pending_history.take();
    assert!(
        !after_first.is_empty(),
        "first sentence commits on its newline"
    );

    if let Some(ChatMessage::Assistant { text, .. }) = app.messages.last_mut() {
        text.push_str("second line continues right after.\n");
    }
    app.flush_streaming_assistant_lines(width);
    let after_second = app.pending_history.take();
    assert!(
        !after_second.is_empty(),
        "second sentence committed even without a blank line in between"
    );
}

#[test]
fn streaming_assistant_not_drained_when_tool_calls_follow() {
    let width: u16 = 60;
    let mut app = App::new("test-model");
    app.state = AppState::Processing;
    app.messages.push(ChatMessage::User {
        text: "do something".into(),
    });
    app.messages.push(ChatMessage::Assistant {
        text: "I'll start by checking the status…\n".into(),
        committed_lines: 0,
        streaming: true,
    });
    app.messages.push(ChatMessage::ToolUse {
        name: "bash".into(),
        summary: Some("Bash(ls)".into()),
        color: None,
        is_read_only: true,
        status: crate::app::ToolCallStatus::Running,
        collapsed_label: None,
    });

    app.drain_finalized_into_pending(width);
    let count = app
        .messages
        .iter()
        .filter(|m| matches!(m, ChatMessage::Assistant { .. }))
        .count();
    assert_eq!(
        count, 1,
        "streaming assistant must survive drain even when tool cells follow it"
    );
}

#[test]
fn thinking_delta_does_not_pollute_messages_by_default() {
    use crate::app_event::AppEvent;
    let mut app = App::new("test-model");
    app.apply_event(AppEvent::ThinkingAppend("step 1\n".into()));
    app.apply_event(AppEvent::ThinkingAppend("step 2\n".into()));
    assert!(
        app.messages.is_empty(),
        "thinking deltas must stay in the spinner status, not enter the transcript"
    );
}
