//! Foldable tool-call card widget.
//!
//! A "call card" is a compact representation of a single tool call:
//! one header row with the tool name, a status glyph, and a one-line
//! summary, followed by an expandable body containing the full output.
//! Cards are stateless with respect to the tool call itself — the
//! caller tracks status, parameters, and output; the card just paints.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget, Wrap};

use crate::animation::Spinner;
use crate::theme::Theme;

/// Execution status of the tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallCardStatus {
    /// Awaiting user approval.
    PendingPermission,
    /// Running — the spinner is animated.
    Running,
    /// Completed successfully.
    Succeeded,
    /// Completed with an error.
    Failed,
}

impl CallCardStatus {
    /// The glyph painted at the head of the card.
    ///
    /// Running cards return the empty string because the caller paints
    /// a live spinner frame in that slot.
    #[must_use]
    pub fn glyph(&self) -> &'static str {
        match self {
            Self::PendingPermission => "○",
            Self::Running => "",
            Self::Succeeded => "✓",
            Self::Failed => "✗",
        }
    }

    #[must_use]
    pub fn style(&self, theme: &Theme) -> Style {
        match self {
            Self::PendingPermission => Style::default().fg(theme.warning),
            Self::Running => Style::default().fg(theme.accent),
            Self::Succeeded => Style::default().fg(theme.success),
            Self::Failed => Style::default()
                .fg(theme.error)
                .add_modifier(Modifier::BOLD),
        }
    }
}

/// Runtime state of a card — used by the caller to choose fold mode.
#[derive(Debug, Clone)]
pub struct CallCard<'a> {
    pub tool_name: &'a str,
    pub summary: &'a str,
    pub status: CallCardStatus,
    pub body_lines: &'a [Line<'static>],
    pub folded: bool,
    pub theme: &'a Theme,
    pub spinner_glyph: Option<&'static str>,
}

impl CallCard<'_> {
    /// The default fold policy: succeed → folded, everything else → expanded.
    #[must_use]
    pub fn default_folded(status: CallCardStatus) -> bool {
        matches!(status, CallCardStatus::Succeeded)
    }

    /// Draw the header row (always visible).
    fn render_header(&self, rect: Rect, buf: &mut Buffer) {
        if rect.width == 0 || rect.height == 0 {
            return;
        }
        let glyph = match (self.status, self.spinner_glyph) {
            (CallCardStatus::Running, Some(s)) => s.to_string(),
            (CallCardStatus::Running, None) => Spinner::braille()
                .frame_at(std::time::Instant::now())
                .to_string(),
            _ => self.status.glyph().to_string(),
        };
        let name_style = Style::default()
            .fg(self.theme.text_bright)
            .add_modifier(Modifier::BOLD);
        let summary_style = Style::default().fg(self.theme.muted);

        let head = Line::from(vec![
            Span::styled(format!("{glyph} "), self.status.style(self.theme)),
            Span::styled(self.tool_name.to_string(), name_style),
            Span::styled("  ".to_string(), summary_style),
            Span::styled(self.summary.to_string(), summary_style),
        ]);
        Paragraph::new(head)
            .wrap(Wrap { trim: false })
            .render(rect, buf);
    }

    /// Approx height of the card at `width`.
    ///
    /// Folded cards are 1 line; expanded cards take 1 + body line count.
    #[must_use]
    pub fn height(&self, width: u16) -> u16 {
        if self.folded || self.body_lines.is_empty() {
            return 1;
        }
        let body_rows: u16 = self.body_lines.iter().map(|_| 1u16).sum::<u16>().min(width); // never taller than the widget's width-derived cap
        1 + body_rows
    }
}

impl Widget for CallCard<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        let header = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 1,
        };
        self.render_header(header, buf);

        if self.folded || self.body_lines.is_empty() {
            return;
        }
        let body_rect = Rect {
            x: area.x + 2,
            y: area.y + 1,
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(1),
        };
        let body = Paragraph::new(self.body_lines.to_vec())
            .style(Style::default().fg(self.theme.fg))
            .wrap(Wrap { trim: false });
        body.render(body_rect, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_folded_policy_matches_status() {
        assert!(CallCard::default_folded(CallCardStatus::Succeeded));
        assert!(!CallCard::default_folded(CallCardStatus::Failed));
        assert!(!CallCard::default_folded(CallCardStatus::Running));
        assert!(!CallCard::default_folded(CallCardStatus::PendingPermission));
    }

    #[test]
    fn folded_card_reports_height_one() {
        let theme = Theme::dark();
        let card = CallCard {
            tool_name: "read",
            summary: "src/lib.rs",
            status: CallCardStatus::Succeeded,
            body_lines: &[Line::raw("content")],
            folded: true,
            theme: &theme,
            spinner_glyph: None,
        };
        assert_eq!(card.height(80), 1);
    }

    #[test]
    fn expanded_card_reports_header_plus_body() {
        let theme = Theme::dark();
        let body: Vec<Line<'static>> = vec![Line::raw("a"), Line::raw("b"), Line::raw("c")];
        let card = CallCard {
            tool_name: "bash",
            summary: "ls /tmp",
            status: CallCardStatus::Failed,
            body_lines: &body,
            folded: false,
            theme: &theme,
            spinner_glyph: None,
        };
        assert_eq!(card.height(80), 4);
    }

    #[test]
    fn glyph_is_empty_for_running_status() {
        assert!(CallCardStatus::Running.glyph().is_empty());
        assert!(!CallCardStatus::Succeeded.glyph().is_empty());
    }
}
