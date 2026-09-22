use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::history::HistoryCell;

const LOGO_ART: &[&str] = &[
    r"  /\_/\  ",
    r" ( o.o ) ",
    r"  > ^ <  ",
    r" /|   |\ ",
    r"(_|___|_)",
];

#[derive(Debug, Clone)]
pub struct WelcomeCell {
    version: String,
    whats_new: String,
    show_project_hint: bool,
    model: String,
    working_dir: String,
}

impl WelcomeCell {
    #[must_use]
    pub fn new(
        version: String,
        whats_new: String,
        show_project_hint: bool,
        model: String,
        working_dir: String,
    ) -> Self {
        Self {
            version,
            whats_new,
            show_project_hint,
            model,
            working_dir,
        }
    }
}

impl HistoryCell for WelcomeCell {
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        let art_style = Style::default().fg(Color::Cyan);
        let mut out: Vec<Line<'static>> = LOGO_ART
            .iter()
            .map(|row| Line::from(Span::styled(row.to_string(), art_style)))
            .collect();

        out.push(Line::from(vec![
            Span::styled(
                "Crab Code ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("v{}", self.version),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ),
        ]));
        // Show model + working dir on one dim line so the welcome cell is
        // self-contained (replaces the persistent HeaderBar that used to
        // hold this info forever-visible at the top).
        if !self.model.is_empty() || !self.working_dir.is_empty() {
            let separator = if !self.model.is_empty() && !self.working_dir.is_empty() {
                " \u{00b7} "
            } else {
                ""
            };
            let combined = format!("{}{}{}", self.model, separator, self.working_dir);
            out.push(Line::from(Span::styled(
                combined,
                Style::default().fg(Color::DarkGray),
            )));
        }
        if !self.whats_new.is_empty() {
            out.push(Line::from(vec![
                Span::styled(
                    "\u{2726} What's new: ",
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    self.whats_new.clone(),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                ),
            ]));
        }
        if self.show_project_hint {
            out.push(Line::from(Span::styled(
                "No AGENTS.md yet \u{00b7} run /init to create one",
                Style::default().fg(Color::DarkGray),
            )));
        }
        out.push(Line::default());
        out
    }

    fn transcript_lines(&self, _width: u16) -> Vec<Line<'static>> {
        Vec::new()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flatten(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn banner_only_when_no_extras() {
        let cell = WelcomeCell::new(
            "0.1.0".into(),
            String::new(),
            false,
            String::new(),
            String::new(),
        );
        let lines = cell.display_lines(120);
        // 5 art rows + 1 title + 1 blank
        assert_eq!(lines.len(), 7);
        let text = flatten(&lines);
        assert!(text.contains("Crab Code v0.1.0"));
    }

    #[test]
    fn includes_whats_new_when_present() {
        let cell = WelcomeCell::new(
            "0.1.0".into(),
            "shiny new thing".into(),
            false,
            String::new(),
            String::new(),
        );
        let lines = cell.display_lines(120);
        // 5 art rows + 1 title + 1 whats_new + 1 blank
        assert_eq!(lines.len(), 8);
        let text = flatten(&lines);
        assert!(text.contains("Crab Code v0.1.0"));
        assert!(text.contains("shiny new thing"));
    }

    #[test]
    fn includes_project_hint_when_enabled() {
        let cell = WelcomeCell::new(
            "0.1.0".into(),
            String::new(),
            true,
            String::new(),
            String::new(),
        );
        let lines = cell.display_lines(120);
        // 5 art rows + 1 title + 1 hint + 1 blank
        assert_eq!(lines.len(), 8);
        let text = flatten(&lines);
        assert!(text.contains("No AGENTS.md yet"));
    }

    #[test]
    fn includes_all_when_both_set() {
        let cell = WelcomeCell::new(
            "0.1.0".into(),
            "release notes".into(),
            true,
            String::new(),
            String::new(),
        );
        let lines = cell.display_lines(120);
        // 5 art rows + 1 title + 1 whats_new + 1 hint + 1 blank
        assert_eq!(lines.len(), 9);
        let text = flatten(&lines);
        assert!(text.contains("Crab Code v0.1.0"));
        assert!(text.contains("release notes"));
        assert!(text.contains("No AGENTS.md yet"));
    }

    #[test]
    fn no_emoji_in_banner() {
        let cell = WelcomeCell::new(
            "0.1.0".into(),
            String::new(),
            false,
            String::new(),
            String::new(),
        );
        let text = flatten(&cell.display_lines(120));
        assert!(!text.contains('✦'));
        assert!(!text.contains('🦀'));
    }

    #[test]
    fn transcript_lines_are_empty() {
        let cell = WelcomeCell::new(
            "0.1.0".into(),
            "x".into(),
            true,
            String::new(),
            String::new(),
        );
        assert!(cell.transcript_lines(120).is_empty());
    }
}
