//! Horizontal tab bar.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::theme::Theme;

/// Horizontal tab strip.
///
/// Selected tab is highlighted with `accent`; inactive tabs are rendered
/// with `muted`. Tabs are separated by a single middle-dot character.
pub struct Tabs<'a> {
    labels: &'a [&'a str],
    selected: usize,
    theme: &'a Theme,
}

impl<'a> Tabs<'a> {
    #[must_use]
    pub fn new(labels: &'a [&'a str], selected: usize, theme: &'a Theme) -> Self {
        Self {
            labels,
            selected,
            theme,
        }
    }
}

impl Widget for Tabs<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let mut spans = Vec::with_capacity(self.labels.len() * 2);
        for (i, label) in self.labels.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled("  ·  ", Style::default().fg(self.theme.muted)));
            }
            let style = if i == self.selected {
                Style::default()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::default().fg(self.theme.fg)
            };
            spans.push(Span::styled((*label).to_string(), style));
        }
        Paragraph::new(Line::from(spans)).render(area, buf);
    }
}
