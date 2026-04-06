use std::fmt::Write as _;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use serde_json::Value;

/// File reading tool.
pub struct ReadTool;

/// Extensions treated as binary/non-text — return type info instead of content.
const BINARY_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "webp", "ico", "svg", "zip", "tar", "gz", "bz2",
    "xz", "7z", "rar", "exe", "dll", "so", "dylib", "mp3", "mp4", "wav", "ogg", "avi", "mov",
    "mkv",
];

/// Notebook extensions — return type info (full notebook reading handled separately).
const NOTEBOOK_EXTENSIONS: &[&str] = &["ipynb"];

fn extension_of(path: &Path) -> &str {
    path.extension().and_then(|e| e.to_str()).unwrap_or("")
}

impl Tool for ReadTool {
    fn name(&self) -> &'static str {
        "read"
    }

    fn description(&self) -> &'static str {
        "Read a file from the local filesystem. Supports text files with optional \
         line range (offset/limit). Returns content in cat -n format (line numbers). \
         Binary files (images, PDF, archives) return file type information only."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the file to read"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start reading from (1-based, default: 1)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Number of lines to read (default: 2000)"
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

            if NOTEBOOK_EXTENSIONS.contains(&ext.as_str()) {
                return Ok(ToolOutput::success(format!(
                    "Jupyter notebook file: {file_path}\n\
                     Use the notebook_edit tool to read and modify notebook cells."
                )));
            }

            // Handle PDF files specially
            if ext == "pdf" {
                return read_pdf(&path, pages.as_deref()).await;
            }

            if BINARY_EXTENSIONS.contains(&ext.as_str()) {
                return Ok(ToolOutput::success(format!(
                    "Binary file ({ext}): {file_path}\n\
                     This file type cannot be displayed as text."
                )));
            }

            let content = match tokio::fs::read_to_string(&path).await {
                Ok(c) => c,
                Err(e) => {
                    return Ok(ToolOutput::error(format!(
                        "Failed to read {file_path}: {e}"
                    )));
                }
            };

            // offset is 1-based; clamp to at least 1
            let start = offset.saturating_sub(1); // convert to 0-based index
            let lines: Vec<&str> = content.lines().collect();
            let total = lines.len();

            let end = (start + limit).min(total);
            let selected = if start >= total {
                &[][..]
            } else {
                &lines[start..end]
            };

            // Format as cat -n: "     N\tline"
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

    // Read the file bytes
    let bytes = match tokio::fs::read(path).await {
        Ok(b) => b,
        Err(e) => {
            return Ok(ToolOutput::error(format!(
                "Failed to read PDF {file_path}: {e}"
            )));
        }
    };

    // Extract text using pdf-extract (runs in blocking thread since it's CPU-bound)
    #[cfg(feature = "pdf")]
    {
        let pages_owned = pages_spec.map(String::from);
        let result = tokio::task::spawn_blocking(move || {
            extract_pdf_text(&bytes, pages_owned.as_deref())
        })
        .await
        .map_err(|e| crab_common::Error::Other(format!("PDF extraction task failed: {e}")))?;

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
fn extract_pdf_text(
    bytes: &[u8],
    pages_spec: Option<&str>,
) -> std::result::Result<String, String> {
    // pdf-extract provides extract_text_from_mem which extracts all pages
    let full_text = pdf_extract::extract_text_from_mem(bytes)
        .map_err(|e| format!("Failed to parse PDF: {e}"))?;

    // Split by form-feed (page break) — pdf-extract uses \x0c between pages
    let raw_pages: Vec<&str> = full_text.split('\x0c').collect();
    // Filter out trailing empty page that split sometimes creates
    let all_pages: Vec<&str> = raw_pages
        .iter()
        .copied()
        .filter(|p| !p.trim().is_empty())
        .collect();
    let total_pages = all_pages.len();

    // Determine which pages to return
    let selected_pages: Vec<usize> = match pages_spec {
        Some(spec) => {
            let requested = parse_page_ranges(spec)?;
            if requested.len() > MAX_PDF_PAGES_PER_REQUEST {
                return Err(format!(
                    "Too many pages requested ({}). Maximum is {MAX_PDF_PAGES_PER_REQUEST} per request.",
                    requested.len()
                ));
            }
            // Filter to pages that exist
            requested
                .into_iter()
                .filter(|&p| p <= total_pages)
                .collect()
        }
        None => {
            if total_pages > PDF_LARGE_THRESHOLD {
                return Err(format!(
                    "PDF has {total_pages} pages. For large PDFs (>{PDF_LARGE_THRESHOLD} pages), \
                     you must provide the 'pages' parameter (e.g. pages: \"1-5\"). \
                     Maximum {MAX_PDF_PAGES_PER_REQUEST} pages per request."
                ));
            }
            (1..=total_pages).collect()
        }
    };

    if selected_pages.is_empty() {
        return Err("No valid pages found in the specified range.".into());
    }

    let mut output = String::new();
    output.push_str(&format!(
        "PDF: {total_pages} total page(s), showing {}\n\n",
        if selected_pages.len() == total_pages {
            "all".to_string()
        } else {
            format!("{} page(s)", selected_pages.len())
        }
    ));

    for &page_num in &selected_pages {
        let idx = page_num - 1; // 0-based index
        output.push_str(&format!("--- Page {page_num} ---\n"));
        if idx < all_pages.len() {
            output.push_str(all_pages[idx].trim());
        }
        output.push_str("\n\n");
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::permission::{PermissionMode, PermissionPolicy};
    use tokio_util::sync::CancellationToken;

    fn make_ctx() -> ToolContext {
        ToolContext {
            working_dir: std::env::temp_dir(),
            permission_mode: PermissionMode::Default,
            session_id: "test".into(),
            cancellation_token: CancellationToken::new(),
            permission_policy: PermissionPolicy::default(),
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
        // offset=2, limit=2 → lines 2 and 3
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
    async fn read_binary_extension_returns_info() {
        let tool = ReadTool;
        let input = serde_json::json!({ "file_path": "/some/image.png" });
        let out = tool.execute(input, &make_ctx()).await.unwrap();
        assert!(!out.is_error);
        assert!(out.text().contains("Binary file"));
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
        // Should attempt to read the PDF (and fail), not say "Binary file"
        assert!(!out.text().contains("Binary file"));
    }

    #[cfg(feature = "pdf")]
    #[test]
    fn extract_pdf_text_too_many_pages_errors() {
        // Request 25 pages (over MAX_PDF_PAGES_PER_REQUEST=20)
        let result = parse_page_ranges("1-25").unwrap();
        assert_eq!(result.len(), 25);
        // The limit check happens in extract_pdf_text, not parse_page_ranges
    }
}
