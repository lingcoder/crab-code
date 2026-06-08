use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::keybindings::KeyContext;

use super::OverlayAction;

// ---------------------------------------------------------------------------
// State structs for each overlay variant
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct TranscriptState {
    pub scroll_offset: usize,
}

impl TranscriptState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Default)]
pub struct HelpState {
    pub scroll_offset: usize,
}

impl HelpState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug)]
pub struct PermissionDialogState {
    pub request_id: String,
    pub tool_name: String,
    pub summary: String,
    pub selected_option: usize,
}

impl PermissionDialogState {
    #[must_use]
    pub fn new(request_id: String, tool_name: String, summary: String) -> Self {
        Self {
            request_id,
            tool_name,
            summary,
            selected_option: 0,
        }
    }
}

#[derive(Debug, Default)]
pub struct DiffOverlayState {
    pub file_path: String,
    pub scroll_offset: usize,
    pub current_hunk: usize,
    pub total_hunks: usize,
}

impl DiffOverlayState {
    #[must_use]
    pub fn new(file_path: impl Into<String>) -> Self {
        Self {
            file_path: file_path.into(),
            ..Default::default()
        }
    }
}

#[derive(Debug, Default)]
pub struct ModelPickerState {
    pub selected: usize,
    pub models: Vec<String>,
}

impl ModelPickerState {
    #[must_use]
    pub fn new(models: Vec<String>) -> Self {
        Self {
            selected: 0,
            models,
        }
    }
}

#[derive(Debug, Default)]
pub struct SessionPickerState {
    pub selected: usize,
    pub sessions: Vec<SessionEntry>,
    pub scroll_offset: usize,
}

#[derive(Debug, Clone)]
pub struct SessionEntry {
    pub id: String,
    pub title: String,
    pub timestamp: String,
}

impl SessionPickerState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Default)]
pub struct HistorySearchState {
    pub query: String,
    pub results: Vec<String>,
    pub selected: usize,
}

impl HistorySearchState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Default)]
pub struct GlobalSearchState {
    pub query: String,
    pub results: Vec<SearchResult>,
    pub selected: usize,
    pub scroll_offset: usize,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub file_path: String,
    pub line_number: usize,
    pub preview: String,
}

impl GlobalSearchState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Default)]
pub struct PermissionRulesState {
    pub selected: usize,
    pub scroll_offset: usize,
}

impl PermissionRulesState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Default)]
pub struct ThemePickerState {
    pub selected: usize,
    pub themes: Vec<String>,
}

impl ThemePickerState {
    #[must_use]
    pub fn new(themes: Vec<String>) -> Self {
        Self {
            selected: 0,
            themes,
        }
    }
}

#[derive(Debug, Default)]
pub struct DoctorState {
    pub checks: Vec<DoctorCheck>,
    pub scroll_offset: usize,
}

#[derive(Debug, Clone)]
pub struct DoctorCheck {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

impl DoctorState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug)]
pub struct OnboardingState {
    pub step: usize,
    pub total_steps: usize,
}

impl OnboardingState {
    #[must_use]
    pub fn new(total_steps: usize) -> Self {
        Self {
            step: 0,
            total_steps,
        }
    }
}

impl Default for OnboardingState {
    fn default() -> Self {
        Self::new(3)
    }
}

#[derive(Debug, Default)]
pub struct OAuthFlowState {
    pub provider: String,
    pub status: OAuthStatus,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum OAuthStatus {
    #[default]
    WaitingForBrowser,
    PollingForToken,
    Success,
    Failed(String),
}

impl OAuthFlowState {
    #[must_use]
    pub fn new(provider: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            status: OAuthStatus::WaitingForBrowser,
        }
    }
}

#[derive(Debug, Default)]
pub struct ApproveApiKeyState {
    pub provider: String,
    pub selected_option: usize,
}

impl ApproveApiKeyState {
    #[must_use]
    pub fn new(provider: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            selected_option: 0,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum ExportFormat {
    #[default]
    Markdown,
    Json,
    PlainText,
}

#[derive(Debug, Default)]
pub struct ExportState {
    pub format: ExportFormat,
    pub selected_option: usize,
}

impl ExportState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Default)]
pub struct BackgroundTasksState {
    pub selected: usize,
    pub scroll_offset: usize,
}

impl BackgroundTasksState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Default)]
pub struct AgentsPanelState {
    pub selected: usize,
    pub scroll_offset: usize,
}

impl AgentsPanelState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Default)]
pub struct McpPanelState {
    pub selected: usize,
    pub scroll_offset: usize,
}

impl McpPanelState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Default)]
pub struct MemoryPanelState {
    pub selected: usize,
    pub scroll_offset: usize,
}

impl MemoryPanelState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug)]
pub struct CostThresholdState {
    pub current_cost: f64,
    pub threshold: f64,
    pub selected_option: usize,
}

impl CostThresholdState {
    #[must_use]
    pub fn new(current_cost: f64, threshold: f64) -> Self {
        Self {
            current_cost,
            threshold,
            selected_option: 0,
        }
    }
}

#[derive(Debug, Default)]
pub struct MessageSelectorState {
    pub selected: usize,
    pub total_messages: usize,
}

impl MessageSelectorState {
    #[must_use]
    pub fn new(total_messages: usize) -> Self {
        Self {
            selected: 0,
            total_messages,
        }
    }
}

// ---------------------------------------------------------------------------
// OverlayKind enum — every modal overlay the TUI can show
// ---------------------------------------------------------------------------

pub enum OverlayKind {
    Transcript(TranscriptState),
    Help(HelpState),
    Permission(PermissionDialogState),
    Diff(DiffOverlayState),
    ModelPicker(ModelPickerState),
    SessionPicker(SessionPickerState),
    HistorySearch(HistorySearchState),
    GlobalSearch(GlobalSearchState),
    PermissionRules(PermissionRulesState),
    ThemePicker(ThemePickerState),
    Doctor(DoctorState),
    Onboarding(OnboardingState),
    OAuthFlow(OAuthFlowState),
    ApproveApiKey(ApproveApiKeyState),
    Export(ExportState),
    BackgroundTasks(BackgroundTasksState),
    AgentsPanel(AgentsPanelState),
    McpPanel(McpPanelState),
    MemoryPanel(MemoryPanelState),
    CostThreshold(CostThresholdState),
    MessageSelector(MessageSelectorState),
}

impl OverlayKind {
    pub fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        match self {
            Self::Transcript(s) => handle_scrollable_key(&mut s.scroll_offset, key),
            Self::Help(s) => handle_scrollable_key(&mut s.scroll_offset, key),
            Self::Permission(s) => handle_permission_key(s, key),
            Self::Diff(s) => handle_diff_key(s, key),
            Self::ModelPicker(s) => handle_list_key(&mut s.selected, s.models.len(), key),
            Self::SessionPicker(s) => handle_list_key(&mut s.selected, s.sessions.len(), key),
            Self::HistorySearch(s) => handle_list_key(&mut s.selected, s.results.len(), key),
            Self::GlobalSearch(s) => handle_list_key(&mut s.selected, s.results.len(), key),
            Self::PermissionRules(s) => handle_scrollable_key(&mut s.scroll_offset, key),
            Self::ThemePicker(s) => handle_list_key(&mut s.selected, s.themes.len(), key),
            Self::Doctor(s) => handle_scrollable_key(&mut s.scroll_offset, key),
            Self::Onboarding(s) => handle_onboarding_key(s, key),
            Self::OAuthFlow(_) => handle_dismiss_only(key),
            Self::ApproveApiKey(s) => handle_confirm_key(&mut s.selected_option, 2, key),
            Self::Export(s) => handle_confirm_key(&mut s.selected_option, 3, key),
            Self::BackgroundTasks(s) => handle_scrollable_key(&mut s.scroll_offset, key),
            Self::AgentsPanel(s) => handle_scrollable_key(&mut s.scroll_offset, key),
            Self::McpPanel(s) => handle_scrollable_key(&mut s.scroll_offset, key),
            Self::MemoryPanel(s) => handle_scrollable_key(&mut s.scroll_offset, key),
            Self::CostThreshold(s) => handle_confirm_key(&mut s.selected_option, 2, key),
            Self::MessageSelector(s) => handle_list_key(&mut s.selected, s.total_messages, key),
        }
    }

    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        match self {
            Self::Transcript(s) => render_scrollable("Transcript", s.scroll_offset, area, buf),
            Self::Help(s) => render_scrollable("Help", s.scroll_offset, area, buf),
            Self::Permission(s) => render_permission(s, area, buf),
            Self::Diff(s) => render_scrollable_with_title(
                &format!("Diff: {}", s.file_path),
                s.scroll_offset,
                area,
                buf,
            ),
            Self::ModelPicker(s) => render_list("Model", &s.models, s.selected, area, buf),
            Self::SessionPicker(s) => {
                let labels: Vec<String> = s.sessions.iter().map(|e| e.title.clone()).collect();
                render_list("Sessions", &labels, s.selected, area, buf);
            }
            Self::HistorySearch(s) => {
                render_list("History", &s.results, s.selected, area, buf);
            }
            Self::GlobalSearch(s) => {
                let labels: Vec<String> = s.results.iter().map(|r| r.preview.clone()).collect();
                render_list("Search", &labels, s.selected, area, buf);
            }
            Self::PermissionRules(s) => {
                render_scrollable("Permission Rules", s.scroll_offset, area, buf);
            }
            Self::ThemePicker(s) => render_list("Theme", &s.themes, s.selected, area, buf),
            Self::Doctor(s) => render_scrollable("Doctor", s.scroll_offset, area, buf),
            Self::Onboarding(s) => render_onboarding(s, area, buf),
            Self::OAuthFlow(s) => render_oauth(s, area, buf),
            Self::ApproveApiKey(s) => render_confirm(
                &format!("Approve API Key: {}", s.provider),
                s.selected_option,
                area,
                buf,
            ),
            Self::Export(s) => render_confirm("Export", s.selected_option, area, buf),
            Self::BackgroundTasks(s) => {
                render_scrollable("Background Tasks", s.scroll_offset, area, buf);
            }
            Self::AgentsPanel(s) => {
                render_scrollable("Agents", s.scroll_offset, area, buf);
            }
            Self::McpPanel(s) => {
                render_scrollable("MCP Servers", s.scroll_offset, area, buf);
            }
            Self::MemoryPanel(s) => {
                render_scrollable("Memory", s.scroll_offset, area, buf);
            }
            Self::CostThreshold(s) => render_confirm(
                &format!("Cost ${:.2} / ${:.2}", s.current_cost, s.threshold),
                s.selected_option,
                area,
                buf,
            ),
            Self::MessageSelector(_) => {
                render_scrollable("Select Message", 0, area, buf);
            }
        }
    }

    pub fn contexts(&self) -> Vec<KeyContext> {
        match self {
            Self::Transcript(_) => vec![KeyContext::Transcript],
            Self::Help(_) => vec![KeyContext::Help],
            Self::Permission(_)
            | Self::PermissionRules(_)
            | Self::ApproveApiKey(_)
            | Self::CostThreshold(_) => {
                vec![KeyContext::Permission]
            }
            Self::Diff(_) => vec![KeyContext::Diff],
            Self::ModelPicker(_) | Self::ThemePicker(_) => vec![KeyContext::ModelPicker],
            Self::SessionPicker(_) => vec![KeyContext::Sidebar],
            Self::HistorySearch(_) => vec![KeyContext::HistorySearch],
            Self::GlobalSearch(_) => vec![KeyContext::GlobalSearch],
            Self::Doctor(_)
            | Self::Onboarding(_)
            | Self::OAuthFlow(_)
            | Self::McpPanel(_)
            | Self::MemoryPanel(_) => {
                vec![KeyContext::ScrollBox]
            }
            Self::Export(_) => vec![KeyContext::CommandPalette],
            Self::BackgroundTasks(_) => vec![KeyContext::TaskList],
            Self::AgentsPanel(_) => vec![KeyContext::AgentDetail],
            Self::MessageSelector(_) => vec![KeyContext::SelectionMode],
        }
    }

    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Transcript(_) => "transcript",
            Self::Help(_) => "help",
            Self::Permission(_) => "permission",
            Self::Diff(_) => "diff",
            Self::ModelPicker(_) => "model_picker",
            Self::SessionPicker(_) => "session_picker",
            Self::HistorySearch(_) => "history_search",
            Self::GlobalSearch(_) => "global_search",
            Self::PermissionRules(_) => "permission_rules",
            Self::ThemePicker(_) => "theme_picker",
            Self::Doctor(_) => "doctor",
            Self::Onboarding(_) => "onboarding",
            Self::OAuthFlow(_) => "oauth_flow",
            Self::ApproveApiKey(_) => "approve_api_key",
            Self::Export(_) => "export",
            Self::BackgroundTasks(_) => "background_tasks",
            Self::AgentsPanel(_) => "agents_panel",
            Self::McpPanel(_) => "mcp_panel",
            Self::MemoryPanel(_) => "memory_panel",
            Self::CostThreshold(_) => "cost_threshold",
            Self::MessageSelector(_) => "message_selector",
        }
    }
}

// ---------------------------------------------------------------------------
// Shared key handlers — reduce duplication across similar overlay types
// ---------------------------------------------------------------------------

fn handle_scrollable_key(scroll_offset: &mut usize, key: KeyEvent) -> OverlayAction {
    use crossterm::event::KeyCode;
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => OverlayAction::Dismiss,
        KeyCode::Up | KeyCode::Char('k') => {
            *scroll_offset = scroll_offset.saturating_sub(1);
            OverlayAction::Consumed
        }
        KeyCode::Down | KeyCode::Char('j') => {
            *scroll_offset = scroll_offset.saturating_add(1);
            OverlayAction::Consumed
        }
        _ => OverlayAction::Passthrough,
    }
}

fn handle_list_key(selected: &mut usize, len: usize, key: KeyEvent) -> OverlayAction {
    use crossterm::event::KeyCode;
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => OverlayAction::Dismiss,
        KeyCode::Up | KeyCode::Char('k') => {
            *selected = selected.saturating_sub(1);
            OverlayAction::Consumed
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if len > 0 {
                *selected = (*selected + 1).min(len - 1);
            }
            OverlayAction::Consumed
        }
        _ => OverlayAction::Passthrough,
    }
}

fn handle_confirm_key(selected: &mut usize, option_count: usize, key: KeyEvent) -> OverlayAction {
    use crossterm::event::KeyCode;
    match key.code {
        KeyCode::Esc | KeyCode::Char('y' | 'n') | KeyCode::Enter => OverlayAction::Dismiss,
        KeyCode::Up => {
            *selected = selected.saturating_sub(1);
            OverlayAction::Consumed
        }
        KeyCode::Down => {
            *selected = (*selected + 1).min(option_count.saturating_sub(1));
            OverlayAction::Consumed
        }
        _ => OverlayAction::Passthrough,
    }
}

fn handle_dismiss_only(key: KeyEvent) -> OverlayAction {
    use crossterm::event::KeyCode;
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => OverlayAction::Dismiss,
        _ => OverlayAction::Passthrough,
    }
}

fn handle_permission_key(state: &mut PermissionDialogState, key: KeyEvent) -> OverlayAction {
    use crossterm::event::KeyCode;
    match key.code {
        KeyCode::Char('y' | 'n') | KeyCode::Enter | KeyCode::Esc => OverlayAction::Dismiss,
        KeyCode::Up => {
            state.selected_option = state.selected_option.saturating_sub(1);
            OverlayAction::Consumed
        }
        KeyCode::Down => {
            state.selected_option = (state.selected_option + 1).min(3);
            OverlayAction::Consumed
        }
        _ => OverlayAction::Passthrough,
    }
}

fn handle_diff_key(state: &mut DiffOverlayState, key: KeyEvent) -> OverlayAction {
    use crossterm::event::KeyCode;
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => OverlayAction::Dismiss,
        KeyCode::Up | KeyCode::Char('k') => {
            state.scroll_offset = state.scroll_offset.saturating_sub(1);
            OverlayAction::Consumed
        }
        KeyCode::Down | KeyCode::Char('j') => {
            state.scroll_offset = state.scroll_offset.saturating_add(1);
            OverlayAction::Consumed
        }
        KeyCode::Char('n') => {
            if state.total_hunks > 0 {
                state.current_hunk = (state.current_hunk + 1).min(state.total_hunks - 1);
            }
            OverlayAction::Consumed
        }
        KeyCode::Char('p') => {
            state.current_hunk = state.current_hunk.saturating_sub(1);
            OverlayAction::Consumed
        }
        _ => OverlayAction::Passthrough,
    }
}

fn handle_onboarding_key(state: &mut OnboardingState, key: KeyEvent) -> OverlayAction {
    use crossterm::event::KeyCode;
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => OverlayAction::Dismiss,
        KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
            if state.step + 1 >= state.total_steps {
                OverlayAction::Dismiss
            } else {
                state.step += 1;
                OverlayAction::Consumed
            }
        }
        KeyCode::Left | KeyCode::Char('h') => {
            state.step = state.step.saturating_sub(1);
            OverlayAction::Consumed
        }
        _ => OverlayAction::Passthrough,
    }
}

// ---------------------------------------------------------------------------
// Render helpers — lightweight placeholder renders
// ---------------------------------------------------------------------------

fn render_scrollable(title: &str, _scroll: usize, area: Rect, buf: &mut Buffer) {
    use ratatui::style::{Color, Style};
    use ratatui::widgets::{Block, Borders, Clear, Widget};

    let popup = centered_popup(area, 60, 20);
    Widget::render(Clear, popup, buf);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(format!(" {title} "));
    Widget::render(block, popup, buf);
}

fn render_scrollable_with_title(title: &str, scroll: usize, area: Rect, buf: &mut Buffer) {
    render_scrollable(title, scroll, area, buf);
}

fn render_list(title: &str, items: &[String], selected: usize, area: Rect, buf: &mut Buffer) {
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Borders, Clear, Widget};

    let height = (items.len() as u16 + 2).min(20);
    let popup = centered_popup(area, 50, height);
    Widget::render(Clear, popup, buf);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(format!(" {title} "));
    let inner = block.inner(popup);
    Widget::render(block, popup, buf);

    for (i, item) in items.iter().enumerate() {
        if i as u16 >= inner.height {
            break;
        }
        let style = if i == selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let line = Line::from(vec![Span::styled(format!(" {item}"), style)]);
        let row = Rect {
            x: inner.x,
            y: inner.y + i as u16,
            width: inner.width,
            height: 1,
        };
        Widget::render(line, row, buf);
    }
}

fn render_confirm(title: &str, _selected: usize, area: Rect, buf: &mut Buffer) {
    render_scrollable(title, 0, area, buf);
}

fn render_permission(_state: &PermissionDialogState, area: Rect, buf: &mut Buffer) {
    render_scrollable("Permission", 0, area, buf);
}

fn render_onboarding(state: &OnboardingState, area: Rect, buf: &mut Buffer) {
    render_scrollable(
        &format!("Welcome ({}/{})", state.step + 1, state.total_steps),
        0,
        area,
        buf,
    );
}

fn render_oauth(state: &OAuthFlowState, area: Rect, buf: &mut Buffer) {
    let status_text = match &state.status {
        OAuthStatus::WaitingForBrowser => "Waiting for browser...",
        OAuthStatus::PollingForToken => "Polling for token...",
        OAuthStatus::Success => "Authenticated!",
        OAuthStatus::Failed(e) => e.as_str(),
    };
    render_scrollable(
        &format!("OAuth: {} — {}", state.provider, status_text),
        0,
        area,
        buf,
    );
}

fn centered_popup(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    // --- Transcript / Help (scrollable) ---

    #[test]
    fn transcript_esc_dismisses() {
        let mut overlay = OverlayKind::Transcript(TranscriptState::new());
        assert!(matches!(
            overlay.handle_key(key(KeyCode::Esc)),
            OverlayAction::Dismiss
        ));
    }

    #[test]
    fn transcript_scroll() {
        let mut overlay = OverlayKind::Transcript(TranscriptState::new());
        overlay.handle_key(key(KeyCode::Down));
        if let OverlayKind::Transcript(s) = &overlay {
            assert_eq!(s.scroll_offset, 1);
        }
    }

    #[test]
    fn help_q_dismisses() {
        let mut overlay = OverlayKind::Help(HelpState::new());
        assert!(matches!(
            overlay.handle_key(key(KeyCode::Char('q'))),
            OverlayAction::Dismiss
        ));
    }

    // --- Permission dialog ---

    #[test]
    fn permission_y_dismisses() {
        let mut overlay = OverlayKind::Permission(PermissionDialogState::new(
            "1".into(),
            "Bash".into(),
            "ls".into(),
        ));
        assert!(matches!(
            overlay.handle_key(key(KeyCode::Char('y'))),
            OverlayAction::Dismiss
        ));
    }

    #[test]
    fn permission_nav() {
        let mut overlay = OverlayKind::Permission(PermissionDialogState::new(
            "1".into(),
            "Bash".into(),
            "ls".into(),
        ));
        overlay.handle_key(key(KeyCode::Down));
        if let OverlayKind::Permission(s) = &overlay {
            assert_eq!(s.selected_option, 1);
        }
    }

    // --- Diff overlay ---

    #[test]
    fn diff_hunk_navigation() {
        let mut state = DiffOverlayState::new("test.rs");
        state.total_hunks = 5;
        let mut overlay = OverlayKind::Diff(state);

        overlay.handle_key(key(KeyCode::Char('n')));
        if let OverlayKind::Diff(s) = &overlay {
            assert_eq!(s.current_hunk, 1);
        }
        overlay.handle_key(key(KeyCode::Char('p')));
        if let OverlayKind::Diff(s) = &overlay {
            assert_eq!(s.current_hunk, 0);
        }
    }

    // --- Model picker (list) ---

    #[test]
    fn model_picker_nav() {
        let mut overlay = OverlayKind::ModelPicker(ModelPickerState::new(vec![
            "opus".into(),
            "sonnet".into(),
            "haiku".into(),
        ]));
        overlay.handle_key(key(KeyCode::Down));
        if let OverlayKind::ModelPicker(s) = &overlay {
            assert_eq!(s.selected, 1);
        }
        assert!(matches!(
            overlay.handle_key(key(KeyCode::Enter)),
            OverlayAction::Dismiss
        ));
    }

    // --- Onboarding ---

    #[test]
    fn onboarding_steps() {
        let mut overlay = OverlayKind::Onboarding(OnboardingState::new(3));
        assert!(matches!(
            overlay.handle_key(key(KeyCode::Enter)),
            OverlayAction::Consumed
        ));
        if let OverlayKind::Onboarding(s) = &overlay {
            assert_eq!(s.step, 1);
        }
        overlay.handle_key(key(KeyCode::Enter));
        assert!(matches!(
            overlay.handle_key(key(KeyCode::Enter)),
            OverlayAction::Dismiss
        ));
    }

    // --- OAuth flow ---

    #[test]
    fn oauth_dismiss_only() {
        let mut overlay = OverlayKind::OAuthFlow(OAuthFlowState::new("anthropic"));
        assert!(matches!(
            overlay.handle_key(key(KeyCode::Char('a'))),
            OverlayAction::Passthrough
        ));
        assert!(matches!(
            overlay.handle_key(key(KeyCode::Esc)),
            OverlayAction::Dismiss
        ));
    }

    // --- Names ---

    #[test]
    fn overlay_names() {
        assert_eq!(
            OverlayKind::Transcript(TranscriptState::new()).name(),
            "transcript"
        );
        assert_eq!(OverlayKind::Help(HelpState::new()).name(), "help");
        assert_eq!(
            OverlayKind::Diff(DiffOverlayState::new("f.rs")).name(),
            "diff"
        );
        assert_eq!(
            OverlayKind::ModelPicker(ModelPickerState::new(vec![])).name(),
            "model_picker"
        );
        assert_eq!(OverlayKind::Doctor(DoctorState::new()).name(), "doctor");
        assert_eq!(
            OverlayKind::OAuthFlow(OAuthFlowState::new("x")).name(),
            "oauth_flow"
        );
        assert_eq!(OverlayKind::Export(ExportState::new()).name(), "export");
        assert_eq!(
            OverlayKind::McpPanel(McpPanelState::new()).name(),
            "mcp_panel"
        );
    }

    // --- Contexts ---

    #[test]
    fn overlay_contexts() {
        assert_eq!(
            OverlayKind::Transcript(TranscriptState::new()).contexts(),
            vec![KeyContext::Transcript]
        );
        assert_eq!(
            OverlayKind::Help(HelpState::new()).contexts(),
            vec![KeyContext::Help]
        );
        assert_eq!(
            OverlayKind::Diff(DiffOverlayState::new("x")).contexts(),
            vec![KeyContext::Diff]
        );
        assert_eq!(
            OverlayKind::GlobalSearch(GlobalSearchState::new()).contexts(),
            vec![KeyContext::GlobalSearch]
        );
    }

    // --- Render no-panic ---

    #[test]
    fn render_all_variants_no_panic() {
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);

        OverlayKind::Transcript(TranscriptState::new()).render(area, &mut buf);
        OverlayKind::Help(HelpState::new()).render(area, &mut buf);
        OverlayKind::Permission(PermissionDialogState::new(
            "1".into(),
            "B".into(),
            "s".into(),
        ))
        .render(area, &mut buf);
        OverlayKind::Diff(DiffOverlayState::new("test.rs")).render(area, &mut buf);
        OverlayKind::ModelPicker(ModelPickerState::new(vec!["m1".into()])).render(area, &mut buf);
        OverlayKind::SessionPicker(SessionPickerState::new()).render(area, &mut buf);
        OverlayKind::HistorySearch(HistorySearchState::new()).render(area, &mut buf);
        OverlayKind::GlobalSearch(GlobalSearchState::new()).render(area, &mut buf);
        OverlayKind::PermissionRules(PermissionRulesState::new()).render(area, &mut buf);
        OverlayKind::ThemePicker(ThemePickerState::new(vec![])).render(area, &mut buf);
        OverlayKind::Doctor(DoctorState::new()).render(area, &mut buf);
        OverlayKind::Onboarding(OnboardingState::default()).render(area, &mut buf);
        OverlayKind::OAuthFlow(OAuthFlowState::new("test")).render(area, &mut buf);
        OverlayKind::ApproveApiKey(ApproveApiKeyState::new("test")).render(area, &mut buf);
        OverlayKind::Export(ExportState::new()).render(area, &mut buf);
        OverlayKind::BackgroundTasks(BackgroundTasksState::new()).render(area, &mut buf);
        OverlayKind::AgentsPanel(AgentsPanelState::new()).render(area, &mut buf);
        OverlayKind::McpPanel(McpPanelState::new()).render(area, &mut buf);
        OverlayKind::MemoryPanel(MemoryPanelState::new()).render(area, &mut buf);
        OverlayKind::CostThreshold(CostThresholdState::new(1.5, 5.0)).render(area, &mut buf);
        OverlayKind::MessageSelector(MessageSelectorState::new(10)).render(area, &mut buf);
    }
}
