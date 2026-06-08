//! Insert history lines above the inline viewport using terminal scroll
//! regions.
//!
//! Standard mode: `DECSTBM` to limit the scroll region to the rows above
//! the viewport, then Reverse Index (`ESC M`) to slide existing scrollback
//! down and write new lines into the freed space without disturbing the
//! viewport. Fallback mode: emit newlines at the screen bottom (Zellij
//! and bare conhost ignore `DECSTBM`) and write lines at absolute
//! positions.

use std::fmt;
use std::io;
use std::io::Write;

use crossterm::Command;
use crossterm::cursor::MoveDown;
use crossterm::cursor::MoveTo;
use crossterm::cursor::MoveToColumn;
use crossterm::cursor::RestorePosition;
use crossterm::cursor::SavePosition;
use crossterm::queue;
use crossterm::style::Color as CColor;
use crossterm::style::Colors;
use crossterm::style::Print;
use crossterm::style::SetAttribute;
use crossterm::style::SetBackgroundColor;
use crossterm::style::SetColors;
use crossterm::style::SetForegroundColor;
use crossterm::terminal::Clear;
use crossterm::terminal::ClearType;
use ratatui::backend::Backend;
use ratatui::backend::IntoCrossterm;
use ratatui::layout::Size;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::text::Line;
use ratatui::text::Span;

pub use crate::terminal_detection::InsertHistoryMode;
use crate::wrapping::RtOptions;
use crate::wrapping::adaptive_wrap_line;
use crate::wrapping::line_contains_url_like;
use crate::wrapping::line_has_mixed_url_and_non_url_tokens;

/// Insert `lines` above the viewport using the standard DECSTBM strategy.
pub fn insert_history_lines<B>(
    terminal: &mut crate::custom_terminal::Terminal<B>,
    lines: &[Line<'static>],
) -> io::Result<()>
where
    B: Backend<Error = io::Error> + Write,
{
    insert_history_lines_with_mode(terminal, lines, InsertHistoryMode::Standard)
}

/// Insert `lines` above the viewport using the strategy selected by `mode`.
pub fn insert_history_lines_with_mode<B>(
    terminal: &mut crate::custom_terminal::Terminal<B>,
    lines: &[Line<'static>],
    mode: InsertHistoryMode,
) -> io::Result<()>
where
    B: Backend<Error = io::Error> + Write,
{
    let screen_size = terminal.backend().size().unwrap_or(Size::new(0, 0));

    let mut area = terminal.viewport_area;
    let mut should_update_area = false;
    let last_cursor_pos = terminal.last_known_cursor_pos;
    let writer = terminal.backend_mut();

    // Pre-wrap so we know how many rows the scrollback insert needs. URL-only
    // lines are not pre-wrapped (the terminal will hard-wrap them at the
    // viewport edge but keep the OSC 8 hyperlink intact). Mixed URL+prose lines
    // and pure prose lines flow through adaptive_wrap.
    let wrap_width = area.width.max(1) as usize;
    let mut wrapped: Vec<Line<'_>> = Vec::new();
    let mut wrapped_rows = 0usize;
    for line in lines {
        let line_wrapped =
            if line_contains_url_like(line) && !line_has_mixed_url_and_non_url_tokens(line) {
                vec![line.clone()]
            } else {
                adaptive_wrap_line(line, RtOptions::new(wrap_width))
            };
        wrapped_rows += line_wrapped
            .iter()
            .map(|w| w.width().max(1).div_ceil(wrap_width))
            .sum::<usize>();
        wrapped.extend(line_wrapped);
    }
    let wrapped_lines = u16::try_from(wrapped_rows).unwrap_or(u16::MAX);

    if matches!(mode, InsertHistoryMode::Fallback) {
        let space_below = screen_size.height.saturating_sub(area.bottom());
        let shift_down = wrapped_lines.min(space_below);
        let scroll_up_amount = wrapped_lines.saturating_sub(shift_down);

        if scroll_up_amount > 0 {
            queue!(writer, MoveTo(0, screen_size.height.saturating_sub(1)))?;
            for _ in 0..scroll_up_amount {
                queue!(writer, Print("\n"))?;
            }
        }

        if shift_down > 0 {
            area.y += shift_down;
            should_update_area = true;
        }

        let cursor_top = area.top().saturating_sub(scroll_up_amount + shift_down);
        queue!(writer, MoveTo(0, cursor_top))?;

        for (i, line) in wrapped.iter().enumerate() {
            if i > 0 {
                queue!(writer, Print("\r\n"))?;
            }
            write_history_line(writer, line, wrap_width)?;
        }
    } else {
        let cursor_top = if area.bottom() < screen_size.height {
            let scroll_amount = wrapped_lines.min(screen_size.height - area.bottom());

            let top_1based = area.top() + 1;
            queue!(writer, SetScrollRegion(top_1based..screen_size.height))?;
            queue!(writer, MoveTo(0, area.top()))?;
            for _ in 0..scroll_amount {
                queue!(writer, Print("\x1bM"))?;
            }
            queue!(writer, ResetScrollRegion)?;

            let cursor_top = area.top().saturating_sub(1);
            area.y += scroll_amount;
            should_update_area = true;
            cursor_top
        } else {
            area.top().saturating_sub(1)
        };

        queue!(writer, SetScrollRegion(1..area.top()))?;
        queue!(writer, MoveTo(0, cursor_top))?;

        for line in &wrapped {
            queue!(writer, Print("\r\n"))?;
            write_history_line(writer, line, wrap_width)?;
        }

        queue!(writer, ResetScrollRegion)?;
    }

    queue!(writer, MoveTo(last_cursor_pos.x, last_cursor_pos.y))?;

    let _ = writer;
    if should_update_area {
        terminal.set_viewport_area(area);
    }
    if wrapped_lines > 0 {
        terminal.note_history_rows_inserted(wrapped_lines);
    }

    Ok(())
}

fn write_history_line<W: Write>(
    writer: &mut W,
    line: &Line<'_>,
    wrap_width: usize,
) -> io::Result<()> {
    let physical_rows = u16::try_from(line.width().max(1).div_ceil(wrap_width)).unwrap_or(u16::MAX);
    if physical_rows > 1 {
        queue!(writer, SavePosition)?;
        for _ in 1..physical_rows {
            queue!(writer, MoveDown(1), MoveToColumn(0))?;
            queue!(writer, Clear(ClearType::UntilNewLine))?;
        }
        queue!(writer, RestorePosition)?;
    }
    queue!(
        writer,
        SetColors(Colors::new(
            line.style
                .fg
                .map_or(CColor::Reset, IntoCrossterm::into_crossterm),
            line.style
                .bg
                .map_or(CColor::Reset, IntoCrossterm::into_crossterm),
        ))
    )?;
    queue!(writer, Clear(ClearType::UntilNewLine))?;
    let merged_spans: Vec<Span<'_>> = line
        .spans
        .iter()
        .map(|s| Span {
            style: s.style.patch(line.style),
            content: s.content.clone(),
        })
        .collect();
    write_spans(writer, merged_spans.iter())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetScrollRegion(pub std::ops::Range<u16>);

impl Command for SetScrollRegion {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        write!(f, "\x1b[{};{}r", self.0.start, self.0.end)
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> io::Result<()> {
        panic!("SetScrollRegion has no WinAPI counterpart; use ANSI mode")
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResetScrollRegion;

impl Command for ResetScrollRegion {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        write!(f, "\x1b[r")
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> io::Result<()> {
        panic!("ResetScrollRegion has no WinAPI counterpart; use ANSI mode")
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        true
    }
}

struct ModifierDiff {
    pub from: Modifier,
    pub to: Modifier,
}

impl ModifierDiff {
    fn queue<W: Write>(self, w: &mut W) -> io::Result<()> {
        use crossterm::style::Attribute as CAttribute;
        let removed = self.from - self.to;
        if removed.contains(Modifier::REVERSED) {
            queue!(w, SetAttribute(CAttribute::NoReverse))?;
        }
        if removed.contains(Modifier::BOLD) {
            queue!(w, SetAttribute(CAttribute::NormalIntensity))?;
            if self.to.contains(Modifier::DIM) {
                queue!(w, SetAttribute(CAttribute::Dim))?;
            }
        }
        if removed.contains(Modifier::ITALIC) {
            queue!(w, SetAttribute(CAttribute::NoItalic))?;
        }
        if removed.contains(Modifier::UNDERLINED) {
            queue!(w, SetAttribute(CAttribute::NoUnderline))?;
        }
        if removed.contains(Modifier::DIM) {
            queue!(w, SetAttribute(CAttribute::NormalIntensity))?;
        }
        if removed.contains(Modifier::CROSSED_OUT) {
            queue!(w, SetAttribute(CAttribute::NotCrossedOut))?;
        }
        if removed.contains(Modifier::HIDDEN) {
            queue!(w, SetAttribute(CAttribute::NoHidden))?;
        }
        if removed.contains(Modifier::SLOW_BLINK) || removed.contains(Modifier::RAPID_BLINK) {
            queue!(w, SetAttribute(CAttribute::NoBlink))?;
        }

        let added = self.to - self.from;
        if added.contains(Modifier::REVERSED) {
            queue!(w, SetAttribute(CAttribute::Reverse))?;
        }
        if added.contains(Modifier::BOLD) {
            queue!(w, SetAttribute(CAttribute::Bold))?;
        }
        if added.contains(Modifier::ITALIC) {
            queue!(w, SetAttribute(CAttribute::Italic))?;
        }
        if added.contains(Modifier::UNDERLINED) {
            queue!(w, SetAttribute(CAttribute::Underlined))?;
        }
        if added.contains(Modifier::DIM) {
            queue!(w, SetAttribute(CAttribute::Dim))?;
        }
        if added.contains(Modifier::CROSSED_OUT) {
            queue!(w, SetAttribute(CAttribute::CrossedOut))?;
        }
        if added.contains(Modifier::HIDDEN) {
            queue!(w, SetAttribute(CAttribute::Hidden))?;
        }
        if added.contains(Modifier::SLOW_BLINK) {
            queue!(w, SetAttribute(CAttribute::SlowBlink))?;
        }
        if added.contains(Modifier::RAPID_BLINK) {
            queue!(w, SetAttribute(CAttribute::RapidBlink))?;
        }
        Ok(())
    }
}

#[allow(clippy::similar_names)]
fn write_spans<'a, I, W: Write>(writer: &mut W, content: I) -> io::Result<()>
where
    I: IntoIterator<Item = &'a Span<'a>>,
{
    let mut fg = Color::Reset;
    let mut bg = Color::Reset;
    let mut last_modifier = Modifier::empty();
    for span in content {
        let mut modifier = Modifier::empty();
        modifier.insert(span.style.add_modifier);
        modifier.remove(span.style.sub_modifier);
        if modifier != last_modifier {
            let diff = ModifierDiff {
                from: last_modifier,
                to: modifier,
            };
            diff.queue(writer)?;
            last_modifier = modifier;
        }
        let next_fg = span.style.fg.unwrap_or(Color::Reset);
        let next_bg = span.style.bg.unwrap_or(Color::Reset);
        if next_fg != fg || next_bg != bg {
            queue!(
                writer,
                SetColors(Colors::new(
                    next_fg.into_crossterm(),
                    next_bg.into_crossterm()
                ))
            )?;
            fg = next_fg;
            bg = next_bg;
        }
        queue!(writer, Print(span.content.clone()))?;
    }

    queue!(
        writer,
        SetForegroundColor(CColor::Reset),
        SetBackgroundColor(CColor::Reset),
        SetAttribute(crossterm::style::Attribute::Reset),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custom_terminal::Terminal;
    use crate::test_backend::VT100Backend;
    use ratatui::layout::Rect;
    use ratatui::style::Color;
    use ratatui::style::Style;

    #[test]
    fn standard_mode_writes_above_viewport() {
        let width: u16 = 32;
        let height: u16 = 8;
        let mut term = Terminal::with_options(VT100Backend::new(width, height)).unwrap();
        term.set_viewport_area(Rect::new(0, height - 1, width, 1));
        insert_history_lines(&mut term, &[Line::from("hello world")]).unwrap();
        let rows: Vec<String> = term.backend().vt100().screen().rows(0, width).collect();
        assert!(rows.iter().any(|r| r.contains("hello world")));
    }

    #[test]
    fn fallback_mode_writes_above_viewport_and_shifts_y() {
        let width: u16 = 32;
        let height: u16 = 8;
        let mut term = Terminal::with_options(VT100Backend::new(width, height)).unwrap();
        term.set_viewport_area(Rect::new(0, 4, width, 2));
        insert_history_lines_with_mode(
            &mut term,
            &[Line::from("fallback works")],
            InsertHistoryMode::Fallback,
        )
        .unwrap();
        let rows: Vec<String> = term.backend().vt100().screen().rows(0, width).collect();
        assert!(rows.iter().any(|r| r.contains("fallback works")));
        assert_eq!(term.viewport_area, Rect::new(0, 5, width, 2));
        assert_eq!(term.visible_history_rows(), 1);
    }

    #[test]
    fn blockquote_line_emits_colored_cells() {
        let width: u16 = 40;
        let height: u16 = 6;
        let mut term = Terminal::with_options(VT100Backend::new(width, height)).unwrap();
        term.set_viewport_area(Rect::new(0, height - 1, width, 1));

        let line = Line::from(vec!["> ".into(), "Hello world".into()]).style(Color::Green);
        insert_history_lines(&mut term, &[line]).unwrap();

        let mut saw_colored = false;
        'outer: for row in 0..height {
            for col in 0..width {
                if let Some(cell) = term.backend().vt100().screen().cell(row, col)
                    && cell.has_contents()
                    && cell.fgcolor() != vt100::Color::Default
                {
                    saw_colored = true;
                    break 'outer;
                }
            }
        }
        assert!(saw_colored);
    }

    #[test]
    fn url_line_remains_intact_at_narrow_width() {
        let width: u16 = 20;
        let height: u16 = 8;
        let mut term = Terminal::with_options(VT100Backend::new(width, height)).unwrap();
        term.set_viewport_area(Rect::new(0, height - 1, width, 1));
        let url = "http://a-long-url.com/this/that/blah/many_segments";
        insert_history_lines(&mut term, &[Line::from(vec!["  │ ".into(), url.into()])]).unwrap();
        let rows: Vec<String> = term.backend().vt100().screen().rows(0, width).collect();
        assert!(rows.iter().any(|r| r.contains("│ http://a-long")));
    }

    #[test]
    fn colored_prefix_resets_on_plain_suffix() {
        let width: u16 = 40;
        let height: u16 = 6;
        let mut term = Terminal::with_options(VT100Backend::new(width, height)).unwrap();
        term.set_viewport_area(Rect::new(0, height - 1, width, 1));

        let line = Line::from(vec![
            Span::styled("1. ", Style::default().fg(Color::LightBlue)),
            Span::raw("Hello world"),
        ]);
        insert_history_lines(&mut term, &[line]).unwrap();

        let screen = term.backend().vt100().screen();
        for row in 0..height {
            let row_text: String = (0..width)
                .filter_map(|c| screen.cell(row, c).map(|cell| cell.contents().to_string()))
                .collect();
            if row_text.contains("Hello world") {
                let prefix_cell = screen.cell(row, 0).unwrap();
                assert_ne!(prefix_cell.fgcolor(), vt100::Color::Default);
                let plain_cell = screen.cell(row, 3).unwrap();
                assert_eq!(plain_cell.fgcolor(), vt100::Color::Default);
                return;
            }
        }
        panic!("expected to find Hello world row");
    }
}
