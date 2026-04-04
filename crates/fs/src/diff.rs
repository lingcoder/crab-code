//! Diff generation and edit application using [`similar`].
//!
//! Provides exact-string replacement with ambiguity checking and unified
//! diff output — the building block for the `EditTool`.

use similar::TextDiff;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Apply an exact string replacement within file content.
///
/// # Errors
///
/// - `old_string` not found in `file_content`.
/// - `old_string` matches multiple times and `replace_all` is `false`.
pub fn apply_edit(opts: &EditOptions<'_>) -> crab_common::Result<EditResult> {
    let count = opts.file_content.matches(opts.old_string).count();

    if count == 0 {
        return Err(crab_common::Error::Other(
            "old_string not found in file content".into(),
        ));
    }

    if count > 1 && !opts.replace_all {
        return Err(crab_common::Error::Other(format!(
            "old_string matches {count} times; use replace_all or provide more context"
        )));
    }

    let new_content = if opts.replace_all {
        opts.file_content.replace(opts.old_string, opts.new_string)
    } else {
        // Replace first (and only) occurrence
        opts.file_content.replacen(opts.old_string, opts.new_string, 1)
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
) -> crab_common::Result<EditResult> {
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
}
