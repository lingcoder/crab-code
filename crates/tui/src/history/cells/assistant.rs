//! Assistant reply cell — renders markdown body prefixed with `● `.

use std::cell::RefCell;

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::components::syntax::SyntaxHighlighter;
use crate::components::text_utils::strip_trailing_tool_json;
use crate::history::HistoryCell;
use crate::markdown::CachedMarkdownRenderer;
use crate::theme::accents::CLAUDE_DARK;
use crate::theme::{self};

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
}

impl AssistantCell {
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Append a streaming token. Clears trailing partial lines so that
    /// incremental renders stay coherent.
    pub fn push_delta(&mut self, delta: &str) {
        self.text.push_str(delta);
    }
}

impl HistoryCell for AssistantCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        if self.text.is_empty() {
            return Vec::new();
        }
        let clean = strip_trailing_tool_json(&self.text);
        if clean.is_empty() {
            return Vec::new();
        }

        let theme = theme::current();
        let highlighter = SyntaxHighlighter::new();

        let md_lines: Vec<Line<'static>> = SHARED_MD_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            (*cache.render(&clean, &theme, &highlighter, width)).clone()
        });

        let mut out: Vec<Line<'static>> = Vec::with_capacity(md_lines.len() + 1);
        if let Some(first) = md_lines.first() {
            let mut spans = vec![Span::styled("● ", Style::default().fg(CLAUDE_DARK))];
            spans.extend(first.spans.iter().cloned());
            out.push(Line::from(spans));
            out.extend(md_lines.into_iter().skip(1));
        }
        out.push(Line::default());
        out
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
    fn non_empty_text_renders_bullet_prefix() {
        let cell = AssistantCell::new("hello");
        let lines = cell.display_lines(80);
        assert!(!lines.is_empty());
        let first: String = lines[0].spans.iter().map(|s| &*s.content).collect();
        assert!(first.starts_with("● "));
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
}
