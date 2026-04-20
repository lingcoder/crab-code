//! Modal dialog shell.
//!
//! A `Dialog` renders a titled, bordered modal centered in its parent
//! rect, reserves space at the bottom for a row of action buttons, and
//! exposes [`Dialog::body_rect`] for the caller to paint the dialog's
//! specific content.

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Widget};

use super::button::{Button, ButtonState};
use crate::theme::Theme;

/// One action on the dialog's footer.
#[derive(Debug, Clone)]
pub struct DialogAction {
    pub label: String,
    pub mnemonic: Option<char>,
    pub is_primary: bool,
    pub is_disabled: bool,
}

impl DialogAction {
    #[must_use]
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            mnemonic: None,
            is_primary: false,
            is_disabled: false,
        }
    }

    #[must_use]
    pub fn primary(mut self) -> Self {
        self.is_primary = true;
        self
    }

    #[must_use]
    pub fn mnemonic(mut self, key: char) -> Self {
        self.mnemonic = Some(key);
        self
    }

    #[must_use]
    pub fn disabled(mut self) -> Self {
        self.is_disabled = true;
        self
    }
}

pub struct Dialog<'a> {
    title: &'a str,
    theme: &'a Theme,
    accent: Option<ratatui::style::Color>,
    actions: &'a [DialogAction],
    focused_action: Option<usize>,
    preferred_width: u16,
    preferred_height: u16,
}

impl<'a> Dialog<'a> {
    #[must_use]
    pub fn new(title: &'a str, theme: &'a Theme) -> Self {
        Self {
            title,
            theme,
            accent: None,
            actions: &[],
            focused_action: None,
            preferred_width: 72,
            preferred_height: 16,
        }
    }

    #[must_use]
    pub fn accent(mut self, color: ratatui::style::Color) -> Self {
        self.accent = Some(color);
        self
    }

    #[must_use]
    pub fn actions(mut self, actions: &'a [DialogAction], focused: Option<usize>) -> Self {
        self.actions = actions;
        self.focused_action = focused;
        self
    }

    #[must_use]
    pub fn preferred_size(mut self, width: u16, height: u16) -> Self {
        self.preferred_width = width;
        self.preferred_height = height;
        self
    }

    /// Rect covering the dialog window itself, centered inside `outer`.
    #[must_use]
    pub fn window_rect(&self, outer: Rect) -> Rect {
        let w = self.preferred_width.min(outer.width.saturating_sub(2));
        let h = self.preferred_height.min(outer.height.saturating_sub(2));
        let x = outer.x + (outer.width.saturating_sub(w)) / 2;
        let y = outer.y + (outer.height.saturating_sub(h)) / 2;
        Rect {
            x,
            y,
            width: w,
            height: h,
        }
    }

    /// Rect reserved for the dialog body (excludes border + footer).
    #[must_use]
    pub fn body_rect(&self, outer: Rect) -> Rect {
        let window = self.window_rect(outer);
        let inner = Block::default().borders(Borders::ALL).inner(window);
        let footer_h = if self.actions.is_empty() { 0 } else { 3 };
        Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: inner.height.saturating_sub(footer_h),
        }
    }

    fn accent_color(&self) -> ratatui::style::Color {
        self.accent.unwrap_or(self.theme.accent)
    }

    fn render_footer(&self, rect: Rect, buf: &mut Buffer) {
        if self.actions.is_empty() || rect.width == 0 || rect.height == 0 {
            return;
        }
        let count = self.actions.len() as u16;
        let constraints: Vec<Constraint> = (0..count)
            .map(|_| Constraint::Ratio(1, count.into()))
            .collect();
        let slots = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(constraints)
            .split(rect);

        for (i, action) in self.actions.iter().enumerate() {
            let state = if action.is_disabled {
                ButtonState::Disabled
            } else if Some(i) == self.focused_action {
                ButtonState::Focused
            } else {
                ButtonState::Default
            };
            let mut btn = Button::new(&action.label, state, self.theme);
            if let Some(k) = action.mnemonic {
                btn = btn.mnemonic(k);
            }
            btn.render(slots[i], buf);
        }
    }
}

impl Widget for Dialog<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let window = self.window_rect(area);
        Clear.render(window, buf);

        let title_style = Style::default()
            .fg(self.accent_color())
            .add_modifier(Modifier::BOLD);
        let border_style = Style::default().fg(self.accent_color());
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(Line::from(Span::styled(
                format!(" {} ", self.title),
                title_style,
            )));
        let inner = block.inner(window);
        block.render(window, buf);

        if !self.actions.is_empty() && inner.height >= 3 {
            let footer_top = inner.y + inner.height.saturating_sub(3);
            let footer = Rect {
                x: inner.x,
                y: footer_top,
                width: inner.width,
                height: 3,
            };
            self.render_footer(footer, buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_rect_respects_outer_bounds() {
        let theme = Theme::dark();
        let dialog = Dialog::new("t", &theme).preferred_size(50, 12);
        let win = dialog.window_rect(Rect::new(0, 0, 30, 10));
        assert!(win.width <= 28);
        assert!(win.height <= 8);
    }

    #[test]
    fn body_rect_reserves_footer_when_actions_set() {
        let theme = Theme::dark();
        let actions = [DialogAction::new("OK")];
        let dialog = Dialog::new("t", &theme)
            .actions(&actions, Some(0))
            .preferred_size(40, 12);
        let outer = Rect::new(0, 0, 60, 20);
        let body = dialog.body_rect(outer);
        let window = dialog.window_rect(outer);
        assert!(body.height + 3 + 2 <= window.height);
    }

    #[test]
    fn action_builder_chain() {
        let action = DialogAction::new("Allow").primary().mnemonic('y');
        assert!(action.is_primary);
        assert_eq!(action.mnemonic, Some('y'));
    }
}
