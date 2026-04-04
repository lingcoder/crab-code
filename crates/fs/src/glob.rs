//! Glob-based file pattern matching.
//!
//! Uses [`globset`] for pattern compilation and [`ignore`] for directory
//! traversal that automatically respects `.gitignore` rules.

use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Result of a glob search.
#[derive(Debug, Clone)]
pub struct GlobResult {
    /// Matched file paths, sorted by modification time (most recent first).
    pub matches: Vec<PathBuf>,
    /// `true` when the result set was truncated at `limit`.
    pub truncated: bool,
}

/// Options controlling a glob search.
///
/// Use [`GlobOptions::new`] for defaults, then override fields as needed.
pub struct GlobOptions<'a> {
    /// Root directory to search from.
    pub root: &'a Path,
    /// Glob pattern in gitglob syntax (e.g. `**/*.rs`, `src/{a,b}/*.toml`).
    pub pattern: &'a str,
    /// Maximum number of results. `0` means unlimited.
    pub limit: usize,
    /// Whether to respect `.gitignore` rules. Default: `true`.
    pub respect_gitignore: bool,
    /// Whether to include hidden files/directories. Default: `false`.
    pub include_hidden: bool,
    /// Optional maximum directory depth (`None` = unlimited).
    pub max_depth: Option<usize>,
}

impl<'a> GlobOptions<'a> {
    /// Create options with sensible defaults.
    #[must_use]
    pub fn new(root: &'a Path, pattern: &'a str) -> Self {
        Self {
            root,
            pattern,
            limit: 0,
            respect_gitignore: true,
            include_hidden: false,
            max_depth: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Find files matching a glob pattern under a directory.
///
/// # Errors
///
/// - Invalid glob pattern.
/// - `root` does not exist or is inaccessible.
pub fn find_files(opts: &GlobOptions<'_>) -> crab_common::Result<GlobResult> {
    // 1. Compile the glob pattern
    let glob = globset::GlobBuilder::new(opts.pattern)
        .literal_separator(true)
        .build()
        .map_err(|e| crab_common::Error::Other(format!("invalid glob pattern: {e}")))?;
    let glob_set = globset::GlobSetBuilder::new()
        .add(glob)
        .build()
        .map_err(|e| crab_common::Error::Other(format!("invalid glob pattern: {e}")))?;

    // 2. Walk the directory with ignore::WalkBuilder
    let mut walker = ignore::WalkBuilder::new(opts.root);
    walker
        .hidden(!opts.include_hidden)
        .git_ignore(opts.respect_gitignore)
        .git_global(opts.respect_gitignore)
        .git_exclude(opts.respect_gitignore)
        .parents(opts.respect_gitignore);

    if let Some(depth) = opts.max_depth {
        walker.max_depth(Some(depth));
    }

    // 3. Filter against the glob set, collecting (path, mtime) pairs
    let mut entries: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();

    for entry in walker.build().flatten() {
        let path = entry.path();

        // Skip directories — we only want files
        if path.is_dir() {
            continue;
        }

        // Match against the glob using relative path from root
        let relative = path.strip_prefix(opts.root).unwrap_or(path);
        if !glob_set.is_match(relative) {
            continue;
        }

        let mtime = path
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

        entries.push((path.to_path_buf(), mtime));
    }

    // 4. Sort by mtime descending (most recent first)
    entries.sort_by(|a, b| b.1.cmp(&a.1));

    // 5. Truncate at limit
    let truncated = opts.limit > 0 && entries.len() > opts.limit;
    if truncated {
        entries.truncate(opts.limit);
    }

    Ok(GlobResult {
        matches: entries.into_iter().map(|(p, _)| p).collect(),
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup_temp_dir() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        // Create a file structure
        fs::write(tmp.path().join("main.rs"), "fn main() {}").unwrap();
        fs::write(tmp.path().join("lib.rs"), "pub mod lib;").unwrap();
        fs::write(tmp.path().join("readme.md"), "# README").unwrap();
        fs::create_dir_all(tmp.path().join("src")).unwrap();
        fs::write(tmp.path().join("src").join("util.rs"), "// util").unwrap();
        fs::write(tmp.path().join("src").join("config.toml"), "[cfg]").unwrap();
        tmp
    }

    #[test]
    fn simple_pattern() {
        let tmp = setup_temp_dir();
        let opts = GlobOptions::new(tmp.path(), "*.rs");
        let result = find_files(&opts).unwrap();
        // Should find main.rs and lib.rs (top-level only for *.rs)
        assert_eq!(result.matches.len(), 2);
        assert!(!result.truncated);
    }

    #[test]
    fn nested_glob() {
        let tmp = setup_temp_dir();
        let opts = GlobOptions::new(tmp.path(), "**/*.rs");
        let result = find_files(&opts).unwrap();
        // main.rs, lib.rs, src/util.rs
        assert_eq!(result.matches.len(), 3);
    }

    #[test]
    fn limit_truncation() {
        let tmp = setup_temp_dir();
        let mut opts = GlobOptions::new(tmp.path(), "**/*");
        opts.limit = 2;
        let result = find_files(&opts).unwrap();
        assert_eq!(result.matches.len(), 2);
        assert!(result.truncated);
    }

    #[test]
    fn mtime_sort() {
        let tmp = tempfile::tempdir().unwrap();
        // Create files with a small delay to ensure different mtimes
        fs::write(tmp.path().join("old.rs"), "old").unwrap();
        // Touch to make newer
        std::thread::sleep(std::time::Duration::from_millis(50));
        fs::write(tmp.path().join("new.rs"), "new").unwrap();

        let opts = GlobOptions::new(tmp.path(), "*.rs");
        let result = find_files(&opts).unwrap();
        assert_eq!(result.matches.len(), 2);
        // Most recent first
        assert!(
            result.matches[0].file_name().unwrap() == "new.rs",
            "expected new.rs first, got {:?}",
            result.matches[0]
        );
    }

    #[test]
    fn gitignore_respected() {
        let tmp = tempfile::tempdir().unwrap();
        // Initialize git repo so gitignore is respected
        fs::create_dir(tmp.path().join(".git")).unwrap();
        fs::write(tmp.path().join(".gitignore"), "*.log\n").unwrap();
        fs::write(tmp.path().join("app.rs"), "fn main() {}").unwrap();
        fs::write(tmp.path().join("debug.log"), "log data").unwrap();

        let opts = GlobOptions::new(tmp.path(), "**/*");
        let result = find_files(&opts).unwrap();
        // debug.log should be excluded
        let names: Vec<_> = result
            .matches
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"app.rs".to_string()));
        assert!(
            !names.contains(&"debug.log".to_string()),
            "debug.log should be ignored, got: {names:?}"
        );
    }

    #[test]
    fn invalid_pattern() {
        let tmp = tempfile::tempdir().unwrap();
        let opts = GlobOptions::new(tmp.path(), "[invalid");
        let result = find_files(&opts);
        assert!(result.is_err());
    }

    #[test]
    fn no_matches() {
        let tmp = setup_temp_dir();
        let opts = GlobOptions::new(tmp.path(), "*.xyz");
        let result = find_files(&opts).unwrap();
        assert!(result.matches.is_empty());
        assert!(!result.truncated);
    }
}
