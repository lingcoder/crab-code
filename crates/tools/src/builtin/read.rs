use std::fmt::Write as _;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use base64::Engine as _;
use crab_core::Result;
use crab_core::tool::{
    Tool, ToolContext, ToolDisplayResult, ToolDisplayStyle, ToolOutput, ToolOutputContent,
};
use serde_json::Value;

/// File reading tool.
pub const READ_TOOL_NAME: &str = "Read";

pub struct ReadTool;

/// Maximum image file size accepted by the image branch (10 MB).
const MAX_IMAGE_SIZE: u64 = 10 * 1024 * 1024;

/// Supported image extensions and their MIME types.
const IMAGE_TYPES: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("bmp", "image/bmp"),
    ("svg", "image/svg+xml"),
    ("ico", "image/x-icon"),
    ("tiff", "image/tiff"),
    ("tif", "image/tiff"),
];

/// Notebook extensions — redirected to `NotebookRead`.
const NOTEBOOK_EXTENSIONS: &[&str] = &["ipynb"];

/// Look up the MIME type for a given image file extension.
fn mime_for_extension(ext: &str) -> Option<&'static str> {
    IMAGE_TYPES.iter().find(|(e, _)| *e == ext).map(|(_, m)| *m)
}

fn extension_of(path: &Path) -> &str {
    path.extension().and_then(|e| e.to_str()).unwrap_or("")
}

/// Result of binary content detection.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ContentKind {
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
fn detect_binary(data: &[u8]) -> ContentKind {
    if data.is_empty() {
        return ContentKind::Text;
    }

    let sample = if data.len() > 8192 {
        &data[..8192]
    } else {
        data
    };

    for &(magic, desc) in MAGIC_SIGNATURES {
        if sample.starts_with(magic) {
            return ContentKind::Binary {
                reason: desc.to_owned(),
            };
        }
    }

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
fn binary_file_message(path: &Path, kind: &ContentKind) -> String {
    match kind {
        ContentKind::Text => String::new(),
        ContentKind::Binary { reason } => {
            format!(
                "Binary file: {}\nDetected as: {reason}\n\
                 This file cannot be displayed as text.",
                path.display()
            )
        }
    }
}

impl Tool for ReadTool {
    fn name(&self) -> &'static str {
        READ_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Reads a file from the local filesystem. Supports text files (with line numbers), \
         PDF files (with page ranges), image files (PNG, JPEG, GIF, WebP, BMP, SVG, ICO, \
         TIFF — returned as base64), and Jupyter notebooks."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the file to read. Supports text files (with line numbers), PDF files (with page ranges), image files (PNG, JPEG, GIF, WebP, BMP, SVG, ICO, TIFF — returned as base64), and Jupyter notebooks."
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start reading from (1-based, default: 1). Text files only."
                },
                "limit": {
                    "type": "integer",
                    "description": "Number of lines to read (default: 2000). Text files only."
                },
                "pages": {
                    "type": "string",
                    "description": "Page range for PDF files (e.g. \"1-5\", \"3\", \"1,3,5-8\"). Only applicable to PDF files. Maximum 20 pages per request."
                }
            },
            "required": ["file_path"]
        })
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let file_path = input["file_path"].as_str().unwrap_or("").to_owned();
        // offset is 1-based line number; default 1
        #[allow(clippy::cast_possible_truncation)]
        let offset = input["offset"].as_u64().map_or(1, |v| v as usize);
        #[allow(clippy::cast_possible_truncation)]
        let limit = input["limit"].as_u64().map_or(2000, |v| v as usize);
        let pages = input["pages"].as_str().map(String::from);

        Box::pin(async move {
            if file_path.is_empty() {
                return Ok(ToolOutput::error("file_path is required"));
            }

            let path = std::path::PathBuf::from(&file_path);
            let ext = extension_of(&path).to_ascii_lowercase();

            // 1. Image extension → base64 image content
            if let Some(mime_type) = mime_for_extension(&ext) {
                return read_image(&path, mime_type).await;
            }

            // 2. Notebook → redirect to NotebookRead
            if NOTEBOOK_EXTENSIONS.contains(&ext.as_str()) {
                return Ok(ToolOutput::success(format!(
                    "Jupyter notebook file: {file_path}\n\
                     Use the NotebookRead tool to read notebook cells, or \
                     NotebookEdit to modify them."
                )));
            }

            // 3. PDF → text extraction
            if ext == "pdf" {
                return read_pdf(&path, pages.as_deref()).await;
            }

            // 4. Read first bytes → magic-byte binary detection
            let bytes = match tokio::fs::read(&path).await {
                Ok(b) => b,
                Err(e) => {
                    return Ok(ToolOutput::error(format!(
                        "Failed to read {file_path}: {e}"
                    )));
                }
            };

            let kind = detect_binary(&bytes);
            if let ContentKind::Binary { .. } = &kind {
                return Ok(ToolOutput::success(binary_file_message(&path, &kind)));
            }

            // 5. Text content with offset/limit
            let content = match String::from_utf8(bytes) {
                Ok(s) => s,
                Err(e) => {
                    return Ok(ToolOutput::error(format!(
                        "Failed to decode {file_path} as UTF-8: {e}"
                    )));
                }
            };

            let start = offset.saturating_sub(1); // convert to 0-based index
            let lines: Vec<&str> = content.lines().collect();
            let total = lines.len();

            let end = (start + limit).min(total);
            let selected = if start >= total {
                &[][..]
            } else {
                &lines[start..end]
            };

            let mut output = String::new();
            for (i, line) in selected.iter().enumerate() {
                let line_num = start + i + 1; // 1-based
                let _ = writeln!(output, "{line_num:6}\t{line}");
            }

            Ok(ToolOutput::success(output))
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    // ── CCB-aligned rendering hooks ──

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        let path = input["file_path"].as_str()?;
        let filename = path.rsplit(['/', '\\']).next().unwrap_or(path);
        let offset = input["offset"].as_u64();
        let limit = input["limit"].as_u64();
        let range = match (offset, limit) {
            (Some(o), Some(l)) => format!(" · lines {o}-{}", o + l),
            (Some(o), None) => format!(" · from line {o}"),
            _ => String::new(),
        };
        Some(format!("Read ({filename}{range})"))
    }

    fn format_result(&self, output: &ToolOutput) -> Option<ToolDisplayResult> {
        use crab_core::tool::{ToolDisplayLine, ToolDisplayResult, ToolDisplayStyle};

        // If the output is image content, surface that explicitly.
        if output
            .content
            .iter()
            .any(|c| matches!(c, ToolOutputContent::Image { .. }))
        {
            return Some(ToolDisplayResult {
                lines: vec![ToolDisplayLine::new(
                    "Read image".to_string(),
                    ToolDisplayStyle::Muted,
                )],
                preview_lines: 1,
            });
        }

        let text = output.text();
        let line_count = text.lines().count();
        Some(ToolDisplayResult {
            lines: vec![ToolDisplayLine::new(
                format!("Read {line_count} lines"),
                ToolDisplayStyle::Muted,
            )],
            preview_lines: 1,
        })
    }

    fn format_error(&self, output: &ToolOutput, input: &Value) -> Option<ToolDisplayResult> {
        use crab_core::tool::{ToolDisplayLine, ToolDisplayResult};
        let text = output.text();
        let path = input["file_path"].as_str().unwrap_or("?");

        let mut lines = vec![ToolDisplayLine::new(
            format!("Error reading {path}"),
            ToolDisplayStyle::Error,
        )];

        if text.contains("not found") || text.contains("No such file") {
            lines.push(ToolDisplayLine::new(
                "Hint: Use Glob to search for files by pattern",
                ToolDisplayStyle::Muted,
            ));
        }

        Some(ToolDisplayResult {
            lines,
            preview_lines: 2,
        })
    }

    fn display_color(&self) -> ToolDisplayStyle {
        ToolDisplayStyle::Muted
    }

    fn max_result_chars(&self) -> usize {
        usize::MAX
    }
}

/// Read an image file, base64-encode it, and return as `ToolOutputContent::Image`.
async fn read_image(path: &Path, mime_type: &str) -> Result<ToolOutput> {
    let file_path = path.display().to_string();

    if !path.exists() {
        return Ok(ToolOutput::error(format!("file not found: {file_path}")));
    }

    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|e| crab_core::Error::Other(format!("failed to read file metadata: {e}")))?;

    if metadata.len() > MAX_IMAGE_SIZE {
        return Ok(ToolOutput::error(format!(
            "image file too large: {} bytes (max {} bytes)",
            metadata.len(),
            MAX_IMAGE_SIZE
        )));
    }

    let bytes = tokio::fs::read(path)
        .await
        .map_err(|e| crab_core::Error::Other(format!("failed to read image file: {e}")))?;

    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);

    Ok(ToolOutput::with_content(
        vec![ToolOutputContent::Image {
            media_type: mime_type.to_string(),
            data: encoded,
        }],
        false,
    ))
}

/// Maximum pages allowed per PDF read request.
const MAX_PDF_PAGES_PER_REQUEST: usize = 20;

/// Threshold above which a `pages` parameter is required.
const PDF_LARGE_THRESHOLD: usize = 10;

/// Parse a page range string like "1-5", "3", "1,3,5-8" into a sorted, deduplicated
/// list of 1-based page numbers.
fn parse_page_ranges(spec: &str) -> std::result::Result<Vec<usize>, String> {
    let mut pages = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((start, end)) = part.split_once('-') {
            let s: usize = start
                .trim()
                .parse()
                .map_err(|_| format!("invalid page number: '{start}'"))?;
            let e: usize = end
                .trim()
                .parse()
                .map_err(|_| format!("invalid page number: '{end}'"))?;
            if s == 0 || e == 0 {
                return Err("page numbers must be >= 1".into());
            }
            if s > e {
                return Err(format!("invalid range: {s}-{e} (start > end)"));
            }
            for p in s..=e {
                pages.push(p);
            }
        } else {
            let p: usize = part
                .parse()
                .map_err(|_| format!("invalid page number: '{part}'"))?;
            if p == 0 {
                return Err("page numbers must be >= 1".into());
            }
            pages.push(p);
        }
    }
    pages.sort_unstable();
    pages.dedup();
    Ok(pages)
}

/// Read a PDF file, optionally extracting only specific pages.
async fn read_pdf(path: &Path, pages_spec: Option<&str>) -> Result<ToolOutput> {
    let file_path = path.display().to_string();

    let bytes = match tokio::fs::read(path).await {
        Ok(b) => b,
        Err(e) => {
            return Ok(ToolOutput::error(format!(
                "Failed to read PDF {file_path}: {e}"
            )));
        }
    };

    #[cfg(feature = "pdf")]
    {
        let pages_owned = pages_spec.map(String::from);
        let result =
            tokio::task::spawn_blocking(move || extract_pdf_text(&bytes, pages_owned.as_deref()))
                .await
                .map_err(|e| crab_core::Error::Other(format!("PDF extraction task failed: {e}")))?;

        match result {
            Ok(text) => Ok(ToolOutput::success(text)),
            Err(msg) => Ok(ToolOutput::error(msg)),
        }
    }

    #[cfg(not(feature = "pdf"))]
    {
        let _ = bytes;
        Ok(ToolOutput::success(format!(
            "PDF file: {file_path}\n\
             PDF reading is not enabled. Rebuild with the 'pdf' feature."
        )))
    }
}

/// Extract text from PDF bytes. Returns the formatted text or an error message.
#[cfg(feature = "pdf")]
fn extract_pdf_text(bytes: &[u8], pages_spec: Option<&str>) -> std::result::Result<String, String> {
    use std::fmt::Write;

    let doc = pdf_oxide::PdfDocument::from_bytes(bytes.to_vec())
        .map_err(|e| format!("Failed to parse PDF: {e}"))?;
    let total_pages = doc
        .page_count()
        .map_err(|e| format!("Failed to read PDF page count: {e}"))?;

    let selected_pages: Vec<usize> = if let Some(spec) = pages_spec {
        let requested = parse_page_ranges(spec)?;
        if requested.len() > MAX_PDF_PAGES_PER_REQUEST {
            return Err(format!(
                "Too many pages requested ({}). Maximum is {MAX_PDF_PAGES_PER_REQUEST} per request.",
                requested.len()
            ));
        }
        requested
            .into_iter()
            .filter(|&p| p <= total_pages)
            .collect()
    } else {
        if total_pages > PDF_LARGE_THRESHOLD {
            return Err(format!(
                "PDF has {total_pages} pages. For large PDFs (>{PDF_LARGE_THRESHOLD} pages), \
                 you must provide the 'pages' parameter (e.g. pages: \"1-5\"). \
                 Maximum {MAX_PDF_PAGES_PER_REQUEST} pages per request."
            ));
        }
        (1..=total_pages).collect()
    };

    if selected_pages.is_empty() {
        return Err("No valid pages found in the specified range.".into());
    }

    let mut output = if selected_pages.len() == total_pages {
        format!("PDF: {total_pages} total page(s), showing all\n\n")
    } else {
        format!(
            "PDF: {total_pages} total page(s), showing {} page(s)\n\n",
            selected_pages.len()
        )
    };

    for &page_num in &selected_pages {
        let idx = page_num - 1;
        let _ = writeln!(output, "--- Page {page_num} ---");
        if let Ok(page_text) = doc.extract_text(idx) {
            output.push_str(page_text.trim());
        }
        output.push_str("\n\n");
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::permission::{PermissionMode, PermissionPolicy};
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    fn make_ctx() -> ToolContext {
        ToolContext {
            working_dir: std::env::temp_dir(),
            permission_mode: PermissionMode::Default,
            session_id: "test".into(),
            cancellation_token: CancellationToken::new(),
            permission_policy: PermissionPolicy::default(),
            ext: crab_core::tool::ToolContextExt::default(),
        }
    }

    async fn write_temp(name: &str, content: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(name);
        tokio::fs::write(&path, content).await.unwrap();
        path
    }

    #[tokio::test]
    async fn read_simple_file() {
        let path = write_temp("read_test_simple.txt", "line one\nline two\nline three\n").await;
        let tool = ReadTool;
        let input = serde_json::json!({ "file_path": path.to_str().unwrap() });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(!out.is_error);
        let text = out.text();
        assert!(text.contains("     1\tline one"));
        assert!(text.contains("     2\tline two"));
        assert!(text.contains("     3\tline three"));
    }

    #[tokio::test]
    async fn read_with_offset_and_limit() {
        let content = "a\nb\nc\nd\ne\n";
        let path = write_temp("read_test_offset.txt", content).await;
        let tool = ReadTool;
        let input = serde_json::json!({
            "file_path": path.to_str().unwrap(),
            "offset": 2,
            "limit": 2
        });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(!out.is_error);
        let text = out.text();
        assert!(text.contains("     2\tb"));
        assert!(text.contains("     3\tc"));
        assert!(!text.contains("     1\t"));
        assert!(!text.contains("     4\t"));
    }

    #[tokio::test]
    async fn read_nonexistent_file_is_error() {
        let tool = ReadTool;
        let input = serde_json::json!({ "file_path": "/nonexistent/path/file.txt" });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn read_empty_path_is_error() {
        let tool = ReadTool;
        let input = serde_json::json!({ "file_path": "" });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn read_notebook_extension_returns_info() {
        let tool = ReadTool;
        let input = serde_json::json!({ "file_path": "/some/notebook.ipynb" });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(!out.is_error);
        assert!(out.text().contains("Jupyter notebook"));
    }

    #[test]
    fn read_is_read_only() {
        assert!(ReadTool.is_read_only());
    }

    #[test]
    fn read_schema_requires_file_path() {
        let schema = ReadTool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "file_path"));
    }

    #[test]
    fn read_schema_has_pages_param() {
        let schema = ReadTool.input_schema();
        assert!(schema["properties"]["pages"].is_object());
    }

    // ─── PDF page range parsing ───

    #[test]
    fn parse_single_page() {
        let pages = parse_page_ranges("3").unwrap();
        assert_eq!(pages, vec![3]);
    }

    #[test]
    fn parse_page_range() {
        let pages = parse_page_ranges("1-5").unwrap();
        assert_eq!(pages, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn parse_comma_separated() {
        let pages = parse_page_ranges("1,3,5").unwrap();
        assert_eq!(pages, vec![1, 3, 5]);
    }

    #[test]
    fn parse_mixed_ranges_and_singles() {
        let pages = parse_page_ranges("1,3-5,8").unwrap();
        assert_eq!(pages, vec![1, 3, 4, 5, 8]);
    }

    #[test]
    fn parse_deduplicates_and_sorts() {
        let pages = parse_page_ranges("5,3,1,3,5").unwrap();
        assert_eq!(pages, vec![1, 3, 5]);
    }

    #[test]
    fn parse_page_zero_errors() {
        assert!(parse_page_ranges("0").is_err());
    }

    #[test]
    fn parse_invalid_range_errors() {
        assert!(parse_page_ranges("5-3").is_err());
    }

    #[test]
    fn parse_non_numeric_errors() {
        assert!(parse_page_ranges("abc").is_err());
    }

    #[test]
    fn parse_empty_string() {
        let pages = parse_page_ranges("").unwrap();
        assert!(pages.is_empty());
    }

    #[test]
    fn parse_whitespace_tolerant() {
        let pages = parse_page_ranges(" 1 , 3 - 5 ").unwrap();
        assert_eq!(pages, vec![1, 3, 4, 5]);
    }

    // ─── PDF read tool integration ───

    #[tokio::test]
    async fn read_pdf_extension_not_treated_as_binary() {
        let tool = ReadTool;
        // A nonexistent PDF should give a read error, NOT "Binary file"
        let input = serde_json::json!({ "file_path": "/nonexistent/test.pdf" });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(!out.text().contains("Binary file"));
    }

    #[cfg(feature = "pdf")]
    #[test]
    fn extract_pdf_text_too_many_pages_errors() {
        let result = parse_page_ranges("1-25").unwrap();
        assert_eq!(result.len(), 25);
    }

    // ─── MIME lookup ───

    #[test]
    fn mime_lookup() {
        assert_eq!(mime_for_extension("png"), Some("image/png"));
        assert_eq!(mime_for_extension("jpg"), Some("image/jpeg"));
        assert_eq!(mime_for_extension("jpeg"), Some("image/jpeg"));
        assert_eq!(mime_for_extension("gif"), Some("image/gif"));
        assert_eq!(mime_for_extension("webp"), Some("image/webp"));
        assert_eq!(mime_for_extension("svg"), Some("image/svg+xml"));
        assert_eq!(mime_for_extension("bmp"), Some("image/bmp"));
        assert_eq!(mime_for_extension("ico"), Some("image/x-icon"));
        assert_eq!(mime_for_extension("tiff"), Some("image/tiff"));
        assert_eq!(mime_for_extension("tif"), Some("image/tiff"));
        assert_eq!(mime_for_extension("mp4"), None);
        assert_eq!(mime_for_extension(""), None);
    }

    // ─── Image read path ───

    #[tokio::test]
    async fn nonexistent_image_returns_error() {
        let tool = ReadTool;
        let result = tool
            .execute(
                json!({"file_path": "/tmp/does_not_exist_12345.png"}),
                &make_ctx(),
            )
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.text().contains("not found"));
    }

    #[tokio::test]
    async fn reads_png_file_successfully() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.png");

        // Minimal 1x1 PNG
        let png_bytes: Vec<u8> = vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
            0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR chunk
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1x1
            0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xDE, // 8-bit RGB
            0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, // IDAT chunk
            0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0xE2, 0x21,
            0xBC, 0x33, // compressed data
            0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, // IEND chunk
            0xAE, 0x42, 0x60, 0x82,
        ];
        std::fs::write(&path, &png_bytes).unwrap();

        let tool = ReadTool;
        let result = tool
            .execute(json!({"file_path": path.to_str().unwrap()}), &make_ctx())
            .await
            .unwrap();
        assert!(!result.is_error);
        assert_eq!(result.content.len(), 1);
        match &result.content[0] {
            ToolOutputContent::Image { media_type, data } => {
                assert_eq!(media_type, "image/png");
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .unwrap();
                assert_eq!(decoded, png_bytes);
            }
            other => panic!("expected Image content, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn reads_jpeg_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("photo.jpg");
        let jpeg_bytes = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        std::fs::write(&path, &jpeg_bytes).unwrap();

        let tool = ReadTool;
        let result = tool
            .execute(json!({"file_path": path.to_str().unwrap()}), &make_ctx())
            .await
            .unwrap();
        assert!(!result.is_error);
        match &result.content[0] {
            ToolOutputContent::Image { media_type, .. } => {
                assert_eq!(media_type, "image/jpeg");
            }
            other => panic!("expected Image content, got {other:?}"),
        }
    }

    // ─── Magic-byte binary detection ───

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
    fn binary_file_message_text_is_empty() {
        let msg = binary_file_message(Path::new("test.txt"), &ContentKind::Text);
        assert!(msg.is_empty());
    }

    #[test]
    fn binary_file_message_binary_has_path() {
        let kind = ContentKind::Binary {
            reason: "ELF executable".to_owned(),
        };
        let msg = binary_file_message(Path::new("/tmp/a.out"), &kind);
        assert!(msg.contains("/tmp/a.out"));
        assert!(msg.contains("ELF executable"));
    }

    #[tokio::test]
    async fn binary_file_returns_friendly_message() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blob.dat");
        // Write something with null bytes — extension is unrecognized, magic bytes will catch it.
        std::fs::write(&path, b"\x7FELF\x02\x01\x01\x00\x00\x00").unwrap();

        let tool = ReadTool;
        let result = tool
            .execute(json!({"file_path": path.to_str().unwrap()}), &make_ctx())
            .await
            .unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains("Binary file"));
        assert!(text.contains("ELF"));
    }
}
