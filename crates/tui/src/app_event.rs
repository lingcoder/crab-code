//! Internal TUI event bus — decouples event interpretation from state mutation.
//!
//! `AppEvent` is the single vocabulary for all state changes in the TUI.
//! External events (`TuiEvent`) are translated into `AppEvent`s, which are
//! then applied to mutate `App` state.

/// Internal application events — produced by `translate_event()`,
/// consumed by `apply_event()`.
#[derive(Debug, Clone)]
pub enum AppEvent {
    // ── Input ──
    /// User submitted text from the input box.
    InputSubmit(String),
    /// User cancelled input (Esc).
    InputCancel,
    /// Replace the input box contents with `text` (e.g. history search selection,
    /// external editor result). Does NOT submit — user can still edit first.
    InsertInputText(String),
    /// Bracketed paste from the terminal — insert `text` at the cursor without
    /// submitting. Distinct from `InsertInputText` (which replaces) because a
    /// paste appends to what the user was already typing.
    Paste(String),

    // ── Navigation ──
    /// Scroll content up by N lines.
    ScrollUp(u16),
    /// Scroll content down by N lines.
    ScrollDown(u16),
    /// Scroll to the bottom of content.
    ScrollToBottom,

    // ── Permission ──
    /// User allowed a permission request.
    PermissionAllow(String),
    /// User denied a permission request.
    PermissionDeny(String),
    /// User allowed a permission request permanently.
    PermissionAllowAlways(String),

    // ── Overlay lifecycle ──
    /// Open in-conversation search.
    OpenSearch,
    /// Close in-conversation search.
    CloseSearch,
    /// Open the command palette overlay.
    OpenCommandPalette,
    /// Open history search overlay.
    OpenHistorySearch,
    /// Open model picker overlay.
    OpenModelPicker,
    /// Open interactive diff viewer overlay.
    OpenDiffViewer { diff_text: String },
    /// Open full-screen transcript.
    OpenTranscript,
    /// Close the topmost overlay.
    CloseOverlay,

    // ── Component updates ──
    /// Toggle session sidebar visibility.
    ToggleSidebar,
    /// Toggle fold/unfold of selected tool output.
    ToggleFold,
    /// Copy focused code block to clipboard.
    CopyCodeBlock,
    /// Cycle permission mode (default → acceptEdits → plan → default).
    CyclePermissionMode,

    // ── Agent lifecycle (translated from `crab_core::event::Event`) ──
    /// Append text to the current assistant message.
    ContentAppend(String),
    /// A tool execution started.
    ///
    /// `input` is the raw tool input JSON so `apply_event` can call
    /// `Tool::format_use_summary` via the tool registry. We compute the
    /// summary inside `apply_event` (not `translate_event`) because the
    /// registry lives on `App`, and this keeps the translator pure.
    ToolStart {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolProgress {
        id: String,
        progress: crab_core::tool::ToolProgress,
    },
    /// Incremental stdout/stderr line(s) from a still-running tool.
    /// Drives the in-transcript [`crate::app::ChatMessage::ToolProgress`]
    /// cell (rebuilt in-place as deltas arrive) so the user sees long
    /// shell commands progressing instead of staring at a spinner.
    ToolOutputDelta { id: String, delta: String },
    ToolFinished {
        id: String,
        output: crab_core::tool::ToolOutput,
    },
    /// The agent message is complete.
    MessageComplete {
        input_tokens: u64,
        output_tokens: u64,
    },
    /// An agent error occurred.
    AgentError(String),
    /// Streaming response was aborted — discard partial assistant content.
    StreamAborted { reason: String },
    /// A permission request arrived from the agent.
    PermissionRequested {
        request_id: String,
        tool_name: String,
        summary: String,
        tool_input: serde_json::Value,
    },

    // ── Session ──
    /// Create a new session.
    NewSession,
    /// Switch to a session by ID.
    SwitchSession(String),

    // ── Per-message actions ──
    /// Copy the selected message's text to the clipboard.
    MessageCopy { index: usize },
    /// Load the selected message into the input and remove it from history.
    MessageEdit { index: usize },
    /// Remove the selected message from history.
    MessageDelete { index: usize },
    /// Truncate history to just before the selected message.
    MessageRewind { index: usize },

    // ── Model ──
    /// Switch the active model by name.
    SwitchModel(String),

    // ── System ──
    /// Periodic tick (animations, spinner).
    Tick,
    /// Terminal resized.
    Resize(u16, u16),
    /// User requested quit.
    Quit,
    /// Force a full terminal redraw.
    Redraw,

    // ── External editor ──
    /// Open external editor.
    ExternalEditorOpen,
    /// External editor closed with resulting text.
    ExternalEditorClosed(String),

    // ── Stash ──
    /// Stash/unstash current input.
    Stash,

    // ── Misc ──
    /// Kill all running agents.
    KillAgents,
    /// Undo last input edit.
    Undo,
    /// Toggle todos panel.
    ToggleTodos,
    /// Paste image from clipboard.
    ImagePaste,

    // ── System events (compact, token warning, session save/resume) ──
    /// Compaction started.
    CompactStart { strategy: String },
    /// Compaction ended.
    CompactEnd {
        after_tokens: u64,
        removed_messages: usize,
    },
    /// Token usage warning.
    TokenWarning {
        usage_pct: f64,
        used: u64,
        limit: u64,
    },
    /// Session saved.
    SessionSaved { session_id: String },
    /// Session resumed.
    SessionResumed {
        session_id: String,
        message_count: usize,
    },

    // ── Thinking ──
    /// Thinking state changed.
    ThinkingChanged { active: bool },
    /// Append incremental thinking text from a `ThinkingDelta` event.
    ThinkingAppend(String),

    // ── Trust ──
    /// User accepted the project trust dialog.
    TrustAccepted { project_path: String },
    /// User denied the project trust dialog (enter bare mode).
    TrustDenied,

    // ── Hot-reload ──
    /// Settings file changed on disk and was reloaded.
    SettingsReloaded { warnings: Vec<String> },
    /// Skills directory changed; skills re-discovered.
    SkillsReloaded { count: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_event_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<AppEvent>();
    }

    #[test]
    fn app_event_clone() {
        let event = AppEvent::ContentAppend("hello".into());
        #[allow(clippy::redundant_clone)]
        let cloned = event.clone();
        let AppEvent::ContentAppend(text) = cloned else {
            panic!("expected ContentAppend");
        };
        assert_eq!(text, "hello");
    }

    #[test]
    fn app_event_debug() {
        let event = AppEvent::Tick;
        let debug = format!("{event:?}");
        assert!(debug.contains("Tick"));
    }
}
