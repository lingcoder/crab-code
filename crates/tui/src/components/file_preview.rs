//! File preview widget — displays file content with syntax highlighting and line numbers.
//!
//! Shows a read-only preview of a file's content in a side panel,
//! with file metadata (size, modification time) in a header.

use std::path::PathBuf;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::theme::Theme;

// ─── Types ──────────────────────────────────────────────────────────────

/// Metadata about a file being previewed.
#[derive(Debug, Clone)]
pub struct FileMetadata {
    /// File path (relative to project root).
    pub path: PathBuf,
    /// File size in bytes.
    pub size_bytes: Option<u64>,
    /// Last modified time as a formatted string.
    pub modified: Option<String>,
    /// Detected language/file type.
    pub language: Option<String>,
}

impl FileMetadata {
    /// Create metadata for a file path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            size_bytes: None,
            modified: None,
            language: None,
        }
    }

    /// Set file size.
    #[must_use]
    pub fn with_size(mut self, size: u64) -> Self {
        self.size_bytes = Some(size);
        self
    }

    /// Set modification time.
    #[must_use]
    pub fn with_modified(mut self, modified: impl Into<String>) -> Self {
        self.modified = Some(modified.into());
        self
    }

    /// Set language.
    #[must_use]
    pub fn with_language(mut self, lang: impl Into<String>) -> Self {
        self.language = Some(lang.into());
        self
    }

    /// Get the file name.
    #[must_use]
    pub fn file_name(&self) -> &str {
        self.path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
    }

    /// Format the size as a human-readable string.
    #[must_use]
    pub fn formatted_size(&self) -> Option<String> {
        self.size_bytes.map(format_file_size)
    }

    /// Detect language from file extension.
    #[must_use]
    pub fn detect_language(&self) -> Option<&'static str> {
        let ext = self.path.extension()?.to_str()?;
        Some(match ext.to_lowercase().as_str() {
            "rs" => "Rust",
            "ts" | "tsx" => "TypeScript",
            "js" | "jsx" => "JavaScript",
            "py" => "Python",
            "go" => "Go",
            "java" => "Java",
            "c" | "h" => "C",
            "cpp" | "hpp" | "cc" => "C++",
            "md" => "Markdown",
            "json" => "JSON",
            "toml" => "TOML",
            "yaml" | "yml" => "YAML",
            "html" => "HTML",
            "css" => "CSS",
            "scss" => "SCSS",
            "sh" | "bash" => "Shell",
            "sql" => "SQL",
            "xml" => "XML",
            "txt" => "Text",
            _ => return None,
        })
    }
}

/// Format bytes as human-readable file size.
fn format_file_size(bytes: u64) -> String {
    if bytes >= 1_048_576 {
        #[allow(clippy::cast_precision_loss)]
        let mb = bytes as f64 / 1_048_576.0;
        format!("{mb:.1} MB")
    } else if bytes >= 1024 {
        #[allow(clippy::cast_precision_loss)]
        let kb = bytes as f64 / 1024.0;
        format!("{kb:.1} KB")
    } else {
        format!("{bytes} B")
    }
}

// ─── FilePreview state ──────────────────────────────────────────────────

/// File preview state holding the content and metadata.
pub struct FilePreview {
    /// File metadata.
    metadata: FileMetadata,
    /// File content lines.
    lines: Vec<String>,
    /// Scroll offset (first visible line).
    scroll_offset: usize,
    /// Whether to show line numbers.
    show_line_numbers: bool,
}

impl FilePreview {
    /// Create a new file preview.
    pub fn new(metadata: FileMetadata, content: &str) -> Self {
        let lines: Vec<String> = content.lines().map(String::from).collect();
        Self {
            metadata,
            lines,
            scroll_offset: 0,
            show_line_numbers: true,
        }
    }

    /// Create an empty preview (no file selected).
    #[must_use]
    pub fn empty() -> Self {
        Self {
            metadata: FileMetadata::new(""),
            lines: Vec::new(),
            scroll_offset: 0,
            show_line_numbers: true,
        }
    }

    /// Whether any content is loaded.
    #[must_use]
    pub fn has_content(&self) -> bool {
        !self.lines.is_empty()
    }

    /// Get the metadata.
    #[must_use]
    pub fn metadata(&self) -> &FileMetadata {
        &self.metadata
    }

    /// Get the content lines.
    #[must_use]
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// Total number of lines.
    #[must_use]
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Current scroll offset.
    #[must_use]
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Whether line numbers are shown.
    #[must_use]
    pub fn show_line_numbers(&self) -> bool {
        self.show_line_numbers
    }

    /// Toggle line numbers.
    pub fn toggle_line_numbers(&mut self) {
        self.show_line_numbers = !self.show_line_numbers;
    }

    /// Scroll up by `n` lines.
    pub fn scroll_up(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }

    /// Scroll down by `n` lines.
    pub fn scroll_down(&mut self, n: usize) {
        if !self.lines.is_empty() {
            self.scroll_offset = self
                .scroll_offset
                .saturating_add(n)
                .min(self.lines.len().saturating_sub(1));
        }
    }

    /// Scroll to a specific line.
    pub fn scroll_to(&mut self, line: usize) {
        self.scroll_offset = line.min(self.lines.len().saturating_sub(1));
    }

    /// Load new content.
    pub fn load(&mut self, metadata: FileMetadata, content: &str) {
        self.metadata = metadata;
        self.lines = content.lines().map(String::from).collect();
        self.scroll_offset = 0;
    }

    /// Clear the preview.
    pub fn clear(&mut self) {
        self.metadata = FileMetadata::new("");
        self.lines.clear();
        self.scroll_offset = 0;
    }
}

// ─── Widget ─────────────────────────────────────────────────────────────

/// Widget for rendering the file preview.
pub struct FilePreviewWidget<'a> {
    preview: &'a FilePreview,
    theme: &'a Theme,
}

impl<'a> FilePreviewWidget<'a> {
    #[must_use]
    pub fn new(preview: &'a FilePreview, theme: &'a Theme) -> Self {
        Self { preview, theme }
    }
}

impl Widget for FilePreviewWidget<'_> {
    #[allow(clippy::cast_possible_truncation)]
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 2 || area.width < 10 {
            return;
        }

        if !self.preview.has_content() {
            // Render empty state
            let msg = " No file selected";
            let style = Style::default().fg(self.theme.muted);
            let center_y = area.y + area.height / 2;
            let line = Line::from(Span::styled(msg, style));
            Widget::render(line, Rect::new(area.x, center_y, area.width, 1), buf);
            return;
        }

        // Layout: 1 line header + content body
        let header_y = area.y;
        let body_y = area.y + 1;
        let body_height = area.height.saturating_sub(1) as usize;

        // ─── Header ───
        let file_name = self.preview.metadata().file_name();
        let lang = self
            .preview
            .metadata()
            .language
            .as_deref()
            .or_else(|| self.preview.metadata().detect_language())
            .unwrap_or("text");
        let size_str = self.preview.metadata().formatted_size().unwrap_or_default();
        let modified_str = self.preview.metadata().modified.as_deref().unwrap_or("");

        let header_style = Style::default()
            .fg(self.theme.heading)
            .bg(self.theme.bg)
            .add_modifier(Modifier::BOLD);
        let meta_style = Style::default().fg(self.theme.muted).bg(self.theme.bg);

        // Clear header
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut((x, header_y)) {
                cell.set_char(' ');
                cell.set_style(Style::default().bg(self.theme.bg));
            }
        }

        let mut header_spans = vec![
            Span::styled(format!(" {file_name}"), header_style),
            Span::styled(format!("  [{lang}]"), meta_style),
        ];
        if !size_str.is_empty() {
            header_spans.push(Span::styled(format!("  {size_str}"), meta_style));
        }
        if !modified_str.is_empty() {
            header_spans.push(Span::styled(format!("  {modified_str}"), meta_style));
        }

        let header_line = Line::from(header_spans);
        Widget::render(header_line, Rect::new(area.x, header_y, area.width, 1), buf);

        // ─── Content body ───
        let total_lines = self.preview.line_count();
        let scroll = self.preview.scroll_offset();
        let num_width = if self.preview.show_line_numbers() {
            digit_count(total_lines) + 1 // +1 for the separator space
        } else {
            0
        };

        let line_style = Style::default().fg(self.theme.fg).bg(self.theme.bg);
        let num_style = Style::default().fg(self.theme.muted).bg(self.theme.bg);

        for vi in 0..body_height {
            let line_idx = scroll + vi;
            let y = body_y + vi as u16;

            if y >= area.y + area.height {
                break;
            }

            // Clear the line
            for x in area.x..area.x + area.width {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(' ');
                    cell.set_style(line_style);
                }
            }

            if line_idx >= total_lines {
                // Render tilde for empty lines past EOF
                let tilde_style = Style::default().fg(self.theme.muted);
                if let Some(cell) = buf.cell_mut((area.x, y)) {
                    cell.set_char('~');
                    cell.set_style(tilde_style);
                }
                continue;
            }

            let content = &self.preview.lines()[line_idx];

            let mut spans = Vec::new();

            if self.preview.show_line_numbers() {
                let num_text = format!("{:>width$} ", line_idx + 1, width = num_width - 1);
                spans.push(Span::styled(num_text, num_style));
            }

            // Truncate content to fit
            let available = area.width as usize - num_width;
            let display: String = content.chars().take(available).collect();
            spans.push(Span::styled(display, line_style));

            let line = Line::from(spans);
            Widget::render(line, Rect::new(area.x, y, area.width, 1), buf);
        }
    }
}

/// Count the number of digits in a number.
fn digit_count(n: usize) -> usize {
    if n == 0 {
        return 1;
    }
    let mut count = 0;
    let mut val = n;
    while val > 0 {
        count += 1;
        val /= 10;
    }
    count
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_metadata_new() {
        let meta = FileMetadata::new("src/main.rs");
        assert_eq!(meta.file_name(), "main.rs");
        assert!(meta.size_bytes.is_none());
        assert!(meta.modified.is_none());
    }

    #[test]
    fn file_metadata_with_size() {
        let meta = FileMetadata::new("test.rs").with_size(2048);
        assert_eq!(meta.size_bytes, Some(2048));
        assert_eq!(meta.formatted_size().unwrap(), "2.0 KB");
    }

    #[test]
    fn file_metadata_with_modified() {
        let meta = FileMetadata::new("test.rs").with_modified("2024-01-15 10:30");
        assert_eq!(meta.modified.as_deref(), Some("2024-01-15 10:30"));
    }

    #[test]
    fn file_metadata_with_language() {
        let meta = FileMetadata::new("test.rs").with_language("Rust");
        assert_eq!(meta.language.as_deref(), Some("Rust"));
    }

    #[test]
    fn file_metadata_detect_language() {
        assert_eq!(FileMetadata::new("main.rs").detect_language(), Some("Rust"));
        assert_eq!(
            FileMetadata::new("app.ts").detect_language(),
            Some("TypeScript")
        );
        assert_eq!(
            FileMetadata::new("script.py").detect_language(),
            Some("Python")
        );
        assert_eq!(FileMetadata::new("main.go").detect_language(), Some("Go"));
        assert_eq!(
            FileMetadata::new("data.json").detect_language(),
            Some("JSON")
        );
        assert_eq!(
            FileMetadata::new("config.toml").detect_language(),
            Some("TOML")
        );
        assert_eq!(
            FileMetadata::new("readme.md").detect_language(),
            Some("Markdown")
        );
        assert_eq!(FileMetadata::new("no_ext").detect_language(), None);
        assert_eq!(FileMetadata::new("file.xyz").detect_language(), None);
    }

    #[test]
    fn file_metadata_file_name() {
        assert_eq!(FileMetadata::new("src/lib.rs").file_name(), "lib.rs");
        assert_eq!(FileMetadata::new("Cargo.toml").file_name(), "Cargo.toml");
        assert_eq!(FileMetadata::new("").file_name(), "unknown");
    }

    #[test]
    fn format_file_size_bytes() {
        assert_eq!(format_file_size(500), "500 B");
    }

    #[test]
    fn format_file_size_kb() {
        assert_eq!(format_file_size(2048), "2.0 KB");
    }

    #[test]
    fn format_file_size_mb() {
        assert_eq!(format_file_size(5_242_880), "5.0 MB");
    }

    #[test]
    fn file_preview_new() {
        let meta = FileMetadata::new("test.rs");
        let preview = FilePreview::new(meta, "line 1\nline 2\nline 3");
        assert!(preview.has_content());
        assert_eq!(preview.line_count(), 3);
        assert_eq!(preview.scroll_offset(), 0);
        assert!(preview.show_line_numbers());
    }

    #[test]
    fn file_preview_empty() {
        let preview = FilePreview::empty();
        assert!(!preview.has_content());
        assert_eq!(preview.line_count(), 0);
    }

    #[test]
    fn file_preview_scroll() {
        let meta = FileMetadata::new("test.rs");
        let content = (0..100)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut preview = FilePreview::new(meta, &content);

        assert_eq!(preview.scroll_offset(), 0);

        preview.scroll_down(10);
        assert_eq!(preview.scroll_offset(), 10);

        preview.scroll_up(5);
        assert_eq!(preview.scroll_offset(), 5);

        preview.scroll_up(100); // should clamp to 0
        assert_eq!(preview.scroll_offset(), 0);

        preview.scroll_down(1000); // should clamp to last line
        assert_eq!(preview.scroll_offset(), 99);
    }

    #[test]
    fn file_preview_scroll_to() {
        let meta = FileMetadata::new("test.rs");
        let content = "a\nb\nc\nd\ne";
        let mut preview = FilePreview::new(meta, content);

        preview.scroll_to(3);
        assert_eq!(preview.scroll_offset(), 3);

        preview.scroll_to(100); // clamp
        assert_eq!(preview.scroll_offset(), 4);
    }

    #[test]
    fn file_preview_toggle_line_numbers() {
        let meta = FileMetadata::new("test.rs");
        let mut preview = FilePreview::new(meta, "hello");
        assert!(preview.show_line_numbers());
        preview.toggle_line_numbers();
        assert!(!preview.show_line_numbers());
        preview.toggle_line_numbers();
        assert!(preview.show_line_numbers());
    }

    #[test]
    fn file_preview_load() {
        let meta = FileMetadata::new("old.rs");
        let mut preview = FilePreview::new(meta, "old content");
        preview.scroll_down(5);

        let new_meta = FileMetadata::new("new.rs");
        preview.load(new_meta, "new line 1\nnew line 2");
        assert_eq!(preview.line_count(), 2);
        assert_eq!(preview.scroll_offset(), 0); // reset
        assert_eq!(preview.metadata().file_name(), "new.rs");
    }

    #[test]
    fn file_preview_clear() {
        let meta = FileMetadata::new("test.rs");
        let mut preview = FilePreview::new(meta, "content");
        preview.clear();
        assert!(!preview.has_content());
        assert_eq!(preview.line_count(), 0);
    }

    #[test]
    fn file_preview_lines() {
        let meta = FileMetadata::new("test.rs");
        let preview = FilePreview::new(meta, "alpha\nbeta\ngamma");
        assert_eq!(preview.lines(), &["alpha", "beta", "gamma"]);
    }

    #[test]
    fn digit_count_values() {
        assert_eq!(digit_count(0), 1);
        assert_eq!(digit_count(1), 1);
        assert_eq!(digit_count(9), 1);
        assert_eq!(digit_count(10), 2);
        assert_eq!(digit_count(99), 2);
        assert_eq!(digit_count(100), 3);
        assert_eq!(digit_count(1000), 4);
    }

    #[test]
    fn widget_renders_content() {
        let meta = FileMetadata::new("test.rs")
            .with_size(1024)
            .with_language("Rust");
        let preview = FilePreview::new(meta, "fn main() {\n    println!(\"hello\");\n}");
        let theme = Theme::dark();
        let widget = FilePreviewWidget::new(&preview, &theme);
        let area = Rect::new(0, 0, 50, 10);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);

        // Header should contain file name
        let header: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(header.contains("test.rs"));
        assert!(header.contains("Rust"));

        // Content should have line numbers
        let line1: String = (0..area.width)
            .map(|x| buf.cell((x, 1)).unwrap().symbol().to_string())
            .collect();
        assert!(line1.contains("1"));
        assert!(line1.contains("fn main()"));
    }

    #[test]
    fn widget_renders_empty() {
        let preview = FilePreview::empty();
        let theme = Theme::dark();
        let widget = FilePreviewWidget::new(&preview, &theme);
        let area = Rect::new(0, 0, 50, 10);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);

        // Should show "No file selected" message
        let center_y = area.height / 2;
        let line: String = (0..area.width)
            .map(|x| buf.cell((x, center_y)).unwrap().symbol().to_string())
            .collect();
        assert!(line.contains("No file selected"));
    }

    #[test]
    fn widget_renders_tilde_past_eof() {
        let meta = FileMetadata::new("test.rs");
        let preview = FilePreview::new(meta, "only one line");
        let theme = Theme::dark();
        let widget = FilePreviewWidget::new(&preview, &theme);
        let area = Rect::new(0, 0, 30, 5);
        let mut buf = Buffer::empty(area);
        Widget::render(widget, area, &mut buf);

        // Row after content (y=2, since y=0 is header, y=1 is content) should have tilde
        let row2: String = (0..area.width)
            .map(|x| buf.cell((x, 2)).unwrap().symbol().to_string())
            .collect();
        assert!(row2.contains('~'));
    }

    #[test]
    fn widget_small_area() {
        let meta = FileMetadata::new("test.rs");
        let preview = FilePreview::new(meta, "content");
        let theme = Theme::dark();
        let widget = FilePreviewWidget::new(&preview, &theme);
        let area = Rect::new(0, 0, 5, 1);
        let mut buf = Buffer::empty(area);
        // Should not panic
        Widget::render(widget, area, &mut buf);
    }

    #[test]
    fn file_preview_scroll_empty() {
        let mut preview = FilePreview::empty();
        preview.scroll_down(10); // should not panic
        assert_eq!(preview.scroll_offset(), 0);
    }
}
