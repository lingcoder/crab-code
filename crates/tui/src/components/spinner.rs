//! Spinner component — animated loading indicator with status message.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

/// Braille-based spinner frames for smooth animation.
const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Loading spinner with a status message.
pub struct Spinner {
    /// Current animation frame index (wraps around).
    frame: usize,
    /// Status message displayed next to the spinner.
    message: String,
    /// Whether the spinner is actively animating.
    active: bool,
}

impl Spinner {
    /// Create a new spinner (inactive by default).
    #[must_use]
    pub fn new() -> Self {
        Self {
            frame: 0,
            message: String::new(),
            active: false,
        }
    }

    /// Start the spinner with a status message.
    pub fn start(&mut self, message: impl Into<String>) {
        self.message = message.into();
        self.active = true;
        self.frame = 0;
    }

    /// Stop the spinner.
    pub fn stop(&mut self) {
        self.active = false;
    }

    /// Advance to the next animation frame. Call on each Tick event.
    pub fn tick(&mut self) {
        if self.active {
            self.frame = (self.frame + 1) % FRAMES.len();
        }
    }

    /// Whether the spinner is currently active.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.active
    }

    /// Current status message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Update the status message without restarting.
    pub fn set_message(&mut self, message: impl Into<String>) {
        self.message = message.into();
    }
}

impl Default for Spinner {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for &Spinner {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if !self.active || area.width < 3 || area.height == 0 {
            return;
        }

        let frame_char = FRAMES[self.frame];
        let line = Line::from(vec![
            Span::styled(frame_char, Style::default().fg(Color::Cyan)),
            Span::raw(" "),
            Span::styled(&self.message, Style::default().fg(Color::Gray)),
        ]);

        // Render into the first line of the area
        let line_area = Rect { height: 1, ..area };
        Widget::render(line, line_area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner_starts_inactive() {
        let spinner = Spinner::new();
        assert!(!spinner.is_active());
        assert!(spinner.message().is_empty());
    }

    #[test]
    fn spinner_start_and_stop() {
        let mut spinner = Spinner::new();
        spinner.start("Loading...");
        assert!(spinner.is_active());
        assert_eq!(spinner.message(), "Loading...");

        spinner.stop();
        assert!(!spinner.is_active());
    }

    #[test]
    fn spinner_tick_advances_frame() {
        let mut spinner = Spinner::new();
        spinner.start("Working");
        assert_eq!(spinner.frame, 0);

        spinner.tick();
        assert_eq!(spinner.frame, 1);

        spinner.tick();
        assert_eq!(spinner.frame, 2);
    }

    #[test]
    fn spinner_tick_wraps_around() {
        let mut spinner = Spinner::new();
        spinner.start("Working");

        for _ in 0..FRAMES.len() {
            spinner.tick();
        }
        assert_eq!(spinner.frame, 0);
    }

    #[test]
    fn spinner_tick_inactive_does_nothing() {
        let mut spinner = Spinner::new();
        spinner.tick();
        assert_eq!(spinner.frame, 0);
    }

    #[test]
    fn spinner_set_message() {
        let mut spinner = Spinner::new();
        spinner.start("First");
        spinner.set_message("Second");
        assert_eq!(spinner.message(), "Second");
        assert!(spinner.is_active());
    }

    #[test]
    fn spinner_default() {
        let spinner = Spinner::default();
        assert!(!spinner.is_active());
    }

    #[test]
    fn spinner_renders_when_active() {
        let mut spinner = Spinner::new();
        spinner.start("Testing");

        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        Widget::render(&spinner, area, &mut buf);

        let content: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(content.contains("Testing"));
    }

    #[test]
    fn spinner_does_not_render_when_inactive() {
        let spinner = Spinner::new();
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        Widget::render(&spinner, area, &mut buf);

        // Buffer should be all spaces
        let content: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert_eq!(content.trim(), "");
    }

    #[test]
    fn spinner_does_not_render_in_tiny_area() {
        let mut spinner = Spinner::new();
        spinner.start("Test");

        let area = Rect::new(0, 0, 2, 1); // too narrow
        let mut buf = Buffer::empty(area);
        Widget::render(&spinner, area, &mut buf);

        let content: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert_eq!(content.trim(), "");
    }
}
