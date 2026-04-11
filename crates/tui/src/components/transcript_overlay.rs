//! Full-screen transcript overlay — browse conversation with j/k navigation.
//!
//! Activated by Ctrl+O or Ctrl+T. Renders the full transcript in the
//! alternate screen with vim-style navigation.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::app::ChatMessage;
use crate::keybindings::KeyContext;
use crate::overlay::{Overlay, OverlayAction};
use crate::traits::Renderable;

/// Terra cotta color matching the crab logo.
const CRAB_COLOR: Color = Color::Rgb(218, 119, 86);

/// Full-screen transcript overlay for browsing conversation history.
pub struct TranscriptOverlay {
    /// Pre-rendered lines (computed once at construction time).
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
        let lines = render_messages_to_lines(messages);
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

/// Convert a message list into styled `Line`s for display.
fn render_messages_to_lines(messages: &[ChatMessage]) -> Vec<Line<'static>> {
    let mut all_lines: Vec<Line<'static>> = Vec::new();
    for msg in messages {
        match msg {
            ChatMessage::User { text } => {
                all_lines.push(Line::from(vec![
                    Span::styled(
                        "❯ ",
                        Style::default().fg(CRAB_COLOR).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(text.clone(), Style::default().fg(Color::White)),
                ]));
                all_lines.push(Line::default());
            }
            ChatMessage::Assistant { text } => {
                if text.is_empty() {
                    continue;
                }
                for (i, line) in text.lines().enumerate() {
                    if i == 0 {
                        all_lines.push(Line::from(vec![
                            Span::styled("● ", Style::default().fg(CRAB_COLOR)),
                            Span::styled(line.to_string(), Style::default().fg(Color::White)),
                        ]));
                    } else {
                        all_lines.push(Line::from(Span::styled(
                            line.to_string(),
                            Style::default().fg(Color::White),
                        )));
                    }
                }
                all_lines.push(Line::default());
            }
            ChatMessage::ToolUse { name, summary } => {
                let label = summary.as_deref().unwrap_or(name);
                all_lines.push(Line::from(Span::styled(
                    format!("● {label}"),
                    Style::default().fg(Color::DarkGray),
                )));
            }
            ChatMessage::ToolResult {
                output, is_error, ..
            } => {
                let style = if *is_error {
                    Style::default().fg(Color::Red)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                let lines: Vec<&str> = output.lines().collect();
                let show = lines.len().min(10);
                for line in &lines[..show] {
                    all_lines.push(Line::from(Span::styled(format!("  {line}"), style)));
                }
                if lines.len() > 10 {
                    all_lines.push(Line::from(Span::styled(
                        format!("  ... ({} more lines)", lines.len() - 10),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
                all_lines.push(Line::default());
            }
            ChatMessage::System { text } => {
                all_lines.push(Line::from(Span::styled(
                    text.clone(),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                )));
            }
        }
    }
    all_lines
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
        // total lines = 50 * 2 (user msg + blank) = 100, minus viewport 10 = 90
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
}
