//! Event translation and state-mutation handlers for the App.
//!
//! Implements the Elm-style reducer half of the TUI: `translate_event`
//! converts incoming `TuiEvent`s to `AppEvent`s, and `apply_event` mutates
//! state in response. Key events still flow through the dedicated `handle_key`
//! path because their interpretation depends on overlay stack, search mode,
//! autocomplete, and current `AppState`.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyModifiers};

use super::App;
use super::state::{ActiveToolInfo, AppAction, AppState, ChatMessage, ExitKey, ThinkingState};
use crate::components::context_collapse::CollapsibleSection;
use crate::components::permission::{PermissionCard, PermissionResponse};
use crate::components::tool_output::ToolOutputEntry;
use crate::event::TuiEvent;
use crate::keybindings::{Action, KeyContext, ResolveOutcome};
use crate::vim::VimAction;

/// Keep at most the last `n` lines of `s` so an arbitrarily long stream
/// of tool output can never grow the in-progress cell unboundedly.
fn trim_to_last_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    if lines.len() <= n {
        return s.to_string();
    }
    lines[lines.len() - n..].join("\n")
}

impl App {
    /// Best-effort text extractor used by the `Message*` actions so Copy /
    /// Edit grab something meaningful regardless of the message variant.
    #[must_use]
    pub(super) fn message_text(msg: &ChatMessage) -> Option<String> {
        match msg {
            ChatMessage::User { text }
            | ChatMessage::Assistant { text }
            | ChatMessage::System { text }
            | ChatMessage::Thinking { text, .. } => Some(text.clone()),
            ChatMessage::ToolResult { output, .. } => Some(output.clone()),
            _ => None,
        }
    }

    /// Scan recent messages for the most recent tool result whose output
    /// contains unified-diff markers (`--- ` / `+++ ` / `@@`). Returns the
    /// matching output text for `DiffViewerOverlay::from_unified_diff`.
    #[must_use]
    pub(super) fn latest_diff_text(&self) -> Option<String> {
        for msg in self.messages.iter().rev() {
            if let ChatMessage::ToolResult { output, .. } = msg
                && output.contains("\n--- ")
                && output.contains("\n+++ ")
                && output.contains("\n@@")
            {
                return Some(output.clone());
            }
        }
        None
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
    pub(super) fn toggle_last_tool_result_collapsed(&mut self) {
        for msg in self.messages.iter_mut().rev() {
            if let ChatMessage::ToolResult { collapsed, .. } = msg {
                *collapsed = !*collapsed;
                self.virtual_list.invalidate();
                return;
            }
        }
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
        use super::state::PromptInputMode;
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
                    let pressed_key = if matches!(key.code, KeyCode::Char('d')) {
                        ExitKey::CtrlD
                    } else {
                        ExitKey::CtrlC
                    };
                    if let Some(last) = self.last_interrupt
                        && now.duration_since(last) < Duration::from_millis(800)
                    {
                        self.should_quit = true;
                        return AppAction::Quit;
                    }
                    self.last_interrupt = Some(now);
                    self.last_interrupt_key = Some(pressed_key);
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
                        return AppAction::InterruptProcessing;
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
                    let overlay = crate::components::mcp_browser::McpBrowserOverlay::new(servers);
                    self.overlay_stack.push(Box::new(overlay));
                    return AppAction::None;
                }
                Action::OpenDiffViewer if self.state != AppState::Confirming => {
                    let Some(diff_text) = self.latest_diff_text() else {
                        self.notifications
                            .warn("No diff in recent tool output".to_string());
                        return AppAction::None;
                    };
                    let overlay =
                        crate::components::diff_viewer::DiffViewerOverlay::from_unified_diff(
                            &diff_text,
                        );
                    self.overlay_stack.push(Box::new(overlay));
                    return AppAction::None;
                }
                Action::OpenTeamBrowser if self.state != AppState::Confirming => {
                    // Empty snapshot for now — a future batch intercepts the
                    // TeamCreateTool JSON marker from the agent loop and
                    // populates this from the live TeamRegistry.
                    let snapshot = crate::components::team_browser::TeamSnapshot {
                        members: Vec::new(),
                        tasks: Vec::new(),
                    };
                    let overlay =
                        crate::components::team_browser::TeamBrowserOverlay::new(snapshot);
                    self.overlay_stack.push(Box::new(overlay));
                    return AppAction::None;
                }
                Action::OpenMessageActions if self.state != AppState::Confirming => {
                    // Target the most recent user message; Copy / Edit /
                    // Delete / Rewind all operate on that index.
                    let Some(index) = self
                        .messages
                        .iter()
                        .rposition(|m| matches!(m, ChatMessage::User { .. }))
                    else {
                        self.notifications
                            .warn("No user message to act on".to_string());
                        return AppAction::None;
                    };
                    let overlay =
                        crate::components::message_actions::MessageActionsMenu::new(index);
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
            TuiEvent::Agent {
                event: agent_event, ..
            } => match agent_event {
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
                Event::ToolOutputDelta { id, delta } => {
                    vec![AppEvent::ToolOutputDelta {
                        id: id.clone(),
                        delta: delta.clone(),
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
                    tool_input,
                } => {
                    vec![AppEvent::PermissionRequested {
                        request_id: request_id.clone(),
                        tool_name: tool_name.clone(),
                        summary: input_summary.clone(),
                        tool_input: tool_input.clone(),
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
                Event::StreamAborted { reason } => {
                    vec![AppEvent::StreamAborted {
                        reason: reason.clone(),
                    }]
                }
                Event::ThinkingDelta { delta, .. } => {
                    vec![AppEvent::ThinkingAppend(delta.clone())]
                }
                // Events with no TUI representation today — dropped silently.
                // Candidates for future AppEvent variants:
                //   TurnStart, MessageStart,
                //   ToolUseInput, PermissionResponse,
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
            AppEvent::ToolOutputDelta { id, delta } => {
                let info = self.active_tools.get(&id);
                let tool_name = info.map_or_else(|| "tool".to_string(), |i| i.name.clone());
                let started_at = self.processing_start;
                let added_lines = delta.matches('\n').count().max(1);

                if let Some(ChatMessage::ToolProgress {
                    tool_use_id,
                    tail_output,
                    total_lines,
                    elapsed_secs,
                    ..
                }) = self.messages.last_mut()
                    && tool_use_id == &id
                {
                    tail_output.push_str(&delta);
                    *tail_output = trim_to_last_lines(tail_output, 20);
                    *total_lines = total_lines.saturating_add(added_lines);
                    *elapsed_secs = started_at.map_or(*elapsed_secs, |t| t.elapsed().as_secs_f64());
                    self.virtual_list.invalidate();
                    return AppAction::None;
                }

                self.messages.push(ChatMessage::ToolProgress {
                    tool_use_id: id,
                    tool_name,
                    tail_output: trim_to_last_lines(&delta, 20),
                    total_lines: added_lines,
                    elapsed_secs: started_at.map_or(0.0, |t| t.elapsed().as_secs_f64()),
                });
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
                let result_msg = ChatMessage::ToolResult {
                    tool_name: tool_name.clone(),
                    output: text.clone(),
                    is_error,
                    display,
                    collapsed,
                    is_read_only,
                };
                // If a `ToolProgress` cell for this tool is still showing,
                // swap it for the final result so the progress line doesn't
                // persist alongside the completed output.
                let replaced = matches!(
                    self.messages.last(),
                    Some(ChatMessage::ToolProgress { tool_use_id, .. }) if tool_use_id == &id,
                );
                if replaced {
                    if let Some(last) = self.messages.last_mut() {
                        *last = result_msg;
                    }
                    self.virtual_list.invalidate();
                } else {
                    self.messages.push(result_msg);
                }
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
            AppEvent::StreamAborted { reason } => {
                if matches!(self.messages.last(), Some(ChatMessage::Assistant { .. })) {
                    self.messages.pop();
                }
                self.notifications.warn(reason);
                AppAction::None
            }
            AppEvent::PermissionRequested {
                request_id,
                tool_name,
                summary,
                tool_input,
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
                        &tool_input,
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
                match persist_trust_accepted(&project_path) {
                    Ok(is_first_time) => {
                        if is_first_time {
                            // Signal runner to fire the Setup hook so
                            // project-level one-shot setup can run.
                            return AppAction::FireSetupHook { project_path };
                        }
                    }
                    Err(e) => self
                        .notifications
                        .warn(format!("Failed to save trust state: {e}")),
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

            AppEvent::MessageCopy { index } => {
                if let Some(text) = self.messages.get(index).and_then(Self::message_text) {
                    match self.clipboard.copy(&text) {
                        Ok(()) => self.notifications.success("Copied to clipboard"),
                        Err(e) => self.notifications.warn(format!("Copy failed: {e}")),
                    }
                }
                self.overlay_stack.pop();
                AppAction::None
            }
            AppEvent::MessageEdit { index } => {
                if let Some(text) = self.messages.get(index).and_then(Self::message_text) {
                    self.input.set_text(&text);
                    self.messages.remove(index);
                }
                self.overlay_stack.pop();
                AppAction::None
            }
            AppEvent::MessageDelete { index } => {
                if index < self.messages.len() {
                    self.messages.remove(index);
                }
                self.overlay_stack.pop();
                AppAction::None
            }
            AppEvent::MessageRewind { index } => {
                self.messages.truncate(index);
                self.overlay_stack.pop();
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
}

/// Returns `Ok(true)` if this is the first time trust was recorded for
/// `project_path` (caller should fire the [`HookTrigger::Setup`] hook),
/// `Ok(false)` if the project was already trusted.
fn persist_trust_accepted(project_path: &str) -> anyhow::Result<bool> {
    let mut state = crate::global_state::load();
    let canonical = std::path::Path::new(project_path)
        .canonicalize()
        .unwrap_or_else(|_| std::path::PathBuf::from(project_path));
    let key = canonical.to_string_lossy().to_string();
    let was_trusted_before = state.project_trust.get(&key).is_some_and(|r| r.accepted);

    crate::global_state::record_trust(&mut state, std::path::Path::new(project_path));
    crate::global_state::save(&state).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(!was_trusted_before)
}
