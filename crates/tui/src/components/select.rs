//! Generic selection list component with keyboard navigation.

use crossterm::event::KeyCode;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

/// A selectable list item.
#[derive(Debug, Clone)]
pub struct SelectItem {
    pub label: String,
    pub key_hint: Option<String>,
}

impl SelectItem {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            key_hint: None,
        }
    }

    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.key_hint = Some(hint.into());
        self
    }
}

/// Outcome of a key press on the select list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectAction {
    /// The user confirmed a selection (Enter).
    Selected(usize),
    /// The user cancelled (Esc).
    Cancelled,
    /// Key was consumed but no final action yet (navigation).
    Consumed,
    /// Key was not handled.
    Ignored,
}

/// Generic selection list with arrow-key navigation.
pub struct SelectList {
    items: Vec<SelectItem>,
    selected: usize,
}

impl SelectList {
    #[must_use]
    pub fn new(items: Vec<SelectItem>) -> Self {
        Self { items, selected: 0 }
    }

    /// Current selected index.
    #[must_use]
    pub const fn selected(&self) -> usize {
        self.selected
    }

    /// Number of items.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the list is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Handle a key event. Returns the resulting action.
    pub fn handle_key(&mut self, code: KeyCode) -> SelectAction {
        match code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
                SelectAction::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !self.items.is_empty() && self.selected < self.items.len() - 1 {
                    self.selected += 1;
                }
                SelectAction::Consumed
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                if self.items.is_empty() {
                    SelectAction::Ignored
                } else {
                    SelectAction::Selected(self.selected)
                }
            }
            KeyCode::Esc => SelectAction::Cancelled,
            _ => SelectAction::Ignored,
        }
    }
}

impl Widget for &SelectList {
    #[allow(clippy::cast_possible_truncation)]
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        for (i, item) in self.items.iter().enumerate() {
            if i >= area.height as usize {
                break;
            }
            let y = area.y + i as u16;
            let is_selected = i == self.selected;

            let prefix = if is_selected { "▸ " } else { "  " };
            let label_style = if is_selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            let mut spans = vec![
                Span::styled(prefix, label_style),
                Span::styled(&item.label, label_style),
            ];

            if let Some(hint) = &item.key_hint {
                spans.push(Span::styled(
                    format!("  ({hint})"),
                    Style::default().fg(Color::DarkGray),
                ));
            }

            let line = Line::from(spans);
            let line_area = Rect {
                x: area.x,
                y,
                width: area.width,
                height: 1,
            };
            Widget::render(line, line_area, buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<SelectItem> {
        vec![
            SelectItem::new("Alpha"),
            SelectItem::new("Beta"),
            SelectItem::new("Gamma"),
        ]
    }

    #[test]
    fn new_selects_first() {
        let list = SelectList::new(items());
        assert_eq!(list.selected(), 0);
        assert_eq!(list.len(), 3);
        assert!(!list.is_empty());
    }

    #[test]
    fn down_moves_selection() {
        let mut list = SelectList::new(items());
        assert_eq!(list.handle_key(KeyCode::Down), SelectAction::Consumed);
        assert_eq!(list.selected(), 1);
        list.handle_key(KeyCode::Down);
        assert_eq!(list.selected(), 2);
    }

    #[test]
    fn down_stops_at_end() {
        let mut list = SelectList::new(items());
        list.handle_key(KeyCode::Down);
        list.handle_key(KeyCode::Down);
        list.handle_key(KeyCode::Down);
        assert_eq!(list.selected(), 2);
    }

    #[test]
    fn up_moves_selection() {
        let mut list = SelectList::new(items());
        list.handle_key(KeyCode::Down);
        list.handle_key(KeyCode::Down);
        list.handle_key(KeyCode::Up);
        assert_eq!(list.selected(), 1);
    }

    #[test]
    fn up_stops_at_zero() {
        let mut list = SelectList::new(items());
        list.handle_key(KeyCode::Up);
        assert_eq!(list.selected(), 0);
    }

    #[test]
    fn enter_selects() {
        let mut list = SelectList::new(items());
        list.handle_key(KeyCode::Down);
        assert_eq!(list.handle_key(KeyCode::Enter), SelectAction::Selected(1));
    }

    #[test]
    fn esc_cancels() {
        let mut list = SelectList::new(items());
        assert_eq!(list.handle_key(KeyCode::Esc), SelectAction::Cancelled);
    }

    #[test]
    fn vim_keys_work() {
        let mut list = SelectList::new(items());
        list.handle_key(KeyCode::Char('j'));
        assert_eq!(list.selected(), 1);
        list.handle_key(KeyCode::Char('k'));
        assert_eq!(list.selected(), 0);
    }

    #[test]
    fn space_selects() {
        let mut list = SelectList::new(items());
        assert_eq!(
            list.handle_key(KeyCode::Char(' ')),
            SelectAction::Selected(0)
        );
    }

    #[test]
    fn empty_list() {
        let mut list = SelectList::new(vec![]);
        assert!(list.is_empty());
        assert_eq!(list.handle_key(KeyCode::Enter), SelectAction::Ignored);
    }

    #[test]
    fn renders_items() {
        let list = SelectList::new(items());
        let area = Rect::new(0, 0, 30, 5);
        let mut buf = Buffer::empty(area);
        Widget::render(&list, area, &mut buf);

        let row0: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(row0.contains("Alpha"));

        let row1: String = (0..area.width)
            .map(|x| buf.cell((x, 1)).unwrap().symbol().to_string())
            .collect();
        assert!(row1.contains("Beta"));
    }

    #[test]
    fn item_with_hint() {
        let item = SelectItem::new("Option A").with_hint("y");
        assert_eq!(item.label, "Option A");
        assert_eq!(item.key_hint.as_deref(), Some("y"));
    }
}
