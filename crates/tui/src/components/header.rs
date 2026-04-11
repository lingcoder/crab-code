//! Header bar component — crab art + model/path info + separator.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::traits::Renderable;

/// Terra cotta color (`#DA7756`, same as CC's `clawd_body`).
const CRAB_COLOR: Color = Color::Rgb(218, 119, 86);

/// Background color for the crab art body.
const CRAB_BG: Color = Color::Black;

/// Header bar: crab ASCII art (left) + info text (right) + separator.
///
/// Layout (4 lines):
/// ```text
///  /| o o |\  Crab Code v0.1.0
///  \_^^^^^_/  claude-sonnet-4-6
///   // ||| \\ C:\path\to\project
/// ────────────────────────────────
/// ```
pub struct HeaderBar<'a> {
    pub model_name: &'a str,
    pub working_dir: &'a str,
}

impl Renderable for HeaderBar<'_> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        render_header(self.model_name, self.working_dir, area, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        4 // 3 art lines + 1 separator
    }
}

/// Render the header: crab art (left) + info text (right) + separator.
#[allow(clippy::cast_possible_truncation)]
fn render_header(model_name: &str, working_dir: &str, area: Rect, buf: &mut Buffer) {
    if area.height == 0 || area.width < 10 {
        return;
    }

    let fg = Style::default().fg(CRAB_COLOR);
    let fg_bg = Style::default().fg(CRAB_COLOR).bg(CRAB_BG);

    let art_lines: [Line<'_>; 3] = [
        Line::from(Span::styled(r" /| o o |\  ", fg)),
        Line::from(vec![
            Span::styled(r" \_", fg),
            Span::styled("^^^^^", fg_bg),
            Span::styled(r"_/  ", fg),
        ]),
        Line::from(Span::styled(r"  // ||| \\  ", fg)),
    ];

    let art_width = 13u16;

    let text_budget = area.width.saturating_sub(art_width) as usize;
    let info_lines: [Line<'_>; 3] = [
        Line::from(vec![
            Span::styled(
                "Crab Code",
                Style::default().fg(CRAB_COLOR).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" v0.1.0", Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(Span::styled(
            model_name,
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            shorten_path(working_dir, text_budget),
            Style::default().fg(Color::DarkGray),
        )),
    ];

    for (i, (art_line, info_line)) in art_lines.iter().zip(info_lines.iter()).enumerate() {
        let y = area.y + i as u16;
        if y >= area.y + area.height {
            break;
        }

        let art_area = Rect {
            x: area.x,
            y,
            width: art_width.min(area.width),
            height: 1,
        };
        Widget::render(art_line.clone(), art_area, buf);

        if area.width > art_width {
            let info_area = Rect {
                x: area.x + art_width,
                y,
                width: area.width.saturating_sub(art_width),
                height: 1,
            };
            Widget::render(info_line.clone(), info_area, buf);
        }
    }

    // Row 4: thin separator
    if area.height >= 4 {
        render_separator(
            Rect {
                x: area.x,
                y: area.y + 3,
                width: area.width,
                height: 1,
            },
            buf,
        );
    }
}

/// Shorten a path to fit within `max_chars`.
fn shorten_path(path: &str, max_chars: usize) -> String {
    if path.len() <= max_chars || max_chars < 6 {
        return path.to_string();
    }
    let suffix_budget = max_chars.saturating_sub(4);
    if let Some(pos) = path[path.len().saturating_sub(suffix_budget)..].find(['/', '\\']) {
        format!(
            "...{}",
            &path[path.len().saturating_sub(suffix_budget) + pos..]
        )
    } else {
        format!("...{}", &path[path.len().saturating_sub(suffix_budget)..])
    }
}

/// Render a thin horizontal separator line.
#[allow(clippy::cast_possible_truncation)]
pub fn render_separator(area: Rect, buf: &mut Buffer) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let sep = "─".repeat(area.width as usize);
    Widget::render(
        Line::from(Span::styled(&*sep, Style::default().fg(Color::DarkGray))),
        area,
        buf,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_desired_height() {
        let h = HeaderBar {
            model_name: "test",
            working_dir: "/tmp",
        };
        assert_eq!(h.desired_height(80), 4);
    }

    #[test]
    fn header_render_does_not_panic() {
        let h = HeaderBar {
            model_name: "claude-sonnet-4-6",
            working_dir: "/home/user/project",
        };
        let area = Rect::new(0, 0, 80, 4);
        let mut buf = Buffer::empty(area);
        h.render(area, &mut buf);
    }

    #[test]
    fn shorten_path_basic() {
        assert_eq!(shorten_path("short", 20), "short");
        let long = "/very/long/path/to/some/deeply/nested/directory";
        let shortened = shorten_path(long, 20);
        assert!(shortened.len() <= 20 + 3); // ...prefix
        assert!(shortened.starts_with("..."));
    }
}
