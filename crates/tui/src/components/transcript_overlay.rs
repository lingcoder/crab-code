//! Full-screen transcript overlay — browse conversation with vim keys.
//!
//! Activated by Ctrl+O. Renders the full transcript in the alternate
//! screen with j/k/g/G navigation. Line content is produced by asking
//! each [`crate::history::HistoryCell`] for its `transcript_lines`, so
//! the overlay never duplicates chat rendering logic.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::app::ChatMessage;
use crate::history::cell_from_chat_message;
use crate::keybindings::KeyContext;
use crate::overlay::{Overlay, OverlayAction};
use crate::traits::Renderable;

/// Full-screen transcript overlay for browsing conversation history.
pub struct TranscriptOverlay {
    /// Pre-rendered lines at construction width. Recomputed on creation
    /// whenever the overlay opens; width is assumed ≈ 80 which is good
    /// enough for the transcript view (overlays re-open per invocation).
    lines: Vec<Line<'static>>,
    /// Scroll offset from top (in lines).
    scroll_top: usize,
    /// Last known viewport height — used by `G` to jump to bottom.
    last_visible_height: std::cell::Cell<usize>,
}

impl TranscriptOverlay {
    /// Create a new transcript overlay from the current messages.
    #[must_use]
    pub fn new(messages: &[ChatMessage]) -> Self {
        // 120 cols is wide enough that most cells won't pre-wrap; the
        // overlay renderer does its own horizontal truncation below.
        let mut lines: Vec<Line<'static>> = Vec::new();
        for msg in messages {
            let cell = cell_from_chat_message(msg);
            lines.extend(cell.transcript_lines(120));
        }
        Self {
            lines,
            scroll_top: 0,
            last_visible_height: std::cell::Cell::new(20),
        }
    }

    /// Total rendered line count.
    fn total_lines(&self) -> usize {
        self.lines.len()
    }

    /// Clamp the scroll offset to keep at least one line of content visible.
    fn clamp_scroll(&mut self) {
        let visible = self.last_visible_height.get().max(1);
        let max_scroll = self.total_lines().saturating_sub(visible);
        if self.scroll_top > max_scroll {
            self.scroll_top = max_scroll;
        }
    }
}

impl Renderable for TranscriptOverlay {
    #[allow(clippy::cast_possible_truncation)]
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 {
            return;
        }

        // Title bar
        let title = Line::from(vec![
            Span::styled(
                " Transcript ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " (j/k/g/G to scroll, q/Esc to close)",
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        Widget::render(
            title,
            Rect {
                x: area.x,
                y: area.y,
                width: area.width,
                height: 1,
            },
            buf,
        );

        // Separator
        let sep = "─".repeat(area.width as usize);
        Widget::render(
            Line::from(Span::styled(&*sep, Style::default().fg(Color::DarkGray))),
            Rect {
                x: area.x,
                y: area.y + 1,
                width: area.width,
                height: 1,
            },
            buf,
        );

        // Content area
        let content_area = Rect {
            x: area.x,
            y: area.y + 2,
            width: area.width,
            height: area.height.saturating_sub(2),
        };
        let visible = content_area.height as usize;
        self.last_visible_height.set(visible);

        for (i, line) in self
            .lines
            .iter()
            .skip(self.scroll_top)
            .take(visible)
            .enumerate()
        {
            let y = content_area.y + i as u16;
            Widget::render(
                line.clone(),
                Rect {
                    x: content_area.x,
                    y,
                    width: content_area.width,
                    height: 1,
                },
                buf,
            );
        }
    }

    fn desired_height(&self, _width: u16) -> u16 {
        0 // fullscreen
    }
}

impl Overlay for TranscriptOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayAction {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => OverlayAction::Dismiss,
            KeyCode::Char('j') | KeyCode::Down => {
                self.scroll_top = self.scroll_top.saturating_add(1);
                self.clamp_scroll();
                OverlayAction::Consumed
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.scroll_top = self.scroll_top.saturating_sub(1);
                OverlayAction::Consumed
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.scroll_top = self.scroll_top.saturating_add(20);
                self.clamp_scroll();
                OverlayAction::Consumed
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.scroll_top = self.scroll_top.saturating_sub(20);
                OverlayAction::Consumed
            }
            KeyCode::Char('g') => {
                self.scroll_top = 0;
                OverlayAction::Consumed
            }
            KeyCode::Char('G') => {
                // Scroll to bottom — leave last viewport of content visible.
                let visible = self.last_visible_height.get().max(1);
                self.scroll_top = self.total_lines().saturating_sub(visible);
                OverlayAction::Consumed
            }
            KeyCode::PageDown => {
                self.scroll_top = self
                    .scroll_top
                    .saturating_add(self.last_visible_height.get().max(1));
                self.clamp_scroll();
                OverlayAction::Consumed
            }
            KeyCode::PageUp => {
                self.scroll_top = self
                    .scroll_top
                    .saturating_sub(self.last_visible_height.get().max(1));
                OverlayAction::Consumed
            }
            _ => OverlayAction::Passthrough,
        }
    }

    fn contexts(&self) -> Vec<KeyContext> {
        vec![KeyContext::Transcript]
    }

    fn name(&self) -> &'static str {
        "transcript"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_overlay_empty() {
        let overlay = TranscriptOverlay::new(&[]);
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        overlay.render(area, &mut buf);
    }

    #[test]
    fn transcript_overlay_navigation() {
        let mut overlay = TranscriptOverlay::new(&[
            ChatMessage::User { text: "hi".into() },
            ChatMessage::Assistant {
                text: "hello".into(),
            },
        ]);

        let down = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        let result = overlay.handle_key(down);
        assert!(matches!(result, OverlayAction::Consumed));

        let up = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE);
        overlay.handle_key(up);
        assert_eq!(overlay.scroll_top, 0);

        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let result = overlay.handle_key(esc);
        assert!(matches!(result, OverlayAction::Dismiss));
    }

    #[test]
    fn transcript_overlay_go_to_bottom() {
        // Build a long transcript so G has somewhere to go.
        let messages: Vec<ChatMessage> = (0..50)
            .map(|i| ChatMessage::User {
                text: format!("msg {i}"),
            })
            .collect();
        let mut overlay = TranscriptOverlay::new(&messages);
        overlay.last_visible_height.set(10);

        let g_cap = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE);
        overlay.handle_key(g_cap);
        // UserCell emits text + blank = 2 lines per msg. 50 * 2 = 100.
        // With visible = 10, max scroll = 90.
        assert_eq!(overlay.scroll_top, 90);

        // `g` goes back to top
        let g_low = KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE);
        overlay.handle_key(g_low);
        assert_eq!(overlay.scroll_top, 0);
    }

    #[test]
    fn transcript_overlay_j_clamped() {
        let mut overlay = TranscriptOverlay::new(&[ChatMessage::User { text: "a".into() }]);
        overlay.last_visible_height.set(10);
        // Spam j — should clamp, not grow unbounded.
        for _ in 0..50 {
            overlay.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        }
        // total = 2 lines, visible = 10 → max_scroll = 0
        assert_eq!(overlay.scroll_top, 0);
    }

    #[test]
    fn transcript_lines_preserve_full_tool_output() {
        // ToolResultCell truncates display_lines to 10; transcript_lines
        // should show the full output instead. This is the headline
        // reason the transcript overlay exists.
        let body = (0..20)
            .map(|i| format!("line-{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let overlay = TranscriptOverlay::new(&[ChatMessage::ToolResult {
            tool_name: "bash".into(),
            output: body,
            is_error: false,
            display: None,
            collapsed: false,
        }]);
        // ToolResultCell::transcript_lines emits 20 body lines + 1 blank.
        assert_eq!(overlay.total_lines(), 21);
    }
}
