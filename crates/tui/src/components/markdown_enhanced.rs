//! Enhanced Markdown rendering with line numbers, tables, OSC 8 links,
//! and theme-integrated syntax highlighting for code blocks.

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::syntax::SyntaxHighlighter;
use crate::theme::{Theme, ThemeName};

// ─── Configuration ──────────────────────────────────────────────────────

/// Options controlling enhanced markdown rendering.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct RenderOptions {
    /// Show line numbers in code blocks.
    pub line_numbers: bool,
    /// Show language label above code blocks.
    pub language_label: bool,
    /// Show a "[copy]" region at the end of the code block header.
    pub copy_region: bool,
    /// Use OSC 8 hyperlinks for clickable URLs in supported terminals.
    pub osc8_links: bool,
    /// Maximum column width for table cells (0 = unlimited).
    pub max_table_col_width: usize,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            line_numbers: true,
            language_label: true,
            copy_region: true,
            osc8_links: true,
            max_table_col_width: 40,
        }
    }
}

// ─── Theme-to-syntect mapping ───────────────────────────────────────────

/// Map a TUI `ThemeName` to the best matching syntect theme name.
#[must_use]
pub fn syntect_theme_for(name: ThemeName) -> &'static str {
    match name {
        ThemeName::Dark | ThemeName::Custom => "base16-ocean.dark",
        ThemeName::Light => "base16-ocean.light",
        ThemeName::Monokai => "base16-mocha.dark",
        ThemeName::Solarized => "Solarized (dark)",
    }
}

// ─── OSC 8 hyperlink helpers ────────────────────────────────────────────

/// Wrap `text` in an OSC 8 hyperlink escape sequence.
#[must_use]
fn osc8_link(url: &str, text: &str) -> String {
    format!("\x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\")
}

// ─── Table builder ──────────────────────────────────────────────────────

/// Accumulates table rows and renders aligned columns.
#[derive(Debug)]
struct TableBuilder {
    alignments: Vec<Alignment>,
    header: Vec<String>,
    rows: Vec<Vec<String>>,
    current_row: Vec<String>,
    current_cell: String,
    in_header: bool,
}

impl TableBuilder {
    fn new(alignments: Vec<Alignment>) -> Self {
        Self {
            alignments,
            header: Vec::new(),
            rows: Vec::new(),
            current_row: Vec::new(),
            current_cell: String::new(),
            in_header: true,
        }
    }

    fn finish_cell(&mut self) {
        self.current_row
            .push(std::mem::take(&mut self.current_cell));
    }

    fn finish_row(&mut self) {
        let row = std::mem::take(&mut self.current_row);
        if self.in_header {
            self.header = row;
            self.in_header = false;
        } else {
            self.rows.push(row);
        }
    }

    fn push_text(&mut self, text: &str) {
        self.current_cell.push_str(text);
    }

    /// Render the table into styled `Line`s.
    fn render(&self, theme: &Theme, max_col_width: usize) -> Vec<Line<'static>> {
        let col_count = self.alignments.len().max(self.header.len());
        if col_count == 0 {
            return Vec::new();
        }

        // Compute column widths
        let mut widths = vec![0usize; col_count];
        for (i, cell) in self.header.iter().enumerate() {
            if i < col_count {
                widths[i] = widths[i].max(cell.len());
            }
        }
        for row in &self.rows {
            for (i, cell) in row.iter().enumerate() {
                if i < col_count {
                    widths[i] = widths[i].max(cell.len());
                }
            }
        }
        if max_col_width > 0 {
            for w in &mut widths {
                *w = (*w).min(max_col_width);
            }
        }
        // Minimum width of 3
        for w in &mut widths {
            *w = (*w).max(3);
        }

        let border_style = Style::default().fg(theme.border);
        let header_style = Style::default()
            .fg(theme.heading)
            .add_modifier(Modifier::BOLD);
        let cell_style = Style::default().fg(theme.fg);

        let mut lines = Vec::new();

        // Header row
        lines.push(render_table_row(
            &self.header,
            &widths,
            &self.alignments,
            header_style,
            border_style,
        ));

        // Separator
        lines.push(render_table_separator(
            &widths,
            &self.alignments,
            border_style,
        ));

        // Data rows
        for row in &self.rows {
            lines.push(render_table_row(
                row,
                &widths,
                &self.alignments,
                cell_style,
                border_style,
            ));
        }

        lines
    }
}

fn render_table_row(
    cells: &[String],
    widths: &[usize],
    alignments: &[Alignment],
    cell_style: Style,
    border_style: Style,
) -> Line<'static> {
    let mut spans = Vec::new();
    spans.push(Span::styled("| ", border_style));

    for (i, width) in widths.iter().enumerate() {
        let text = cells.get(i).map_or("", String::as_str);
        let truncated = if text.len() > *width {
            &text[..*width]
        } else {
            text
        };
        let alignment = alignments.get(i).copied().unwrap_or(Alignment::None);
        let padded = pad_cell(truncated, *width, alignment);
        spans.push(Span::styled(padded, cell_style));
        spans.push(Span::styled(" | ", border_style));
    }

    Line::from(spans)
}

fn render_table_separator(
    widths: &[usize],
    alignments: &[Alignment],
    border_style: Style,
) -> Line<'static> {
    let mut spans = Vec::new();
    spans.push(Span::styled("|", border_style));

    for (i, width) in widths.iter().enumerate() {
        let alignment = alignments.get(i).copied().unwrap_or(Alignment::None);
        let sep = match alignment {
            Alignment::Left => format!(":{}|", "-".repeat(*width + 1)),
            Alignment::Right => format!("{}:|", "-".repeat(*width + 1)),
            Alignment::Center => format!(":{}:|", "-".repeat(*width)),
            Alignment::None => format!("{}|", "-".repeat(*width + 2)),
        };
        spans.push(Span::styled(sep, border_style));
    }

    Line::from(spans)
}

fn pad_cell(text: &str, width: usize, alignment: Alignment) -> String {
    let text_len = text.len();
    if text_len >= width {
        return text.to_string();
    }
    let padding = width - text_len;
    match alignment {
        Alignment::Right => format!("{}{text}", " ".repeat(padding)),
        Alignment::Center => {
            let left = padding / 2;
            let right = padding - left;
            format!("{}{text}{}", " ".repeat(left), " ".repeat(right))
        }
        _ => format!("{text}{}", " ".repeat(padding)),
    }
}

// ─── Enhanced code block rendering ──────────────────────────────────────

/// Render a code block with line numbers, language label, and copy region.
#[allow(clippy::cast_possible_truncation)]
fn render_code_block(
    code: &str,
    language: &str,
    highlighted: Vec<Line<'_>>,
    opts: &RenderOptions,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let border_style = Style::default().fg(theme.border);
    let label_style = Style::default()
        .fg(theme.syntax_keyword)
        .add_modifier(Modifier::BOLD);
    let line_num_style = Style::default().fg(theme.muted);

    // Header line: ┌─ language ─── [copy] ┐
    if opts.language_label || opts.copy_region {
        let mut header_spans = Vec::new();
        header_spans.push(Span::styled("┌─ ", border_style));
        if opts.language_label && !language.is_empty() {
            header_spans.push(Span::styled(language.to_string(), label_style));
            header_spans.push(Span::styled(" ", border_style));
        }
        if opts.copy_region {
            header_spans.push(Span::styled("─── ", border_style));
            header_spans.push(Span::styled("[copy]", Style::default().fg(theme.muted)));
            header_spans.push(Span::styled(" ─┐", border_style));
        }
        lines.push(Line::from(header_spans));
    }

    // Code lines with optional line numbers
    let total_lines = code.lines().count().max(highlighted.len());
    let num_width = if total_lines == 0 {
        1
    } else {
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        {
            (total_lines as f64).log10().floor() as usize + 1
        }
    };

    for (i, hl_line) in highlighted.into_iter().enumerate() {
        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.push(Span::styled("│ ", border_style));

        if opts.line_numbers {
            let num = format!("{:>width$} │ ", i + 1, width = num_width);
            spans.push(Span::styled(num, line_num_style));
        }

        // Convert highlighted spans to owned
        for span in hl_line.spans {
            spans.push(Span::styled(span.content.to_string(), span.style));
        }

        lines.push(Line::from(spans));
    }

    // Footer
    lines.push(Line::from(Span::styled(
        format!("└{}┘", "─".repeat(40)),
        border_style,
    )));

    lines
}

// ─── Enhanced Markdown renderer ─────────────────────────────────────────

/// Enhanced Markdown renderer with tables, line-numbered code blocks,
/// OSC 8 hyperlinks, and theme-integrated syntax highlighting.
pub struct EnhancedMarkdownRenderer<'t> {
    theme: &'t Theme,
    highlighter: &'t SyntaxHighlighter,
    options: RenderOptions,
}

impl<'t> EnhancedMarkdownRenderer<'t> {
    /// Create a new enhanced renderer.
    #[must_use]
    pub fn new(theme: &'t Theme, highlighter: &'t SyntaxHighlighter) -> Self {
        Self {
            theme,
            highlighter,
            options: RenderOptions::default(),
        }
    }

    /// Create with custom options.
    #[must_use]
    pub fn with_options(
        theme: &'t Theme,
        highlighter: &'t SyntaxHighlighter,
        options: RenderOptions,
    ) -> Self {
        Self {
            theme,
            highlighter,
            options,
        }
    }

    /// Parse and render Markdown into styled `Line`s.
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

        // Table state
        let mut table_builder: Option<TableBuilder> = None;
        let mut in_table_head = false;

        // Link state — store URL so we can emit after text
        let mut link_url: Option<String> = None;

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
                        link_url = Some(dest_url.to_string());
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
                    Tag::Table(alignments) => {
                        flush_line(&mut current_spans, &mut lines);
                        table_builder = Some(TableBuilder::new(alignments));
                    }
                    Tag::TableHead => {
                        in_table_head = true;
                    }
                    _ => {}
                },

                Event::End(tag_end) => match tag_end {
                    TagEnd::Paragraph => {
                        flush_line(&mut current_spans, &mut lines);
                        lines.push(Line::from(""));
                    }
                    TagEnd::CodeBlock => {
                        in_code_block = false;
                        let highlighted = if code_lang.is_empty() {
                            SyntaxHighlighter::highlight_plain(&code_buf, self.theme)
                        } else {
                            self.highlighter.highlight(&code_buf, &code_lang)
                        };
                        let code_lines = render_code_block(
                            &code_buf,
                            &code_lang,
                            highlighted,
                            &self.options,
                            self.theme,
                        );
                        lines.extend(code_lines);
                        code_buf.clear();
                    }
                    TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                        style_stack.pop();
                    }
                    TagEnd::Link => {
                        style_stack.pop();
                        // Append (url) after link text, optionally with OSC 8
                        if let Some(url) = link_url.take() {
                            if self.options.osc8_links {
                                let link_text = osc8_link(&url, &format!(" ({url})"));
                                current_spans.push(Span::styled(
                                    link_text,
                                    Style::default().fg(self.theme.muted),
                                ));
                            } else {
                                current_spans.push(Span::styled(
                                    format!(" ({url})"),
                                    Style::default().fg(self.theme.muted),
                                ));
                            }
                        }
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
                    TagEnd::Table => {
                        if let Some(builder) = table_builder.take() {
                            let table_lines =
                                builder.render(self.theme, self.options.max_table_col_width);
                            lines.extend(table_lines);
                        }
                    }
                    TagEnd::TableHead => {
                        in_table_head = false;
                    }
                    TagEnd::TableRow => {
                        if let Some(ref mut builder) = table_builder {
                            builder.finish_row();
                        }
                    }
                    TagEnd::TableCell => {
                        if let Some(ref mut builder) = table_builder {
                            builder.finish_cell();
                        }
                    }
                    _ => {}
                },

                Event::Text(text) => {
                    if in_code_block {
                        code_buf.push_str(&text);
                    } else if let Some(ref mut builder) = table_builder {
                        builder.push_text(&text);
                    } else {
                        let style = current_style(&style_stack);
                        current_spans.push(Span::styled(text.to_string(), style));
                    }
                }

                Event::Code(code) => {
                    if let Some(ref mut builder) = table_builder {
                        builder.push_text(&format!("`{code}`"));
                    } else {
                        let style = Style::default()
                            .fg(self.theme.inline_code_fg)
                            .bg(self.theme.inline_code_bg);
                        current_spans.push(Span::styled(format!("`{code}`"), style));
                    }
                }

                Event::SoftBreak => {
                    if table_builder.is_none() {
                        current_spans.push(Span::raw(" ".to_string()));
                    }
                }

                Event::HardBreak => {
                    if table_builder.is_none() {
                        flush_line(&mut current_spans, &mut lines);
                    }
                }

                Event::Rule => {
                    flush_line(&mut current_spans, &mut lines);
                    let style = Style::default().fg(self.theme.muted);
                    lines.push(Line::from(Span::styled("─".repeat(40), style)));
                }

                _ => {}
            }
        }

        flush_line(&mut current_spans, &mut lines);

        // Drop any unused variables to silence warnings
        let _ = in_table_head;

        lines
    }
}

// ─── Shared helpers ─────────────────────────────────────────────────────

fn current_style(stack: &[Style]) -> Style {
    stack.last().copied().unwrap_or_default()
}

fn flush_line(spans: &mut Vec<Span<'static>>, lines: &mut Vec<Line<'static>>) {
    if !spans.is_empty() {
        lines.push(Line::from(std::mem::take(spans)));
    }
}

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

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_renderer() -> (Theme, SyntaxHighlighter) {
        (Theme::dark(), SyntaxHighlighter::new())
    }

    fn collect_text(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect()
    }

    // ── RenderOptions ──

    #[test]
    fn render_options_default() {
        let opts = RenderOptions::default();
        assert!(opts.line_numbers);
        assert!(opts.language_label);
        assert!(opts.copy_region);
        assert!(opts.osc8_links);
        assert_eq!(opts.max_table_col_width, 40);
    }

    // ── syntect theme mapping ──

    #[test]
    fn syntect_theme_mapping() {
        assert_eq!(syntect_theme_for(ThemeName::Dark), "base16-ocean.dark");
        assert_eq!(syntect_theme_for(ThemeName::Light), "base16-ocean.light");
        assert_eq!(syntect_theme_for(ThemeName::Monokai), "base16-mocha.dark");
        assert_eq!(syntect_theme_for(ThemeName::Solarized), "Solarized (dark)");
        assert_eq!(syntect_theme_for(ThemeName::Custom), "base16-ocean.dark");
    }

    // ── OSC 8 links ──

    #[test]
    fn osc8_link_format() {
        let result = osc8_link("https://example.com", "click");
        assert!(result.contains("https://example.com"));
        assert!(result.contains("click"));
        assert!(result.starts_with("\x1b]8;;"));
        assert!(result.ends_with("\x1b]8;;\x1b\\"));
    }

    // ── Table rendering ──

    #[test]
    fn table_basic_render() {
        let (theme, hl) = make_renderer();
        let r = EnhancedMarkdownRenderer::new(&theme, &hl);
        let md = "| Name | Age |\n|------|-----|\n| Alice | 30 |\n| Bob | 25 |";
        let lines = r.render(md);
        let text = collect_text(&lines);
        assert!(text.contains("Name"), "Missing header: {text}");
        assert!(text.contains("Age"), "Missing header: {text}");
        assert!(text.contains("Alice"), "Missing data: {text}");
        assert!(text.contains("Bob"), "Missing data: {text}");
    }

    #[test]
    fn table_alignment() {
        let (theme, hl) = make_renderer();
        let r = EnhancedMarkdownRenderer::new(&theme, &hl);
        let md = "| Left | Center | Right |\n|:-----|:------:|------:|\n| a | b | c |";
        let lines = r.render(md);
        let text = collect_text(&lines);
        assert!(text.contains("Left"));
        assert!(text.contains("Center"));
        assert!(text.contains("Right"));
    }

    #[test]
    fn table_empty_cells() {
        let (theme, hl) = make_renderer();
        let r = EnhancedMarkdownRenderer::new(&theme, &hl);
        let md = "| A | B |\n|---|---|\n|  | x |";
        let lines = r.render(md);
        let text = collect_text(&lines);
        assert!(text.contains("x"));
    }

    // ── Code block with line numbers ──

    #[test]
    fn code_block_has_line_numbers() {
        let (theme, hl) = make_renderer();
        let r = EnhancedMarkdownRenderer::new(&theme, &hl);
        let md = "```rust\nfn main() {\n    println!(\"hi\");\n}\n```";
        let lines = r.render(md);
        let text = collect_text(&lines);
        assert!(text.contains("1"), "Missing line number 1: {text}");
        assert!(text.contains("2"), "Missing line number 2: {text}");
        assert!(text.contains("3"), "Missing line number 3: {text}");
    }

    #[test]
    fn code_block_language_label() {
        let (theme, hl) = make_renderer();
        let r = EnhancedMarkdownRenderer::new(&theme, &hl);
        let md = "```python\nprint(1)\n```";
        let lines = r.render(md);
        let text = collect_text(&lines);
        assert!(text.contains("python"), "Missing language label: {text}");
    }

    #[test]
    fn code_block_copy_region() {
        let (theme, hl) = make_renderer();
        let r = EnhancedMarkdownRenderer::new(&theme, &hl);
        let md = "```js\nconsole.log(1)\n```";
        let lines = r.render(md);
        let text = collect_text(&lines);
        assert!(text.contains("[copy]"), "Missing copy region: {text}");
    }

    #[test]
    fn code_block_no_line_numbers() {
        let (theme, hl) = make_renderer();
        let opts = RenderOptions {
            line_numbers: false,
            ..RenderOptions::default()
        };
        let r = EnhancedMarkdownRenderer::with_options(&theme, &hl, opts);
        let md = "```\nhello\n```";
        let lines = r.render(md);
        let text = collect_text(&lines);
        // Should NOT have line number formatting like " 1 │ "
        assert!(
            !text.contains(" 1 │"),
            "Should not have line numbers: {text}"
        );
        assert!(text.contains("hello"));
    }

    #[test]
    fn code_block_no_language_label() {
        let (theme, hl) = make_renderer();
        let opts = RenderOptions {
            language_label: false,
            copy_region: false,
            ..RenderOptions::default()
        };
        let r = EnhancedMarkdownRenderer::with_options(&theme, &hl, opts);
        let md = "```rust\nfn main() {}\n```";
        let lines = r.render(md);
        let text = collect_text(&lines);
        // No header line with "rust" label
        // But code content should be there
        assert!(text.contains("fn"));
    }

    #[test]
    fn code_block_footer_border() {
        let (theme, hl) = make_renderer();
        let r = EnhancedMarkdownRenderer::new(&theme, &hl);
        let md = "```\ntest\n```";
        let lines = r.render(md);
        let text = collect_text(&lines);
        assert!(text.contains("└"), "Missing footer: {text}");
        assert!(text.contains("┘"), "Missing footer: {text}");
    }

    // ── Link rendering ──

    #[test]
    fn link_renders_url() {
        let (theme, hl) = make_renderer();
        let opts = RenderOptions {
            osc8_links: false,
            ..RenderOptions::default()
        };
        let r = EnhancedMarkdownRenderer::with_options(&theme, &hl, opts);
        let md = "[click here](https://example.com)";
        let lines = r.render(md);
        let text = collect_text(&lines);
        assert!(text.contains("click here"), "Missing link text: {text}");
        assert!(text.contains("https://example.com"), "Missing URL: {text}");
    }

    #[test]
    fn link_osc8_format() {
        let (theme, hl) = make_renderer();
        let r = EnhancedMarkdownRenderer::new(&theme, &hl);
        let md = "[test](https://example.com)";
        let lines = r.render(md);
        let text = collect_text(&lines);
        assert!(text.contains("test"), "Missing link text: {text}");
        // OSC 8 escape
        assert!(text.contains("\x1b]8;;"), "Missing OSC 8 escape: {text}");
    }

    // ── Standard markdown elements still work ──

    #[test]
    fn render_heading() {
        let (theme, hl) = make_renderer();
        let r = EnhancedMarkdownRenderer::new(&theme, &hl);
        let lines = r.render("# Title");
        let text = collect_text(&lines);
        assert!(text.contains("# "));
        assert!(text.contains("Title"));
    }

    #[test]
    fn render_bold_italic() {
        let (theme, hl) = make_renderer();
        let r = EnhancedMarkdownRenderer::new(&theme, &hl);
        let lines = r.render("**bold** and *italic*");
        let text = collect_text(&lines);
        assert!(text.contains("bold"));
        assert!(text.contains("italic"));
    }

    #[test]
    fn render_inline_code() {
        let (theme, hl) = make_renderer();
        let r = EnhancedMarkdownRenderer::new(&theme, &hl);
        let lines = r.render("Use `foo()` here");
        let text = collect_text(&lines);
        assert!(text.contains("`foo()`"));
    }

    #[test]
    fn render_unordered_list() {
        let (theme, hl) = make_renderer();
        let r = EnhancedMarkdownRenderer::new(&theme, &hl);
        let lines = r.render("- one\n- two");
        let text = collect_text(&lines);
        assert!(text.contains("- "));
        assert!(text.contains("one"));
    }

    #[test]
    fn render_ordered_list() {
        let (theme, hl) = make_renderer();
        let r = EnhancedMarkdownRenderer::new(&theme, &hl);
        let lines = r.render("1. first\n2. second");
        let text = collect_text(&lines);
        assert!(text.contains("1. "));
        assert!(text.contains("first"));
    }

    #[test]
    fn render_blockquote() {
        let (theme, hl) = make_renderer();
        let r = EnhancedMarkdownRenderer::new(&theme, &hl);
        let lines = r.render("> quoted");
        let text = collect_text(&lines);
        assert!(text.contains("│ "));
        assert!(text.contains("quoted"));
    }

    #[test]
    fn render_horizontal_rule() {
        let (theme, hl) = make_renderer();
        let r = EnhancedMarkdownRenderer::new(&theme, &hl);
        let lines = r.render("---");
        let text = collect_text(&lines);
        assert!(text.contains("─"));
    }

    #[test]
    fn render_empty() {
        let (theme, hl) = make_renderer();
        let r = EnhancedMarkdownRenderer::new(&theme, &hl);
        let lines = r.render("");
        assert!(lines.is_empty());
    }

    // ── Table helpers ──

    #[test]
    fn pad_cell_left() {
        assert_eq!(pad_cell("hi", 5, Alignment::None), "hi   ");
        assert_eq!(pad_cell("hi", 5, Alignment::Left), "hi   ");
    }

    #[test]
    fn pad_cell_right() {
        assert_eq!(pad_cell("hi", 5, Alignment::Right), "   hi");
    }

    #[test]
    fn pad_cell_center() {
        assert_eq!(pad_cell("hi", 6, Alignment::Center), "  hi  ");
    }

    #[test]
    fn pad_cell_exact_width() {
        assert_eq!(pad_cell("hello", 5, Alignment::Left), "hello");
    }

    // ── Table builder ──

    #[test]
    fn table_builder_basic() {
        let mut builder = TableBuilder::new(vec![Alignment::None, Alignment::None]);
        builder.push_text("A");
        builder.finish_cell();
        builder.push_text("B");
        builder.finish_cell();
        builder.finish_row(); // header

        builder.push_text("1");
        builder.finish_cell();
        builder.push_text("2");
        builder.finish_cell();
        builder.finish_row();

        let theme = Theme::dark();
        let lines = builder.render(&theme, 0);
        // header + separator + 1 data row = 3 lines
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn table_builder_empty() {
        let builder = TableBuilder::new(Vec::new());
        let theme = Theme::dark();
        let lines = builder.render(&theme, 0);
        // No columns → no output (header was empty)
        assert!(lines.is_empty());
    }

    // ── Code block rendering helper ──

    #[test]
    fn render_code_block_with_all_options() {
        let theme = Theme::dark();
        let highlighted = vec![
            Line::from(Span::raw("line one")),
            Line::from(Span::raw("line two")),
        ];
        let opts = RenderOptions::default();
        let lines = render_code_block("line one\nline two", "rust", highlighted, &opts, &theme);
        let text = collect_text(&lines);
        assert!(text.contains("rust"), "Missing language label");
        assert!(text.contains("[copy]"), "Missing copy region");
        assert!(text.contains("1"), "Missing line num 1");
        assert!(text.contains("2"), "Missing line num 2");
        assert!(text.contains("└"), "Missing footer");
    }

    #[test]
    fn render_code_block_no_options() {
        let theme = Theme::dark();
        let highlighted = vec![Line::from(Span::raw("hello"))];
        let opts = RenderOptions {
            line_numbers: false,
            language_label: false,
            copy_region: false,
            osc8_links: false,
            max_table_col_width: 0,
        };
        let lines = render_code_block("hello", "", highlighted, &opts, &theme);
        let text = collect_text(&lines);
        assert!(text.contains("hello"));
        assert!(!text.contains("[copy]"));
    }

    // ── Integration: table with inline code ──

    #[test]
    fn table_with_inline_code() {
        let (theme, hl) = make_renderer();
        let r = EnhancedMarkdownRenderer::new(&theme, &hl);
        let md = "| Function | Usage |\n|----------|-------|\n| `foo()` | calls foo |";
        let lines = r.render(md);
        let text = collect_text(&lines);
        assert!(
            text.contains("`foo()`"),
            "Missing inline code in table: {text}"
        );
    }

    // ── Theme integration ──

    #[test]
    fn different_themes_produce_output() {
        let hl = SyntaxHighlighter::new();
        let md = "# Hello\n\n```rust\nfn main() {}\n```\n\n| A |\n|---|\n| 1 |";

        for theme_name in [
            ThemeName::Dark,
            ThemeName::Light,
            ThemeName::Monokai,
            ThemeName::Solarized,
        ] {
            let theme = Theme::by_name(theme_name);
            let r = EnhancedMarkdownRenderer::new(&theme, &hl);
            let lines = r.render(md);
            assert!(!lines.is_empty(), "No output for theme {theme_name}");
        }
    }
}
