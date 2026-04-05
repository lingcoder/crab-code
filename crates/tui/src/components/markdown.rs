use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::syntax::SyntaxHighlighter;
use crate::theme::Theme;

/// Renders Markdown text to ratatui `Line`s.
///
/// Uses pulldown-cmark for parsing and `SyntaxHighlighter` for fenced
/// code blocks. All styling is controlled by the `Theme`.
pub struct MarkdownRenderer<'t> {
    theme: &'t Theme,
    highlighter: &'t SyntaxHighlighter,
}

impl<'t> MarkdownRenderer<'t> {
    /// Create a new renderer referencing the given theme and syntax highlighter.
    #[must_use]
    pub fn new(theme: &'t Theme, highlighter: &'t SyntaxHighlighter) -> Self {
        Self { theme, highlighter }
    }

    /// Parse and render a Markdown string into styled `Line`s.
    #[allow(clippy::too_many_lines)]
    pub fn render(&self, markdown: &str) -> Vec<Line<'static>> {
        let opts =
            Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
        let parser = Parser::new_ext(markdown, opts);

        let mut lines: Vec<Line<'static>> = Vec::new();
        let mut current_spans: Vec<Span<'static>> = Vec::new();
        let mut style_stack: Vec<Style> = vec![Style::default().fg(self.theme.fg)];
        let mut in_code_block = false;
        let mut code_lang = String::new();
        let mut code_buf = String::new();
        let mut list_depth: usize = 0;
        let mut ordered_index: Option<u64> = None;

        for event in parser {
            match event {
                Event::Start(tag) => match tag {
                    Tag::Heading { level, .. } => {
                        flush_line(&mut current_spans, &mut lines);
                        let prefix = heading_prefix(level);
                        let style = Style::default()
                            .fg(self.theme.heading)
                            .add_modifier(Modifier::BOLD);
                        current_spans.push(Span::styled(prefix, style));
                        style_stack.push(style);
                    }
                    Tag::Paragraph => {
                        flush_line(&mut current_spans, &mut lines);
                    }
                    Tag::CodeBlock(kind) => {
                        flush_line(&mut current_spans, &mut lines);
                        in_code_block = true;
                        code_buf.clear();
                        code_lang = match kind {
                            CodeBlockKind::Fenced(lang) => lang.to_string(),
                            CodeBlockKind::Indented => String::new(),
                        };
                    }
                    Tag::Emphasis => {
                        let style = current_style(&style_stack).add_modifier(self.theme.italic);
                        style_stack.push(style);
                    }
                    Tag::Strong => {
                        let style = current_style(&style_stack).add_modifier(self.theme.bold);
                        style_stack.push(style);
                    }
                    Tag::Strikethrough => {
                        let style = current_style(&style_stack).add_modifier(Modifier::CROSSED_OUT);
                        style_stack.push(style);
                    }
                    Tag::Link { dest_url, .. } => {
                        let style = Style::default()
                            .fg(self.theme.link)
                            .add_modifier(Modifier::UNDERLINED);
                        style_stack.push(style);
                        // Store the URL for later — we'll append it after the link text
                        // by using a sentinel approach: push URL as hidden state
                        // Actually, we just render [text](url) inline for terminal
                        let _ = dest_url; // URL rendered after End(Link)
                    }
                    Tag::List(start) => {
                        flush_line(&mut current_spans, &mut lines);
                        ordered_index = start;
                        list_depth += 1;
                    }
                    Tag::Item => {
                        flush_line(&mut current_spans, &mut lines);
                        let indent = "  ".repeat(list_depth.saturating_sub(1));
                        let marker = ordered_index.as_mut().map_or_else(
                            || format!("{indent}- "),
                            |idx| {
                                let m = format!("{indent}{idx}. ");
                                *idx += 1;
                                m
                            },
                        );
                        let style = Style::default().fg(self.theme.list_marker);
                        current_spans.push(Span::styled(marker, style));
                    }
                    Tag::BlockQuote(_) => {
                        flush_line(&mut current_spans, &mut lines);
                        let style = Style::default().fg(self.theme.blockquote);
                        current_spans.push(Span::styled("│ ".to_string(), style));
                        style_stack.push(Style::default().fg(self.theme.blockquote));
                    }
                    _ => {}
                },

                Event::End(tag_end) => match tag_end {
                    TagEnd::Paragraph => {
                        flush_line(&mut current_spans, &mut lines);
                        // Add blank line after paragraph
                        lines.push(Line::from(""));
                    }
                    TagEnd::CodeBlock => {
                        in_code_block = false;
                        let highlighted = if code_lang.is_empty() {
                            SyntaxHighlighter::highlight_plain(&code_buf, self.theme)
                        } else {
                            self.highlighter.highlight(&code_buf, &code_lang)
                        };
                        lines.extend(highlighted.into_iter().map(|l| {
                            // Owned conversion
                            Line::from(
                                l.spans
                                    .into_iter()
                                    .map(|s| Span::styled(s.content.to_string(), s.style))
                                    .collect::<Vec<_>>(),
                            )
                        }));
                        code_buf.clear();
                    }
                    TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link => {
                        style_stack.pop();
                    }
                    TagEnd::Heading(_) | TagEnd::BlockQuote(_) => {
                        style_stack.pop();
                        flush_line(&mut current_spans, &mut lines);
                    }
                    TagEnd::List(_) => {
                        list_depth = list_depth.saturating_sub(1);
                        if list_depth == 0 {
                            ordered_index = None;
                        }
                    }
                    TagEnd::Item => {
                        flush_line(&mut current_spans, &mut lines);
                    }
                    _ => {}
                },

                Event::Text(text) => {
                    if in_code_block {
                        code_buf.push_str(&text);
                    } else {
                        let style = current_style(&style_stack);
                        current_spans.push(Span::styled(text.to_string(), style));
                    }
                }

                Event::Code(code) => {
                    let style = Style::default()
                        .fg(self.theme.inline_code_fg)
                        .bg(self.theme.inline_code_bg);
                    current_spans.push(Span::styled(format!("`{code}`"), style));
                }

                Event::SoftBreak => {
                    current_spans.push(Span::raw(" ".to_string()));
                }

                Event::HardBreak => {
                    flush_line(&mut current_spans, &mut lines);
                }

                Event::Rule => {
                    flush_line(&mut current_spans, &mut lines);
                    let style = Style::default().fg(self.theme.muted);
                    lines.push(Line::from(Span::styled("─".repeat(40), style)));
                }

                _ => {}
            }
        }

        // Flush any remaining spans
        flush_line(&mut current_spans, &mut lines);
        lines
    }
}

/// Get the current top-of-stack style.
fn current_style(stack: &[Style]) -> Style {
    stack.last().copied().unwrap_or_default()
}

/// Move accumulated spans into a new `Line`, clearing the buffer.
fn flush_line(spans: &mut Vec<Span<'static>>, lines: &mut Vec<Line<'static>>) {
    if !spans.is_empty() {
        lines.push(Line::from(std::mem::take(spans)));
    }
}

/// Generate heading prefix like "# ", "## ", etc.
fn heading_prefix(level: HeadingLevel) -> String {
    let n = match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    };
    format!("{} ", "#".repeat(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_renderer() -> (Theme, SyntaxHighlighter) {
        (Theme::dark(), SyntaxHighlighter::new())
    }

    #[test]
    fn render_plain_paragraph() {
        let (theme, hl) = make_renderer();
        let r = MarkdownRenderer::new(&theme, &hl);
        let lines = r.render("Hello world");
        assert!(!lines.is_empty());
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains("Hello world"));
    }

    #[test]
    fn render_heading() {
        let (theme, hl) = make_renderer();
        let r = MarkdownRenderer::new(&theme, &hl);
        let lines = r.render("# Title");
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains("# "));
        assert!(text.contains("Title"));
    }

    #[test]
    fn render_h2_heading() {
        let (theme, hl) = make_renderer();
        let r = MarkdownRenderer::new(&theme, &hl);
        let lines = r.render("## Sub-heading");
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains("## "));
    }

    #[test]
    fn render_bold_and_italic() {
        let (theme, hl) = make_renderer();
        let r = MarkdownRenderer::new(&theme, &hl);
        let lines = r.render("**bold** and *italic*");
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains("bold"));
        assert!(text.contains("italic"));
    }

    #[test]
    fn render_inline_code() {
        let (theme, hl) = make_renderer();
        let r = MarkdownRenderer::new(&theme, &hl);
        let lines = r.render("Use `foo()` here");
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains("`foo()`"));
    }

    #[test]
    fn render_code_block() {
        let (theme, hl) = make_renderer();
        let r = MarkdownRenderer::new(&theme, &hl);
        let md = "```rust\nfn main() {}\n```";
        let lines = r.render(md);
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains("fn"));
        assert!(text.contains("main"));
    }

    #[test]
    fn render_unordered_list() {
        let (theme, hl) = make_renderer();
        let r = MarkdownRenderer::new(&theme, &hl);
        let lines = r.render("- one\n- two\n- three");
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains("- "));
        assert!(text.contains("one"));
        assert!(text.contains("two"));
    }

    #[test]
    fn render_ordered_list() {
        let (theme, hl) = make_renderer();
        let r = MarkdownRenderer::new(&theme, &hl);
        let lines = r.render("1. first\n2. second");
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains("1. "));
        assert!(text.contains("first"));
    }

    #[test]
    fn render_link() {
        let (theme, hl) = make_renderer();
        let r = MarkdownRenderer::new(&theme, &hl);
        let lines = r.render("[click](https://example.com)");
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains("click"));
    }

    #[test]
    fn render_horizontal_rule() {
        let (theme, hl) = make_renderer();
        let r = MarkdownRenderer::new(&theme, &hl);
        let lines = r.render("---");
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains("─"));
    }

    #[test]
    fn render_blockquote() {
        let (theme, hl) = make_renderer();
        let r = MarkdownRenderer::new(&theme, &hl);
        let lines = r.render("> quoted text");
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains("│ "));
        assert!(text.contains("quoted text"));
    }

    #[test]
    fn render_empty_string() {
        let (theme, hl) = make_renderer();
        let r = MarkdownRenderer::new(&theme, &hl);
        let lines = r.render("");
        assert!(lines.is_empty());
    }

    #[test]
    fn heading_prefix_levels() {
        assert_eq!(heading_prefix(HeadingLevel::H1), "# ");
        assert_eq!(heading_prefix(HeadingLevel::H3), "### ");
        assert_eq!(heading_prefix(HeadingLevel::H6), "###### ");
    }
}
