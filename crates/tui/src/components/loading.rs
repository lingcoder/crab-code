//! Loading animations — overlay, thinking animation, and streaming indicator.
//!
//! Provides visual feedback during async operations: a semi-transparent
//! overlay with a centered message, a "thinking" dot animation, and a
//! real-time token counter for streaming responses.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::theme::Theme;

// ─── Loading overlay ────────────────────────────────────────────────────

/// A full-screen overlay with a centered loading message.
pub struct LoadingOverlay {
    /// Whether the overlay is visible.
    visible: bool,
    /// The loading message.
    message: String,
    /// Spinner frame index.
    frame: usize,
}

/// Braille spinner frames (same as the existing spinner component).
const SPINNER_FRAMES: &[&str] = &[
    "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}", "\u{2827}",
    "\u{2807}", "\u{280f}",
];

impl LoadingOverlay {
    /// Create a new hidden overlay.
    #[must_use]
    pub fn new() -> Self {
        Self {
            visible: false,
            message: String::new(),
            frame: 0,
        }
    }

    /// Show the overlay with a message.
    pub fn show(&mut self, message: impl Into<String>) {
        self.visible = true;
        self.message = message.into();
        self.frame = 0;
    }

    /// Hide the overlay.
    pub fn hide(&mut self) {
        self.visible = false;
    }

    /// Whether the overlay is visible.
    #[must_use]
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Get the current message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Set the message without changing visibility.
    pub fn set_message(&mut self, message: impl Into<String>) {
        self.message = message.into();
    }

    /// Advance the spinner animation.
    pub fn tick(&mut self) {
        if self.visible {
            self.frame = (self.frame + 1) % SPINNER_FRAMES.len();
        }
    }

    /// Current spinner frame character.
    #[must_use]
    pub fn spinner_char(&self) -> &str {
        SPINNER_FRAMES[self.frame]
    }
}

impl Default for LoadingOverlay {
    fn default() -> Self {
        Self::new()
    }
}

/// Widget for rendering the loading overlay.
pub struct LoadingOverlayWidget<'a> {
    overlay: &'a LoadingOverlay,
    theme: &'a Theme,
}

impl<'a> LoadingOverlayWidget<'a> {
    #[must_use]
    pub fn new(overlay: &'a LoadingOverlay, theme: &'a Theme) -> Self {
        Self { overlay, theme }
    }
}

impl Widget for LoadingOverlayWidget<'_> {
    #[allow(clippy::cast_possible_truncation)]
    fn render(self, area: Rect, buf: &mut Buffer) {
        if !self.overlay.is_visible() || area.height < 3 || area.width < 10 {
            return;
        }

        // Dim the background
        let dim_style = Style::default().fg(self.theme.muted).bg(self.theme.bg);
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_style(dim_style);
                }
            }
        }

        // Center the message box
        let spinner = self.overlay.spinner_char();
        let msg = self.overlay.message();
        let content = format!("{spinner} {msg}");
        let content_len = content.len() as u16;

        // Box dimensions
        let box_width = content_len + 4; // 2 padding + 2 border
        let box_height: u16 = 3;

        let box_x = area.x + (area.width.saturating_sub(box_width)) / 2;
        let box_y = area.y + (area.height.saturating_sub(box_height)) / 2;

        if box_x + box_width > area.x + area.width || box_y + box_height > area.y + area.height {
            // Fallback: just render text at center
            let center_y = area.y + area.height / 2;
            let line = Line::from(Span::styled(
                &content,
                Style::default()
                    .fg(self.theme.fg)
                    .add_modifier(Modifier::BOLD),
            ));
            Widget::render(line, Rect::new(area.x, center_y, area.width, 1), buf);
            return;
        }

        let border_style = Style::default().fg(self.theme.border);
        let content_style = Style::default()
            .fg(self.theme.fg)
            .add_modifier(Modifier::BOLD);

        // Top border
        render_box_line(
            buf,
            box_x,
            box_y,
            box_width,
            '\u{250c}',
            '\u{2500}',
            '\u{2510}',
            border_style,
        );

        // Content line
        if let Some(cell) = buf.cell_mut((box_x, box_y + 1)) {
            cell.set_char('\u{2502}');
            cell.set_style(border_style);
        }
        // Clear middle
        for x in box_x + 1..box_x + box_width - 1 {
            if let Some(cell) = buf.cell_mut((x, box_y + 1)) {
                cell.set_char(' ');
                cell.set_style(content_style);
            }
        }
        // Write content centered
        let pad_left = box_x + 1 + (box_width - 2).saturating_sub(content_len) / 2;
        let line = Line::from(Span::styled(&content, content_style));
        Widget::render(line, Rect::new(pad_left, box_y + 1, content_len, 1), buf);

        if let Some(cell) = buf.cell_mut((box_x + box_width - 1, box_y + 1)) {
            cell.set_char('\u{2502}');
            cell.set_style(border_style);
        }

        // Bottom border
        render_box_line(
            buf,
            box_x,
            box_y + 2,
            box_width,
            '\u{2514}',
            '\u{2500}',
            '\u{2518}',
            border_style,
        );
    }
}

/// Render a horizontal box border line.
#[allow(clippy::too_many_arguments)]
fn render_box_line(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    left: char,
    fill: char,
    right: char,
    style: Style,
) {
    if let Some(cell) = buf.cell_mut((x, y)) {
        cell.set_char(left);
        cell.set_style(style);
    }
    for xi in x + 1..x + width - 1 {
        if let Some(cell) = buf.cell_mut((xi, y)) {
            cell.set_char(fill);
            cell.set_style(style);
        }
    }
    if let Some(cell) = buf.cell_mut((x + width - 1, y)) {
        cell.set_char(right);
        cell.set_style(style);
    }
}

// ─── Thinking animation ─────────────────────────────────────────────────

/// "AI is thinking..." animation with cycling dots.
pub struct ThinkingAnimation {
    /// Whether the animation is active.
    active: bool,
    /// Base message (e.g. "AI is thinking").
    message: String,
    /// Current frame (controls dot count: 0=., 1=.., 2=..., 3=).
    frame: usize,
    /// Maximum number of dots.
    max_dots: usize,
}

impl ThinkingAnimation {
    /// Create a new thinking animation.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            active: false,
            message: message.into(),
            frame: 0,
            max_dots: 3,
        }
    }

    /// Start the animation.
    pub fn start(&mut self) {
        self.active = true;
        self.frame = 0;
    }

    /// Stop the animation.
    pub fn stop(&mut self) {
        self.active = false;
    }

    /// Whether the animation is active.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Advance to the next frame.
    pub fn tick(&mut self) {
        if self.active {
            self.frame = (self.frame + 1) % (self.max_dots + 1);
        }
    }

    /// Get the current display text.
    #[must_use]
    pub fn display_text(&self) -> String {
        if !self.active {
            return String::new();
        }
        let dots = ".".repeat(self.frame);
        let padding = " ".repeat(self.max_dots - self.frame);
        format!("{}{dots}{padding}", self.message)
    }

    /// Set the message.
    pub fn set_message(&mut self, message: impl Into<String>) {
        self.message = message.into();
    }

    /// Get the base message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl Default for ThinkingAnimation {
    fn default() -> Self {
        Self::new("AI is thinking")
    }
}

/// Widget for rendering the thinking animation.
pub struct ThinkingAnimationWidget<'a> {
    animation: &'a ThinkingAnimation,
    theme: &'a Theme,
}

impl<'a> ThinkingAnimationWidget<'a> {
    #[must_use]
    pub fn new(animation: &'a ThinkingAnimation, theme: &'a Theme) -> Self {
        Self { animation, theme }
    }
}

impl Widget for ThinkingAnimationWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if !self.animation.is_active() || area.height == 0 || area.width < 5 {
            return;
        }

        let text = self.animation.display_text();
        let style = Style::default()
            .fg(self.theme.muted)
            .add_modifier(Modifier::ITALIC);
        let line = Line::from(Span::styled(text, style));
        Widget::render(line, Rect::new(area.x, area.y, area.width, 1), buf);
    }
}

// ─── Streaming indicator ────────────────────────────────────────────────

/// Real-time token counter for streaming responses.
pub struct StreamingIndicator {
    /// Whether streaming is active.
    active: bool,
    /// Number of tokens received so far.
    token_count: usize,
    /// Tokens per second (computed externally).
    tokens_per_second: Option<f64>,
    /// Elapsed time in milliseconds.
    elapsed_ms: u64,
}

impl StreamingIndicator {
    /// Create a new streaming indicator.
    #[must_use]
    pub fn new() -> Self {
        Self {
            active: false,
            token_count: 0,
            tokens_per_second: None,
            elapsed_ms: 0,
        }
    }

    /// Start streaming.
    pub fn start(&mut self) {
        self.active = true;
        self.token_count = 0;
        self.tokens_per_second = None;
        self.elapsed_ms = 0;
    }

    /// Stop streaming.
    pub fn stop(&mut self) {
        self.active = false;
    }

    /// Whether streaming is active.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Add tokens to the count.
    pub fn add_tokens(&mut self, count: usize) {
        self.token_count += count;
    }

    /// Set the elapsed time and recompute tokens/second.
    pub fn set_elapsed(&mut self, ms: u64) {
        self.elapsed_ms = ms;
        if ms > 0 {
            #[allow(clippy::cast_precision_loss)]
            let tps = (self.token_count as f64) / (ms as f64 / 1000.0);
            self.tokens_per_second = Some(tps);
        }
    }

    /// Get the token count.
    #[must_use]
    pub fn token_count(&self) -> usize {
        self.token_count
    }

    /// Get tokens per second.
    #[must_use]
    pub fn tokens_per_second(&self) -> Option<f64> {
        self.tokens_per_second
    }

    /// Get elapsed time in milliseconds.
    #[must_use]
    pub fn elapsed_ms(&self) -> u64 {
        self.elapsed_ms
    }

    /// Format the token count for display.
    #[must_use]
    pub fn display_text(&self) -> String {
        if !self.active {
            return String::new();
        }
        let tps_suffix = self
            .tokens_per_second
            .map_or(String::new(), |tps| format!(" ({tps:.0} tok/s)"));
        format!("{} tokens{tps_suffix}", self.token_count)
    }
}

impl Default for StreamingIndicator {
    fn default() -> Self {
        Self::new()
    }
}

/// Widget for rendering the streaming indicator.
pub struct StreamingIndicatorWidget<'a> {
    indicator: &'a StreamingIndicator,
    theme: &'a Theme,
}

impl<'a> StreamingIndicatorWidget<'a> {
    #[must_use]
    pub fn new(indicator: &'a StreamingIndicator, theme: &'a Theme) -> Self {
        Self { indicator, theme }
    }
}

impl Widget for StreamingIndicatorWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if !self.indicator.is_active() || area.height == 0 || area.width < 5 {
            return;
        }

        let text = self.indicator.display_text();
        let style = Style::default().fg(self.theme.success);

        let dot = "\u{25cf} "; // ● streaming dot
        let line = Line::from(vec![
            Span::styled(
                dot,
                Style::default()
                    .fg(self.theme.success)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(text, style),
        ]);
        Widget::render(line, Rect::new(area.x, area.y, area.width, 1), buf);
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ─── LoadingOverlay tests ───

    #[test]
    fn loading_overlay_new() {
        let overlay = LoadingOverlay::new();
        assert!(!overlay.is_visible());
        assert!(overlay.message().is_empty());
    }

    #[test]
    fn loading_overlay_default() {
        let overlay = LoadingOverlay::default();
        assert!(!overlay.is_visible());
    }

    #[test]
    fn loading_overlay_show_hide() {
        let mut overlay = LoadingOverlay::new();
        overlay.show("Loading...");
        assert!(overlay.is_visible());
        assert_eq!(overlay.message(), "Loading...");

        overlay.hide();
        assert!(!overlay.is_visible());
    }

    #[test]
    fn loading_overlay_set_message() {
        let mut overlay = LoadingOverlay::new();
        overlay.show("First");
        overlay.set_message("Second");
        assert_eq!(overlay.message(), "Second");
    }

    #[test]
    fn loading_overlay_tick() {
        let mut overlay = LoadingOverlay::new();
        overlay.show("Loading");
        let first = overlay.spinner_char().to_string();
        overlay.tick();
        let second = overlay.spinner_char().to_string();
        assert_ne!(first, second);
    }

    #[test]
    fn loading_overlay_tick_wraps() {
        let mut overlay = LoadingOverlay::new();
        overlay.show("Test");
        for _ in 0..SPINNER_FRAMES.len() {
            overlay.tick();
        }
        // Should wrap back to 0
        assert_eq!(overlay.frame, 0);
    }

    #[test]
    fn loading_overlay_tick_inactive() {
        let mut overlay = LoadingOverlay::new();
        overlay.tick();
        assert_eq!(overlay.frame, 0);
    }

    #[test]
    fn loading_overlay_widget_hidden() {
        let overlay = LoadingOverlay::new();
        let theme = Theme::dark();
        let widget = LoadingOverlayWidget::new(&overlay, &theme);
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);
        // Should not render anything special
    }

    #[test]
    fn loading_overlay_widget_visible() {
        let mut overlay = LoadingOverlay::new();
        overlay.show("Please wait");
        let theme = Theme::dark();
        let widget = LoadingOverlayWidget::new(&overlay, &theme);
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);

        // Check that the message appears somewhere in the buffer
        let mut found = false;
        for y in 0..area.height {
            let row: String = (0..area.width)
                .map(|x| buf.cell((x, y)).unwrap().symbol().to_string())
                .collect();
            if row.contains("Please wait") {
                found = true;
                break;
            }
        }
        assert!(found, "Loading message should be visible");
    }

    #[test]
    fn loading_overlay_widget_small_area() {
        let mut overlay = LoadingOverlay::new();
        overlay.show("Test");
        let theme = Theme::dark();
        let widget = LoadingOverlayWidget::new(&overlay, &theme);
        let area = Rect::new(0, 0, 5, 1);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);
    }

    // ─── ThinkingAnimation tests ───

    #[test]
    fn thinking_new() {
        let t = ThinkingAnimation::new("Thinking");
        assert!(!t.is_active());
        assert_eq!(t.message(), "Thinking");
        assert!(t.display_text().is_empty());
    }

    #[test]
    fn thinking_default() {
        let t = ThinkingAnimation::default();
        assert_eq!(t.message(), "AI is thinking");
    }

    #[test]
    fn thinking_start_stop() {
        let mut t = ThinkingAnimation::new("Processing");
        t.start();
        assert!(t.is_active());
        assert!(!t.display_text().is_empty());

        t.stop();
        assert!(!t.is_active());
        assert!(t.display_text().is_empty());
    }

    #[test]
    fn thinking_tick_cycles_dots() {
        let mut t = ThinkingAnimation::new("Working");
        t.start();

        // frame 0: no dots
        assert_eq!(t.display_text(), "Working   ");

        t.tick(); // frame 1: .
        assert_eq!(t.display_text(), "Working.  ");

        t.tick(); // frame 2: ..
        assert_eq!(t.display_text(), "Working.. ");

        t.tick(); // frame 3: ...
        assert_eq!(t.display_text(), "Working...");

        t.tick(); // frame 0: wraps
        assert_eq!(t.display_text(), "Working   ");
    }

    #[test]
    fn thinking_tick_inactive() {
        let mut t = ThinkingAnimation::new("Test");
        t.tick();
        assert_eq!(t.frame, 0);
    }

    #[test]
    fn thinking_set_message() {
        let mut t = ThinkingAnimation::new("Old");
        t.set_message("New");
        assert_eq!(t.message(), "New");
    }

    #[test]
    fn thinking_widget_inactive() {
        let t = ThinkingAnimation::new("Test");
        let theme = Theme::dark();
        let widget = ThinkingAnimationWidget::new(&t, &theme);
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);

        let row: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert_eq!(row.trim(), "");
    }

    #[test]
    fn thinking_widget_active() {
        let mut t = ThinkingAnimation::new("Thinking");
        t.start();
        t.tick(); // frame 1
        let theme = Theme::dark();
        let widget = ThinkingAnimationWidget::new(&t, &theme);
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);

        let row: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(row.contains("Thinking"));
    }

    // ─── StreamingIndicator tests ───

    #[test]
    fn streaming_new() {
        let si = StreamingIndicator::new();
        assert!(!si.is_active());
        assert_eq!(si.token_count(), 0);
        assert!(si.tokens_per_second().is_none());
        assert!(si.display_text().is_empty());
    }

    #[test]
    fn streaming_default() {
        let si = StreamingIndicator::default();
        assert!(!si.is_active());
    }

    #[test]
    fn streaming_start_stop() {
        let mut si = StreamingIndicator::new();
        si.start();
        assert!(si.is_active());
        assert_eq!(si.token_count(), 0);

        si.add_tokens(10);
        assert_eq!(si.token_count(), 10);

        si.stop();
        assert!(!si.is_active());
    }

    #[test]
    fn streaming_add_tokens() {
        let mut si = StreamingIndicator::new();
        si.start();
        si.add_tokens(5);
        si.add_tokens(3);
        assert_eq!(si.token_count(), 8);
    }

    #[test]
    fn streaming_set_elapsed() {
        let mut si = StreamingIndicator::new();
        si.start();
        si.add_tokens(100);
        si.set_elapsed(2000); // 2 seconds
        assert_eq!(si.elapsed_ms(), 2000);

        let tps = si.tokens_per_second().unwrap();
        assert!((tps - 50.0).abs() < 0.1); // 100 tokens / 2s = 50 tok/s
    }

    #[test]
    fn streaming_display_text() {
        let mut si = StreamingIndicator::new();
        si.start();
        si.add_tokens(42);
        assert_eq!(si.display_text(), "42 tokens");

        si.set_elapsed(1000);
        let text = si.display_text();
        assert!(text.contains("42 tokens"));
        assert!(text.contains("tok/s"));
    }

    #[test]
    fn streaming_display_inactive() {
        let si = StreamingIndicator::new();
        assert!(si.display_text().is_empty());
    }

    #[test]
    fn streaming_start_resets() {
        let mut si = StreamingIndicator::new();
        si.start();
        si.add_tokens(50);
        si.set_elapsed(1000);

        si.start(); // reset
        assert_eq!(si.token_count(), 0);
        assert!(si.tokens_per_second().is_none());
        assert_eq!(si.elapsed_ms(), 0);
    }

    #[test]
    fn streaming_widget_inactive() {
        let si = StreamingIndicator::new();
        let theme = Theme::dark();
        let widget = StreamingIndicatorWidget::new(&si, &theme);
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);

        let row: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert_eq!(row.trim(), "");
    }

    #[test]
    fn streaming_widget_active() {
        let mut si = StreamingIndicator::new();
        si.start();
        si.add_tokens(25);
        let theme = Theme::dark();
        let widget = StreamingIndicatorWidget::new(&si, &theme);
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);

        let row: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(row.contains("25 tokens"));
    }
}
