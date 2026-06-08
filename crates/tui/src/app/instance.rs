//! `App` struct + main render loop + private render helpers + tests.

#[cfg(test)]
use super::state::AppAction;
use super::state::{
    ActiveToolInfo, AppState, ChatMessage, ExitKey, PromptInputMode, ThinkingState,
};

use std::collections::HashMap;
use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::clipboard::Clipboard;
use crate::command_queue::CommandQueue;
use crate::components::approval_queue::ApprovalQueue;
use crate::components::autocomplete::AutoComplete;
use crate::components::bottom_bar::BottomBar;
use crate::components::code_block::{CodeBlockTracker, ImagePlaceholder};
use crate::components::context_collapse::ContextCollapse;
use crate::components::input::InputBox;
use crate::components::input_area::InputArea;
use crate::components::notification::NotificationManager;
use crate::components::output_styles::OutputStyles;
use crate::components::search::{self, SearchState};
use crate::components::session_sidebar::SessionSidebar;
use crate::components::spinner::Spinner;
use crate::components::tool_output::ToolOutputList;

use crate::keybindings::Keybindings;
use crate::layout::AppLayout;
use crate::traits::Renderable;
use crate::vim::VimHandler;

/// Main TUI application.
pub struct App {
    /// Tool registry — used to call rendering hooks (`format_use_summary`, `format_result`).
    pub tool_registry: Option<std::sync::Arc<crab_agents::ToolRegistry>>,
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
    pub(super) keybindings: Keybindings,
    /// Cumulative token usage for status bar.
    pub total_input_tokens: u64,
    /// Cumulative output token usage.
    pub total_output_tokens: u64,
    /// Cumulative cost in USD (refreshed from runtime's `CostAccumulator`).
    pub total_cost_usd: f64,
    /// Model context window size in tokens. Set once at session init.
    pub context_window_size: u64,
    /// Content scroll offset (lines from bottom).
    pub(super) content_scroll: usize,
    /// Tool output list with fold/unfold state.
    pub tool_outputs: ToolOutputList,
    /// Code block tracker for copy support.
    pub code_blocks: CodeBlockTracker,
    /// System clipboard access.
    pub(super) clipboard: Clipboard,
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
    /// Snapshot of the agent runtime's team coordinator, refreshed by the
    /// runner after every query. `/team` reads this instead of hitting
    /// the runtime synchronously from the render thread.
    pub team_snapshot: crab_agents::TeamSnapshot,
    /// Current LLM thinking state (extended thinking / chain-of-thought).
    pub thinking: ThinkingState,
    /// Scroll anchor: when the user scrolls up, this holds the line index
    /// where they anchored. `None` means following the tail (auto-scroll).
    pub scroll_anchor: Option<usize>,
    /// Number of new messages received while the user was scrolled up.
    pub(super) unseen_message_count: usize,
    /// Current prompt input mode.
    pub input_mode: PromptInputMode,
    /// Timestamp of last Ctrl+C / Ctrl+D press for double-press detection.
    pub(super) last_interrupt: Option<Instant>,
    /// Which key initiated the current pending exit window. Read by the
    /// bottom bar to pick the correct keyName in the hint.
    pub(super) last_interrupt_key: Option<ExitKey>,
    /// Timestamp of the most recent printable/edit keystroke into the input
    /// box. Used to defer surfacing an incoming permission prompt while the
    /// user is actively typing, so a stray key cannot answer a prompt the user
    /// has not read yet (see `Self::PROMPT_SUPPRESSION`).
    pub(super) last_input_keystroke: Option<Instant>,
    /// Permission cards that arrived while the user was typing. Held back out
    /// of `approval_queue` (so the inline card stays hidden and keys cannot
    /// answer it) until the suppression window elapses, then promoted into
    /// `approval_queue` on a `Tick`.
    pub(super) pending_suppressed: Vec<crate::components::permission::PermissionCard>,
    /// Current permission mode (cycled via Shift+Tab).
    pub permission_mode: crab_core::permission::PermissionMode,
    /// Session-level "always allow" grants (tool names granted via 'a' key).
    pub session_grants: std::collections::HashSet<String>,
    /// Persistent permission rule store (`~/.crab/permissions.json`).
    /// `None` until `init_persistent_permissions` is called (after the home
    /// directory is known). Once loaded, permanent rules are merged into
    /// `session_grants` so they survive across sessions.
    pub permission_rules: Option<crab_core::permission::stored::PermissionRuleSet>,
    /// Open the resume picker once the runtime is ready (set from bare
    /// `--resume`; consumed by `push_startup_overlays`).
    pub open_resume_picker: bool,
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
    pub(super) processing_start: Option<Instant>,
    /// Last render width — used by callers that need to know the width of
    /// the most recently painted content area.
    pub(super) last_render_width: u16,
    /// Lines drained from finalized cells, waiting to be flushed into the
    /// terminal's native scrollback by the next render pass.
    pub pending_history: crate::history::PendingHistory,
    /// Copies of drained finalized messages, kept for scrollback rebuild on
    /// terminal resize. Each entry preserves the `ChatMessage` so the cell
    /// can be re-rendered at a different width.
    pub(super) finalized_history: Vec<ChatMessage>,
    /// The terminal width at which `finalized_history` was last flushed into
    /// scrollback. When the width changes, the scrollback must be rebuilt.
    pub(super) last_scrollback_width: Option<u16>,
    /// Registry of large pastes collapsed into `[Pasted text #N +K lines]`
    /// reference tokens, expanded back to full content at submit.
    pub(super) paste_registry: super::PastedContents,
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
            total_cost_usd: 0.0,
            context_window_size: 0,
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
            team_snapshot: crab_agents::TeamSnapshot::default(),
            thinking: ThinkingState::Idle,
            scroll_anchor: None,
            unseen_message_count: 0,
            input_mode: PromptInputMode::Prompt,
            last_interrupt: None,
            last_interrupt_key: None,
            last_input_keystroke: None,
            pending_suppressed: Vec::new(),
            permission_mode: crab_core::permission::PermissionMode::Default,
            session_grants: std::collections::HashSet::new(),
            permission_rules: None,
            open_resume_picker: false,
            messages: Vec::new(),
            command_queue: CommandQueue::new(),
            stash: None,
            input_history_list: Vec::new(),
            overlay_stack: crate::overlay::OverlayStack::new(),
            vim: VimHandler::new(),
            notifications: NotificationManager::new(),
            processing_start: None,
            last_render_width: 0,
            pending_history: crate::history::PendingHistory::new(),
            finalized_history: Vec::new(),
            last_scrollback_width: None,
            paste_registry: super::PastedContents::new(),
        }
    }

    /// How long after the last keystroke an incoming permission prompt is held
    /// back so a stray key cannot answer a prompt the user has not yet read.
    /// Promoted on the first `Tick` once this window elapses.
    pub(super) const PROMPT_SUPPRESSION: Duration = Duration::from_millis(1500);

    /// Record a printable/edit keystroke into the input box for the
    /// permission-prompt suppression window. Only meaningful while the input
    /// is non-empty (matches the active-typing signal).
    pub(super) fn note_input_keystroke(&mut self) {
        if !self.input.is_empty() {
            self.last_input_keystroke = Some(Instant::now());
        }
    }

    /// Whether an incoming permission prompt should currently be deferred
    /// because the user is actively typing.
    pub(super) fn is_input_suppressing(&self) -> bool {
        !self.input.is_empty()
            && self
                .last_input_keystroke
                .is_some_and(|t| t.elapsed() < Self::PROMPT_SUPPRESSION)
    }

    /// Move any held-back permission cards into the live approval queue and
    /// enter `Confirming`. Called from the `Tick` handler once the suppression
    /// window has elapsed.
    pub(super) fn promote_suppressed_permissions(&mut self) {
        if self.pending_suppressed.is_empty() {
            return;
        }
        for card in self.pending_suppressed.drain(..) {
            self.approval_queue.push(card);
        }
        self.state = AppState::Confirming;
    }

    /// Mark every assistant cell as no-longer-streaming. Called at stream
    /// end so subsequent drains can flush them whole into scrollback.
    pub fn clear_streaming_assistant_flag(&mut self) {
        for msg in self.messages.iter_mut().rev() {
            if let ChatMessage::Assistant { streaming, .. } = msg {
                *streaming = false;
                break;
            }
        }
    }

    /// Whether a periodic `Tick` can still change what is on screen, so the
    /// run loop must keep redrawing every tick rather than idling. True while
    /// any time-driven element is live: the spinner, an unexpired toast, the
    /// thinking label, the double-press exit-hint window, or a permission card
    /// held back behind the typing-suppression window.
    #[must_use]
    pub fn has_active_animation(&self) -> bool {
        self.spinner.is_active()
            || self.notifications.has_active()
            || !matches!(self.thinking, ThinkingState::Idle)
            || self
                .last_interrupt
                .is_some_and(|t| t.elapsed() < Duration::from_millis(800))
            || !self.pending_suppressed.is_empty()
    }

    /// Commit any newly-completed lines from the most recent streaming
    /// assistant cell into `pending_history` so streaming output flows into
    /// terminal scrollback line by line instead of being repainted in the
    /// viewport on every frame.
    pub fn flush_streaming_assistant_lines(&mut self, width: u16) {
        if width == 0 {
            return;
        }
        let Some(idx) = self.messages.iter().rposition(|m| {
            matches!(
                m,
                ChatMessage::Assistant {
                    streaming: true,
                    ..
                }
            )
        }) else {
            return;
        };
        let ChatMessage::Assistant {
            text,
            committed_lines,
            ..
        } = &mut self.messages[idx]
        else {
            return;
        };
        if !text.contains('\n') {
            return;
        }
        let cell = crate::history::cells::AssistantCell::new(text.clone());
        let new_lines = cell.render_committed_lines(width, *committed_lines);
        if new_lines.is_empty() {
            return;
        }
        let count = new_lines.len();
        self.pending_history.extend(new_lines);
        *committed_lines = committed_lines.saturating_add(count);
    }

    /// Drain finalized prefix messages into `pending_history` so the next
    /// render pass can flush them above the inline viewport. Stops at the
    /// first non-finalized cell to keep streaming output anchored to the
    /// viewport. The streaming-tail check (cell is the last AND state is
    /// Processing) prevents draining the assistant turn that's still
    /// receiving `content_delta` events.
    pub fn drain_finalized_into_pending(&mut self, width: u16) {
        if width == 0 {
            return;
        }
        let streaming_tail = matches!(self.state, AppState::Processing | AppState::Confirming);
        let total = self.messages.len();
        let mut drain_count = 0usize;
        for (idx, msg) in self.messages.iter().enumerate() {
            // Keep the very last cell anchored in the viewport while the
            // agent is still producing tokens.
            if streaming_tail && idx + 1 == total {
                break;
            }
            let cell = crate::history::cell_from_chat_message(msg);
            if !cell.is_finalized() {
                break;
            }
            drain_count += 1;
        }
        // Don't split a read-only run across the drain boundary: if the run
        // continues into the viewport, back up to its start so its collapsed
        // "Read N files" summary isn't frozen half-formed into scrollback (it
        // stays in the viewport until the whole run finalizes).
        let is_read_only_run_msg = |m: &ChatMessage| {
            matches!(
                m,
                ChatMessage::ToolUse {
                    is_read_only: true,
                    ..
                } | ChatMessage::ToolResult {
                    is_read_only: true,
                    ..
                }
            )
        };
        if drain_count < total && is_read_only_run_msg(&self.messages[drain_count]) {
            while drain_count > 0 && is_read_only_run_msg(&self.messages[drain_count - 1]) {
                drain_count -= 1;
            }
        }
        if drain_count == 0 {
            return;
        }
        // Preserve drained messages so we can re-render them at a different
        // width when the terminal is resized. Group the same way the viewport
        // does so a run's summary looks identical frozen and live.
        let drained: Vec<ChatMessage> = self.messages.drain(..drain_count).collect();
        for cell in crate::history::group_messages(&drained) {
            self.pending_history.extend(cell.display_lines(width));
        }
        self.finalized_history.extend(drained);
        self.last_scrollback_width = Some(width);
    }

    /// Returns `true` if the terminal width has changed since the last
    /// scrollback flush, indicating that finalized history needs to be
    /// re-rendered at the new width.
    pub fn needs_scrollback_rebuild(&self, current_width: u16) -> bool {
        match self.last_scrollback_width {
            Some(prev) => prev != current_width,
            None => false,
        }
    }

    /// Re-render all finalized history at `new_width` and stage it for
    /// insertion into the terminal scrollback. Call this after a terminal
    /// resize when the width has changed.
    ///
    /// The caller must clear the terminal screen before calling this so
    /// the re-inserted lines replace the stale (old-width) scrollback.
    pub fn rebuild_scrollback(&mut self, new_width: u16) {
        if self.finalized_history.is_empty() {
            self.last_scrollback_width = Some(new_width);
            return;
        }
        for cell in crate::history::group_messages(&self.finalized_history) {
            self.pending_history.extend(cell.display_lines(new_width));
        }
        self.last_scrollback_width = Some(new_width);
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

    /// Seed the input box with text at startup (from `--prefill`) without
    /// submitting it. The cursor lands at the end of the seeded text.
    pub fn seed_input(&mut self, text: &str) {
        if !text.is_empty() {
            self.input.set_text(text);
        }
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
        self.session_grants.clear();
        self.total_input_tokens = 0;
        self.total_output_tokens = 0;
        self.total_cost_usd = 0.0;
        self.content_scroll = 0;
        self.scroll_anchor = None;
        self.unseen_message_count = 0;
        self.command_queue.clear();
        self.pending_suppressed.clear();
        self.last_input_keystroke = None;
        self.finalized_history.clear();
        self.last_scrollback_width = None;
        self.paste_registry.clear();
    }

    /// Rebuild the message list from a loaded conversation.
    pub fn load_session_messages(&mut self, conversation: &crab_agents::Conversation) {
        self.reset_for_new_session();
        self.session_id.clone_from(&conversation.id);

        // Compaction injects its summary back into the conversation as a
        // synthetic user (+ assistant ack) pair. The live `/compact` flow only
        // ever shows a boundary divider for it, so collapse the same pair here
        // to keep resumed sessions consistent with live ones.
        let messages = conversation.messages();
        let mut idx = 0;
        while idx < messages.len() {
            let msg = &messages[idx];
            let text = msg.text();

            if msg.role == crab_core::message::Role::User
                && (text.starts_with(crab_agents::COMPACT_SUMMARY_USER_PREFIX)
                    || text.starts_with(crab_agents::COMPACT_HEURISTIC_USER_PREFIX))
            {
                self.messages.push(ChatMessage::CompactBoundary {
                    strategy: "summary".into(),
                    after_tokens: 0,
                    removed_messages: 0,
                });
                // Swallow the synthetic assistant acknowledgement that the LLM
                // summarize path appends right after the summary message.
                if matches!(messages.get(idx + 1), Some(next)
                    if next.role == crab_core::message::Role::Assistant
                        && next.text() == crab_agents::COMPACT_SUMMARY_ACK)
                {
                    idx += 1;
                }
                idx += 1;
                continue;
            }

            let chat_msg = match msg.role {
                crab_core::message::Role::User => ChatMessage::User { text },
                crab_core::message::Role::Assistant => ChatMessage::Assistant {
                    streaming: false,
                    text,
                    committed_lines: 0,
                },
                crab_core::message::Role::System => ChatMessage::System {
                    text,
                    kind: crate::history::cells::SystemKind::Info,
                },
            };
            self.messages.push(chat_msg);
            idx += 1;
        }
    }

    /// Set custom keybindings.
    pub fn set_keybindings(&mut self, keybindings: Keybindings) {
        self.keybindings = keybindings;
    }

    /// Render the full app into a ratatui frame.
    ///
    /// Delegates to `Renderable` components.
    pub fn render(&mut self, area: Rect, buf: &mut Buffer) {
        #[allow(clippy::cast_possible_truncation)]
        let layout = AppLayout::compute_with_sidebar(
            area,
            self.input.line_count() as u16,
            self.sidebar_visible,
            crate::layout::DEFAULT_SIDEBAR_WIDTH,
        );

        // Session sidebar
        if let Some(sidebar_area) = layout.sidebar {
            Widget::render(&self.session_sidebar, sidebar_area, buf);
        }

        self.last_render_width = layout.content.width;
        crate::history::paint_messages_bottom_up(&self.messages, layout.content, buf);

        if self.spinner.is_active() {
            Widget::render(&self.spinner, layout.status, buf);
        }

        // Separators
        crate::components::header::render_separator(layout.separator_top, buf);

        // Input — delegated to InputArea Renderable. A leading `!` puts the
        // line into bash mode (the submit path runs it as a shell command), so
        // derive the indicator from the text unless a special mode is pinned.
        let render_mode = match self.input_mode {
            PromptInputMode::Prompt | PromptInputMode::Bash
                if self.input.text().starts_with('!') =>
            {
                PromptInputMode::Bash
            }
            other => other,
        };
        let input_area = InputArea {
            input: &self.input,
            mode: render_mode,
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
            .is_some_and(|t| t.elapsed() < Duration::from_millis(800))
            .then(|| self.last_interrupt_key.unwrap_or(ExitKey::CtrlC));
        let bottom_bar = BottomBar {
            state: self.state,
            search_active: self.search.is_active(),
            permission_mode: self.permission_mode,
            chord_prefix: self.keybindings.pending_chord(),
            vim_mode: vim_label,
            exit_pending,
            model_name: Some(self.model_name.as_str()),
            context_used_pct: compute_context_pct(
                self.total_input_tokens,
                self.total_output_tokens,
                self.context_window_size,
            ),
            context_window_size: self.context_window_size,
            used_tokens: self
                .total_input_tokens
                .saturating_add(self.total_output_tokens),
            total_cost_usd: self.total_cost_usd,
            total_input_tokens: self.total_input_tokens,
            total_output_tokens: self.total_output_tokens,
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

    // ── Persistent permission grants ───────────────────────────────────

    /// Load permanent permission rules from `~/.crab/permissions.json` and
    /// seed `session_grants` so they survive across sessions.
    ///
    /// Called once at startup after the home directory is known. Missing or
    /// malformed files are silently treated as empty (consistent with
    /// `PermissionRuleSet::load`).
    pub fn init_persistent_permissions(&mut self, home_dir: &std::path::Path) {
        use crab_core::permission::stored::{RuleScope, RuleVerdict};

        let path = crab_core::permission::stored::PermissionRuleSet::global_path(home_dir);
        let mut rule_set = crab_core::permission::stored::PermissionRuleSet::new(path);
        if rule_set.load().is_err() {
            // Corrupt or unreadable — start clean, don't crash the TUI.
            return;
        }

        // Seed session_grants from permanent Allow rules.
        for rule in rule_set.list_rules() {
            if rule.verdict == RuleVerdict::Allow && rule.scope == RuleScope::Permanent {
                self.session_grants.insert(rule.tool_pattern.clone());
            }
        }

        self.permission_rules = Some(rule_set);
    }

    /// Persist a new "always allow" grant to `~/.crab/permissions.json`.
    ///
    /// Called from the `AllowAlways` handler alongside the existing
    /// `session_grants.insert()`. Writes are best-effort — a failure logs a
    /// warning but does not block the TUI.
    pub fn persist_grant(&mut self, grant_key: &str) {
        use crab_core::permission::stored::{RuleScope, RuleVerdict, StoredPermissionRule};

        let Some(rule_set) = self.permission_rules.as_mut() else {
            return; // persistence not initialised (e.g. unit tests)
        };

        let rule = StoredPermissionRule {
            tool_pattern: grant_key.to_string(),
            verdict: RuleVerdict::Allow,
            scope: RuleScope::Permanent,
            created_at: crab_core::permission::now_iso8601(),
            context: None,
        };
        rule_set.add_rule(rule);
        if let Err(e) = rule_set.save() {
            tracing::warn!("failed to persist permission grant: {e}");
        }
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn compute_context_pct(input: u64, output: u64, window: u64) -> u8 {
    if window == 0 {
        return 0;
    }
    let used = input.saturating_add(output);
    let pct = (used * 100) / window;
    pct.min(100) as u8
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::autocomplete::CommandInfo;
    use crate::components::permission::PermissionCard;
    use crate::event::TuiEvent;
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
            | ChatMessage::Assistant { text, .. }
            | ChatMessage::System { text, .. }
            | ChatMessage::Thinking { text, .. } => text.contains(needle),
            ChatMessage::ToolUse { name, .. } => name.contains(needle),
            ChatMessage::ToolResult {
                tool_name, output, ..
            } => tool_name.contains(needle) || output.contains(needle),
            ChatMessage::ToolProgress {
                tool_name,
                tail_output,
                ..
            } => tool_name.contains(needle) || tail_output.contains(needle),
            ChatMessage::ToolRejected {
                tool_name, summary, ..
            } => tool_name.contains(needle) || summary.contains(needle),
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
    fn ctrl_c_during_processing_returns_interrupt_processing() {
        let mut app = App::new("test");
        app.state = AppState::Processing;
        app.spinner.start("Thinking...");

        let action = app.handle_event(ctrl_key('c'));

        // First Ctrl-C during Processing must signal runner to cancel the
        // in-flight turn so backend SSE stops streaming into an Idle UI.
        assert_eq!(action, AppAction::InterruptProcessing);
        assert_eq!(app.state, AppState::Idle);
        assert!(!app.spinner.is_active());
        assert!(!app.should_quit);
    }

    #[test]
    fn ctrl_c_double_press_during_processing_still_quits() {
        let mut app = App::new("test");
        app.state = AppState::Processing;
        app.spinner.start("Thinking...");

        // First press interrupts the turn but keeps the app alive.
        let first = app.handle_event(ctrl_key('c'));
        assert_eq!(first, AppAction::InterruptProcessing);
        assert!(!app.should_quit);

        // Second press within 800ms exits the app.
        let second = app.handle_event(ctrl_key('c'));
        assert_eq!(second, AppAction::Quit);
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
    fn first_ctrl_c_records_ctrl_c_exit_key() {
        let mut app = App::new("test");
        app.handle_event(ctrl_key('c'));
        assert_eq!(app.last_interrupt_key, Some(ExitKey::CtrlC));
    }

    #[test]
    fn first_ctrl_d_records_ctrl_d_exit_key() {
        let mut app = App::new("test");
        app.handle_event(ctrl_key('d'));
        assert_eq!(app.last_interrupt_key, Some(ExitKey::CtrlD));
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
        app.handle_event(TuiEvent::Agent {
            epoch: 0,
            event: crab_core::event::Event::ContentDelta {
                index: 0,
                delta: "Hello ".into(),
            },
        });
        app.handle_event(TuiEvent::Agent {
            epoch: 0,
            event: crab_core::event::Event::ContentDelta {
                index: 0,
                delta: "world".into(),
            },
        });
        assert!(messages_contain(&app.messages, "Hello world"));
        assert_eq!(app.content_scroll, 0); // auto-scrolled
    }

    #[test]
    fn agent_message_end_stops_spinner() {
        let mut app = App::new("test");
        app.state = AppState::Processing;
        app.spinner.start("Thinking...");

        app.handle_event(TuiEvent::Agent {
            epoch: 0,
            event: crab_core::event::Event::MessageEnd {
                usage: crab_core::model::TokenUsage::default(),
            },
        });

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
        app.handle_event(TuiEvent::Agent {
            epoch: 0,
            event: crab_core::event::Event::MessageEnd { usage },
        });

        assert_eq!(app.total_input_tokens, 100);
        assert_eq!(app.total_output_tokens, 50);

        // Second turn
        app.state = AppState::Processing;
        let usage2 = crab_core::model::TokenUsage {
            input_tokens: 200,
            output_tokens: 80,
            ..Default::default()
        };
        app.handle_event(TuiEvent::Agent {
            epoch: 0,
            event: crab_core::event::Event::MessageEnd { usage: usage2 },
        });

        assert_eq!(app.total_input_tokens, 300);
        assert_eq!(app.total_output_tokens, 130);
    }

    #[test]
    fn agent_tool_use_updates_spinner() {
        let mut app = App::new("test");
        app.state = AppState::Processing;
        app.spinner.start("Thinking...");

        app.handle_event(TuiEvent::Agent {
            epoch: 0,
            event: crab_core::event::Event::ToolUseStart {
                id: "tu_1".into(),
                name: "bash".into(),
                input: serde_json::json!({"command": "ls"}),
            },
        });

        assert!(app.spinner.message().contains("bash"));
    }

    #[test]
    fn permission_request_enters_confirming() {
        let mut app = App::new("test");
        app.state = AppState::Processing;

        app.handle_event(TuiEvent::Agent {
            epoch: 0,
            event: crab_core::event::Event::PermissionRequest {
                tool_name: "bash".into(),
                input_summary: "rm -rf /tmp".into(),
                request_id: "req_1".into(),
                tool_input: serde_json::json!({"command": "rm -rf /tmp"}),
            },
        });

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
            &serde_json::Value::Null,
        ));

        let action = app.handle_event(key(KeyCode::Char('y')));
        assert_eq!(
            action,
            AppAction::PermissionResponse {
                request_id: "req_1".into(),
                allowed: true,
                feedback: None,
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
            &serde_json::Value::Null,
        ));

        let action = app.handle_event(key(KeyCode::Char('n')));
        assert_eq!(
            action,
            AppAction::PermissionResponse {
                request_id: "req_1".into(),
                allowed: false,
                feedback: None,
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
            &serde_json::Value::Null,
        ));

        let action = app.handle_event(key(KeyCode::Esc));
        assert_eq!(
            action,
            AppAction::PermissionResponse {
                request_id: "req_2".into(),
                allowed: false,
                feedback: None,
            }
        );
    }

    #[test]
    fn confirming_tab_then_text_then_enter_emits_deny_with_feedback() {
        let mut app = App::new("test");
        app.state = AppState::Confirming;
        app.approval_queue.push(PermissionCard::from_event(
            "bash",
            "rm -rf /tmp",
            "req_3".into(),
            &serde_json::Value::Null,
        ));

        // Tab enters feedback mode — no decision yet.
        let action = app.handle_event(key(KeyCode::Tab));
        assert_eq!(action, AppAction::None);
        assert_eq!(app.state, AppState::Confirming);

        // Type a feedback note (ASCII chars route through the card while
        // in feedback mode).
        for c in "use Read tool".chars() {
            let action = app.handle_event(key(KeyCode::Char(c)));
            assert_eq!(action, AppAction::None);
        }

        // Enter submits a deny carrying the feedback string.
        let action = app.handle_event(key(KeyCode::Enter));
        assert_eq!(
            action,
            AppAction::PermissionResponse {
                request_id: "req_3".into(),
                allowed: false,
                feedback: Some("use Read tool".into()),
            }
        );
        assert_eq!(app.state, AppState::Processing);
        assert!(app.approval_queue.is_empty());
        // The user-feedback note should be appended as a User message so the
        // transcript shows what was sent back to the model.
        assert!(messages_contain(&app.messages, "(feedback) use Read tool"));
    }

    #[test]
    fn confirming_tab_then_esc_cancels_back_to_decision_mode() {
        let mut app = App::new("test");
        app.state = AppState::Confirming;
        app.approval_queue.push(PermissionCard::from_event(
            "bash",
            "ls -la",
            "req_4".into(),
            &serde_json::Value::Null,
        ));

        app.handle_event(key(KeyCode::Tab));
        for c in "draft".chars() {
            app.handle_event(key(KeyCode::Char(c)));
        }
        // Esc inside feedback mode cancels — no decision, no state change.
        let action = app.handle_event(key(KeyCode::Esc));
        assert_eq!(action, AppAction::None);
        assert_eq!(app.state, AppState::Confirming);
        // y now works again because we exited feedback mode.
        let action = app.handle_event(key(KeyCode::Char('y')));
        assert_eq!(
            action,
            AppAction::PermissionResponse {
                request_id: "req_4".into(),
                allowed: true,
                feedback: None,
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
            &serde_json::Value::Null,
        ));
        app.approval_queue.push(PermissionCard::from_event(
            "edit",
            "src/main.rs",
            "req_2".into(),
            &serde_json::Value::Null,
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

        app.handle_event(TuiEvent::Agent {
            epoch: 0,
            event: crab_core::event::Event::Error {
                message: "rate limit".into(),
            },
        });

        assert_eq!(app.state, AppState::Idle);
        assert!(!app.spinner.is_active());
        // The classifier rewrites "rate limit" to a user-friendly message
        // and tags it as a Warning kind.
        assert!(messages_contain(&app.messages, "Rate limit reached"));
        let last_kind = app.messages.iter().rev().find_map(|m| match m {
            ChatMessage::System { kind, .. } => Some(*kind),
            _ => None,
        });
        assert_eq!(last_kind, Some(crate::history::cells::SystemKind::Warning));
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
                interruptible: true,
            },
        );

        app.handle_event(TuiEvent::Agent {
            epoch: 0,
            event: crab_core::event::Event::ToolResult {
                id: "tu_1".into(),
                output: crab_core::tool::ToolOutput::success("file1.txt\nfile2.txt"),
            },
        });

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
                interruptible: true,
            },
        );

        app.handle_event(TuiEvent::Agent {
            epoch: 0,
            event: crab_core::event::Event::ToolResult {
                id: "tu_1".into(),
                output: crab_core::tool::ToolOutput::error("command not found"),
            },
        });

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

        app.handle_event(TuiEvent::Agent {
            epoch: 0,
            event: crab_core::event::Event::ToolUseStart {
                id: "tu_1".into(),
                name: "read".into(),
                input: serde_json::json!({"file_path": "test.rs"}),
            },
        });

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
            &serde_json::Value::Null,
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

        let null = serde_json::Value::Null;
        let card = PermissionCard::from_event("bash", "ls -la", "r1".into(), &null);
        assert!(matches!(card.kind, PermissionKind::Bash { .. }));

        let card = PermissionCard::from_event("edit", "file.rs", "r2".into(), &null);
        assert!(matches!(card.kind, PermissionKind::FileEdit { .. }));

        let card = PermissionCard::from_event("write", "out.txt", "r3".into(), &null);
        assert!(matches!(card.kind, PermissionKind::FileWrite { .. }));

        let card = PermissionCard::from_event("custom_tool", "data", "r4".into(), &null);
        assert!(matches!(card.kind, PermissionKind::Generic { .. }));
    }

    #[test]
    fn session_saved_event_shown() {
        let mut app = App::new("test");
        app.handle_event(TuiEvent::Agent {
            epoch: 0,
            event: crab_core::event::Event::SessionSaved {
                session_id: "sess_abc".into(),
            },
        });
        assert!(messages_contain(&app.messages, "sess_abc"));
    }

    #[test]
    fn token_warning_shown() {
        let mut app = App::new("test");
        app.handle_event(TuiEvent::Agent {
            epoch: 0,
            event: crab_core::event::Event::TokenWarning {
                usage_pct: 0.90,
                used: 90000,
                limit: 100_000,
            },
        });
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

    // ── Sidebar tests ──

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

        app.handle_event(TuiEvent::Agent {
            epoch: 0,
            event: crab_core::event::Event::ContentDelta {
                index: 0,
                delta: "new text".into(),
            },
        });
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

        app.handle_event(TuiEvent::Agent {
            epoch: 0,
            event: crab_core::event::Event::ContentDelta {
                index: 0,
                delta: "line1\nline2\n".into(),
            },
        });

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
    fn render_bash_mode_shows_bang_indicator() {
        // Bash mode swaps the `❯` chevron for a `!` indicator, without any
        // bracketed `[bash]` text label.
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
        assert!(text.contains('!'));
        assert!(!text.contains('❯'));
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
    /// `active_tools` by `tool_use_id`, producing the correct `ChatMessage::ToolResult`.
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

        assert!(!app.active_tools.contains_key("tu_1"));

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
            ChatMessage::Assistant { text, .. } => assert_eq!(text, "Hello"),
            other => panic!("expected Assistant, got {other:?}"),
        }

        // Second delta — appends to the existing Assistant message AND
        // appends to the mirror. Both sides must stay in sync.
        app.apply_event(AppEvent::ContentAppend(", world!\n".to_string()));
        assert_eq!(app.content_buffer, "Hello, world!\n");
        assert_eq!(app.messages.len(), 1);
        match app.messages.last().unwrap() {
            ChatMessage::Assistant { text, .. } => assert_eq!(text, "Hello, world!\n"),
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
    fn reset_for_new_session_clears_session_grants() {
        let mut app = App::new("test");
        app.session_grants.insert("Bash".to_string());
        app.session_grants.insert("Write".to_string());
        app.reset_for_new_session();
        assert!(app.session_grants.is_empty());
    }

    #[test]
    fn reset_for_new_session_preserves_welcome() {
        let mut app = App::new("test");
        app.messages.push(ChatMessage::Welcome {
            version: "0.1.0".into(),
            whats_new: String::new(),
            show_project_hint: false,
            model: String::new(),
            working_dir: String::new(),
        });
        app.messages.push(ChatMessage::User { text: "hi".into() });
        app.messages.push(ChatMessage::Assistant {
            streaming: false,
            committed_lines: 0,
            text: "hello".into(),
        });
        app.reset_for_new_session();
        assert_eq!(app.messages.len(), 1);
        assert!(matches!(app.messages[0], ChatMessage::Welcome { .. }));
    }

    #[test]
    fn enter_during_processing_with_interruptible_tool_interrupts() {
        let mut app = App::new("test");
        app.state = AppState::Processing;
        app.active_tools.insert(
            "tu_1".into(),
            ActiveToolInfo {
                name: "sleep".into(),
                input: serde_json::Value::Null,
                progress: None,
                interruptible: true,
            },
        );
        app.handle_event(key(KeyCode::Char('h')));
        app.handle_event(key(KeyCode::Char('i')));
        let action = app.handle_event(key(KeyCode::Enter));
        assert_eq!(action, AppAction::InterruptProcessing);
        assert_eq!(app.command_queue.len(), 1);
    }

    #[test]
    fn enter_during_processing_with_blocking_tool_only_queues() {
        let mut app = App::new("test");
        app.state = AppState::Processing;
        app.active_tools.insert(
            "tu_1".into(),
            ActiveToolInfo {
                name: "apply".into(),
                input: serde_json::Value::Null,
                progress: None,
                interruptible: false,
            },
        );
        app.handle_event(key(KeyCode::Char('h')));
        let action = app.handle_event(key(KeyCode::Enter));
        assert_eq!(action, AppAction::None);
        assert_eq!(app.command_queue.len(), 1);
        assert_eq!(app.state, AppState::Processing);
    }

    #[test]
    fn permission_request_deferred_while_typing() {
        let mut app = App::new("test");
        app.state = AppState::Processing;
        app.handle_event(key(KeyCode::Char('h')));
        assert!(!app.input.is_empty());

        app.handle_event(TuiEvent::Agent {
            epoch: 0,
            event: crab_core::event::Event::PermissionRequest {
                tool_name: "bash".into(),
                input_summary: "rm -rf /tmp".into(),
                request_id: "req_1".into(),
                tool_input: serde_json::json!({"command": "rm -rf /tmp"}),
            },
        });

        assert_ne!(app.state, AppState::Confirming);
        assert!(app.approval_queue.is_empty());
        assert!(!app.pending_suppressed.is_empty());
    }

    #[test]
    fn suppressed_permission_promoted_on_tick() {
        let mut app = App::new("test");
        app.state = AppState::Processing;
        app.input.set_text("draft");
        app.last_input_keystroke =
            Some(Instant::now().checked_sub(Duration::from_secs(2)).unwrap());
        app.pending_suppressed.push(PermissionCard::from_event(
            "bash",
            "rm -rf /tmp",
            "req_1".into(),
            &serde_json::Value::Null,
        ));

        app.handle_event(TuiEvent::Tick);

        assert_eq!(app.state, AppState::Confirming);
        assert!(!app.approval_queue.is_empty());
        assert!(app.pending_suppressed.is_empty());
    }

    #[test]
    fn granted_tool_not_deferred_while_typing() {
        let mut app = App::new("test");
        app.state = AppState::Processing;
        // The grant uses the scoped key AllowAlways stores for `ls`, and the
        // incoming event carries the capitalized registry name `Bash`.
        app.session_grants.insert("Bash:ls".to_string());
        app.handle_event(key(KeyCode::Char('h')));

        let action = app.handle_event(TuiEvent::Agent {
            epoch: 0,
            event: crab_core::event::Event::PermissionRequest {
                tool_name: "Bash".into(),
                input_summary: "ls".into(),
                request_id: "req_1".into(),
                tool_input: serde_json::json!({"command": "ls"}),
            },
        });

        assert_eq!(
            action,
            AppAction::PermissionResponse {
                request_id: "req_1".into(),
                allowed: true,
                feedback: None,
            }
        );
        assert!(app.pending_suppressed.is_empty());
    }

    #[test]
    fn ctrl_c_idle_with_queue_restores_input() {
        let mut app = App::new("test");
        app.state = AppState::Idle;
        app.command_queue.push("queued cmd".into());

        let action = app.handle_event(ctrl_key('c'));
        assert_eq!(action, AppAction::None);
        assert!(app.command_queue.is_empty());
        assert_eq!(app.input.text(), "queued cmd");
        assert!(app.last_interrupt.is_none());
    }

    #[test]
    fn esc_idle_with_queue_restores_input() {
        let mut app = App::new("test");
        app.state = AppState::Idle;
        app.command_queue.push("queued cmd".into());

        let action = app.handle_event(key(KeyCode::Esc));
        assert_eq!(action, AppAction::None);
        assert!(app.command_queue.is_empty());
        assert_eq!(app.input.text(), "queued cmd");
    }

    #[test]
    fn ctrl_c_idle_empty_queue_still_arms_exit() {
        let mut app = App::new("test");
        app.state = AppState::Idle;
        let first = app.handle_event(ctrl_key('c'));
        assert_eq!(first, AppAction::None);
        assert!(!app.should_quit);
        assert!(app.last_interrupt.is_some());
        let second = app.handle_event(ctrl_key('c'));
        assert_eq!(second, AppAction::Quit);
        assert!(app.should_quit);
    }

    #[test]
    fn restore_queued_input_prepends_to_current() {
        let mut app = App::new("test");
        app.command_queue.push("a".into());
        app.command_queue.push("b".into());
        app.input.set_text("typed");
        app.restore_queued_input();
        assert_eq!(app.input.text(), "a\nb\ntyped");
        assert!(app.command_queue.is_empty());
    }

    #[test]
    fn has_active_animation_false_when_idle() {
        let app = App::new("test");
        assert!(!app.has_active_animation());
    }

    #[test]
    fn has_active_animation_true_while_spinner_active() {
        let mut app = App::new("test");
        app.spinner.start("Working");
        assert!(app.has_active_animation());
    }

    #[test]
    fn has_active_animation_true_with_pending_permission() {
        let mut app = App::new("test");
        app.pending_suppressed.push(PermissionCard::from_event(
            "bash",
            "ls",
            "req_1".into(),
            &serde_json::Value::Null,
        ));
        assert!(app.has_active_animation());
    }

    #[test]
    fn restore_cancelled_prompt_pops_user_and_restores_input() {
        let mut app = App::new("test");
        app.messages.push(ChatMessage::User {
            text: "draft prompt".into(),
        });
        app.messages.push(ChatMessage::System {
            text: "Interrupted".into(),
            kind: crate::history::cells::SystemKind::Info,
        });
        app.input_history_list.push("draft prompt".into());

        app.restore_cancelled_prompt();

        assert!(
            !app.messages
                .iter()
                .any(|m| matches!(m, ChatMessage::User { .. }))
        );
        assert_eq!(app.input.text(), "draft prompt");
        assert!(app.input_history_list.is_empty());
        // The Interrupted system cell stays.
        assert!(
            app.messages
                .iter()
                .any(|m| matches!(m, ChatMessage::System { .. }))
        );
    }

    #[test]
    fn shift_tab_cycles_permission_mode_and_syncs() {
        use crab_core::permission::PermissionMode;
        let mut app = App::new("test");
        let action = app.handle_event(TuiEvent::Key(KeyEvent::new(
            KeyCode::BackTab,
            KeyModifiers::SHIFT,
        )));
        assert_eq!(app.permission_mode, PermissionMode::AcceptEdits);
        assert_eq!(
            action,
            AppAction::SyncPermissionMode(PermissionMode::AcceptEdits)
        );
    }

    #[test]
    fn overlay_passthrough_falls_through_to_input() {
        let mut app = App::new("test");
        app.overlay_stack.push(Box::new(
            crate::components::shortcut_hint::HelpOverlay::new(),
        ));
        // The help overlay declines 'z' (Passthrough), so it must reach the
        // input box instead of being swallowed by the overlay block.
        app.handle_event(key(KeyCode::Char('z')));
        assert_eq!(app.state, AppState::WaitingForInput);
        assert_eq!(app.input.text(), "z");
    }

    #[test]
    fn overlay_consumed_key_does_not_reach_input() {
        let mut app = App::new("test");
        app.overlay_stack.push(Box::new(
            crate::components::shortcut_hint::HelpOverlay::new(),
        ));
        app.handle_event(key(KeyCode::Esc));
        assert!(app.input.is_empty());
        assert_eq!(app.state, AppState::Idle);
    }

    #[test]
    fn tick_keeps_fresh_chord_prefix_armed() {
        let mut app = App::new("test");
        // Ctrl+K is both a binding and a chord prefix, so it arms a pending
        // prefix; a Tick well within the timeout must keep it armed.
        app.handle_event(ctrl_key('k'));
        assert!(app.keybindings.pending_chord().is_some());
        app.handle_event(TuiEvent::Tick);
        assert!(app.keybindings.pending_chord().is_some());
    }

    #[test]
    fn ctrl_f_search_stays_active_and_cycles_on_enter() {
        let mut app = App::new("test");
        app.content_buffer = "x a\nx b\nx c\n".into();
        app.handle_event(ctrl_key('f'));
        app.handle_event(key(KeyCode::Char('x')));
        assert!(app.search.is_active());
        assert_eq!(app.search.match_count(), 3);
        app.handle_event(key(KeyCode::Enter));
        assert!(app.search.is_active());
        assert_eq!(app.search.current_match_index(), 1);
        app.handle_event(key(KeyCode::Enter));
        assert_eq!(app.search.current_match_index(), 2);
        app.handle_event(key(KeyCode::Enter));
        assert_eq!(app.search.current_match_index(), 0);
        assert!(app.search.is_active());
    }

    #[test]
    fn search_shift_enter_steps_back() {
        let mut app = App::new("test");
        app.content_buffer = "x a\nx b\nx c\n".into();
        app.handle_event(ctrl_key('f'));
        app.handle_event(key(KeyCode::Char('x')));
        let shift_enter = TuiEvent::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
        app.handle_event(shift_enter);
        assert_eq!(app.search.current_match_index(), 2);
        assert!(app.search.is_active());
    }

    #[test]
    fn search_esc_exits() {
        let mut app = App::new("test");
        app.content_buffer = "x a\nx b\n".into();
        app.handle_event(ctrl_key('f'));
        app.handle_event(key(KeyCode::Char('x')));
        app.handle_event(key(KeyCode::Esc));
        assert!(!app.search.is_active());
        assert!(app.search.query().is_empty());
    }

    // Alt+V resolves to EnterSelectionMode in the Chat context; on Windows the
    // Input context binds Alt+V to image paste, which shadows it (focus chain
    // checks Input before Chat). Selection mode is therefore reachable via
    // Alt+V only on non-Windows.
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn alt_v_opens_message_selector() {
        let mut app = App::new("test");
        app.messages.push(ChatMessage::User { text: "hi".into() });
        app.handle_event(TuiEvent::Key(KeyEvent::new(
            KeyCode::Char('v'),
            KeyModifiers::ALT,
        )));
        assert!(!app.overlay_stack.is_empty());
    }

    #[test]
    fn ctrl_k_ctrl_s_opens_global_search() {
        let mut app = App::new("test");
        app.messages.push(ChatMessage::User { text: "hi".into() });
        app.handle_event(ctrl_key('k'));
        app.handle_event(ctrl_key('s'));
        assert!(!app.overlay_stack.is_empty());
    }

    #[test]
    fn resume_collapses_llm_summary_pair_into_boundary() {
        let mut conv = crab_agents::Conversation::new("s1".into(), String::new(), 100_000);
        conv.push_user("hello");
        conv.push_assistant("hi there");
        conv.push_user(format!(
            "{}\nold stuff happened",
            crab_agents::COMPACT_SUMMARY_USER_PREFIX
        ));
        conv.push_assistant(crab_agents::COMPACT_SUMMARY_ACK);
        conv.push_user("what next?");

        let mut app = App::new("test");
        app.load_session_messages(&conv);

        // The summary user message + assistant ack collapse to one boundary; the
        // surrounding real turns survive verbatim.
        let boundaries = app
            .messages
            .iter()
            .filter(|m| matches!(m, ChatMessage::CompactBoundary { .. }))
            .count();
        assert_eq!(boundaries, 1);
        assert!(!messages_contain(
            &app.messages,
            crab_agents::COMPACT_SUMMARY_USER_PREFIX
        ));
        assert!(!messages_contain(
            &app.messages,
            crab_agents::COMPACT_SUMMARY_ACK
        ));
        assert!(messages_contain(&app.messages, "what next?"));
        assert!(messages_contain(&app.messages, "hello"));
    }

    #[test]
    fn resume_collapses_heuristic_summary_into_boundary() {
        let mut conv = crab_agents::Conversation::new("s2".into(), String::new(), 100_000);
        conv.push_user(format!(
            "{}\n\nsummary body",
            crab_agents::COMPACT_HEURISTIC_USER_PREFIX
        ));
        conv.push_user("resumed question");

        let mut app = App::new("test");
        app.load_session_messages(&conv);

        assert_eq!(
            app.messages
                .iter()
                .filter(|m| matches!(m, ChatMessage::CompactBoundary { .. }))
                .count(),
            1
        );
        // No follower ack for the heuristic path, so the next real user message
        // must remain intact.
        assert!(messages_contain(&app.messages, "resumed question"));
    }

    #[test]
    fn persistent_permissions_seed_session_grants() {
        use crab_core::permission::stored::{
            PermissionRuleSet, RuleScope, RuleVerdict, StoredPermissionRule,
        };

        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        // Write a permanent Allow rule to disk.
        let path = PermissionRuleSet::global_path(home);
        let mut set = PermissionRuleSet::new(path);
        set.add_rule(StoredPermissionRule {
            tool_pattern: "Bash:git".to_string(),
            verdict: RuleVerdict::Allow,
            scope: RuleScope::Permanent,
            created_at: "2026-06-03T00:00:00Z".to_string(),
            context: None,
        });
        set.save().unwrap();

        // App starts empty, init loads the rule.
        let mut app = App::new("test");
        assert!(!app.session_grants.contains("Bash:git"));
        app.init_persistent_permissions(home);
        assert!(app.session_grants.contains("Bash:git"));
        assert!(app.permission_rules.is_some());
    }

    #[test]
    fn persistent_permissions_missing_file_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new("test");
        app.init_persistent_permissions(dir.path());
        // No crash, no rules loaded, permission_rules still set (empty set).
        assert!(app.permission_rules.is_some());
        assert!(app.session_grants.is_empty());
    }

    #[test]
    fn persist_grant_writes_to_disk() {
        use crab_core::permission::stored::PermissionRuleSet;

        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let mut app = App::new("test");
        app.init_persistent_permissions(home);

        // Grant and persist.
        app.persist_grant("Edit");

        // Reload from disk — the rule should be there.
        let path = PermissionRuleSet::global_path(home);
        let mut set2 = PermissionRuleSet::new(path);
        set2.load().unwrap();
        assert_eq!(set2.list_permanent_rules().len(), 1);
        assert_eq!(set2.list_permanent_rules()[0].tool_pattern, "Edit");
    }

    #[test]
    fn persist_grant_noop_without_init() {
        let mut app = App::new("test");
        // permission_rules is None — persist_grant should silently return.
        app.persist_grant("Bash:git");
        // No panic, no error.
    }
}
