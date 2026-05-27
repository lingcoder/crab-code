//! Assistant reply cell — renders markdown body with a `⎿` corner prefix on
//! the first line and matching padding on continuation lines.

use std::cell::RefCell;

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::components::syntax::SyntaxHighlighter;
use crate::components::text_utils::strip_trailing_tool_json;
use crate::history::HistoryCell;
use crate::markdown::CachedMarkdownRenderer;
use crate::theme;

const BULLET_PREFIX: &str = "● ";
const CONT_LINE_PREFIX: &str = "  ";

thread_local! {
    /// Shared per-render-thread markdown cache. Keeps expensive
    /// pulldown-cmark parses memoized by (content, theme, width).
    static SHARED_MD_CACHE: RefCell<CachedMarkdownRenderer> =
        RefCell::new(CachedMarkdownRenderer::new());
}

/// Assistant reply with markdown + code highlight.
#[derive(Debug, Clone)]
pub struct AssistantCell {
    text: String,
    pub(crate) skip_prefix: usize,
    pub(crate) streaming: bool,
}

impl AssistantCell {
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            skip_prefix: 0,
            streaming: false,
        }
    }

    #[must_use]
    pub fn with_skip(text: impl Into<String>, skip_prefix: usize) -> Self {
        Self {
            text: text.into(),
            skip_prefix,
            streaming: false,
        }
    }

    #[must_use]
    pub fn streaming(mut self, streaming: bool) -> Self {
        self.streaming = streaming;
        self
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn push_delta(&mut self, delta: &str) {
        self.text.push_str(delta);
    }

    /// Render the markdown body up to the last newline and return any lines
    /// beyond `committed`. The cell still owns the full text so subsequent
    /// frames can re-render the streaming tail. Holds back commits while an
    /// opened fenced code block is still missing its closing fence, or while
    /// a markdown table is incomplete (missing delimiter row or rows after
    /// the delimiter).
    pub fn render_committed_lines(&self, width: u16, committed: usize) -> Vec<Line<'static>> {
        let body = match self.text.rfind('\n') {
            Some(idx) => &self.text[..=idx],
            None => return Vec::new(),
        };
        if has_unclosed_code_fence(body) {
            return Vec::new();
        }
        let render_body = match find_unclosed_table_start(body) {
            Some(table_line) => &body[..byte_offset_of_line(body, table_line)],
            None => body,
        };
        if render_body.is_empty() {
            return Vec::new();
        }
        let mut rendered = render_with_bullet_no_trailing_blank(render_body, width);
        while rendered.last().is_some_and(line_is_blank) {
            rendered.pop();
        }
        if rendered.len() <= committed {
            return Vec::new();
        }
        rendered[committed..].to_vec()
    }

    /// Render the entire current text (including any unfinished tail) and
    /// return its line count — used by callers that need to know how many
    /// lines the cell occupies right now.
    pub fn rendered_line_count(&self, width: u16) -> usize {
        render_with_bullet(&self.text, width).len()
    }
}

fn has_unclosed_code_fence(text: &str) -> bool {
    let mut open = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            open = !open;
        }
    }
    open
}

/// If a markdown table is still incomplete (header present but no delimiter,
/// or delimiter present but stream hasn't left the table yet), return the
/// source line index where the table starts. Lines from that point onward
/// must not be committed to scrollback because column widths may change as
/// new rows arrive.
fn find_unclosed_table_start(text: &str) -> Option<usize> {
    let mut in_fence = false;
    let mut pending_header: Option<usize> = None;
    let mut confirmed_table_start: Option<usize> = None;

    for (line_idx, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            if in_fence {
                pending_header = None;
            }
            continue;
        }
        if in_fence {
            continue;
        }

        if is_table_delimiter_line(trimmed) {
            if pending_header.is_some() {
                confirmed_table_start = pending_header;
                pending_header = None;
            }
        } else if is_table_header_line(trimmed) {
            if confirmed_table_start.is_none() {
                pending_header = Some(line_idx);
            }
        } else if !trimmed.is_empty() {
            pending_header = None;
            confirmed_table_start = None;
        }
    }

    confirmed_table_start.or(pending_header)
}

/// A line looks like a table header if it contains `|`-delimited segments
/// with at least one non-empty cell, and is not a delimiter row.
fn is_table_header_line(line: &str) -> bool {
    if is_table_delimiter_line(line) {
        return false;
    }
    let stripped = line.strip_prefix('|').unwrap_or(line);
    let stripped = stripped.strip_suffix('|').unwrap_or(stripped);
    stripped.split('|').any(|seg| !seg.trim().is_empty()) && stripped.contains('|')
}

/// A GFM table delimiter row: every cell is a run of `-` with optional
/// `:` alignment markers, at least 3 dashes per cell.
fn is_table_delimiter_line(line: &str) -> bool {
    let stripped = line.strip_prefix('|').unwrap_or(line);
    let stripped = stripped.strip_suffix('|').unwrap_or(stripped);
    let segments: Vec<&str> = stripped.split('|').map(str::trim).collect();
    !segments.is_empty()
        && segments.iter().all(|seg| {
            let s = seg.strip_prefix(':').unwrap_or(seg);
            let s = s.strip_suffix(':').unwrap_or(s);
            s.len() >= 3 && s.chars().all(|c| c == '-')
        })
}

/// Byte offset of the start of line `n` in `text`.
fn byte_offset_of_line(text: &str, n: usize) -> usize {
    text.lines()
        .take(n)
        .map(|l| l.len() + 1) // +1 for the '\n'
        .sum()
}

fn line_is_blank(line: &Line<'static>) -> bool {
    line.spans
        .iter()
        .all(|s| s.content.chars().all(|c| c == ' '))
}

fn render_with_bullet_no_trailing_blank(text: &str, width: u16) -> Vec<Line<'static>> {
    let mut out = render_with_bullet(text, width);
    if out.last().is_some_and(line_is_blank) {
        out.pop();
    }
    out
}

fn render_with_bullet(text: &str, width: u16) -> Vec<Line<'static>> {
    let clean = strip_trailing_tool_json(text);
    if clean.is_empty() {
        return Vec::new();
    }
    let theme = theme::current();
    let highlighter = SyntaxHighlighter::new();
    let prefix_width = BULLET_PREFIX.chars().count() as u16;
    let inner_width = width.saturating_sub(prefix_width).max(1);
    let md_lines: Vec<Line<'static>> = SHARED_MD_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        (*cache.render(&clean, &theme, &highlighter, inner_width)).clone()
    });
    let bullet_style = Style::default().fg(Color::DarkGray);
    let cont_style = Style::default().fg(Color::DarkGray);
    let mut out: Vec<Line<'static>> = Vec::with_capacity(md_lines.len() + 1);
    for (idx, line) in md_lines.into_iter().enumerate() {
        let mut spans: Vec<Span<'static>> = Vec::with_capacity(line.spans.len() + 1);
        if idx == 0 {
            spans.push(Span::styled(BULLET_PREFIX, bullet_style));
        } else {
            spans.push(Span::styled(CONT_LINE_PREFIX, cont_style));
        }
        spans.extend(line.spans);
        out.push(Line::from(spans));
    }
    out.push(Line::default());
    out
}

impl HistoryCell for AssistantCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut rendered = render_with_bullet(&self.text, width);
        if self.skip_prefix >= rendered.len() {
            return Vec::new();
        }
        rendered.drain(..self.skip_prefix);
        rendered
    }

    fn is_finalized(&self) -> bool {
        !self.streaming
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_produces_no_lines() {
        let cell = AssistantCell::new("");
        assert!(cell.display_lines(80).is_empty());
    }

    #[test]
    fn non_empty_text_renders_with_bullet_prefix() {
        let cell = AssistantCell::new("hello");
        let lines = cell.display_lines(80);
        assert!(!lines.is_empty());
        let first: String = lines[0].spans.iter().map(|s| &*s.content).collect();
        assert!(
            first.starts_with(BULLET_PREFIX),
            "assistant text should start with bullet prefix, got {first:?}"
        );
    }

    #[test]
    fn first_line_has_bullet_prefix_continuations_padded() {
        let cell = AssistantCell::new("line one\n\nline two\n\nline three");
        let lines = cell.display_lines(80);
        // Markdown paragraphs produce multiple lines; last is trailing blank.
        let content_lines: Vec<_> = lines.iter().filter(|l| !l.spans.is_empty()).collect();
        assert!(
            content_lines.len() >= 2,
            "expected multiple content lines, got {}",
            content_lines.len()
        );

        let first: String = content_lines[0].spans.iter().map(|s| &*s.content).collect();
        assert!(
            first.starts_with(BULLET_PREFIX),
            "first line should start with bullet prefix, got {first:?}"
        );

        for line in content_lines.iter().skip(1) {
            let rendered: String = line.spans.iter().map(|s| &*s.content).collect();
            assert!(
                rendered.starts_with(CONT_LINE_PREFIX),
                "continuation line should start with padded prefix, got {rendered:?}"
            );
        }
    }

    #[test]
    fn push_delta_appends() {
        let mut cell = AssistantCell::new("hel");
        cell.push_delta("lo");
        assert_eq!(cell.text(), "hello");
    }

    #[test]
    fn search_text_contains_body() {
        let cell = AssistantCell::new("the **answer** is 42");
        let needle = "the";
        assert!(cell.search_text().contains(needle));
    }

    // -- Table holdback tests --

    #[test]
    fn table_header_detected() {
        assert!(is_table_header_line("| Name | Age |"));
        assert!(is_table_header_line("Name | Age"));
        assert!(!is_table_header_line("just plain text"));
        assert!(!is_table_header_line("|---|---|"));
    }

    #[test]
    fn table_delimiter_detected() {
        assert!(is_table_delimiter_line("| --- | --- |"));
        assert!(is_table_delimiter_line("|:---:|:---:|"));
        assert!(is_table_delimiter_line("---|---"));
        assert!(!is_table_delimiter_line("| Name | Age |"));
        assert!(!is_table_delimiter_line("| -- |")); // only 2 dashes
    }

    #[test]
    fn unclosed_table_holds_back() {
        // Header + delimiter present but no blank line to close the table.
        let text = "intro\n| Name | Age |\n| --- | --- |\n| Alice | 30 |\n";
        assert_eq!(find_unclosed_table_start(text), Some(1));
    }

    #[test]
    fn closed_table_does_not_hold_back() {
        // Blank line after the table closes it.
        let text = "intro\n| Name | Age |\n| --- | --- |\n| Alice | 30 |\n\nnext section\n";
        assert_eq!(find_unclosed_table_start(text), None);
    }

    #[test]
    fn header_without_delimiter_holds_back() {
        let text = "intro\n| Name | Age |\n";
        assert_eq!(find_unclosed_table_start(text), Some(1));
    }

    #[test]
    fn table_inside_code_fence_ignored() {
        let text = "intro\n```\n| Name | Age |\n| --- | --- |\n```\n";
        assert_eq!(find_unclosed_table_start(text), None);
    }

    #[test]
    fn byte_offset_calculation() {
        let text = "line0\nline1\nline2\n";
        assert_eq!(byte_offset_of_line(text, 0), 0);
        assert_eq!(byte_offset_of_line(text, 1), 6); // "line0\n"
        assert_eq!(byte_offset_of_line(text, 2), 12); // "line0\nline1\n"
    }

    #[test]
    fn render_committed_lines_holds_back_table() {
        let cell = AssistantCell::new("before\n| Name | Age |\n| --- | --- |\n| Alice | 30 |\n");
        let lines = cell.render_committed_lines(80, 0);
        // Only "before" should be committed; table lines are held back.
        assert_eq!(lines.len(), 1);
        let text: String = lines[0].spans.iter().map(|s| &*s.content).collect();
        assert!(text.contains("before"), "got: {text}");
    }

    #[test]
    fn render_committed_lines_releases_table_on_close() {
        let cell =
            AssistantCell::new("before\n| Name | Age |\n| --- | --- |\n| Alice | 30 |\n\nafter\n");
        let lines = cell.render_committed_lines(80, 0);
        // Table is closed by blank line, so all content commits.
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(joined.contains("before"));
        assert!(joined.contains("after"));
    }
}
