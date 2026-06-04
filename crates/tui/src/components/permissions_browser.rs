//! Permissions browser overlay — shows the current permission mode and
//! session-level "always allow" grants.
//!
//! Opened via `/permissions`. Read-only: mode cycling uses Shift+Tab in
//! the main chat view (`CyclePermissionMode`), and grants are populated as
//! users answer permission prompts with "Yes, and don't ask again".

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Widget};

use crab_core::permission::PermissionMode;

use crate::keybindings::KeyContext;
use crate::overlay::{Overlay, OverlayAction};
use crate::traits::Renderable;

pub struct PermissionsBrowserOverlay {
    mode: PermissionMode,
    grants: Vec<String>,
    scroll: usize,
}

impl PermissionsBrowserOverlay {
    #[must_use]
    pub fn new(mode: PermissionMode, mut grants: Vec<String>) -> Self {
        grants.sort();
        Self {
            mode,
            grants,
            scroll: 0,
        }
    }

    fn mode_description(&self) -> &'static str {
        // Keep these terse — the overlay is small.
        match self.mode {
            PermissionMode::Default => "Ask before every tool use (safest)",
            PermissionMode::AcceptEdits => "Auto-allow file edits; still ask for shell/network",
            PermissionMode::Plan => "Plan-only: block tool execution entirely",
            PermissionMode::Dangerously => "Allow everything (DANGEROUS — audit log only)",
            PermissionMode::DontAsk => "Never prompt; deny anything needing confirmation",
            PermissionMode::TrustProject => "Trust the project: allow in-project operations",
            PermissionMode::Auto => "Auto: classifier allows safe, prompts risky, blocks dangerous",
        }
    }

    fn lines(&self) -> Vec<Line<'_>> {
        let key_style = Style::default().fg(Color::Gray);
        let val_style = Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD);
        let dim_style = Style::default().fg(Color::DarkGray);

        let mut lines = vec![
            Line::from(Span::styled(
                " Permissions ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled("  Mode            ", key_style),
                Span::styled(self.mode.to_string(), val_style),
            ]),
            Line::from(vec![
                Span::styled("  ", dim_style),
                Span::styled(self.mode_description(), dim_style),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                format!("  Session grants ({})", self.grants.len()),
                key_style,
            )),
        ];

        if self.grants.is_empty() {
            lines.push(Line::from(Span::styled(
                "    (none — granted by answering \u{201C}Yes, and don't ask again\u{201D})",
                dim_style,
            )));
        } else {
            for name in &self.grants {
                lines.push(Line::from(vec![
                    Span::styled("    \u{2022} ", Style::default().fg(Color::Green)),
                    Span::styled(name, val_style),
                ]));
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Shift+Tab in chat to cycle mode. Esc/q to close.",
            dim_style,
        )));

        if self.scroll > 0 {
            lines.drain(0..self.scroll.min(lines.len()));
        }
        lines
    }
}

impl Renderable for PermissionsBrowserOverlay {
    #[allow(clippy::cast_possible_truncation)]
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height < 4 || area.width < 30 {
            return;
        }
        Widget::render(Clear, area, buf);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(
                " Permissions ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
        let inner = block.inner(area);
        Widget::render(block, area, buf);

        let lines = self.lines();
        for (i, line) in lines.iter().enumerate() {
            if i as u16 >= inner.height {
                break;
            }
            buf.set_line(inner.x, inner.y + i as u16, line, inner.width);
        }
    }

    fn desired_height(&self, _width: u16) -> u16 {
        self.lines().len().saturating_add(2).min(u16::MAX as usize) as u16
    }
}

impl Overlay for PermissionsBrowserOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => OverlayAction::Dismiss,
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll = self.scroll.saturating_add(1);
                OverlayAction::Consumed
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll = self.scroll.saturating_sub(1);
                OverlayAction::Consumed
            }
            _ => OverlayAction::Passthrough,
        }
    }

    fn contexts(&self) -> Vec<KeyContext> {
        vec![KeyContext::ScrollBox]
    }

    fn name(&self) -> &'static str {
        "permissions_browser"
    }
}
