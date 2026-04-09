//! App state machine and main event loop.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crab_tools::builtin::bash::BASH_TOOL_NAME;
use crab_tools::builtin::edit::EDIT_TOOL_NAME;
use crab_tools::builtin::notebook::NOTEBOOK_EDIT_TOOL_NAME;
use crab_tools::builtin::write::WRITE_TOOL_NAME;

use crate::components::autocomplete::{AutoComplete, CommandInfo};
use crate::components::code_block::{CodeBlockTracker, ImagePlaceholder};
use crate::components::context_collapse::{CollapsibleSection, ContextCollapse};
use crate::components::dialog::{PermissionDialog, PermissionResponse, RiskLevel};
use crate::components::input::InputBox;
use crate::components::output_styles::{ContentType, OutputStyles};
use crate::components::search::{self, SearchState};
use crate::components::session_sidebar::SessionSidebar;
use crate::components::spinner::Spinner;
use crate::components::tool_output::{ToolOutputEntry, ToolOutputList};
use crate::event::TuiEvent;
use crate::keybindings::{Action, Keybindings};
use crate::layout::AppLayout;

/// Application state phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppState {
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
    /// User requested quit (Ctrl+C / Ctrl+D).
    Quit,
    /// User wants to create a new session.
    NewSession,
    /// User wants to switch to a different session by ID.
    SwitchSession(String),
}

/// Main TUI application.
pub struct App {
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
    /// Current pending permission dialog, if any.
    permission_dialog: Option<PermissionDialog>,
    /// Whether the app should exit.
    pub should_quit: bool,
    /// Name of the tool currently executing (for display).
    current_tool: Option<String>,
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
}

impl App {
    /// Create a new App with default state.
    #[must_use]
    pub fn new(model_name: impl Into<String>) -> Self {
        Self {
            state: AppState::Idle,
            input: InputBox::new(),
            spinner: Spinner::new(),
            content_buffer: "Welcome! Type a message to start, or /help for commands.\n\n"
                .to_string(),
            model_name: model_name.into(),
            permission_dialog: None,
            should_quit: false,
            current_tool: None,
            sidebar_visible: false,
            session_sidebar: SessionSidebar::new(),
            session_id: String::new(),
            keybindings: Keybindings::defaults(),
            total_input_tokens: 0,
            total_output_tokens: 0,
            content_scroll: 0,
            tool_outputs: ToolOutputList::new(),
            code_blocks: CodeBlockTracker::new(),
            search: SearchState::new(),
            image_placeholders: Vec::new(),
            autocomplete: AutoComplete::default(),
            context_collapse: ContextCollapse::new(Vec::new()),
            output_styles: OutputStyles::default_styles(),
            working_dir: String::new(),
            thinking: ThinkingState::Idle,
            scroll_anchor: None,
            unseen_message_count: 0,
            input_mode: PromptInputMode::Prompt,
            last_interrupt: None,
            permission_mode: crab_core::permission::PermissionMode::Default,
        }
    }

    /// Set the working directory (displayed in header).
    pub fn set_working_dir(&mut self, dir: impl Into<String>) {
        self.working_dir = dir.into();
    }

    /// Set the current session ID.
    pub fn set_session_id(&mut self, id: impl Into<String>) {
        self.session_id = id.into();
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
        match event {
            TuiEvent::Key(key) => self.handle_key(key),
            TuiEvent::Agent(agent_event) => {
                self.handle_agent_event(agent_event);
                AppAction::None
            }
            TuiEvent::Tick => {
                self.spinner.tick();
                // Expire the "thought for Ns" display after the timeout
                if let ThinkingState::ThoughtFor { finished_at, .. } = self.thinking
                    && finished_at.elapsed() >= ThinkingState::DISPLAY_DURATION
                {
                    self.thinking = ThinkingState::Idle;
                }
                AppAction::None
            }
            TuiEvent::Resize { .. } => AppAction::None,
        }
    }

    #[allow(clippy::too_many_lines)]
    fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> AppAction {
        // Search mode intercepts all keys except Esc and Enter
        if self.search.is_active() {
            return self.handle_search_key(key);
        }

        // Check keybinding actions first (global shortcuts)
        if let Some(action) = self.keybindings.resolve(key.code, key.modifiers) {
            match action {
                Action::Quit => {
                    // CC-aligned double-press: first Ctrl+C interrupts, second exits.
                    let now = Instant::now();
                    if let Some(last) = self.last_interrupt
                        && now.duration_since(last) < Duration::from_millis(500)
                    {
                        // Double press within 500ms → actually quit
                        self.should_quit = true;
                        return AppAction::Quit;
                    }
                    // First press → interrupt current operation
                    self.last_interrupt = Some(now);
                    if self.state == AppState::Processing {
                        self.spinner.stop();
                        self.state = AppState::Idle;
                        let _ = writeln!(self.content_buffer, "\n[interrupted]");
                    }
                    // Show hint that double-press exits
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
                    // Set scroll anchor so we know user is scrolled up
                    let total = self.content_buffer.lines().count();
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
                    return AppAction::None;
                }
                Action::CopyCodeBlock if self.state != AppState::Confirming => {
                    self.code_blocks.update(&self.content_buffer);
                    if let Some(text) = self.code_blocks.copy_focused() {
                        let _ = write!(
                            self.content_buffer,
                            "\n[copied {} bytes to clipboard]",
                            text.len()
                        );
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
                // These actions are recognized but currently act as no-ops
                // until the corresponding subsystems are wired up.
                Action::NextSession
                | Action::PrevSession
                | Action::HistorySearch
                | Action::ExternalEditor
                | Action::Stash
                | Action::ToggleTodos
                | Action::ToggleTranscript
                | Action::KillAgents
                | Action::ModelPicker
                | Action::ImagePaste
                | Action::Undo
                    if self.state != AppState::Confirming =>
                {
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
                let _ = write!(
                    self.content_buffer,
                    "\n[copied {} bytes to clipboard]",
                    text.len()
                );
            }
            return AppAction::None;
        }

        // Enter toggles fold when idle and input is empty
        if self.state == AppState::Idle
            && key.code == KeyCode::Enter
            && key.modifiers.is_empty()
            && self.input.is_empty()
        {
            self.tool_outputs.toggle_selected();
            return AppAction::None;
        }

        match self.state {
            AppState::Confirming => self.handle_confirming_key(key),
            AppState::Processing => {
                // During processing, Esc could cancel (future: send cancel signal)
                AppAction::None
            }
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
                        // Show user prompt in content area (no divider — CC doesn't have one)
                        let _ = writeln!(self.content_buffer, "\n❯ {text}\n");
                        self.state = AppState::Processing;
                        self.spinner.start_with_random_verb();
                        return AppAction::Submit(text);
                    }
                    return AppAction::None;
                }

                self.input.handle_key(key);
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

    /// Scroll content to show the current search match.
    fn scroll_to_search_match(&mut self) {
        if let Some(m) = self.search.current() {
            let total_lines = self.content_buffer.lines().count();
            let from_bottom = total_lines.saturating_sub(m.line + 1);
            self.content_scroll = from_bottom;
        }
    }

    fn handle_confirming_key(&mut self, key: crossterm::event::KeyEvent) -> AppAction {
        if let Some(ref mut dialog) = self.permission_dialog {
            if let Some(response) = dialog.handle_key(key.code) {
                let request_id = dialog.request_id.clone();
                let allowed = matches!(
                    response,
                    PermissionResponse::Allow | PermissionResponse::AlwaysAllow
                );
                self.permission_dialog = None;
                self.state = AppState::Processing;
                if allowed {
                    self.spinner.start_with_random_verb();
                }
                return AppAction::PermissionResponse {
                    request_id,
                    allowed,
                };
            }
            return AppAction::None;
        }
        AppAction::None
    }

    #[allow(clippy::too_many_lines)]
    fn handle_agent_event(&mut self, event: crab_core::event::Event) {
        use crab_core::event::Event;
        match event {
            Event::ContentDelta { delta, .. } => {
                // Track unseen content when the user is scrolled up
                if self.scroll_anchor.is_some() {
                    // Count newlines in the delta as new "messages"
                    let new_lines = delta.chars().filter(|&c| c == '\n').count();
                    self.unseen_message_count =
                        self.unseen_message_count.saturating_add(new_lines.max(1));
                } else {
                    self.content_scroll = 0; // auto-scroll on new content
                }
                self.content_buffer.push_str(&delta);
                // Approximate token count for spinner display (~4 chars per token)
                self.spinner.response_tokens += (delta.len() as u64).div_ceil(4);
            }
            Event::MessageEnd { usage, .. } => {
                self.spinner.stop();
                self.current_tool = None;
                self.state = AppState::Idle;
                // Accumulate token usage (displayed in status bar, not inline)
                self.total_input_tokens += usage.input_tokens;
                self.total_output_tokens += usage.output_tokens;
                // Turn separator after assistant response
                let _ = writeln!(self.content_buffer, "\n");
            }
            Event::ToolUseStart { name, .. } => {
                self.current_tool = Some(name.clone());
                self.spinner.set_message(format!("Running {name}…"));
                let _ = write!(self.content_buffer, "\n[tool] {name}\n");
            }
            Event::ToolResult { output, .. } => {
                let tool_name = self.current_tool.take().unwrap_or_default();
                self.spinner.clear_override();
                // Show tool result summary in content area
                let text = output.text();
                if output.is_error {
                    let _ = writeln!(self.content_buffer, "[tool error] {text}");
                    self.tool_outputs
                        .push(ToolOutputEntry::new(&tool_name, text.clone(), true));
                    // Add to context collapse as a collapsible error section
                    let mut section =
                        CollapsibleSection::new(format!("Tool error: {tool_name}"), text.clone());
                    section.collapsed = true;
                    self.context_collapse.push_section(section);
                } else if !text.is_empty() {
                    // Truncate long output for display
                    if text.len() > 500 {
                        let _ = writeln!(
                            self.content_buffer,
                            "[{tool_name} result] {}...",
                            &text[..500]
                        );
                    } else {
                        let _ = writeln!(self.content_buffer, "[{tool_name} result] {text}");
                    }
                    self.tool_outputs
                        .push(ToolOutputEntry::new(&tool_name, text.clone(), false));
                    // Add long tool outputs to context collapse (auto-collapsed)
                    if text.lines().count() > 5 {
                        let mut section = CollapsibleSection::new(
                            format!("Tool output: {tool_name}"),
                            text.clone(),
                        );
                        section.collapsed = true;
                        self.context_collapse.push_section(section);
                    }
                }
                // Update code block detection
                self.code_blocks.update(&self.content_buffer);
            }
            Event::PermissionRequest {
                request_id,
                tool_name,
                input_summary,
            } => {
                self.spinner.stop();
                self.state = AppState::Confirming;
                // Determine risk level based on tool name
                let risk = classify_tool_risk(&tool_name);
                self.permission_dialog = Some(PermissionDialog::new(
                    tool_name,
                    input_summary,
                    risk,
                    request_id,
                ));
            }
            Event::CompactStart { strategy, .. } => {
                let _ = writeln!(
                    self.content_buffer,
                    "\n[compact] Starting compaction: {strategy}"
                );
            }
            Event::CompactEnd {
                after_tokens,
                removed_messages,
            } => {
                let _ = writeln!(
                    self.content_buffer,
                    "[compact] Removed {removed_messages} messages, now {after_tokens} tokens"
                );
            }
            Event::TokenWarning {
                usage_pct,
                used,
                limit,
            } => {
                let _ = writeln!(
                    self.content_buffer,
                    "[warn] Token usage {:.0}% ({used}/{limit})",
                    usage_pct * 100.0,
                );
            }
            Event::SessionSaved { session_id } => {
                let _ = writeln!(self.content_buffer, "[session] Saved: {session_id}");
            }
            Event::SessionResumed {
                session_id,
                message_count,
            } => {
                let _ = writeln!(
                    self.content_buffer,
                    "[session] Resumed {session_id} ({message_count} messages)"
                );
            }
            Event::Error { message } => {
                self.spinner.stop();
                self.current_tool = None;
                let _ = writeln!(self.content_buffer, "\n[Error: {message}]");
                self.state = AppState::Idle;
            }
            _ => {}
        }
    }

    /// Render the full app into a ratatui frame.
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        #[allow(clippy::cast_possible_truncation)]
        let layout = AppLayout::compute_with_sidebar(
            area,
            self.input.line_count() as u16,
            self.sidebar_visible,
            crate::layout::DEFAULT_SIDEBAR_WIDTH,
        );

        // Header (3 lines art/info + 1 line separator, no border box)
        render_header(&self.model_name, &self.working_dir, layout.header, buf);

        // Session sidebar
        if let Some(sidebar_area) = layout.sidebar {
            Widget::render(&self.session_sidebar, sidebar_area, buf);
        }

        // Content area with scroll support
        render_content_scrolled(
            &self.content_buffer,
            self.content_scroll,
            &self.output_styles,
            layout.content,
            buf,
        );

        // Status line: spinner (when active) or status bar (model/tokens/cost)
        if self.spinner.is_active() {
            Widget::render(&self.spinner, layout.status, buf);
        } else {
            render_status_line(
                &self.model_name,
                self.permission_mode,
                self.total_input_tokens,
                self.total_output_tokens,
                &self.thinking,
                layout.status,
                buf,
            );
        }

        // Separator above input
        render_separator(layout.separator_top, buf);

        // Input with ❯ prompt and mode indicator (no border box)
        render_input_with_prompt(&self.input, self.input_mode, layout.input, buf);

        // Separator below input
        render_separator(layout.separator_bottom, buf);

        // Unseen message divider (when user is scrolled up and new content arrives)
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

        // Search bar (overlays bottom of content when active)
        if self.search.is_active() {
            let search_area = Rect {
                x: layout.content.x,
                y: layout.content.y + layout.content.height.saturating_sub(1),
                width: layout.content.width,
                height: 1,
            };
            search::render_search_bar(&self.search, search_area, buf);
        }

        // Bottom bar
        render_bottom_bar(self.state, self.search.is_active(), layout.bottom_bar, buf);

        // Autocomplete popup (renders above input)
        if self.autocomplete.is_active() {
            render_autocomplete_popup(&self.autocomplete, layout.input, buf);
        }

        // Permission dialog overlay (renders on top of everything)
        if let Some(ref dialog) = self.permission_dialog {
            let dialog_area = PermissionDialog::dialog_area(area);
            Widget::render(dialog, dialog_area, buf);
        }
    }
}

/// Terra cotta color (`#DA7756`, same as CC's `clawd_body`).
const CRAB_COLOR: Color = Color::Rgb(218, 119, 86);

/// Background color for the crab art body (same as CC's `clawd_background`).
const CRAB_BG: Color = Color::Black;

/// Render the header: crab art (left) + info text (right) + separator.
///
/// Crab logo using Unicode block/box characters:
/// ```text
///  ╱▔╲ ● ● ╱▔╲  Crab Code v0.1.0
///  ╲▂╱╲███╱╲▂╱  claude-sonnet-4-6
///    ╱╱ ███ ╲╲   C:\path\to\project
/// ────────────────────────────────────────
/// ```
#[allow(clippy::cast_possible_truncation)]
fn render_header(model_name: &str, working_dir: &str, area: Rect, buf: &mut Buffer) {
    if area.height == 0 || area.width < 10 {
        return;
    }

    let fg = Style::default().fg(CRAB_COLOR);
    let fg_bg = Style::default().fg(CRAB_COLOR).bg(CRAB_BG);

    // Crab art — 3 rows, ASCII-safe characters for consistent width
    // All elements use CRAB_COLOR (#DA7756) matching the project logo
    let art_lines: [Line<'_>; 3] = [
        Line::from(Span::styled(r" /| o o |\  ", fg)),
        Line::from(vec![
            Span::styled(r" \_", fg),
            Span::styled("^^^^^", fg_bg),
            Span::styled(r"_/  ", fg),
        ]),
        Line::from(Span::styled(r"  // ||| \\  ", fg)),
    ];

    let art_width = 13u16;

    // Info text beside the art (mirrors CC's CondensedLogo text)
    let text_budget = area.width.saturating_sub(art_width) as usize;
    let info_lines: [Line<'_>; 3] = [
        Line::from(vec![
            Span::styled(
                "Crab Code",
                Style::default().fg(CRAB_COLOR).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" v0.1.0", Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(Span::styled(
            model_name,
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            shorten_path(working_dir, text_budget),
            Style::default().fg(Color::DarkGray),
        )),
    ];

    for (i, (art_line, info_line)) in art_lines.iter().zip(info_lines.iter()).enumerate() {
        let y = area.y + i as u16;
        if y >= area.y + area.height {
            break;
        }

        let art_area = Rect {
            x: area.x,
            y,
            width: art_width.min(area.width),
            height: 1,
        };
        Widget::render(art_line.clone(), art_area, buf);

        if area.width > art_width {
            let info_area = Rect {
                x: area.x + art_width,
                y,
                width: area.width.saturating_sub(art_width),
                height: 1,
            };
            Widget::render(info_line.clone(), info_area, buf);
        }
    }

    // Row 4: thin separator ───
    if area.height >= 4 {
        render_separator(
            Rect {
                x: area.x,
                y: area.y + 3,
                width: area.width,
                height: 1,
            },
            buf,
        );
    }
}

/// Shorten a path to fit within `max_chars`.
fn shorten_path(path: &str, max_chars: usize) -> String {
    if path.len() <= max_chars || max_chars < 6 {
        return path.to_string();
    }
    let suffix_budget = max_chars.saturating_sub(4);
    if let Some(pos) = path[path.len().saturating_sub(suffix_budget)..].find(['/', '\\']) {
        format!(
            "...{}",
            &path[path.len().saturating_sub(suffix_budget) + pos..]
        )
    } else {
        format!("...{}", &path[path.len().saturating_sub(suffix_budget)..])
    }
}

/// Render a thin horizontal separator line (`───`).
#[allow(clippy::cast_possible_truncation)]
fn render_separator(area: Rect, buf: &mut Buffer) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let sep = "─".repeat(area.width as usize);
    Widget::render(
        Line::from(Span::styled(&*sep, Style::default().fg(Color::DarkGray))),
        area,
        buf,
    );
}

/// Render the thinking state indicator.
///
/// When thinking is active, shows `"Thinking... (Ns)"` with elapsed time.
/// After thinking finishes, shows `"(thought for Ns)"` for 2 seconds.
#[allow(dead_code, clippy::cast_possible_truncation)]
fn render_thinking_state(thinking: &ThinkingState, area: Rect, buf: &mut Buffer) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let text = match thinking {
        ThinkingState::Idle => return,
        ThinkingState::Thinking { started_at } => {
            let elapsed = started_at.elapsed().as_secs();
            format!("Thinking\u{2026} ({elapsed}s)")
        }
        ThinkingState::ThoughtFor { duration, .. } => {
            format!("(thought for {}s)", duration.as_secs())
        }
    };

    let style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::ITALIC);
    let line = Line::from(Span::styled(text, style));
    Widget::render(line, area, buf);
}

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

/// Render input with `❯` prompt and mode indicator — no border box (matches CC's flat style).
#[allow(clippy::cast_possible_truncation)]
fn render_input_with_prompt(
    input: &InputBox,
    _mode: PromptInputMode,
    area: Rect,
    buf: &mut Buffer,
) {
    if area.height == 0 || area.width < 4 {
        Widget::render(input, area, buf);
        return;
    }

    // No mode indicator — CC shows permission mode elsewhere (status line)
    let prefix_width = 0u16;

    // Placeholder for future mode indicator
    if false {
        let mode_span = Span::styled(
            "",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );
        let mode_area = Rect {
            x: area.x,
            y: area.y,
            width: prefix_width.min(area.width),
            height: 1,
        };
        Widget::render(Line::from(mode_span), mode_area, buf);
    }

    // Prompt chevron
    let prompt_x = area.x + prefix_width;
    let prompt_span = Span::styled(
        "\u{276f} ",
        Style::default().fg(CRAB_COLOR).add_modifier(Modifier::BOLD),
    );
    let prompt_area = Rect {
        x: prompt_x,
        y: area.y,
        width: 2.min(area.width.saturating_sub(prefix_width)),
        height: 1,
    };
    Widget::render(Line::from(prompt_span), prompt_area, buf);

    let input_x = prompt_x + 2;
    let input_area = Rect {
        x: input_x,
        y: area.y,
        width: area.width.saturating_sub(prefix_width + 2),
        height: area.height,
    };
    Widget::render(input, input_area, buf);
}

#[allow(clippy::cast_possible_truncation)]
fn render_content_scrolled(
    text: &str,
    scroll_offset: usize,
    styles: &OutputStyles,
    area: Rect,
    buf: &mut Buffer,
) {
    if area.height == 0 || text.is_empty() {
        return;
    }

    // Render lines: system prefixed lines get color-coded styles,
    // everything else goes through markdown rendering.
    let theme = crate::theme::Theme::dark();
    let highlighter = crate::components::syntax::SyntaxHighlighter::new();
    let md_renderer = crate::components::markdown::MarkdownRenderer::new(&theme, &highlighter);

    let mut rendered_lines: Vec<Line<'static>> = Vec::new();

    // Split content into segments: system lines vs markdown blocks
    let mut md_block = String::new();
    for raw_line in text.lines() {
        if is_system_line(raw_line) {
            // Flush any accumulated markdown
            if !md_block.is_empty() {
                rendered_lines.extend(md_renderer.render(&md_block));
                md_block.clear();
            }
            // Render system line with prefix-based styling
            let style = classify_content_style(raw_line, styles);
            rendered_lines.push(Line::from(Span::styled(raw_line.to_string(), style)));
        } else {
            md_block.push_str(raw_line);
            md_block.push('\n');
        }
    }
    // Flush remaining markdown
    if !md_block.is_empty() {
        rendered_lines.extend(md_renderer.render(&md_block));
    }

    let visible = area.height as usize;
    let end = rendered_lines.len().saturating_sub(scroll_offset);
    let start = end.saturating_sub(visible);

    for (i, line) in rendered_lines
        .iter()
        .skip(start)
        .take(visible.min(end.saturating_sub(start)))
        .enumerate()
    {
        let y = area.y + i as u16;
        let line_area = Rect {
            x: area.x,
            y,
            width: area.width,
            height: 1,
        };
        Widget::render(line.clone(), line_area, buf);
    }
}

/// Check if a line is a system/tool prefix line (not markdown).
fn is_system_line(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("[tool")
        || t.starts_with("[Error:")
        || t.starts_with("[warn]")
        || t.starts_with("[session]")
        || t.starts_with("[compact]")
        || t.starts_with("[interrupted]")
        || t.starts_with("❯ ")
        || t.starts_with("────")
        || t.starts_with("Welcome!")
}

/// Choose a style for a content line based on its prefix/content.
fn classify_content_style(line: &str, styles: &OutputStyles) -> Style {
    let trimmed = line.trim_start();
    if trimmed.starts_with("[tool error]") || trimmed.starts_with("[Error:") {
        styles.style_for(ContentType::Error)
    } else if trimmed.starts_with("[warn]") {
        styles.style_for(ContentType::Warning)
    } else if trimmed.starts_with("[tool]")
        || trimmed.starts_with('[') && trimmed.contains("result]")
    {
        styles.style_for(ContentType::ToolResult)
    } else if trimmed.starts_with("[session]") || trimmed.starts_with("[compact]") {
        styles.style_for(ContentType::SystemMessage)
    } else if trimmed.starts_with("[tokens:") {
        styles.style_for(ContentType::Muted)
    } else {
        styles.style_for(ContentType::AssistantResponse)
    }
}

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

/// Classify tool risk level based on tool name for the permission dialog.
fn classify_tool_risk(tool_name: &str) -> RiskLevel {
    match tool_name {
        BASH_TOOL_NAME | WRITE_TOOL_NAME | NOTEBOOK_EDIT_TOOL_NAME => RiskLevel::High,
        EDIT_TOOL_NAME => RiskLevel::Medium,
        _ => RiskLevel::Low,
    }
}

/// Render the status line: model name | token counts | thinking state.
///
/// Matches CC's `StatusLine` component showing operational data.
fn render_status_line(
    model: &str,
    perm_mode: crab_core::permission::PermissionMode,
    input_tokens: u64,
    output_tokens: u64,
    thinking: &ThinkingState,
    area: Rect,
    buf: &mut Buffer,
) {
    if area.width < 10 || area.height == 0 {
        return;
    }

    let mut spans = vec![
        Span::styled(model, Style::default().fg(Color::Cyan)),
        Span::styled(" │ ", Style::default().fg(Color::DarkGray)),
        Span::styled(perm_mode.to_string(), Style::default().fg(Color::Yellow)),
        Span::styled(" │ ", Style::default().fg(Color::DarkGray)),
    ];

    // Token counts
    let in_str = format_token_count(input_tokens);
    let out_str = format_token_count(output_tokens);
    spans.push(Span::styled(
        format!("{in_str} in"),
        Style::default().fg(Color::DarkGray),
    ));
    spans.push(Span::styled(" · ", Style::default().fg(Color::DarkGray)));
    spans.push(Span::styled(
        format!("{out_str} out"),
        Style::default().fg(Color::DarkGray),
    ));

    // Thinking state
    match thinking {
        ThinkingState::Thinking { started_at } => {
            let elapsed = started_at.elapsed().as_secs();
            spans.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
            spans.push(Span::styled(
                format!("thinking ({elapsed}s)"),
                Style::default().fg(Color::Yellow),
            ));
        }
        ThinkingState::ThoughtFor { duration, .. } => {
            spans.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
            spans.push(Span::styled(
                format!("thought for {}s", duration.as_secs()),
                Style::default().fg(Color::DarkGray),
            ));
        }
        ThinkingState::Idle => {}
    }

    Widget::render(Line::from(spans), area, buf);
}

/// Format token count: 1234 → "1.2k", 500 → "500"
fn format_token_count(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1000 {
        format!("{:.1}k", tokens as f64 / 1000.0)
    } else {
        tokens.to_string()
    }
}

fn render_bottom_bar(state: AppState, search_active: bool, area: Rect, buf: &mut Buffer) {
    let line = if search_active {
        Line::from(Span::styled(
            "Enter: next match | Esc: close | type to search",
            Style::default().fg(Color::DarkGray),
        ))
    } else {
        match state {
            AppState::Confirming => Line::from(Span::styled(
                "y: allow | n: deny | a: always | Esc: deny",
                Style::default().fg(Color::DarkGray),
            )),
            _ => Line::from(Span::styled(
                "? for shortcuts",
                Style::default().fg(Color::DarkGray),
            )),
        }
    };
    Widget::render(line, area, buf);
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

    #[test]
    fn app_initial_state() {
        let app = App::new("gpt-4o");
        assert_eq!(app.state, AppState::Idle);
        assert!(app.input.is_empty());
        assert!(!app.spinner.is_active());
        assert!(app.content_buffer.contains("Welcome"));
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
        // Second press within 500ms: quit
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
        assert!(app.content_buffer.ends_with("Hello world"));
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

        let mut usage = crab_core::model::TokenUsage::default();
        usage.input_tokens = 100;
        usage.output_tokens = 50;
        app.handle_event(TuiEvent::Agent(crab_core::event::Event::MessageEnd {
            usage,
        }));

        assert_eq!(app.total_input_tokens, 100);
        assert_eq!(app.total_output_tokens, 50);

        // Second turn
        app.state = AppState::Processing;
        let mut usage2 = crab_core::model::TokenUsage::default();
        usage2.input_tokens = 200;
        usage2.output_tokens = 80;
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
        app.permission_dialog = Some(PermissionDialog::new(
            "bash",
            "rm -rf /tmp",
            RiskLevel::High,
            "req_1",
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
        assert!(app.permission_dialog.is_none());
    }

    #[test]
    fn confirming_n_denies() {
        let mut app = App::new("test");
        app.state = AppState::Confirming;
        app.permission_dialog = Some(PermissionDialog::new(
            "bash",
            "rm -rf /tmp",
            RiskLevel::High,
            "req_1",
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
        app.permission_dialog = Some(PermissionDialog::new(
            "edit",
            "src/main.rs",
            RiskLevel::Medium,
            "req_2",
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
    fn agent_error_returns_to_idle() {
        let mut app = App::new("test");
        app.state = AppState::Processing;
        app.spinner.start("Working");

        app.handle_event(TuiEvent::Agent(crab_core::event::Event::Error {
            message: "rate limit".into(),
        }));

        assert_eq!(app.state, AppState::Idle);
        assert!(!app.spinner.is_active());
        assert!(app.content_buffer.contains("rate limit"));
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
        app.current_tool = Some("bash".into());

        app.handle_event(TuiEvent::Agent(crab_core::event::Event::ToolResult {
            id: "tu_1".into(),
            output: crab_core::tool::ToolOutput::success("file1.txt\nfile2.txt"),
        }));

        assert!(app.content_buffer.contains("file1.txt"));
        assert!(app.content_buffer.contains("bash result"));
    }

    #[test]
    fn tool_error_shown_in_content() {
        let mut app = App::new("test");
        app.state = AppState::Processing;
        app.current_tool = Some("bash".into());

        app.handle_event(TuiEvent::Agent(crab_core::event::Event::ToolResult {
            id: "tu_1".into(),
            output: crab_core::tool::ToolOutput::error("command not found"),
        }));

        assert!(app.content_buffer.contains("tool error"));
        assert!(app.content_buffer.contains("command not found"));
    }

    #[test]
    fn tool_use_start_shown_in_content() {
        let mut app = App::new("test");
        app.state = AppState::Processing;
        app.spinner.start("Thinking...");

        app.handle_event(TuiEvent::Agent(crab_core::event::Event::ToolUseStart {
            id: "tu_1".into(),
            name: "read".into(),
        }));

        assert!(app.content_buffer.contains("[tool] read"));
        assert_eq!(app.current_tool.as_deref(), Some("read"));
    }

    #[test]
    fn permission_dialog_renders_in_frame() {
        let mut app = App::new("test");
        app.state = AppState::Confirming;
        app.permission_dialog = Some(PermissionDialog::new(
            "bash",
            "rm -rf /tmp",
            RiskLevel::High,
            "req_1",
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
        assert!(all_text.contains("bash"));
        assert!(all_text.contains("Permission"));
    }

    #[test]
    fn classify_tool_risk_levels() {
        assert_eq!(classify_tool_risk(BASH_TOOL_NAME), RiskLevel::High);
        assert_eq!(classify_tool_risk(WRITE_TOOL_NAME), RiskLevel::High);
        assert_eq!(classify_tool_risk(EDIT_TOOL_NAME), RiskLevel::Medium);
        assert_eq!(
            classify_tool_risk(crab_tools::builtin::read::READ_TOOL_NAME),
            RiskLevel::Low
        );
        assert_eq!(
            classify_tool_risk(crab_tools::builtin::glob::GLOB_TOOL_NAME),
            RiskLevel::Low
        );
    }

    #[test]
    fn session_saved_event_shown() {
        let mut app = App::new("test");
        app.handle_event(TuiEvent::Agent(crab_core::event::Event::SessionSaved {
            session_id: "sess_abc".into(),
        }));
        assert!(app.content_buffer.contains("[session] Saved: sess_abc"));
    }

    #[test]
    fn token_warning_shown() {
        let mut app = App::new("test");
        app.handle_event(TuiEvent::Agent(crab_core::event::Event::TokenWarning {
            usage_pct: 0.90,
            used: 90000,
            limit: 100_000,
        }));
        assert!(app.content_buffer.contains("90%"));
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
            finished_at: Instant::now() - Duration::from_secs(3),
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
    fn render_thinking_state_does_not_panic() {
        let area = Rect::new(0, 0, 60, 1);
        let mut buf = Buffer::empty(area);

        render_thinking_state(&ThinkingState::Idle, area, &mut buf);
        render_thinking_state(
            &ThinkingState::Thinking {
                started_at: Instant::now(),
            },
            area,
            &mut buf,
        );
        render_thinking_state(
            &ThinkingState::ThoughtFor {
                duration: Duration::from_secs(5),
                finished_at: Instant::now(),
            },
            area,
            &mut buf,
        );
    }

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
        render_input_with_prompt(&input, PromptInputMode::Bash, area, &mut buf);

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
        render_input_with_prompt(&input, PromptInputMode::Prompt, area, &mut buf);

        let text: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        // Should NOT contain a mode prefix
        assert!(!text.contains("[prompt]"));
    }
}
