//! Content search using the ripgrep crate family.
//!
//! Uses [`grep_regex`] for pattern compilation, [`grep_searcher`] for efficient
//! line-oriented searching (with binary detection and streaming I/O), and
//! [`ignore`] for directory traversal that respects `.gitignore` rules.

use std::path::{Path, PathBuf};

use grep_matcher::Matcher;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::UTF8;
use grep_searcher::{BinaryDetection, SearcherBuilder};

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
/// Uses the ripgrep crate family (`grep-regex`, `grep-searcher`, `ignore`)
/// for efficient, streaming search with built-in binary detection.
///
/// # Errors
///
/// - Invalid regex pattern.
/// - `path` does not exist or is inaccessible.
pub fn search(opts: &GrepOptions) -> crab_common::Result<Vec<GrepMatch>> {
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(opts.case_insensitive)
        .build(&opts.pattern)
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

    let max = if opts.max_results == 0 {
        usize::MAX
    } else {
        opts.max_results
    };

    let mut all_matches = Vec::new();

    if opts.path.is_file() {
        search_file_grep(
            &opts.path,
            &matcher,
            opts.context_lines,
            max,
            &mut all_matches,
        )?;
    } else {
        let mut walker = ignore::WalkBuilder::new(&opts.path);
        walker
            .hidden(true)
            .git_ignore(opts.respect_gitignore)
            .git_global(opts.respect_gitignore)
            .git_exclude(opts.respect_gitignore)
            .parents(opts.respect_gitignore);

        for entry in walker.build().flatten() {
            if all_matches.len() >= max {
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

            let remaining = max - all_matches.len();
            search_file_grep(
                path,
                &matcher,
                opts.context_lines,
                remaining,
                &mut all_matches,
            )?;
        }
    }

    Ok(all_matches)
}

// ---------------------------------------------------------------------------
// Internal: file-level search using grep-searcher
// ---------------------------------------------------------------------------

/// Search a single file using `grep-searcher` with binary detection.
fn search_file_grep(
    path: &Path,
    matcher: &grep_regex::RegexMatcher,
    context_lines: usize,
    max_matches: usize,
    results: &mut Vec<GrepMatch>,
) -> crab_common::Result<()> {
    // When context is requested, we need a two-pass approach:
    // first collect all matching line numbers, then re-read to extract context.
    // For the no-context case, we stream directly.
    if context_lines > 0 {
        search_file_with_context(path, matcher, context_lines, max_matches, results)
    } else {
        search_file_no_context(path, matcher, max_matches, results)
    }
}

/// Streaming search without context lines — uses `grep_searcher::Searcher`.
fn search_file_no_context(
    path: &Path,
    matcher: &grep_regex::RegexMatcher,
    max_matches: usize,
    results: &mut Vec<GrepMatch>,
) -> crab_common::Result<()> {
    let mut searcher = SearcherBuilder::new()
        .binary_detection(BinaryDetection::quit(0))
        .line_number(true)
        .build();

    let path_buf = path.to_path_buf();

    // grep_searcher errors are non-fatal (binary file quit, encoding, etc.)
    let _ = searcher.search_path(
        matcher,
        path,
        UTF8(|line_number, line_content| {
            if results.len() >= max_matches {
                return Ok(false); // stop searching
            }
            results.push(GrepMatch {
                path: path_buf.clone(),
                line_number: line_number as usize,
                line_content: line_content.trim_end_matches('\n').to_string(),
                context_before: Vec::new(),
                context_after: Vec::new(),
            });
            Ok(true)
        }),
    );

    Ok(())
}

/// Search with context lines. Reads the file to collect lines, then matches.
///
/// `grep-searcher` does support context via `SearcherBuilder::after_context()`
/// and `before_context()`, but the sink API for context is more complex
/// (`SinkContext`). We use a simpler approach: collect matches first, then
/// extract context from the line buffer.
fn search_file_with_context(
    path: &Path,
    matcher: &grep_regex::RegexMatcher,
    context_lines: usize,
    max_matches: usize,
    results: &mut Vec<GrepMatch>,
) -> crab_common::Result<()> {
    // Read the file — grep-searcher handles binary detection
    let content = std::fs::read(path)?;

    // Quick binary check (same heuristic as grep-searcher)
    if content.contains(&0) {
        return Ok(());
    }

    let Ok(text) = String::from_utf8(content) else {
        return Ok(());
    };

    let lines: Vec<&str> = text.lines().collect();

    for (i, line) in lines.iter().enumerate() {
        if results.len() >= max_matches {
            break;
        }

        if matcher.is_match(line.as_bytes()).unwrap_or(false) {
            let context_before: Vec<String> = {
                let start = i.saturating_sub(context_lines);
                lines[start..i].iter().map(|&s| s.to_string()).collect()
            };

            let context_after: Vec<String> = {
                let end = (i + 1 + context_lines).min(lines.len());
                lines[i + 1..end].iter().map(|&s| s.to_string()).collect()
            };

            results.push(GrepMatch {
                path: path.to_path_buf(),
                line_number: i + 1, // 1-based
                line_content: (*line).to_string(),
                context_before,
                context_after,
            });
        }
    }

    Ok(())
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
