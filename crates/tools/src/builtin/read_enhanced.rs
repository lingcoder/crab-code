//! Enhanced read capabilities for `ReadTool`.
//!
//! Provides:
//! - `SmartRange` — expand a line range to cover full function/class boundaries
//! - `BinaryDetector` — detect binary content and return a friendly message
//! - `PdfReader` — skeleton interface for PDF text extraction with plain-text fallback
//! - `ImageMetadata` — extract basic image information (dimensions, format, size)

use std::fmt::Write as _;
use std::path::Path;

// ── SmartRange — function/class boundary expansion ───────────────────

/// Scope boundary detected in source code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeBoundary {
    /// 1-based start line of the scope.
    pub start: usize,
    /// 1-based end line of the scope (inclusive).
    pub end: usize,
    /// The kind of scope (function, class, struct, impl, etc.).
    pub kind: &'static str,
    /// The name or signature snippet of the scope.
    pub name: String,
}

/// Patterns that open a code scope, with the label for that kind.
const SCOPE_OPENERS: &[(&str, &str)] = &[
    ("fn ", "function"),
    ("pub fn ", "function"),
    ("pub(crate) fn ", "function"),
    ("async fn ", "function"),
    ("pub async fn ", "function"),
    ("struct ", "struct"),
    ("pub struct ", "struct"),
    ("enum ", "enum"),
    ("pub enum ", "enum"),
    ("impl ", "impl"),
    ("trait ", "trait"),
    ("pub trait ", "trait"),
    ("class ", "class"),
    ("def ", "function"),
    ("function ", "function"),
    ("export function ", "function"),
    ("export default function ", "function"),
    ("export class ", "class"),
    ("interface ", "interface"),
    ("export interface ", "interface"),
];

/// Find scope boundaries in a source file.
///
/// Returns a list of `ScopeBoundary` values detected by matching brace pairs
/// after lines that contain a scope-opening keyword.
#[must_use]
pub fn find_scope_boundaries(content: &str) -> Vec<ScopeBoundary> {
    let lines: Vec<&str> = content.lines().collect();
    let mut boundaries = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let Some((kind, name)) = detect_scope_opener(trimmed) else {
            continue;
        };

        // Find the matching closing brace
        if let Some(end) = find_closing_brace(&lines, i) {
            boundaries.push(ScopeBoundary {
                start: i + 1, // 1-based
                end: end + 1, // 1-based
                kind,
                name,
            });
        }
    }

    boundaries
}

/// Detect if a line starts a scope, returning `(kind, name_snippet)`.
fn detect_scope_opener(trimmed: &str) -> Option<(&'static str, String)> {
    // Check decorators/attributes — skip them
    if trimmed.starts_with('#') || trimmed.starts_with('@') {
        return None;
    }

    for &(pattern, kind) in SCOPE_OPENERS {
        if let Some(rest) = trimmed.strip_prefix(pattern) {
            let name = rest
                .split(&['(', '{', '<', ':'][..])
                .next()
                .unwrap_or("")
                .trim()
                .to_owned();
            return Some((kind, name));
        }
    }
    None
}

/// Find the line index of the closing brace that matches the first `{` on
/// or after `start_line`.
fn find_closing_brace(lines: &[&str], start_line: usize) -> Option<usize> {
    let mut depth: i32 = 0;
    let mut found_open = false;

    for (i, line) in lines.iter().enumerate().skip(start_line) {
        for ch in line.chars() {
            if ch == '{' {
                depth += 1;
                found_open = true;
            } else if ch == '}' {
                depth -= 1;
                if found_open && depth == 0 {
                    return Some(i);
                }
            }
        }
    }
    None
}

/// Expand a requested line range to cover the enclosing scope boundaries.
///
/// Given an `offset` (1-based) and `limit`, this finds any scope that
/// overlaps the requested range and expands to include it fully.
/// Returns `(new_offset, new_limit)` — both 1-based.
#[must_use]
pub fn expand_to_scope(content: &str, offset: usize, limit: usize) -> (usize, usize) {
    let boundaries = find_scope_boundaries(content);
    let req_start = offset;
    let req_end = offset + limit.saturating_sub(1);

    let mut expanded_start = req_start;
    let mut expanded_end = req_end;

    for b in &boundaries {
        // If the requested range overlaps with this scope, expand
        if b.start <= req_end && b.end >= req_start {
            expanded_start = expanded_start.min(b.start);
            expanded_end = expanded_end.max(b.end);
        }
    }

    (expanded_start, expanded_end - expanded_start + 1)
}

// ── BinaryDetector — detect binary content ──────────────────────────

/// Result of binary content detection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentKind {
    /// File appears to be text.
    Text,
    /// File appears to be binary, with a friendly description.
    Binary {
        /// Human-readable reason (e.g. "ELF executable", "contains null bytes").
        reason: String,
    },
}

/// Magic byte signatures for common binary formats.
const MAGIC_SIGNATURES: &[(&[u8], &str)] = &[
    (b"\x89PNG", "PNG image"),
    (b"\xFF\xD8\xFF", "JPEG image"),
    (b"GIF87a", "GIF image"),
    (b"GIF89a", "GIF image"),
    (b"RIFF", "RIFF container (WAV/AVI/WebP)"),
    (b"PK\x03\x04", "ZIP archive (or DOCX/XLSX/JAR)"),
    (b"%PDF", "PDF document"),
    (b"\x7FELF", "ELF executable"),
    (b"MZ", "Windows executable (PE/MZ)"),
    (b"\xCA\xFE\xBA\xBE", "Mach-O / Java class"),
    (b"\x1F\x8B", "gzip compressed"),
    (b"BZh", "bzip2 compressed"),
    (b"\xFD7zXZ", "xz compressed"),
    (b"7z\xBC\xAF", "7-Zip archive"),
    (b"\x00\x00\x01\x00", "ICO image"),
    (b"OggS", "Ogg container"),
    (b"fLaC", "FLAC audio"),
    (b"ID3", "MP3 audio (ID3 tag)"),
];

/// Check whether a byte slice looks like binary content.
///
/// Inspects up to the first 8192 bytes. Returns `ContentKind::Binary`
/// if the content matches a known magic signature or contains null bytes.
#[must_use]
pub fn detect_binary(data: &[u8]) -> ContentKind {
    if data.is_empty() {
        return ContentKind::Text;
    }

    let sample = if data.len() > 8192 {
        &data[..8192]
    } else {
        data
    };

    // Check magic signatures
    for &(magic, desc) in MAGIC_SIGNATURES {
        if sample.starts_with(magic) {
            return ContentKind::Binary {
                reason: desc.to_owned(),
            };
        }
    }

    // Check for null bytes (strong binary indicator)
    #[allow(clippy::naive_bytecount)]
    let null_count = sample.iter().filter(|&&b| b == 0).count();
    if null_count > 0 {
        return ContentKind::Binary {
            reason: format!(
                "contains {null_count} null byte(s) in first {} bytes",
                sample.len()
            ),
        };
    }

    // Check for high proportion of non-text bytes
    let non_text = sample
        .iter()
        .filter(|&&b| b < 0x07 || (b > 0x0D && b < 0x20 && b != 0x1B))
        .count();
    #[allow(clippy::cast_precision_loss)]
    let ratio = non_text as f64 / sample.len() as f64;
    if ratio > 0.10 {
        return ContentKind::Binary {
            reason: format!(
                "{:.0}% non-text bytes in first {} bytes",
                ratio * 100.0,
                sample.len()
            ),
        };
    }

    ContentKind::Text
}

/// Format a friendly message for binary files.
#[must_use]
pub fn binary_file_message(path: &Path, kind: &ContentKind) -> String {
    match kind {
        ContentKind::Text => String::new(),
        ContentKind::Binary { reason } => {
            format!(
                "Binary file: {}\nDetected as: {reason}\n\
                 This file cannot be displayed as text. \
                 Use file_type or image_read for more details.",
                path.display()
            )
        }
    }
}

// ── PdfReader — skeleton for PDF text extraction ────────────────────

/// Extracted text from a PDF page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfPage {
    /// 1-based page number.
    pub number: usize,
    /// Extracted text content (may be empty if extraction fails).
    pub text: String,
}

/// Result of attempting to read a PDF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PdfReadResult {
    /// Successfully extracted text from the PDF.
    Success { pages: Vec<PdfPage>, total: usize },
    /// PDF reading is not available — provides a fallback message.
    Unavailable { message: String },
}

/// Attempt to extract text from a PDF file.
///
/// This is currently a skeleton that returns a helpful fallback message.
/// A real implementation would integrate a PDF parsing library (e.g. `pdf-extract`
/// or `lopdf`).
#[must_use]
pub fn read_pdf(path: &Path, page_range: Option<(usize, usize)>) -> PdfReadResult {
    let range_desc = page_range.map_or_else(
        || "all pages".to_owned(),
        |(start, end)| format!("pages {start}-{end}"),
    );

    PdfReadResult::Unavailable {
        message: format!(
            "PDF reading not yet implemented.\n\
             File: {}\n\
             Requested: {range_desc}\n\
             \n\
             To read this PDF, you can:\n\
             - Use an external tool: `pdftotext {} -` (if available)\n\
             - Use the bash tool to run a PDF extraction command\n\
             - Install a PDF reading plugin via MCP",
            path.display(),
            path.display()
        ),
    }
}

/// Parse a page range string like "1-5", "3", "10-20".
///
/// Returns `(start_page, end_page)` as 1-based inclusive range.
pub fn parse_page_range(s: &str) -> Result<(usize, usize), String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty page range".to_owned());
    }

    if let Some((left, right)) = s.split_once('-') {
        let start: usize = left
            .trim()
            .parse()
            .map_err(|_| format!("invalid start page: '{}'", left.trim()))?;
        let end: usize = right
            .trim()
            .parse()
            .map_err(|_| format!("invalid end page: '{}'", right.trim()))?;
        if start == 0 || end == 0 {
            return Err("page numbers must be >= 1".to_owned());
        }
        if start > end {
            return Err(format!("start page ({start}) > end page ({end})"));
        }
        Ok((start, end))
    } else {
        let page: usize = s
            .parse()
            .map_err(|_| format!("invalid page number: '{s}'"))?;
        if page == 0 {
            return Err("page number must be >= 1".to_owned());
        }
        Ok((page, page))
    }
}

// ── ImageMetadata — basic image info extraction ─────────────────────

/// Basic metadata about an image file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageMetadata {
    /// Image format (e.g. "PNG", "JPEG", "GIF").
    pub format: String,
    /// Width in pixels (if detectable).
    pub width: Option<u32>,
    /// Height in pixels (if detectable).
    pub height: Option<u32>,
    /// File size in bytes.
    pub file_size: u64,
}

impl std::fmt::Display for ImageMetadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "format: {}", self.format)?;
        if let (Some(w), Some(h)) = (self.width, self.height) {
            write!(f, "\ndimensions: {w}x{h}")?;
        }
        write!(f, "\nsize: {}", format_file_size(self.file_size))?;
        Ok(())
    }
}

/// Format a byte count as a human-readable size string.
#[must_use]
fn format_file_size(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    #[allow(clippy::cast_precision_loss)]
    let kb = bytes as f64 / 1024.0;
    if kb < 1024.0 {
        return format!("{kb:.1} KB");
    }
    let mb = kb / 1024.0;
    if mb < 1024.0 {
        return format!("{mb:.1} MB");
    }
    let gb = mb / 1024.0;
    format!("{gb:.2} GB")
}

/// Extract metadata from an image file by reading its header bytes.
///
/// Supports PNG, JPEG, GIF, and BMP dimension detection.
/// For other formats, returns format and size without dimensions.
pub fn read_image_metadata(path: &Path) -> Result<ImageMetadata, String> {
    let metadata =
        std::fs::metadata(path).map_err(|e| format!("failed to read file metadata: {e}"))?;
    let file_size = metadata.len();

    let data = std::fs::read(path).map_err(|e| format!("failed to read file: {e}"))?;
    if data.is_empty() {
        return Err("file is empty".to_owned());
    }

    // PNG: 8-byte signature + IHDR chunk contains width/height at bytes 16-23
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        let (w, h) = if data.len() >= 24 {
            (
                Some(u32::from_be_bytes([data[16], data[17], data[18], data[19]])),
                Some(u32::from_be_bytes([data[20], data[21], data[22], data[23]])),
            )
        } else {
            (None, None)
        };
        return Ok(ImageMetadata {
            format: "PNG".to_owned(),
            width: w,
            height: h,
            file_size,
        });
    }

    // JPEG: search for SOF0 marker (0xFF 0xC0) which contains dimensions
    if data.starts_with(b"\xFF\xD8\xFF") {
        let (w, h) = find_jpeg_dimensions(&data);
        return Ok(ImageMetadata {
            format: "JPEG".to_owned(),
            width: w,
            height: h,
            file_size,
        });
    }

    // GIF: dimensions at bytes 6-9 (little-endian)
    if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        let (w, h) = if data.len() >= 10 {
            (
                Some(u16::from_le_bytes([data[6], data[7]]).into()),
                Some(u16::from_le_bytes([data[8], data[9]]).into()),
            )
        } else {
            (None, None)
        };
        return Ok(ImageMetadata {
            format: "GIF".to_owned(),
            width: w,
            height: h,
            file_size,
        });
    }

    // BMP: dimensions at bytes 18-25 (little-endian i32)
    if data.len() >= 26 && data[0] == b'B' && data[1] == b'M' {
        let w = i32::from_le_bytes([data[18], data[19], data[20], data[21]]);
        let h = i32::from_le_bytes([data[22], data[23], data[24], data[25]]);
        return Ok(ImageMetadata {
            format: "BMP".to_owned(),
            width: Some(w.unsigned_abs()),
            height: Some(h.unsigned_abs()),
            file_size,
        });
    }

    // WebP: "RIFF" + 4 bytes + "WEBP"
    if data.len() >= 12 && data.starts_with(b"RIFF") && &data[8..12] == b"WEBP" {
        return Ok(ImageMetadata {
            format: "WebP".to_owned(),
            width: None,
            height: None,
            file_size,
        });
    }

    // Fallback: detect by extension
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("unknown")
        .to_ascii_uppercase();

    Ok(ImageMetadata {
        format: ext,
        width: None,
        height: None,
        file_size,
    })
}

/// Search for JPEG SOF markers to find dimensions.
fn find_jpeg_dimensions(data: &[u8]) -> (Option<u32>, Option<u32>) {
    let mut i = 2; // skip SOI marker
    while i + 1 < data.len() {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = data[i + 1];
        // SOF0..SOF3 markers contain dimensions
        if (0xC0..=0xC3).contains(&marker) && i + 9 < data.len() {
            let h = u16::from_be_bytes([data[i + 5], data[i + 6]]);
            let w = u16::from_be_bytes([data[i + 7], data[i + 8]]);
            return (Some(w.into()), Some(h.into()));
        }
        // Skip to next marker
        if i + 3 < data.len() {
            let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
            i += 2 + len;
        } else {
            break;
        }
    }
    (None, None)
}

/// Summarize image metadata as a formatted string suitable for tool output.
#[must_use]
pub fn format_image_info(path: &Path, meta: &ImageMetadata) -> String {
    let mut out = String::new();
    let _ = write!(out, "Image: {}", path.display());
    let _ = write!(out, "\n{meta}");
    out
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── SmartRange tests ────────────────────────────────────────────

    #[test]
    fn find_scope_boundaries_rust_function() {
        let src = "fn main() {\n    println!(\"hello\");\n}\n";
        let bounds = find_scope_boundaries(src);
        assert_eq!(bounds.len(), 1);
        assert_eq!(bounds[0].start, 1);
        assert_eq!(bounds[0].end, 3);
        assert_eq!(bounds[0].kind, "function");
        assert_eq!(bounds[0].name, "main");
    }

    #[test]
    fn find_scope_boundaries_multiple() {
        let src = "\
fn foo() {
    1
}

fn bar() {
    2
}
";
        let bounds = find_scope_boundaries(src);
        assert_eq!(bounds.len(), 2);
        assert_eq!(bounds[0].name, "foo");
        assert_eq!(bounds[0].start, 1);
        assert_eq!(bounds[0].end, 3);
        assert_eq!(bounds[1].name, "bar");
        assert_eq!(bounds[1].start, 5);
        assert_eq!(bounds[1].end, 7);
    }

    #[test]
    fn find_scope_boundaries_nested() {
        let src = "\
impl Foo {
    fn method(&self) {
        body
    }
}
";
        let bounds = find_scope_boundaries(src);
        // Should find impl (lines 1-5) and method (lines 2-4)
        assert!(bounds.len() >= 2);
        let impl_b = bounds.iter().find(|b| b.kind == "impl").unwrap();
        assert_eq!(impl_b.start, 1);
        assert_eq!(impl_b.end, 5);
    }

    #[test]
    fn find_scope_boundaries_python_def() {
        let src = "def hello():\n    pass\n";
        let bounds = find_scope_boundaries(src);
        // No braces, so no closing brace found
        assert!(bounds.is_empty());
    }

    #[test]
    fn find_scope_boundaries_js_class() {
        let src = "class Foo {\n  constructor() {\n  }\n}\n";
        let bounds = find_scope_boundaries(src);
        assert!(!bounds.is_empty());
        let class_b = bounds.iter().find(|b| b.kind == "class").unwrap();
        assert_eq!(class_b.name, "Foo");
    }

    #[test]
    fn expand_to_scope_no_overlap() {
        let src = "fn foo() {\n    1\n}\n\nsome_other_line\n";
        // Request line 5, which is outside any scope
        let (offset, limit) = expand_to_scope(src, 5, 1);
        assert_eq!(offset, 5);
        assert_eq!(limit, 1);
    }

    #[test]
    fn expand_to_scope_expands_to_function() {
        let src = "\
fn foo() {
    line1
    line2
    line3
}
";
        // Request line 3 only — should expand to lines 1-5
        let (offset, limit) = expand_to_scope(src, 3, 1);
        assert_eq!(offset, 1);
        assert_eq!(limit, 5);
    }

    #[test]
    fn expand_to_scope_already_covers() {
        let src = "fn foo() {\n    body\n}\n";
        // Request already covers the full scope
        let (offset, limit) = expand_to_scope(src, 1, 3);
        assert_eq!(offset, 1);
        assert_eq!(limit, 3);
    }

    // ── BinaryDetector tests ────────────────────────────────────────

    #[test]
    fn detect_binary_empty_is_text() {
        assert_eq!(detect_binary(b""), ContentKind::Text);
    }

    #[test]
    fn detect_binary_plain_text() {
        assert_eq!(
            detect_binary(b"Hello, world!\nThis is text.\n"),
            ContentKind::Text
        );
    }

    #[test]
    fn detect_binary_png_magic() {
        let data = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0DIHDR";
        assert!(matches!(
            detect_binary(data),
            ContentKind::Binary { reason } if reason.contains("PNG")
        ));
    }

    #[test]
    fn detect_binary_pdf_magic() {
        let data = b"%PDF-1.7 some pdf content";
        assert!(matches!(
            detect_binary(data),
            ContentKind::Binary { reason } if reason.contains("PDF")
        ));
    }

    #[test]
    fn detect_binary_null_bytes() {
        let data = b"some\x00text\x00with\x00nulls";
        assert!(matches!(
            detect_binary(data),
            ContentKind::Binary { reason } if reason.contains("null byte")
        ));
    }

    #[test]
    fn detect_binary_elf() {
        let data = b"\x7FELF\x02\x01\x01\x00";
        assert!(matches!(
            detect_binary(data),
            ContentKind::Binary { reason } if reason.contains("ELF")
        ));
    }

    #[test]
    fn detect_binary_windows_exe() {
        let data = b"MZ\x90\x00\x03\x00\x00\x00";
        assert!(matches!(
            detect_binary(data),
            ContentKind::Binary { reason } if reason.contains("Windows")
        ));
    }

    #[test]
    fn binary_file_message_text_is_empty() {
        let msg = binary_file_message(Path::new("test.txt"), &ContentKind::Text);
        assert!(msg.is_empty());
    }

    #[test]
    fn binary_file_message_binary_has_path() {
        let kind = ContentKind::Binary {
            reason: "PNG image".to_owned(),
        };
        let msg = binary_file_message(Path::new("/tmp/logo.png"), &kind);
        assert!(msg.contains("/tmp/logo.png"));
        assert!(msg.contains("PNG image"));
    }

    // ── PdfReader tests ─────────────────────────────────────────────

    #[test]
    fn read_pdf_returns_unavailable() {
        let result = read_pdf(Path::new("/tmp/doc.pdf"), None);
        assert!(matches!(result, PdfReadResult::Unavailable { .. }));
    }

    #[test]
    fn read_pdf_with_range_mentions_pages() {
        let result = read_pdf(Path::new("/tmp/doc.pdf"), Some((1, 5)));
        match result {
            PdfReadResult::Unavailable { message } => {
                assert!(message.contains("pages 1-5"));
            }
            PdfReadResult::Success { .. } => panic!("expected Unavailable"),
        }
    }

    #[test]
    fn parse_page_range_single() {
        assert_eq!(parse_page_range("3"), Ok((3, 3)));
    }

    #[test]
    fn parse_page_range_span() {
        assert_eq!(parse_page_range("1-5"), Ok((1, 5)));
    }

    #[test]
    fn parse_page_range_with_spaces() {
        assert_eq!(parse_page_range(" 2 - 10 "), Ok((2, 10)));
    }

    #[test]
    fn parse_page_range_empty_is_error() {
        assert!(parse_page_range("").is_err());
    }

    #[test]
    fn parse_page_range_zero_is_error() {
        assert!(parse_page_range("0").is_err());
        assert!(parse_page_range("0-5").is_err());
    }

    #[test]
    fn parse_page_range_inverted_is_error() {
        assert!(parse_page_range("5-1").is_err());
    }

    #[test]
    fn parse_page_range_invalid_is_error() {
        assert!(parse_page_range("abc").is_err());
        assert!(parse_page_range("1-xyz").is_err());
    }

    // ── ImageMetadata tests ─────────────────────────────────────────

    #[test]
    fn read_image_metadata_png() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.png");
        // Minimal 1x1 PNG
        let png: Vec<u8> = vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // signature
            0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1x1
            0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xDE,
        ];
        std::fs::write(&path, &png).unwrap();

        let meta = read_image_metadata(&path).unwrap();
        assert_eq!(meta.format, "PNG");
        assert_eq!(meta.width, Some(1));
        assert_eq!(meta.height, Some(1));
        assert!(meta.file_size > 0);
    }

    #[test]
    fn read_image_metadata_gif() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.gif");
        // GIF89a header + 2x3 dimensions (little-endian)
        let gif: Vec<u8> = vec![b'G', b'I', b'F', b'8', b'9', b'a', 0x02, 0x00, 0x03, 0x00];
        std::fs::write(&path, &gif).unwrap();

        let meta = read_image_metadata(&path).unwrap();
        assert_eq!(meta.format, "GIF");
        assert_eq!(meta.width, Some(2));
        assert_eq!(meta.height, Some(3));
    }

    #[test]
    fn read_image_metadata_bmp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.bmp");
        let mut bmp = vec![0u8; 30];
        bmp[0] = b'B';
        bmp[1] = b'M';
        // Width = 10 at offset 18 (LE i32)
        bmp[18] = 10;
        // Height = 20 at offset 22 (LE i32)
        bmp[22] = 20;
        std::fs::write(&path, &bmp).unwrap();

        let meta = read_image_metadata(&path).unwrap();
        assert_eq!(meta.format, "BMP");
        assert_eq!(meta.width, Some(10));
        assert_eq!(meta.height, Some(20));
    }

    #[test]
    fn read_image_metadata_nonexistent_is_error() {
        let result = read_image_metadata(Path::new("/nonexistent/image.png"));
        assert!(result.is_err());
    }

    #[test]
    fn read_image_metadata_empty_file_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.png");
        std::fs::write(&path, b"").unwrap();

        let result = read_image_metadata(&path);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty"));
    }

    #[test]
    fn read_image_metadata_unknown_format_uses_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.tga");
        std::fs::write(&path, b"\x00\x00\x02some data here").unwrap();

        let meta = read_image_metadata(&path).unwrap();
        assert_eq!(meta.format, "TGA");
        assert!(meta.width.is_none());
    }

    #[test]
    fn format_file_size_bytes() {
        assert_eq!(format_file_size(100), "100 B");
        assert_eq!(format_file_size(0), "0 B");
    }

    #[test]
    fn format_file_size_kilobytes() {
        assert_eq!(format_file_size(2048), "2.0 KB");
    }

    #[test]
    fn format_file_size_megabytes() {
        assert_eq!(format_file_size(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn format_image_info_output() {
        let meta = ImageMetadata {
            format: "PNG".to_owned(),
            width: Some(800),
            height: Some(600),
            file_size: 12345,
        };
        let info = format_image_info(Path::new("/tmp/pic.png"), &meta);
        assert!(info.contains("/tmp/pic.png"));
        assert!(info.contains("800x600"));
        assert!(info.contains("PNG"));
        assert!(info.contains("12.1 KB"));
    }

    #[test]
    fn image_metadata_display_no_dimensions() {
        let meta = ImageMetadata {
            format: "WebP".to_owned(),
            width: None,
            height: None,
            file_size: 500,
        };
        let s = meta.to_string();
        assert!(s.contains("WebP"));
        assert!(s.contains("500 B"));
        assert!(!s.contains("dimensions"));
    }

    // ── detect_scope_opener edge cases ──────────────────────────────

    #[test]
    fn detect_scope_opener_pub_fn() {
        let result = detect_scope_opener("pub fn new() {");
        assert!(result.is_some());
        let (kind, name) = result.unwrap();
        assert_eq!(kind, "function");
        assert_eq!(name, "new");
    }

    #[test]
    fn detect_scope_opener_attribute_skipped() {
        assert!(detect_scope_opener("#[derive(Debug)]").is_none());
        assert!(detect_scope_opener("@decorator").is_none());
    }

    #[test]
    fn detect_scope_opener_struct() {
        let result = detect_scope_opener("pub struct Config {");
        assert!(result.is_some());
        let (kind, name) = result.unwrap();
        assert_eq!(kind, "struct");
        assert_eq!(name, "Config");
    }

    #[test]
    fn detect_scope_opener_no_match() {
        assert!(detect_scope_opener("let x = 5;").is_none());
        assert!(detect_scope_opener("// fn fake").is_none());
    }
}
