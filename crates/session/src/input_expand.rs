use std::path::Path;

use base64::Engine as _;
use crab_core::message::{ContentBlock, ImageSource, Message};

const MAX_FILE_SIZE: u64 = 100 * 1024; // 100 KB
const MAX_IMAGE_SIZE: u64 = 5 * 1024 * 1024; // 5 MB

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp"];

/// Expand `@path` references in user input into content blocks.
///
/// Scans for `@<path>` tokens in the text. For each one found:
/// - If the file exists and is small enough, its content is injected as a text
///   block (or image block for image files).
/// - If the file is too large or binary, a warning is injected instead.
/// - Unresolved references are left as-is in the text.
///
/// Returns a `Message` with role `User` containing text blocks and any
/// injected file content.
pub fn expand_at_mentions(input: &str, working_dir: &Path) -> Message {
    let mut blocks: Vec<ContentBlock> = Vec::new();
    let mut remaining = input;
    let mut text_buf = String::new();

    while let Some(at_pos) = remaining.find('@') {
        text_buf.push_str(&remaining[..at_pos]);

        let after_at = &remaining[at_pos + 1..];
        if let Some(path_str) = extract_path(after_at) {
            let consumed = path_str.len();
            let resolved = working_dir.join(path_str);

            if resolved.is_file() {
                if !text_buf.is_empty() {
                    blocks.push(ContentBlock::text(std::mem::take(&mut text_buf)));
                }
                blocks.push(expand_file(&resolved, path_str));
                remaining = &after_at[consumed..];
                continue;
            }
        }

        // Not a valid file reference — keep the '@' literal
        text_buf.push('@');
        remaining = after_at;
    }

    text_buf.push_str(remaining);
    if !text_buf.is_empty() {
        blocks.push(ContentBlock::text(text_buf));
    }

    if blocks.is_empty() {
        blocks.push(ContentBlock::text(String::new()));
    }

    Message::new(crab_core::message::Role::User, blocks)
}

fn extract_path(s: &str) -> Option<&str> {
    if s.is_empty() || s.starts_with(char::is_whitespace) {
        return None;
    }
    let end = s
        .find(|c: char| c.is_whitespace() || c == '@')
        .unwrap_or(s.len());
    if end == 0 {
        return None;
    }
    Some(&s[..end])
}

fn expand_file(path: &Path, display_path: &str) -> ContentBlock {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        return expand_image(path, display_path);
    }

    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => {
            return ContentBlock::text(format!("[Error reading @{display_path}: {e}]"));
        }
    };

    if meta.len() > MAX_FILE_SIZE {
        return ContentBlock::text(format!(
            "[File @{display_path} is too large ({} KB, limit {} KB)]",
            meta.len() / 1024,
            MAX_FILE_SIZE / 1024,
        ));
    }

    match std::fs::read_to_string(path) {
        Ok(content) => ContentBlock::text(format!(
            "<file path=\"{display_path}\">\n{content}\n</file>"
        )),
        Err(_) => ContentBlock::text(format!(
            "[File @{display_path} appears to be binary and cannot be displayed]"
        )),
    }
}

fn expand_image(path: &Path, display_path: &str) -> ContentBlock {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => {
            return ContentBlock::text(format!("[Error reading @{display_path}: {e}]"));
        }
    };

    if meta.len() > MAX_IMAGE_SIZE {
        return ContentBlock::text(format!(
            "[Image @{display_path} is too large ({} KB, limit {} KB)]",
            meta.len() / 1024,
            MAX_IMAGE_SIZE / 1024,
        ));
    }

    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            return ContentBlock::text(format!("[Error reading @{display_path}: {e}]"));
        }
    };

    let encoded = base64::engine::general_purpose::STANDARD.encode(&data);
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .to_ascii_lowercase();
    let media_type = match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        _ => "image/png",
    };

    ContentBlock::Image {
        source: ImageSource::base64(media_type, encoded),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_mentions_passes_through() {
        let msg = expand_at_mentions("hello world", Path::new("."));
        assert_eq!(msg.content.len(), 1);
        assert_eq!(msg.text(), "hello world");
    }

    #[test]
    fn unresolved_mention_kept_as_text() {
        let msg = expand_at_mentions("see @nonexistent_file_xyz.txt", Path::new("."));
        assert_eq!(msg.content.len(), 1);
        assert!(msg.text().contains("@nonexistent_file_xyz.txt"));
    }

    #[test]
    fn expands_real_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.rs"), "fn main() {}").unwrap();

        let msg = expand_at_mentions("check @test.rs please", dir.path());
        assert!(msg.content.len() >= 2);
        let full_text: String = msg
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");
        assert!(full_text.contains("fn main()"));
        assert!(full_text.contains("please"));
    }

    #[test]
    fn rejects_large_file() {
        let dir = tempfile::tempdir().unwrap();
        let big = vec![b'x'; (MAX_FILE_SIZE + 1) as usize];
        std::fs::write(dir.path().join("big.txt"), &big).unwrap();

        let msg = expand_at_mentions("read @big.txt", dir.path());
        assert!(msg.text().contains("too large"));
    }

    #[test]
    fn multiple_mentions() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "aaa").unwrap();
        std::fs::write(dir.path().join("b.txt"), "bbb").unwrap();

        let msg = expand_at_mentions("@a.txt and @b.txt", dir.path());
        let text: String = msg
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");
        assert!(text.contains("aaa"));
        assert!(text.contains("bbb"));
    }

    #[test]
    fn email_not_treated_as_mention() {
        let msg = expand_at_mentions("user@example.com", Path::new("."));
        assert_eq!(msg.content.len(), 1);
        // The @example.com part won't resolve as a file, so the whole thing passes through
        assert!(msg.text().contains("user@example.com"));
    }

    #[test]
    fn at_end_of_input() {
        let msg = expand_at_mentions("trailing @", Path::new("."));
        assert_eq!(msg.text(), "trailing @");
    }
}
