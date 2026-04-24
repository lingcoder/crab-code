//! App state machine and main event loop.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::clipboard::Clipboard;
use crate::command_queue::CommandQueue;
use crate::components::approval_queue::ApprovalQueue;
use crate::components::autocomplete::{AutoComplete, CommandInfo};
use crate::components::bottom_bar::BottomBar;
use crate::components::code_block::{CodeBlockTracker, ImagePlaceholder};
use crate::components::context_collapse::{CollapsibleSection, ContextCollapse};
use crate::components::header::HeaderBar;
use crate::components::input::InputBox;
use crate::components::input_area::InputArea;
use crate::components::notification::NotificationManager;
use crate::components::output_styles::OutputStyles;
use crate::components::permission::{PermissionCard, PermissionResponse};
use crate::components::search::{self, SearchState};
use crate::components::session_sidebar::SessionSidebar;
use crate::components::spinner::Spinner;
use crate::components::tool_output::{ToolOutputEntry, ToolOutputList};
use crate::components::virtual_list::{ViewportState, VirtualMessage, VirtualMessageList};
use crate::event::TuiEvent;
use crate::keybindings::{Action, KeyContext, Keybindings, ResolveOutcome};
use crate::layout::AppLayout;
use crate::traits::Renderable;
use crate::vim::{VimAction, VimHandler};

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
    const DISPLAY_DURATION: Duration = Duration::from_secs(2);
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
    /// User confirmed a permission request.
    PermissionResponse { request_id: String, allowed: bool },
    /// Ctrl+C during `Confirming` — reject all queued permission requests
    /// and interrupt the engine loop.
    InterruptPermissions { rejected_ids: Vec<String> },
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

/// Main TUI application.
pub struct App {
    /// Tool registry — used to call rendering hooks (`format_use_summary`, `format_result`).
    pub tool_registry: Option<std::sync::Arc<crab_agent::ToolRegistry>>,
    /// Current application state.
    pub state: AppState,
    /// Text input component.
    pub input: InputBox,
    /// Spinner component.
    pub spinner: Spinner,
    /// Accumulated content from the current assistant message.
    pub content_buffer: String,
    /// Model name (displayed in top bar).
    pub model_name: String,
    /// FIFO queue of pending permission approvals (CC-style inline cards).
    pub approval_queue: ApprovalQueue,
    /// Whether the app should exit.
    pub should_quit: bool,
    /// Active tools keyed by `tool_use_id` — supports parallel tool execution.
    pub active_tools: HashMap<String, ActiveToolInfo>,
    /// Whether the sidebar is visible.
    pub sidebar_visible: bool,
    /// Session sidebar component (session list + navigation).
    pub session_sidebar: SessionSidebar,
    /// Current session ID.
    pub session_id: String,
    /// Keybinding configuration.
    keybindings: Keybindings,
    /// Cumulative token usage for status bar.
    pub total_input_tokens: u64,
    /// Cumulative output token usage.
    pub total_output_tokens: u64,
    /// Content scroll offset (lines from bottom).
    content_scroll: usize,
    /// Tool output list with fold/unfold state.
    pub tool_outputs: ToolOutputList,
    /// Code block tracker for copy support.
    pub code_blocks: CodeBlockTracker,
    /// System clipboard access.
    clipboard: Clipboard,
    /// Search state for in-conversation search.
    pub search: SearchState,
    /// Image placeholders encountered during the session.
    pub image_placeholders: Vec<ImagePlaceholder>,
    /// Tab-completion engine for `/commands` and file paths.
    pub autocomplete: AutoComplete,
    /// Collapsible context sections for long tool outputs in the transcript.
    pub context_collapse: ContextCollapse,
    /// Centralized output style registry for content rendering.
    pub output_styles: OutputStyles,
    /// Working directory (displayed in header).
    pub working_dir: String,
    /// Memory store directory (populated from `SessionConfig.memory_dir`).
    /// Used on demand by the memory browser overlay.
    pub memory_dir: Option<std::path::PathBuf>,
    /// Current LLM thinking state (extended thinking / chain-of-thought).
    pub thinking: ThinkingState,
    /// Scroll anchor: when the user scrolls up, this holds the line index
    /// where they anchored. `None` means following the tail (auto-scroll).
    pub scroll_anchor: Option<usize>,
    /// Number of new messages received while the user was scrolled up.
    unseen_message_count: usize,
    /// Current prompt input mode.
    pub input_mode: PromptInputMode,
    /// Timestamp of last Ctrl+C press for double-press detection.
    last_interrupt: Option<Instant>,
    /// Current permission mode (cycled via Shift+Tab).
    pub permission_mode: crab_core::permission::PermissionMode,
    /// Session-level "always allow" grants (tool names granted via 'a' key).
    pub session_grants: std::collections::HashSet<String>,
    /// Structured message list — the source of truth for conversation display.
    pub messages: Vec<ChatMessage>,
    /// Queue of user commands submitted while the agent is processing.
    pub command_queue: CommandQueue,
    /// Stashed input text (Ctrl+S to save/restore).
    pub stash: Option<String>,
    /// Input history for history search (Ctrl+R).
    pub input_history_list: Vec<String>,
    /// Overlay stack for modal views (command palette, history search, etc.).
    pub overlay_stack: crate::overlay::OverlayStack,
    /// Vim-style input handler.
    pub vim: VimHandler,
    /// Toast notification manager.
    pub notifications: NotificationManager,
    /// When agent processing started (for terminal notification after timeout).
    processing_start: Option<Instant>,
    /// Virtualized message list — caches laid-out lines per `(id, width)`.
    virtual_list: VirtualMessageList,
    /// Monotonic counter for assigning stable IDs to `VirtualMessage`s.
    next_msg_id: u64,
    /// Last render width — used to invalidate the virtual list cache on resize.
    last_render_width: u16,
}

impl App {
    /// Create a new App with default state.
    #[must_use]
    pub fn new(model_name: impl Into<String>) -> Self {
        Self {
            tool_registry: None,
            state: AppState::Idle,
            input: InputBox::new(),
            spinner: Spinner::new(),
            content_buffer: String::new(),
            model_name: model_name.into(),
            approval_queue: ApprovalQueue::new(),
            should_quit: false,
            active_tools: HashMap::new(),
            sidebar_visible: false,
            session_sidebar: SessionSidebar::new(),
            session_id: String::new(),
            keybindings: Keybindings::defaults(),
            total_input_tokens: 0,
            total_output_tokens: 0,
            content_scroll: 0,
            tool_outputs: ToolOutputList::new(),
            code_blocks: CodeBlockTracker::new(),
            clipboard: Clipboard::new(),
            search: SearchState::new(),
            image_placeholders: Vec::new(),
            autocomplete: AutoComplete::default(),
            context_collapse: ContextCollapse::new(Vec::new()),
            output_styles: OutputStyles::default_styles(),
            working_dir: String::new(),
            memory_dir: None,
            thinking: ThinkingState::Idle,
            scroll_anchor: None,
            unseen_message_count: 0,
            input_mode: PromptInputMode::Prompt,
            last_interrupt: None,
            permission_mode: crab_core::permission::PermissionMode::Default,
            session_grants: std::collections::HashSet::new(),
            messages: Vec::new(),
            command_queue: CommandQueue::new(),
            stash: None,
            input_history_list: Vec::new(),
            overlay_stack: crate::overlay::OverlayStack::new(),
            vim: VimHandler::new(),
            notifications: NotificationManager::new(),
            processing_start: None,
            virtual_list: VirtualMessageList::new(),
            next_msg_id: 0,
            last_render_width: 0,
        }
    }

    /// Set the working directory (displayed in header).
    pub fn set_working_dir(&mut self, dir: impl Into<String>) {
        self.working_dir = dir.into();
    }

    /// Set the current session ID.
    /// Set the memory store directory (for the memory browser overlay).
    pub fn set_memory_dir(&mut self, dir: impl Into<std::path::PathBuf>) {
        self.memory_dir = Some(dir.into());
    }

    pub fn set_session_id(&mut self, id: impl Into<String>) {
        self.session_id = id.into();
    }

    /// Reset app state for a new session (clear messages, input, counters).
    ///
    /// Preserves any `Welcome` cell at the front — it's ambient context,
    /// not conversation content, so `/clear` should not remove it.
    pub fn reset_for_new_session(&mut self) {
        self.messages
            .retain(|m| matches!(m, ChatMessage::Welcome { .. }));
        self.content_buffer.clear();
        self.input.clear();
        self.state = AppState::Idle;
        self.spinner.stop();
        self.active_tools.clear();
        self.total_input_tokens = 0;
        self.total_output_tokens = 0;
        self.content_scroll = 0;
        self.scroll_anchor = None;
        self.unseen_message_count = 0;
        self.command_queue.clear();
        self.virtual_list.invalidate();
        self.next_msg_id = 0;
    }

    /// Dequeue the next queued command and prepare the app to submit it.
    ///
    /// Returns `Some(text)` if a command was waiting, `None` if the queue is
    /// empty. When a command is dequeued, the user message is pushed to the
    /// message list and the state transitions back to `Processing`.
    pub fn dequeue_command(&mut self) -> Option<String> {
        let text = self.command_queue.pop()?;
        self.messages.push(ChatMessage::User { text: text.clone() });
        self.state = AppState::Processing;
        self.spinner.start_with_random_verb();
        Some(text)
    }

    /// Toggle `collapsed` on the last `ToolResult` in the message list.
    fn toggle_last_tool_result_collapsed(&mut self) {
        for msg in self.messages.iter_mut().rev() {
            if let ChatMessage::ToolResult { collapsed, .. } = msg {
                *collapsed = !*collapsed;
                self.virtual_list.invalidate();
                return;
            }
        }
    }

    /// Rebuild the message list from a loaded conversation.
    pub fn load_session_messages(&mut self, conversation: &crab_agent::Conversation) {
        self.reset_for_new_session();
        self.session_id.clone_from(&conversation.id);
        for msg in conversation.messages() {
            let text = msg.text();
            let chat_msg = match msg.role {
                crab_core::message::Role::User => ChatMessage::User { text },
                crab_core::message::Role::Assistant => ChatMessage::Assistant { text },
                crab_core::message::Role::System => ChatMessage::System { text },
            };
            self.messages.push(chat_msg);
        }
    }

    /// Set custom keybindings.
    pub fn set_keybindings(&mut self, keybindings: Keybindings) {
        self.keybindings = keybindings;
    }

    /// Register slash commands for Tab completion.
    pub fn set_slash_commands(&mut self, commands: Vec<CommandInfo>) {
        self.autocomplete.set_commands(commands);
    }

    /// Set the working directory for file path completion.
    pub fn set_completion_cwd(&mut self, cwd: impl Into<std::path::PathBuf>) {
        self.autocomplete.set_cwd(cwd);
    }

    /// Transition the thinking state.
    ///
    /// When `active` is `true`, enters `Thinking` with the current timestamp.
    /// When `false`, transitions from `Thinking` to `ThoughtFor` so the elapsed
    /// duration can be displayed briefly, or resets to `Idle` if not thinking.
    pub fn set_thinking(&mut self, active: bool) {
        if active {
            self.thinking = ThinkingState::Thinking {
                started_at: Instant::now(),
            };
        } else if let ThinkingState::Thinking { started_at } = self.thinking {
            self.thinking = ThinkingState::ThoughtFor {
                duration: started_at.elapsed(),
                finished_at: Instant::now(),
            };
        } else {
            self.thinking = ThinkingState::Idle;
        }
    }

    /// Cycle to the next `PromptInputMode`.
    pub fn cycle_input_mode(&mut self) {
        self.input_mode = match self.input_mode {
            PromptInputMode::Prompt => PromptInputMode::Bash,
            PromptInputMode::Bash => PromptInputMode::Prompt,
            // Non-cycleable modes stay put until explicitly cleared
            other => other,
        };
    }

    /// Handle a TUI event and return an action for the outer loop.
    pub fn handle_event(&mut self, event: TuiEvent) -> AppAction {
        // Key events stay on the dedicated `handle_key` path — their
        // interpretation depends on overlay stack, search mode, autocomplete,
        // and `AppState`, which is too much conditional state to model as a
        // pure translator today. Everything else goes through the
        // `translate_event` → `apply_event` pipeline (the Elm-style reducer).
        match event {
            TuiEvent::Key(key) => self.handle_key(key),
            other => {
                let app_events = self.translate_event(&other);
                let mut action = AppAction::None;
                for app_event in app_events {
                    // The translator currently produces at most one `AppEvent`
                    // per `TuiEvent`, but the shape is kept for future growth.
                    // Apply each in order; the last non-`None` action wins.
                    let next = self.apply_event(app_event);
                    if !matches!(next, AppAction::None) {
                        action = next;
                    }
                }
                action
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> AppAction {
        // Overlay stack gets first priority
        if !self.overlay_stack.is_empty() {
            if let Some(action) = self.overlay_stack.handle_key(key) {
                match action {
                    crate::overlay::OverlayAction::Execute(app_event) => {
                        return self.apply_event(app_event);
                    }
                    crate::overlay::OverlayAction::Consumed
                    | crate::overlay::OverlayAction::Dismiss => {
                        return AppAction::None;
                    }
                    crate::overlay::OverlayAction::Passthrough => {}
                }
            }
            return AppAction::None;
        }

        // Search mode intercepts all keys except Esc and Enter
        if self.search.is_active() {
            return self.handle_search_key(key);
        }

        // Check keybinding actions first (global shortcuts + chord bindings).
        //
        // Build the focus chain innermost-first: overlay contexts, then the
        // state-dependent primary context, then `Chat` as the outer fallback
        // (Resolver implicitly appends Global underneath).
        let mut focus_chain = self.overlay_stack.active_contexts();
        let state_ctx = match self.state {
            AppState::Confirming => KeyContext::Permission,
            AppState::Processing | AppState::Initializing => KeyContext::Chat,
            AppState::Idle | AppState::WaitingForInput => KeyContext::Input,
        };
        if !focus_chain.contains(&state_ctx) {
            focus_chain.push(state_ctx);
        }
        if !focus_chain.contains(&KeyContext::Chat) {
            focus_chain.push(KeyContext::Chat);
        }

        let outcome = self.keybindings.feed(key, &focus_chain);
        let resolved_action: Option<Action> = match outcome {
            ResolveOutcome::Action(action) => Some(action),
            ResolveOutcome::PendingChord { .. } => {
                // A chord prefix is in flight; absorb the key and wait for
                // the continuation (or timeout) to come through.
                return AppAction::None;
            }
            ResolveOutcome::Timeout | ResolveOutcome::Unhandled(_) => None,
        };
        if let Some(action) = resolved_action {
            match action {
                Action::Quit => {
                    let now = Instant::now();
                    if let Some(last) = self.last_interrupt
                        && now.duration_since(last) < Duration::from_millis(800)
                    {
                        self.should_quit = true;
                        return AppAction::Quit;
                    }
                    self.last_interrupt = Some(now);
                    self.input.clear();
                    if self.state == AppState::Confirming {
                        let rejected_ids = self.approval_queue.reject_all();
                        self.spinner.stop();
                        self.state = AppState::Idle;
                        let _ = writeln!(self.content_buffer, "\n[interrupted]");
                        return AppAction::InterruptPermissions { rejected_ids };
                    }
                    if self.state == AppState::Processing {
                        self.spinner.stop();
                        self.state = AppState::Idle;
                        let _ = writeln!(self.content_buffer, "\n[interrupted]");
                    }
                    return AppAction::None;
                }
                Action::NewSession if self.state != AppState::Confirming => {
                    return AppAction::NewSession;
                }
                Action::ToggleSidebar => {
                    self.sidebar_visible = !self.sidebar_visible;
                    self.session_sidebar.visible = self.sidebar_visible;
                    return AppAction::None;
                }
                Action::ScrollUp if self.state != AppState::Confirming => {
                    self.content_scroll = self.content_scroll.saturating_add(10);
                    let total = self.virtual_list.last_total_lines();
                    self.scroll_anchor = Some(total.saturating_sub(self.content_scroll));
                    return AppAction::None;
                }
                Action::ScrollDown if self.state != AppState::Confirming => {
                    self.content_scroll = self.content_scroll.saturating_sub(10);
                    // Clear anchor when scrolled back to bottom
                    if self.content_scroll == 0 {
                        self.scroll_anchor = None;
                        self.unseen_message_count = 0;
                    }
                    return AppAction::None;
                }
                Action::ToggleFold if self.state != AppState::Confirming => {
                    self.tool_outputs.toggle_selected();
                    self.toggle_last_tool_result_collapsed();
                    return AppAction::None;
                }
                Action::CopyCodeBlock if self.state != AppState::Confirming => {
                    self.code_blocks.update(&self.content_buffer);
                    if let Some(text) = self.code_blocks.copy_focused() {
                        match self.clipboard.copy(&text) {
                            Ok(()) => {
                                let _ = write!(
                                    self.content_buffer,
                                    "\n[copied {} bytes to clipboard]",
                                    text.len()
                                );
                            }
                            Err(e) => {
                                let _ = write!(self.content_buffer, "\n[copy failed: {e}]");
                            }
                        }
                    }
                    return AppAction::None;
                }
                Action::Search if self.state != AppState::Confirming => {
                    self.search.activate();
                    return AppAction::None;
                }
                Action::SearchNext if self.state != AppState::Confirming => {
                    self.search.next_match();
                    self.scroll_to_search_match();
                    return AppAction::None;
                }
                Action::SearchPrev if self.state != AppState::Confirming => {
                    self.search.prev_match();
                    self.scroll_to_search_match();
                    return AppAction::None;
                }
                Action::CycleMode if self.state != AppState::Confirming => {
                    // CC cycles: default → acceptEdits → plan → default
                    use crab_core::permission::PermissionMode;
                    self.permission_mode = match self.permission_mode {
                        PermissionMode::Default => PermissionMode::AcceptEdits,
                        PermissionMode::AcceptEdits => PermissionMode::Plan,
                        // All other modes cycle back to Default
                        _ => PermissionMode::Default,
                    };
                    return AppAction::None;
                }
                // Redraw: handled by outer loop on next frame.
                Action::Redraw => {
                    return AppAction::None;
                }
                Action::HistorySearch if self.state != AppState::Confirming => {
                    let overlay = crate::components::history_search::HistorySearchOverlay::new(
                        self.input_history_list.clone(),
                    );
                    self.overlay_stack.push(Box::new(overlay));
                    return AppAction::None;
                }
                Action::ToggleTranscript if self.state != AppState::Confirming => {
                    let overlay = crate::components::transcript_overlay::TranscriptOverlay::new(
                        &self.messages,
                    );
                    self.overlay_stack.push(Box::new(overlay));
                    return AppAction::None;
                }
                Action::Stash if self.state != AppState::Confirming => {
                    if let Some(stashed) = self.stash.take() {
                        // Restore stashed text
                        let current = self.input.text();
                        if !current.is_empty() {
                            self.stash = Some(current);
                        }
                        self.input.set_text(&stashed);
                    } else if !self.input.is_empty() {
                        // Stash current text
                        self.stash = Some(self.input.text());
                        self.input.set_text("");
                    }
                    return AppAction::None;
                }
                Action::Undo if self.state != AppState::Confirming => {
                    self.input.undo();
                    return AppAction::None;
                }
                Action::KillAgents if self.state != AppState::Confirming => {
                    if self.state == AppState::Processing {
                        self.spinner.stop();
                        self.state = AppState::Idle;
                        self.messages.push(ChatMessage::System {
                            text: "[agents killed]".into(),
                        });
                    }
                    return AppAction::None;
                }
                Action::OpenHelp if self.state != AppState::Confirming => {
                    let overlay = crate::components::shortcut_hint::HelpOverlay::new();
                    self.overlay_stack.push(Box::new(overlay));
                    return AppAction::None;
                }
                Action::OpenMemoryBrowser if self.state != AppState::Confirming => {
                    let Some(dir) = self.memory_dir.as_ref() else {
                        self.notifications
                            .warn("Memory directory not configured".to_string());
                        return AppAction::None;
                    };
                    let entries = crate::components::memory_browser::load_memories(dir);
                    let overlay =
                        crate::components::memory_browser::MemoryBrowserOverlay::new(entries);
                    self.overlay_stack.push(Box::new(overlay));
                    return AppAction::None;
                }
                Action::OpenMcpBrowser if self.state != AppState::Confirming => {
                    let Some(registry) = self.tool_registry.as_ref() else {
                        self.notifications
                            .warn("Tool registry not yet initialized".to_string());
                        return AppAction::None;
                    };
                    let servers = crate::components::mcp_browser::load_mcp_servers(registry);
                    let overlay =
                        crate::components::mcp_browser::McpBrowserOverlay::new(servers);
                    self.overlay_stack.push(Box::new(overlay));
                    return AppAction::None;
                }
                Action::ModelPicker if self.state != AppState::Confirming => {
                    let models = vec![
                        "claude-opus-4-6".to_string(),
                        "claude-sonnet-4-6".to_string(),
                        "claude-haiku-4-5-20251001".to_string(),
                        "gpt-4o".to_string(),
                        "deepseek-chat".to_string(),
                    ];
                    let overlay = crate::components::model_picker::ModelPickerOverlay::new(
                        models,
                        self.model_name.clone(),
                    );
                    self.overlay_stack.push(Box::new(overlay));
                    return AppAction::None;
                }
                Action::ToggleTodos if self.state != AppState::Confirming => {
                    // Toggle todos: show as system message for now
                    self.messages.push(ChatMessage::System {
                        text: "[todos panel toggled]".into(),
                    });
                    return AppAction::None;
                }
                Action::NextSession if self.state != AppState::Confirming => {
                    if let Some(next_id) = self.session_sidebar.next_session_id() {
                        return AppAction::SwitchSession(next_id);
                    }
                    return AppAction::None;
                }
                Action::PrevSession if self.state != AppState::Confirming => {
                    if let Some(prev_id) = self.session_sidebar.prev_session_id() {
                        return AppAction::SwitchSession(prev_id);
                    }
                    return AppAction::None;
                }
                Action::ExternalEditor if self.state != AppState::Confirming => {
                    // Hand the current input text off to the runner. The runner
                    // pauses the EventBroker, spawns `$EDITOR` against a tempfile
                    // seeded with this text, and on exit injects
                    // `AppEvent::ExternalEditorClosed(text)` back into the app.
                    return AppAction::ExternalEditor(self.input.text());
                }
                Action::ToggleVimMode if self.state != AppState::Confirming => {
                    self.vim.toggle();
                    let label = if self.vim.is_enabled() { "ON" } else { "OFF" };
                    self.messages.push(ChatMessage::System {
                        text: format!("[vim mode {label}]"),
                    });
                    return AppAction::None;
                }
                Action::ImagePaste if self.state != AppState::Confirming => {
                    self.messages.push(ChatMessage::System {
                        text: "[image paste: clipboard image not available]".into(),
                    });
                    return AppAction::None;
                }
                _ => {} // Fall through for non-matching states
            }
        }

        // '/' key activates search when idle/waiting and input is empty
        if (self.state == AppState::Idle || self.state == AppState::WaitingForInput)
            && key.code == KeyCode::Char('/')
            && key.modifiers.is_empty()
            && self.input.is_empty()
        {
            self.search.activate();
            return AppAction::None;
        }

        // 'y' key copies focused code block when idle and input is empty
        if self.state == AppState::Idle
            && key.code == KeyCode::Char('y')
            && key.modifiers.is_empty()
            && self.input.is_empty()
        {
            self.code_blocks.update(&self.content_buffer);
            if let Some(text) = self.code_blocks.copy_focused() {
                match self.clipboard.copy(&text) {
                    Ok(()) => {
                        let _ = write!(
                            self.content_buffer,
                            "\n[copied {} bytes to clipboard]",
                            text.len()
                        );
                    }
                    Err(e) => {
                        let _ = write!(self.content_buffer, "\n[copy failed: {e}]");
                    }
                }
            }
            return AppAction::None;
        }

        // Enter toggles fold when idle, input is empty, and there are tool outputs
        if self.state == AppState::Idle
            && key.code == KeyCode::Enter
            && key.modifiers.is_empty()
            && self.input.is_empty()
            && !self.tool_outputs.is_empty()
        {
            self.tool_outputs.toggle_selected();
            self.toggle_last_tool_result_collapsed();
            return AppAction::None;
        }

        match self.state {
            AppState::Confirming => self.handle_confirming_key(key),
            AppState::Initializing => AppAction::None,
            AppState::Processing => self.handle_processing_key(key),
            AppState::Idle | AppState::WaitingForInput => {
                // Switch to WaitingForInput on first keystroke
                if self.state == AppState::Idle {
                    self.state = AppState::WaitingForInput;
                }

                // Reset scroll to bottom on new input
                self.content_scroll = 0;
                self.scroll_anchor = None;
                self.unseen_message_count = 0;

                // ── Autocomplete popup is active ──
                if self.autocomplete.is_active() {
                    match key.code {
                        KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
                            self.autocomplete.prev();
                            return AppAction::None;
                        }
                        KeyCode::Tab | KeyCode::Down => {
                            self.autocomplete.next();
                            return AppAction::None;
                        }
                        KeyCode::Up => {
                            self.autocomplete.prev();
                            return AppAction::None;
                        }
                        KeyCode::Enter => {
                            if let Some((token, replacement)) = self.autocomplete.accept() {
                                let text = self.input.text();
                                let new_text = text.replacen(&token, &replacement, 1);
                                self.input.set_text(&new_text);
                            }
                            return AppAction::None;
                        }
                        KeyCode::Esc => {
                            self.autocomplete.dismiss();
                            return AppAction::None;
                        }
                        _ => {
                            // Any other key dismisses autocomplete and falls through
                            self.autocomplete.dismiss();
                        }
                    }
                }

                // ── Tab triggers autocomplete ──
                if key.code == KeyCode::Tab && !self.input.is_empty() {
                    let text = self.input.text();
                    let (_, col) = self.input.cursor();
                    let count = self.autocomplete.complete(&text, col);
                    if count > 0 {
                        return AppAction::None;
                    }
                    // No completions — fall through (don't insert tab)
                    return AppAction::None;
                }

                // Enter (without shift) submits
                if key.code == KeyCode::Enter && !key.modifiers.contains(KeyModifiers::SHIFT) {
                    if !self.input.is_empty() {
                        let text = self.input.submit();
                        // Track in history for Ctrl+R history search
                        self.input_history_list.push(text.clone());
                        self.messages.push(ChatMessage::User { text: text.clone() });
                        self.state = AppState::Processing;
                        self.spinner.start_with_random_verb();
                        return AppAction::Submit(text);
                    }
                    return AppAction::None;
                }

                if self.vim.is_enabled() {
                    match self.vim.handle_key(key, &mut self.input) {
                        VimAction::Consumed => {}
                        VimAction::Submit => {
                            if !self.input.is_empty() {
                                let text = self.input.submit();
                                self.input_history_list.push(text.clone());
                                self.messages.push(ChatMessage::User { text: text.clone() });
                                self.state = AppState::Processing;
                                self.spinner.start_with_random_verb();
                                return AppAction::Submit(text);
                            }
                        }
                        VimAction::Ignored => {
                            self.input.handle_key(key);
                        }
                    }
                } else {
                    self.input.handle_key(key);
                }
                AppAction::None
            }
        }
    }

    /// Handle keystrokes in search mode.
    fn handle_search_key(&mut self, key: crossterm::event::KeyEvent) -> AppAction {
        match key.code {
            KeyCode::Esc => {
                self.search.deactivate();
            }
            KeyCode::Enter => {
                // Move to next match and exit search mode
                self.search.next_match();
                self.scroll_to_search_match();
                self.search.deactivate();
            }
            KeyCode::Backspace => {
                self.search.pop_char();
                self.search.search(&self.content_buffer);
            }
            KeyCode::Char(c)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                self.search.push_char(c);
                self.search.search(&self.content_buffer);
            }
            _ => {}
        }
        AppAction::None
    }

    /// Handle keystrokes while the agent is processing.
    ///
    /// The user can type ahead and press Enter to queue commands.
    /// Queued commands are auto-submitted after the current turn finishes.
    fn handle_processing_key(&mut self, key: crossterm::event::KeyEvent) -> AppAction {
        if key.code == KeyCode::Enter && !key.modifiers.contains(KeyModifiers::SHIFT) {
            if !self.input.is_empty() {
                let text = self.input.submit();
                self.input_history_list.push(text.clone());
                self.command_queue.push(text);
                self.notifications
                    .info(format!("Queued ({} pending)", self.command_queue.len()));
            }
            return AppAction::None;
        }
        self.input.handle_key(key);
        AppAction::None
    }

    /// Scroll content to show the current search match.
    fn scroll_to_search_match(&mut self) {
        if let Some(m) = self.search.current() {
            let total_lines = self.content_buffer.lines().count();
            let from_bottom = total_lines.saturating_sub(m.line + 1);
            self.content_scroll = from_bottom;
        }
    }

    fn handle_confirming_key(&mut self, key: crossterm::event::KeyEvent) -> AppAction {
        // Ctrl+E / Ctrl+D toggle the current pending approval's explanation / debug panels.
        if key.modifiers == KeyModifiers::CONTROL {
            match key.code {
                KeyCode::Char('e') => {
                    if let Some(current) = self.approval_queue.current_mut() {
                        current.toggle_explanation();
                    }
                    return AppAction::None;
                }
                KeyCode::Char('d') => {
                    if let Some(current) = self.approval_queue.current_mut() {
                        current.toggle_debug();
                    }
                    return AppAction::None;
                }
                _ => {}
            }
        }

        let current_info = self.approval_queue.current().map(|pa| {
            (
                pa.card.kind.tool_name().to_string(),
                pa.card.rejection_summary(),
            )
        });

        if let Some((request_id, response)) = self.approval_queue.handle_key(key.code) {
            let allowed = matches!(
                response,
                PermissionResponse::Allow | PermissionResponse::AllowAlways
            );
            if response == PermissionResponse::AllowAlways
                && let Some((ref grant_name, _)) = current_info
            {
                self.session_grants.insert(grant_name.clone());
            }
            let rejection_info = current_info.map(|(_, ri)| ri);
            if !allowed && let Some((tool_name, summary)) = rejection_info {
                let tool_input = self
                    .active_tools
                    .values()
                    .find(|info| info.name == tool_name)
                    .map(|info| &info.input);
                let display = self
                    .tool_registry
                    .as_ref()
                    .and_then(|reg| reg.get(&tool_name))
                    .and_then(|tool| {
                        tool.format_rejected(tool_input.unwrap_or(&serde_json::Value::Null))
                    });
                self.messages.push(ChatMessage::ToolRejected {
                    tool_name,
                    summary,
                    display,
                });
            }
            if self.approval_queue.is_empty() {
                self.state = AppState::Processing;
                if allowed {
                    self.spinner.start_with_random_verb();
                }
            }
            return AppAction::PermissionResponse {
                request_id,
                allowed,
            };
        }
        AppAction::None
    }

    /// Translate a `TuiEvent` into zero or more `AppEvent`s.
    ///
    /// Pure translation — no state mutation, no registry lookups. Registry-
    /// dependent work (tool result formatting, summary rendering) is done in
    /// `apply_event` instead, where `&mut self` gives access to both state
    /// and `tool_registry`.
    ///
    /// Key events (`TuiEvent::Key`) are NOT translated here — they go through
    /// `handle_key` directly because their interpretation depends on complex
    /// state (overlay stack, search mode, autocomplete, `AppState`). A later
    /// task will migrate key routing to `AppEvent` too.
    #[allow(clippy::unused_self)]
    pub fn translate_event(&self, event: &TuiEvent) -> Vec<crate::app_event::AppEvent> {
        use crate::app_event::AppEvent;
        use crab_core::event::Event;

        match event {
            TuiEvent::Tick => vec![AppEvent::Tick],
            TuiEvent::Resize { width, height } => vec![AppEvent::Resize(*width, *height)],
            TuiEvent::Paste(text) => vec![AppEvent::Paste(text.clone())],
            TuiEvent::Key(_) => {
                // Key translation is complex (depends on state, search, autocomplete).
                // For now, key events go through the existing handle_key path.
                Vec::new()
            }
            TuiEvent::Agent(agent_event) => match agent_event {
                Event::ContentDelta { index, delta } => {
                    // Skip tool-argument content blocks (indices >= TOOL_ARG_INDEX_BASE)
                    // to avoid leaking raw tool-call JSON into the assistant message.
                    // See `crab_core::event::TOOL_ARG_INDEX_BASE` for background.
                    if *index >= crab_core::event::TOOL_ARG_INDEX_BASE {
                        Vec::new()
                    } else {
                        vec![AppEvent::ContentAppend(delta.clone())]
                    }
                }
                Event::MessageEnd { usage, .. } => {
                    vec![AppEvent::MessageComplete {
                        input_tokens: usage.input_tokens,
                        output_tokens: usage.output_tokens,
                    }]
                }
                Event::ToolUseStart { id, name, input } => {
                    vec![AppEvent::ToolStart {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    }]
                }
                Event::ToolProgress { id, progress } => {
                    vec![AppEvent::ToolProgress {
                        id: id.clone(),
                        progress: progress.clone(),
                    }]
                }
                Event::ToolResult { id, output } => {
                    vec![AppEvent::ToolFinished {
                        id: id.clone(),
                        output: output.clone(),
                    }]
                }
                Event::PermissionRequest {
                    request_id,
                    tool_name,
                    input_summary,
                } => {
                    vec![AppEvent::PermissionRequested {
                        request_id: request_id.clone(),
                        tool_name: tool_name.clone(),
                        summary: input_summary.clone(),
                    }]
                }
                Event::CompactStart { strategy, .. } => {
                    vec![AppEvent::CompactStart {
                        strategy: strategy.clone(),
                    }]
                }
                Event::CompactEnd {
                    after_tokens,
                    removed_messages,
                } => {
                    vec![AppEvent::CompactEnd {
                        after_tokens: *after_tokens,
                        removed_messages: *removed_messages,
                    }]
                }
                Event::TokenWarning {
                    usage_pct,
                    used,
                    limit,
                } => {
                    vec![AppEvent::TokenWarning {
                        usage_pct: f64::from(*usage_pct),
                        used: *used,
                        limit: *limit,
                    }]
                }
                Event::SessionSaved { session_id } => {
                    vec![AppEvent::SessionSaved {
                        session_id: session_id.clone(),
                    }]
                }
                Event::SessionResumed {
                    session_id,
                    message_count,
                } => {
                    vec![AppEvent::SessionResumed {
                        session_id: session_id.clone(),
                        message_count: *message_count,
                    }]
                }
                Event::Error { message } => {
                    vec![AppEvent::AgentError(message.clone())]
                }
                Event::ThinkingDelta { delta, .. } => {
                    vec![AppEvent::ThinkingAppend(delta.clone())]
                }
                // Events with no TUI representation today — dropped silently.
                // Candidates for future AppEvent variants:
                //   TurnStart, MessageStart,
                //   ToolUseInput, ToolOutputDelta, PermissionResponse,
                //   MemoryLoaded, MemorySaved,
                //   AgentWorkerStarted, AgentWorkerCompleted
                _ => Vec::new(),
            },
        }
    }

    /// Apply a single `AppEvent` to mutate state and optionally produce an `AppAction`.
    ///
    /// This is the state-mutation half of the event bus pattern.
    ///
    /// `#[allow(clippy::match_same_arms)]`: the no-op catch-all legitimately
    /// groups many unrelated variants under a single `AppAction::None` return
    /// (pending key-event migration). Clippy's suggestion to merge them with
    /// `Redraw` would erase the semantic distinction between "genuine no-op"
    /// and "not yet wired up", which is load-bearing for the WHY comments.
    #[allow(clippy::match_same_arms)]
    pub fn apply_event(&mut self, event: crate::app_event::AppEvent) -> AppAction {
        use crate::app_event::AppEvent;
        match event {
            AppEvent::Tick => {
                self.spinner.tick();
                self.notifications.tick();
                if let ThinkingState::ThoughtFor { finished_at, .. } = self.thinking
                    && finished_at.elapsed() >= ThinkingState::DISPLAY_DURATION
                {
                    self.thinking = ThinkingState::Idle;
                }
                AppAction::None
            }
            AppEvent::Resize(..) => AppAction::None,
            AppEvent::ContentAppend(delta) => {
                if matches!(self.thinking, ThinkingState::Thinking { .. }) {
                    self.set_thinking(false);
                    let dur = if let ThinkingState::ThoughtFor { duration, .. } = &self.thinking {
                        Some(*duration)
                    } else {
                        None
                    };
                    if let Some(dur) = dur {
                        for msg in self.messages.iter_mut().rev() {
                            if let ChatMessage::Thinking { duration: d, .. } = msg {
                                *d = Some(dur);
                                break;
                            }
                        }
                    }
                }
                if let Some(ChatMessage::Assistant { text }) = self.messages.last_mut() {
                    text.push_str(&delta);
                } else {
                    self.messages.push(ChatMessage::Assistant {
                        text: delta.clone(),
                    });
                }
                // Mirror the delta into `content_buffer` so the legacy
                // flat-string readers still see it. After #13 the render
                // path iterates `self.messages` directly, but Ctrl+F search,
                // Ctrl+Y code-block copy, and the scroll-anchor math at
                // app.rs:399/701/994 still read `content_buffer`. Until
                // ticket #27 rewrites those read sites to iterate
                // `self.messages`, this mirror keeps those features alive.
                // Tracked by `apply_event_content_append_mirrors_into_content_buffer`.
                self.content_buffer.push_str(&delta);
                if self.scroll_anchor.is_some() {
                    let new_lines = delta.chars().filter(|&c| c == '\n').count();
                    self.unseen_message_count =
                        self.unseen_message_count.saturating_add(new_lines.max(1));
                } else {
                    self.content_scroll = 0;
                }
                self.spinner.response_tokens += (delta.len() as u64).div_ceil(4);
                AppAction::None
            }
            AppEvent::ToolStart { id, name, input } => {
                let tool_ref = self
                    .tool_registry
                    .as_ref()
                    .and_then(|reg| reg.get(&name));
                let summary = tool_ref.and_then(|t| t.format_use_summary(&input));
                let color = tool_ref.map(|t| t.display_color());
                self.active_tools.insert(
                    id,
                    ActiveToolInfo {
                        name: name.clone(),
                        input,
                        progress: None,
                    },
                );
                let is_read_only = tool_ref.is_some_and(|t| t.is_read_only());
                self.messages.push(ChatMessage::ToolUse {
                    name: name.clone(),
                    summary,
                    color,
                    is_read_only,
                });
                self.spinner.set_message(format!("Running {name}…"));
                if self.processing_start.is_none() {
                    self.processing_start = Some(Instant::now());
                }
                AppAction::None
            }
            AppEvent::ToolProgress { id, progress } => {
                if let Some(info) = self.active_tools.get_mut(&id) {
                    info.progress = Some(progress);
                }
                AppAction::None
            }
            AppEvent::ToolFinished { id, output } => {
                let removed = self.active_tools.remove(&id);
                let tool_name = removed
                    .as_ref()
                    .map(|info| info.name.clone())
                    .unwrap_or_default();
                let tool_input = removed.map(|info| info.input);
                self.spinner.clear_override();
                let tool_ref = self
                    .tool_registry
                    .as_ref()
                    .and_then(|reg| reg.get(&tool_name));
                let display = if output.is_error {
                    let input = tool_input.as_ref().unwrap_or(&serde_json::Value::Null);
                    tool_ref
                        .and_then(|tool| tool.format_error(&output, input))
                        .or_else(|| tool_ref.and_then(|tool| tool.format_result(&output)))
                } else {
                    tool_ref.and_then(|tool| tool.format_result(&output))
                };
                let text = output.text();
                let is_error = output.is_error;
                let collapsed = tool_ref.is_some_and(|t| t.is_result_collapsible(&output));
                let is_read_only = tool_ref.is_some_and(|t| t.is_read_only());
                self.messages.push(ChatMessage::ToolResult {
                    tool_name: tool_name.clone(),
                    output: text.clone(),
                    is_error,
                    display,
                    collapsed,
                    is_read_only,
                });
                self.tool_outputs
                    .push(ToolOutputEntry::new(&tool_name, text.clone(), is_error));
                if is_error {
                    let mut section =
                        CollapsibleSection::new(format!("Tool error: {tool_name}"), text);
                    section.collapsed = true;
                    self.context_collapse.push_section(section);
                } else if text.lines().count() > 5 {
                    let mut section =
                        CollapsibleSection::new(format!("Tool output: {tool_name}"), text);
                    section.collapsed = true;
                    self.context_collapse.push_section(section);
                }
                AppAction::None
            }
            AppEvent::MessageComplete {
                input_tokens,
                output_tokens,
            } => {
                self.spinner.stop();
                self.active_tools.clear();
                self.state = AppState::Idle;
                self.total_input_tokens += input_tokens;
                self.total_output_tokens += output_tokens;
                if let Some(start) = self.processing_start.take()
                    && start.elapsed() > Duration::from_secs(10)
                {
                    crate::terminal_notify::notify("Crab Code", "Task completed");
                }
                AppAction::None
            }
            AppEvent::AgentError(message) => {
                self.spinner.stop();
                self.active_tools.clear();
                self.state = AppState::Idle;
                self.processing_start = None;
                self.messages.push(ChatMessage::System {
                    text: format!("Error: {message}"),
                });
                self.notifications.error(&message);
                crate::terminal_notify::notify("Crab Code", "Agent error");
                AppAction::None
            }
            AppEvent::PermissionRequested {
                request_id,
                tool_name,
                summary,
            } => {
                if self.session_grants.contains(&tool_name) {
                    AppAction::PermissionResponse {
                        request_id,
                        allowed: true,
                    }
                } else {
                    self.spinner.pause();
                    self.state = AppState::Confirming;
                    self.approval_queue.push(PermissionCard::from_event(
                        &tool_name,
                        &summary,
                        request_id,
                    ));
                    AppAction::None
                }
            }
            AppEvent::ScrollUp(n) => {
                self.content_scroll = self.content_scroll.saturating_add(n as usize);
                let total = self.virtual_list.last_total_lines();
                self.scroll_anchor = Some(total.saturating_sub(self.content_scroll));
                AppAction::None
            }
            AppEvent::ScrollDown(n) => {
                self.content_scroll = self.content_scroll.saturating_sub(n as usize);
                if self.content_scroll == 0 {
                    self.scroll_anchor = None;
                    self.unseen_message_count = 0;
                }
                AppAction::None
            }
            AppEvent::ScrollToBottom => {
                self.content_scroll = 0;
                self.scroll_anchor = None;
                self.unseen_message_count = 0;
                AppAction::None
            }
            AppEvent::ToggleSidebar => {
                self.sidebar_visible = !self.sidebar_visible;
                self.session_sidebar.visible = self.sidebar_visible;
                AppAction::None
            }
            AppEvent::ToggleFold => {
                self.tool_outputs.toggle_selected();
                self.toggle_last_tool_result_collapsed();
                AppAction::None
            }
            AppEvent::CyclePermissionMode => {
                use crab_core::permission::PermissionMode;
                self.permission_mode = match self.permission_mode {
                    PermissionMode::Default => PermissionMode::AcceptEdits,
                    PermissionMode::AcceptEdits => PermissionMode::Plan,
                    _ => PermissionMode::Default,
                };
                AppAction::None
            }
            AppEvent::OpenSearch => {
                self.search.activate();
                AppAction::None
            }
            AppEvent::CloseSearch => {
                self.search.deactivate();
                AppAction::None
            }
            AppEvent::NewSession => AppAction::NewSession,
            AppEvent::SwitchSession(id) => AppAction::SwitchSession(id),
            AppEvent::SwitchModel(model) => {
                self.messages.push(ChatMessage::System {
                    text: format!("[model switched to {model}]"),
                });
                self.model_name = model;
                AppAction::None
            }
            AppEvent::Quit => {
                self.should_quit = true;
                AppAction::Quit
            }
            AppEvent::CompactStart { .. } => {
                AppAction::None
            }
            AppEvent::CompactEnd {
                after_tokens,
                removed_messages,
            } => {
                self.messages.push(ChatMessage::CompactBoundary {
                    strategy: "summary".into(),
                    after_tokens,
                    removed_messages,
                });
                self.notifications.success("Context compacted");
                AppAction::None
            }
            AppEvent::TokenWarning {
                usage_pct,
                used,
                limit,
            } => {
                self.messages.push(ChatMessage::System {
                    text: format!("Token usage {:.0}% ({used}/{limit})", usage_pct * 100.0),
                });
                AppAction::None
            }
            AppEvent::SessionSaved { session_id } => {
                self.messages.push(ChatMessage::System {
                    text: format!("Session saved: {session_id}"),
                });
                AppAction::None
            }
            AppEvent::SessionResumed {
                session_id,
                message_count,
            } => {
                self.messages.push(ChatMessage::System {
                    text: format!("Resumed {session_id} ({message_count} messages)"),
                });
                AppAction::None
            }
            AppEvent::ThinkingChanged { active } => {
                self.set_thinking(active);
                if !active {
                    let dur = if let ThinkingState::ThoughtFor { duration, .. } = &self.thinking {
                        Some(*duration)
                    } else {
                        None
                    };
                    if let Some(dur) = dur {
                        for msg in self.messages.iter_mut().rev() {
                            if let ChatMessage::Thinking { duration: d, .. } = msg {
                                *d = Some(dur);
                                break;
                            }
                        }
                    }
                }
                AppAction::None
            }
            AppEvent::ThinkingAppend(delta) => {
                if !matches!(self.thinking, ThinkingState::Thinking { .. }) {
                    self.set_thinking(true);
                }
                if let Some(ChatMessage::Thinking { text, .. }) = self.messages.last_mut() {
                    text.push_str(&delta);
                } else {
                    self.messages.push(ChatMessage::Thinking {
                        text: delta,
                        collapsed: true,
                        duration: None,
                    });
                }
                AppAction::None
            }
            // Both variants replace the input box contents outright; they
            // differ only in provenance (history-search pick vs. external
            // editor result) which does not matter at the state-mutation layer.
            AppEvent::InsertInputText(text) | AppEvent::ExternalEditorClosed(text) => {
                self.input.set_text(&text);
                AppAction::None
            }
            // Bracketed paste: insert at cursor regardless of AppState so the
            // Processing-state command-queue type-ahead works too.
            AppEvent::Paste(text) => {
                self.input.insert_text(&text);
                AppAction::None
            }
            // Genuine no-op: the renderer always draws on the next frame, so
            // there is no state to mutate here. Kept as an explicit variant
            // so key bindings can still emit it as a signal.
            AppEvent::Redraw => AppAction::None,

            AppEvent::TrustAccepted { project_path } => {
                self.overlay_stack.pop();
                if let Err(e) = persist_trust_accepted(&project_path) {
                    self.notifications
                        .warn(format!("Failed to save trust state: {e}"));
                }
                AppAction::None
            }
            AppEvent::TrustDenied => {
                self.overlay_stack.pop();
                self.notifications
                    .warn("Project settings skipped (bare mode)".to_string());
                AppAction::None
            }
            AppEvent::SettingsReloaded { warnings } => {
                if warnings.is_empty() {
                    self.notifications.success("Settings reloaded");
                } else {
                    for w in &warnings {
                        self.notifications.warn(w.clone());
                    }
                    self.notifications.info("Settings reloaded with warnings");
                }
                AppAction::None
            }
            AppEvent::SkillsReloaded { count } => {
                self.notifications
                    .info(format!("Skills reloaded ({count} discovered)"));
                AppAction::None
            }

            // Pending key-event migration: these variants exist in the
            // vocabulary but are NOT yet emitted by any AppEvent producer.
            // The key-event path (`handle_key` / `handle_confirming_key`)
            // still interprets the matching keys directly and returns the
            // corresponding `AppAction` inline, so the bus never sees them.
            // A future task will move key translation into the bus, at
            // which point each of these arms needs a real handler.
            //
            // Input lifecycle (submitted/cancelled via InputBox key path)
            AppEvent::InputSubmit(_)
            | AppEvent::InputCancel
            // Permission response (handle_confirming_key emits AppAction::PermissionResponse directly)
            | AppEvent::PermissionAllow(_)
            | AppEvent::PermissionDeny(_)
            | AppEvent::PermissionAllowAlways(_)
            // Overlay open/close (handle_key pushes overlays directly onto overlay_stack)
            | AppEvent::OpenCommandPalette
            | AppEvent::OpenHistorySearch
            | AppEvent::OpenModelPicker
            | AppEvent::OpenTranscript
            | AppEvent::CloseOverlay
            | AppEvent::OpenDiffViewer { .. }
            // Content actions (handle_key mutates state directly)
            | AppEvent::CopyCodeBlock
            | AppEvent::ExternalEditorOpen
            | AppEvent::Stash
            | AppEvent::KillAgents
            | AppEvent::Undo
            | AppEvent::ToggleTodos
            | AppEvent::ImagePaste => AppAction::None,

        }
    }

    /// Render the full app into a ratatui frame.
    ///
    /// Delegates to `Renderable` components (Phase 1 refactor).
    pub fn render(&mut self, area: Rect, buf: &mut Buffer) {
        #[allow(clippy::cast_possible_truncation)]
        let layout = AppLayout::compute_with_sidebar(
            area,
            self.input.line_count() as u16,
            self.sidebar_visible,
            crate::layout::DEFAULT_SIDEBAR_WIDTH,
        );

        // Header — delegated to HeaderBar Renderable
        let header = HeaderBar {
            model_name: &self.model_name,
            working_dir: &self.working_dir,
        };
        header.render(layout.header, buf);

        // Session sidebar
        if let Some(sidebar_area) = layout.sidebar {
            Widget::render(&self.session_sidebar, sidebar_area, buf);
        }

        // Content area — virtualized message list with LRU cache
        if layout.content.width != self.last_render_width {
            self.virtual_list.invalidate();
            self.last_render_width = layout.content.width;
        }
        let is_streaming = self.state == AppState::Processing;
        let vm: Vec<VirtualMessage> = self
            .messages
            .iter()
            .enumerate()
            .map(|(i, msg)| {
                let cell = crate::history::cell_from_chat_message(msg);
                let lines = cell.display_lines(layout.content.width);
                let mut vm = VirtualMessage::new(i as u64, lines);
                if is_streaming && i + 1 == self.messages.len() {
                    vm = vm.streaming();
                }
                vm
            })
            .collect();
        let vp = ViewportState {
            scroll_offset: self.content_scroll,
        };
        self.virtual_list.render(&vm, vp, layout.content, buf);

        if self.spinner.is_active() {
            Widget::render(&self.spinner, layout.status, buf);
        }

        // Separators
        crate::components::header::render_separator(layout.separator_top, buf);

        // Input — delegated to InputArea Renderable
        let input_area = InputArea {
            input: &self.input,
            mode: self.input_mode,
        };
        input_area.render(layout.input, buf);

        crate::components::header::render_separator(layout.separator_bottom, buf);

        // Unseen message divider
        if self.scroll_anchor.is_some() && self.unseen_message_count > 0 {
            let divider_y = layout.content.y + layout.content.height.saturating_sub(2);
            if divider_y > layout.content.y {
                render_unseen_divider(
                    self.unseen_message_count,
                    Rect {
                        x: layout.content.x,
                        y: divider_y,
                        width: layout.content.width,
                        height: 1,
                    },
                    buf,
                );
            }
        }

        // Search bar
        if self.search.is_active() {
            let search_area = Rect {
                x: layout.content.x,
                y: layout.content.y + layout.content.height.saturating_sub(1),
                width: layout.content.width,
                height: 1,
            };
            // Theme is not yet threaded through App — use default (dark)
            // to preserve byte-identical output. When App grows a theme
            // field, replace this with a reference to it.
            let theme = crate::theme::Theme::default();
            search::render_search_bar(&self.search, &theme, search_area, buf);
        }

        // Bottom bar — delegated to BottomBar Renderable.
        // Surface any in-flight chord prefix so the user sees "Ctrl+K …"
        // after the first key of a chord binding.
        let vim_label = if self.vim.is_enabled() {
            Some(self.vim.mode().label())
        } else {
            None
        };
        let exit_pending = self
            .last_interrupt
            .is_some_and(|t| t.elapsed() < Duration::from_millis(800));
        let bottom_bar = BottomBar {
            state: self.state,
            search_active: self.search.is_active(),
            permission_mode: self.permission_mode,
            chord_prefix: self.keybindings.pending_chord(),
            vim_mode: vim_label,
            exit_pending,
        };
        bottom_bar.render(layout.bottom_bar, buf);

        // Autocomplete popup
        if self.autocomplete.is_active() {
            render_autocomplete_popup(&self.autocomplete, layout.input, buf);
        }

        // Permission card(s) — rendered inline at bottom of content area
        // from the FIFO approval queue. Clear the card area first to prevent
        // overlap with message text.
        if let Some(pending) = self.approval_queue.current() {
            let card_lines = pending.card.render_lines(layout.content.width);
            let card_height = (card_lines.len() as u16).min(layout.content.height);
            let card_area = Rect {
                x: layout.content.x,
                y: layout.content.y + layout.content.height - card_height,
                width: layout.content.width,
                height: card_height,
            };
            // Clear the card area background to prevent text overlap
            for y in card_area.y..card_area.y + card_area.height {
                for x in card_area.x..card_area.x + card_area.width {
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        cell.reset();
                    }
                }
            }
            for (i, line) in card_lines.iter().enumerate() {
                if i >= card_height as usize {
                    break;
                }
                let line_area = Rect {
                    x: card_area.x,
                    y: card_area.y + i as u16,
                    width: card_area.width,
                    height: 1,
                };
                let paragraph = Paragraph::new(line.clone());
                Widget::render(paragraph, line_area, buf);
            }
        }

        // Toast notifications — rendered above bottom bar
        if self.notifications.has_active() {
            let toast_height = self.notifications.visible().len() as u16;
            let toast_y = layout.bottom_bar.y.saturating_sub(toast_height);
            let toast_area = Rect {
                x: layout.content.x,
                y: toast_y,
                width: layout.content.width,
                height: toast_height,
            };
            Widget::render(
                crate::components::notification::ToastRenderer::new(&self.notifications),
                toast_area,
                buf,
            );
        }

        // Overlay stack (renders on top of everything)
        if !self.overlay_stack.is_empty() {
            self.overlay_stack.render(area, buf);
        }
    }
}

// Old render functions (render_header, shorten_path, render_separator)
// have been extracted to components/header.rs — see HeaderBar.
// Agent-event translation used to live here as a free `translate_agent_event`
// function; it has been folded into `App::translate_event` so it can share
// a single entry point with `Tick`/`Resize` translation.

/// Render the unseen message divider when the user is scrolled up.
///
/// Displays something like `"─── 5 new messages ───"` centered in the area.
#[allow(clippy::cast_possible_truncation)]
fn render_unseen_divider(count: usize, area: Rect, buf: &mut Buffer) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let label = if count == 1 {
        " 1 new message ".to_string()
    } else {
        format!(" {count} new messages ")
    };

    let total_width = area.width as usize;
    let label_len = label.len();
    let side = total_width.saturating_sub(label_len) / 2;
    let left_dashes = "\u{2500}".repeat(side);
    let right_dashes = "\u{2500}".repeat(total_width.saturating_sub(side + label_len));

    let line = Line::from(vec![
        Span::styled(&*left_dashes, Style::default().fg(Color::DarkGray)),
        Span::styled(
            label,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(&*right_dashes, Style::default().fg(Color::DarkGray)),
    ]);
    Widget::render(line, area, buf);
}

// Old render_input_with_prompt extracted to components/input_area.rs — see InputArea.

// Old render_messages extracted to components/message_list.rs — see MessageList.

// Old render_content_scrolled, strip_trailing_tool_json, is_system_line,
// classify_content_style extracted to components/message_list.rs.

/// Render the autocomplete popup above the input area.
#[allow(clippy::cast_possible_truncation)]
fn render_autocomplete_popup(ac: &AutoComplete, input_area: Rect, buf: &mut Buffer) {
    let candidates = ac.candidates();
    if candidates.is_empty() {
        return;
    }

    let max_visible = 8.min(candidates.len());
    let popup_height = max_visible as u16;
    let popup_width = input_area.width.min(60);

    // Position above the input area
    let popup_y = input_area.y.saturating_sub(popup_height);
    let popup_area = Rect {
        x: input_area.x,
        y: popup_y,
        width: popup_width,
        height: popup_height,
    };

    let selected_idx = ac.selected_index().unwrap_or(0);

    // Scroll if needed so selected item is visible
    let scroll_offset = if selected_idx >= max_visible {
        selected_idx - max_visible + 1
    } else {
        0
    };

    for (i, candidate) in candidates
        .iter()
        .skip(scroll_offset)
        .take(max_visible)
        .enumerate()
    {
        let y = popup_area.y + i as u16;
        let is_selected = (i + scroll_offset) == selected_idx;

        let style = if is_selected {
            Style::default().fg(Color::Black).bg(Color::Cyan)
        } else {
            Style::default().fg(Color::White).bg(Color::DarkGray)
        };

        // Render candidate text + description
        let desc_width = (popup_width as usize).saturating_sub(candidate.text.len() + 3);
        let desc = if candidate.description.len() > desc_width {
            &candidate.description[..desc_width]
        } else {
            &candidate.description
        };

        let line = Line::from(vec![
            Span::styled(&candidate.text, style.add_modifier(Modifier::BOLD)),
            Span::styled(
                format!(
                    " {desc:>width$}",
                    desc = desc,
                    width = popup_width as usize - candidate.text.len() - 1
                ),
                style,
            ),
        ]);

        let line_area = Rect {
            x: popup_area.x,
            y,
            width: popup_width,
            height: 1,
        };
        Widget::render(line, line_area, buf);
    }
}

// Old render_status_line and format_token_count extracted to
// components/status_line.rs — see StatusLine.

// Old render_bottom_bar extracted to components/bottom_bar.rs — see BottomBar.

fn persist_trust_accepted(project_path: &str) -> anyhow::Result<()> {
    let mut state = crab_config::global_state::load();
    crab_config::global_state::record_trust(&mut state, std::path::Path::new(project_path));
    crab_config::global_state::save(&state).map_err(|e| anyhow::anyhow!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> TuiEvent {
        TuiEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn ctrl_key(c: char) -> TuiEvent {
        TuiEvent::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    /// Check if any message in the list contains the given text.
    fn messages_contain(messages: &[ChatMessage], needle: &str) -> bool {
        messages.iter().any(|m| match m {
            ChatMessage::User { text }
            | ChatMessage::Assistant { text }
            | ChatMessage::System { text } => text.contains(needle),
            ChatMessage::ToolUse { name, .. } => name.contains(needle),
            ChatMessage::ToolResult {
                tool_name, output, ..
            } => tool_name.contains(needle) || output.contains(needle),
            ChatMessage::ToolRejected {
                tool_name, summary, ..
            } => tool_name.contains(needle) || summary.contains(needle),
            ChatMessage::Thinking { text, .. } => text.contains(needle),
            ChatMessage::CompactBoundary { .. }
            | ChatMessage::PlanStep { .. }
            | ChatMessage::Welcome { .. } => false,
        })
    }

    #[test]
    fn app_initial_state() {
        let app = App::new("gpt-4o");
        assert_eq!(app.state, AppState::Idle);
        assert!(app.input.is_empty());
        assert!(!app.spinner.is_active());
        assert!(app.messages.is_empty());
        assert_eq!(app.model_name, "gpt-4o");
        assert!(!app.should_quit);
        assert!(!app.sidebar_visible);
        assert!(app.session_id.is_empty());
        assert_eq!(app.total_input_tokens, 0);
        assert_eq!(app.total_output_tokens, 0);
        assert_eq!(app.content_scroll, 0);
        assert!(matches!(app.thinking, ThinkingState::Idle));
        assert!(app.scroll_anchor.is_none());
        assert_eq!(app.input_mode, PromptInputMode::Prompt);
    }

    #[test]
    fn typing_switches_to_waiting_for_input() {
        let mut app = App::new("test");
        app.handle_event(key(KeyCode::Char('h')));
        assert_eq!(app.state, AppState::WaitingForInput);
        assert_eq!(app.input.text(), "h");
    }

    #[test]
    fn enter_submits_message() {
        let mut app = App::new("test");
        app.handle_event(key(KeyCode::Char('h')));
        app.handle_event(key(KeyCode::Char('i')));
        let action = app.handle_event(key(KeyCode::Enter));
        assert_eq!(action, AppAction::Submit("hi".into()));
        assert_eq!(app.state, AppState::Processing);
        assert!(app.spinner.is_active());
    }

    #[test]
    fn enter_on_empty_does_nothing() {
        let mut app = App::new("test");
        let action = app.handle_event(key(KeyCode::Enter));
        assert_eq!(action, AppAction::None);
    }

    #[test]
    fn ctrl_c_single_interrupts() {
        let mut app = App::new("test");
        let action = app.handle_event(ctrl_key('c'));
        // Single Ctrl+C should interrupt, not quit
        assert_eq!(action, AppAction::None);
        assert!(!app.should_quit);
    }

    #[test]
    fn ctrl_c_double_quits() {
        let mut app = App::new("test");
        // First press: interrupt
        app.handle_event(ctrl_key('c'));
        // Second press within 800ms: quit
        let action = app.handle_event(ctrl_key('c'));
        assert_eq!(action, AppAction::Quit);
        assert!(app.should_quit);
    }

    #[test]
    fn ctrl_d_quits() {
        let mut app = App::new("test");
        // Ctrl+D also goes through Quit action (same double-press logic)
        app.handle_event(ctrl_key('d'));
        let action = app.handle_event(ctrl_key('d'));
        assert_eq!(action, AppAction::Quit);
    }

    #[test]
    fn tick_advances_spinner() {
        let mut app = App::new("test");
        app.spinner.start("Working");
        app.handle_event(TuiEvent::Tick);
        assert!(app.spinner.is_active());
    }

    #[test]
    fn agent_content_delta_appends() {
        let mut app = App::new("test");
        app.handle_event(TuiEvent::Agent(crab_core::event::Event::ContentDelta {
            index: 0,
            delta: "Hello ".into(),
        }));
        app.handle_event(TuiEvent::Agent(crab_core::event::Event::ContentDelta {
            index: 0,
            delta: "world".into(),
        }));
        assert!(messages_contain(&app.messages, "Hello world"));
        assert_eq!(app.content_scroll, 0); // auto-scrolled
    }

    #[test]
    fn agent_message_end_stops_spinner() {
        let mut app = App::new("test");
        app.state = AppState::Processing;
        app.spinner.start("Thinking...");

        app.handle_event(TuiEvent::Agent(crab_core::event::Event::MessageEnd {
            usage: crab_core::model::TokenUsage::default(),
        }));

        assert!(!app.spinner.is_active());
        assert_eq!(app.state, AppState::Idle);
    }

    #[test]
    fn agent_message_end_accumulates_tokens() {
        let mut app = App::new("test");
        app.state = AppState::Processing;

        let usage = crab_core::model::TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            ..Default::default()
        };
        app.handle_event(TuiEvent::Agent(crab_core::event::Event::MessageEnd {
            usage,
        }));

        assert_eq!(app.total_input_tokens, 100);
        assert_eq!(app.total_output_tokens, 50);

        // Second turn
        app.state = AppState::Processing;
        let usage2 = crab_core::model::TokenUsage {
            input_tokens: 200,
            output_tokens: 80,
            ..Default::default()
        };
        app.handle_event(TuiEvent::Agent(crab_core::event::Event::MessageEnd {
            usage: usage2,
        }));

        assert_eq!(app.total_input_tokens, 300);
        assert_eq!(app.total_output_tokens, 130);
    }

    #[test]
    fn agent_tool_use_updates_spinner() {
        let mut app = App::new("test");
        app.state = AppState::Processing;
        app.spinner.start("Thinking...");

        app.handle_event(TuiEvent::Agent(crab_core::event::Event::ToolUseStart {
            id: "tu_1".into(),
            name: "bash".into(),
            input: serde_json::json!({"command": "ls"}),
        }));

        assert!(app.spinner.message().contains("bash"));
    }

    #[test]
    fn permission_request_enters_confirming() {
        let mut app = App::new("test");
        app.state = AppState::Processing;

        app.handle_event(TuiEvent::Agent(
            crab_core::event::Event::PermissionRequest {
                tool_name: "bash".into(),
                input_summary: "rm -rf /tmp".into(),
                request_id: "req_1".into(),
            },
        ));

        assert_eq!(app.state, AppState::Confirming);
    }

    #[test]
    fn confirming_y_allows() {
        let mut app = App::new("test");
        app.state = AppState::Confirming;
        app.approval_queue.push(PermissionCard::from_event(
            "bash",
            "rm -rf /tmp",
            "req_1".into(),
        ));

        let action = app.handle_event(key(KeyCode::Char('y')));
        assert_eq!(
            action,
            AppAction::PermissionResponse {
                request_id: "req_1".into(),
                allowed: true,
            }
        );
        assert_eq!(app.state, AppState::Processing);
        assert!(app.approval_queue.is_empty());
    }

    #[test]
    fn confirming_n_denies() {
        let mut app = App::new("test");
        app.state = AppState::Confirming;
        app.approval_queue.push(PermissionCard::from_event(
            "bash",
            "rm -rf /tmp",
            "req_1".into(),
        ));

        let action = app.handle_event(key(KeyCode::Char('n')));
        assert_eq!(
            action,
            AppAction::PermissionResponse {
                request_id: "req_1".into(),
                allowed: false,
            }
        );
    }

    #[test]
    fn confirming_esc_denies() {
        let mut app = App::new("test");
        app.state = AppState::Confirming;
        app.approval_queue.push(PermissionCard::from_event(
            "edit",
            "src/main.rs",
            "req_2".into(),
        ));

        let action = app.handle_event(key(KeyCode::Esc));
        assert_eq!(
            action,
            AppAction::PermissionResponse {
                request_id: "req_2".into(),
                allowed: false,
            }
        );
    }

    #[test]
    fn ctrl_c_in_confirming_rejects_all_and_interrupts() {
        let mut app = App::new("test");
        app.state = AppState::Confirming;
        app.approval_queue.push(PermissionCard::from_event(
            "bash",
            "rm -rf /tmp",
            "req_1".into(),
        ));
        app.approval_queue.push(PermissionCard::from_event(
            "edit",
            "src/main.rs",
            "req_2".into(),
        ));

        let action = app.handle_event(ctrl_key('c'));
        match action {
            AppAction::InterruptPermissions { rejected_ids } => {
                assert_eq!(rejected_ids, vec!["req_1", "req_2"]);
            }
            other => panic!("expected InterruptPermissions, got {other:?}"),
        }
        assert_eq!(app.state, AppState::Idle);
        assert!(app.approval_queue.is_empty());
    }

    #[test]
    fn agent_error_returns_to_idle() {
        let mut app = App::new("test");
        app.state = AppState::Processing;
        app.spinner.start("Working");

        app.handle_event(TuiEvent::Agent(crab_core::event::Event::Error {
            message: "rate limit".into(),
        }));

        assert_eq!(app.state, AppState::Idle);
        assert!(!app.spinner.is_active());
        assert!(messages_contain(&app.messages, "rate limit"));
    }

    #[test]
    fn resize_is_noop() {
        let mut app = App::new("test");
        let action = app.handle_event(TuiEvent::Resize {
            width: 120,
            height: 40,
        });
        assert_eq!(action, AppAction::None);
    }

    #[test]
    fn render_does_not_panic() {
        let mut app = App::new("claude-3.5-sonnet");
        app.set_working_dir("/home/user/project");
        app.content_buffer = "Hello, world!\nLine 2\n".into();
        app.spinner.start("Thinking...");

        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        app.render(area, &mut buf);

        // Header should contain ASCII art crab and "Crab Code"
        let header_text: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(header_text.contains("/\\_/\\") || header_text.contains("Crab"));
    }

    #[test]
    fn tool_result_shown_in_content() {
        let mut app = App::new("test");
        app.state = AppState::Processing;
        app.active_tools.insert(
            "tu_1".into(),
            ActiveToolInfo {
                name: "bash".into(),
                input: serde_json::Value::Null,
                progress: None,
            },
        );

        app.handle_event(TuiEvent::Agent(crab_core::event::Event::ToolResult {
            id: "tu_1".into(),
            output: crab_core::tool::ToolOutput::success("file1.txt\nfile2.txt"),
        }));

        assert!(messages_contain(&app.messages, "file1.txt"));
        assert!(messages_contain(&app.messages, "bash"));
    }

    #[test]
    fn tool_error_shown_in_content() {
        let mut app = App::new("test");
        app.state = AppState::Processing;
        app.active_tools.insert(
            "tu_1".into(),
            ActiveToolInfo {
                name: "bash".into(),
                input: serde_json::Value::Null,
                progress: None,
            },
        );

        app.handle_event(TuiEvent::Agent(crab_core::event::Event::ToolResult {
            id: "tu_1".into(),
            output: crab_core::tool::ToolOutput::error("command not found"),
        }));

        assert!(messages_contain(&app.messages, "command not found"));
        // Verify it's marked as an error
        assert!(
            app.messages
                .iter()
                .any(|m| matches!(m, ChatMessage::ToolResult { is_error: true, .. }))
        );
    }

    #[test]
    fn tool_use_start_shown_in_content() {
        let mut app = App::new("test");
        app.state = AppState::Processing;
        app.spinner.start("Thinking...");

        app.handle_event(TuiEvent::Agent(crab_core::event::Event::ToolUseStart {
            id: "tu_1".into(),
            name: "read".into(),
            input: serde_json::json!({"file_path": "test.rs"}),
        }));

        assert!(messages_contain(&app.messages, "read"));
        assert!(
            app.active_tools
                .get("tu_1")
                .is_some_and(|t| t.name == "read")
        );
    }

    #[test]
    fn permission_card_renders_in_frame() {
        let mut app = App::new("test");
        app.state = AppState::Confirming;
        app.approval_queue.push(PermissionCard::from_event(
            "bash",
            "rm -rf /tmp",
            "req_1".into(),
        ));

        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        app.render(area, &mut buf);

        let buf_ref = &buf;
        let all_text: String = (0..area.height)
            .flat_map(|y| {
                (0..area.width).map(move |x| buf_ref.cell((x, y)).unwrap().symbol().to_string())
            })
            .collect();
        // Card renders inline at bottom of content; verify it renders
        assert!(!all_text.trim().is_empty());
        // Should contain the card title
        assert!(all_text.contains("Bash command"));
    }

    #[test]
    fn permission_kind_classification() {
        use crate::components::permission::PermissionKind;

        let card = PermissionCard::from_event("bash", "ls -la", "r1".into());
        assert!(matches!(card.kind, PermissionKind::Bash { .. }));

        let card = PermissionCard::from_event("edit", "file.rs", "r2".into());
        assert!(matches!(card.kind, PermissionKind::FileEdit { .. }));

        let card = PermissionCard::from_event("write", "out.txt", "r3".into());
        assert!(matches!(card.kind, PermissionKind::FileWrite { .. }));

        let card = PermissionCard::from_event("custom_tool", "data", "r4".into());
        assert!(matches!(card.kind, PermissionKind::Generic { .. }));
    }

    #[test]
    fn session_saved_event_shown() {
        let mut app = App::new("test");
        app.handle_event(TuiEvent::Agent(crab_core::event::Event::SessionSaved {
            session_id: "sess_abc".into(),
        }));
        assert!(messages_contain(&app.messages, "sess_abc"));
    }

    #[test]
    fn token_warning_shown() {
        let mut app = App::new("test");
        app.handle_event(TuiEvent::Agent(crab_core::event::Event::TokenWarning {
            usage_pct: 0.90,
            used: 90000,
            limit: 100_000,
        }));
        assert!(messages_contain(&app.messages, "90%"));
    }

    #[test]
    fn app_state_variants() {
        assert_ne!(AppState::Idle, AppState::WaitingForInput);
        assert_ne!(AppState::Processing, AppState::Confirming);
    }

    #[test]
    fn app_action_variants() {
        assert_eq!(AppAction::None, AppAction::None);
        assert_ne!(AppAction::Quit, AppAction::None);
    }

    // ── New Phase 2 tests ──

    #[test]
    fn ctrl_b_toggles_sidebar() {
        let mut app = App::new("test");
        assert!(!app.sidebar_visible);

        let action = app.handle_event(ctrl_key('b'));
        assert_eq!(action, AppAction::None);
        assert!(app.sidebar_visible);

        let action = app.handle_event(ctrl_key('b'));
        assert_eq!(action, AppAction::None);
        assert!(!app.sidebar_visible);
    }

    #[test]
    fn ctrl_n_creates_new_session() {
        let mut app = App::new("test");
        let action = app.handle_event(ctrl_key('n'));
        assert_eq!(action, AppAction::NewSession);
    }

    #[test]
    fn set_session_id_updates_field() {
        let mut app = App::new("test");
        app.set_session_id("s2");
        assert_eq!(app.session_id, "s2");
    }

    #[test]
    fn set_keybindings_custom() {
        let mut app = App::new("test");
        let kb = Keybindings::defaults();
        app.set_keybindings(kb);
        // Single Ctrl+C should interrupt (not quit with double-press logic)
        let action = app.handle_event(ctrl_key('c'));
        assert_eq!(action, AppAction::None);
        // Double press should quit
        let action = app.handle_event(ctrl_key('c'));
        assert_eq!(action, AppAction::Quit);
    }

    #[test]
    fn render_header_shows_model_and_dir() {
        let mut app = App::new("gpt-4o");
        app.set_working_dir("/home/user/project");

        let area = Rect::new(0, 0, 120, 24);
        let mut buf = Buffer::empty(area);
        app.render(area, &mut buf);

        // Line 1 should show model name
        let line1: String = (0..area.width)
            .map(|x| buf.cell((x, 1)).unwrap().symbol().to_string())
            .collect();
        assert!(line1.contains("gpt-4o"));

        // Line 2 should show working dir
        let line2: String = (0..area.width)
            .map(|x| buf.cell((x, 2)).unwrap().symbol().to_string())
            .collect();
        assert!(line2.contains("project"));
    }

    #[test]
    fn page_up_scrolls_content() {
        let mut app = App::new("test");
        app.content_buffer = (0..100)
            .map(|i| format!("Line {i}"))
            .collect::<Vec<_>>()
            .join("\n");

        let action = app.handle_event(TuiEvent::Key(KeyEvent::new(
            KeyCode::PageUp,
            KeyModifiers::empty(),
        )));
        assert_eq!(action, AppAction::None);
        assert_eq!(app.content_scroll, 10);

        let action = app.handle_event(TuiEvent::Key(KeyEvent::new(
            KeyCode::PageDown,
            KeyModifiers::empty(),
        )));
        assert_eq!(action, AppAction::None);
        assert_eq!(app.content_scroll, 0);
    }

    #[test]
    fn new_session_action_variant() {
        assert_eq!(AppAction::NewSession, AppAction::NewSession);
        assert_ne!(AppAction::NewSession, AppAction::Quit);
    }

    #[test]
    fn switch_session_action_variant() {
        let a = AppAction::SwitchSession("s1".into());
        let b = AppAction::SwitchSession("s1".into());
        assert_eq!(a, b);
    }

    #[test]
    fn content_scroll_resets_on_input() {
        let mut app = App::new("test");
        app.content_scroll = 20;
        app.state = AppState::WaitingForInput;

        // Typing should reset scroll
        app.handle_event(key(KeyCode::Char('a')));
        assert_eq!(app.content_scroll, 0);
    }

    #[test]
    fn content_delta_resets_scroll() {
        let mut app = App::new("test");
        app.content_scroll = 15;

        app.handle_event(TuiEvent::Agent(crab_core::event::Event::ContentDelta {
            index: 0,
            delta: "new text".into(),
        }));
        assert_eq!(app.content_scroll, 0);
    }

    // ── Tab completion tests ──

    fn setup_app_with_commands() -> App {
        let mut app = App::new("test");
        app.set_slash_commands(vec![
            CommandInfo {
                name: "help".into(),
                description: "Show help".into(),
            },
            CommandInfo {
                name: "history".into(),
                description: "Show history".into(),
            },
            CommandInfo {
                name: "commit".into(),
                description: "Create a commit".into(),
            },
            CommandInfo {
                name: "compact".into(),
                description: "Compact context".into(),
            },
            CommandInfo {
                name: "config".into(),
                description: "Show config".into(),
            },
            CommandInfo {
                name: "cost".into(),
                description: "Show cost".into(),
            },
            CommandInfo {
                name: "clear".into(),
                description: "Clear screen".into(),
            },
        ]);
        // Start in WaitingForInput so `/` is treated as text, not search trigger
        app.state = AppState::WaitingForInput;
        app
    }

    #[test]
    fn tab_on_slash_triggers_autocomplete() {
        let mut app = setup_app_with_commands();
        app.input.set_text("/co");

        // Press Tab
        let action = app.handle_event(key(KeyCode::Tab));
        assert_eq!(action, AppAction::None);
        assert!(app.autocomplete.is_active());
        // Should match: commit, compact, config, cost
        assert!(app.autocomplete.candidates().len() >= 3);
    }

    #[test]
    fn tab_cycles_autocomplete() {
        let mut app = setup_app_with_commands();
        app.input.set_text("/h");
        // Tab to trigger
        app.handle_event(key(KeyCode::Tab));
        assert!(app.autocomplete.is_active());
        let first = app.autocomplete.selected_index();

        // Tab again to cycle
        app.handle_event(key(KeyCode::Tab));
        let second = app.autocomplete.selected_index();
        assert_ne!(first, second);
    }

    #[test]
    fn enter_accepts_autocomplete() {
        let mut app = setup_app_with_commands();
        app.input.set_text("/he");
        // Tab triggers completion, first candidate should be /help
        app.handle_event(key(KeyCode::Tab));
        assert!(app.autocomplete.is_active());

        // Enter accepts
        let action = app.handle_event(key(KeyCode::Enter));
        assert_eq!(action, AppAction::None);
        assert!(!app.autocomplete.is_active());
        assert_eq!(app.input.text(), "/help");
    }

    #[test]
    fn esc_dismisses_autocomplete() {
        let mut app = setup_app_with_commands();
        app.input.set_text("/c");
        app.handle_event(key(KeyCode::Tab));
        assert!(app.autocomplete.is_active());

        app.handle_event(key(KeyCode::Esc));
        assert!(!app.autocomplete.is_active());
    }

    #[test]
    fn tab_on_empty_does_nothing() {
        let mut app = setup_app_with_commands();
        let action = app.handle_event(key(KeyCode::Tab));
        // Empty input, Tab goes through to input handler but no completion
        assert_eq!(action, AppAction::None);
        assert!(!app.autocomplete.is_active());
    }

    #[test]
    fn tab_no_match_stays_inactive() {
        let mut app = setup_app_with_commands();
        app.input.set_text("/zz");
        app.handle_event(key(KeyCode::Tab));
        assert!(!app.autocomplete.is_active());
    }

    #[test]
    fn set_slash_commands_and_completion_cwd() {
        let mut app = App::new("test");
        app.set_slash_commands(vec![CommandInfo {
            name: "test".into(),
            description: "A test command".into(),
        }]);
        app.set_completion_cwd("/tmp");
        // Should not panic
        assert!(!app.autocomplete.is_active());
    }

    #[test]
    fn up_down_navigate_autocomplete() {
        let mut app = setup_app_with_commands();
        app.input.set_text("/c");
        app.handle_event(key(KeyCode::Tab));
        assert!(app.autocomplete.is_active());

        let idx0 = app.autocomplete.selected_index();
        app.handle_event(key(KeyCode::Down));
        let idx1 = app.autocomplete.selected_index();
        assert_ne!(idx0, idx1);

        app.handle_event(key(KeyCode::Up));
        let idx2 = app.autocomplete.selected_index();
        assert_eq!(idx0, idx2);
    }

    #[test]
    fn typing_dismisses_autocomplete() {
        let mut app = setup_app_with_commands();
        app.input.set_text("/c");
        app.handle_event(key(KeyCode::Tab));
        assert!(app.autocomplete.is_active());

        // Typing a character should dismiss and fall through
        app.handle_event(key(KeyCode::Char('x')));
        assert!(!app.autocomplete.is_active());
    }

    // ── Thinking state tests ──

    #[test]
    fn set_thinking_active() {
        let mut app = App::new("test");
        app.set_thinking(true);
        assert!(matches!(app.thinking, ThinkingState::Thinking { .. }));
    }

    #[test]
    fn set_thinking_inactive_transitions_to_thought_for() {
        let mut app = App::new("test");
        app.set_thinking(true);
        // Small delay to ensure elapsed > 0
        app.set_thinking(false);
        assert!(matches!(app.thinking, ThinkingState::ThoughtFor { .. }));
    }

    #[test]
    fn set_thinking_inactive_when_idle_stays_idle() {
        let mut app = App::new("test");
        app.set_thinking(false);
        assert!(matches!(app.thinking, ThinkingState::Idle));
    }

    #[test]
    fn thought_for_expires_after_tick() {
        let mut app = App::new("test");
        // Manually set a ThoughtFor state that's already expired
        app.thinking = ThinkingState::ThoughtFor {
            duration: Duration::from_secs(3),
            finished_at: Instant::now().checked_sub(Duration::from_secs(3)).unwrap(),
        };
        app.handle_event(TuiEvent::Tick);
        assert!(matches!(app.thinking, ThinkingState::Idle));
    }

    #[test]
    fn thought_for_persists_within_timeout() {
        let mut app = App::new("test");
        app.thinking = ThinkingState::ThoughtFor {
            duration: Duration::from_secs(1),
            finished_at: Instant::now(),
        };
        app.handle_event(TuiEvent::Tick);
        assert!(matches!(app.thinking, ThinkingState::ThoughtFor { .. }));
    }

    // ── Prompt input mode tests ──

    #[test]
    fn prompt_input_mode_labels() {
        assert_eq!(PromptInputMode::Prompt.label(), "prompt");
        assert_eq!(PromptInputMode::Bash.label(), "bash");
        assert_eq!(PromptInputMode::OrphanedPermission.label(), "permission");
        assert_eq!(PromptInputMode::TaskNotification.label(), "task");
    }

    #[test]
    fn cycle_input_mode_toggles() {
        let mut app = App::new("test");
        assert_eq!(app.input_mode, PromptInputMode::Prompt);
        app.cycle_input_mode();
        assert_eq!(app.input_mode, PromptInputMode::Bash);
        app.cycle_input_mode();
        assert_eq!(app.input_mode, PromptInputMode::Prompt);
    }

    #[test]
    fn cycle_input_mode_noop_for_special_modes() {
        let mut app = App::new("test");
        app.input_mode = PromptInputMode::OrphanedPermission;
        app.cycle_input_mode();
        assert_eq!(app.input_mode, PromptInputMode::OrphanedPermission);

        app.input_mode = PromptInputMode::TaskNotification;
        app.cycle_input_mode();
        assert_eq!(app.input_mode, PromptInputMode::TaskNotification);
    }

    // ── Scroll anchor / unseen message tests ──

    #[test]
    fn scroll_up_sets_anchor() {
        let mut app = App::new("test");
        app.content_buffer = (0..50)
            .map(|i| format!("Line {i}"))
            .collect::<Vec<_>>()
            .join("\n");

        app.handle_event(TuiEvent::Key(KeyEvent::new(
            KeyCode::PageUp,
            KeyModifiers::empty(),
        )));
        assert!(app.scroll_anchor.is_some());
        assert_eq!(app.content_scroll, 10);
    }

    #[test]
    fn scroll_back_to_bottom_clears_anchor() {
        let mut app = App::new("test");
        app.content_buffer = (0..50)
            .map(|i| format!("Line {i}"))
            .collect::<Vec<_>>()
            .join("\n");

        // Scroll up
        app.handle_event(TuiEvent::Key(KeyEvent::new(
            KeyCode::PageUp,
            KeyModifiers::empty(),
        )));
        assert!(app.scroll_anchor.is_some());

        // Scroll back down
        app.handle_event(TuiEvent::Key(KeyEvent::new(
            KeyCode::PageDown,
            KeyModifiers::empty(),
        )));
        assert!(app.scroll_anchor.is_none());
        assert_eq!(app.unseen_message_count, 0);
    }

    #[test]
    fn content_delta_tracks_unseen_when_scrolled() {
        let mut app = App::new("test");
        app.scroll_anchor = Some(10);
        app.content_scroll = 10;

        app.handle_event(TuiEvent::Agent(crab_core::event::Event::ContentDelta {
            index: 0,
            delta: "line1\nline2\n".into(),
        }));

        // Should count newlines (2) as unseen
        assert!(app.unseen_message_count >= 2);
        // Should NOT reset scroll
        assert_eq!(app.content_scroll, 10);
    }

    #[test]
    fn typing_resets_scroll_anchor() {
        let mut app = App::new("test");
        app.scroll_anchor = Some(10);
        app.content_scroll = 10;
        app.unseen_message_count = 5;
        app.state = AppState::WaitingForInput;

        app.handle_event(key(KeyCode::Char('a')));
        assert!(app.scroll_anchor.is_none());
        assert_eq!(app.unseen_message_count, 0);
        assert_eq!(app.content_scroll, 0);
    }

    // ── Render tests for new features ──

    #[test]
    fn render_unseen_divider_does_not_panic() {
        let area = Rect::new(0, 0, 60, 1);
        let mut buf = Buffer::empty(area);
        render_unseen_divider(3, area, &mut buf);

        let text: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(text.contains("3 new messages"));
    }

    #[test]
    fn render_unseen_divider_singular() {
        let area = Rect::new(0, 0, 60, 1);
        let mut buf = Buffer::empty(area);
        render_unseen_divider(1, area, &mut buf);

        let text: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(text.contains("1 new message"));
        // Should NOT say "messages" (plural)
        assert!(!text.contains("messages"));
    }

    #[test]
    fn render_input_no_mode_prefix() {
        // Mode indicator was removed — all modes render the same ❯ prompt
        let input = InputBox::new();
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        let ia = InputArea {
            input: &input,
            mode: PromptInputMode::Bash,
        };
        ia.render(area, &mut buf);

        let text: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(!text.contains("[bash]"));
        assert!(text.contains('❯'));
    }

    #[test]
    fn render_input_with_prompt_mode_no_prefix() {
        let input = InputBox::new();
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        let ia = InputArea {
            input: &input,
            mode: PromptInputMode::Prompt,
        };
        ia.render(area, &mut buf);

        let text: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        // Should NOT contain a mode prefix
        assert!(!text.contains("[prompt]"));
    }

    /// Regression test: `ToolFinished` resolves the tool name from
    /// `active_tools` by tool_use_id, producing the correct `ChatMessage::ToolResult`.
    #[test]
    fn apply_event_tool_finished_resolves_name_from_active_tools() {
        use crate::app_event::AppEvent;
        use crab_core::tool::ToolOutput;

        let mut app = App::new("test");

        app.apply_event(AppEvent::ToolStart {
            id: "tu_1".into(),
            name: "Read".to_string(),
            input: serde_json::Value::Null,
        });
        assert!(
            app.active_tools
                .get("tu_1")
                .is_some_and(|t| t.name == "Read")
        );

        app.apply_event(AppEvent::ToolFinished {
            id: "tu_1".into(),
            output: ToolOutput::success("ok"),
        });

        assert!(app.active_tools.get("tu_1").is_none());

        let last = app.messages.last().expect("expected a message");
        match last {
            ChatMessage::ToolResult {
                tool_name, output, ..
            } => {
                assert_eq!(tool_name, "Read");
                assert_eq!(output, "ok");
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    /// Regression test for task #22: after #13's translate→apply migration,
    /// `apply_event(ContentAppend)` must mirror the streamed delta into
    /// `content_buffer` so the legacy readers — Ctrl+F search, Ctrl+Y
    /// code-block copy, scroll-anchor math at app.rs:399/701/994, and the
    /// external-editor-error banner at runner.rs:550 — still see the text.
    ///
    /// The mirror is a short-term band-aid; ticket #27 will delete
    /// `content_buffer` entirely and rewrite the 7 read sites to iterate
    /// `self.messages` directly. Until then, this test locks in the mirror
    /// so a future refactor cannot silently break it again.
    #[test]
    fn apply_event_content_append_mirrors_into_content_buffer() {
        use crate::app_event::AppEvent;

        let mut app = App::new("test");
        assert!(app.content_buffer.is_empty());
        assert!(app.messages.is_empty());

        // Single delta — starts a new Assistant message and mirrors.
        app.apply_event(AppEvent::ContentAppend("Hello".to_string()));
        assert_eq!(app.content_buffer, "Hello");
        assert_eq!(app.messages.len(), 1);
        match app.messages.last().unwrap() {
            ChatMessage::Assistant { text } => assert_eq!(text, "Hello"),
            other => panic!("expected Assistant, got {other:?}"),
        }

        // Second delta — appends to the existing Assistant message AND
        // appends to the mirror. Both sides must stay in sync.
        app.apply_event(AppEvent::ContentAppend(", world!\n".to_string()));
        assert_eq!(app.content_buffer, "Hello, world!\n");
        assert_eq!(app.messages.len(), 1);
        match app.messages.last().unwrap() {
            ChatMessage::Assistant { text } => assert_eq!(text, "Hello, world!\n"),
            other => panic!("expected Assistant, got {other:?}"),
        }

        // Third delta — multi-line content, still mirrored byte-for-byte.
        app.apply_event(AppEvent::ContentAppend("line2\nline3\n".to_string()));
        assert_eq!(app.content_buffer, "Hello, world!\nline2\nline3\n");
        // And scroll-math sees the full line count (regression anchor for
        // app.rs:399/701/994 scroll-anchor computation).
        assert_eq!(app.content_buffer.lines().count(), 3);
    }

    #[test]
    fn typing_during_processing_queues_on_enter() {
        let mut app = App::new("test");
        app.state = AppState::Processing;

        app.handle_event(key(KeyCode::Char('h')));
        app.handle_event(key(KeyCode::Char('i')));
        let action = app.handle_event(key(KeyCode::Enter));
        assert_eq!(action, AppAction::None);
        assert_eq!(app.command_queue.len(), 1);
        assert!(app.input.is_empty());
        assert_eq!(app.state, AppState::Processing);
    }

    #[test]
    fn dequeue_command_returns_fifo() {
        let mut app = App::new("test");
        app.state = AppState::Processing;
        app.handle_event(key(KeyCode::Char('a')));
        app.handle_event(key(KeyCode::Enter));
        app.handle_event(key(KeyCode::Char('b')));
        app.handle_event(key(KeyCode::Enter));

        assert_eq!(app.command_queue.len(), 2);

        let first = app.dequeue_command();
        assert_eq!(first, Some("a".into()));
        assert_eq!(app.state, AppState::Processing);
        assert!(messages_contain(&app.messages, "a"));

        let second = app.dequeue_command();
        assert_eq!(second, Some("b".into()));

        let third = app.dequeue_command();
        assert_eq!(third, None);
    }

    #[test]
    fn empty_enter_during_processing_does_not_queue() {
        let mut app = App::new("test");
        app.state = AppState::Processing;
        let action = app.handle_event(key(KeyCode::Enter));
        assert_eq!(action, AppAction::None);
        assert!(app.command_queue.is_empty());
    }

    #[test]
    fn reset_clears_command_queue() {
        let mut app = App::new("test");
        app.command_queue.push("test".into());
        app.reset_for_new_session();
        assert!(app.command_queue.is_empty());
    }

    #[test]
    fn reset_for_new_session_preserves_welcome() {
        let mut app = App::new("test");
        app.messages.push(ChatMessage::Welcome {
            version: "0.1.0".into(),
            whats_new: Vec::new(),
            show_project_hint: false,
        });
        app.messages.push(ChatMessage::User {
            text: "hi".into(),
        });
        app.messages.push(ChatMessage::Assistant {
            text: "hello".into(),
        });
        app.reset_for_new_session();
        assert_eq!(app.messages.len(), 1);
        assert!(matches!(app.messages[0], ChatMessage::Welcome { .. }));
    }
}
