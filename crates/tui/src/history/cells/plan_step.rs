//! Plan step cell — renders a plan checklist in the transcript.

use ratatui::text::Line;

use crate::components::plan_card::{PlanStepStatus, render_plan_checklist};
use crate::history::HistoryCell;

#[derive(Debug, Clone)]
pub struct PlanStepCell {
    title: String,
    steps: Vec<(String, PlanStepStatus)>,
    awaiting_approval: bool,
}

impl PlanStepCell {
    #[must_use]
    pub fn new(
        title: String,
        steps: Vec<(String, PlanStepStatus)>,
        awaiting_approval: bool,
    ) -> Self {
        Self {
            title,
            steps,
            awaiting_approval,
        }
    }
}

impl HistoryCell for PlanStepCell {
    #[allow(clippy::cast_possible_truncation)]
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        render_plan_checklist(
            &self.title,
            &self.steps,
            width as usize,
            self.awaiting_approval,
        )
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_step_cell_renders() {
        let cell = PlanStepCell::new(
            "Test Plan".into(),
            vec![
                ("Step 1".into(), PlanStepStatus::Done),
                ("Step 2".into(), PlanStepStatus::InProgress),
            ],
            false,
        );
        let lines = cell.display_lines(80);
        assert!(lines.len() >= 4);
    }

    #[test]
    fn plan_step_cell_with_approval() {
        let cell = PlanStepCell::new(
            "Plan".into(),
            vec![("Step 1".into(), PlanStepStatus::Pending)],
            true,
        );
        let lines = cell.display_lines(80);
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(all_text.contains("Approve"));
    }
}
