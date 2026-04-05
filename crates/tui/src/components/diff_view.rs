//! Split-pane diff view with inline character-level highlighting and
//! hunk navigation.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;
use similar::{ChangeTag, TextDiff};

use crate::theme::Theme;

// ─── Change types ───────────────────────────────────────────────────────

/// Kind of inline change at character level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineChangeKind {
    Equal,
    Delete,
    Insert,
}

/// A single inline (character-level) change fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineFragment {
    pub kind: InlineChangeKind,
    pub text: String,
}

/// A line-level diff entry for the split view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffLine {
    /// Unchanged line present on both sides.
    Context {
        line_num_old: usize,
        line_num_new: usize,
        text: String,
    },
    /// Line removed from the old side.
    Removed {
        line_num_old: usize,
        text: String,
    },
    /// Line added on the new side.
    Added {
        line_num_new: usize,
        text: String,
    },
    /// A modified line pair — old and new with inline (char-level) diff fragments.
    Modified {
        line_num_old: usize,
        line_num_new: usize,
        old_fragments: Vec<InlineFragment>,
        new_fragments: Vec<InlineFragment>,
    },
}

/// A hunk is a contiguous group of changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    /// Index of the first `DiffLine` in this hunk.
    pub start_index: usize,
    /// Number of `DiffLine`s in this hunk.
    pub length: usize,
}

// ─── Diff computation ───────────────────────────────────────────────────

/// Compute line-level diff entries from two text inputs.
/// Adjacent delete+insert pairs are merged into `Modified` with character-level fragments.
#[must_use]
pub fn compute_diff_lines(old: &str, new: &str) -> Vec<DiffLine> {
    let diff = TextDiff::from_lines(old, new);
    let mut raw: Vec<(ChangeTag, String, Option<usize>, Option<usize>)> = Vec::new();

    let mut old_num = 0usize;
    let mut new_num = 0usize;

    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Equal => {
                old_num += 1;
                new_num += 1;
                raw.push((
                    ChangeTag::Equal,
                    strip_newline(change.value()),
                    Some(old_num),
                    Some(new_num),
                ));
            }
            ChangeTag::Delete => {
                old_num += 1;
                raw.push((
                    ChangeTag::Delete,
                    strip_newline(change.value()),
                    Some(old_num),
                    None,
                ));
            }
            ChangeTag::Insert => {
                new_num += 1;
                raw.push((
                    ChangeTag::Insert,
                    strip_newline(change.value()),
                    None,
                    Some(new_num),
                ));
            }
        }
    }

    // Merge adjacent Delete+Insert into Modified
    let mut result = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let (tag, text, old_ln, new_ln) = &raw[i];
        match tag {
            ChangeTag::Equal => {
                result.push(DiffLine::Context {
                    line_num_old: old_ln.unwrap(),
                    line_num_new: new_ln.unwrap(),
                    text: text.clone(),
                });
                i += 1;
            }
            ChangeTag::Delete => {
                // Check if next is Insert — if so, merge into Modified
                if i + 1 < raw.len() && raw[i + 1].0 == ChangeTag::Insert {
                    let old_text = text;
                    let new_text = &raw[i + 1].1;
                    let (old_frags, new_frags) = compute_inline_diff(old_text, new_text);
                    result.push(DiffLine::Modified {
                        line_num_old: old_ln.unwrap(),
                        line_num_new: raw[i + 1].3.unwrap(),
                        old_fragments: old_frags,
                        new_fragments: new_frags,
                    });
                    i += 2;
                } else {
                    result.push(DiffLine::Removed {
                        line_num_old: old_ln.unwrap(),
                        text: text.clone(),
                    });
                    i += 1;
                }
            }
            ChangeTag::Insert => {
                result.push(DiffLine::Added {
                    line_num_new: new_ln.unwrap(),
                    text: text.clone(),
                });
                i += 1;
            }
        }
    }

    result
}

/// Compute character-level inline diff between two strings.
/// Returns (old_fragments, new_fragments).
#[must_use]
pub fn compute_inline_diff(
    old: &str,
    new: &str,
) -> (Vec<InlineFragment>, Vec<InlineFragment>) {
    let diff = TextDiff::from_chars(old, new);
    let mut old_frags = Vec::new();
    let mut new_frags = Vec::new();

    for change in diff.iter_all_changes() {
        let text = change.value().to_string();
        match change.tag() {
            ChangeTag::Equal => {
                old_frags.push(InlineFragment {
                    kind: InlineChangeKind::Equal,
                    text: text.clone(),
                });
                new_frags.push(InlineFragment {
                    kind: InlineChangeKind::Equal,
                    text,
                });
            }
            ChangeTag::Delete => {
                old_frags.push(InlineFragment {
                    kind: InlineChangeKind::Delete,
                    text,
                });
            }
            ChangeTag::Insert => {
                new_frags.push(InlineFragment {
                    kind: InlineChangeKind::Insert,
                    text,
                });
            }
        }
    }

    // Coalesce adjacent fragments of same kind
    (coalesce_fragments(old_frags), coalesce_fragments(new_frags))
}

fn coalesce_fragments(frags: Vec<InlineFragment>) -> Vec<InlineFragment> {
    let mut result: Vec<InlineFragment> = Vec::new();
    for f in frags {
        if let Some(last) = result.last_mut() {
            if last.kind == f.kind {
                last.text.push_str(&f.text);
                continue;
            }
        }
        result.push(f);
    }
    result
}

fn strip_newline(s: &str) -> String {
    s.strip_suffix('\n')
        .or_else(|| s.strip_suffix("\r\n"))
        .unwrap_or(s)
        .to_string()
}

/// Detect hunk boundaries in diff lines.
/// A hunk is a maximal contiguous run of non-Context lines.
#[must_use]
pub fn detect_hunks(diff_lines: &[DiffLine]) -> Vec<DiffHunk> {
    let mut hunks = Vec::new();
    let mut i = 0;
    while i < diff_lines.len() {
        match &diff_lines[i] {
            DiffLine::Context { .. } => {
                i += 1;
            }
            _ => {
                let start = i;
                while i < diff_lines.len() && !matches!(&diff_lines[i], DiffLine::Context { .. })
                {
                    i += 1;
                }
                hunks.push(DiffHunk {
                    start_index: start,
                    length: i - start,
                });
            }
        }
    }
    hunks
}

// ─── DiffNavigator ──────────────────────────────────────────────────────

/// Navigates between diff hunks.
pub struct DiffNavigator {
    hunks: Vec<DiffHunk>,
    current: Option<usize>,
}

impl DiffNavigator {
    /// Create a navigator from diff lines.
    #[must_use]
    pub fn new(diff_lines: &[DiffLine]) -> Self {
        Self {
            hunks: detect_hunks(diff_lines),
            current: None,
        }
    }

    /// Number of hunks.
    #[must_use]
    pub fn hunk_count(&self) -> usize {
        self.hunks.len()
    }

    /// Currently focused hunk index.
    #[must_use]
    pub fn current_hunk(&self) -> Option<usize> {
        self.current
    }

    /// Get the current hunk.
    #[must_use]
    pub fn current_hunk_info(&self) -> Option<&DiffHunk> {
        self.current.and_then(|i| self.hunks.get(i))
    }

    /// Jump to the next hunk. Wraps around.
    pub fn next_hunk(&mut self) {
        if self.hunks.is_empty() {
            return;
        }
        self.current = Some(match self.current {
            Some(i) if i + 1 < self.hunks.len() => i + 1,
            _ => 0,
        });
    }

    /// Jump to the previous hunk. Wraps around.
    pub fn prev_hunk(&mut self) {
        if self.hunks.is_empty() {
            return;
        }
        self.current = Some(match self.current {
            Some(0) | None => self.hunks.len() - 1,
            Some(i) => i - 1,
        });
    }

    /// Jump to hunk containing the given diff line index.
    pub fn jump_to_line(&mut self, line_index: usize) {
        self.current = self.hunks.iter().position(|h| {
            line_index >= h.start_index && line_index < h.start_index + h.length
        });
    }
}

// ─── SplitView rendering ────────────────────────────────────────────────

/// Configuration for the split diff view.
#[derive(Debug, Clone)]
pub struct SplitViewConfig {
    /// Show line numbers.
    pub line_numbers: bool,
    /// Context lines around hunks (0 = show all).
    pub context_lines: usize,
}

impl Default for SplitViewConfig {
    fn default() -> Self {
        Self {
            line_numbers: true,
            context_lines: 3,
        }
    }
}

/// Renders a side-by-side (split) diff view.
pub struct SplitDiffView<'t> {
    theme: &'t Theme,
    diff_lines: Vec<DiffLine>,
    config: SplitViewConfig,
    /// Vertical scroll offset.
    scroll: usize,
    /// Optional navigator for hunk jumping.
    navigator: DiffNavigator,
    /// Labels for old/new files.
    old_label: String,
    new_label: String,
}

impl<'t> SplitDiffView<'t> {
    /// Create a split diff view from two text inputs.
    #[must_use]
    pub fn new(theme: &'t Theme, old: &str, new: &str) -> Self {
        let diff_lines = compute_diff_lines(old, new);
        let navigator = DiffNavigator::new(&diff_lines);
        Self {
            theme,
            diff_lines,
            config: SplitViewConfig::default(),
            scroll: 0,
            navigator,
            old_label: "old".to_string(),
            new_label: "new".to_string(),
        }
    }

    /// Create with labels.
    #[must_use]
    pub fn with_labels(
        theme: &'t Theme,
        old: &str,
        new: &str,
        old_label: impl Into<String>,
        new_label: impl Into<String>,
    ) -> Self {
        let mut view = Self::new(theme, old, new);
        view.old_label = old_label.into();
        view.new_label = new_label.into();
        view
    }

    /// Set config.
    pub fn set_config(&mut self, config: SplitViewConfig) {
        self.config = config;
    }

    /// Set scroll offset.
    pub fn set_scroll(&mut self, scroll: usize) {
        self.scroll = scroll;
    }

    /// Total number of diff lines.
    #[must_use]
    pub fn line_count(&self) -> usize {
        self.diff_lines.len()
    }

    /// Access the navigator for hunk jumping.
    #[must_use]
    pub fn navigator(&self) -> &DiffNavigator {
        &self.navigator
    }

    /// Mutable access to navigator.
    pub fn navigator_mut(&mut self) -> &mut DiffNavigator {
        &mut self.navigator
    }

    /// Get the diff lines.
    #[must_use]
    pub fn diff_lines(&self) -> &[DiffLine] {
        &self.diff_lines
    }

    /// Render the left (old) pane lines.
    #[must_use]
    pub fn render_left_pane(&self) -> Vec<Line<'static>> {
        self.diff_lines
            .iter()
            .map(|dl| self.render_left_line(dl))
            .collect()
    }

    /// Render the right (new) pane lines.
    #[must_use]
    pub fn render_right_pane(&self) -> Vec<Line<'static>> {
        self.diff_lines
            .iter()
            .map(|dl| self.render_right_line(dl))
            .collect()
    }

    fn render_left_line(&self, dl: &DiffLine) -> Line<'static> {
        let num_style = Style::default().fg(self.theme.muted);
        let ctx_style = Style::default().fg(self.theme.fg);
        let del_style = Style::default()
            .fg(self.theme.diff_remove_fg)
            .bg(self.theme.diff_remove_bg);
        let del_highlight = Style::default()
            .fg(self.theme.diff_remove_fg)
            .bg(self.theme.diff_remove_bg)
            .add_modifier(Modifier::BOLD);
        let empty_style = Style::default().fg(self.theme.muted);

        match dl {
            DiffLine::Context {
                line_num_old, text, ..
            } => {
                let mut spans = Vec::new();
                if self.config.line_numbers {
                    spans.push(Span::styled(
                        format!("{:>4} ", line_num_old),
                        num_style,
                    ));
                }
                spans.push(Span::styled(format!("  {text}"), ctx_style));
                Line::from(spans)
            }
            DiffLine::Removed {
                line_num_old, text, ..
            } => {
                let mut spans = Vec::new();
                if self.config.line_numbers {
                    spans.push(Span::styled(
                        format!("{:>4} ", line_num_old),
                        num_style,
                    ));
                }
                spans.push(Span::styled(format!("- {text}"), del_style));
                Line::from(spans)
            }
            DiffLine::Added { .. } => {
                // Nothing on the left for added lines
                let mut spans = Vec::new();
                if self.config.line_numbers {
                    spans.push(Span::styled("     ", num_style));
                }
                spans.push(Span::styled("  ", empty_style));
                Line::from(spans)
            }
            DiffLine::Modified {
                line_num_old,
                old_fragments,
                ..
            } => {
                let mut spans = Vec::new();
                if self.config.line_numbers {
                    spans.push(Span::styled(
                        format!("{:>4} ", line_num_old),
                        num_style,
                    ));
                }
                spans.push(Span::styled("- ", del_style));
                for frag in old_fragments {
                    let style = match frag.kind {
                        InlineChangeKind::Equal => del_style,
                        InlineChangeKind::Delete => del_highlight,
                        InlineChangeKind::Insert => del_style, // shouldn't occur
                    };
                    spans.push(Span::styled(frag.text.clone(), style));
                }
                Line::from(spans)
            }
        }
    }

    fn render_right_line(&self, dl: &DiffLine) -> Line<'static> {
        let num_style = Style::default().fg(self.theme.muted);
        let ctx_style = Style::default().fg(self.theme.fg);
        let add_style = Style::default()
            .fg(self.theme.diff_add_fg)
            .bg(self.theme.diff_add_bg);
        let add_highlight = Style::default()
            .fg(self.theme.diff_add_fg)
            .bg(self.theme.diff_add_bg)
            .add_modifier(Modifier::BOLD);
        let empty_style = Style::default().fg(self.theme.muted);

        match dl {
            DiffLine::Context {
                line_num_new, text, ..
            } => {
                let mut spans = Vec::new();
                if self.config.line_numbers {
                    spans.push(Span::styled(
                        format!("{:>4} ", line_num_new),
                        num_style,
                    ));
                }
                spans.push(Span::styled(format!("  {text}"), ctx_style));
                Line::from(spans)
            }
            DiffLine::Added {
                line_num_new, text, ..
            } => {
                let mut spans = Vec::new();
                if self.config.line_numbers {
                    spans.push(Span::styled(
                        format!("{:>4} ", line_num_new),
                        num_style,
                    ));
                }
                spans.push(Span::styled(format!("+ {text}"), add_style));
                Line::from(spans)
            }
            DiffLine::Removed { .. } => {
                // Nothing on the right for removed lines
                let mut spans = Vec::new();
                if self.config.line_numbers {
                    spans.push(Span::styled("     ", num_style));
                }
                spans.push(Span::styled("  ", empty_style));
                Line::from(spans)
            }
            DiffLine::Modified {
                line_num_new,
                new_fragments,
                ..
            } => {
                let mut spans = Vec::new();
                if self.config.line_numbers {
                    spans.push(Span::styled(
                        format!("{:>4} ", line_num_new),
                        num_style,
                    ));
                }
                spans.push(Span::styled("+ ", add_style));
                for frag in new_fragments {
                    let style = match frag.kind {
                        InlineChangeKind::Equal => add_style,
                        InlineChangeKind::Insert => add_highlight,
                        InlineChangeKind::Delete => add_style, // shouldn't occur
                    };
                    spans.push(Span::styled(frag.text.clone(), style));
                }
                Line::from(spans)
            }
        }
    }
}

/// Widget that renders the split diff view into a given area.
/// Divides the area into two halves with a vertical separator.
impl Widget for &SplitDiffView<'_> {
    #[allow(clippy::cast_possible_truncation)]
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 10 || area.height < 3 {
            return;
        }

        let border_style = Style::default().fg(self.theme.border);
        let header_style = Style::default()
            .fg(self.theme.heading)
            .add_modifier(Modifier::BOLD);

        // Split area: left half | separator | right half
        let half_width = (area.width - 1) / 2;
        let left_area = Rect::new(area.x, area.y + 1, half_width, area.height.saturating_sub(1));
        let sep_x = area.x + half_width;
        let right_area = Rect::new(
            sep_x + 1,
            area.y + 1,
            area.width.saturating_sub(half_width + 1),
            area.height.saturating_sub(1),
        );

        // Header: labels
        let old_label = truncate_str(&self.old_label, half_width as usize);
        let new_label = truncate_str(&self.new_label, right_area.width as usize);
        if let Some(cell) = buf.cell_mut((area.x, area.y)) {
            // Write old label
            let header_line = Line::from(vec![
                Span::styled(format!("{old_label:<width$}", width = half_width as usize), header_style),
                Span::styled("│", border_style),
                Span::styled(
                    format!("{new_label:<width$}", width = right_area.width as usize),
                    header_style,
                ),
            ]);
            let _ = cell; // just to check bounds
            let header_area = Rect::new(area.x, area.y, area.width, 1);
            Widget::render(header_line, header_area, buf);
        }

        // Vertical separator
        for y in (area.y + 1)..area.y.saturating_add(area.height) {
            if let Some(cell) = buf.cell_mut((sep_x, y)) {
                cell.set_symbol("│");
                cell.set_style(border_style);
            }
        }

        // Render panes
        let left_lines = self.render_left_pane();
        let right_lines = self.render_right_pane();

        let visible_rows = left_area.height as usize;
        let start = self.scroll;

        for row in 0..visible_rows {
            let idx = start + row;
            let y = left_area.y + row as u16;

            if idx < left_lines.len() {
                let line_area = Rect::new(left_area.x, y, left_area.width, 1);
                Widget::render(left_lines[idx].clone(), line_area, buf);
            }

            if idx < right_lines.len() {
                let line_area = Rect::new(right_area.x, y, right_area.width, 1);
                Widget::render(right_lines[idx].clone(), line_area, buf);
            }
        }
    }
}

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else if max_len > 3 {
        format!("{}...", &s[..max_len - 3])
    } else {
        s[..max_len].to_string()
    }
}

// ─── Inline diff rendering (unified style) ──────────────────────────────

/// Render a single line with character-level inline diff highlighting.
/// Returns spans for a unified-style view (not split).
#[must_use]
pub fn render_inline_diff_line(
    old_line: &str,
    new_line: &str,
    theme: &Theme,
) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
    let (old_frags, new_frags) = compute_inline_diff(old_line, new_line);

    let del_style = Style::default()
        .fg(theme.diff_remove_fg)
        .bg(theme.diff_remove_bg);
    let del_bold = del_style.add_modifier(Modifier::BOLD);
    let add_style = Style::default()
        .fg(theme.diff_add_fg)
        .bg(theme.diff_add_bg);
    let add_bold = add_style.add_modifier(Modifier::BOLD);

    let old_spans: Vec<Span<'static>> = old_frags
        .into_iter()
        .map(|f| {
            let style = match f.kind {
                InlineChangeKind::Equal => del_style,
                InlineChangeKind::Delete => del_bold,
                InlineChangeKind::Insert => del_style,
            };
            Span::styled(f.text, style)
        })
        .collect();

    let new_spans: Vec<Span<'static>> = new_frags
        .into_iter()
        .map(|f| {
            let style = match f.kind {
                InlineChangeKind::Equal => add_style,
                InlineChangeKind::Insert => add_bold,
                InlineChangeKind::Delete => add_style,
            };
            Span::styled(f.text, style)
        })
        .collect();

    (old_spans, new_spans)
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    const OLD: &str = "line one\nline two\nline three\n";
    const NEW: &str = "line one\nline TWO\nline three\nline four\n";

    // ── compute_diff_lines ──

    #[test]
    fn diff_lines_context() {
        let lines = compute_diff_lines(OLD, NEW);
        let ctx_count = lines
            .iter()
            .filter(|l| matches!(l, DiffLine::Context { .. }))
            .count();
        // "line one" and "line three" are context
        assert_eq!(ctx_count, 2);
    }

    #[test]
    fn diff_lines_modified() {
        let lines = compute_diff_lines(OLD, NEW);
        let mod_count = lines
            .iter()
            .filter(|l| matches!(l, DiffLine::Modified { .. }))
            .count();
        // "line two" -> "line TWO" is modified
        assert_eq!(mod_count, 1);
    }

    #[test]
    fn diff_lines_added() {
        let lines = compute_diff_lines(OLD, NEW);
        let add_count = lines
            .iter()
            .filter(|l| matches!(l, DiffLine::Added { .. }))
            .count();
        // "line four" is added
        assert_eq!(add_count, 1);
    }

    #[test]
    fn diff_lines_identical() {
        let lines = compute_diff_lines("same\n", "same\n");
        assert_eq!(lines.len(), 1);
        assert!(matches!(&lines[0], DiffLine::Context { .. }));
    }

    #[test]
    fn diff_lines_empty_old() {
        let lines = compute_diff_lines("", "new\n");
        assert_eq!(lines.len(), 1);
        assert!(matches!(&lines[0], DiffLine::Added { .. }));
    }

    #[test]
    fn diff_lines_empty_new() {
        let lines = compute_diff_lines("old\n", "");
        assert_eq!(lines.len(), 1);
        assert!(matches!(&lines[0], DiffLine::Removed { .. }));
    }

    #[test]
    fn diff_lines_both_empty() {
        let lines = compute_diff_lines("", "");
        assert!(lines.is_empty());
    }

    // ── compute_inline_diff ──

    #[test]
    fn inline_diff_detects_change() {
        let (old_frags, new_frags) = compute_inline_diff("hello world", "hello WORLD");
        // "hello " is equal, then "world" -> "WORLD"
        assert!(old_frags.iter().any(|f| f.kind == InlineChangeKind::Delete));
        assert!(new_frags.iter().any(|f| f.kind == InlineChangeKind::Insert));
    }

    #[test]
    fn inline_diff_identical() {
        let (old_frags, new_frags) = compute_inline_diff("same", "same");
        assert!(old_frags.iter().all(|f| f.kind == InlineChangeKind::Equal));
        assert!(new_frags.iter().all(|f| f.kind == InlineChangeKind::Equal));
    }

    #[test]
    fn inline_diff_completely_different() {
        let (old_frags, new_frags) = compute_inline_diff("abc", "xyz");
        assert!(old_frags.iter().any(|f| f.kind == InlineChangeKind::Delete));
        assert!(new_frags.iter().any(|f| f.kind == InlineChangeKind::Insert));
    }

    #[test]
    fn coalesce_merges_adjacent() {
        let frags = vec![
            InlineFragment { kind: InlineChangeKind::Equal, text: "a".into() },
            InlineFragment { kind: InlineChangeKind::Equal, text: "b".into() },
            InlineFragment { kind: InlineChangeKind::Delete, text: "c".into() },
        ];
        let result = coalesce_fragments(frags);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].text, "ab");
        assert_eq!(result[1].text, "c");
    }

    // ── detect_hunks ──

    #[test]
    fn detect_hunks_basic() {
        let lines = compute_diff_lines(OLD, NEW);
        let hunks = detect_hunks(&lines);
        assert!(!hunks.is_empty());
    }

    #[test]
    fn detect_hunks_no_changes() {
        let lines = compute_diff_lines("a\n", "a\n");
        let hunks = detect_hunks(&lines);
        assert!(hunks.is_empty());
    }

    #[test]
    fn detect_hunks_all_changed() {
        let lines = compute_diff_lines("a\n", "b\n");
        let hunks = detect_hunks(&lines);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].start_index, 0);
    }

    #[test]
    fn detect_hunks_multiple() {
        let old = "a\nb\nc\nd\ne\n";
        let new = "a\nB\nc\nD\ne\n";
        let lines = compute_diff_lines(old, new);
        let hunks = detect_hunks(&lines);
        // Two separate modified hunks (b->B and d->D), separated by context "c"
        assert_eq!(hunks.len(), 2);
    }

    // ── DiffNavigator ──

    #[test]
    fn navigator_empty() {
        let lines = compute_diff_lines("a\n", "a\n");
        let nav = DiffNavigator::new(&lines);
        assert_eq!(nav.hunk_count(), 0);
        assert!(nav.current_hunk().is_none());
    }

    #[test]
    fn navigator_next_prev() {
        let old = "a\nb\nc\nd\ne\n";
        let new = "a\nB\nc\nD\ne\n";
        let lines = compute_diff_lines(old, new);
        let mut nav = DiffNavigator::new(&lines);
        assert_eq!(nav.hunk_count(), 2);

        nav.next_hunk();
        assert_eq!(nav.current_hunk(), Some(0));

        nav.next_hunk();
        assert_eq!(nav.current_hunk(), Some(1));

        nav.next_hunk(); // wraps
        assert_eq!(nav.current_hunk(), Some(0));

        nav.prev_hunk(); // wraps back
        assert_eq!(nav.current_hunk(), Some(1));
    }

    #[test]
    fn navigator_jump_to_line() {
        let old = "a\nb\nc\nd\ne\n";
        let new = "a\nB\nc\nD\ne\n";
        let lines = compute_diff_lines(old, new);
        let mut nav = DiffNavigator::new(&lines);

        nav.jump_to_line(1); // should be in first hunk (b->B)
        assert_eq!(nav.current_hunk(), Some(0));

        nav.jump_to_line(3); // should be in second hunk (d->D)
        assert_eq!(nav.current_hunk(), Some(1));

        nav.jump_to_line(0); // context line — no hunk
        assert!(nav.current_hunk().is_none());
    }

    #[test]
    fn navigator_current_hunk_info() {
        let lines = compute_diff_lines("a\n", "b\n");
        let mut nav = DiffNavigator::new(&lines);
        assert!(nav.current_hunk_info().is_none());
        nav.next_hunk();
        let info = nav.current_hunk_info().unwrap();
        assert_eq!(info.start_index, 0);
        assert!(info.length > 0);
    }

    // ── SplitDiffView ──

    #[test]
    fn split_view_line_count() {
        let theme = Theme::dark();
        let view = SplitDiffView::new(&theme, OLD, NEW);
        assert_eq!(view.line_count(), 4); // context + modified + context + added
    }

    #[test]
    fn split_view_left_pane() {
        let theme = Theme::dark();
        let view = SplitDiffView::new(&theme, OLD, NEW);
        let left = view.render_left_pane();
        assert_eq!(left.len(), view.line_count());
    }

    #[test]
    fn split_view_right_pane() {
        let theme = Theme::dark();
        let view = SplitDiffView::new(&theme, OLD, NEW);
        let right = view.render_right_pane();
        assert_eq!(right.len(), view.line_count());
    }

    #[test]
    fn split_view_with_labels() {
        let theme = Theme::dark();
        let view =
            SplitDiffView::with_labels(&theme, OLD, NEW, "file_a.rs", "file_b.rs");
        assert_eq!(view.old_label, "file_a.rs");
        assert_eq!(view.new_label, "file_b.rs");
    }

    #[test]
    fn split_view_navigator_access() {
        let theme = Theme::dark();
        let mut view = SplitDiffView::new(&theme, OLD, NEW);
        view.navigator_mut().next_hunk();
        assert!(view.navigator().current_hunk().is_some());
    }

    #[test]
    fn split_view_renders_without_panic() {
        let theme = Theme::dark();
        let view = SplitDiffView::new(&theme, OLD, NEW);
        let area = Rect::new(0, 0, 80, 20);
        let mut buf = Buffer::empty(area);
        Widget::render(&view, area, &mut buf);
    }

    #[test]
    fn split_view_tiny_area_no_panic() {
        let theme = Theme::dark();
        let view = SplitDiffView::new(&theme, OLD, NEW);
        let area = Rect::new(0, 0, 5, 2);
        let mut buf = Buffer::empty(area);
        Widget::render(&view, area, &mut buf);
    }

    #[test]
    fn split_view_zero_height_no_panic() {
        let theme = Theme::dark();
        let view = SplitDiffView::new(&theme, OLD, NEW);
        let area = Rect::new(0, 0, 80, 0);
        let mut buf = Buffer::empty(area);
        Widget::render(&view, area, &mut buf);
    }

    // ── render_inline_diff_line ──

    #[test]
    fn inline_diff_line_produces_spans() {
        let theme = Theme::dark();
        let (old_spans, new_spans) =
            render_inline_diff_line("hello world", "hello WORLD", &theme);
        assert!(!old_spans.is_empty());
        assert!(!new_spans.is_empty());
    }

    #[test]
    fn inline_diff_line_colors() {
        let theme = Theme::dark();
        let (old_spans, new_spans) = render_inline_diff_line("abc", "xyz", &theme);
        // Old spans should use remove color
        assert!(old_spans
            .iter()
            .any(|s| s.style.fg == Some(Color::Red)));
        // New spans should use add color
        assert!(new_spans
            .iter()
            .any(|s| s.style.fg == Some(Color::Green)));
    }

    // ── SplitViewConfig ──

    #[test]
    fn config_default() {
        let config = SplitViewConfig::default();
        assert!(config.line_numbers);
        assert_eq!(config.context_lines, 3);
    }

    #[test]
    fn config_custom() {
        let theme = Theme::dark();
        let mut view = SplitDiffView::new(&theme, OLD, NEW);
        view.set_config(SplitViewConfig {
            line_numbers: false,
            context_lines: 0,
        });
        let left = view.render_left_pane();
        // Lines should not start with line numbers
        let first_text: String = left[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        // Without line numbers, first span shouldn't be "   1 "
        assert!(!first_text.starts_with("   1"));
    }

    // ── truncate_str ──

    #[test]
    fn truncate_short_string() {
        assert_eq!(truncate_str("hi", 10), "hi");
    }

    #[test]
    fn truncate_long_string() {
        assert_eq!(truncate_str("hello world", 8), "hello...");
    }

    #[test]
    fn truncate_exact_length() {
        assert_eq!(truncate_str("abc", 3), "abc");
    }

    // ── DiffLine line numbers ──

    #[test]
    fn diff_line_numbers_sequential() {
        let lines = compute_diff_lines(OLD, NEW);
        let mut seen_old = Vec::new();
        let mut seen_new = Vec::new();
        for dl in &lines {
            match dl {
                DiffLine::Context {
                    line_num_old,
                    line_num_new,
                    ..
                } => {
                    seen_old.push(*line_num_old);
                    seen_new.push(*line_num_new);
                }
                DiffLine::Removed { line_num_old, .. } => {
                    seen_old.push(*line_num_old);
                }
                DiffLine::Added { line_num_new, .. } => {
                    seen_new.push(*line_num_new);
                }
                DiffLine::Modified {
                    line_num_old,
                    line_num_new,
                    ..
                } => {
                    seen_old.push(*line_num_old);
                    seen_new.push(*line_num_new);
                }
            }
        }
        // Line numbers should be monotonically increasing
        for w in seen_old.windows(2) {
            assert!(w[1] >= w[0], "Old line numbers not increasing: {:?}", seen_old);
        }
        for w in seen_new.windows(2) {
            assert!(w[1] >= w[0], "New line numbers not increasing: {:?}", seen_new);
        }
    }

    // ── Widget rendering content ──

    #[test]
    fn widget_renders_labels() {
        let theme = Theme::dark();
        let view = SplitDiffView::with_labels(&theme, "a\n", "b\n", "old.rs", "new.rs");
        let area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(area);
        Widget::render(&view, area, &mut buf);

        let first_row: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(first_row.contains("old.rs"), "Missing old label: {first_row}");
        assert!(first_row.contains("new.rs"), "Missing new label: {first_row}");
    }

    #[test]
    fn widget_renders_separator() {
        let theme = Theme::dark();
        let view = SplitDiffView::new(&theme, "a\n", "b\n");
        let area = Rect::new(0, 0, 40, 5);
        let mut buf = Buffer::empty(area);
        Widget::render(&view, area, &mut buf);

        let sep_x = (area.width - 1) / 2;
        // Check separator column has │
        let sep_cell = buf.cell((sep_x, 1)).unwrap();
        assert_eq!(sep_cell.symbol(), "│");
    }

    #[test]
    fn split_view_scroll() {
        let theme = Theme::dark();
        let mut view = SplitDiffView::new(&theme, OLD, NEW);
        view.set_scroll(1);
        let area = Rect::new(0, 0, 80, 5);
        let mut buf = Buffer::empty(area);
        Widget::render(&view, area, &mut buf);
        // Should render without panic even with scroll offset
    }
}
