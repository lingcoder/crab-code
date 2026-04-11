//! Status line component — model, tokens, cost, permission mode, thinking state.
//!
//! Matches CC's `StatusLine` component showing operational data.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::app::ThinkingState;
use crate::traits::Renderable;

/// Complete status line — model, tokens, cost, permission mode.
pub struct StatusLine<'a> {
    pub model: &'a str,
    pub permission_mode: crab_core::permission::PermissionMode,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub thinking: &'a ThinkingState,
    pub context_pct: Option<f32>,
    pub cost_usd: Option<f64>,
}

impl Renderable for StatusLine<'_> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.width < 10 || area.height == 0 {
            return;
        }

        let mut spans = vec![
            Span::styled(self.model, Style::default().fg(Color::Cyan)),
            Span::styled(" │ ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                self.permission_mode.to_string(),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled(" │ ", Style::default().fg(Color::DarkGray)),
        ];

        // Token counts
        let in_str = format_token_count(self.input_tokens);
        let out_str = format_token_count(self.output_tokens);
        spans.push(Span::styled(
            format!("{in_str} in"),
            Style::default().fg(Color::DarkGray),
        ));
        spans.push(Span::styled(" · ", Style::default().fg(Color::DarkGray)));
        spans.push(Span::styled(
            format!("{out_str} out"),
            Style::default().fg(Color::DarkGray),
        ));

        // Context percentage
        if let Some(pct) = self.context_pct {
            spans.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
            let color = if pct > 0.9 {
                Color::Red
            } else if pct > 0.7 {
                Color::Yellow
            } else {
                Color::DarkGray
            };
            spans.push(Span::styled(
                format!("{:.0}% ctx", pct * 100.0),
                Style::default().fg(color),
            ));
        }

        // Cost
        if let Some(cost) = self.cost_usd {
            spans.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
            spans.push(Span::styled(
                format!("${cost:.4}"),
                Style::default().fg(Color::DarkGray),
            ));
        }

        // Thinking state
        match self.thinking {
            ThinkingState::Thinking { started_at } => {
                let elapsed = started_at.elapsed().as_secs();
                spans.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
                spans.push(Span::styled(
                    format!("thinking ({elapsed}s)"),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::ITALIC),
                ));
            }
            ThinkingState::ThoughtFor { duration, .. } => {
                spans.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
                spans.push(Span::styled(
                    format!("thought for {}s", duration.as_secs()),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            ThinkingState::Idle => {}
        }

        Widget::render(Line::from(spans), area, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        1
    }
}

/// Format token count: 1234 → "1.2k", 500 → "500"
fn format_token_count(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1000 {
        format!("{:.1}k", tokens as f64 / 1000.0)
    } else {
        tokens.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_line_desired_height() {
        let sl = StatusLine {
            model: "gpt-4o",
            permission_mode: crab_core::permission::PermissionMode::Default,
            input_tokens: 1000,
            output_tokens: 500,
            thinking: &ThinkingState::Idle,
            context_pct: None,
            cost_usd: None,
        };
        assert_eq!(sl.desired_height(80), 1);
    }

    #[test]
    fn status_line_render_does_not_panic() {
        let sl = StatusLine {
            model: "claude-sonnet-4-6",
            permission_mode: crab_core::permission::PermissionMode::AcceptEdits,
            input_tokens: 12345,
            output_tokens: 6789,
            thinking: &ThinkingState::Idle,
            context_pct: Some(0.42),
            cost_usd: Some(0.0123),
        };
        let area = Rect::new(0, 0, 120, 1);
        let mut buf = Buffer::empty(area);
        sl.render(area, &mut buf);
    }

    #[test]
    fn format_token_count_works() {
        assert_eq!(format_token_count(0), "0");
        assert_eq!(format_token_count(500), "500");
        assert_eq!(format_token_count(1234), "1.2k");
        assert_eq!(format_token_count(1_500_000), "1.5M");
    }
}
