//! Tool execution progress list component.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

/// Status of a tool execution task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Pending,
    Running,
    Done,
    Error,
}

impl TaskStatus {
    fn symbol(self) -> &'static str {
        match self {
            Self::Pending => "○",
            Self::Running => "◉",
            Self::Done => "✓",
            Self::Error => "✗",
        }
    }

    fn color(self) -> Color {
        match self {
            Self::Pending => Color::DarkGray,
            Self::Running => Color::Cyan,
            Self::Done => Color::Green,
            Self::Error => Color::Red,
        }
    }
}

/// A single tool execution entry in the task list.
#[derive(Debug, Clone)]
pub struct TaskEntry {
    pub id: String,
    pub tool_name: String,
    pub status: TaskStatus,
    /// Elapsed time in milliseconds (None if not started).
    pub elapsed_ms: Option<u64>,
}

impl TaskEntry {
    pub fn new(id: impl Into<String>, tool_name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            tool_name: tool_name.into(),
            status: TaskStatus::Pending,
            elapsed_ms: None,
        }
    }

    /// Format elapsed time as a human-readable string.
    #[allow(clippy::cast_precision_loss)]
    fn elapsed_str(&self) -> String {
        match self.elapsed_ms {
            None => String::new(),
            Some(ms) if ms < 1000 => format!("{ms}ms"),
            Some(ms) => format!("{:.1}s", ms as f64 / 1000.0),
        }
    }
}

/// Displays a list of tool execution tasks with status indicators.
pub struct TaskListView {
    tasks: Vec<TaskEntry>,
    /// Maximum number of visible tasks (scroll from bottom).
    max_visible: usize,
}

impl TaskListView {
    #[must_use]
    pub fn new() -> Self {
        Self {
            tasks: Vec::new(),
            max_visible: 20,
        }
    }

    /// Add a new pending task.
    pub fn add(&mut self, id: impl Into<String>, tool_name: impl Into<String>) {
        self.tasks.push(TaskEntry::new(id, tool_name));
    }

    /// Update a task's status by ID.
    pub fn set_status(&mut self, id: &str, status: TaskStatus, elapsed_ms: Option<u64>) {
        if let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) {
            task.status = status;
            if elapsed_ms.is_some() {
                task.elapsed_ms = elapsed_ms;
            }
        }
    }

    /// Get a task by ID.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&TaskEntry> {
        self.tasks.iter().find(|t| t.id == id)
    }

    /// Number of tasks.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    /// Whether the list is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// Clear all tasks.
    pub fn clear(&mut self) {
        self.tasks.clear();
    }

    /// Count of tasks with a given status.
    #[must_use]
    pub fn count_by_status(&self, status: TaskStatus) -> usize {
        self.tasks.iter().filter(|t| t.status == status).count()
    }
}

impl Default for TaskListView {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for &TaskListView {
    #[allow(clippy::cast_possible_truncation)]
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width < 10 || self.tasks.is_empty() {
            return;
        }

        let visible = (area.height as usize).min(self.max_visible);
        let skip = self.tasks.len().saturating_sub(visible);

        for (i, task) in self.tasks.iter().skip(skip).take(visible).enumerate() {
            let y = area.y + i as u16;
            if y >= area.y + area.height {
                break;
            }

            let status_style = Style::default().fg(task.status.color()).add_modifier(
                if task.status == TaskStatus::Running {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                },
            );

            let mut spans = vec![
                Span::styled(task.status.symbol(), status_style),
                Span::raw(" "),
                Span::styled(&task.tool_name, Style::default().fg(Color::White)),
            ];

            let elapsed = task.elapsed_str();
            if !elapsed.is_empty() {
                spans.push(Span::styled(
                    format!("  {elapsed}"),
                    Style::default().fg(Color::DarkGray),
                ));
            }

            let line = Line::from(spans);
            let line_area = Rect {
                x: area.x,
                y,
                width: area.width,
                height: 1,
            };
            Widget::render(line, line_area, buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_is_empty() {
        let view = TaskListView::new();
        assert!(view.is_empty());
        assert_eq!(view.len(), 0);
    }

    #[test]
    fn add_tasks() {
        let mut view = TaskListView::new();
        view.add("t1", "bash");
        view.add("t2", "read");
        assert_eq!(view.len(), 2);
        assert!(!view.is_empty());
    }

    #[test]
    fn set_status() {
        let mut view = TaskListView::new();
        view.add("t1", "bash");
        view.set_status("t1", TaskStatus::Running, None);
        assert_eq!(view.get("t1").unwrap().status, TaskStatus::Running);

        view.set_status("t1", TaskStatus::Done, Some(150));
        let task = view.get("t1").unwrap();
        assert_eq!(task.status, TaskStatus::Done);
        assert_eq!(task.elapsed_ms, Some(150));
    }

    #[test]
    fn set_status_nonexistent_is_noop() {
        let mut view = TaskListView::new();
        view.set_status("missing", TaskStatus::Done, None);
        assert!(view.is_empty());
    }

    #[test]
    fn count_by_status() {
        let mut view = TaskListView::new();
        view.add("t1", "bash");
        view.add("t2", "read");
        view.add("t3", "write");
        view.set_status("t1", TaskStatus::Done, Some(100));
        view.set_status("t2", TaskStatus::Running, None);

        assert_eq!(view.count_by_status(TaskStatus::Pending), 1);
        assert_eq!(view.count_by_status(TaskStatus::Running), 1);
        assert_eq!(view.count_by_status(TaskStatus::Done), 1);
        assert_eq!(view.count_by_status(TaskStatus::Error), 0);
    }

    #[test]
    fn clear_removes_all() {
        let mut view = TaskListView::new();
        view.add("t1", "bash");
        view.clear();
        assert!(view.is_empty());
    }

    #[test]
    fn elapsed_formatting() {
        let mut entry = TaskEntry::new("id", "tool");
        assert_eq!(entry.elapsed_str(), "");

        entry.elapsed_ms = Some(50);
        assert_eq!(entry.elapsed_str(), "50ms");

        entry.elapsed_ms = Some(999);
        assert_eq!(entry.elapsed_str(), "999ms");

        entry.elapsed_ms = Some(1500);
        assert_eq!(entry.elapsed_str(), "1.5s");

        entry.elapsed_ms = Some(12_345);
        assert_eq!(entry.elapsed_str(), "12.3s");
    }

    #[test]
    fn status_symbols() {
        assert_eq!(TaskStatus::Pending.symbol(), "○");
        assert_eq!(TaskStatus::Running.symbol(), "◉");
        assert_eq!(TaskStatus::Done.symbol(), "✓");
        assert_eq!(TaskStatus::Error.symbol(), "✗");
    }

    #[test]
    fn status_colors() {
        assert_eq!(TaskStatus::Pending.color(), Color::DarkGray);
        assert_eq!(TaskStatus::Running.color(), Color::Cyan);
        assert_eq!(TaskStatus::Done.color(), Color::Green);
        assert_eq!(TaskStatus::Error.color(), Color::Red);
    }

    #[test]
    fn renders_tasks() {
        let mut view = TaskListView::new();
        view.add("t1", "bash");
        view.add("t2", "read");
        view.set_status("t1", TaskStatus::Done, Some(200));

        let area = Rect::new(0, 0, 40, 5);
        let mut buf = Buffer::empty(area);
        Widget::render(&view, area, &mut buf);

        let row0: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(row0.contains("bash"));

        let row1: String = (0..area.width)
            .map(|x| buf.cell((x, 1)).unwrap().symbol().to_string())
            .collect();
        assert!(row1.contains("read"));
    }

    #[test]
    fn empty_list_does_not_render() {
        let view = TaskListView::new();
        let area = Rect::new(0, 0, 40, 5);
        let mut buf = Buffer::empty(area);
        Widget::render(&view, area, &mut buf);
        // Should be all blank
        let content: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert_eq!(content.trim(), "");
    }

    #[test]
    fn default_is_empty() {
        let view = TaskListView::default();
        assert!(view.is_empty());
    }
}
