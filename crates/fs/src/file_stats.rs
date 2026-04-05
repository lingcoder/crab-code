//! Project file statistics — line counts and file counts by language.
//!
//! Provides [`FileStats`] for computing per-language statistics across a
//! project directory, respecting `.gitignore` rules.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

// ── Language classification ──────────────────────────────────────────

/// Recognized languages for statistics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    Rust,
    JavaScript,
    TypeScript,
    Python,
    Go,
    Java,
    Cpp,
    C,
    Ruby,
    Swift,
    Shell,
    Toml,
    Yaml,
    Json,
    Markdown,
    Html,
    Css,
    Sql,
    Other,
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rust => write!(f, "Rust"),
            Self::JavaScript => write!(f, "JavaScript"),
            Self::TypeScript => write!(f, "TypeScript"),
            Self::Python => write!(f, "Python"),
            Self::Go => write!(f, "Go"),
            Self::Java => write!(f, "Java"),
            Self::Cpp => write!(f, "C++"),
            Self::C => write!(f, "C"),
            Self::Ruby => write!(f, "Ruby"),
            Self::Swift => write!(f, "Swift"),
            Self::Shell => write!(f, "Shell"),
            Self::Toml => write!(f, "TOML"),
            Self::Yaml => write!(f, "YAML"),
            Self::Json => write!(f, "JSON"),
            Self::Markdown => write!(f, "Markdown"),
            Self::Html => write!(f, "HTML"),
            Self::Css => write!(f, "CSS"),
            Self::Sql => write!(f, "SQL"),
            Self::Other => write!(f, "Other"),
        }
    }
}

/// Map file extension to language.
#[must_use]
pub fn language_from_extension(ext: &str) -> Language {
    match ext.to_lowercase().as_str() {
        "rs" => Language::Rust,
        "js" | "mjs" | "cjs" | "jsx" => Language::JavaScript,
        "ts" | "mts" | "cts" | "tsx" => Language::TypeScript,
        "py" | "pyw" | "pyi" => Language::Python,
        "go" => Language::Go,
        "java" => Language::Java,
        "cpp" | "cxx" | "cc" | "hpp" | "hxx" | "hh" => Language::Cpp,
        "c" | "h" => Language::C,
        "rb" | "rake" => Language::Ruby,
        "swift" => Language::Swift,
        "sh" | "bash" | "zsh" | "fish" => Language::Shell,
        "toml" => Language::Toml,
        "yml" | "yaml" => Language::Yaml,
        "json" | "jsonc" => Language::Json,
        "md" | "mdx" => Language::Markdown,
        "html" | "htm" => Language::Html,
        "css" | "scss" | "sass" | "less" => Language::Css,
        "sql" => Language::Sql,
        _ => Language::Other,
    }
}

// ── Stats ────────────────────────────────────────────────────────────

/// Per-language statistics.
#[derive(Debug, Clone, Default, Serialize)]
pub struct LanguageStats {
    /// Number of files.
    pub files: usize,
    /// Total lines (including blanks and comments).
    pub lines: u64,
    /// Blank lines.
    pub blank_lines: u64,
}

/// Overall project file statistics.
#[derive(Debug, Clone, Serialize)]
pub struct FileStats {
    /// Per-language breakdown.
    pub by_language: BTreeMap<Language, LanguageStats>,
    /// Total number of files counted.
    pub total_files: usize,
    /// Total lines across all files.
    pub total_lines: u64,
    /// Total blank lines.
    pub total_blank_lines: u64,
}

impl FileStats {
    /// Compute statistics for a directory, respecting `.gitignore`.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory cannot be walked.
    pub fn compute(root: &Path) -> crab_common::Result<Self> {
        let mut by_language: BTreeMap<Language, LanguageStats> = BTreeMap::new();
        let mut total_files = 0usize;
        let mut total_lines = 0u64;
        let mut total_blank_lines = 0u64;

        let walker = ignore::WalkBuilder::new(root)
            .hidden(true) // skip hidden by default
            .git_ignore(true)
            .build();

        for entry in walker {
            let entry = entry.map_err(|e| crab_common::Error::Other(format!("walk error: {e}")))?;

            // Only count files
            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                continue;
            }

            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

            if ext.is_empty() {
                continue; // skip extensionless files
            }

            let lang = language_from_extension(ext);

            // Count lines
            let Ok(content) = std::fs::read_to_string(path) else {
                continue; // skip binary / unreadable files
            };

            let lines = content.lines().count() as u64;
            let blanks = content.lines().filter(|l| l.trim().is_empty()).count() as u64;

            let stats = by_language.entry(lang).or_default();
            stats.files += 1;
            stats.lines += lines;
            stats.blank_lines += blanks;

            total_files += 1;
            total_lines += lines;
            total_blank_lines += blanks;
        }

        Ok(Self {
            by_language,
            total_files,
            total_lines,
            total_blank_lines,
        })
    }

    /// Compute stats from an explicit list of files (for testing or selective analysis).
    pub fn compute_files(files: &[&Path]) -> crab_common::Result<Self> {
        let mut by_language: BTreeMap<Language, LanguageStats> = BTreeMap::new();
        let mut total_files = 0usize;
        let mut total_lines = 0u64;
        let mut total_blank_lines = 0u64;

        for path in files {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

            if ext.is_empty() {
                continue;
            }

            let lang = language_from_extension(ext);
            let content = std::fs::read_to_string(path)?;
            let lines = content.lines().count() as u64;
            let blanks = content.lines().filter(|l| l.trim().is_empty()).count() as u64;

            let stats = by_language.entry(lang).or_default();
            stats.files += 1;
            stats.lines += lines;
            stats.blank_lines += blanks;

            total_files += 1;
            total_lines += lines;
            total_blank_lines += blanks;
        }

        Ok(Self {
            by_language,
            total_files,
            total_lines,
            total_blank_lines,
        })
    }

    /// Languages sorted by line count (descending).
    #[must_use]
    pub fn sorted_by_lines(&self) -> Vec<(Language, &LanguageStats)> {
        let mut sorted: Vec<_> = self.by_language.iter().map(|(l, s)| (*l, s)).collect();
        sorted.sort_by(|a, b| b.1.lines.cmp(&a.1.lines));
        sorted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    // ── Language ─────────────────────────────────────────────────────

    #[test]
    fn language_display() {
        assert_eq!(Language::Rust.to_string(), "Rust");
        assert_eq!(Language::TypeScript.to_string(), "TypeScript");
        assert_eq!(Language::Cpp.to_string(), "C++");
        assert_eq!(Language::Other.to_string(), "Other");
    }

    #[test]
    fn language_from_ext() {
        assert_eq!(language_from_extension("rs"), Language::Rust);
        assert_eq!(language_from_extension("js"), Language::JavaScript);
        assert_eq!(language_from_extension("tsx"), Language::TypeScript);
        assert_eq!(language_from_extension("py"), Language::Python);
        assert_eq!(language_from_extension("go"), Language::Go);
        assert_eq!(language_from_extension("java"), Language::Java);
        assert_eq!(language_from_extension("cpp"), Language::Cpp);
        assert_eq!(language_from_extension("c"), Language::C);
        assert_eq!(language_from_extension("h"), Language::C);
        assert_eq!(language_from_extension("rb"), Language::Ruby);
        assert_eq!(language_from_extension("swift"), Language::Swift);
        assert_eq!(language_from_extension("sh"), Language::Shell);
        assert_eq!(language_from_extension("toml"), Language::Toml);
        assert_eq!(language_from_extension("yml"), Language::Yaml);
        assert_eq!(language_from_extension("json"), Language::Json);
        assert_eq!(language_from_extension("md"), Language::Markdown);
        assert_eq!(language_from_extension("html"), Language::Html);
        assert_eq!(language_from_extension("css"), Language::Css);
        assert_eq!(language_from_extension("sql"), Language::Sql);
        assert_eq!(language_from_extension("xyz"), Language::Other);
    }

    #[test]
    fn language_from_ext_case_insensitive() {
        assert_eq!(language_from_extension("RS"), Language::Rust);
        assert_eq!(language_from_extension("Py"), Language::Python);
    }

    // ── FileStats ────────────────────────────────────────────────────

    #[test]
    fn stats_empty_dir() {
        let dir = tempdir().unwrap();
        let stats = FileStats::compute(dir.path()).unwrap();
        assert_eq!(stats.total_files, 0);
        assert_eq!(stats.total_lines, 0);
        assert!(stats.by_language.is_empty());
    }

    #[test]
    fn stats_single_rust_file() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("main.rs"),
            "fn main() {\n    println!(\"hi\");\n}\n",
        )
        .unwrap();

        let stats = FileStats::compute(dir.path()).unwrap();
        assert_eq!(stats.total_files, 1);
        assert_eq!(stats.total_lines, 3);
        let rust = stats.by_language.get(&Language::Rust).unwrap();
        assert_eq!(rust.files, 1);
        assert_eq!(rust.lines, 3);
    }

    #[test]
    fn stats_counts_blanks() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("lib.rs"), "use std;\n\nfn foo() {}\n\n").unwrap();

        let stats = FileStats::compute(dir.path()).unwrap();
        let rust = stats.by_language.get(&Language::Rust).unwrap();
        assert_eq!(rust.blank_lines, 2);
        assert_eq!(stats.total_blank_lines, 2);
    }

    #[test]
    fn stats_multiple_languages() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("main.rs"), "fn main() {}\n").unwrap();
        fs::write(dir.path().join("app.js"), "console.log('hi');\n").unwrap();
        fs::write(dir.path().join("readme.md"), "# Title\n\nContent\n").unwrap();

        let stats = FileStats::compute(dir.path()).unwrap();
        assert_eq!(stats.total_files, 3);
        assert!(stats.by_language.contains_key(&Language::Rust));
        assert!(stats.by_language.contains_key(&Language::JavaScript));
        assert!(stats.by_language.contains_key(&Language::Markdown));
    }

    #[test]
    fn stats_skips_extensionless() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("Makefile"), "all:\n\techo hi\n").unwrap();
        fs::write(dir.path().join("main.rs"), "fn main() {}\n").unwrap();

        let stats = FileStats::compute(dir.path()).unwrap();
        assert_eq!(stats.total_files, 1); // only main.rs
    }

    #[test]
    fn stats_sorted_by_lines() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("small.rs"), "fn a() {}\n").unwrap();
        fs::write(
            dir.path().join("big.py"),
            "a = 1\nb = 2\nc = 3\nd = 4\ne = 5\n",
        )
        .unwrap();

        let stats = FileStats::compute(dir.path()).unwrap();
        let sorted = stats.sorted_by_lines();
        assert_eq!(sorted[0].0, Language::Python);
        assert_eq!(sorted[1].0, Language::Rust);
    }

    #[test]
    fn stats_compute_files_explicit() {
        let dir = tempdir().unwrap();
        let rs = dir.path().join("a.rs");
        let py = dir.path().join("b.py");
        fs::write(&rs, "fn a() {}\nfn b() {}\n").unwrap();
        fs::write(&py, "x = 1\n").unwrap();

        let stats = FileStats::compute_files(&[rs.as_path(), py.as_path()]).unwrap();
        assert_eq!(stats.total_files, 2);
        assert_eq!(stats.total_lines, 3);
    }

    #[test]
    fn stats_serializes() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("x.rs"), "fn x() {}\n").unwrap();
        let stats = FileStats::compute(dir.path()).unwrap();
        let json = serde_json::to_string(&stats).unwrap();
        assert!(json.contains("total_files"));
        assert!(json.contains("rust"));
    }

    #[test]
    fn language_stats_default() {
        let ls = LanguageStats::default();
        assert_eq!(ls.files, 0);
        assert_eq!(ls.lines, 0);
        assert_eq!(ls.blank_lines, 0);
    }
}
