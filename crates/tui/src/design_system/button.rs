//! Focus-aware button.

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::theme::Theme;

/// Visible state of a button at render time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonState {
    Default,
    Focused,
    Disabled,
}

pub struct Button<'a> {
    label: &'a str,
    state: ButtonState,
    theme: &'a Theme,
    mnemonic: Option<char>,
}

impl<'a> Button<'a> {
    #[must_use]
    pub fn new(label: &'a str, state: ButtonState, theme: &'a Theme) -> Self {
        Self {
            label,
            state,
            theme,
            mnemonic: None,
        }
    }

    /// Set the single-letter mnemonic shown in `[x]` form to the left
    /// of the label.
    #[must_use]
    pub fn mnemonic(mut self, key: char) -> Self {
        self.mnemonic = Some(key);
        self
    }

    fn border_style(&self) -> Style {
        match self.state {
            ButtonState::Focused => Style::default()
                .fg(self.theme.accent)
                .add_modifier(Modifier::BOLD),
            ButtonState::Disabled => Style::default().fg(self.theme.muted),
            ButtonState::Default => Style::default().fg(self.theme.border),
        }
    }

    fn label_style(&self) -> Style {
        match self.state {
            ButtonState::Focused => Style::default()
                .fg(self.theme.text_bright)
                .add_modifier(Modifier::BOLD),
            ButtonState::Disabled => Style::default().fg(self.theme.muted),
            ButtonState::Default => Style::default().fg(self.theme.fg),
        }
    }
}

impl Widget for Button<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let label = Span::styled(self.label.to_string(), self.label_style());
        let line = if let Some(key) = self.mnemonic {
            Line::from(vec![
                Span::styled(
                    format!("[{key}] "),
                    Style::default()
                        .fg(self.theme.accent)
                        .add_modifier(Modifier::BOLD),
                ),
                label,
            ])
        } else {
            Line::from(label)
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.border_style());
        Paragraph::new(line)
            .alignment(Alignment::Center)
            .block(block)
            .render(area, buf);
    }
}
