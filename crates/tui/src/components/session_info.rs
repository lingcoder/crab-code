//! Session info panel — detailed information about the current session.
//!
//! Displays token usage, cost estimates, session duration, tool statistics,
//! and a sparkline of token usage over time. Supports export.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::theme::Theme;

// ─── Types ──────────────────────────────────────────────────────────────

/// Tool usage statistics for the session.
#[derive(Debug, Clone)]
pub struct ToolUsageStat {
    /// Tool name.
    pub name: String,
    /// Number of invocations.
    pub invocations: usize,
    /// Total execution time in milliseconds.
    pub total_time_ms: u64,
}

impl ToolUsageStat {
    /// Create a new tool stat.
    pub fn new(name: impl Into<String>, invocations: usize, total_time_ms: u64) -> Self {
        Self {
            name: name.into(),
            invocations,
            total_time_ms,
        }
    }

    /// Average execution time per invocation.
    #[must_use]
    pub fn avg_time_ms(&self) -> u64 {
        if self.invocations == 0 {
            0
        } else {
            self.total_time_ms / self.invocations as u64
        }
    }
}

/// Export format options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Markdown,
    Json,
}

impl std::fmt::Display for ExportFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Markdown => write!(f, "Markdown"),
            Self::Json => write!(f, "JSON"),
        }
    }
}

/// Sparkline data point.
#[derive(Debug, Clone, Copy)]
pub struct SparklinePoint {
    /// Value (e.g., token count for this interval).
    pub value: u64,
}

// ─── Sparkline renderer ─────────────────────────────────────────────────

/// Braille-style sparkline characters (8 levels).
const SPARK_CHARS: &[char] = &[
    ' ', '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}',
];

/// Render a sparkline from data points into a single-line string.
#[must_use]
pub fn render_sparkline(data: &[SparklinePoint], width: usize) -> String {
    if data.is_empty() || width == 0 {
        return String::new();
    }

    let max_val = data.iter().map(|p| p.value).max().unwrap_or(1).max(1);

    // Sample or take data points to fit width
    let points: Vec<u64> = if data.len() <= width {
        data.iter().map(|p| p.value).collect()
    } else {
        // Downsample
        (0..width)
            .map(|i| {
                let start = i * data.len() / width;
                let end = ((i + 1) * data.len() / width).min(data.len());
                let sum: u64 = data[start..end].iter().map(|p| p.value).sum();
                let count = (end - start) as u64;
                if count > 0 { sum / count } else { 0 }
            })
            .collect()
    };

    points
        .iter()
        .map(|&v| {
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_precision_loss,
                clippy::cast_sign_loss,
                clippy::too_many_lines
            )]
            let level =
                ((v as f64 / max_val as f64) * (SPARK_CHARS.len() - 1) as f64).round() as usize;
            SPARK_CHARS[level.min(SPARK_CHARS.len() - 1)]
        })
        .collect()
}

// ─── SessionInfo state ──────────────────────────────────────────────────

/// Session information panel state.
pub struct SessionInfo {
    /// Session title.
    title: String,
    /// Session ID.
    session_id: String,
    /// Total input tokens.
    input_tokens: u64,
    /// Total output tokens.
    output_tokens: u64,
    /// Estimated cost in USD (cents).
    cost_cents: f64,
    /// Session duration in seconds.
    duration_secs: u64,
    /// Number of messages.
    message_count: usize,
    /// Tool usage statistics.
    tool_stats: Vec<ToolUsageStat>,
    /// Token usage over time (sparkline data).
    token_history: Vec<SparklinePoint>,
    /// Selected export format.
    export_format: ExportFormat,
}

impl SessionInfo {
    /// Create a new session info panel.
    #[must_use]
    pub fn new(session_id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            session_id: session_id.into(),
            input_tokens: 0,
            output_tokens: 0,
            cost_cents: 0.0,
            duration_secs: 0,
            message_count: 0,
            tool_stats: Vec::new(),
            token_history: Vec::new(),
            export_format: ExportFormat::Markdown,
        }
    }

    /// Set token usage.
    pub fn set_tokens(&mut self, input: u64, output: u64) {
        self.input_tokens = input;
        self.output_tokens = output;
    }

    /// Set cost estimate.
    pub fn set_cost(&mut self, cents: f64) {
        self.cost_cents = cents;
    }

    /// Set session duration.
    pub fn set_duration(&mut self, secs: u64) {
        self.duration_secs = secs;
    }

    /// Set message count.
    pub fn set_message_count(&mut self, count: usize) {
        self.message_count = count;
    }

    /// Set tool statistics.
    pub fn set_tool_stats(&mut self, stats: Vec<ToolUsageStat>) {
        self.tool_stats = stats;
    }

    /// Set token history for sparkline.
    pub fn set_token_history(&mut self, history: Vec<SparklinePoint>) {
        self.token_history = history;
    }

    /// Toggle export format.
    pub fn toggle_export_format(&mut self) {
        self.export_format = match self.export_format {
            ExportFormat::Markdown => ExportFormat::Json,
            ExportFormat::Json => ExportFormat::Markdown,
        };
    }

    // ─── Getters ───

    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    #[must_use]
    pub fn input_tokens(&self) -> u64 {
        self.input_tokens
    }

    #[must_use]
    pub fn output_tokens(&self) -> u64 {
        self.output_tokens
    }

    #[must_use]
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }

    #[must_use]
    pub fn cost_cents(&self) -> f64 {
        self.cost_cents
    }

    #[must_use]
    pub fn duration_secs(&self) -> u64 {
        self.duration_secs
    }

    #[must_use]
    pub fn message_count(&self) -> usize {
        self.message_count
    }

    #[must_use]
    pub fn tool_stats(&self) -> &[ToolUsageStat] {
        &self.tool_stats
    }

    #[must_use]
    pub fn token_history(&self) -> &[SparklinePoint] {
        &self.token_history
    }

    #[must_use]
    pub fn export_format(&self) -> ExportFormat {
        self.export_format
    }

    /// Format duration as human-readable.
    #[must_use]
    pub fn formatted_duration(&self) -> String {
        let secs = self.duration_secs;
        if secs < 60 {
            return format!("{secs}s");
        }
        let mins = secs / 60;
        let rem_secs = secs % 60;
        if mins < 60 {
            return format!("{mins}m {rem_secs}s");
        }
        let hours = mins / 60;
        let rem_mins = mins % 60;
        format!("{hours}h {rem_mins}m")
    }

    /// Format cost as a dollar string.
    #[must_use]
    pub fn formatted_cost(&self) -> String {
        if self.cost_cents < 1.0 {
            "<$0.01".to_string()
        } else {
            format!("${:.2}", self.cost_cents / 100.0)
        }
    }

    /// Total tool invocations.
    #[must_use]
    pub fn total_tool_invocations(&self) -> usize {
        self.tool_stats.iter().map(|s| s.invocations).sum()
    }
}

// ─── Widget ─────────────────────────────────────────────────────────────

/// Widget for rendering the session info panel.
pub struct SessionInfoWidget<'a> {
    info: &'a SessionInfo,
    theme: &'a Theme,
}

impl<'a> SessionInfoWidget<'a> {
    #[must_use]
    pub fn new(info: &'a SessionInfo, theme: &'a Theme) -> Self {
        Self { info, theme }
    }
}

impl Widget for SessionInfoWidget<'_> {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss,
        clippy::too_many_lines
    )]
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 5 || area.width < 20 {
            return;
        }

        let mut y = area.y;
        let heading_style = Style::default()
            .fg(self.theme.heading)
            .add_modifier(Modifier::BOLD);
        let label_style = Style::default().fg(self.theme.muted);
        let value_style = Style::default().fg(self.theme.fg);

        // Title
        let title_line = Line::from(Span::styled(
            format!(" {}", self.info.title()),
            heading_style,
        ));
        Widget::render(title_line, Rect::new(area.x, y, area.width, 1), buf);
        y += 1;

        // Session ID
        let id_line = Line::from(vec![
            Span::styled(" ID: ", label_style),
            Span::styled(self.info.session_id(), value_style),
        ]);
        Widget::render(id_line, Rect::new(area.x, y, area.width, 1), buf);
        y += 2; // blank line

        if y >= area.y + area.height {
            return;
        }

        // Token usage
        let tokens_line = Line::from(vec![
            Span::styled(" Tokens: ", label_style),
            Span::styled(
                format!(
                    "{} total ({} in / {} out)",
                    self.info.total_tokens(),
                    self.info.input_tokens(),
                    self.info.output_tokens(),
                ),
                value_style,
            ),
        ]);
        Widget::render(tokens_line, Rect::new(area.x, y, area.width, 1), buf);
        y += 1;

        if y >= area.y + area.height {
            return;
        }

        // Cost
        let cost_line = Line::from(vec![
            Span::styled(" Cost:   ", label_style),
            Span::styled(self.info.formatted_cost(), value_style),
        ]);
        Widget::render(cost_line, Rect::new(area.x, y, area.width, 1), buf);
        y += 1;

        if y >= area.y + area.height {
            return;
        }

        // Duration
        let dur_line = Line::from(vec![
            Span::styled(" Duration:", label_style),
            Span::styled(format!(" {}", self.info.formatted_duration()), value_style),
        ]);
        Widget::render(dur_line, Rect::new(area.x, y, area.width, 1), buf);
        y += 1;

        // Messages
        let msg_line = Line::from(vec![
            Span::styled(" Messages:", label_style),
            Span::styled(format!(" {}", self.info.message_count()), value_style),
        ]);
        Widget::render(msg_line, Rect::new(area.x, y, area.width, 1), buf);
        y += 2;

        if y >= area.y + area.height {
            return;
        }

        // Sparkline
        if !self.info.token_history().is_empty() {
            let spark_label = Line::from(Span::styled(" Token Usage:", label_style));
            Widget::render(spark_label, Rect::new(area.x, y, area.width, 1), buf);
            y += 1;

            if y < area.y + area.height {
                let spark_width = (area.width - 2) as usize;
                let sparkline = render_sparkline(self.info.token_history(), spark_width);
                let spark_display = Line::from(Span::styled(
                    format!(" {sparkline}"),
                    Style::default().fg(self.theme.success),
                ));
                Widget::render(spark_display, Rect::new(area.x, y, area.width, 1), buf);
                y += 2;
            }
        }

        if y >= area.y + area.height {
            return;
        }

        // Tool stats
        if !self.info.tool_stats().is_empty() {
            let tools_label = Line::from(Span::styled(
                format!(" Tools ({}):", self.info.total_tool_invocations()),
                label_style,
            ));
            Widget::render(tools_label, Rect::new(area.x, y, area.width, 1), buf);
            y += 1;

            for stat in self.info.tool_stats() {
                if y >= area.y + area.height {
                    break;
                }
                let stat_line = Line::from(vec![
                    Span::styled(format!("   {} ", stat.name), value_style),
                    Span::styled(
                        format!("x{} (avg {}ms)", stat.invocations, stat.avg_time_ms()),
                        label_style,
                    ),
                ]);
                Widget::render(stat_line, Rect::new(area.x, y, area.width, 1), buf);
                y += 1;
            }
            y += 1;
        }

        if y >= area.y + area.height {
            return;
        }

        // Export footer
        let export_line = Line::from(vec![
            Span::styled(" Export: ", label_style),
            Span::styled(
                format!("[{}]", self.info.export_format()),
                Style::default()
                    .fg(self.theme.link)
                    .add_modifier(Modifier::UNDERLINED),
            ),
        ]);
        Widget::render(export_line, Rect::new(area.x, y, area.width, 1), buf);
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_usage_stat_new() {
        let stat = ToolUsageStat::new("bash", 10, 5000);
        assert_eq!(stat.name, "bash");
        assert_eq!(stat.invocations, 10);
        assert_eq!(stat.total_time_ms, 5000);
        assert_eq!(stat.avg_time_ms(), 500);
    }

    #[test]
    fn tool_usage_stat_zero_invocations() {
        let stat = ToolUsageStat::new("read", 0, 0);
        assert_eq!(stat.avg_time_ms(), 0);
    }

    #[test]
    fn export_format_display() {
        assert_eq!(ExportFormat::Markdown.to_string(), "Markdown");
        assert_eq!(ExportFormat::Json.to_string(), "JSON");
    }

    #[test]
    fn render_sparkline_empty() {
        assert_eq!(render_sparkline(&[], 10), "");
    }

    #[test]
    fn render_sparkline_zero_width() {
        let data = vec![SparklinePoint { value: 5 }];
        assert_eq!(render_sparkline(&data, 0), "");
    }

    #[test]
    fn render_sparkline_basic() {
        let data = vec![
            SparklinePoint { value: 0 },
            SparklinePoint { value: 50 },
            SparklinePoint { value: 100 },
        ];
        let result = render_sparkline(&data, 3);
        assert_eq!(result.chars().count(), 3);
    }

    #[test]
    fn render_sparkline_all_same() {
        let data = vec![
            SparklinePoint { value: 10 },
            SparklinePoint { value: 10 },
            SparklinePoint { value: 10 },
        ];
        let result = render_sparkline(&data, 3);
        // All same value should use the top character
        let chars: Vec<char> = result.chars().collect();
        assert_eq!(chars[0], chars[1]);
        assert_eq!(chars[1], chars[2]);
    }

    #[test]
    fn render_sparkline_downsample() {
        let data: Vec<SparklinePoint> = (0..100).map(|i| SparklinePoint { value: i }).collect();
        let result = render_sparkline(&data, 10);
        assert_eq!(result.chars().count(), 10);
    }

    #[test]
    fn session_info_new() {
        let info = SessionInfo::new("s1", "Test Session");
        assert_eq!(info.session_id(), "s1");
        assert_eq!(info.title(), "Test Session");
        assert_eq!(info.input_tokens(), 0);
        assert_eq!(info.output_tokens(), 0);
        assert_eq!(info.total_tokens(), 0);
        assert_eq!(info.message_count(), 0);
        assert_eq!(info.export_format(), ExportFormat::Markdown);
    }

    #[test]
    fn session_info_set_tokens() {
        let mut info = SessionInfo::new("s1", "T");
        info.set_tokens(1000, 500);
        assert_eq!(info.input_tokens(), 1000);
        assert_eq!(info.output_tokens(), 500);
        assert_eq!(info.total_tokens(), 1500);
    }

    #[test]
    fn session_info_set_cost() {
        let mut info = SessionInfo::new("s1", "T");
        info.set_cost(150.0);
        assert!((info.cost_cents() - 150.0).abs() < f64::EPSILON);
        assert_eq!(info.formatted_cost(), "$1.50");
    }

    #[test]
    fn session_info_formatted_cost_small() {
        let mut info = SessionInfo::new("s1", "T");
        info.set_cost(0.5);
        assert_eq!(info.formatted_cost(), "<$0.01");
    }

    #[test]
    fn session_info_set_duration() {
        let mut info = SessionInfo::new("s1", "T");
        info.set_duration(30);
        assert_eq!(info.formatted_duration(), "30s");

        info.set_duration(90);
        assert_eq!(info.formatted_duration(), "1m 30s");

        info.set_duration(3661);
        assert_eq!(info.formatted_duration(), "1h 1m");
    }

    #[test]
    fn session_info_set_message_count() {
        let mut info = SessionInfo::new("s1", "T");
        info.set_message_count(42);
        assert_eq!(info.message_count(), 42);
    }

    #[test]
    fn session_info_tool_stats() {
        let mut info = SessionInfo::new("s1", "T");
        info.set_tool_stats(vec![
            ToolUsageStat::new("bash", 5, 2500),
            ToolUsageStat::new("read", 10, 1000),
        ]);
        assert_eq!(info.tool_stats().len(), 2);
        assert_eq!(info.total_tool_invocations(), 15);
    }

    #[test]
    fn session_info_token_history() {
        let mut info = SessionInfo::new("s1", "T");
        info.set_token_history(vec![
            SparklinePoint { value: 10 },
            SparklinePoint { value: 20 },
        ]);
        assert_eq!(info.token_history().len(), 2);
    }

    #[test]
    fn session_info_toggle_export() {
        let mut info = SessionInfo::new("s1", "T");
        assert_eq!(info.export_format(), ExportFormat::Markdown);
        info.toggle_export_format();
        assert_eq!(info.export_format(), ExportFormat::Json);
        info.toggle_export_format();
        assert_eq!(info.export_format(), ExportFormat::Markdown);
    }

    #[test]
    fn widget_renders() {
        let mut info = SessionInfo::new("sess_001", "My Test Session");
        info.set_tokens(5000, 3000);
        info.set_cost(250.0);
        info.set_duration(180);
        info.set_message_count(15);
        info.set_tool_stats(vec![ToolUsageStat::new("bash", 5, 2000)]);
        info.set_token_history(vec![
            SparklinePoint { value: 100 },
            SparklinePoint { value: 200 },
            SparklinePoint { value: 150 },
        ]);

        let theme = Theme::dark();
        let widget = SessionInfoWidget::new(&info, &theme);
        let area = Rect::new(0, 0, 50, 20);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);

        // Check title is rendered
        let row0: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(row0.contains("My Test Session"));

        // Check tokens line exists somewhere
        let mut found_tokens = false;
        for y in 0..area.height {
            let row: String = (0..area.width)
                .map(|x| buf.cell((x, y)).unwrap().symbol().to_string())
                .collect();
            if row.contains("8000 total") {
                found_tokens = true;
                break;
            }
        }
        assert!(found_tokens, "Should show total tokens");
    }

    #[test]
    fn widget_renders_minimal() {
        let info = SessionInfo::new("s1", "Minimal");
        let theme = Theme::dark();
        let widget = SessionInfoWidget::new(&info, &theme);
        let area = Rect::new(0, 0, 30, 8);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);
    }

    #[test]
    fn widget_small_area() {
        let info = SessionInfo::new("s1", "Test");
        let theme = Theme::dark();
        let widget = SessionInfoWidget::new(&info, &theme);
        let area = Rect::new(0, 0, 10, 3);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);
    }
}
