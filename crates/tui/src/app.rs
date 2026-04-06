//! App state machine and main event loop.

use std::fmt::Write as _;

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::components::autocomplete::{AutoComplete, CommandInfo};
use crate::components::code_block::{CodeBlockTracker, ImagePlaceholder};
use crate::components::dialog::{PermissionDialog, PermissionResponse, RiskLevel};
use crate::components::input::InputBox;
use crate::components::search::{self, SearchState};
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
}

impl App {
    /// Create a new App with default state.
    #[must_use]
    pub fn new(model_name: impl Into<String>) -> Self {
        Self {
            state: AppState::Idle,
            input: InputBox::new(),
            spinner: Spinner::new(),
            content_buffer: String::new(),
            model_name: model_name.into(),
            permission_dialog: None,
            should_quit: false,
            current_tool: None,
            sidebar_visible: false,
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
        }
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
                    self.should_quit = true;
                    return AppAction::Quit;
                }
                Action::NewSession if self.state != AppState::Confirming => {
                    return AppAction::NewSession;
                }
                Action::NextSession | Action::PrevSession if self.state != AppState::Confirming => {
                    return AppAction::None;
                }
                Action::ToggleSidebar => {
                    self.sidebar_visible = !self.sidebar_visible;
                    return AppAction::None;
                }
                Action::ScrollUp if self.state != AppState::Confirming => {
                    self.content_scroll = self.content_scroll.saturating_add(10);
                    return AppAction::None;
                }
                Action::ScrollDown if self.state != AppState::Confirming => {
                    self.content_scroll = self.content_scroll.saturating_sub(10);
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
                        self.state = AppState::Processing;
                        self.spinner.start("Thinking...");
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
                    self.spinner.start("Executing tool...");
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
                self.content_buffer.push_str(&delta);
                self.content_scroll = 0; // auto-scroll on new content
            }
            Event::MessageEnd { usage, .. } => {
                self.spinner.stop();
                self.current_tool = None;
                self.state = AppState::Idle;
                // Accumulate token usage
                self.total_input_tokens += usage.input_tokens;
                self.total_output_tokens += usage.output_tokens;
                // Show token usage summary
                let total = usage.input_tokens + usage.output_tokens;
                if total > 0 {
                    let _ = write!(
                        self.content_buffer,
                        "\n[tokens: {}in/{}out",
                        usage.input_tokens, usage.output_tokens
                    );
                    if usage.cache_read_tokens > 0 {
                        let _ = write!(self.content_buffer, " cache:{}r", usage.cache_read_tokens);
                    }
                    let _ = writeln!(self.content_buffer, "]");
                }
            }
            Event::ToolUseStart { name, .. } => {
                self.current_tool = Some(name.clone());
                self.spinner.set_message(format!("Running {name}..."));
                let _ = write!(self.content_buffer, "\n[tool] {name}\n");
            }
            Event::ToolResult { output, .. } => {
                let tool_name = self.current_tool.take().unwrap_or_default();
                self.spinner.set_message("Thinking...".to_string());
                // Show tool result summary in content area
                let text = output.text();
                if output.is_error {
                    let _ = writeln!(self.content_buffer, "[tool error] {text}");
                    self.tool_outputs
                        .push(ToolOutputEntry::new(&tool_name, text.clone(), true));
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

        // Top bar (enhanced with session ID + token usage)
        render_top_bar_enhanced(
            &self.model_name,
            self.state,
            &self.session_id,
            self.total_input_tokens,
            self.total_output_tokens,
            layout.top_bar,
            buf,
        );

        // Sidebar placeholder (session panel removed)
        if let Some(_sidebar_area) = layout.sidebar {
            // Session panel was removed; sidebar rendering is a no-op for now.
        }

        // Content area with scroll support
        render_content_scrolled(
            &self.content_buffer,
            self.content_scroll,
            layout.content,
            buf,
        );

        // Status line / spinner
        Widget::render(&self.spinner, layout.status, buf);

        // Input
        Widget::render(&self.input, layout.input, buf);

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

        // Bottom bar (enhanced with sidebar toggle hint)
        render_bottom_bar_enhanced(
            self.state,
            self.sidebar_visible,
            self.search.is_active(),
            layout.bottom_bar,
            buf,
        );

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

/// Format token count with `k` suffix for large values.
#[allow(clippy::cast_precision_loss)]
fn format_tokens(n: u64) -> String {
    if n >= 10_000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

#[allow(clippy::too_many_arguments)]
fn render_top_bar_enhanced(
    model_name: &str,
    state: AppState,
    session_id: &str,
    input_tokens: u64,
    output_tokens: u64,
    area: Rect,
    buf: &mut Buffer,
) {
    let state_str = match state {
        AppState::Idle => "idle",
        AppState::WaitingForInput => "input",
        AppState::Processing => "processing",
        AppState::Confirming => "confirm",
    };

    let mut spans = vec![
        Span::styled(
            " crab ",
            Style::default()
                .fg(Color::White)
                .bg(Color::Red)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(model_name, Style::default().fg(Color::Cyan)),
        Span::raw(" | "),
        Span::styled(state_str, Style::default().fg(Color::Yellow)),
    ];

    // Session ID (show short version)
    if !session_id.is_empty() {
        let short_id = if session_id.len() > 8 {
            &session_id[..8]
        } else {
            session_id
        };
        spans.push(Span::raw(" | "));
        spans.push(Span::styled(short_id, Style::default().fg(Color::DarkGray)));
    }

    // Token usage
    let total = input_tokens + output_tokens;
    if total > 0 {
        spans.push(Span::raw(" | "));
        spans.push(Span::styled(
            format!(
                "{}in/{}out",
                format_tokens(input_tokens),
                format_tokens(output_tokens)
            ),
            Style::default().fg(Color::DarkGray),
        ));
    }

    Widget::render(Line::from(spans), area, buf);
}

#[allow(clippy::cast_possible_truncation)]
fn render_content_scrolled(text: &str, scroll_offset: usize, area: Rect, buf: &mut Buffer) {
    if area.height == 0 || text.is_empty() {
        return;
    }

    let lines: Vec<&str> = text.lines().collect();
    let visible = area.height as usize;
    // Show the last N lines minus scroll offset (auto-scroll to bottom)
    let end = lines.len().saturating_sub(scroll_offset);
    let start = end.saturating_sub(visible);

    for (i, line) in lines
        .iter()
        .skip(start)
        .take(visible.min(end - start))
        .enumerate()
    {
        let y = area.y + i as u16;
        let line_widget = Line::from(*line);
        let line_area = Rect {
            x: area.x,
            y,
            width: area.width,
            height: 1,
        };
        Widget::render(line_widget, line_area, buf);
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
        "bash" | "write" | "notebook_edit" => RiskLevel::High,
        "edit" | "multi_edit" => RiskLevel::Medium,
        _ => RiskLevel::Low,
    }
}

fn render_bottom_bar_enhanced(
    state: AppState,
    sidebar_visible: bool,
    search_active: bool,
    area: Rect,
    buf: &mut Buffer,
) {
    let hints = if search_active {
        "Enter: next match | Esc: close search | type to search"
    } else {
        match state {
            AppState::Idle | AppState::WaitingForInput => {
                if sidebar_visible {
                    "Enter: send | Tab: complete | /: search | y: copy | Ctrl+B: sidebar | Ctrl+C: quit"
                } else {
                    "Enter: send | Tab: complete | /: search | y: copy | Ctrl+N: new | Ctrl+C: quit"
                }
            }
            AppState::Processing => "Ctrl+C: quit | PageUp/Down: scroll",
            AppState::Confirming => "y: allow | n: deny | a: always | Esc: deny",
        }
    };

    let line = Line::from(Span::styled(hints, Style::default().fg(Color::DarkGray)));
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
        assert!(app.content_buffer.is_empty());
        assert_eq!(app.model_name, "gpt-4o");
        assert!(!app.should_quit);
        assert!(!app.sidebar_visible);
        assert!(app.session_id.is_empty());
        assert_eq!(app.total_input_tokens, 0);
        assert_eq!(app.total_output_tokens, 0);
        assert_eq!(app.content_scroll, 0);
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
    fn ctrl_c_quits() {
        let mut app = App::new("test");
        let action = app.handle_event(ctrl_key('c'));
        assert_eq!(action, AppAction::Quit);
        assert!(app.should_quit);
    }

    #[test]
    fn ctrl_d_quits() {
        let mut app = App::new("test");
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
        assert_eq!(app.content_buffer, "Hello world");
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
        app.content_buffer = "Hello, world!\nLine 2\n".into();
        app.spinner.start("Thinking...");

        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        app.render(area, &mut buf);

        let top_row: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(top_row.contains("crab"));
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
        assert_eq!(classify_tool_risk("bash"), RiskLevel::High);
        assert_eq!(classify_tool_risk("write"), RiskLevel::High);
        assert_eq!(classify_tool_risk("edit"), RiskLevel::Medium);
        assert_eq!(classify_tool_risk("read"), RiskLevel::Low);
        assert_eq!(classify_tool_risk("glob"), RiskLevel::Low);
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
        // Should not panic
        let action = app.handle_event(ctrl_key('c'));
        assert_eq!(action, AppAction::Quit);
    }

    #[test]
    fn render_top_bar_shows_session_id() {
        let mut app = App::new("gpt-4o");
        app.set_session_id("sess_abcdef12345");
        app.total_input_tokens = 15000;
        app.total_output_tokens = 3000;

        let area = Rect::new(0, 0, 120, 24);
        let mut buf = Buffer::empty(area);
        app.render(area, &mut buf);

        let top_row: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(top_row.contains("sess_abc")); // truncated to 8 chars
        assert!(top_row.contains("15.0k")); // formatted tokens
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
    fn format_tokens_basic() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(500), "500");
        assert_eq!(format_tokens(10_000), "10.0k");
        assert_eq!(format_tokens(150_000), "150.0k");
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
}
