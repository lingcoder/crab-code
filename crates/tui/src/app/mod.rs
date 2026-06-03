//! App state machine and main event loop.

mod commands;
mod instance;
mod paste_registry;
mod state;
mod update;

pub use instance::App;
pub use paste_registry::{PASTE_COLLAPSE_MIN_LINES, PastedContents};
pub use state::{
    ActiveToolInfo, AppAction, AppState, ChatMessage, ExitKey, PromptInputMode, ThinkingState,
    ToolCallStatus,
};
