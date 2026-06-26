use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::traits::Renderable;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BannerLevel {
    Info,
    Warning,
    Error,
}

impl BannerLevel {
    #[must_use]
    fn style(&self) -> Style {
        match self {
            Self::Info => Style::default().fg(Color::White).bg(Color::DarkGray),
            Self::Warning => Style::default().fg(Color::Black).bg(Color::Yellow),
            Self::Error => Style::default()
                .fg(Color::White)
                .bg(Color::Red)
                .add_modifier(Modifier::BOLD),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Banner {
    pub message: String,
    pub level: BannerLevel,
    pub dismissible: bool,
}

impl Banner {
    #[must_use]
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            level: BannerLevel::Info,
            dismissible: true,
        }
    }

    #[must_use]
    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            level: BannerLevel::Warning,
            dismissible: true,
        }
    }

    #[must_use]
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            level: BannerLevel::Error,
            dismissible: true,
        }
    }

    #[must_use]
    pub fn persistent(mut self) -> Self {
        self.dismissible = false;
        self
    }
}

pub struct NotificationBanner {
    banners: Vec<Banner>,
}

impl NotificationBanner {
    #[must_use]
    pub fn new() -> Self {
        Self {
            banners: Vec::new(),
        }
    }

    pub fn push(&mut self, banner: Banner) {
        self.banners.push(banner);
    }

    pub fn dismiss_top(&mut self) {
        if let Some(pos) = self.banners.iter().position(|b| b.dismissible) {
            self.banners.remove(pos);
        }
    }

    pub fn dismiss_all(&mut self) {
        self.banners.retain(|b| !b.dismissible);
    }

    pub fn clear(&mut self) {
        self.banners.clear();
    }

    #[must_use]
    pub fn has_banners(&self) -> bool {
        !self.banners.is_empty()
    }

    #[must_use]
    pub fn banner_count(&self) -> usize {
        self.banners.len()
    }
}

impl Default for NotificationBanner {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderable for NotificationBanner {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if self.banners.is_empty() || area.height == 0 {
            return;
        }

        for (i, banner) in self.banners.iter().enumerate() {
            if i as u16 >= area.height {
                break;
            }
            let row = Rect {
                x: area.x,
                y: area.y + i as u16,
                width: area.width,
                height: 1,
            };
            let style = banner.level.style();
            let dismiss_hint = if banner.dismissible { " [x]" } else { "" };
            let text = format!(" {}{dismiss_hint}", banner.message);
            let truncated = if crab_utils::text::display_width(&text) > area.width as usize {
                format!(
                    "{}…",
                    crab_utils::text::truncate_to_width(
                        &text,
                        (area.width as usize).saturating_sub(1)
                    )
                )
            } else {
                text
            };
            let line = Line::from(vec![Span::styled(truncated, style)]);
            Widget::render(line, row, buf);
        }
    }

    fn desired_height(&self, _width: u16) -> u16 {
        self.banners.len() as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_count() {
        let mut banner = NotificationBanner::new();
        assert!(!banner.has_banners());
        banner.push(Banner::info("test"));
        assert!(banner.has_banners());
        assert_eq!(banner.banner_count(), 1);
    }

    #[test]
    fn dismiss_top() {
        let mut nb = NotificationBanner::new();
        nb.push(Banner::warning("Rate limited"));
        nb.push(Banner::error("Connection lost").persistent());
        nb.dismiss_top();
        assert_eq!(nb.banner_count(), 1);
        assert_eq!(nb.banners[0].message, "Connection lost");
    }

    #[test]
    fn dismiss_all_keeps_persistent() {
        let mut nb = NotificationBanner::new();
        nb.push(Banner::info("dismissible"));
        nb.push(Banner::error("persistent").persistent());
        nb.dismiss_all();
        assert_eq!(nb.banner_count(), 1);
        assert!(!nb.banners[0].dismissible);
    }

    #[test]
    fn desired_height_matches_count() {
        let mut nb = NotificationBanner::new();
        assert_eq!(nb.desired_height(80), 0);
        nb.push(Banner::info("a"));
        nb.push(Banner::warning("b"));
        assert_eq!(nb.desired_height(80), 2);
    }

    #[test]
    fn render_no_panic() {
        let mut nb = NotificationBanner::new();
        nb.push(Banner::error("MCP connection failed"));
        let area = Rect::new(0, 0, 80, 2);
        let mut buf = Buffer::empty(area);
        nb.render(area, &mut buf);
    }
}
