//! Content search using regex pattern matching.
//!
//! Uses [`regex`] for pattern compilation and [`ignore`] for directory
//! traversal that respects `.gitignore` rules. Binary files are silently
//! skipped.

use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A single match from a content search.
#[derive(Debug, Clone)]
pub struct GrepMatch {
    /// File containing the match.
    pub path: PathBuf,
    /// 1-based line number of the match.
    pub line_number: usize,
    /// The matched line content (trailing newline stripped).
    pub line_content: String,
    /// Context lines *before* the match (when `context_lines > 0`).
    pub context_before: Vec<String>,
    /// Context lines *after* the match (when `context_lines > 0`).
    pub context_after: Vec<String>,
}

/// Options controlling a content search.
pub struct GrepOptions {
    /// Regex pattern to search for.
    pub pattern: String,
    /// Root path — may be a single file or a directory.
    pub path: PathBuf,
    /// Enable case-insensitive matching.
    pub case_insensitive: bool,
    /// Optional file-name glob filter (e.g. `"*.rs"`). Only files whose name
    /// matches are searched.
    pub file_glob: Option<String>,
    /// Maximum number of matches to return. `0` means unlimited.
    pub max_results: usize,
    /// Number of context lines to capture before and after each match.
    pub context_lines: usize,
    /// Whether to respect `.gitignore` rules. Default: `true`.
    pub respect_gitignore: bool,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Search file contents by regex pattern.
///
/// # Errors
///
/// - Invalid regex pattern.
/// - `path` does not exist or is inaccessible.
pub fn search(opts: &GrepOptions) -> crab_common::Result<Vec<GrepMatch>> {
    let re = regex::RegexBuilder::new(&opts.pattern)
        .case_insensitive(opts.case_insensitive)
        .build()
        .map_err(|e| crab_common::Error::Other(format!("invalid regex: {e}")))?;

    let file_glob = if let Some(ref glob_pat) = opts.file_glob {
        Some(
            globset::GlobBuilder::new(glob_pat)
                .build()
                .map_err(|e| crab_common::Error::Other(format!("invalid file glob: {e}")))?
                .compile_matcher(),
        )
    } else {
        None
    };

    let mut matches = Vec::new();
    let max = if opts.max_results == 0 {
        usize::MAX
    } else {
        opts.max_results
    };

    if opts.path.is_file() {
        // Single file search
        if let Ok(file_matches) = search_file(&opts.path, &re, opts.context_lines) {
            for m in file_matches {
                if matches.len() >= max {
                    break;
                }
                matches.push(m);
            }
        }
    } else {
        // Directory walk
        let mut walker = ignore::WalkBuilder::new(&opts.path);
        walker
            .hidden(true)
            .git_ignore(opts.respect_gitignore)
            .git_global(opts.respect_gitignore)
            .git_exclude(opts.respect_gitignore)
            .parents(opts.respect_gitignore);

        for entry in walker.build().flatten() {
            if matches.len() >= max {
                break;
            }

            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            // Apply file glob filter
            if let Some(ref glob) = file_glob {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                if !glob.is_match(&name) {
                    continue;
                }
            }

            if let Ok(file_matches) = search_file(path, &re, opts.context_lines) {
                for m in file_matches {
                    if matches.len() >= max {
                        break;
                    }
                    matches.push(m);
                }
            }
        }
    }

    Ok(matches)
}

/// Search a single file and return all matches.
///
/// # Errors
///
/// Returns an error if the file cannot be read.
pub(crate) fn search_file(
    path: &Path,
    regex: &regex::Regex,
    context_lines: usize,
) -> crab_common::Result<Vec<GrepMatch>> {
    let content = std::fs::read(path)?;

    // Skip binary files (contain NUL bytes)
    if content.contains(&0) {
        return Ok(Vec::new());
    }

    let Ok(text) = String::from_utf8(content) else {
        return Ok(Vec::new()); // Non-UTF8, skip
    };

    let lines: Vec<&str> = text.lines().collect();
    let mut matches = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        if regex.is_match(line) {
            let context_before: Vec<String> = if context_lines > 0 {
                let start = i.saturating_sub(context_lines);
                lines[start..i].iter().map(|&s| s.to_string()).collect()
            } else {
                Vec::new()
            };

            let context_after: Vec<String> = if context_lines > 0 {
                let end = (i + 1 + context_lines).min(lines.len());
                lines[i + 1..end].iter().map(|&s| s.to_string()).collect()
            } else {
                Vec::new()
            };

            matches.push(GrepMatch {
                path: path.to_path_buf(),
                line_number: i + 1, // 1-based
                line_content: (*line).to_string(),
                context_before,
                context_after,
            });
        }
    }

    Ok(matches)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn create_test_file(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn simple_match() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_file(tmp.path(), "test.txt", "hello world\ngoodbye world\n");

        let opts = GrepOptions {
            pattern: "hello".into(),
            path: tmp.path().to_path_buf(),
            case_insensitive: false,
            file_glob: None,
            max_results: 0,
            context_lines: 0,
            respect_gitignore: false,
        };
        let results = search(&opts).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].line_number, 1);
        assert_eq!(results[0].line_content, "hello world");
    }

    #[test]
    fn regex_match() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_file(
            tmp.path(),
            "code.rs",
            "fn main() {}\nfn helper() {}\nlet x = 5;\n",
        );

        let opts = GrepOptions {
            pattern: r"fn\s+\w+".into(),
            path: tmp.path().to_path_buf(),
            case_insensitive: false,
            file_glob: None,
            max_results: 0,
            context_lines: 0,
            respect_gitignore: false,
        };
        let results = search(&opts).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn case_insensitive() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_file(tmp.path(), "test.txt", "Hello World\nhello world\n");

        let opts = GrepOptions {
            pattern: "hello".into(),
            path: tmp.path().to_path_buf(),
            case_insensitive: true,
            file_glob: None,
            max_results: 0,
            context_lines: 0,
            respect_gitignore: false,
        };
        let results = search(&opts).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn context_lines() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_file(
            tmp.path(),
            "ctx.txt",
            "line1\nline2\nTARGET\nline4\nline5\n",
        );

        let opts = GrepOptions {
            pattern: "TARGET".into(),
            path: tmp.path().to_path_buf(),
            case_insensitive: false,
            file_glob: None,
            max_results: 0,
            context_lines: 1,
            respect_gitignore: false,
        };
        let results = search(&opts).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].context_before, vec!["line2"]);
        assert_eq!(results[0].context_after, vec!["line4"]);
    }

    #[test]
    fn file_glob_filter() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_file(tmp.path(), "code.rs", "hello\n");
        create_test_file(tmp.path(), "doc.md", "hello\n");

        let opts = GrepOptions {
            pattern: "hello".into(),
            path: tmp.path().to_path_buf(),
            case_insensitive: false,
            file_glob: Some("*.rs".into()),
            max_results: 0,
            context_lines: 0,
            respect_gitignore: false,
        };
        let results = search(&opts).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].path.to_string_lossy().contains("code.rs"));
    }

    #[test]
    fn max_results() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_file(
            tmp.path(),
            "many.txt",
            "match1\nmatch2\nmatch3\nmatch4\nmatch5\n",
        );

        let opts = GrepOptions {
            pattern: "match".into(),
            path: tmp.path().to_path_buf(),
            case_insensitive: false,
            file_glob: None,
            max_results: 2,
            context_lines: 0,
            respect_gitignore: false,
        };
        let results = search(&opts).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn binary_file_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let binary_path = tmp.path().join("binary.bin");
        fs::write(&binary_path, b"hello\x00world").unwrap();

        let opts = GrepOptions {
            pattern: "hello".into(),
            path: tmp.path().to_path_buf(),
            case_insensitive: false,
            file_glob: None,
            max_results: 0,
            context_lines: 0,
            respect_gitignore: false,
        };
        let results = search(&opts).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn single_file_path() {
        let tmp = tempfile::tempdir().unwrap();
        let file = create_test_file(tmp.path(), "single.txt", "find me\nnot me\n");

        let opts = GrepOptions {
            pattern: "find".into(),
            path: file,
            case_insensitive: false,
            file_glob: None,
            max_results: 0,
            context_lines: 0,
            respect_gitignore: false,
        };
        let results = search(&opts).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].line_content, "find me");
    }

    #[test]
    fn invalid_regex() {
        let tmp = tempfile::tempdir().unwrap();
        let opts = GrepOptions {
            pattern: "[invalid".into(),
            path: tmp.path().to_path_buf(),
            case_insensitive: false,
            file_glob: None,
            max_results: 0,
            context_lines: 0,
            respect_gitignore: false,
        };
        let result = search(&opts);
        assert!(result.is_err());
    }
}
