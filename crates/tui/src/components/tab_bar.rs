//! Horizontal tab bar component.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

/// A horizontal tab bar with keyboard navigation.
#[derive(Debug, Clone)]
pub struct TabBar {
    labels: Vec<String>,
    selected: usize,
}

impl TabBar {
    #[must_use]
    pub fn new(labels: Vec<impl Into<String>>) -> Self {
        Self {
            labels: labels.into_iter().map(Into::into).collect(),
            selected: 0,
        }
    }

    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn select(&mut self, idx: usize) {
        if idx < self.labels.len() {
            self.selected = idx;
        }
    }

    pub fn move_left(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn move_right(&mut self) {
        if self.selected + 1 < self.labels.len() {
            self.selected += 1;
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.labels.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.labels.is_empty()
    }
}

impl Widget for &TabBar {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 || self.labels.is_empty() {
            return;
        }

        let mut spans: Vec<Span<'static>> = Vec::new();
        for (i, label) in self.labels.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(
                    " \u{2502} ",
                    Style::default().fg(Color::DarkGray),
                ));
            }
            let style = if i == self.selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            spans.push(Span::styled(label.clone(), style));
        }

        let line = Line::from(spans);
        Widget::render(line, area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_selects_first() {
        let bar = TabBar::new(vec!["A", "B", "C"]);
        assert_eq!(bar.selected(), 0);
        assert_eq!(bar.len(), 3);
    }

    #[test]
    fn move_right_advances() {
        let mut bar = TabBar::new(vec!["A", "B"]);
        bar.move_right();
        assert_eq!(bar.selected(), 1);
    }

    #[test]
    fn move_right_clamps_at_end() {
        let mut bar = TabBar::new(vec!["A", "B"]);
        bar.move_right();
        bar.move_right();
        assert_eq!(bar.selected(), 1);
    }

    #[test]
    fn move_left_clamps_at_start() {
        let bar = TabBar::new(vec!["A", "B"]);
        let mut bar = bar;
        bar.move_left();
        assert_eq!(bar.selected(), 0);
    }

    #[test]
    fn select_out_of_range_noop() {
        let mut bar = TabBar::new(vec!["A", "B"]);
        bar.select(5);
        assert_eq!(bar.selected(), 0);
    }

    #[test]
    fn render_no_panic() {
        let bar = TabBar::new(vec!["Recent", "Saved"]);
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        Widget::render(&bar, area, &mut buf);
    }

    #[test]
    fn render_contains_labels() {
        let bar = TabBar::new(vec!["Recent", "Saved"]);
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        Widget::render(&bar, area, &mut buf);
        let content: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(content.contains("Recent"));
        assert!(content.contains("Saved"));
    }

    #[test]
    fn is_empty_works() {
        let bar = TabBar::new(Vec::<String>::new());
        assert!(bar.is_empty());
    }
}
