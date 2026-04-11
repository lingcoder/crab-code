//! Bottom bar component — contextual shortcut hints.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::app::AppState;
use crate::traits::Renderable;

/// Bottom bar showing contextual key hints.
pub struct BottomBar {
    pub state: AppState,
    pub search_active: bool,
    pub permission_mode: crab_core::permission::PermissionMode,
}

impl Renderable for BottomBar {
    fn render(&self, area: Rect, buf: &mut Buffer) {
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

    #[test]
    fn bottom_bar_desired_height() {
        let bb = BottomBar {
            state: AppState::Idle,
            search_active: false,
            permission_mode: crab_core::permission::PermissionMode::Default,
        };
        assert_eq!(bb.desired_height(80), 1);
    }

    #[test]
    fn bottom_bar_render_does_not_panic() {
        let bb = BottomBar {
            state: AppState::Idle,
            search_active: false,
            permission_mode: crab_core::permission::PermissionMode::Default,
        };
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        bb.render(area, &mut buf);
    }
}
