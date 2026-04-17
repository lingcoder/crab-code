//! Notification system with toast messages and queued notifications.
//!
//! Provides a `NotificationManager` for managing a queue of notifications
//! at different severity levels, and a `Toast` component that auto-dismisses
//! after a configurable duration.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

/// Notification severity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotificationLevel {
    Info,
    Warning,
    Error,
    Success,
}

impl NotificationLevel {
    /// Color associated with this level.
    #[must_use]
    pub fn color(self) -> Color {
        match self {
            Self::Info => Color::Cyan,
            Self::Warning => Color::Yellow,
            Self::Error => Color::Red,
            Self::Success => Color::Green,
        }
    }

    /// Icon prefix for display.
    #[must_use]
    pub fn icon(self) -> &'static str {
        match self {
            Self::Info => "i",
            Self::Warning => "!",
            Self::Error => "x",
            Self::Success => "*",
        }
    }
}

impl std::fmt::Display for NotificationLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Info => write!(f, "info"),
            Self::Warning => write!(f, "warning"),
            Self::Error => write!(f, "error"),
            Self::Success => write!(f, "success"),
        }
    }
}

/// A single notification entry.
#[derive(Debug, Clone)]
pub struct Notification {
    /// Severity level.
    pub level: NotificationLevel,
    /// Message text.
    pub message: String,
    /// When the notification was created.
    pub created_at: Instant,
    /// How long to display (None = until dismissed).
    pub duration: Option<Duration>,
}

impl Notification {
    /// Create a new notification.
    #[must_use]
    pub fn new(level: NotificationLevel, message: impl Into<String>) -> Self {
        Self {
            level,
            message: message.into(),
            created_at: Instant::now(),
            duration: None,
        }
    }

    /// Set auto-dismiss duration.
    #[must_use]
    pub fn with_duration(mut self, duration: Duration) -> Self {
        self.duration = Some(duration);
        self
    }

    /// Whether this notification has expired.
    #[must_use]
    pub fn is_expired(&self) -> bool {
        self.duration
            .is_some_and(|d| self.created_at.elapsed() >= d)
    }

    /// Time remaining before expiry (None if no duration set or already expired).
    #[must_use]
    pub fn time_remaining(&self) -> Option<Duration> {
        self.duration.and_then(|d| {
            let elapsed = self.created_at.elapsed();
            d.checked_sub(elapsed)
        })
    }
}

/// Default toast display duration.
const DEFAULT_TOAST_DURATION: Duration = Duration::from_secs(3);

/// Maximum number of notifications to keep in history.
const MAX_HISTORY: usize = 100;

/// Manages a queue of notifications with auto-expiry.
pub struct NotificationManager {
    /// Active notifications (displayed as toasts).
    active: VecDeque<Notification>,
    /// Historical notifications (for review).
    history: VecDeque<Notification>,
    /// Maximum number of simultaneously visible toasts.
    max_visible: usize,
}

impl NotificationManager {
    #[must_use]
    pub fn new() -> Self {
        Self {
            active: VecDeque::new(),
            history: VecDeque::new(),
            max_visible: 3,
        }
    }

    /// Set max simultaneously visible notifications.
    #[must_use]
    pub fn with_max_visible(mut self, max: usize) -> Self {
        self.max_visible = max.max(1);
        self
    }

    /// Push a notification with default toast duration.
    pub fn notify(&mut self, level: NotificationLevel, message: impl Into<String>) {
        let notification = Notification::new(level, message).with_duration(DEFAULT_TOAST_DURATION);
        self.push(notification);
    }

    /// Push a notification with custom duration.
    pub fn notify_with_duration(
        &mut self,
        level: NotificationLevel,
        message: impl Into<String>,
        duration: Duration,
    ) {
        let notification = Notification::new(level, message).with_duration(duration);
        self.push(notification);
    }

    /// Push a persistent notification (no auto-dismiss).
    pub fn notify_persistent(&mut self, level: NotificationLevel, message: impl Into<String>) {
        let notification = Notification::new(level, message);
        self.push(notification);
    }

    /// Convenience: info notification.
    pub fn info(&mut self, message: impl Into<String>) {
        self.notify(NotificationLevel::Info, message);
    }

    /// Convenience: warning notification.
    pub fn warn(&mut self, message: impl Into<String>) {
        self.notify(NotificationLevel::Warning, message);
    }

    /// Convenience: error notification.
    pub fn error(&mut self, message: impl Into<String>) {
        self.notify(NotificationLevel::Error, message);
    }

    /// Convenience: success notification.
    pub fn success(&mut self, message: impl Into<String>) {
        self.notify(NotificationLevel::Success, message);
    }

    /// Tick: remove expired notifications, move to history.
    pub fn tick(&mut self) {
        let mut i = 0;
        while i < self.active.len() {
            if self.active[i].is_expired() {
                if let Some(n) = self.active.remove(i) {
                    self.history.push_back(n);
                    if self.history.len() > MAX_HISTORY {
                        self.history.pop_front();
                    }
                }
            } else {
                i += 1;
            }
        }
    }

    /// Dismiss the oldest active notification.
    pub fn dismiss(&mut self) {
        if let Some(n) = self.active.pop_front() {
            self.history.push_back(n);
            if self.history.len() > MAX_HISTORY {
                self.history.pop_front();
            }
        }
    }

    /// Dismiss all active notifications.
    pub fn dismiss_all(&mut self) {
        while let Some(n) = self.active.pop_front() {
            self.history.push_back(n);
        }
        while self.history.len() > MAX_HISTORY {
            self.history.pop_front();
        }
    }

    /// Get visible active notifications (up to `max_visible`).
    #[must_use]
    pub fn visible(&self) -> Vec<&Notification> {
        self.active.iter().take(self.max_visible).collect()
    }

    /// Number of active notifications.
    #[must_use]
    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    /// Number of historical notifications.
    #[must_use]
    pub fn history_count(&self) -> usize {
        self.history.len()
    }

    /// Whether there are any active notifications to display.
    #[must_use]
    pub fn has_active(&self) -> bool {
        !self.active.is_empty()
    }

    /// Get history entries (newest first).
    #[must_use]
    pub fn history(&self) -> Vec<&Notification> {
        self.history.iter().rev().collect()
    }

    /// Clear all history.
    pub fn clear_history(&mut self) {
        self.history.clear();
    }

    fn push(&mut self, notification: Notification) {
        self.active.push_back(notification);
    }
}

impl Default for NotificationManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Renders active toast notifications as an overlay.
pub struct ToastRenderer<'a> {
    manager: &'a NotificationManager,
}

impl<'a> ToastRenderer<'a> {
    #[must_use]
    pub fn new(manager: &'a NotificationManager) -> Self {
        Self { manager }
    }
}

impl Widget for ToastRenderer<'_> {
    #[allow(clippy::cast_possible_truncation)]
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width < 10 {
            return;
        }

        let visible = self.manager.visible();
        if visible.is_empty() {
            return;
        }

        // Render toasts from the top of the area, right-aligned
        let max_width = area.width.min(60);
        let x_offset = area.x + area.width.saturating_sub(max_width);

        for (i, notification) in visible.iter().enumerate() {
            let y = area.y + i as u16;
            if y >= area.y + area.height {
                break;
            }

            let level_color = notification.level.color();
            let icon = notification.level.icon();

            // Truncate message to fit
            let max_msg_len = (max_width as usize).saturating_sub(5); // icon + brackets + spaces
            let msg = if notification.message.len() > max_msg_len {
                format!(
                    "{}...",
                    &notification.message[..max_msg_len.saturating_sub(3)]
                )
            } else {
                notification.message.clone()
            };

            let line = Line::from(vec![
                Span::styled(
                    format!("[{icon}]"),
                    Style::default()
                        .fg(level_color)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(msg, Style::default().fg(Color::White)),
            ]);

            let toast_area = Rect {
                x: x_offset,
                y,
                width: max_width,
                height: 1,
            };
            Widget::render(line, toast_area, buf);
        }
    }
}

/// Progress indicator for tool execution.
#[derive(Debug, Clone)]
pub struct ProgressIndicator {
    /// Label describing what's in progress.
    label: String,
    /// Progress value (0.0 to 1.0). None = indeterminate.
    progress: Option<f64>,
    /// Whether the indicator is active.
    active: bool,
    /// Spinner frame for indeterminate progress.
    frame: usize,
}

/// Spinner frames for the progress indicator.
const PROGRESS_FRAMES: &[&str] = &["|", "/", "-", "\\"];

impl ProgressIndicator {
    #[must_use]
    pub fn new() -> Self {
        Self {
            label: String::new(),
            progress: None,
            active: false,
            frame: 0,
        }
    }

    /// Start indeterminate progress.
    pub fn start(&mut self, label: impl Into<String>) {
        self.label = label.into();
        self.progress = None;
        self.active = true;
        self.frame = 0;
    }

    /// Start determinate progress (0.0 to 1.0).
    pub fn start_determinate(&mut self, label: impl Into<String>, progress: f64) {
        self.label = label.into();
        self.progress = Some(progress.clamp(0.0, 1.0));
        self.active = true;
    }

    /// Update progress value.
    pub fn set_progress(&mut self, progress: f64) {
        self.progress = Some(progress.clamp(0.0, 1.0));
    }

    /// Update the label.
    pub fn set_label(&mut self, label: impl Into<String>) {
        self.label = label.into();
    }

    /// Stop the indicator.
    pub fn stop(&mut self) {
        self.active = false;
    }

    /// Advance spinner frame (call on tick).
    pub fn tick(&mut self) {
        if self.active && self.progress.is_none() {
            self.frame = (self.frame + 1) % PROGRESS_FRAMES.len();
        }
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active
    }

    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    #[must_use]
    pub fn progress(&self) -> Option<f64> {
        self.progress
    }
}

impl Default for ProgressIndicator {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for &ProgressIndicator {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn render(self, area: Rect, buf: &mut Buffer) {
        if !self.active || area.height == 0 || area.width < 10 {
            return;
        }

        if let Some(pct) = self.progress {
            // Determinate: [========>   ] 75% label
            let bar_width = (area.width as usize).saturating_sub(self.label.len() + 8);
            #[allow(clippy::cast_precision_loss)]
            let filled = (pct * bar_width as f64) as usize;
            let empty = bar_width.saturating_sub(filled);

            let bar = format!(
                "[{}{}] {:>3}%",
                "=".repeat(filled.saturating_sub(1)) + if filled > 0 { ">" } else { "" },
                " ".repeat(empty),
                (pct * 100.0) as u32
            );

            let line = Line::from(vec![
                Span::styled(bar, Style::default().fg(Color::Cyan)),
                Span::raw(" "),
                Span::styled(&self.label, Style::default().fg(Color::Gray)),
            ]);
            let line_area = Rect { height: 1, ..area };
            Widget::render(line, line_area, buf);
        } else {
            // Indeterminate: spinner + label
            let frame_char = PROGRESS_FRAMES[self.frame];
            let line = Line::from(vec![
                Span::styled(frame_char, Style::default().fg(Color::Cyan)),
                Span::raw(" "),
                Span::styled(&self.label, Style::default().fg(Color::Gray)),
            ]);
            let line_area = Rect { height: 1, ..area };
            Widget::render(line, line_area, buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── NotificationLevel tests ───

    #[test]
    fn level_colors() {
        assert_eq!(NotificationLevel::Info.color(), Color::Cyan);
        assert_eq!(NotificationLevel::Warning.color(), Color::Yellow);
        assert_eq!(NotificationLevel::Error.color(), Color::Red);
        assert_eq!(NotificationLevel::Success.color(), Color::Green);
    }

    #[test]
    fn level_icons() {
        assert_eq!(NotificationLevel::Info.icon(), "i");
        assert_eq!(NotificationLevel::Warning.icon(), "!");
        assert_eq!(NotificationLevel::Error.icon(), "x");
        assert_eq!(NotificationLevel::Success.icon(), "*");
    }

    #[test]
    fn level_display() {
        assert_eq!(NotificationLevel::Info.to_string(), "info");
        assert_eq!(NotificationLevel::Error.to_string(), "error");
    }

    // ─── Notification tests ───

    #[test]
    fn notification_not_expired_by_default() {
        let n = Notification::new(NotificationLevel::Info, "test");
        assert!(!n.is_expired());
        assert!(n.time_remaining().is_none());
    }

    #[test]
    fn notification_with_zero_duration_expires_immediately() {
        let n = Notification::new(NotificationLevel::Info, "test").with_duration(Duration::ZERO);
        assert!(n.is_expired());
    }

    #[test]
    fn notification_time_remaining() {
        let n = Notification::new(NotificationLevel::Info, "test")
            .with_duration(Duration::from_secs(60));
        let remaining = n.time_remaining();
        assert!(remaining.is_some());
        assert!(remaining.unwrap() <= Duration::from_secs(60));
    }

    // ─── NotificationManager tests ───

    #[test]
    fn manager_starts_empty() {
        let mgr = NotificationManager::new();
        assert_eq!(mgr.active_count(), 0);
        assert_eq!(mgr.history_count(), 0);
        assert!(!mgr.has_active());
    }

    #[test]
    fn manager_notify_adds_active() {
        let mut mgr = NotificationManager::new();
        mgr.info("hello");
        assert_eq!(mgr.active_count(), 1);
        assert!(mgr.has_active());
    }

    #[test]
    fn manager_convenience_methods() {
        let mut mgr = NotificationManager::new();
        mgr.info("i");
        mgr.warn("w");
        mgr.error("e");
        mgr.success("s");
        assert_eq!(mgr.active_count(), 4);

        let visible = mgr.visible();
        assert_eq!(visible.len(), 3); // max_visible default = 3
        assert_eq!(visible[0].level, NotificationLevel::Info);
        assert_eq!(visible[1].level, NotificationLevel::Warning);
        assert_eq!(visible[2].level, NotificationLevel::Error);
    }

    #[test]
    fn manager_dismiss() {
        let mut mgr = NotificationManager::new();
        mgr.info("first");
        mgr.warn("second");
        mgr.dismiss();
        assert_eq!(mgr.active_count(), 1);
        assert_eq!(mgr.history_count(), 1);
    }

    #[test]
    fn manager_dismiss_all() {
        let mut mgr = NotificationManager::new();
        mgr.info("a");
        mgr.warn("b");
        mgr.error("c");
        mgr.dismiss_all();
        assert_eq!(mgr.active_count(), 0);
        assert_eq!(mgr.history_count(), 3);
    }

    #[test]
    fn manager_tick_removes_expired() {
        let mut mgr = NotificationManager::new();
        mgr.notify_with_duration(
            NotificationLevel::Info,
            "quick",
            Duration::ZERO, // expires immediately
        );
        mgr.notify_persistent(NotificationLevel::Warning, "stays");
        mgr.tick();
        assert_eq!(mgr.active_count(), 1);
        assert_eq!(mgr.history_count(), 1);
        assert_eq!(mgr.visible()[0].level, NotificationLevel::Warning);
    }

    #[test]
    fn manager_persistent_stays() {
        let mut mgr = NotificationManager::new();
        mgr.notify_persistent(NotificationLevel::Error, "persistent");
        mgr.tick();
        assert_eq!(mgr.active_count(), 1);
    }

    #[test]
    fn manager_max_visible() {
        let mut mgr = NotificationManager::new().with_max_visible(2);
        mgr.info("a");
        mgr.info("b");
        mgr.info("c");
        assert_eq!(mgr.visible().len(), 2);
        assert_eq!(mgr.active_count(), 3);
    }

    #[test]
    fn manager_history_order() {
        let mut mgr = NotificationManager::new();
        mgr.info("first");
        mgr.info("second");
        mgr.dismiss();
        mgr.dismiss();
        let history = mgr.history();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].message, "second"); // newest first
        assert_eq!(history[1].message, "first");
    }

    #[test]
    fn manager_clear_history() {
        let mut mgr = NotificationManager::new();
        mgr.info("a");
        mgr.dismiss();
        mgr.clear_history();
        assert_eq!(mgr.history_count(), 0);
    }

    #[test]
    fn manager_default() {
        let mgr = NotificationManager::default();
        assert_eq!(mgr.active_count(), 0);
    }

    // ─── ToastRenderer tests ───

    #[test]
    fn toast_renders_active_notifications() {
        let mut mgr = NotificationManager::new();
        mgr.error("Something failed");

        let renderer = ToastRenderer::new(&mgr);
        let area = Rect::new(0, 0, 60, 5);
        let mut buf = Buffer::empty(area);
        Widget::render(renderer, area, &mut buf);

        let content: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(content.contains("Something failed"));
    }

    #[test]
    fn toast_renders_nothing_when_empty() {
        let mgr = NotificationManager::new();
        let renderer = ToastRenderer::new(&mgr);
        let area = Rect::new(0, 0, 60, 5);
        let mut buf = Buffer::empty(area);
        Widget::render(renderer, area, &mut buf);

        let content: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert_eq!(content.trim(), "");
    }

    #[test]
    fn toast_tiny_area_no_panic() {
        let mut mgr = NotificationManager::new();
        mgr.info("test");
        let renderer = ToastRenderer::new(&mgr);
        let area = Rect::new(0, 0, 5, 1);
        let mut buf = Buffer::empty(area);
        Widget::render(renderer, area, &mut buf);
    }

    // ─── ProgressIndicator tests ───

    #[test]
    fn progress_starts_inactive() {
        let pi = ProgressIndicator::new();
        assert!(!pi.is_active());
        assert!(pi.label().is_empty());
        assert!(pi.progress().is_none());
    }

    #[test]
    fn progress_indeterminate() {
        let mut pi = ProgressIndicator::new();
        pi.start("Running tool...");
        assert!(pi.is_active());
        assert!(pi.progress().is_none());
        assert_eq!(pi.label(), "Running tool...");
    }

    #[test]
    fn progress_determinate() {
        let mut pi = ProgressIndicator::new();
        pi.start_determinate("Uploading", 0.5);
        assert!(pi.is_active());
        assert!((pi.progress().unwrap() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn progress_set_progress_clamps() {
        let mut pi = ProgressIndicator::new();
        pi.start_determinate("test", 0.0);
        pi.set_progress(1.5);
        assert!((pi.progress().unwrap() - 1.0).abs() < f64::EPSILON);
        pi.set_progress(-0.5);
        assert!((pi.progress().unwrap() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn progress_tick_advances_indeterminate() {
        let mut pi = ProgressIndicator::new();
        pi.start("test");
        assert_eq!(pi.frame, 0);
        pi.tick();
        assert_eq!(pi.frame, 1);
    }

    #[test]
    fn progress_tick_no_advance_determinate() {
        let mut pi = ProgressIndicator::new();
        pi.start_determinate("test", 0.5);
        pi.tick();
        assert_eq!(pi.frame, 0);
    }

    #[test]
    fn progress_stop() {
        let mut pi = ProgressIndicator::new();
        pi.start("test");
        pi.stop();
        assert!(!pi.is_active());
    }

    #[test]
    fn progress_set_label() {
        let mut pi = ProgressIndicator::new();
        pi.start("first");
        pi.set_label("second");
        assert_eq!(pi.label(), "second");
    }

    #[test]
    fn progress_default() {
        let pi = ProgressIndicator::default();
        assert!(!pi.is_active());
    }

    #[test]
    fn progress_renders_indeterminate() {
        let mut pi = ProgressIndicator::new();
        pi.start("Working...");

        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        Widget::render(&pi, area, &mut buf);

        let content: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(content.contains("Working..."));
    }

    #[test]
    fn progress_renders_determinate() {
        let mut pi = ProgressIndicator::new();
        pi.start_determinate("Uploading", 0.75);

        let area = Rect::new(0, 0, 50, 1);
        let mut buf = Buffer::empty(area);
        Widget::render(&pi, area, &mut buf);

        let content: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(content.contains("75%"));
        assert!(content.contains("Uploading"));
    }

    #[test]
    fn progress_inactive_no_render() {
        let pi = ProgressIndicator::new();
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        Widget::render(&pi, area, &mut buf);

        let content: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert_eq!(content.trim(), "");
    }
}
