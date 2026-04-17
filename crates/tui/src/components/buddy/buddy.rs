//! The [`Buddy`] companion state + its [`Widget`] impl.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use super::personality::Personality;
use super::sprite::{self, Sprite};

/// The buddy companion state.
///
/// Holds a generated sprite and its derived personality. Implements the
/// ratatui [`Widget`] trait (via `&Buddy`) so it can be rendered directly
/// in a TUI layout.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Buddy {
    /// The sprite appearance.
    sprite: Sprite,
    /// The buddy personality (derived from sprite).
    personality: Personality,
    /// The session identifier that seeded this buddy.
    session_id: String,
}

#[allow(dead_code)]
impl Buddy {
    /// Create a buddy from a session identifier.
    ///
    /// The sprite and personality are derived deterministically so the
    /// same session always produces the same buddy.
    pub fn from_session(session_id: &str) -> Self {
        let sprite = sprite::generate_sprite(session_id);
        let personality = Personality::from_sprite(&sprite);
        Self {
            sprite,
            personality,
            session_id: session_id.to_owned(),
        }
    }

    /// The buddy sprite.
    pub fn sprite(&self) -> &Sprite {
        &self.sprite
    }

    /// The buddy personality.
    pub fn personality(&self) -> Personality {
        self.personality
    }

    /// The session identifier.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Get a greeting message appropriate for this buddy.
    pub fn greeting(&self) -> &'static str {
        self.personality.greeting()
    }
}

impl Widget for &Buddy {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 8 || area.height == 0 {
            return;
        }

        let art_lines = sprite::render_ascii(&self.sprite);

        for (i, art) in art_lines.iter().enumerate() {
            let y = area.y + i as u16;
            if y >= area.y + area.height {
                break;
            }
            // Split on newlines for multi-line body strings
            for (j, sub_line) in art.split('\n').enumerate() {
                let row = y + j as u16;
                if row >= area.y + area.height {
                    break;
                }
                let line = Line::from(vec![Span::styled(
                    sub_line,
                    Style::default().fg(Color::Cyan),
                )]);
                let line_area = Rect {
                    x: area.x,
                    y: row,
                    width: area.width,
                    height: 1,
                };
                Widget::render(line, line_area, buf);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buddy_from_session_is_deterministic() {
        let a = Buddy::from_session("my-session-42");
        let b = Buddy::from_session("my-session-42");
        assert_eq!(a.sprite(), b.sprite());
        assert_eq!(a.personality(), b.personality());
    }

    #[test]
    fn buddy_greeting_not_empty() {
        let buddy = Buddy::from_session("test");
        assert!(!buddy.greeting().is_empty());
    }

    #[test]
    fn buddy_session_id_stored() {
        let buddy = Buddy::from_session("sess-xyz");
        assert_eq!(buddy.session_id(), "sess-xyz");
    }

    #[test]
    fn buddy_renders_without_panic() {
        let buddy = Buddy::from_session("render-test");
        let area = Rect::new(0, 0, 30, 5);
        let mut buf = Buffer::empty(area);
        Widget::render(&buddy, area, &mut buf);

        // At least some non-space content should be present
        let has_content = (0..area.width)
            .any(|x| (0..area.height).any(|y| buf.cell((x, y)).unwrap().symbol() != " "));
        assert!(has_content);
    }

    #[test]
    fn buddy_tiny_area_does_not_panic() {
        let buddy = Buddy::from_session("tiny");
        let area = Rect::new(0, 0, 3, 1);
        let mut buf = Buffer::empty(area);
        Widget::render(&buddy, area, &mut buf);
    }
}
