use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Widget};

use crate::traits::Renderable;

pub struct StickyHeader {
    prompt_text: Option<String>,
    visible: bool,
}

impl StickyHeader {
    #[must_use]
    pub fn new() -> Self {
        Self {
            prompt_text: None,
            visible: false,
        }
    }

    pub fn update(&mut self, prompt_text: Option<String>, at_bottom: bool, overlay_open: bool) {
        self.visible = !at_bottom && !overlay_open && prompt_text.is_some();
        self.prompt_text = prompt_text;
    }

    #[must_use]
    pub fn is_visible(&self) -> bool {
        self.visible
    }
}

impl Default for StickyHeader {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderable for StickyHeader {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if !self.visible || area.height == 0 || area.width == 0 {
            return;
        }

        let text = match &self.prompt_text {
            Some(t) => t.as_str(),
            None => return,
        };

        let row = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 1,
        };

        Widget::render(Clear, row, buf);

        let bg = Color::Rgb(40, 40, 50);
        let style = Style::default()
            .fg(Color::White)
            .bg(bg)
            .add_modifier(Modifier::ITALIC);
        let prefix_style = Style::default()
            .fg(Color::Cyan)
            .bg(bg)
            .add_modifier(Modifier::BOLD);

        let max_text = (area.width as usize).saturating_sub(4);
        let truncated = if crab_utils::text::display_width(text) > max_text {
            format!(
                "{}…",
                crab_utils::text::truncate_to_width(text, max_text.saturating_sub(1))
            )
        } else {
            text.to_string()
        };

        let line = Line::from(vec![
            Span::styled(" ❯ ", prefix_style),
            Span::styled(truncated, style),
        ]);
        Widget::render(line, row, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        u16::from(self.visible)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_by_default() {
        let header = StickyHeader::new();
        assert!(!header.is_visible());
    }

    #[test]
    fn shows_when_scrolled_up() {
        let mut header = StickyHeader::new();
        header.update(Some("What is Rust?".into()), false, false);
        assert!(header.is_visible());
    }

    #[test]
    fn hidden_at_bottom() {
        let mut header = StickyHeader::new();
        header.update(Some("What is Rust?".into()), true, false);
        assert!(!header.is_visible());
    }

    #[test]
    fn hidden_with_overlay() {
        let mut header = StickyHeader::new();
        header.update(Some("What is Rust?".into()), false, true);
        assert!(!header.is_visible());
    }

    #[test]
    fn hidden_without_prompt() {
        let mut header = StickyHeader::new();
        header.update(None, false, false);
        assert!(!header.is_visible());
    }

    #[test]
    fn render_no_panic() {
        let mut header = StickyHeader::new();
        header.update(Some("Tell me about X".into()), false, false);
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        header.render(area, &mut buf);
    }
}
