//! App state machine and main event loop.

mod commands;
mod instance;
mod paste_registry;
mod state;
mod update;

pub use instance::App;
pub use paste_registry::{PASTE_COLLAPSE_MIN_LINES, PastedContents};

/// Whether extended-thinking content should be rendered as inline transcript
/// cells. Off by default — thinking only refreshes the transient spinner —
/// unless `CRAB_SHOW_THINKING` is set to a truthy value. Shared by the live
/// `ThinkingAppend` path and resume replay so both render consistently.
#[must_use]
pub(crate) fn thinking_transcript_enabled() -> bool {
    std::env::var("CRAB_SHOW_THINKING")
        .is_ok_and(|v| !matches!(v.as_str(), "" | "0" | "false" | "no" | "off"))
}
pub use state::{
    ActiveToolInfo, AppAction, AppState, ChatMessage, ExitKey, PromptInputMode, ThinkingState,
    ToolCallStatus,
};
