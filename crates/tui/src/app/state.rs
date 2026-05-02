//! Standalone types used by the App: state phases, actions, message variants.

use std::time::{Duration, Instant};

/// Application state phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppState {
    /// Backend services loading (MCP, memory, skills).
    Initializing,
    /// Waiting for user input.
    Idle,
    /// User is typing a message.
    WaitingForInput,
    /// Agent is processing (streaming response).
    Processing,
    /// Waiting for user to confirm a tool execution.
    Confirming,
}

/// Tracks whether the LLM is currently in a "thinking" phase (extended thinking / chain-of-thought).
#[derive(Debug, Clone)]
pub enum ThinkingState {
    /// Not thinking.
    Idle,
    /// Currently thinking; tracks when thinking started.
    Thinking { started_at: Instant },
    /// Thinking finished; shows elapsed duration briefly before clearing.
    ThoughtFor {
        duration: Duration,
        finished_at: Instant,
    },
}

impl ThinkingState {
    /// How long the "(thought for Ns)" label remains visible after thinking ends.
    pub const DISPLAY_DURATION: Duration = Duration::from_secs(2);
}

/// The current prompt input mode — determines what the input area is used for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptInputMode {
    /// Normal prompt input to the agent.
    Prompt,
    /// Bash / shell command input.
    Bash,
    /// Waiting for the user to accept or deny an orphaned permission request.
    OrphanedPermission,
    /// Displaying a task notification that needs acknowledgement.
    TaskNotification,
}

impl PromptInputMode {
    /// Short label for displaying in the input area indicator.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Prompt => "prompt",
            Self::Bash => "bash",
            Self::OrphanedPermission => "permission",
            Self::TaskNotification => "task",
        }
    }
}

/// Action returned by the app's event handler to signal the outer loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppAction {
    /// No action needed — continue the loop.
    None,
    /// User submitted a message to send to the agent.
    Submit(String),
    /// User confirmed a permission request. `feedback` is an optional
    /// free-text note from the user (typically only set on a deny) that
    /// the runner forwards back to the model so it can adjust its approach.
    PermissionResponse {
        request_id: String,
        allowed: bool,
        feedback: Option<String>,
    },
    /// Ctrl+C during `Confirming` — reject all queued permission requests
    /// and interrupt the engine loop.
    InterruptPermissions { rejected_ids: Vec<String> },
    /// First Ctrl-C during Processing state: signal runner to cancel the
    /// in-flight turn without quitting the app.
    InterruptProcessing,
    /// User requested quit (Ctrl+C / Ctrl+D).
    Quit,
    /// User wants to create a new session.
    NewSession,
    /// User wants to switch to a different session by ID.
    SwitchSession(String),
    /// User pressed Ctrl+G to open the external editor with the given input text.
    /// Runner pauses the event loop, spawns `$EDITOR`, then injects
    /// `AppEvent::ExternalEditorClosed(text)` once the editor exits.
    ExternalEditor(String),
    /// User accepted the project trust dialog for the first time. The
    /// runner fires the [`HookTrigger::Setup`] lifecycle hook so projects
    /// can run one-shot setup (install dependencies, materialize config,
    /// …) the first time Crab Code is trusted there.
    FireSetupHook { project_path: String },
}

/// Which key initiated the current double-press exit window.
///
/// Recorded on first press so the bottom-bar hint can name the exact key
/// the user must press again (`Press Ctrl-C again to exit` vs
/// `Press Ctrl-D again to exit`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitKey {
    CtrlC,
    CtrlD,
}

impl ExitKey {
    /// Display name shown in the bottom-bar hint.
    #[must_use]
    pub fn display_name(self) -> &'static str {
        match self {
            Self::CtrlC => "Ctrl-C",
            Self::CtrlD => "Ctrl-D",
        }
    }
}

/// A single message in the conversation, structurally typed for rendering.
///
/// Replaces the flat `content_buffer: String` with a typed message list.
/// Each variant maps to a distinct visual representation in the TUI.
#[derive(Debug, Clone)]
pub enum ChatMessage {
    /// User input — rendered as `❯ {text}`.
    User { text: String },
    /// Assistant text response — rendered with `●` prefix + markdown.
    /// `text` is appended incrementally during streaming.
    Assistant { text: String },
    /// Tool invocation start — rendered as `● {summary}` or `● {name}`.
    ToolUse {
        name: String,
        /// Custom summary from `Tool::format_use_summary()`.
        summary: Option<String>,
        /// Color hint from `Tool::display_color()`.
        color: Option<crab_core::tool::ToolDisplayStyle>,
        /// Cached at push time from `Tool::is_read_only()`. Read-only calls
        /// participate in the collapsed-run grouping in `history::grouping`.
        is_read_only: bool,
    },
    /// Tool execution result — collapsible, rendered as output text.
    ToolResult {
        tool_name: String,
        output: String,
        is_error: bool,
        /// Custom display from `Tool::format_result()` or `format_error()`.
        display: Option<crab_core::tool::ToolDisplayResult>,
        /// Whether the result is currently collapsed (only `preview_lines` shown).
        collapsed: bool,
        /// Cached at push time from `Tool::is_read_only()`. Mirrors the
        /// matching `ToolUse` so grouping can work from either side.
        is_read_only: bool,
    },
    /// Live tool execution progress — replaced in-place as more output
    /// arrives, then swapped for [`ChatMessage::ToolResult`] when the
    /// tool finishes. Distinct from `ActiveToolInfo` (which is keyed by
    /// id and lives in `App::active_tools`); this variant is what the
    /// renderer iterates so progress shows up in the transcript itself.
    ToolProgress {
        tool_use_id: String,
        tool_name: String,
        /// Last few raw output lines, rebuilt on every delta. Caps at
        /// roughly 20 lines so a chatty tool can't blow up the transcript.
        tail_output: String,
        /// Total number of newline-separated chunks observed; used in the
        /// header line so a 100k-line bash output is summarized as
        /// "100000 lines" instead of being scrolled past.
        total_lines: usize,
        /// Wall-clock seconds since the tool started, refreshed on each
        /// delta. The renderer uses this for the `{elapsed:.1}s` field.
        elapsed_secs: f64,
    },
    /// System/informational message — rendered in dim gray.
    System { text: String },
    /// Compact boundary — visual separator after context compaction.
    CompactBoundary {
        strategy: String,
        after_tokens: u64,
        removed_messages: usize,
    },
    /// Plan step checklist — rendered with status glyphs and progress bar.
    PlanStep {
        title: String,
        steps: Vec<(String, crate::components::plan_card::PlanStepStatus)>,
        awaiting_approval: bool,
    },
    /// Tool invocation rejected by user — shows what was rejected.
    ToolRejected {
        tool_name: String,
        summary: String,
        /// Rich preview of the rejected content (command / diff / file).
        display: Option<crab_core::tool::ToolDisplayResult>,
    },
    /// Extended thinking content — collapsible reasoning block.
    Thinking {
        text: String,
        /// Whether the thinking block is collapsed.
        collapsed: bool,
        /// Elapsed thinking time (set when thinking completes).
        duration: Option<Duration>,
    },
    /// Welcome panel — ambient info shown at startup when there are
    /// release notes the user hasn't seen, or on a new project, or when
    /// forced via `CRAB_FORCE_FULL_LOGO`. Compact single-column layout
    /// (≤ 6 lines) that always fits any reasonable viewport so the panel
    /// isn't clipped by the bottom-anchored message-list scroller. Not
    /// cleared by `/clear`; not included in the transcript overlay.
    ///
    /// Recent activity lives in the session sidebar, not here — duplicating
    /// it made the old three-column layout overflow on short terminals.
    Welcome {
        /// Binary version this welcome was generated for — shown in the header.
        version: String,
        /// Release-note bullets pulled from the CHANGELOG for the current
        /// version. Up to 3 are rendered.
        whats_new: Vec<String>,
        /// When true, the hint row suggests creating `AGENTS.md`.
        show_project_hint: bool,
    },
}

/// Info for a tool currently being executed, keyed by `tool_use_id`.
#[derive(Debug, Clone)]
pub struct ActiveToolInfo {
    pub name: String,
    pub input: serde_json::Value,
    pub progress: Option<crab_core::tool::ToolProgress>,
}
