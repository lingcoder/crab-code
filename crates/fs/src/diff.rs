//! Diff generation and edit application using [`similar`].
//!
//! Provides exact-string replacement with ambiguity checking and unified
//! diff output — the building block for the `EditTool`.

use similar::TextDiff;

// Types

/// Result of applying an edit operation.
#[derive(Debug, Clone)]
pub struct EditResult {
    /// File content *before* the edit.
    pub old_content: String,
    /// File content *after* the edit.
    pub new_content: String,
    /// Unified diff between old and new content.
    pub unified_diff: String,
    /// Number of replacements that were made.
    pub replacements: usize,
}

/// Options controlling an edit operation.
///
/// Use [`EditOptions::new`] for the common single-replacement case.
pub struct EditOptions<'a> {
    /// Full file content to edit.
    pub file_content: &'a str,
    /// Exact string to find.
    pub old_string: &'a str,
    /// Replacement string.
    pub new_string: &'a str,
    /// If `true`, replace **all** occurrences. If `false` (default), return an
    /// error when `old_string` matches more than once.
    pub replace_all: bool,
    /// Optional file-path label for the diff header (display only).
    pub file_label: Option<&'a str>,
}

impl<'a> EditOptions<'a> {
    /// Create options for a single-occurrence replacement.
    #[must_use]
    pub fn new(file_content: &'a str, old_string: &'a str, new_string: &'a str) -> Self {
        Self {
            file_content,
            old_string,
            new_string,
            replace_all: false,
            file_label: None,
        }
    }
}

// Public API

/// Apply an exact string replacement within file content.
///
/// # Errors
///
/// - `old_string` not found in `file_content`.
/// - `old_string` matches multiple times and `replace_all` is `false`.
pub fn apply_edit(opts: &EditOptions<'_>) -> crab_core::Result<EditResult> {
    let count = opts.file_content.matches(opts.old_string).count();

    if count == 0 {
        return Err(crab_core::Error::Other(
            "old_string not found in file content".into(),
        ));
    }

    if count > 1 && !opts.replace_all {
        return Err(crab_core::Error::Other(format!(
            "old_string matches {count} times; use replace_all or provide more context"
        )));
    }

    let new_content = if opts.replace_all {
        opts.file_content.replace(opts.old_string, opts.new_string)
    } else {
        // Replace first (and only) occurrence
        opts.file_content
            .replacen(opts.old_string, opts.new_string, 1)
    };

    let old_label = opts.file_label.unwrap_or("a");
    let new_label = opts.file_label.unwrap_or("b");
    let diff_str = unified_diff(opts.file_content, &new_content, old_label, new_label);

    Ok(EditResult {
        old_content: opts.file_content.to_string(),
        new_content,
        unified_diff: diff_str,
        replacements: count,
    })
}

/// Convenience wrapper matching the original skeleton signature.
///
/// Equivalent to `apply_edit(&EditOptions::new(file_content, old_string, new_string))`.
///
/// # Errors
///
/// Same as [`apply_edit`].
pub fn apply_edit_simple(
    file_content: &str,
    old_string: &str,
    new_string: &str,
) -> crab_core::Result<EditResult> {
    apply_edit(&EditOptions::new(file_content, old_string, new_string))
}

/// Generate a unified diff between two strings without applying any edit.
///
/// Useful for dry-run previews (e.g. `WriteTool` showing what will change).
#[must_use]
pub fn unified_diff(old: &str, new: &str, old_label: &str, new_label: &str) -> String {
    TextDiff::from_lines(old, new)
        .unified_diff()
        .header(old_label, new_label)
        .to_string()
}

/// Generate a unified diff with a configurable number of context lines.
#[must_use]
pub fn unified_diff_with_context(
    old: &str,
    new: &str,
    old_label: &str,
    new_label: &str,
    context_lines: usize,
) -> String {
    TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(context_lines)
        .header(old_label, new_label)
        .to_string()
}

/// Inline (word-level) diff between two strings.
///
/// Returns a vector of `(tag, text)` pairs where tag is one of:
/// - `"equal"` — unchanged text
/// - `"delete"` — text only in old
/// - `"insert"` — text only in new
/// - `"replace"` — text changed from old to new
#[must_use]
pub fn inline_diff(old: &str, new: &str) -> Vec<InlineChange> {
    let diff = TextDiff::from_words(old, new);
    diff.iter_all_changes()
        .map(|change| {
            let tag = match change.tag() {
                similar::ChangeTag::Equal => ChangeKind::Equal,
                similar::ChangeTag::Delete => ChangeKind::Delete,
                similar::ChangeTag::Insert => ChangeKind::Insert,
            };
            InlineChange {
                kind: tag,
                text: change.value().to_string(),
            }
        })
        .collect()
}

/// Kind of inline change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Equal,
    Delete,
    Insert,
}

impl std::fmt::Display for ChangeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Equal => f.write_str("equal"),
            Self::Delete => f.write_str("delete"),
            Self::Insert => f.write_str("insert"),
        }
    }
}

/// A single inline change (word-level or character-level).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineChange {
    pub kind: ChangeKind,
    pub text: String,
}

/// Summary statistics for a diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffStats {
    pub lines_added: usize,
    pub lines_removed: usize,
    pub lines_unchanged: usize,
}

/// Compute diff statistics between two strings.
#[must_use]
pub fn diff_stats(old: &str, new: &str) -> DiffStats {
    let diff = TextDiff::from_lines(old, new);
    let mut added = 0;
    let mut removed = 0;
    let mut unchanged = 0;

    for change in diff.iter_all_changes() {
        match change.tag() {
            similar::ChangeTag::Equal => unchanged += 1,
            similar::ChangeTag::Delete => removed += 1,
            similar::ChangeTag::Insert => added += 1,
        }
    }

    DiffStats {
        lines_added: added,
        lines_removed: removed,
        lines_unchanged: unchanged,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_replacement() {
        let result = apply_edit_simple("hello world", "world", "rust").unwrap();
        assert_eq!(result.new_content, "hello rust");
        assert_eq!(result.replacements, 1);
        assert!(!result.unified_diff.is_empty());
    }

    #[test]
    fn not_found() {
        let result = apply_edit_simple("hello world", "missing", "x");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn ambiguous_match() {
        let result = apply_edit_simple("aaa bbb aaa", "aaa", "ccc");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("2 times"));
    }

    #[test]
    fn replace_all() {
        let opts = EditOptions {
            file_content: "aaa bbb aaa",
            old_string: "aaa",
            new_string: "ccc",
            replace_all: true,
            file_label: None,
        };
        let result = apply_edit(&opts).unwrap();
        assert_eq!(result.new_content, "ccc bbb ccc");
        assert_eq!(result.replacements, 2);
    }

    #[test]
    fn unified_diff_format() {
        let diff = unified_diff("line1\nline2\n", "line1\nline3\n", "old.txt", "new.txt");
        assert!(diff.contains("--- old.txt"));
        assert!(diff.contains("+++ new.txt"));
        assert!(diff.contains("-line2"));
        assert!(diff.contains("+line3"));
    }

    #[test]
    fn empty_replacement_deletion() {
        let result = apply_edit_simple("hello world", " world", "").unwrap();
        assert_eq!(result.new_content, "hello");
        assert_eq!(result.replacements, 1);
    }

    #[test]
    fn multiline_edit() {
        let content = "line1\nline2\nline3\n";
        let result = apply_edit_simple(content, "line2\nline3", "replaced").unwrap();
        assert_eq!(result.new_content, "line1\nreplaced\n");
        assert_eq!(result.replacements, 1);
    }

    #[test]
    fn file_label_in_diff() {
        let opts = EditOptions {
            file_content: "old\n",
            old_string: "old",
            new_string: "new",
            replace_all: false,
            file_label: Some("src/main.rs"),
        };
        let result = apply_edit(&opts).unwrap();
        assert!(result.unified_diff.contains("src/main.rs"));
    }

    // ── Enhanced diff tests ───────────────────────────────────────

    #[test]
    fn unified_diff_with_context_lines() {
        let old = "a\nb\nc\nd\ne\nf\ng\n";
        let new = "a\nb\nC\nd\ne\nf\ng\n";
        let diff = unified_diff_with_context(old, new, "old", "new", 1);
        assert!(diff.contains("-c"));
        assert!(diff.contains("+C"));
        // With context=1, should not include distant lines
    }

    #[test]
    fn unified_diff_zero_context() {
        let old = "a\nb\nc\n";
        let new = "a\nB\nc\n";
        let diff = unified_diff_with_context(old, new, "old", "new", 0);
        assert!(diff.contains("-b"));
        assert!(diff.contains("+B"));
    }

    #[test]
    fn inline_diff_detects_word_changes() {
        let old = "hello world";
        let new = "hello rust";
        let changes = inline_diff(old, new);
        assert!(!changes.is_empty());

        let has_equal = changes.iter().any(|c| c.kind == ChangeKind::Equal);
        let has_change = changes
            .iter()
            .any(|c| c.kind == ChangeKind::Delete || c.kind == ChangeKind::Insert);
        assert!(has_equal, "should have equal parts");
        assert!(has_change, "should have changed parts");
    }

    #[test]
    fn inline_diff_identical() {
        let changes = inline_diff("same", "same");
        assert!(changes.iter().all(|c| c.kind == ChangeKind::Equal));
    }

    #[test]
    fn inline_diff_completely_different() {
        let changes = inline_diff("old", "new");
        let has_delete = changes.iter().any(|c| c.kind == ChangeKind::Delete);
        let has_insert = changes.iter().any(|c| c.kind == ChangeKind::Insert);
        assert!(has_delete);
        assert!(has_insert);
    }

    #[test]
    fn diff_stats_basic() {
        let old = "a\nb\nc\n";
        let new = "a\nB\nc\nd\n";
        let stats = diff_stats(old, new);
        assert_eq!(stats.lines_unchanged, 2); // "a\n" and "c\n"
        assert_eq!(stats.lines_removed, 1); // "b\n"
        assert_eq!(stats.lines_added, 2); // "B\n" and "d\n"
    }

    #[test]
    fn diff_stats_identical() {
        let text = "a\nb\nc\n";
        let stats = diff_stats(text, text);
        assert_eq!(stats.lines_added, 0);
        assert_eq!(stats.lines_removed, 0);
        assert_eq!(stats.lines_unchanged, 3);
    }

    #[test]
    fn diff_stats_empty_to_content() {
        let stats = diff_stats("", "new\n");
        assert_eq!(stats.lines_added, 1);
        assert_eq!(stats.lines_removed, 0);
    }

    #[test]
    fn change_kind_display() {
        assert_eq!(ChangeKind::Equal.to_string(), "equal");
        assert_eq!(ChangeKind::Delete.to_string(), "delete");
        assert_eq!(ChangeKind::Insert.to_string(), "insert");
    }
}
