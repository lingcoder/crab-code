//! Bottom bar component — contextual shortcut hints.
//!
//! When a chord prefix is in flight (e.g. `Ctrl+K` pressed, waiting for
//! the second key), the chord hint takes precedence over the normal
//! state-specific hint so the user sees what the resolver is waiting
//! for.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::app::AppState;
use crate::keybindings::KeyChord;
use crate::traits::Renderable;

/// Bottom bar showing contextual key hints.
pub struct BottomBar<'a> {
    pub state: AppState,
    pub search_active: bool,
    pub permission_mode: crab_core::permission::PermissionMode,
    /// In-flight chord prefix. When present, rendered as
    /// `"Ctrl+K …"` to tell the user another key is expected.
    pub chord_prefix: Option<&'a [KeyChord]>,
    /// Vim mode label (e.g. "NORMAL", "INSERT") when vim is active.
    pub vim_mode: Option<&'a str>,
    /// When true, show "Press Ctrl+C again to exit" instead of the
    /// normal state hint (first-press pending, clears after timeout).
    pub exit_pending: bool,
}

impl Renderable for BottomBar<'_> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if let Some(prefix) = self.chord_prefix {
            render_chord_hint(prefix, area, buf);
            return;
        }

        if self.exit_pending {
            let line = Line::from(Span::styled(
                "  Press Ctrl+C again to exit",
                Style::default().fg(Color::DarkGray),
            ));
            Widget::render(line, area, buf);
            return;
        }

        if let Some(vim_label) = self.vim_mode {
            let (label_style, rest_area) = render_vim_badge(vim_label, area, buf);
            let _ = label_style;
            if rest_area.width > 0 {
                render_bottom_bar(
                    self.state,
                    self.search_active,
                    self.permission_mode,
                    rest_area,
                    buf,
                );
            }
            return;
        }

        render_bottom_bar(
            self.state,
            self.search_active,
            self.permission_mode,
            area,
            buf,
        );
    }

    fn desired_height(&self, _width: u16) -> u16 {
        1
    }
}

fn render_vim_badge(label: &str, area: Rect, buf: &mut Buffer) -> (Style, Rect) {
    let badge = format!(" [{label}] ");
    let badge_width = badge.len() as u16;
    let style = Style::default()
        .fg(Color::Black)
        .bg(Color::Green)
        .add_modifier(Modifier::BOLD);
    let badge_area = Rect {
        x: area.x,
        y: area.y,
        width: badge_width.min(area.width),
        height: 1,
    };
    Widget::render(Span::styled(badge, style), badge_area, buf);
    let rest = Rect {
        x: area.x + badge_area.width,
        y: area.y,
        width: area.width.saturating_sub(badge_area.width),
        height: 1,
    };
    (style, rest)
}

fn render_chord_hint(prefix: &[KeyChord], area: Rect, buf: &mut Buffer) {
    let prefix_text = format_chord_prefix(prefix);
    let line = Line::from(vec![
        Span::styled(
            format!("  {prefix_text} "),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "… (waiting for next key)",
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    Widget::render(line, area, buf);
}

/// Render a chord sequence like `[Ctrl+K]` as the hint string `Ctrl+K`.
/// Multiple chords are separated by spaces: `Ctrl+K Ctrl+S`.
fn format_chord_prefix(prefix: &[KeyChord]) -> String {
    prefix
        .iter()
        .map(format_chord)
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_chord(chord: &KeyChord) -> String {
    use crossterm::event::{KeyCode, KeyModifiers};

    let mut parts: Vec<&str> = Vec::new();
    if chord.modifiers.contains(KeyModifiers::CONTROL) {
        parts.push("Ctrl");
    }
    if chord.modifiers.contains(KeyModifiers::ALT) {
        parts.push("Alt");
    }
    if chord.modifiers.contains(KeyModifiers::SHIFT) {
        parts.push("Shift");
    }
    let key = match chord.code {
        KeyCode::Char(' ') => "Space".to_string(),
        KeyCode::Char(c) => c.to_ascii_uppercase().to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::BackTab => "BackTab".to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Esc => "Esc".to_string(),
        KeyCode::Backspace => "BS".to_string(),
        KeyCode::Delete => "Del".to_string(),
        KeyCode::Up => "↑".to_string(),
        KeyCode::Down => "↓".to_string(),
        KeyCode::Left => "←".to_string(),
        KeyCode::Right => "→".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::PageUp => "PgUp".to_string(),
        KeyCode::PageDown => "PgDn".to_string(),
        KeyCode::F(n) => format!("F{n}"),
        other => format!("{other:?}"),
    };
    parts.push(key.as_str());
    parts.join("+")
}

fn render_bottom_bar(
    state: AppState,
    search_active: bool,
    perm_mode: crab_core::permission::PermissionMode,
    area: Rect,
    buf: &mut Buffer,
) {
    let line = if search_active {
        Line::from(Span::styled(
            "Enter: next match | Esc: close | type to search",
            Style::default().fg(Color::DarkGray),
        ))
    } else {
        match state {
            AppState::Confirming => Line::from(Span::styled(
                "y: allow | n: deny | a: always | Esc: deny",
                Style::default().fg(Color::DarkGray),
            )),
            AppState::Processing => Line::from(vec![
                Span::styled("  ▶▶ ", Style::default().fg(Color::DarkGray)),
                Span::styled(perm_mode.to_string(), Style::default().fg(Color::DarkGray)),
                Span::styled(
                    " (shift+tab to cycle) · esc to interrupt",
                    Style::default().fg(Color::DarkGray),
                ),
            ]),
            _ => {
                if perm_mode == crab_core::permission::PermissionMode::Default {
                    Line::from(Span::styled(
                        "  ? for shortcuts",
                        Style::default().fg(Color::DarkGray),
                    ))
                } else {
                    Line::from(vec![
                        Span::styled("  ▶▶ ", Style::default().fg(Color::DarkGray)),
                        Span::styled(perm_mode.to_string(), Style::default().fg(Color::DarkGray)),
                        Span::styled(
                            " (shift+tab to cycle)",
                            Style::default().fg(Color::DarkGray),
                        ),
                    ])
                }
            }
        }
    };
    Widget::render(line, area, buf);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};

    #[test]
    fn bottom_bar_desired_height() {
        let bb = BottomBar {
            state: AppState::Idle,
            search_active: false,
            permission_mode: crab_core::permission::PermissionMode::Default,
            chord_prefix: None,
            vim_mode: None,
            exit_pending: false,
        };
        assert_eq!(bb.desired_height(80), 1);
    }

    #[test]
    fn bottom_bar_render_does_not_panic() {
        let bb = BottomBar {
            state: AppState::Idle,
            search_active: false,
            permission_mode: crab_core::permission::PermissionMode::Default,
            chord_prefix: None,
            vim_mode: None,
            exit_pending: false,
        };
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        bb.render(area, &mut buf);
    }

    #[test]
    fn format_single_ctrl_chord() {
        let chord = KeyChord::new(KeyCode::Char('k'), KeyModifiers::CONTROL);
        assert_eq!(format_chord(&chord), "Ctrl+K");
    }

    #[test]
    fn format_multi_chord_prefix() {
        let prefix = [
            KeyChord::new(KeyCode::Char('k'), KeyModifiers::CONTROL),
            KeyChord::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
        ];
        assert_eq!(format_chord_prefix(&prefix), "Ctrl+K Ctrl+S");
    }

    #[test]
    fn format_alt_shift_chord() {
        let chord = KeyChord::new(KeyCode::Char('p'), KeyModifiers::ALT | KeyModifiers::SHIFT);
        assert_eq!(format_chord(&chord), "Alt+Shift+P");
    }

    #[test]
    fn format_named_key() {
        let chord = KeyChord::new(KeyCode::PageUp, KeyModifiers::NONE);
        assert_eq!(format_chord(&chord), "PgUp");
    }

    #[test]
    fn chord_hint_takes_precedence_over_state_hint() {
        let prefix = [KeyChord::new(KeyCode::Char('k'), KeyModifiers::CONTROL)];
        let bb = BottomBar {
            state: AppState::Processing, // would normally show the processing hint
            search_active: false,
            permission_mode: crab_core::permission::PermissionMode::Default,
            chord_prefix: Some(&prefix),
            vim_mode: None,
            exit_pending: false,
        };
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        bb.render(area, &mut buf);

        // Confirm the chord prefix text was written into the buffer.
        let rendered: String = (0..area.width)
            .map(|x| buf[(x, 0)].symbol())
            .collect::<Vec<_>>()
            .join("");
        assert!(rendered.contains("Ctrl+K"));
    }
}
