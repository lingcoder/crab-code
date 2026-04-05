//! Code block detection and clipboard copy support.
//!
//! Detects fenced code blocks (triple backticks) in content and allows copying with `y`.

/// A detected code block in the content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeBlock {
    /// Starting line index (inclusive, the opening fence line).
    pub start_line: usize,
    /// Ending line index (inclusive, the closing fence line).
    pub end_line: usize,
    /// Language tag from the opening fence (if any).
    pub language: Option<String>,
    /// The code content (without fences).
    pub content: String,
}

impl CodeBlock {
    /// Check if a line number falls within this code block.
    #[must_use]
    pub fn contains_line(&self, line: usize) -> bool {
        line >= self.start_line && line <= self.end_line
    }

    /// Number of lines in the code block (including fences).
    #[must_use]
    pub fn line_count(&self) -> usize {
        self.end_line - self.start_line + 1
    }
}

/// Detect all fenced code blocks in the content.
///
/// Recognizes blocks delimited by triple backtick fences (with optional language tag).
#[must_use]
pub fn detect_code_blocks(content: &str) -> Vec<CodeBlock> {
    let mut blocks = Vec::new();
    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let trimmed = lines[i].trim();
        if trimmed.starts_with("```") {
            let lang = trimmed.strip_prefix("```").map(|s| s.trim().to_string());
            let language = lang.filter(|s| !s.is_empty());
            let start = i;
            i += 1;

            // Find closing fence
            let mut code_lines = Vec::new();
            while i < lines.len() {
                if lines[i].trim() == "```" {
                    break;
                }
                code_lines.push(lines[i]);
                i += 1;
            }

            let end = if i < lines.len() { i } else { lines.len() - 1 };
            blocks.push(CodeBlock {
                start_line: start,
                end_line: end,
                language,
                content: code_lines.join("\n"),
            });
        }
        i += 1;
    }

    blocks
}

/// Find the code block containing a given line number.
#[must_use]
pub fn code_block_at_line(blocks: &[CodeBlock], line: usize) -> Option<&CodeBlock> {
    blocks.iter().find(|b| b.contains_line(line))
}

/// Manages code block navigation and copy state.
#[derive(Debug, Clone)]
pub struct CodeBlockTracker {
    /// Detected code blocks.
    blocks: Vec<CodeBlock>,
    /// Currently focused block index.
    focused: Option<usize>,
}

impl CodeBlockTracker {
    /// Create an empty tracker.
    #[must_use]
    pub fn new() -> Self {
        Self {
            blocks: Vec::new(),
            focused: None,
        }
    }

    /// Update blocks from content.
    pub fn update(&mut self, content: &str) {
        self.blocks = detect_code_blocks(content);
        // Reset focus if out of range
        if let Some(idx) = self.focused
            && idx >= self.blocks.len()
        {
            self.focused = None;
        }
    }

    /// Get all detected blocks.
    #[must_use]
    pub fn blocks(&self) -> &[CodeBlock] {
        &self.blocks
    }

    /// Number of code blocks.
    #[must_use]
    pub fn count(&self) -> usize {
        self.blocks.len()
    }

    /// Currently focused block index.
    #[must_use]
    pub fn focused(&self) -> Option<usize> {
        self.focused
    }

    /// Get the focused block.
    #[must_use]
    pub fn focused_block(&self) -> Option<&CodeBlock> {
        self.focused.and_then(|i| self.blocks.get(i))
    }

    /// Focus the block containing the given line.
    pub fn focus_at_line(&mut self, line: usize) {
        self.focused = self.blocks.iter().position(|b| b.contains_line(line));
    }

    /// Move focus to the next code block.
    pub fn focus_next(&mut self) {
        if self.blocks.is_empty() {
            return;
        }
        self.focused = Some(match self.focused {
            Some(i) if i + 1 < self.blocks.len() => i + 1,
            _ => 0,
        });
    }

    /// Move focus to the previous code block.
    pub fn focus_prev(&mut self) {
        if self.blocks.is_empty() {
            return;
        }
        self.focused = Some(match self.focused {
            Some(0) | None => self.blocks.len() - 1,
            Some(i) => i - 1,
        });
    }

    /// Get the content of the focused block for copying.
    #[must_use]
    pub fn copy_focused(&self) -> Option<String> {
        self.focused_block().map(|b| b.content.clone())
    }
}

impl Default for CodeBlockTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Image content placeholder info.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImagePlaceholder {
    /// Image media type (e.g. "image/png").
    pub media_type: String,
    /// Width in pixels, if known.
    pub width: Option<u32>,
    /// Height in pixels, if known.
    pub height: Option<u32>,
    /// File size in bytes, if known.
    pub size_bytes: Option<u64>,
}

impl ImagePlaceholder {
    /// Create a new image placeholder.
    pub fn new(media_type: impl Into<String>) -> Self {
        Self {
            media_type: media_type.into(),
            width: None,
            height: None,
            size_bytes: None,
        }
    }

    /// Set dimensions.
    #[must_use]
    pub fn with_dimensions(mut self, width: u32, height: u32) -> Self {
        self.width = Some(width);
        self.height = Some(height);
        self
    }

    /// Set file size.
    #[must_use]
    pub fn with_size(mut self, size_bytes: u64) -> Self {
        self.size_bytes = Some(size_bytes);
        self
    }

    /// Format as display string.
    #[must_use]
    pub fn display_text(&self) -> String {
        let mut parts = vec![format!("[Image: {}]", self.media_type)];

        if let (Some(w), Some(h)) = (self.width, self.height) {
            parts.push(format!("{w}x{h}"));
        }

        if let Some(size) = self.size_bytes {
            parts.push(format_file_size(size));
        }

        parts.join(" ")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_simple_code_block() {
        let content = "some text\n```rust\nfn main() {}\n```\nmore text";
        let blocks = detect_code_blocks(content);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].start_line, 1);
        assert_eq!(blocks[0].end_line, 3);
        assert_eq!(blocks[0].language.as_deref(), Some("rust"));
        assert_eq!(blocks[0].content, "fn main() {}");
    }

    #[test]
    fn detect_no_language() {
        let content = "```\nhello\n```";
        let blocks = detect_code_blocks(content);
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].language.is_none());
        assert_eq!(blocks[0].content, "hello");
    }

    #[test]
    fn detect_multiple_blocks() {
        let content = "```python\nprint(1)\n```\ntext\n```js\nconsole.log(2)\n```";
        let blocks = detect_code_blocks(content);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].language.as_deref(), Some("python"));
        assert_eq!(blocks[1].language.as_deref(), Some("js"));
    }

    #[test]
    fn detect_empty_block() {
        let content = "```\n```";
        let blocks = detect_code_blocks(content);
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].content.is_empty());
    }

    #[test]
    fn detect_unclosed_block() {
        let content = "```rust\nfn main() {}";
        let blocks = detect_code_blocks(content);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].content, "fn main() {}");
    }

    #[test]
    fn detect_no_blocks() {
        let content = "just plain text\nno code here";
        let blocks = detect_code_blocks(content);
        assert!(blocks.is_empty());
    }

    #[test]
    fn code_block_contains_line() {
        let block = CodeBlock {
            start_line: 5,
            end_line: 10,
            language: None,
            content: String::new(),
        };
        assert!(block.contains_line(5));
        assert!(block.contains_line(7));
        assert!(block.contains_line(10));
        assert!(!block.contains_line(4));
        assert!(!block.contains_line(11));
    }

    #[test]
    fn code_block_line_count() {
        let block = CodeBlock {
            start_line: 0,
            end_line: 4,
            language: None,
            content: String::new(),
        };
        assert_eq!(block.line_count(), 5);
    }

    #[test]
    fn code_block_at_line_found() {
        let content = "text\n```\ncode\n```\nmore";
        let blocks = detect_code_blocks(content);
        assert!(code_block_at_line(&blocks, 2).is_some());
        assert!(code_block_at_line(&blocks, 0).is_none());
    }

    #[test]
    fn tracker_new_empty() {
        let tracker = CodeBlockTracker::new();
        assert_eq!(tracker.count(), 0);
        assert!(tracker.focused().is_none());
    }

    #[test]
    fn tracker_update() {
        let mut tracker = CodeBlockTracker::new();
        tracker.update("```\nhello\n```\ntext\n```\nworld\n```");
        assert_eq!(tracker.count(), 2);
    }

    #[test]
    fn tracker_focus_next_prev() {
        let mut tracker = CodeBlockTracker::new();
        tracker.update("```\na\n```\n```\nb\n```\n```\nc\n```");
        assert_eq!(tracker.count(), 3);

        tracker.focus_next();
        assert_eq!(tracker.focused(), Some(0));

        tracker.focus_next();
        assert_eq!(tracker.focused(), Some(1));

        tracker.focus_next();
        assert_eq!(tracker.focused(), Some(2));

        tracker.focus_next(); // wraps
        assert_eq!(tracker.focused(), Some(0));

        tracker.focus_prev(); // wraps back
        assert_eq!(tracker.focused(), Some(2));
    }

    #[test]
    fn tracker_focus_at_line() {
        let mut tracker = CodeBlockTracker::new();
        tracker.update("text\n```\ncode\n```\nmore");
        tracker.focus_at_line(2); // inside block
        assert_eq!(tracker.focused(), Some(0));

        tracker.focus_at_line(0); // outside block
        assert!(tracker.focused().is_none());
    }

    #[test]
    fn tracker_copy_focused() {
        let mut tracker = CodeBlockTracker::new();
        tracker.update("```rust\nfn main() {}\n```");
        tracker.focus_next();
        let copied = tracker.copy_focused();
        assert_eq!(copied.as_deref(), Some("fn main() {}"));
    }

    #[test]
    fn tracker_copy_no_focus() {
        let tracker = CodeBlockTracker::new();
        assert!(tracker.copy_focused().is_none());
    }

    #[test]
    fn tracker_update_resets_out_of_range_focus() {
        let mut tracker = CodeBlockTracker::new();
        tracker.update("```\na\n```\n```\nb\n```");
        tracker.focus_next();
        tracker.focus_next(); // focused = 1

        tracker.update("```\nonly one\n```");
        // Focus should reset since idx 1 is now out of range
        assert!(tracker.focused().is_none());
    }

    #[test]
    fn image_placeholder_basic() {
        let img = ImagePlaceholder::new("image/png");
        assert_eq!(img.display_text(), "[Image: image/png]");
    }

    #[test]
    fn image_placeholder_with_dimensions() {
        let img = ImagePlaceholder::new("image/jpeg").with_dimensions(1920, 1080);
        let text = img.display_text();
        assert!(text.contains("1920x1080"));
        assert!(text.contains("image/jpeg"));
    }

    #[test]
    fn image_placeholder_with_size() {
        let img = ImagePlaceholder::new("image/png").with_size(2_500_000);
        let text = img.display_text();
        assert!(text.contains("2.4 MB"));
    }

    #[test]
    fn image_placeholder_full() {
        let img = ImagePlaceholder::new("image/webp")
            .with_dimensions(800, 600)
            .with_size(150_000);
        let text = img.display_text();
        assert!(text.contains("image/webp"));
        assert!(text.contains("800x600"));
        assert!(text.contains("146.5 KB"));
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
    fn default_tracker() {
        let tracker = CodeBlockTracker::default();
        assert_eq!(tracker.count(), 0);
    }

    #[test]
    fn tracker_focused_block() {
        let mut tracker = CodeBlockTracker::new();
        tracker.update("```\nhello\n```");
        assert!(tracker.focused_block().is_none());
        tracker.focus_next();
        assert!(tracker.focused_block().is_some());
        assert_eq!(tracker.focused_block().unwrap().content, "hello");
    }
}
