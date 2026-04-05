//! App state machine and main event loop.

use std::fmt::Write as _;

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::components::input::InputBox;
use crate::components::spinner::Spinner;
use crate::event::TuiEvent;
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
    /// Current pending permission request ID, if any.
    pending_permission: Option<String>,
    /// Whether the app should exit.
    pub should_quit: bool,
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
            pending_permission: None,
            should_quit: false,
        }
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

    fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> AppAction {
        // Global: Ctrl+C / Ctrl+D quits
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && let KeyCode::Char('c' | 'd') = key.code
        {
            self.should_quit = true;
            return AppAction::Quit;
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

    fn handle_confirming_key(&mut self, key: crossterm::event::KeyEvent) -> AppAction {
        match key.code {
            KeyCode::Char('y' | 'Y') | KeyCode::Enter => {
                if let Some(id) = self.pending_permission.take() {
                    self.state = AppState::Processing;
                    self.spinner.start("Executing tool...");
                    return AppAction::PermissionResponse {
                        request_id: id,
                        allowed: true,
                    };
                }
                AppAction::None
            }
            KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                if let Some(id) = self.pending_permission.take() {
                    self.state = AppState::Processing;
                    return AppAction::PermissionResponse {
                        request_id: id,
                        allowed: false,
                    };
                }
                AppAction::None
            }
            _ => AppAction::None,
        }
    }

    fn handle_agent_event(&mut self, event: crab_core::event::Event) {
        use crab_core::event::Event;
        match event {
            Event::ContentDelta { delta, .. } => {
                self.content_buffer.push_str(&delta);
            }
            Event::MessageEnd { .. } => {
                self.spinner.stop();
                self.state = AppState::Idle;
            }
            Event::ToolUseStart { name, .. } => {
                self.spinner.set_message(format!("Running {name}..."));
            }
            Event::ToolResult { .. } => {
                self.spinner.set_message("Thinking...".to_string());
            }
            Event::PermissionRequest {
                request_id,
                tool_name,
                input_summary,
            } => {
                self.spinner.stop();
                self.state = AppState::Confirming;
                self.pending_permission = Some(request_id);
                // Store info for display (simplified — just update spinner message)
                self.spinner
                    .start(format!("Allow {tool_name}? {input_summary} [y/n]"));
            }
            Event::Error { message } => {
                self.spinner.stop();
                let _ = write!(self.content_buffer, "\n[Error: {message}]\n");
                self.state = AppState::Idle;
            }
            _ => {}
        }
    }

    /// Render the full app into a ratatui frame.
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        #[allow(clippy::cast_possible_truncation)]
        let layout = AppLayout::compute(area, self.input.line_count() as u16);

        // Top bar
        render_top_bar(&self.model_name, self.state, layout.top_bar, buf);

        // Content area (just render the buffer text for now)
        render_content(&self.content_buffer, layout.content, buf);

        // Status line / spinner
        Widget::render(&self.spinner, layout.status, buf);

        // Input
        Widget::render(&self.input, layout.input, buf);

        // Bottom bar
        render_bottom_bar(self.state, layout.bottom_bar, buf);
    }
}

fn render_top_bar(model_name: &str, state: AppState, area: Rect, buf: &mut Buffer) {
    let state_str = match state {
        AppState::Idle => "idle",
        AppState::WaitingForInput => "input",
        AppState::Processing => "processing",
        AppState::Confirming => "confirm",
    };

    let line = Line::from(vec![
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
    ]);

    Widget::render(line, area, buf);
}

#[allow(clippy::cast_possible_truncation)]
fn render_content(text: &str, area: Rect, buf: &mut Buffer) {
    if area.height == 0 || text.is_empty() {
        return;
    }

    let lines: Vec<&str> = text.lines().collect();
    let visible = area.height as usize;
    // Show the last N lines (auto-scroll to bottom)
    let start = lines.len().saturating_sub(visible);

    for (i, line) in lines.iter().skip(start).take(visible).enumerate() {
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

fn render_bottom_bar(state: AppState, area: Rect, buf: &mut Buffer) {
    let hints = match state {
        AppState::Idle | AppState::WaitingForInput => {
            "Enter: send | Shift+Enter: newline | Ctrl+C: quit"
        }
        AppState::Processing => "Ctrl+C: quit",
        AppState::Confirming => "y: allow | n: deny | Esc: deny",
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
        // Just verify it doesn't panic and spinner advanced
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
        app.pending_permission = Some("req_1".into());

        let action = app.handle_event(key(KeyCode::Char('y')));
        assert_eq!(
            action,
            AppAction::PermissionResponse {
                request_id: "req_1".into(),
                allowed: true,
            }
        );
        assert_eq!(app.state, AppState::Processing);
    }

    #[test]
    fn confirming_n_denies() {
        let mut app = App::new("test");
        app.state = AppState::Confirming;
        app.pending_permission = Some("req_1".into());

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
        app.pending_permission = Some("req_2".into());

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

        // Verify some expected content is rendered
        let top_row: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(top_row.contains("crab"));
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
}
