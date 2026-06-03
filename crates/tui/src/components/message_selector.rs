//! Selection-mode overlay — pick a past message and copy its text (Alt+V).
//!
//! A minimal modal list over the conversation: arrow keys move the selection,
//! `y`/Enter copies the chosen message via `AppEvent::MessageCopy`, Esc closes.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Widget};

use crate::app::ChatMessage;
use crate::app_event::AppEvent;
use crate::keybindings::KeyContext;
use crate::overlay::{Overlay, OverlayAction};
use crate::traits::Renderable;

/// Modal list for selecting a conversation message and copying its text.
pub struct MessageSelectorOverlay {
    messages: Vec<ChatMessage>,
    selected: usize,
}

impl MessageSelectorOverlay {
    #[must_use]
    pub fn new(messages: Vec<ChatMessage>) -> Self {
        let selected = messages.len().saturating_sub(1);
        Self { messages, selected }
    }

    fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    fn move_down(&mut self) {
        if self.selected + 1 < self.messages.len() {
            self.selected += 1;
        }
    }
}

/// One-line `role: first-line` label for a message row.
fn message_label(msg: &ChatMessage) -> String {
    let (tag, body) = match msg {
        ChatMessage::User { text } => ("user", text.as_str()),
        ChatMessage::Assistant { text, .. } => ("assistant", text.as_str()),
        ChatMessage::System { text, .. } => ("system", text.as_str()),
        ChatMessage::ToolUse { name, .. } => ("tool", name.as_str()),
        ChatMessage::ToolResult { tool_name, .. } => ("result", tool_name.as_str()),
        ChatMessage::Thinking { text, .. } => ("thinking", text.as_str()),
        _ => ("·", ""),
    };
    let first = body.lines().next().unwrap_or("");
    format!("{tag}: {first}")
}

impl Renderable for MessageSelectorOverlay {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.width < 8 || area.height < 3 {
            return;
        }
        Widget::render(Clear, area, buf);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" Select message (\u{2191}/\u{2193}, y/Enter copy, Esc close) ");
        let inner = block.inner(area);
        Widget::render(block, area, buf);

        let visible = inner.height as usize;
        let scroll = if self.selected >= visible {
            self.selected - visible + 1
        } else {
            0
        };
        for (row, (i, msg)) in self
            .messages
            .iter()
            .enumerate()
            .skip(scroll)
            .take(visible)
            .enumerate()
        {
            let style = if i == self.selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            let line = Line::from(Span::styled(message_label(msg), style));
            let line_area = Rect {
                x: inner.x,
                y: inner.y + row as u16,
                width: inner.width,
                height: 1,
            };
            Widget::render(line, line_area, buf);
        }
    }

    fn desired_height(&self, _width: u16) -> u16 {
        0
    }
}

impl Overlay for MessageSelectorOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => OverlayAction::Dismiss,
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_up();
                OverlayAction::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_down();
                OverlayAction::Consumed
            }
            KeyCode::Char('y') | KeyCode::Enter => OverlayAction::Execute(AppEvent::MessageCopy {
                index: self.selected,
            }),
            _ => OverlayAction::Passthrough,
        }
    }

    fn contexts(&self) -> Vec<KeyContext> {
        vec![KeyContext::SelectionMode]
    }

    fn name(&self) -> &'static str {
        "message_selector"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msgs() -> Vec<ChatMessage> {
        vec![
            ChatMessage::User {
                text: "hello".into(),
            },
            ChatMessage::Assistant {
                text: "world".into(),
                committed_lines: 0,
                streaming: false,
            },
        ]
    }

    #[test]
    fn new_selects_last() {
        let overlay = MessageSelectorOverlay::new(msgs());
        assert_eq!(overlay.selected, 1);
    }

    #[test]
    fn esc_dismisses() {
        let mut overlay = MessageSelectorOverlay::new(msgs());
        assert!(matches!(
            overlay.handle_key(KeyEvent::from(KeyCode::Esc)),
            OverlayAction::Dismiss
        ));
    }

    #[test]
    fn y_emits_message_copy_of_selected() {
        let mut overlay = MessageSelectorOverlay::new(msgs());
        overlay.handle_key(KeyEvent::from(KeyCode::Up));
        assert!(matches!(
            overlay.handle_key(KeyEvent::from(KeyCode::Char('y'))),
            OverlayAction::Execute(AppEvent::MessageCopy { index: 0 })
        ));
    }

    #[test]
    fn navigation_clamps() {
        let mut overlay = MessageSelectorOverlay::new(msgs());
        overlay.handle_key(KeyEvent::from(KeyCode::Down));
        assert_eq!(overlay.selected, 1);
        overlay.handle_key(KeyEvent::from(KeyCode::Up));
        overlay.handle_key(KeyEvent::from(KeyCode::Up));
        assert_eq!(overlay.selected, 0);
    }

    #[test]
    fn render_no_panic() {
        let overlay = MessageSelectorOverlay::new(msgs());
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        overlay.render(area, &mut buf);
    }
}
