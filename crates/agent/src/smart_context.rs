//! Smart context injection: selects relevant file snippets based on user query
//! and project structure, then formats them for system prompt injection.
//!
//! Uses keyword extraction + file scoring from `project_context` to pick
//! the most relevant source fragments without exceeding a token budget.

use std::collections::HashMap;
use std::fmt::Write;
use std::path::{Path, PathBuf};

// ── Query analysis ────────────────────────────────────────────────────

/// Extracted terms from a user query that guide file selection.
#[derive(Debug, Clone, Default)]
pub struct QueryTerms {
    /// Identifiers (function/type/module names) mentioned in the query.
    pub identifiers: Vec<String>,
    /// File paths or partial paths mentioned in the query.
    pub file_hints: Vec<String>,
    /// General keywords for topic matching.
    pub keywords: Vec<String>,
}

/// Extract structured terms from a user query string.
#[must_use]
pub fn extract_query_terms(query: &str) -> QueryTerms {
    let mut terms = QueryTerms::default();

    for word in query.split_whitespace() {
        let clean = word.trim_matches(|c: char| {
            !c.is_alphanumeric() && c != '_' && c != '.' && c != '/' && c != '\\'
        });
        if clean.is_empty() {
            continue;
        }

        // File hints: contains path separators or known extensions
        if clean.contains('/') || clean.contains('\\') || has_source_extension(clean) {
            terms.file_hints.push(clean.to_string());
            continue;
        }

        // Identifiers: snake_case, CamelCase, or SCREAMING_CASE with length >= 2
        if clean.len() >= 2 && is_identifier(clean) {
            terms.identifiers.push(clean.to_string());
            continue;
        }

        // General keywords
        if clean.len() >= 3 {
            terms.keywords.push(clean.to_lowercase());
        }
    }

    terms
}

fn has_source_extension(s: &str) -> bool {
    let exts = [
        ".rs", ".js", ".ts", ".py", ".go", ".java", ".kt", ".toml", ".json",
    ];
    exts.iter().any(|ext| s.ends_with(ext))
}

fn is_identifier(s: &str) -> bool {
    let first = s.chars().next().unwrap_or('0');
    if !first.is_alphabetic() && first != '_' {
        return false;
    }
    // Must contain underscore (snake_case) or mixed case (CamelCase) or be ALL_CAPS
    let has_underscore = s.contains('_');
    let has_upper = s.chars().any(char::is_uppercase);
    let has_lower = s.chars().any(char::is_lowercase);
    has_underscore || (has_upper && has_lower)
}

// ── File relevance scoring ────────────────────────────────────────────

/// A file with its relevance score relative to a query.
#[derive(Debug, Clone)]
pub struct RelevantFile {
    pub path: PathBuf,
    /// 0.0 to 1.0 relevance score.
    pub relevance: f64,
    /// Which query terms matched.
    pub matched_terms: Vec<String>,
}

/// Score project files against extracted query terms.
///
/// Returns files sorted by relevance (highest first), limited to `max_files`.
#[must_use]
pub fn score_file_relevance(
    files: &[PathBuf],
    terms: &QueryTerms,
    max_files: usize,
) -> Vec<RelevantFile> {
    if files.is_empty()
        || (terms.identifiers.is_empty()
            && terms.file_hints.is_empty()
            && terms.keywords.is_empty())
    {
        return Vec::new();
    }

    let mut scored: Vec<RelevantFile> = files
        .iter()
        .map(|path| {
            let path_str = path.to_string_lossy().to_lowercase();
            let filename = path
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default();

            let mut score = 0.0_f64;
            let mut matched = Vec::new();

            // File hint matching (strongest signal)
            for hint in &terms.file_hints {
                let hint_lower = hint.to_lowercase();
                if path_str.contains(&hint_lower) || filename == hint_lower {
                    score += 1.0;
                    matched.push(hint.clone());
                }
            }

            // Identifier matching in path
            for ident in &terms.identifiers {
                let ident_lower = ident.to_lowercase();
                // Convert CamelCase to parts for matching
                let parts = split_identifier(&ident_lower);
                let mut ident_score = 0.0;
                for part in &parts {
                    if path_str.contains(part) {
                        ident_score += 0.3;
                    }
                }
                if path_str.contains(&ident_lower) {
                    ident_score = 0.6;
                }
                if ident_score > 0.0 {
                    score += ident_score;
                    matched.push(ident.clone());
                }
            }

            // Keyword matching in path
            for kw in &terms.keywords {
                if path_str.contains(kw) {
                    score += 0.2;
                    matched.push(kw.clone());
                }
            }

            RelevantFile {
                path: path.clone(),
                relevance: score,
                matched_terms: matched,
            }
        })
        .filter(|rf| rf.relevance > 0.0)
        .collect();

    // Normalize
    let max_score = scored.iter().map(|r| r.relevance).fold(0.0_f64, f64::max);
    if max_score > 0.0 {
        for rf in &mut scored {
            rf.relevance /= max_score;
        }
    }

    scored.sort_by(|a, b| {
        b.relevance
            .partial_cmp(&a.relevance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(max_files);
    scored
}

/// Split an identifier into lowercase parts (by underscore or case boundary).
fn split_identifier(s: &str) -> Vec<String> {
    // Already lowercase, just split on underscore
    s.split('_')
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect()
}

// ── Context window builder ────────────────────────────────────────────

/// Configuration for smart context injection.
#[derive(Debug, Clone)]
pub struct ContextConfig {
    /// Maximum approximate token budget for injected context.
    pub max_tokens: usize,
    /// Maximum number of files to include.
    pub max_files: usize,
    /// Maximum lines to include per file snippet.
    pub max_lines_per_file: usize,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            max_tokens: 4000,
            max_files: 8,
            max_lines_per_file: 50,
        }
    }
}

/// A snippet of file content selected for context injection.
#[derive(Debug, Clone)]
pub struct ContextSnippet {
    pub path: PathBuf,
    pub content: String,
    /// Approximate token count (chars / 4).
    pub token_estimate: usize,
    pub relevance: f64,
}

/// Build context snippets from relevant files, respecting the token budget.
///
/// Reads file contents from disk, truncates to `max_lines_per_file`, and
/// stops adding files once the token budget is exhausted.
#[must_use]
pub fn build_context_snippets(
    relevant_files: &[RelevantFile],
    config: &ContextConfig,
) -> Vec<ContextSnippet> {
    let mut snippets = Vec::new();
    let mut tokens_used = 0;

    for rf in relevant_files.iter().take(config.max_files) {
        let Ok(content) = std::fs::read_to_string(&rf.path) else {
            continue;
        };

        let truncated: String = content
            .lines()
            .take(config.max_lines_per_file)
            .collect::<Vec<_>>()
            .join("\n");

        let token_est = estimate_tokens(&truncated);

        if tokens_used + token_est > config.max_tokens && !snippets.is_empty() {
            break;
        }

        tokens_used += token_est;
        snippets.push(ContextSnippet {
            path: rf.path.clone(),
            content: truncated,
            token_estimate: token_est,
            relevance: rf.relevance,
        });
    }

    snippets
}

/// Build context snippets from pre-loaded content (for testing without disk I/O).
#[must_use]
pub fn build_context_snippets_from_content(
    files: &[(PathBuf, String, f64)],
    config: &ContextConfig,
) -> Vec<ContextSnippet> {
    let mut snippets = Vec::new();
    let mut tokens_used = 0;

    for (path, content, relevance) in files.iter().take(config.max_files) {
        let truncated: String = content
            .lines()
            .take(config.max_lines_per_file)
            .collect::<Vec<_>>()
            .join("\n");

        let token_est = estimate_tokens(&truncated);

        if tokens_used + token_est > config.max_tokens && !snippets.is_empty() {
            break;
        }

        tokens_used += token_est;
        snippets.push(ContextSnippet {
            path: path.clone(),
            content: truncated,
            token_estimate: token_est,
            relevance: *relevance,
        });
    }

    snippets
}

/// Format context snippets as a system prompt section.
#[must_use]
pub fn format_context_section(snippets: &[ContextSnippet]) -> String {
    if snippets.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    let _ = writeln!(out, "# Relevant Source Context\n");
    let _ = writeln!(
        out,
        "The following code snippets are automatically selected as relevant to the current query.\n"
    );

    for snippet in snippets {
        let ext = snippet
            .path
            .extension()
            .map(|e| e.to_string_lossy().to_string())
            .unwrap_or_default();
        let _ = writeln!(out, "## {}\n", snippet.path.display());
        let _ = writeln!(out, "```{ext}");
        let _ = writeln!(out, "{}", snippet.content);
        let _ = writeln!(out, "```\n");
    }

    out
}

/// Rough token estimate: chars / 4.
fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

// ── High-level API ────────────────────────────────────────────────────

/// Smart context: given a user query and list of project files, select and
/// format relevant file snippets for system prompt injection.
///
/// This is the main entry point combining extraction, scoring, reading, and formatting.
#[must_use]
pub fn smart_context_for_query(
    query: &str,
    project_files: &[PathBuf],
    config: &ContextConfig,
) -> String {
    let terms = extract_query_terms(query);
    let relevant = score_file_relevance(project_files, &terms, config.max_files);
    let snippets = build_context_snippets(&relevant, config);
    format_context_section(&snippets)
}

// ── Usage tracking ────────────────────────────────────────────────────

/// Tracks which files have been accessed across queries to improve future relevance.
#[derive(Debug, Clone, Default)]
pub struct ContextUsageTracker {
    /// File path -> access count.
    access_counts: HashMap<PathBuf, usize>,
    /// File path -> last query terms that led to its selection.
    last_terms: HashMap<PathBuf, Vec<String>>,
}

impl ContextUsageTracker {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that a file was included in context for a given set of terms.
    pub fn record_access(&mut self, path: &Path, terms: &[String]) {
        *self.access_counts.entry(path.to_path_buf()).or_insert(0) += 1;
        self.last_terms.insert(path.to_path_buf(), terms.to_vec());
    }

    /// Get the access count for a file.
    #[must_use]
    pub fn access_count(&self, path: &Path) -> usize {
        self.access_counts.get(path).copied().unwrap_or(0)
    }

    /// Get the top N most-accessed files.
    #[must_use]
    pub fn top_files(&self, n: usize) -> Vec<(PathBuf, usize)> {
        let mut entries: Vec<_> = self
            .access_counts
            .iter()
            .map(|(p, &c)| (p.clone(), c))
            .collect();
        entries.sort_by(|a, b| b.1.cmp(&a.1));
        entries.truncate(n);
        entries
    }

    /// Get total number of tracked files.
    #[must_use]
    pub fn tracked_count(&self) -> usize {
        self.access_counts.len()
    }

    /// Boost relevance scores based on historical access patterns.
    /// Returns a multiplier (1.0 = no boost, up to 1.5 for frequently accessed).
    #[must_use]
    pub fn relevance_boost(&self, path: &Path) -> f64 {
        let count = self.access_count(path);
        if count == 0 {
            1.0
        } else {
            // Logarithmic boost, capped at 1.5x
            #[allow(clippy::cast_precision_loss)]
            let boost = (count as f64).ln().mul_add(0.15, 1.0);
            boost.min(1.5)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Query term extraction ──────────────────────────────────────

    #[test]
    fn extract_empty_query() {
        let terms = extract_query_terms("");
        assert!(terms.identifiers.is_empty());
        assert!(terms.file_hints.is_empty());
        assert!(terms.keywords.is_empty());
    }

    #[test]
    fn extract_file_hints() {
        let terms = extract_query_terms("look at src/main.rs and config.toml");
        assert!(terms.file_hints.iter().any(|h| h.contains("main.rs")));
        assert!(terms.file_hints.iter().any(|h| h.contains("config.toml")));
    }

    #[test]
    fn extract_identifiers_snake_case() {
        let terms = extract_query_terms("what does query_loop do");
        assert!(terms.identifiers.contains(&"query_loop".to_string()));
    }

    #[test]
    fn extract_identifiers_camel_case() {
        let terms = extract_query_terms("explain the AgentSession struct");
        assert!(terms.identifiers.contains(&"AgentSession".to_string()));
    }

    #[test]
    fn extract_keywords() {
        let terms = extract_query_terms("how does the caching work");
        assert!(terms.keywords.contains(&"how".to_string()));
        assert!(terms.keywords.contains(&"caching".to_string()));
    }

    #[test]
    fn extract_mixed_query() {
        let terms = extract_query_terms("fix the build_system_prompt function in system_prompt.rs");
        assert!(!terms.file_hints.is_empty());
        assert!(!terms.identifiers.is_empty());
    }

    #[test]
    fn extract_short_words_ignored() {
        let terms = extract_query_terms("a is it");
        assert!(terms.keywords.is_empty());
        assert!(terms.identifiers.is_empty());
    }

    // ── File relevance scoring ─────────────────────────────────────

    #[test]
    fn score_empty_files() {
        let terms = extract_query_terms("test");
        let scored = score_file_relevance(&[], &terms, 5);
        assert!(scored.is_empty());
    }

    #[test]
    fn score_empty_terms() {
        let files = vec![PathBuf::from("src/main.rs")];
        let terms = QueryTerms::default();
        let scored = score_file_relevance(&files, &terms, 5);
        assert!(scored.is_empty());
    }

    #[test]
    fn score_file_hint_match() {
        let files = vec![
            PathBuf::from("src/main.rs"),
            PathBuf::from("src/lib.rs"),
            PathBuf::from("src/utils.rs"),
        ];
        let terms = extract_query_terms("look at src/main.rs");
        let scored = score_file_relevance(&files, &terms, 5);
        assert!(!scored.is_empty());
        assert_eq!(scored[0].path, PathBuf::from("src/main.rs"));
    }

    #[test]
    fn score_identifier_match() {
        let files = vec![
            PathBuf::from("src/query_loop.rs"),
            PathBuf::from("src/worker.rs"),
        ];
        let terms = extract_query_terms("how does query_loop work");
        let scored = score_file_relevance(&files, &terms, 5);
        assert!(!scored.is_empty());
        assert_eq!(scored[0].path, PathBuf::from("src/query_loop.rs"));
    }

    #[test]
    fn score_keyword_match() {
        let files = vec![PathBuf::from("src/cache.rs"), PathBuf::from("src/auth.rs")];
        let terms = extract_query_terms("how does the cache work");
        let scored = score_file_relevance(&files, &terms, 5);
        assert!(!scored.is_empty());
        assert_eq!(scored[0].path, PathBuf::from("src/cache.rs"));
    }

    #[test]
    fn score_respects_max_files() {
        let files: Vec<PathBuf> = (0..20)
            .map(|i| PathBuf::from(format!("src/mod_{i}.rs")))
            .collect();
        let terms = extract_query_terms("look at src/mod_5.rs and src/mod_10.rs and src/mod_15.rs");
        let scored = score_file_relevance(&files, &terms, 2);
        assert!(scored.len() <= 2);
    }

    #[test]
    fn score_normalized_to_one() {
        let files = vec![PathBuf::from("src/main.rs"), PathBuf::from("tests/test.rs")];
        let terms = extract_query_terms("main.rs");
        let scored = score_file_relevance(&files, &terms, 5);
        if let Some(first) = scored.first() {
            assert!((first.relevance - 1.0).abs() < f64::EPSILON || first.relevance <= 1.0);
        }
    }

    // ── Context snippets from content ──────────────────────────────

    #[test]
    fn build_snippets_from_content_basic() {
        let files = vec![(
            PathBuf::from("src/main.rs"),
            "fn main() {\n    println!(\"hello\");\n}".to_string(),
            1.0,
        )];
        let config = ContextConfig::default();
        let snippets = build_context_snippets_from_content(&files, &config);
        assert_eq!(snippets.len(), 1);
        assert!(snippets[0].content.contains("fn main()"));
    }

    #[test]
    fn build_snippets_respects_token_budget() {
        let big_content = "x\n".repeat(1000);
        let files = vec![
            (PathBuf::from("a.rs"), big_content.clone(), 1.0),
            (PathBuf::from("b.rs"), big_content.clone(), 0.8),
            (PathBuf::from("c.rs"), big_content, 0.6),
        ];
        let config = ContextConfig {
            max_tokens: 100,
            max_files: 10,
            max_lines_per_file: 50,
            ..Default::default()
        };
        let snippets = build_context_snippets_from_content(&files, &config);
        // Should include at least the first file (always included even if over budget)
        assert!(!snippets.is_empty());
    }

    #[test]
    fn build_snippets_truncates_lines() {
        let content = (0..100)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let files = vec![(PathBuf::from("big.rs"), content, 1.0)];
        let config = ContextConfig {
            max_lines_per_file: 10,
            ..Default::default()
        };
        let snippets = build_context_snippets_from_content(&files, &config);
        assert_eq!(snippets[0].content.lines().count(), 10);
    }

    #[test]
    fn build_snippets_empty_input() {
        let config = ContextConfig::default();
        let snippets = build_context_snippets_from_content(&[], &config);
        assert!(snippets.is_empty());
    }

    // ── Format context section ─────────────────────────────────────

    #[test]
    fn format_empty_snippets() {
        let section = format_context_section(&[]);
        assert!(section.is_empty());
    }

    #[test]
    fn format_single_snippet() {
        let snippets = vec![ContextSnippet {
            path: PathBuf::from("src/main.rs"),
            content: "fn main() {}".into(),
            token_estimate: 3,
            relevance: 1.0,
        }];
        let section = format_context_section(&snippets);
        assert!(section.contains("Relevant Source Context"));
        assert!(section.contains("src/main.rs"));
        assert!(section.contains("```rs"));
        assert!(section.contains("fn main() {}"));
    }

    #[test]
    fn format_multiple_snippets() {
        let snippets = vec![
            ContextSnippet {
                path: PathBuf::from("src/main.rs"),
                content: "fn main() {}".into(),
                token_estimate: 3,
                relevance: 1.0,
            },
            ContextSnippet {
                path: PathBuf::from("src/lib.py"),
                content: "def foo(): pass".into(),
                token_estimate: 4,
                relevance: 0.8,
            },
        ];
        let section = format_context_section(&snippets);
        assert!(section.contains("```rs"));
        assert!(section.contains("```py"));
    }

    // ── Token estimation ───────────────────────────────────────────

    #[test]
    fn estimate_tokens_empty() {
        assert_eq!(estimate_tokens(""), 0 + 3 / 4); // (0+3)/4 = 0
    }

    #[test]
    fn estimate_tokens_basic() {
        // 20 chars -> ~5 tokens
        assert_eq!(estimate_tokens("12345678901234567890"), 5);
    }

    // ── Context config defaults ────────────────────────────────────

    #[test]
    fn context_config_defaults() {
        let config = ContextConfig::default();
        assert_eq!(config.max_tokens, 4000);
        assert_eq!(config.max_files, 8);
        assert_eq!(config.max_lines_per_file, 50);
    }

    // ── Usage tracker ──────────────────────────────────────────────

    #[test]
    fn tracker_new_empty() {
        let tracker = ContextUsageTracker::new();
        assert_eq!(tracker.tracked_count(), 0);
    }

    #[test]
    fn tracker_record_access() {
        let mut tracker = ContextUsageTracker::new();
        let path = PathBuf::from("src/main.rs");
        tracker.record_access(&path, &["main".to_string()]);
        assert_eq!(tracker.access_count(&path), 1);
        tracker.record_access(&path, &["main".to_string()]);
        assert_eq!(tracker.access_count(&path), 2);
    }

    #[test]
    fn tracker_access_count_unknown() {
        let tracker = ContextUsageTracker::new();
        assert_eq!(tracker.access_count(Path::new("unknown.rs")), 0);
    }

    #[test]
    fn tracker_top_files() {
        let mut tracker = ContextUsageTracker::new();
        let a = PathBuf::from("a.rs");
        let b = PathBuf::from("b.rs");
        tracker.record_access(&a, &[]);
        tracker.record_access(&a, &[]);
        tracker.record_access(&a, &[]);
        tracker.record_access(&b, &[]);

        let top = tracker.top_files(1);
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].0, a);
        assert_eq!(top[0].1, 3);
    }

    #[test]
    fn tracker_relevance_boost_no_access() {
        let tracker = ContextUsageTracker::new();
        let boost = tracker.relevance_boost(Path::new("new.rs"));
        assert!((boost - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn tracker_relevance_boost_increases() {
        let mut tracker = ContextUsageTracker::new();
        let path = PathBuf::from("frequent.rs");
        for _ in 0..10 {
            tracker.record_access(&path, &[]);
        }
        let boost = tracker.relevance_boost(&path);
        assert!(boost > 1.0);
        assert!(boost <= 1.5);
    }

    #[test]
    fn tracker_relevance_boost_capped() {
        let mut tracker = ContextUsageTracker::new();
        let path = PathBuf::from("hot.rs");
        for _ in 0..1000 {
            tracker.record_access(&path, &[]);
        }
        let boost = tracker.relevance_boost(&path);
        assert!(boost <= 1.5);
    }

    // ── split_identifier ───────────────────────────────────────────

    #[test]
    fn split_snake_case() {
        let parts = split_identifier("query_loop");
        assert_eq!(parts, vec!["query", "loop"]);
    }

    #[test]
    fn split_single_word() {
        let parts = split_identifier("main");
        assert_eq!(parts, vec!["main"]);
    }

    // ── is_identifier ──────────────────────────────────────────────

    #[test]
    fn identifier_snake_case() {
        assert!(is_identifier("query_loop"));
    }

    #[test]
    fn identifier_camel_case() {
        assert!(is_identifier("AgentSession"));
    }

    #[test]
    fn identifier_all_lower_no_underscore_rejected() {
        assert!(!is_identifier("hello"));
    }

    #[test]
    fn identifier_starts_with_number_rejected() {
        assert!(!is_identifier("3foo"));
    }

    // ── has_source_extension ───────────────────────────────────────

    #[test]
    fn source_extension_rs() {
        assert!(has_source_extension("main.rs"));
    }

    #[test]
    fn source_extension_unknown() {
        assert!(!has_source_extension("data.csv"));
    }
}
