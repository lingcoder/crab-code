//! GFM table rendering helpers.
//!
//! The existing [`crate::components::markdown::MarkdownRenderer`] flattens
//! tables into simple pipe-delimited lines. This module exposes a
//! column-aware alternative that pads cells to align visually.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::Theme;

/// A parsed table row: header row + data rows share the same shape.
#[derive(Debug, Clone)]
pub struct TableRow {
    pub cells: Vec<String>,
}

impl TableRow {
    #[must_use]
    pub fn new(cells: Vec<String>) -> Self {
        Self { cells }
    }
}

/// Render a GFM table with aligned columns.
///
/// Emits one `Line` per rendered row (header, separator, body rows).
/// Column widths are padded to fit the widest cell across the whole
/// table so entries line up visually.
#[must_use]
pub fn render_gfm_table(header: &TableRow, body: &[TableRow], theme: &Theme) -> Vec<Line<'static>> {
    let col_count = header.cells.len();
    if col_count == 0 {
        return Vec::new();
    }
    let mut widths: Vec<usize> = header.cells.iter().map(|c| display_width(c)).collect();
    for row in body {
        for (i, cell) in row.cells.iter().enumerate() {
            if i >= widths.len() {
                widths.push(display_width(cell));
            } else {
                widths[i] = widths[i].max(display_width(cell));
            }
        }
    }

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(body.len() + 2);
    lines.push(format_row(&header.cells, &widths, theme, true));
    lines.push(format_separator(&widths, theme));
    for row in body {
        lines.push(format_row(&row.cells, &widths, theme, false));
    }
    lines
}

fn format_row(cells: &[String], widths: &[usize], theme: &Theme, header: bool) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(cells.len() * 2 + 1);
    let border = Style::default().fg(theme.border);
    let text_style = if header {
        Style::default()
            .fg(theme.heading)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.fg)
    };

    spans.push(Span::styled("│ ", border));
    for (i, width) in widths.iter().enumerate() {
        let empty = String::new();
        let content = cells.get(i).unwrap_or(&empty);
        let padded = pad_right(content, *width);
        spans.push(Span::styled(padded, text_style));
        spans.push(Span::styled(" │ ", border));
    }
    Line::from(spans)
}

fn format_separator(widths: &[usize], theme: &Theme) -> Line<'static> {
    let border = Style::default().fg(theme.border);
    let mut s = String::new();
    s.push('├');
    for (i, w) in widths.iter().enumerate() {
        if i > 0 {
            s.push('┼');
        }
        for _ in 0..(w + 2) {
            s.push('─');
        }
    }
    s.push('┤');
    Line::from(Span::styled(s, border))
}

/// Compute the minimum width (in characters) a horizontal table would need.
///
/// This accounts for cell content + column borders (`│ ` padding per col).
#[must_use]
pub fn compute_min_table_width(header: &TableRow, body: &[TableRow]) -> usize {
    let col_count = header.cells.len();
    if col_count == 0 {
        return 0;
    }
    let mut widths: Vec<usize> = header.cells.iter().map(|c| display_width(c)).collect();
    for row in body {
        for (i, cell) in row.cells.iter().enumerate() {
            if i >= widths.len() {
                widths.push(display_width(cell));
            } else {
                widths[i] = widths[i].max(display_width(cell));
            }
        }
    }
    // Each column: content_width + 3 (` │ `) except the last gets 2 after + leading `│ `
    // Formula: 2 (leading "│ ") + sum(width + 3 for each col)
    2 + widths.iter().map(|w| w + 3).sum::<usize>()
}

/// Render a table in vertical (key: value) format for narrow terminals.
///
/// Each row is rendered as a block of `header: value` pairs separated by
/// blank lines. Used when the terminal is too narrow for horizontal layout.
#[must_use]
pub fn render_vertical_table(
    header: &TableRow,
    body: &[TableRow],
    theme: &Theme,
) -> Vec<Line<'static>> {
    let label_style = Style::default()
        .fg(theme.heading)
        .add_modifier(Modifier::BOLD);
    let value_style = Style::default().fg(theme.fg);
    let sep_style = Style::default().fg(theme.border);

    let mut lines: Vec<Line<'static>> = Vec::new();

    for (row_idx, row) in body.iter().enumerate() {
        if row_idx > 0 {
            lines.push(Line::from(Span::styled("───", sep_style)));
        }
        for (col_idx, cell) in row.cells.iter().enumerate() {
            let label = header
                .cells
                .get(col_idx)
                .map_or("?", |s| s.as_str());
            lines.push(Line::from(vec![
                Span::styled(format!("{label}: "), label_style),
                Span::styled(cell.clone(), value_style),
            ]));
        }
    }
    lines
}

fn display_width(s: &str) -> usize {
    s.chars().count()
}

fn pad_right(s: &str, target: usize) -> String {
    let len = display_width(s);
    if len >= target {
        s.to_string()
    } else {
        let mut out = String::with_capacity(s.len() + (target - len));
        out.push_str(s);
        for _ in 0..(target - len) {
            out.push(' ');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_header_separator_body() {
        let theme = Theme::dark();
        let header = TableRow::new(vec!["Col A".into(), "Col B".into()]);
        let body = [
            TableRow::new(vec!["x".into(), "yy".into()]),
            TableRow::new(vec!["aaa".into(), "b".into()]),
        ];
        let lines = render_gfm_table(&header, &body, &theme);
        assert_eq!(lines.len(), 4); // header + sep + 2 body
    }

    #[test]
    fn empty_header_returns_empty() {
        let theme = Theme::dark();
        let header = TableRow::new(Vec::new());
        let out = render_gfm_table(&header, &[], &theme);
        assert!(out.is_empty());
    }

    #[test]
    fn column_widths_accommodate_wider_body_cells() {
        let theme = Theme::dark();
        let header = TableRow::new(vec!["a".into()]);
        let body = [TableRow::new(vec!["very long cell".into()])];
        let lines = render_gfm_table(&header, &body, &theme);
        let header_line = lines.first().expect("lines");
        let rendered: String = header_line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(rendered.contains("a             "));
    }

    #[test]
    fn compute_min_width_accounts_for_borders() {
        let header = TableRow::new(vec!["AB".into(), "CD".into()]);
        let body = [TableRow::new(vec!["x".into(), "y".into()])];
        let w = compute_min_table_width(&header, &body);
        // 2 (leading "│ ") + (2+3) + (2+3) = 12
        assert_eq!(w, 12);
    }

    #[test]
    fn compute_min_width_empty() {
        let header = TableRow::new(Vec::new());
        assert_eq!(compute_min_table_width(&header, &[]), 0);
    }

    #[test]
    fn vertical_table_renders_key_value_pairs() {
        let theme = Theme::dark();
        let header = TableRow::new(vec!["Name".into(), "Age".into()]);
        let body = [
            TableRow::new(vec!["Alice".into(), "30".into()]),
            TableRow::new(vec!["Bob".into(), "25".into()]),
        ];
        let lines = render_vertical_table(&header, &body, &theme);
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains("Name: "));
        assert!(text.contains("Alice"));
        assert!(text.contains("Age: "));
        assert!(text.contains("30"));
    }

    #[test]
    fn vertical_table_empty_body() {
        let theme = Theme::dark();
        let header = TableRow::new(vec!["X".into()]);
        let lines = render_vertical_table(&header, &[], &theme);
        assert!(lines.is_empty());
    }
}
