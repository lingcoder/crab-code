//! Web content formatting — HTML-to-Markdown conversion and content extraction.
//!
//! Provides `HtmlToMarkdown` for converting common HTML tags to Markdown,
//! `ContentExtractor` for stripping boilerplate (nav, ads, footer), and
//! `truncate_content` for intelligent truncation at paragraph boundaries.

use std::fmt::Write as _;

// ── HTML to Markdown ─────────────────────────────────────────────────

/// Convert HTML to Markdown, handling common tags.
///
/// Supported tags: h1-h6, p, a, img, code, pre, ul, ol, li, table,
/// strong/b, em/i, br, hr, blockquote.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn html_to_markdown(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut chars = html.chars().peekable();
    let mut in_pre = false;
    let mut list_stack: Vec<ListKind> = Vec::new();
    let mut ol_counter: u32 = 0;

    while let Some(ch) = chars.next() {
        if ch == '<' {
            process_tag(
                &mut chars,
                &mut out,
                &mut in_pre,
                &mut list_stack,
                &mut ol_counter,
            );
        } else if ch == '&' {
            decode_entity(&mut chars, &mut out);
        } else {
            out.push(ch);
        }
    }

    collapse_blank_lines(&out)
}

/// Process a single HTML tag and write Markdown to `out`.
fn process_tag(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    out: &mut String,
    in_pre: &mut bool,
    list_stack: &mut Vec<ListKind>,
    ol_counter: &mut u32,
) {
    let tag = collect_tag(chars);
    let tag_lower = tag.to_lowercase();
    let tag_name = extract_tag_name(&tag_lower);
    let is_closing = tag_lower.starts_with('/');
    let clean_name = tag_name.trim_start_matches('/');

    match clean_name {
        "h1" if !is_closing => out.push_str("\n# "),
        "h2" if !is_closing => out.push_str("\n## "),
        "h3" if !is_closing => out.push_str("\n### "),
        "h4" if !is_closing => out.push_str("\n#### "),
        "h5" if !is_closing => out.push_str("\n##### "),
        "h6" if !is_closing => out.push_str("\n###### "),
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "p" | "table" if is_closing => {
            out.push_str("\n\n");
        }
        "p" | "table" if !is_closing => out.push_str("\n\n"),
        "br" => out.push('\n'),
        "hr" => out.push_str("\n---\n\n"),
        "strong" | "b" => out.push_str("**"),
        "em" | "i" => out.push('*'),
        "code" if !*in_pre => out.push('`'),
        "pre" if !is_closing => {
            *in_pre = true;
            out.push_str("\n```\n");
        }
        "pre" if is_closing => {
            *in_pre = false;
            out.push_str("\n```\n\n");
        }
        "blockquote" if !is_closing => out.push_str("\n> "),
        "blockquote" | "li" if is_closing => out.push('\n'),
        "ul" if !is_closing => {
            list_stack.push(ListKind::Unordered);
            out.push('\n');
        }
        "ol" if !is_closing => {
            list_stack.push(ListKind::Ordered);
            *ol_counter = 0;
            out.push('\n');
        }
        "ul" | "ol" if is_closing => {
            list_stack.pop();
            out.push('\n');
        }
        "li" if !is_closing => match list_stack.last() {
            Some(ListKind::Ordered) => {
                *ol_counter += 1;
                let _ = write!(out, "{}. ", *ol_counter);
            }
            _ => out.push_str("- "),
        },
        "a" if !is_closing => {
            if let Some(href) = extract_attr(&tag, "href") {
                out.push('[');
                let link_text = collect_until_closing_tag(chars, "a");
                out.push_str(link_text.trim());
                let _ = write!(out, "]({href})");
            }
        }
        "img" if !is_closing => {
            let alt = extract_attr(&tag, "alt").unwrap_or_default();
            if let Some(src) = extract_attr(&tag, "src") {
                let _ = write!(out, "![{alt}]({src})");
            }
        }
        "tr" if is_closing => out.push_str(" |\n"),
        "th" | "td" if !is_closing => out.push_str("| "),
        "th" | "td" if is_closing => out.push(' '),
        "script" | "style" | "noscript" if !is_closing => {
            skip_until_closing_tag(chars, clean_name);
        }
        _ => {}
    }
}

/// Decode an HTML entity and push the result to `out`.
fn decode_entity(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, out: &mut String) {
    let entity = collect_entity(chars);
    match entity.as_str() {
        "amp" => out.push('&'),
        "lt" => out.push('<'),
        "gt" => out.push('>'),
        "quot" => out.push('"'),
        "apos" => out.push('\''),
        "nbsp" => out.push(' '),
        _ => {
            out.push('&');
            out.push_str(&entity);
            out.push(';');
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum ListKind {
    Ordered,
    Unordered,
}

/// Collect characters until `>` to form a tag string.
fn collect_tag(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut tag = String::new();
    for ch in chars.by_ref() {
        if ch == '>' {
            break;
        }
        tag.push(ch);
    }
    tag
}

/// Extract the tag name from a tag string (e.g., "a href=..." -> "a").
fn extract_tag_name(tag: &str) -> &str {
    let trimmed = tag.trim().trim_start_matches('/');
    trimmed.split_whitespace().next().unwrap_or("")
}

/// Extract an attribute value from a tag string.
fn extract_attr(tag: &str, attr: &str) -> Option<String> {
    let lower = tag.to_lowercase();
    let pattern = format!("{attr}=\"");
    if let Some(start) = lower.find(&pattern) {
        let value_start = start + pattern.len();
        let rest = &tag[value_start..];
        if let Some(end) = rest.find('"') {
            return Some(rest[..end].to_owned());
        }
    }
    // Try single quotes
    let pattern_sq = format!("{attr}='");
    if let Some(start) = lower.find(&pattern_sq) {
        let value_start = start + pattern_sq.len();
        let rest = &tag[value_start..];
        if let Some(end) = rest.find('\'') {
            return Some(rest[..end].to_owned());
        }
    }
    None
}

/// Collect text until the closing tag for the given tag name.
fn collect_until_closing_tag(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    tag_name: &str,
) -> String {
    let mut text = String::new();
    let closing = format!("/{tag_name}");
    while let Some(ch) = chars.next() {
        if ch == '<' {
            let inner_tag = collect_tag(chars);
            if inner_tag.to_lowercase().trim() == closing {
                break;
            }
            // Nested tag — just skip it
        } else {
            text.push(ch);
        }
    }
    text
}

/// Skip all content until the closing tag.
fn skip_until_closing_tag(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, tag_name: &str) {
    let closing = format!("/{tag_name}");
    while let Some(ch) = chars.next() {
        if ch == '<' {
            let inner_tag = collect_tag(chars);
            if inner_tag.to_lowercase().trim() == closing {
                break;
            }
        }
    }
}

/// Collect an HTML entity name (text between & and ;).
fn collect_entity(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut entity = String::new();
    for ch in chars.by_ref() {
        if ch == ';' {
            break;
        }
        entity.push(ch);
        if !(ch.is_alphanumeric() || ch == '#') {
            // Not a valid entity, return what we have
            break;
        }
    }
    entity
}

/// Collapse runs of 3+ blank lines into 2.
fn collapse_blank_lines(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut blank_count = 0;
    for line in text.lines() {
        if line.trim().is_empty() {
            blank_count += 1;
            if blank_count <= 1 {
                result.push('\n');
            }
        } else {
            blank_count = 0;
            result.push_str(line);
            result.push('\n');
        }
    }
    result.trim().to_owned()
}

// ── Content Extraction ───────────────────────────────────────────────

/// Tags considered boilerplate that should be stripped entirely.
const BOILERPLATE_TAGS: &[&str] = &[
    "nav", "header", "footer", "aside", "script", "style", "noscript", "iframe", "form",
];

/// Extract main content from HTML, removing boilerplate elements.
#[must_use]
pub fn extract_content(html: &str) -> String {
    let mut cleaned = html.to_owned();

    // Remove boilerplate tags and their contents
    for tag in BOILERPLATE_TAGS {
        cleaned = remove_tag_with_content(&cleaned, tag);
    }

    // Remove HTML comments
    while let Some(start) = cleaned.find("<!--") {
        if let Some(end) = cleaned[start..].find("-->") {
            cleaned = format!("{}{}", &cleaned[..start], &cleaned[start + end + 3..]);
        } else {
            break;
        }
    }

    html_to_markdown(&cleaned)
}

/// Remove all occurrences of a tag and its content from HTML.
fn remove_tag_with_content(html: &str, tag: &str) -> String {
    let mut result = html.to_owned();
    let open_pattern = format!("<{tag}");
    let close_pattern = format!("</{tag}>");

    loop {
        let lower = result.to_lowercase();
        let Some(start) = lower.find(&open_pattern) else {
            break;
        };
        // Find the end of the opening tag
        let Some(tag_end) = result[start..].find('>') else {
            break;
        };
        // Find closing tag
        let search_start = start + tag_end + 1;
        if let Some(close_pos) = lower[search_start..].find(&close_pattern) {
            let end = search_start + close_pos + close_pattern.len();
            result = format!("{}{}", &result[..start], &result[end..]);
        } else {
            // Self-closing or no matching close — remove just the opening tag
            result = format!("{}{}", &result[..start], &result[start + tag_end + 1..]);
        }
    }

    result
}

// ── Truncation ───────────────────────────────────────────────────────

/// Truncate content to at most `max_chars`, breaking at paragraph boundaries
/// to preserve readability. Returns the original if already within limits.
#[must_use]
pub fn truncate_content(content: &str, max_chars: usize) -> String {
    if content.len() <= max_chars {
        return content.to_owned();
    }

    // Try to break at a paragraph boundary (double newline)
    let search_region = &content[..max_chars];
    if let Some(pos) = search_region.rfind("\n\n")
        && pos > max_chars / 2
    {
        return format!("{}\n\n[Content truncated]", &content[..pos]);
    }

    // Fall back to breaking at a single newline
    if let Some(pos) = search_region.rfind('\n')
        && pos > max_chars / 2
    {
        return format!("{}\n\n[Content truncated]", &content[..pos]);
    }

    // Last resort: break at a space
    if let Some(pos) = search_region.rfind(' ') {
        return format!("{}\n\n[Content truncated]", &content[..pos]);
    }

    // Absolute last resort: hard cut
    format!("{}\n\n[Content truncated]", &content[..max_chars])
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── html_to_markdown tests ──

    #[test]
    fn headings_convert() {
        let html = "<h1>Title</h1><h2>Subtitle</h2><h3>Section</h3>";
        let md = html_to_markdown(html);
        assert!(md.contains("# Title"));
        assert!(md.contains("## Subtitle"));
        assert!(md.contains("### Section"));
    }

    #[test]
    fn paragraphs_convert() {
        let html = "<p>First paragraph.</p><p>Second paragraph.</p>";
        let md = html_to_markdown(html);
        assert!(md.contains("First paragraph."));
        assert!(md.contains("Second paragraph."));
    }

    #[test]
    fn bold_and_italic() {
        let html = "<strong>bold</strong> and <em>italic</em>";
        let md = html_to_markdown(html);
        assert!(md.contains("**bold**"));
        assert!(md.contains("*italic*"));
    }

    #[test]
    fn inline_code() {
        let html = "Use <code>println!</code> to print";
        let md = html_to_markdown(html);
        assert!(md.contains("`println!`"));
    }

    #[test]
    fn code_block() {
        let html = "<pre><code>fn main() {}</code></pre>";
        let md = html_to_markdown(html);
        assert!(md.contains("```"));
        assert!(md.contains("fn main() {}"));
    }

    #[test]
    fn links_convert() {
        let html = r#"<a href="https://example.com">Example</a>"#;
        let md = html_to_markdown(html);
        assert!(md.contains("[Example](https://example.com)"));
    }

    #[test]
    fn images_convert() {
        let html = r#"<img src="pic.png" alt="A picture">"#;
        let md = html_to_markdown(html);
        assert!(md.contains("![A picture](pic.png)"));
    }

    #[test]
    fn unordered_list() {
        let html = "<ul><li>One</li><li>Two</li></ul>";
        let md = html_to_markdown(html);
        assert!(md.contains("- One"));
        assert!(md.contains("- Two"));
    }

    #[test]
    fn ordered_list() {
        let html = "<ol><li>First</li><li>Second</li></ol>";
        let md = html_to_markdown(html);
        assert!(md.contains("1. First"));
        assert!(md.contains("2. Second"));
    }

    #[test]
    fn hr_converts() {
        let html = "Before<hr>After";
        let md = html_to_markdown(html);
        assert!(md.contains("---"));
    }

    #[test]
    fn br_converts() {
        let html = "Line 1<br>Line 2";
        let md = html_to_markdown(html);
        assert!(md.contains("Line 1\nLine 2"));
    }

    #[test]
    fn blockquote_converts() {
        let html = "<blockquote>A quote</blockquote>";
        let md = html_to_markdown(html);
        assert!(md.contains("> A quote"));
    }

    #[test]
    fn html_entities_decoded() {
        let html = "&amp; &lt; &gt; &quot; &apos; &nbsp;";
        let md = html_to_markdown(html);
        assert!(md.contains("& < > \" '"));
    }

    #[test]
    fn script_tags_stripped() {
        let html = "<p>Visible</p><script>alert('xss')</script><p>Also visible</p>";
        let md = html_to_markdown(html);
        assert!(md.contains("Visible"));
        assert!(md.contains("Also visible"));
        assert!(!md.contains("alert"));
    }

    #[test]
    #[allow(clippy::literal_string_with_formatting_args)] // {display:none} is CSS, not a format arg
    fn style_tags_stripped() {
        let html = "<style>.hide{display:none}</style><p>Content</p>";
        let md = html_to_markdown(html);
        assert!(md.contains("Content"));
        assert!(!md.contains("display:none"));
    }

    #[test]
    fn table_basic() {
        let html =
            "<table><tr><th>Name</th><th>Age</th></tr><tr><td>Alice</td><td>30</td></tr></table>";
        let md = html_to_markdown(html);
        assert!(md.contains("Name"));
        assert!(md.contains("Alice"));
        assert!(md.contains('|'));
    }

    #[test]
    fn empty_html() {
        assert!(html_to_markdown("").is_empty());
    }

    #[test]
    fn plain_text_passthrough() {
        let md = html_to_markdown("Just plain text");
        assert_eq!(md, "Just plain text");
    }

    // ── extract_content tests ──

    #[test]
    fn extract_removes_nav() {
        let html = "<nav><a href='/'>Home</a></nav><p>Main content</p>";
        let md = extract_content(html);
        assert!(md.contains("Main content"));
        assert!(!md.contains("Home"));
    }

    #[test]
    fn extract_removes_footer() {
        let html = "<p>Article text</p><footer>Copyright 2024</footer>";
        let md = extract_content(html);
        assert!(md.contains("Article text"));
        assert!(!md.contains("Copyright"));
    }

    #[test]
    fn extract_removes_comments() {
        let html = "<p>Real</p><!-- This is a comment --><p>Content</p>";
        let md = extract_content(html);
        assert!(md.contains("Real"));
        assert!(md.contains("Content"));
        assert!(!md.contains("comment"));
    }

    #[test]
    fn extract_removes_aside() {
        let html = "<aside>Sidebar</aside><main><p>Main</p></main>";
        let md = extract_content(html);
        assert!(md.contains("Main"));
        assert!(!md.contains("Sidebar"));
    }

    // ── truncate_content tests ──

    #[test]
    fn truncate_short_content_unchanged() {
        let text = "Short content";
        assert_eq!(truncate_content(text, 100), text);
    }

    #[test]
    fn truncate_at_paragraph_boundary() {
        let text = "First paragraph.\n\nSecond paragraph.\n\nThird paragraph which is much longer.";
        let result = truncate_content(text, 40);
        assert!(result.contains("First paragraph."));
        assert!(result.contains("[Content truncated]"));
        assert!(!result.contains("Third"));
    }

    #[test]
    fn truncate_at_newline_fallback() {
        let text = "Line one\nLine two\nLine three which is much longer and goes over the limit";
        let result = truncate_content(text, 30);
        assert!(result.contains("[Content truncated]"));
    }

    #[test]
    fn truncate_at_space_fallback() {
        let text = "One Two Three Four Five Six Seven Eight Nine Ten Eleven Twelve";
        let result = truncate_content(text, 20);
        assert!(result.contains("[Content truncated]"));
        assert!(result.len() < text.len());
    }

    #[test]
    fn truncate_hard_cut() {
        let text = "aaaaaaaaaaaaaaaaaaaabbbbbbbbbbbbbbbbbbbb";
        let result = truncate_content(text, 20);
        assert!(result.contains("[Content truncated]"));
    }

    // ── Helper tests ──

    #[test]
    fn extract_attr_double_quotes() {
        assert_eq!(
            extract_attr(r#"a href="https://test.com" class="link""#, "href"),
            Some("https://test.com".into())
        );
    }

    #[test]
    fn extract_attr_single_quotes() {
        assert_eq!(
            extract_attr("a href='https://test.com'", "href"),
            Some("https://test.com".into())
        );
    }

    #[test]
    fn extract_attr_missing() {
        assert_eq!(extract_attr("a class='link'", "href"), None);
    }

    #[test]
    fn extract_tag_name_simple() {
        assert_eq!(extract_tag_name("div"), "div");
        assert_eq!(extract_tag_name("div class='x'"), "div");
        assert_eq!(extract_tag_name("/div"), "div");
    }

    #[test]
    fn collapse_blank_lines_limits() {
        let text = "A\n\n\n\n\nB";
        let result = collapse_blank_lines(text);
        assert!(!result.contains("\n\n\n"));
    }

    #[test]
    fn remove_tag_with_content_basic() {
        let html = "<div>keep</div><nav>remove</nav><div>keep2</div>";
        let result = remove_tag_with_content(html, "nav");
        assert!(result.contains("keep"));
        assert!(result.contains("keep2"));
        assert!(!result.contains("remove"));
    }

    #[test]
    fn remove_tag_case_insensitive() {
        let html = "<NAV>content</NAV><p>keep</p>";
        let result = remove_tag_with_content(html, "nav");
        assert!(result.contains("keep"));
        assert!(!result.contains("content"));
    }
}
