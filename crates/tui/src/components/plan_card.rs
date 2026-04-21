//! Plan step checklist — renders plan steps with status glyphs and a progress bar.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanStepStatus {
    Pending,
    InProgress,
    Done,
    Skipped,
}

impl PlanStepStatus {
    #[must_use]
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Pending => "[ ]",
            Self::InProgress => "[~]",
            Self::Done => "[x]",
            Self::Skipped => "[-]",
        }
    }

    #[must_use]
    pub fn color(self) -> Color {
        match self {
            Self::InProgress => Color::Yellow,
            Self::Done => Color::Green,
            Self::Pending | Self::Skipped => Color::DarkGray,
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

#[allow(clippy::cast_possible_truncation)]
#[must_use]
pub fn render_progress_bar(done: usize, total: usize, width: usize) -> Line<'static> {
    if total == 0 {
        return Line::from(Span::styled(
            "[no steps]",
            Style::default().fg(Color::DarkGray),
        ));
    }

    let bar_width = width.saturating_sub(8); // " [====----] N/M"
    let filled = (done * bar_width).checked_div(total).unwrap_or(0);
    let empty = bar_width.saturating_sub(filled);

    let pct_color = if done == total {
        Color::Green
    } else if done > 0 {
        Color::Yellow
    } else {
        Color::DarkGray
    };

    Line::from(vec![
        Span::styled(" [", Style::default().fg(Color::DarkGray)),
        Span::styled("=".repeat(filled), Style::default().fg(pct_color)),
        Span::styled("-".repeat(empty), Style::default().fg(Color::DarkGray)),
        Span::styled("] ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{done}/{total}"),
            Style::default().fg(pct_color).add_modifier(Modifier::BOLD),
        ),
    ])
}

#[must_use]
pub fn render_step(title: &str, status: PlanStepStatus) -> Line<'static> {
    let glyph = status.glyph();
    let color = status.color();

    let text_style = match status {
        PlanStepStatus::Done => Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::DIM),
        PlanStepStatus::Skipped => Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
        PlanStepStatus::InProgress => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
        PlanStepStatus::Pending => Style::default().fg(Color::White),
    };

    Line::from(vec![
        Span::styled(format!("  {glyph} "), Style::default().fg(color)),
        Span::styled(title.to_string(), text_style),
    ])
}

#[must_use]
pub fn render_plan_checklist(
    title: &str,
    steps: &[(String, PlanStepStatus)],
    width: usize,
    awaiting_approval: bool,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    let title_line = Line::from(Span::styled(
        format!("  {title}"),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ));
    lines.push(title_line);

    let done_count = steps
        .iter()
        .filter(|(_, s)| *s == PlanStepStatus::Done)
        .count();
    lines.push(render_progress_bar(done_count, steps.len(), width));

    lines.push(Line::default());

    for (step_title, status) in steps {
        lines.push(render_step(step_title, *status));
    }

    if awaiting_approval {
        lines.push(Line::default());
        lines.push(Line::from(vec![
            Span::styled("  (", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "y",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(") Approve  (", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "n",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::styled(") Reject", Style::default().fg(Color::DarkGray)),
        ]));
    }

    lines
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_glyphs() {
        assert_eq!(PlanStepStatus::Pending.glyph(), "[ ]");
        assert_eq!(PlanStepStatus::InProgress.glyph(), "[~]");
        assert_eq!(PlanStepStatus::Done.glyph(), "[x]");
        assert_eq!(PlanStepStatus::Skipped.glyph(), "[-]");
    }

    #[test]
    fn status_colors_distinct() {
        assert_ne!(
            PlanStepStatus::Pending.color(),
            PlanStepStatus::InProgress.color()
        );
        assert_ne!(
            PlanStepStatus::InProgress.color(),
            PlanStepStatus::Done.color()
        );
    }

    #[test]
    fn progress_bar_empty() {
        let line = render_progress_bar(0, 0, 40);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("no steps"));
    }

    #[test]
    fn progress_bar_partial() {
        let line = render_progress_bar(3, 8, 40);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("3/8"));
        assert!(text.contains('='));
        assert!(text.contains('-'));
    }

    #[test]
    fn progress_bar_complete() {
        let line = render_progress_bar(5, 5, 40);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("5/5"));
    }

    #[test]
    fn render_step_done() {
        let line = render_step("Setup environment", PlanStepStatus::Done);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("[x]"));
        assert!(text.contains("Setup environment"));
    }

    #[test]
    fn render_step_in_progress() {
        let line = render_step("Build server", PlanStepStatus::InProgress);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("[~]"));
    }

    #[test]
    fn checklist_renders_all_steps() {
        let steps = vec![
            ("Step 1".into(), PlanStepStatus::Done),
            ("Step 2".into(), PlanStepStatus::InProgress),
            ("Step 3".into(), PlanStepStatus::Pending),
        ];
        let lines = render_plan_checklist("My Plan", &steps, 60, false);
        assert!(lines.len() >= 6); // title + progress + blank + 3 steps
    }

    #[test]
    fn checklist_with_approval() {
        let steps = vec![("Step 1".into(), PlanStepStatus::Pending)];
        let lines = render_plan_checklist("Plan", &steps, 60, true);
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(all_text.contains("Approve"));
        assert!(all_text.contains("Reject"));
    }

    #[test]
    fn checklist_without_approval() {
        let steps = vec![("Step 1".into(), PlanStepStatus::Done)];
        let lines = render_plan_checklist("Plan", &steps, 60, false);
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(!all_text.contains("Approve"));
    }

    #[test]
    fn checklist_empty_steps() {
        let lines = render_plan_checklist("Empty", &[], 60, false);
        assert!(lines.len() >= 2); // title + progress bar
    }
}
