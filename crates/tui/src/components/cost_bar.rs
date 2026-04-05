//! Token usage and cost status bar component.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

/// Bottom status bar showing token usage and cost.
pub struct CostBar {
    input_tokens: u64,
    output_tokens: u64,
    cache_read: u64,
    cache_write: u64,
    cost_usd: f64,
    api_calls: u64,
}

impl CostBar {
    #[must_use]
    pub fn new() -> Self {
        Self {
            input_tokens: 0,
            output_tokens: 0,
            cache_read: 0,
            cache_write: 0,
            cost_usd: 0.0,
            api_calls: 0,
        }
    }

    /// Update from a cost summary line's data.
    pub fn update(
        &mut self,
        input_tokens: u64,
        output_tokens: u64,
        cache_read: u64,
        cache_write: u64,
        cost_usd: f64,
        api_calls: u64,
    ) {
        self.input_tokens = input_tokens;
        self.output_tokens = output_tokens;
        self.cache_read = cache_read;
        self.cache_write = cache_write;
        self.cost_usd = cost_usd;
        self.api_calls = api_calls;
    }

    /// Total tokens (input + output).
    #[must_use]
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
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
}

impl Default for CostBar {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for &CostBar {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width < 20 {
            return;
        }

        let dim = Style::default().fg(Color::DarkGray);
        let value = Style::default().fg(Color::White);
        let cost_style = Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD);

        let line = Line::from(vec![
            Span::styled(" tokens: ", dim),
            Span::styled(CostBar::format_tokens(self.input_tokens), value),
            Span::styled("in/", dim),
            Span::styled(CostBar::format_tokens(self.output_tokens), value),
            Span::styled("out", dim),
            Span::styled(" │ ", dim),
            Span::styled("cache: ", dim),
            Span::styled(CostBar::format_tokens(self.cache_read), value),
            Span::styled("r/", dim),
            Span::styled(CostBar::format_tokens(self.cache_write), value),
            Span::styled("w", dim),
            Span::styled(" │ ", dim),
            Span::styled(format!("${:.4}", self.cost_usd), cost_style),
            Span::styled(" │ ", dim),
            Span::styled(format!("{} calls", self.api_calls), value),
        ]);

        let line_area = Rect { height: 1, ..area };
        Widget::render(line, line_area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_is_zero() {
        let bar = CostBar::new();
        assert_eq!(bar.total_tokens(), 0);
        assert_eq!(bar.api_calls, 0);
    }

    #[test]
    fn update_sets_values() {
        let mut bar = CostBar::new();
        bar.update(1000, 500, 200, 100, 0.015, 3);
        assert_eq!(bar.input_tokens, 1000);
        assert_eq!(bar.output_tokens, 500);
        assert_eq!(bar.total_tokens(), 1500);
        assert_eq!(bar.cache_read, 200);
        assert_eq!(bar.cache_write, 100);
        assert!((bar.cost_usd - 0.015).abs() < f64::EPSILON);
        assert_eq!(bar.api_calls, 3);
    }

    #[test]
    fn format_tokens_small() {
        assert_eq!(CostBar::format_tokens(0), "0");
        assert_eq!(CostBar::format_tokens(500), "500");
        assert_eq!(CostBar::format_tokens(9999), "9999");
    }

    #[test]
    fn format_tokens_large() {
        assert_eq!(CostBar::format_tokens(10_000), "10.0k");
        assert_eq!(CostBar::format_tokens(150_000), "150.0k");
    }

    #[test]
    fn default_is_zero() {
        let bar = CostBar::default();
        assert_eq!(bar.total_tokens(), 0);
    }

    #[test]
    fn renders_without_panic() {
        let mut bar = CostBar::new();
        bar.update(50_000, 10_000, 5000, 1000, 0.0832, 12);

        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        Widget::render(&bar, area, &mut buf);

        let content: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(content.contains("$0.0832"));
        assert!(content.contains("12 calls"));
    }

    #[test]
    fn renders_large_tokens_with_k() {
        let mut bar = CostBar::new();
        bar.update(100_000, 25_000, 0, 0, 0.0, 1);

        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        Widget::render(&bar, area, &mut buf);

        let content: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(content.contains("100.0k"));
        assert!(content.contains("25.0k"));
    }

    #[test]
    fn tiny_area_does_not_panic() {
        let bar = CostBar::new();
        let area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(area);
        Widget::render(&bar, area, &mut buf);
    }
}
