//! Input area component — wraps `InputBox` with prompt decoration.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::app::PromptInputMode;
use crate::components::input::InputBox;
use crate::theme::accents::CLAUDE_DARK as CRAB_COLOR;
use crate::traits::Renderable;

/// Bash-mode prompt accent — green, to distinguish a shell line from a normal
/// prompt.
const BASH_COLOR: ratatui::style::Color = ratatui::style::Color::Rgb(126, 191, 130);

/// Input area: `❯` prompt + input box, with optional mode indicator.
pub struct InputArea<'a> {
    pub input: &'a InputBox,
    pub mode: PromptInputMode,
}

impl Renderable for InputArea<'_> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        render_input_with_prompt(self.input, self.mode, area, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        #[allow(clippy::cast_possible_truncation)]
        let h = self.input.line_count() as u16;
        h.max(1)
    }

    fn cursor_position(&self, area: Rect) -> Option<(u16, u16)> {
        let (row, col) = self.input.cursor();
        // +2 for "❯ " prefix
        #[allow(clippy::cast_possible_truncation)]
        Some((area.x + 2 + col as u16, area.y + row as u16))
    }
}

/// Render input with `❯` prompt — no border box (matches CC's flat style).
/// In `Bash` mode the chevron becomes a `!` indicator in a distinct color so
/// the user sees the line will run as a shell command.
#[allow(clippy::cast_possible_truncation)]
fn render_input_with_prompt(input: &InputBox, mode: PromptInputMode, area: Rect, buf: &mut Buffer) {
    if area.height == 0 || area.width < 4 {
        Widget::render(input, area, buf);
        return;
    }

    let (glyph, color) = match mode {
        PromptInputMode::Bash => ("! ", BASH_COLOR),
        _ => ("\u{276f} ", CRAB_COLOR),
    };
    let prompt_span = Span::styled(
        glyph,
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    );
    let prompt_area = Rect {
        x: area.x,
        y: area.y,
        width: 2.min(area.width),
        height: 1,
    };
    Widget::render(Line::from(prompt_span), prompt_area, buf);

    let input_area = Rect {
        x: area.x + 2,
        y: area.y,
        width: area.width.saturating_sub(2),
        height: area.height,
    };
    Widget::render(input, input_area, buf);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_area_desired_height() {
        let input = InputBox::new();
        let ia = InputArea {
            input: &input,
            mode: PromptInputMode::Prompt,
        };
        assert_eq!(ia.desired_height(80), 1);
    }

    #[test]
    fn input_area_render_does_not_panic() {
        let input = InputBox::new();
        let ia = InputArea {
            input: &input,
            mode: PromptInputMode::Prompt,
        };
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        ia.render(area, &mut buf);
    }

    #[test]
    fn bash_mode_renders_bang_glyph() {
        let mut input = InputBox::new();
        input.insert_text("ls -la");
        let ia = InputArea {
            input: &input,
            mode: PromptInputMode::Bash,
        };
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        ia.render(area, &mut buf);
        assert_eq!(buf[(0, 0)].symbol(), "!");
    }

    #[test]
    fn prompt_mode_renders_chevron_glyph() {
        let input = InputBox::new();
        let ia = InputArea {
            input: &input,
            mode: PromptInputMode::Prompt,
        };
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        ia.render(area, &mut buf);
        assert_eq!(buf[(0, 0)].symbol(), "\u{276f}");
    }
}
