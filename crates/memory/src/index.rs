use std::fmt::Write as _;
use std::fs;
use std::path::Path;

/// Maximum number of lines kept when loading a `MEMORY.md` index.
pub const MAX_INDEX_LINES: usize = 200;

/// Maximum number of bytes kept when loading a `MEMORY.md` index.
pub const MAX_INDEX_BYTES: usize = 25_000;

// ─── Types ─────────────────────────────────────────────────────

/// A single entry parsed from `MEMORY.md`.
///
/// Each entry maps to one line: `- [title](filename) — description`.
#[derive(Debug, Clone)]
pub struct IndexEntry {
    pub title: String,
    pub filename: String,
    pub description: String,
}

/// Information about how index content was truncated during loading.
#[derive(Debug, Clone)]
pub struct Truncation {
    pub original_lines: usize,
    pub original_bytes: usize,
    pub was_line_truncated: bool,
    pub was_byte_truncated: bool,
}

/// A parsed `MEMORY.md` index, optionally truncated.
#[derive(Debug)]
pub struct MemoryIndex {
    pub entries: Vec<IndexEntry>,
    pub truncation: Option<Truncation>,
}

// ─── Public API ────────────────────────────────────────────────

/// Load and parse `MEMORY.md` from `dir`.
///
/// Returns an empty index if the file does not exist. Content is truncated
/// to [`MAX_INDEX_LINES`] / [`MAX_INDEX_BYTES`] before parsing.
pub fn load_index(dir: &Path) -> crab_common::Result<MemoryIndex> {
    let path = dir.join("MEMORY.md");
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(MemoryIndex {
                entries: Vec::new(),
                truncation: None,
            });
        }
        Err(e) => return Err(e.into()),
    };

    let (truncated, trunc_info) = truncate_index_content(&content);
    let truncation = if trunc_info.was_line_truncated || trunc_info.was_byte_truncated {
        Some(trunc_info)
    } else {
        None
    };

    let entries = parse_index_content(&truncated);
    Ok(MemoryIndex {
        entries,
        truncation,
    })
}

/// Write `entries` to `MEMORY.md` inside `dir`, creating directories as needed.
pub fn save_index(dir: &Path, entries: &[IndexEntry]) -> crab_common::Result<()> {
    fs::create_dir_all(dir)?;
    let content = format_index_content(entries);
    fs::write(dir.join("MEMORY.md"), content)?;
    Ok(())
}

/// Parse lines of `MEMORY.md` content into [`IndexEntry`] values.
///
/// Recognises `- [Title](file.md) — description` and the ASCII fallback
/// `- [Title](file.md) -- description`. Lines that do not match are skipped.
pub fn parse_index_content(content: &str) -> Vec<IndexEntry> {
    let mut entries = Vec::new();
    for line in content.lines() {
        if let Some(entry) = parse_entry_line(line) {
            entries.push(entry);
        }
    }
    entries
}

/// Format entries as `MEMORY.md` content.
pub fn format_index_content(entries: &[IndexEntry]) -> String {
    let mut out = String::new();
    for entry in entries {
        let _ = writeln!(
            out,
            "- [{}]({}) \u{2014} {}",
            entry.title, entry.filename, entry.description
        );
    }
    out
}

/// Truncate `content` to at most [`MAX_INDEX_LINES`] lines and
/// [`MAX_INDEX_BYTES`] bytes.
///
/// For byte truncation the cut is snapped back to the last newline before
/// the limit so that partial lines are never included. Returns the
/// (possibly unchanged) content together with [`Truncation`] metadata.
pub fn truncate_index_content(content: &str) -> (String, Truncation) {
    let original_lines = content.lines().count();
    let original_bytes = content.len();

    let mut was_line_truncated = false;
    let mut was_byte_truncated = false;

    // 1. Line truncation
    let after_lines: String = if original_lines > MAX_INDEX_LINES {
        was_line_truncated = true;
        let mut result = String::new();
        for (i, line) in content.lines().enumerate() {
            if i >= MAX_INDEX_LINES {
                break;
            }
            if i > 0 {
                result.push('\n');
            }
            result.push_str(line);
        }
        result.push('\n');
        result
    } else {
        content.to_owned()
    };

    // 2. Byte truncation
    let final_content = if after_lines.len() > MAX_INDEX_BYTES {
        was_byte_truncated = true;
        // Find the last newline at or before the byte limit.
        let slice = &after_lines[..MAX_INDEX_BYTES];
        match slice.rfind('\n') {
            Some(pos) => after_lines[..=pos].to_owned(),
            // No newline found — take the whole slice.
            None => slice.to_owned(),
        }
    } else {
        after_lines
    };

    let trunc = Truncation {
        original_lines,
        original_bytes,
        was_line_truncated,
        was_byte_truncated,
    };

    (final_content, trunc)
}

// ─── Helpers ───────────────────────────────────────────────────

/// Try to parse a single entry line.
///
/// Accepts:
/// - `- [Title](file.md) — Description`
/// - `- [Title](file.md) -- Description`
fn parse_entry_line(line: &str) -> Option<IndexEntry> {
    let line = line.trim();
    let rest = line.strip_prefix("- [")?;

    let close_bracket = rest.find(']')?;
    let title = rest[..close_bracket].to_owned();

    let after_bracket = &rest[close_bracket + 1..];
    let after_paren_open = after_bracket.strip_prefix('(')?;
    let close_paren = after_paren_open.find(')')?;
    let filename = after_paren_open[..close_paren].to_owned();

    let remainder = &after_paren_open[close_paren + 1..];
    let remainder = remainder.trim();

    // Accept either em-dash (—) or double-dash (--)
    let description = if let Some(desc) = remainder.strip_prefix('\u{2014}') {
        desc.trim().to_owned()
    } else if let Some(desc) = remainder.strip_prefix("--") {
        desc.trim().to_owned()
    } else {
        return None;
    };

    Some(IndexEntry {
        title,
        filename,
        description,
    })
}

// ─── Tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_index_entries() {
        let content = "\
- [No remote telemetry](project_no_remote_telemetry.md) \u{2014} All telemetry must stay local
- [Own config path](project_own_config_path.md) \u{2014} Use ~/.crab/ only\n";

        let entries = parse_index_content(content);
        assert_eq!(entries.len(), 2);

        assert_eq!(entries[0].title, "No remote telemetry");
        assert_eq!(entries[0].filename, "project_no_remote_telemetry.md");
        assert_eq!(entries[0].description, "All telemetry must stay local");

        assert_eq!(entries[1].title, "Own config path");
        assert_eq!(entries[1].filename, "project_own_config_path.md");
        assert_eq!(entries[1].description, "Use ~/.crab/ only");
    }

    #[test]
    fn parse_index_skips_non_entries() {
        let content = "\
# MEMORY.md
Some random text here
- [Valid](valid.md) \u{2014} A valid entry
Not a list item
- invalid line without brackets\n";

        let entries = parse_index_content(content);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "Valid");
    }

    #[test]
    fn parse_index_dash_dash_separator() {
        let content = "- [Title](file.md) -- description with double dash\n";
        let entries = parse_index_content(content);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "Title");
        assert_eq!(entries[0].filename, "file.md");
        assert_eq!(entries[0].description, "description with double dash");
    }

    #[test]
    fn format_index_roundtrip() {
        let entries = vec![
            IndexEntry {
                title: "Alpha".into(),
                filename: "alpha.md".into(),
                description: "First entry".into(),
            },
            IndexEntry {
                title: "Beta".into(),
                filename: "beta.md".into(),
                description: "Second entry".into(),
            },
        ];

        let formatted = format_index_content(&entries);
        let parsed = parse_index_content(&formatted);

        assert_eq!(parsed.len(), entries.len());
        for (orig, roundtripped) in entries.iter().zip(parsed.iter()) {
            assert_eq!(orig.title, roundtripped.title);
            assert_eq!(orig.filename, roundtripped.filename);
            assert_eq!(orig.description, roundtripped.description);
        }
    }

    #[test]
    fn truncate_within_limits() {
        let content = "- [A](a.md) \u{2014} short\n- [B](b.md) \u{2014} also short\n";
        let (result, trunc) = truncate_index_content(content);
        assert_eq!(result, content);
        assert!(!trunc.was_line_truncated);
        assert!(!trunc.was_byte_truncated);
    }

    #[test]
    fn truncate_exceeds_line_limit() {
        let mut content = String::new();
        for i in 0..250 {
            writeln!(content, "- [Item {i}](item_{i}.md) \u{2014} entry {i}").unwrap();
        }
        let (result, trunc) = truncate_index_content(&content);
        assert!(trunc.was_line_truncated);
        assert_eq!(trunc.original_lines, 250);
        // Result should have exactly MAX_INDEX_LINES lines.
        assert_eq!(result.lines().count(), MAX_INDEX_LINES);
    }

    #[test]
    fn truncate_exceeds_byte_limit() {
        // Create content where each line is long enough to exceed the byte limit.
        let mut content = String::new();
        for i in 0..50 {
            let padding = "x".repeat(600);
            writeln!(content, "- [Item {i}](item_{i}.md) \u{2014} {padding}").unwrap();
        }
        assert!(content.len() > MAX_INDEX_BYTES);

        let (result, trunc) = truncate_index_content(&content);
        assert!(trunc.was_byte_truncated);
        assert!(result.len() <= MAX_INDEX_BYTES);
        // Should end at a newline boundary.
        assert!(result.ends_with('\n'));
    }

    #[test]
    fn save_and_load_index() {
        let tmp = tempfile::tempdir().unwrap();
        let entries = vec![
            IndexEntry {
                title: "Foo".into(),
                filename: "foo.md".into(),
                description: "Foo description".into(),
            },
            IndexEntry {
                title: "Bar".into(),
                filename: "bar.md".into(),
                description: "Bar description".into(),
            },
        ];

        save_index(tmp.path(), &entries).unwrap();
        let loaded = load_index(tmp.path()).unwrap();

        assert_eq!(loaded.entries.len(), 2);
        assert_eq!(loaded.entries[0].title, "Foo");
        assert_eq!(loaded.entries[1].title, "Bar");
        assert!(loaded.truncation.is_none());
    }

    #[test]
    fn load_index_nonexistent_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("nonexistent");
        let index = load_index(&dir).unwrap();
        assert!(index.entries.is_empty());
        assert!(index.truncation.is_none());
    }
}
