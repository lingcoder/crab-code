use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::Theme;

/// Renders a unified diff into styled ratatui `Line`s.
///
/// Lines starting with `+` are colored with the add style, `-` with the
/// remove style, `@@` with the hunk header style, and everything else
/// uses the default foreground.
pub struct DiffView<'t> {
    theme: &'t Theme,
}

impl<'t> DiffView<'t> {
    #[must_use]
    pub fn new(theme: &'t Theme) -> Self {
        Self { theme }
    }

    /// Render a unified diff string into styled `Line`s.
    pub fn render(&self, diff: &str) -> Vec<Line<'static>> {
        diff.lines().map(|line| self.render_line(line)).collect()
    }

    /// Render a single diff line with appropriate styling.
    fn render_line(&self, line: &str) -> Line<'static> {
        let owned = line.to_string();

        if line.starts_with("+++") || line.starts_with("---") {
            // File header lines
            let style = Style::default()
                .fg(self.theme.fg)
                .add_modifier(Modifier::BOLD);
            Line::from(Span::styled(owned, style))
        } else if line.starts_with('+') {
            let style = Style::default()
                .fg(self.theme.diff_add_fg)
                .bg(self.theme.diff_add_bg);
            Line::from(Span::styled(owned, style))
        } else if line.starts_with('-') {
            let style = Style::default()
                .fg(self.theme.diff_remove_fg)
                .bg(self.theme.diff_remove_bg);
            Line::from(Span::styled(owned, style))
        } else if line.starts_with("@@") {
            let style = Style::default()
                .fg(self.theme.diff_hunk)
                .add_modifier(Modifier::BOLD);
            Line::from(Span::styled(owned, style))
        } else {
            // Context lines
            let style = Style::default().fg(self.theme.muted);
            Line::from(Span::styled(owned, style))
        }
    }

    /// Render a side-by-side summary: file path with stats.
    ///
    /// Returns a single `Line` like: `path/to/file.rs  +12 -3`
    pub fn render_stat_line(
        &self,
        file_path: &str,
        additions: usize,
        deletions: usize,
    ) -> Line<'static> {
        let mut spans = vec![Span::styled(
            format!("{file_path}  "),
            Style::default().fg(self.theme.fg),
        )];

        if additions > 0 {
            spans.push(Span::styled(
                format!("+{additions}"),
                Style::default().fg(self.theme.diff_add_fg),
            ));
            spans.push(Span::raw(" ".to_string()));
        }

        if deletions > 0 {
            spans.push(Span::styled(
                format!("-{deletions}"),
                Style::default().fg(self.theme.diff_remove_fg),
            ));
        }

        Line::from(spans)
    }
}

/// Count additions and deletions in a unified diff string.
///
/// Returns `(additions, deletions)`. File header lines (`+++`/`---`)
/// are excluded from the count.
#[must_use]
pub fn count_diff_stats(diff: &str) -> (usize, usize) {
    let mut adds = 0;
    let mut dels = 0;
    for line in diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            adds += 1;
        } else if line.starts_with('-') {
            dels += 1;
        }
    }
    (adds, dels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    const SAMPLE_DIFF: &str = "\
--- a/file.rs
+++ b/file.rs
@@ -1,3 +1,4 @@
 fn main() {
-    println!(\"old\");
+    println!(\"new\");
+    println!(\"extra\");
 }";

    #[test]
    fn render_diff_line_count() {
        let theme = Theme::dark();
        let view = DiffView::new(&theme);
        let lines = view.render(SAMPLE_DIFF);
        assert_eq!(lines.len(), 8);
    }

    #[test]
    fn added_lines_use_green() {
        let theme = Theme::dark();
        let view = DiffView::new(&theme);
        let lines = view.render(SAMPLE_DIFF);
        // Lines starting with '+' (not +++) should have diff_add_fg
        let add_line = &lines[5]; // +    println!("new");
        let span = &add_line.spans[0];
        assert_eq!(span.style.fg, Some(Color::Green));
    }

    #[test]
    fn removed_lines_use_red() {
        let theme = Theme::dark();
        let view = DiffView::new(&theme);
        let lines = view.render(SAMPLE_DIFF);
        let del_line = &lines[4]; // -    println!("old");
        let span = &del_line.spans[0];
        assert_eq!(span.style.fg, Some(Color::Red));
    }

    #[test]
    fn hunk_header_uses_cyan() {
        let theme = Theme::dark();
        let view = DiffView::new(&theme);
        let lines = view.render(SAMPLE_DIFF);
        let hunk = &lines[2]; // @@ -1,3 +1,4 @@
        let span = &hunk.spans[0];
        assert_eq!(span.style.fg, Some(Color::Cyan));
    }

    #[test]
    fn file_headers_are_bold() {
        let theme = Theme::dark();
        let view = DiffView::new(&theme);
        let lines = view.render(SAMPLE_DIFF);
        let header = &lines[0]; // --- a/file.rs
        assert!(header.spans[0].style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn context_lines_use_muted() {
        let theme = Theme::dark();
        let view = DiffView::new(&theme);
        let lines = view.render(SAMPLE_DIFF);
        let ctx = &lines[3]; // " fn main() {"
        assert_eq!(ctx.spans[0].style.fg, Some(Color::DarkGray));
    }

    #[test]
    fn count_diff_stats_correct() {
        let (adds, dels) = count_diff_stats(SAMPLE_DIFF);
        assert_eq!(adds, 2);
        assert_eq!(dels, 1);
    }

    #[test]
    fn count_diff_stats_empty() {
        let (adds, dels) = count_diff_stats("");
        assert_eq!(adds, 0);
        assert_eq!(dels, 0);
    }

    #[test]
    fn render_stat_line() {
        let theme = Theme::dark();
        let view = DiffView::new(&theme);
        let line = view.render_stat_line("src/main.rs", 10, 3);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("src/main.rs"));
        assert!(text.contains("+10"));
        assert!(text.contains("-3"));
    }

    #[test]
    fn render_stat_line_no_deletions() {
        let theme = Theme::dark();
        let view = DiffView::new(&theme);
        let line = view.render_stat_line("new.rs", 5, 0);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("+5"));
        assert!(!text.contains("-0"));
    }

    #[test]
    fn empty_diff_produces_no_lines() {
        let theme = Theme::dark();
        let view = DiffView::new(&theme);
        let lines = view.render("");
        assert!(lines.is_empty());
    }
}
