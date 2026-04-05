//! Permission confirmation dialog for tool execution.

use crossterm::event::KeyCode;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap};

/// Risk level for a tool execution request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

impl RiskLevel {
    fn color(self) -> Color {
        match self {
            Self::Low => Color::Green,
            Self::Medium => Color::Yellow,
            Self::High => Color::Red,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

/// User response to a permission dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionResponse {
    Allow,
    Deny,
    AlwaysAllow,
}

/// Permission confirmation dialog state.
pub struct PermissionDialog {
    pub tool_name: String,
    pub input_summary: String,
    pub risk: RiskLevel,
    pub request_id: String,
    selected: usize,
    options: Vec<(&'static str, PermissionResponse)>,
}

impl PermissionDialog {
    pub fn new(
        tool_name: impl Into<String>,
        input_summary: impl Into<String>,
        risk: RiskLevel,
        request_id: impl Into<String>,
    ) -> Self {
        Self {
            tool_name: tool_name.into(),
            input_summary: input_summary.into(),
            risk,
            request_id: request_id.into(),
            selected: 0,
            options: vec![
                ("Yes", PermissionResponse::Allow),
                ("No", PermissionResponse::Deny),
                ("Always allow", PermissionResponse::AlwaysAllow),
            ],
        }
    }

    /// Handle a key event. Returns `Some(response)` when the user confirms.
    pub fn handle_key(&mut self, code: KeyCode) -> Option<PermissionResponse> {
        match code {
            KeyCode::Left | KeyCode::Char('h') => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
                None
            }
            KeyCode::Right | KeyCode::Char('l') => {
                if self.selected < self.options.len() - 1 {
                    self.selected += 1;
                }
                None
            }
            KeyCode::Enter | KeyCode::Char(' ') => Some(self.options[self.selected].1),
            KeyCode::Char('y' | 'Y') => Some(PermissionResponse::Allow),
            KeyCode::Char('n' | 'N') | KeyCode::Esc => Some(PermissionResponse::Deny),
            KeyCode::Char('a' | 'A') => Some(PermissionResponse::AlwaysAllow),
            _ => None,
        }
    }

    /// Currently selected option index.
    #[must_use]
    pub const fn selected(&self) -> usize {
        self.selected
    }

    /// Compute the centered dialog area within the given terminal area.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub fn dialog_area(terminal: Rect) -> Rect {
        let width = 60.min(terminal.width.saturating_sub(4));
        let height = 10.min(terminal.height.saturating_sub(2));
        let x = (terminal.width.saturating_sub(width)) / 2;
        let y = (terminal.height.saturating_sub(height)) / 2;
        Rect::new(x, y, width, height)
    }
}

impl Widget for &PermissionDialog {
    #[allow(clippy::cast_possible_truncation)]
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 6 || area.width < 20 {
            return;
        }

        // Clear the area behind the dialog
        Widget::render(Clear, area, buf);

        let block = Block::default()
            .title(" Permission Required ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow));

        let inner = block.inner(area);
        Widget::render(block, area, buf);

        if inner.height < 4 || inner.width < 10 {
            return;
        }

        // Layout: tool info, summary, risk, buttons
        let chunks = Layout::vertical([
            Constraint::Length(1), // tool name + risk
            Constraint::Length(1), // spacer
            Constraint::Min(1),    // input summary
            Constraint::Length(1), // spacer
            Constraint::Length(1), // buttons
        ])
        .split(inner);

        // Tool name + risk badge
        let risk_style = Style::default()
            .fg(self.risk.color())
            .add_modifier(Modifier::BOLD);
        let tool_line = Line::from(vec![
            Span::styled("Tool: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                &self.tool_name,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled("[", Style::default().fg(Color::DarkGray)),
            Span::styled(self.risk.label(), risk_style),
            Span::styled("]", Style::default().fg(Color::DarkGray)),
        ]);
        Widget::render(tool_line, chunks[0], buf);

        // Input summary (wrapping paragraph)
        let summary = Paragraph::new(self.input_summary.as_str())
            .style(Style::default().fg(Color::Gray))
            .wrap(Wrap { trim: true });
        Widget::render(summary, chunks[2], buf);

        // Button row
        let button_spans: Vec<Span> = self
            .options
            .iter()
            .enumerate()
            .flat_map(|(i, (label, _))| {
                let style = if i == self.selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                let mut spans = vec![Span::styled(format!(" {label} "), style)];
                if i + 1 < self.options.len() {
                    spans.push(Span::raw("  "));
                }
                spans
            })
            .collect();

        let buttons = Paragraph::new(Line::from(button_spans)).alignment(Alignment::Center);
        Widget::render(buttons, chunks[4], buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dialog() -> PermissionDialog {
        PermissionDialog::new("bash", "rm -rf /tmp/cache", RiskLevel::High, "req_1")
    }

    #[test]
    fn new_dialog_defaults() {
        let d = dialog();
        assert_eq!(d.tool_name, "bash");
        assert_eq!(d.risk, RiskLevel::High);
        assert_eq!(d.selected(), 0);
        assert_eq!(d.request_id, "req_1");
    }

    #[test]
    fn navigate_left_right() {
        let mut d = dialog();
        assert_eq!(d.selected(), 0);

        d.handle_key(KeyCode::Right);
        assert_eq!(d.selected(), 1);

        d.handle_key(KeyCode::Right);
        assert_eq!(d.selected(), 2);

        // Stops at end
        d.handle_key(KeyCode::Right);
        assert_eq!(d.selected(), 2);

        d.handle_key(KeyCode::Left);
        assert_eq!(d.selected(), 1);

        // Stops at start
        d.handle_key(KeyCode::Left);
        d.handle_key(KeyCode::Left);
        assert_eq!(d.selected(), 0);
    }

    #[test]
    fn enter_confirms_selection() {
        let mut d = dialog();
        assert_eq!(
            d.handle_key(KeyCode::Enter),
            Some(PermissionResponse::Allow)
        );

        d.handle_key(KeyCode::Right);
        assert_eq!(d.handle_key(KeyCode::Enter), Some(PermissionResponse::Deny));

        d.handle_key(KeyCode::Right);
        assert_eq!(
            d.handle_key(KeyCode::Enter),
            Some(PermissionResponse::AlwaysAllow)
        );
    }

    #[test]
    fn shortcut_keys() {
        let mut d = dialog();
        assert_eq!(
            d.handle_key(KeyCode::Char('y')),
            Some(PermissionResponse::Allow)
        );
        assert_eq!(
            d.handle_key(KeyCode::Char('n')),
            Some(PermissionResponse::Deny)
        );
        assert_eq!(
            d.handle_key(KeyCode::Char('a')),
            Some(PermissionResponse::AlwaysAllow)
        );
    }

    #[test]
    fn esc_denies() {
        let mut d = dialog();
        assert_eq!(d.handle_key(KeyCode::Esc), Some(PermissionResponse::Deny));
    }

    #[test]
    fn unknown_key_returns_none() {
        let mut d = dialog();
        assert_eq!(d.handle_key(KeyCode::F(1)), None);
        assert_eq!(d.handle_key(KeyCode::Tab), None);
    }

    #[test]
    fn vim_navigation() {
        let mut d = dialog();
        d.handle_key(KeyCode::Char('l'));
        assert_eq!(d.selected(), 1);
        d.handle_key(KeyCode::Char('h'));
        assert_eq!(d.selected(), 0);
    }

    #[test]
    fn risk_levels() {
        assert_eq!(RiskLevel::Low.label(), "low");
        assert_eq!(RiskLevel::Medium.label(), "medium");
        assert_eq!(RiskLevel::High.label(), "high");
        assert_eq!(RiskLevel::Low.color(), Color::Green);
        assert_eq!(RiskLevel::High.color(), Color::Red);
    }

    #[test]
    fn dialog_area_centered() {
        let terminal = Rect::new(0, 0, 80, 24);
        let area = PermissionDialog::dialog_area(terminal);
        assert!(area.x > 0);
        assert!(area.y > 0);
        assert!(area.width <= 60);
        assert!(area.height <= 10);
        // Centered
        assert_eq!(area.x, (80 - area.width) / 2);
        assert_eq!(area.y, (24 - area.height) / 2);
    }

    #[test]
    fn dialog_area_small_terminal() {
        let terminal = Rect::new(0, 0, 30, 8);
        let area = PermissionDialog::dialog_area(terminal);
        assert!(area.width <= 30);
        assert!(area.height <= 8);
    }

    #[test]
    fn renders_without_panic() {
        let d = dialog();
        let area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(area);
        Widget::render(&d, area, &mut buf);

        // Check dialog contains tool name
        let all_text: String = (0..area.height)
            .flat_map(|y| {
                let row: Vec<String> = (0..area.width)
                    .map(|x| buf.cell((x, y)).unwrap().symbol().to_string())
                    .collect();
                row
            })
            .collect();
        assert!(all_text.contains("bash"));
    }

    #[test]
    fn tiny_area_does_not_panic() {
        let d = dialog();
        let area = Rect::new(0, 0, 10, 3);
        let mut buf = Buffer::empty(area);
        Widget::render(&d, area, &mut buf);
        // Should not panic, just skip rendering
    }
}
