//! Scrollable region with a right-side indicator bar.
//!
//! The widget is purely visual — it takes a `ScrollBoxState` describing
//! current position and total content height, renders a vertical scroll
//! indicator, and leaves the inner rectangle for the caller to paint.
//!
//! Use [`ScrollBox::inner_rect`] to obtain the body rect (one column
//! narrower on the right to reserve space for the indicator).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;

use crate::theme::Theme;

/// Tracks scroll offset and total content height. `offset` is in lines;
/// `content_len` is total lines available; `viewport` is visible rows.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScrollBoxState {
    pub offset: usize,
    pub content_len: usize,
    pub viewport: usize,
}

impl ScrollBoxState {
    #[must_use]
    pub fn new(offset: usize, content_len: usize, viewport: usize) -> Self {
        Self {
            offset,
            content_len,
            viewport,
        }
    }

    #[must_use]
    pub fn fraction(&self) -> (usize, usize) {
        (self.offset, self.content_len.max(1))
    }

    /// Whether the scrollbar should be drawn at all (content is taller
    /// than the viewport).
    #[must_use]
    pub fn is_scrollable(&self) -> bool {
        self.content_len > self.viewport
    }
}

pub struct ScrollBox<'a> {
    theme: &'a Theme,
    state: ScrollBoxState,
}

impl<'a> ScrollBox<'a> {
    #[must_use]
    pub fn new(theme: &'a Theme, state: ScrollBoxState) -> Self {
        Self { theme, state }
    }

    /// Inner body rect — one column narrower on the right when a
    /// scrollbar will be drawn.
    #[must_use]
    pub fn inner_rect(&self, area: Rect) -> Rect {
        if self.state.is_scrollable() && area.width >= 1 {
            Rect {
                x: area.x,
                y: area.y,
                width: area.width.saturating_sub(1),
                height: area.height,
            }
        } else {
            area
        }
    }
}

impl Widget for ScrollBox<'_> {
    #[allow(clippy::cast_sign_loss, clippy::cast_precision_loss)]
    fn render(self, area: Rect, buf: &mut Buffer) {
        if !self.state.is_scrollable() || area.width == 0 || area.height == 0 {
            return;
        }
        let bar_x = area.x + area.width.saturating_sub(1);
        let height = area.height as usize;
        if height == 0 {
            return;
        }
        let ratio = self.state.viewport as f32 / self.state.content_len.max(1) as f32;
        let thumb_height_f = (ratio * height as f32).clamp(1.0, height as f32);
        let thumb_height = thumb_height_f as usize;
        let travel = height.saturating_sub(thumb_height);
        let max_offset = self
            .state
            .content_len
            .saturating_sub(self.state.viewport)
            .max(1);
        let thumb_top = if travel == 0 {
            0
        } else {
            let offset_ratio = self.state.offset as f32 / max_offset as f32;
            let thumb_top_f = (offset_ratio * travel as f32)
                .round()
                .clamp(0.0, travel as f32);
            thumb_top_f as usize
        };

        let track_style = Style::default().fg(self.theme.border);
        let thumb_style = Style::default().fg(self.theme.accent);

        for row in 0..height {
            let y = area.y + row as u16;
            let in_thumb = row >= thumb_top && row < thumb_top + thumb_height;
            let glyph = if in_thumb { "█" } else { "│" };
            let style = if in_thumb { thumb_style } else { track_style };
            buf.set_string(bar_x, y, glyph, style);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_scrollable_when_content_fits() {
        let state = ScrollBoxState::new(0, 5, 20);
        assert!(!state.is_scrollable());
    }

    #[test]
    fn scrollable_when_content_exceeds() {
        let state = ScrollBoxState::new(0, 100, 20);
        assert!(state.is_scrollable());
    }

    #[test]
    fn inner_rect_reserves_column_when_scrollable() {
        let theme = Theme::dark();
        let state = ScrollBoxState::new(0, 100, 10);
        let sb = ScrollBox::new(&theme, state);
        let rect = Rect::new(0, 0, 20, 10);
        let inner = sb.inner_rect(rect);
        assert_eq!(inner.width, 19);
    }

    #[test]
    fn inner_rect_full_when_content_fits() {
        let theme = Theme::dark();
        let state = ScrollBoxState::new(0, 5, 10);
        let sb = ScrollBox::new(&theme, state);
        let rect = Rect::new(0, 0, 20, 10);
        let inner = sb.inner_rect(rect);
        assert_eq!(inner.width, 20);
    }
}
