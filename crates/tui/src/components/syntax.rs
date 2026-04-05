use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;

use crate::theme::Theme;

/// Syntax highlighter backed by syntect.
///
/// Lazily loads the default syntax set and theme set on construction.
/// Use `highlight()` to convert a code block into styled ratatui `Line`s.
pub struct SyntaxHighlighter {
    syntax_set: SyntaxSet,
    theme_set: ThemeSet,
    /// syntect theme name to use (e.g. "base16-ocean.dark").
    syntect_theme: String,
}

impl SyntaxHighlighter {
    /// Create a highlighter with default syntect bundles.
    #[must_use]
    pub fn new() -> Self {
        Self {
            syntax_set: SyntaxSet::load_defaults_newlines(),
            theme_set: ThemeSet::load_defaults(),
            syntect_theme: "base16-ocean.dark".to_string(),
        }
    }

    /// Create a highlighter that uses a light syntect theme.
    #[must_use]
    pub fn with_light_theme() -> Self {
        Self {
            syntax_set: SyntaxSet::load_defaults_newlines(),
            theme_set: ThemeSet::load_defaults(),
            syntect_theme: "base16-ocean.light".to_string(),
        }
    }

    /// Set the syntect theme name (must exist in the default `ThemeSet`).
    pub fn set_theme(&mut self, name: &str) {
        self.syntect_theme = name.to_string();
    }

    /// Highlight a block of source code.
    ///
    /// `language` should be a file extension (e.g. `"rs"`, `"py"`, `"js"`)
    /// or a language name (e.g. `"Rust"`, `"Python"`).
    /// Falls back to plain text if the language is unknown.
    ///
    /// Returns a `Vec<Line>` suitable for rendering in a ratatui widget.
    pub fn highlight<'a>(&self, code: &'a str, language: &str) -> Vec<Line<'a>> {
        let syntax = self
            .syntax_set
            .find_syntax_by_token(language)
            .or_else(|| self.syntax_set.find_syntax_by_extension(language))
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());

        let theme = self
            .theme_set
            .themes
            .get(&self.syntect_theme)
            .unwrap_or_else(|| {
                self.theme_set
                    .themes
                    .values()
                    .next()
                    .expect("syntect ThemeSet should have at least one theme")
            });

        let mut highlighter = HighlightLines::new(syntax, theme);
        let mut lines = Vec::new();

        for line in code.lines() {
            let regions = highlighter
                .highlight_line(line, &self.syntax_set)
                .unwrap_or_default();

            let spans: Vec<Span<'a>> = regions
                .into_iter()
                .map(|(style, text)| {
                    let fg = syntect_color_to_ratatui(style.foreground);
                    let mut ratatui_style = Style::default().fg(fg);
                    if style
                        .font_style
                        .contains(syntect::highlighting::FontStyle::BOLD)
                    {
                        ratatui_style = ratatui_style.add_modifier(Modifier::BOLD);
                    }
                    if style
                        .font_style
                        .contains(syntect::highlighting::FontStyle::ITALIC)
                    {
                        ratatui_style = ratatui_style.add_modifier(Modifier::ITALIC);
                    }
                    if style
                        .font_style
                        .contains(syntect::highlighting::FontStyle::UNDERLINE)
                    {
                        ratatui_style = ratatui_style.add_modifier(Modifier::UNDERLINED);
                    }
                    // text is &'b str from highlight_line — we need owned for 'a lifetime
                    Span::styled(text.to_string(), ratatui_style)
                })
                .collect();

            lines.push(Line::from(spans));
        }

        lines
    }

    /// Highlight code without syntect — fallback using the TUI `Theme` colors.
    ///
    /// This produces a simple, non-semantic highlight: just returns the code
    /// as plain styled lines using the theme's default fg color.
    pub fn highlight_plain<'a>(code: &'a str, theme: &Theme) -> Vec<Line<'a>> {
        let style = Style::default().fg(theme.fg);
        code.lines()
            .map(|line| Line::from(Span::styled(line.to_string(), style)))
            .collect()
    }

    /// List recognized language extensions.
    #[must_use]
    pub fn supported_extensions(&self) -> Vec<&str> {
        let mut exts: Vec<&str> = self
            .syntax_set
            .syntaxes()
            .iter()
            .flat_map(|s| s.file_extensions.iter().map(String::as_str))
            .collect();
        exts.sort_unstable();
        exts.dedup();
        exts
    }

    /// Check if a language token is recognized.
    #[must_use]
    pub fn supports_language(&self, token: &str) -> bool {
        self.syntax_set.find_syntax_by_token(token).is_some()
            || self.syntax_set.find_syntax_by_extension(token).is_some()
    }
}

impl Default for SyntaxHighlighter {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a syntect `Color` to a ratatui `Color`.
fn syntect_color_to_ratatui(c: syntect::highlighting::Color) -> Color {
    Color::Rgb(c.r, c.g, c.b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_rust_produces_lines() {
        let hl = SyntaxHighlighter::new();
        let code = "fn main() {\n    println!(\"hello\");\n}";
        let lines = hl.highlight(code, "rs");
        assert_eq!(lines.len(), 3);
        // Each line should have at least one span
        for line in &lines {
            assert!(!line.spans.is_empty());
        }
    }

    #[test]
    fn highlight_unknown_language_falls_back() {
        let hl = SyntaxHighlighter::new();
        let code = "some plain text\nsecond line";
        let lines = hl.highlight(code, "zzz_unknown");
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn highlight_plain_uses_theme_fg() {
        let theme = Theme::dark();
        let code = "line one\nline two";
        let lines = SyntaxHighlighter::highlight_plain(code, &theme);
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn supports_common_languages() {
        let hl = SyntaxHighlighter::new();
        assert!(hl.supports_language("rs"));
        assert!(hl.supports_language("py"));
        assert!(hl.supports_language("js"));
        assert!(hl.supports_language("go"));
        assert!(hl.supports_language("java"));
    }

    #[test]
    fn supported_extensions_not_empty() {
        let hl = SyntaxHighlighter::new();
        let exts = hl.supported_extensions();
        assert!(!exts.is_empty());
        assert!(exts.contains(&"rs"));
    }

    #[test]
    fn syntect_color_conversion() {
        let c = syntect::highlighting::Color {
            r: 255,
            g: 128,
            b: 0,
            a: 255,
        };
        assert_eq!(syntect_color_to_ratatui(c), Color::Rgb(255, 128, 0));
    }

    #[test]
    fn light_theme_highlighter() {
        let hl = SyntaxHighlighter::with_light_theme();
        let lines = hl.highlight("let x = 42;", "rs");
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn set_theme_updates_name() {
        let mut hl = SyntaxHighlighter::new();
        hl.set_theme("InspiredGitHub");
        let lines = hl.highlight("fn foo() {}", "rs");
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn empty_code_produces_no_lines() {
        let hl = SyntaxHighlighter::new();
        let lines = hl.highlight("", "rs");
        assert!(lines.is_empty());
    }
}
