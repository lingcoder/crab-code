//! Progress indicators — horizontal progress bar, spinner animation,
//! and multi-progress display for concurrent operations.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;

use crate::theme::Theme;

// ─── Progress style ─────────────────────────────────────────────────────

/// Configuration for how a progress bar is rendered.
#[derive(Debug, Clone)]
pub struct ProgressStyle {
    /// Character for the filled portion.
    pub bar_char: char,
    /// Character for the empty portion.
    pub empty_char: char,
    /// Left bracket character.
    pub bracket_left: char,
    /// Right bracket character.
    pub bracket_right: char,
    /// Whether to show percentage text.
    pub show_percentage: bool,
    /// Whether to show estimated time remaining.
    pub show_eta: bool,
}

impl ProgressStyle {
    /// Default style with block characters.
    #[must_use]
    pub fn block() -> Self {
        Self {
            bar_char: '\u{2588}',   // █
            empty_char: '\u{2591}', // ░
            bracket_left: '[',
            bracket_right: ']',
            show_percentage: true,
            show_eta: false,
        }
    }

    /// Arrow-style progress bar.
    #[must_use]
    pub fn arrow() -> Self {
        Self {
            bar_char: '=',
            empty_char: ' ',
            bracket_left: '[',
            bracket_right: ']',
            show_percentage: true,
            show_eta: false,
        }
    }

    /// Thin line style.
    #[must_use]
    pub fn thin() -> Self {
        Self {
            bar_char: '\u{2501}',      // ━
            empty_char: '\u{2500}',    // ─
            bracket_left: '\u{2523}',  // ┣
            bracket_right: '\u{252b}', // ┫
            show_percentage: true,
            show_eta: false,
        }
    }

    /// Set whether to show percentage.
    #[must_use]
    pub fn with_percentage(mut self, show: bool) -> Self {
        self.show_percentage = show;
        self
    }

    /// Set whether to show ETA.
    #[must_use]
    pub fn with_eta(mut self, show: bool) -> Self {
        self.show_eta = show;
        self
    }
}

impl Default for ProgressStyle {
    fn default() -> Self {
        Self::block()
    }
}

// ─── ProgressBar ────────────────────────────────────────────────────────

/// A horizontal progress bar with percentage and optional label.
pub struct ProgressBar {
    /// Current progress (0.0 to 1.0).
    progress: f64,
    /// Optional label displayed before the bar.
    label: Option<String>,
    /// Rendering style.
    style: ProgressStyle,
    /// Optional estimated time remaining in seconds.
    eta_seconds: Option<u64>,
}

impl ProgressBar {
    /// Create a new progress bar at 0%.
    #[must_use]
    pub fn new() -> Self {
        Self {
            progress: 0.0,
            label: None,
            style: ProgressStyle::default(),
            eta_seconds: None,
        }
    }

    /// Create with a specific progress value.
    #[must_use]
    pub fn with_progress(mut self, progress: f64) -> Self {
        self.progress = progress.clamp(0.0, 1.0);
        self
    }

    /// Set the label.
    #[must_use]
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Set the rendering style.
    #[must_use]
    pub fn with_style(mut self, style: ProgressStyle) -> Self {
        self.style = style;
        self
    }

    /// Set progress (clamped to 0.0..=1.0).
    pub fn set_progress(&mut self, progress: f64) {
        self.progress = progress.clamp(0.0, 1.0);
    }

    /// Set the label.
    pub fn set_label(&mut self, label: impl Into<String>) {
        self.label = Some(label.into());
    }

    /// Set ETA in seconds.
    pub fn set_eta(&mut self, seconds: Option<u64>) {
        self.eta_seconds = seconds;
    }

    /// Get current progress.
    #[must_use]
    pub fn progress(&self) -> f64 {
        self.progress
    }

    /// Get the label.
    #[must_use]
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    /// Whether the progress is complete.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.progress >= 1.0
    }

    /// Get percentage as integer (0-100).
    #[must_use]
    pub fn percentage(&self) -> u8 {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let pct = (self.progress * 100.0).round() as u8;
        pct.min(100)
    }
}

impl Default for ProgressBar {
    fn default() -> Self {
        Self::new()
    }
}

/// Widget for rendering a progress bar.
pub struct ProgressBarWidget<'a> {
    bar: &'a ProgressBar,
    theme: &'a Theme,
}

impl<'a> ProgressBarWidget<'a> {
    #[must_use]
    pub fn new(bar: &'a ProgressBar, theme: &'a Theme) -> Self {
        Self { bar, theme }
    }
}

impl Widget for ProgressBarWidget<'_> {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width < 5 {
            return;
        }

        let y = area.y;
        let mut x_offset = area.x;
        let style = &self.bar.style;

        // Label
        if let Some(label) = &self.bar.label {
            let label_text = format!("{label} ");
            let label_style = Style::default().fg(self.theme.fg);
            for ch in label_text.chars() {
                if x_offset >= area.x + area.width {
                    break;
                }
                if let Some(cell) = buf.cell_mut((x_offset, y)) {
                    cell.set_char(ch);
                    cell.set_style(label_style);
                }
                x_offset += 1;
            }
        }

        // Percentage text (reserve space at the end)
        let pct_text = if style.show_percentage {
            format!(" {:>3}%", self.bar.percentage())
        } else {
            String::new()
        };
        let eta_text = if style.show_eta {
            self.bar
                .eta_seconds
                .map(|s| format!(" ETA {s}s"))
                .unwrap_or_default()
        } else {
            String::new()
        };
        let suffix_len = pct_text.len() + eta_text.len();

        // Bar area: remaining width minus brackets and suffix
        let total_remaining = (area.x + area.width).saturating_sub(x_offset) as usize;
        if total_remaining < 4 + suffix_len {
            return;
        }
        let bar_inner_width = total_remaining - 2 - suffix_len; // -2 for brackets

        // Left bracket
        if let Some(cell) = buf.cell_mut((x_offset, y)) {
            cell.set_char(style.bracket_left);
            cell.set_style(Style::default().fg(self.theme.border));
        }
        x_offset += 1;

        // Bar fill
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
        let filled = (self.bar.progress * bar_inner_width as f64).round() as usize;
        let bar_fg = if self.bar.is_complete() {
            self.theme.success
        } else {
            self.theme.heading
        };

        for i in 0..bar_inner_width {
            if x_offset >= area.x + area.width {
                break;
            }
            let ch = if i < filled {
                style.bar_char
            } else {
                style.empty_char
            };
            let fg = if i < filled { bar_fg } else { self.theme.muted };
            if let Some(cell) = buf.cell_mut((x_offset, y)) {
                cell.set_char(ch);
                cell.set_style(Style::default().fg(fg));
            }
            x_offset += 1;
        }

        // Right bracket
        if x_offset < area.x + area.width {
            if let Some(cell) = buf.cell_mut((x_offset, y)) {
                cell.set_char(style.bracket_right);
                cell.set_style(Style::default().fg(self.theme.border));
            }
            x_offset += 1;
        }

        // Percentage
        let suffix = format!("{pct_text}{eta_text}");
        let pct_style = Style::default().fg(self.theme.fg);
        for ch in suffix.chars() {
            if x_offset >= area.x + area.width {
                break;
            }
            if let Some(cell) = buf.cell_mut((x_offset, y)) {
                cell.set_char(ch);
                cell.set_style(pct_style);
            }
            x_offset += 1;
        }
    }
}

// ─── MultiProgress ──────────────────────────────────────────────────────

/// A named progress entry for multi-progress display.
#[derive(Debug, Clone)]
pub struct ProgressEntry {
    /// Unique identifier.
    pub id: String,
    /// Display label.
    pub label: String,
    /// Current progress (0.0 to 1.0).
    pub progress: f64,
    /// Whether this entry is complete.
    pub complete: bool,
}

impl ProgressEntry {
    /// Create a new progress entry.
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            progress: 0.0,
            complete: false,
        }
    }

    /// Set progress and mark complete if >= 1.0.
    pub fn set_progress(&mut self, progress: f64) {
        self.progress = progress.clamp(0.0, 1.0);
        if self.progress >= 1.0 {
            self.complete = true;
        }
    }
}

/// Displays multiple progress bars stacked vertically.
pub struct MultiProgress {
    entries: Vec<ProgressEntry>,
    style: ProgressStyle,
}

impl MultiProgress {
    /// Create a new multi-progress display.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            style: ProgressStyle::default(),
        }
    }

    /// Set the shared style.
    #[must_use]
    pub fn with_style(mut self, style: ProgressStyle) -> Self {
        self.style = style;
        self
    }

    /// Add a progress entry.
    pub fn add(&mut self, entry: ProgressEntry) {
        self.entries.push(entry);
    }

    /// Update a specific entry's progress by ID.
    pub fn update(&mut self, id: &str, progress: f64) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) {
            entry.set_progress(progress);
        }
    }

    /// Remove a completed entry by ID.
    pub fn remove(&mut self, id: &str) {
        self.entries.retain(|e| e.id != id);
    }

    /// Get all entries.
    #[must_use]
    pub fn entries(&self) -> &[ProgressEntry] {
        &self.entries
    }

    /// Number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether all entries are complete.
    #[must_use]
    pub fn all_complete(&self) -> bool {
        !self.entries.is_empty() && self.entries.iter().all(|e| e.complete)
    }

    /// Clear all entries.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

impl Default for MultiProgress {
    fn default() -> Self {
        Self::new()
    }
}

/// Widget for rendering multi-progress.
pub struct MultiProgressWidget<'a> {
    multi: &'a MultiProgress,
    theme: &'a Theme,
}

impl<'a> MultiProgressWidget<'a> {
    #[must_use]
    pub fn new(multi: &'a MultiProgress, theme: &'a Theme) -> Self {
        Self { multi, theme }
    }
}

impl Widget for MultiProgressWidget<'_> {
    #[allow(clippy::cast_possible_truncation)]
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width < 10 {
            return;
        }

        for (i, entry) in self.multi.entries().iter().enumerate() {
            if i >= area.height as usize {
                break;
            }
            let y = area.y + i as u16;
            let bar = ProgressBar::new()
                .with_progress(entry.progress)
                .with_label(&entry.label)
                .with_style(self.multi.style.clone());
            let bar_widget = ProgressBarWidget::new(&bar, self.theme);
            Widget::render(bar_widget, Rect::new(area.x, y, area.width, 1), buf);
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ─── ProgressStyle tests ───

    #[test]
    fn progress_style_block() {
        let s = ProgressStyle::block();
        assert_eq!(s.bar_char, '\u{2588}');
        assert!(s.show_percentage);
        assert!(!s.show_eta);
    }

    #[test]
    fn progress_style_arrow() {
        let s = ProgressStyle::arrow();
        assert_eq!(s.bar_char, '=');
    }

    #[test]
    fn progress_style_thin() {
        let s = ProgressStyle::thin();
        assert_eq!(s.bar_char, '\u{2501}');
    }

    #[test]
    fn progress_style_builders() {
        let s = ProgressStyle::block().with_percentage(false).with_eta(true);
        assert!(!s.show_percentage);
        assert!(s.show_eta);
    }

    #[test]
    fn progress_style_default() {
        let s = ProgressStyle::default();
        assert_eq!(s.bar_char, '\u{2588}');
    }

    // ─── ProgressBar tests ───

    #[test]
    fn progress_bar_new() {
        let bar = ProgressBar::new();
        assert_eq!(bar.progress(), 0.0);
        assert!(bar.label().is_none());
        assert!(!bar.is_complete());
        assert_eq!(bar.percentage(), 0);
    }

    #[test]
    fn progress_bar_default() {
        let bar = ProgressBar::default();
        assert_eq!(bar.progress(), 0.0);
    }

    #[test]
    fn progress_bar_with_progress() {
        let bar = ProgressBar::new().with_progress(0.5);
        assert!((bar.progress() - 0.5).abs() < f64::EPSILON);
        assert_eq!(bar.percentage(), 50);
    }

    #[test]
    fn progress_bar_clamp() {
        let bar = ProgressBar::new().with_progress(1.5);
        assert!((bar.progress() - 1.0).abs() < f64::EPSILON);
        assert!(bar.is_complete());

        let bar2 = ProgressBar::new().with_progress(-0.5);
        assert!((bar2.progress()).abs() < f64::EPSILON);
    }

    #[test]
    fn progress_bar_with_label() {
        let bar = ProgressBar::new().with_label("Downloading");
        assert_eq!(bar.label(), Some("Downloading"));
    }

    #[test]
    fn progress_bar_set_progress() {
        let mut bar = ProgressBar::new();
        bar.set_progress(0.75);
        assert_eq!(bar.percentage(), 75);

        bar.set_progress(2.0);
        assert!(bar.is_complete());
    }

    #[test]
    fn progress_bar_set_label() {
        let mut bar = ProgressBar::new();
        bar.set_label("New label");
        assert_eq!(bar.label(), Some("New label"));
    }

    #[test]
    fn progress_bar_set_eta() {
        let mut bar = ProgressBar::new();
        bar.set_eta(Some(30));
        assert_eq!(bar.eta_seconds, Some(30));
    }

    #[test]
    fn progress_bar_percentage_rounding() {
        let bar = ProgressBar::new().with_progress(0.333);
        assert_eq!(bar.percentage(), 33);

        let bar2 = ProgressBar::new().with_progress(0.999);
        assert_eq!(bar2.percentage(), 100);
    }

    #[test]
    fn progress_bar_complete() {
        let bar = ProgressBar::new().with_progress(1.0);
        assert!(bar.is_complete());
        assert_eq!(bar.percentage(), 100);
    }

    #[test]
    fn progress_bar_widget_renders() {
        let bar = ProgressBar::new().with_progress(0.5).with_label("Test");
        let theme = Theme::dark();
        let widget = ProgressBarWidget::new(&bar, &theme);
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);

        let row: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(row.contains("Test"));
        assert!(row.contains("50%"));
    }

    #[test]
    fn progress_bar_widget_no_label() {
        let bar = ProgressBar::new().with_progress(0.8);
        let theme = Theme::dark();
        let widget = ProgressBarWidget::new(&bar, &theme);
        let area = Rect::new(0, 0, 30, 1);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);

        let row: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(row.contains("80%"));
    }

    #[test]
    fn progress_bar_widget_small_area() {
        let bar = ProgressBar::new().with_progress(0.5);
        let theme = Theme::dark();
        let widget = ProgressBarWidget::new(&bar, &theme);
        let area = Rect::new(0, 0, 3, 1);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);
        // Should not panic
    }

    // ─── ProgressEntry tests ───

    #[test]
    fn progress_entry_new() {
        let entry = ProgressEntry::new("dl-1", "Downloading");
        assert_eq!(entry.id, "dl-1");
        assert_eq!(entry.label, "Downloading");
        assert!((entry.progress).abs() < f64::EPSILON);
        assert!(!entry.complete);
    }

    #[test]
    fn progress_entry_set_progress() {
        let mut entry = ProgressEntry::new("t", "Test");
        entry.set_progress(0.5);
        assert!((entry.progress - 0.5).abs() < f64::EPSILON);
        assert!(!entry.complete);

        entry.set_progress(1.0);
        assert!(entry.complete);
    }

    // ─── MultiProgress tests ───

    #[test]
    fn multi_progress_new() {
        let mp = MultiProgress::new();
        assert!(mp.is_empty());
        assert_eq!(mp.len(), 0);
        assert!(!mp.all_complete());
    }

    #[test]
    fn multi_progress_default() {
        let mp = MultiProgress::default();
        assert!(mp.is_empty());
    }

    #[test]
    fn multi_progress_add_update() {
        let mut mp = MultiProgress::new();
        mp.add(ProgressEntry::new("a", "Task A"));
        mp.add(ProgressEntry::new("b", "Task B"));
        assert_eq!(mp.len(), 2);

        mp.update("a", 0.5);
        assert!((mp.entries()[0].progress - 0.5).abs() < f64::EPSILON);

        mp.update("b", 1.0);
        assert!(mp.entries()[1].complete);
        assert!(!mp.all_complete());

        mp.update("a", 1.0);
        assert!(mp.all_complete());
    }

    #[test]
    fn multi_progress_remove() {
        let mut mp = MultiProgress::new();
        mp.add(ProgressEntry::new("a", "A"));
        mp.add(ProgressEntry::new("b", "B"));
        mp.remove("a");
        assert_eq!(mp.len(), 1);
        assert_eq!(mp.entries()[0].id, "b");
    }

    #[test]
    fn multi_progress_clear() {
        let mut mp = MultiProgress::new();
        mp.add(ProgressEntry::new("a", "A"));
        mp.clear();
        assert!(mp.is_empty());
    }

    #[test]
    fn multi_progress_widget_renders() {
        let mut mp = MultiProgress::new();
        mp.add(ProgressEntry::new("a", "Task A"));
        mp.add(ProgressEntry::new("b", "Task B"));
        mp.update("a", 0.3);
        mp.update("b", 0.7);

        let theme = Theme::dark();
        let widget = MultiProgressWidget::new(&mp, &theme);
        let area = Rect::new(0, 0, 50, 5);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);

        let row0: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(row0.contains("Task A"));

        let row1: String = (0..area.width)
            .map(|x| buf.cell((x, 1)).unwrap().symbol().to_string())
            .collect();
        assert!(row1.contains("Task B"));
    }
}
