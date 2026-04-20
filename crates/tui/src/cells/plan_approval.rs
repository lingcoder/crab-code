use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

#[derive(Debug, Clone)]
pub struct PlanApproval {
    pub plan_text: String,
    pub selected_option: usize,
}

impl PlanApproval {
    #[must_use]
    pub fn new(plan_text: impl Into<String>) -> Self {
        Self {
            plan_text: plan_text.into(),
            selected_option: 0,
        }
    }

    pub fn render_lines(&self, _width: u16) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();

        for line in self.plan_text.lines() {
            lines.push(Line::from(Span::styled(
                format!("  {line}"),
                Style::default().fg(Color::White),
            )));
        }

        lines.push(Line::default());

        let options = ["Approve", "Reject", "Edit"];
        let mut spans = Vec::new();
        for (i, opt) in options.iter().enumerate() {
            let style = if i == self.selected_option {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            spans.push(Span::styled(format!(" [{opt}] "), style));
        }
        lines.push(Line::from(spans));

        lines
    }

    pub fn desired_height(&self, width: u16) -> u16 {
        self.render_lines(width).len() as u16
    }

    #[must_use]
    pub fn is_streaming(&self) -> bool {
        false
    }

    pub fn search_text(&self) -> String {
        self.plan_text.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_plan_and_buttons() {
        let plan = PlanApproval::new("Step 1: do this\nStep 2: do that");
        let lines = plan.render_lines(80);
        assert!(lines.len() >= 4);
        let last: String = lines
            .last()
            .unwrap()
            .spans
            .iter()
            .map(|s| &*s.content)
            .collect();
        assert!(last.contains("Approve"));
        assert!(last.contains("Reject"));
        assert!(last.contains("Edit"));
    }

    #[test]
    fn selected_option_highlighted() {
        let mut plan = PlanApproval::new("plan");
        plan.selected_option = 1;
        let lines = plan.render_lines(80);
        let buttons = lines.last().unwrap();
        assert_eq!(buttons.spans[1].style.bg, Some(Color::White));
    }
}
