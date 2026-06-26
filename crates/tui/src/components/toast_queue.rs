use std::collections::VecDeque;
use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Widget};

use crate::traits::Renderable;

const MAX_VISIBLE: usize = 3;
const DEFAULT_TTL: Duration = Duration::from_secs(3);
const TOAST_WIDTH: u16 = 40;
const TOAST_HEIGHT: u16 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToastLevel {
    Success,
    Warning,
    Error,
    Info,
}

impl ToastLevel {
    #[must_use]
    pub fn icon(&self) -> &'static str {
        match self {
            Self::Success => "✓",
            Self::Warning => "⚠",
            Self::Error => "✗",
            Self::Info => "ℹ",
        }
    }

    #[must_use]
    pub fn color(&self) -> Color {
        match self {
            Self::Success => Color::Green,
            Self::Warning => Color::Yellow,
            Self::Error => Color::Red,
            Self::Info => Color::Cyan,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub message: String,
    pub level: ToastLevel,
    pub created_at: Instant,
    pub ttl: Duration,
}

impl Toast {
    #[must_use]
    pub fn new(message: impl Into<String>, level: ToastLevel) -> Self {
        Self {
            message: message.into(),
            level,
            created_at: Instant::now(),
            ttl: DEFAULT_TTL,
        }
    }

    #[must_use]
    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed() >= self.ttl
    }
}

pub struct ToastQueue {
    toasts: VecDeque<Toast>,
}

impl ToastQueue {
    #[must_use]
    pub fn new() -> Self {
        Self {
            toasts: VecDeque::new(),
        }
    }

    pub fn push(&mut self, toast: Toast) {
        self.toasts.push_back(toast);
    }

    pub fn push_success(&mut self, message: impl Into<String>) {
        self.push(Toast::new(message, ToastLevel::Success));
    }

    pub fn push_warning(&mut self, message: impl Into<String>) {
        self.push(Toast::new(message, ToastLevel::Warning));
    }

    pub fn push_error(&mut self, message: impl Into<String>) {
        self.push(Toast::new(message, ToastLevel::Error));
    }

    pub fn push_info(&mut self, message: impl Into<String>) {
        self.push(Toast::new(message, ToastLevel::Info));
    }

    pub fn tick(&mut self) {
        self.toasts.retain(|t| !t.is_expired());
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.toasts.is_empty()
    }

    #[must_use]
    pub fn visible_toasts(&self) -> Vec<&Toast> {
        self.toasts
            .iter()
            .filter(|t| !t.is_expired())
            .take(MAX_VISIBLE)
            .collect()
    }
}

impl Default for ToastQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderable for ToastQueue {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let visible = self.visible_toasts();
        if visible.is_empty() || area.width < TOAST_WIDTH || area.height < TOAST_HEIGHT {
            return;
        }

        let x = area.right().saturating_sub(TOAST_WIDTH + 1);

        for (i, toast) in visible.iter().enumerate() {
            let y = area.y + 1 + (i as u16 * TOAST_HEIGHT);
            if y + TOAST_HEIGHT > area.bottom() {
                break;
            }

            let toast_area = Rect {
                x,
                y,
                width: TOAST_WIDTH,
                height: TOAST_HEIGHT,
            };

            Widget::render(Clear, toast_area, buf);

            let border_color = toast.level.color();
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color));
            let inner = block.inner(toast_area);
            Widget::render(block, toast_area, buf);

            let icon_style = Style::default()
                .fg(toast.level.color())
                .add_modifier(Modifier::BOLD);
            let msg_style = Style::default().fg(Color::White);

            let truncated = if crab_utils::text::display_width(&toast.message)
                > (inner.width as usize).saturating_sub(4)
            {
                format!(
                    "{}…",
                    crab_utils::text::truncate_to_width(
                        &toast.message,
                        (inner.width as usize).saturating_sub(5)
                    )
                )
            } else {
                toast.message.clone()
            };

            let line = Line::from(vec![
                Span::styled(format!(" {} ", toast.level.icon()), icon_style),
                Span::styled(truncated, msg_style),
            ]);
            Widget::render(line, inner, buf);
        }
    }

    fn desired_height(&self, _width: u16) -> u16 {
        let count = self.visible_toasts().len() as u16;
        if count == 0 {
            0
        } else {
            count * TOAST_HEIGHT + 1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_visible() {
        let mut queue = ToastQueue::new();
        queue.push_success("Done!");
        queue.push_error("Failed");
        assert_eq!(queue.visible_toasts().len(), 2);
    }

    #[test]
    fn max_visible_capped() {
        let mut queue = ToastQueue::new();
        for i in 0..10 {
            queue.push_info(format!("Toast {i}"));
        }
        assert!(queue.visible_toasts().len() <= MAX_VISIBLE);
    }

    #[test]
    fn expired_removed_on_tick() {
        let mut queue = ToastQueue::new();
        queue.toasts.push_back(Toast {
            message: "old".into(),
            level: ToastLevel::Info,
            created_at: Instant::now().checked_sub(Duration::from_secs(60)).unwrap(),
            ttl: Duration::from_secs(1),
        });
        queue.push_info("fresh");
        queue.tick();
        assert_eq!(queue.toasts.len(), 1);
        assert_eq!(queue.toasts[0].message, "fresh");
    }

    #[test]
    fn icons() {
        assert_eq!(ToastLevel::Success.icon(), "✓");
        assert_eq!(ToastLevel::Warning.icon(), "⚠");
        assert_eq!(ToastLevel::Error.icon(), "✗");
        assert_eq!(ToastLevel::Info.icon(), "ℹ");
    }

    #[test]
    fn render_no_panic() {
        let mut queue = ToastQueue::new();
        queue.push_success("test toast");
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        queue.render(area, &mut buf);
    }

    #[test]
    fn empty_desired_height_zero() {
        let queue = ToastQueue::new();
        assert_eq!(queue.desired_height(80), 0);
    }
}
