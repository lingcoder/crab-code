//! Titled content block with a border.

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Widget};

use crate::theme::Theme;

/// A panel with a border and an optional left-aligned title.
///
/// The widget does not store content — call `render_content` inside the
/// region returned by [`Pane::inner_rect`] to paint the body.
pub struct Pane<'a> {
    title: Option<&'a str>,
    theme: &'a Theme,
    focused: bool,
    muted_title: bool,
}

impl<'a> Pane<'a> {
    #[must_use]
    pub fn new(theme: &'a Theme) -> Self {
        Self {
            title: None,
            theme,
            focused: false,
            muted_title: false,
        }
    }

    #[must_use]
    pub fn title(mut self, title: &'a str) -> Self {
        self.title = Some(title);
        self
    }

    #[must_use]
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    #[must_use]
    pub fn muted_title(mut self, muted: bool) -> Self {
        self.muted_title = muted;
        self
    }

    /// The inner rect after borders are drawn.
    #[must_use]
    pub fn inner_rect(rect: Rect) -> Rect {
        Block::default().borders(Borders::ALL).inner(rect)
    }

    fn border_style(&self) -> Style {
        if self.focused {
            Style::default()
                .fg(self.theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(self.theme.border)
        }
    }

    fn title_style(&self) -> Style {
        if self.muted_title {
            Style::default().fg(self.theme.muted)
        } else if self.focused {
            Style::default()
                .fg(self.theme.text_bright)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(self.theme.fg)
        }
    }
}

impl Widget for Pane<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.border_style());
        if let Some(title) = self.title {
            let span = Span::styled(format!(" {title} "), self.title_style());
            block = block
                .title(Line::from(span))
                .title_alignment(Alignment::Left);
        }
        block.render(area, buf);
    }
}
