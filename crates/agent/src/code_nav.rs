//! Code navigation: go-to-definition and find-references via pattern matching.
//!
//! Provides lightweight symbol resolution without a full LSP. Works by scanning
//! source files with language-specific regex patterns to locate definitions,
//! references, and implementations.

use std::fmt::Write;
use std::path::{Path, PathBuf};

// ── Symbol types ──────────────────────────────────────────────────────

/// Kind of symbol being navigated to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Function,
    Struct,
    Enum,
    Trait,
    Impl,
    Type,
    Const,
    Module,
    Class,
    Interface,
    Variable,
}

impl std::fmt::Display for SymbolKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Function => write!(f, "function"),
            Self::Struct => write!(f, "struct"),
            Self::Enum => write!(f, "enum"),
            Self::Trait => write!(f, "trait"),
            Self::Impl => write!(f, "impl"),
            Self::Type => write!(f, "type"),
            Self::Const => write!(f, "const"),
            Self::Module => write!(f, "module"),
            Self::Class => write!(f, "class"),
            Self::Interface => write!(f, "interface"),
            Self::Variable => write!(f, "variable"),
        }
    }
}

/// A located symbol definition or reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolLocation {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
    pub kind: SymbolKind,
    /// The matched line content (trimmed).
    pub context: String,
}

impl SymbolLocation {
    /// Format as "path:line:column".
    #[must_use]
    pub fn to_location_string(&self) -> String {
        format!("{}:{}:{}", self.path.display(), self.line, self.column)
    }
}

// ── Language detection ────────────────────────────────────────────────

/// Supported languages for pattern matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    JavaScript,
    TypeScript,
    Python,
    Go,
    Unknown,
}

/// Detect language from file extension.
#[must_use]
pub fn detect_language(path: &Path) -> Language {
    match path.extension().and_then(|e| e.to_str()) {
        Some("rs") => Language::Rust,
        Some("js" | "jsx" | "mjs") => Language::JavaScript,
        Some("ts" | "tsx") => Language::TypeScript,
        Some("py") => Language::Python,
        Some("go") => Language::Go,
        _ => Language::Unknown,
    }
}

// ── Definition patterns ───────────────────────────────────────────────

/// A pattern that matches a symbol definition in source code.
struct DefPattern {
    /// Prefix to search for before the symbol name.
    prefix: &'static str,
    kind: SymbolKind,
    language: Language,
}

const DEF_PATTERNS: &[DefPattern] = &[
    // Rust
    DefPattern {
        prefix: "fn ",
        kind: SymbolKind::Function,
        language: Language::Rust,
    },
    DefPattern {
        prefix: "pub fn ",
        kind: SymbolKind::Function,
        language: Language::Rust,
    },
    DefPattern {
        prefix: "pub(crate) fn ",
        kind: SymbolKind::Function,
        language: Language::Rust,
    },
    DefPattern {
        prefix: "async fn ",
        kind: SymbolKind::Function,
        language: Language::Rust,
    },
    DefPattern {
        prefix: "pub async fn ",
        kind: SymbolKind::Function,
        language: Language::Rust,
    },
    DefPattern {
        prefix: "struct ",
        kind: SymbolKind::Struct,
        language: Language::Rust,
    },
    DefPattern {
        prefix: "pub struct ",
        kind: SymbolKind::Struct,
        language: Language::Rust,
    },
    DefPattern {
        prefix: "enum ",
        kind: SymbolKind::Enum,
        language: Language::Rust,
    },
    DefPattern {
        prefix: "pub enum ",
        kind: SymbolKind::Enum,
        language: Language::Rust,
    },
    DefPattern {
        prefix: "trait ",
        kind: SymbolKind::Trait,
        language: Language::Rust,
    },
    DefPattern {
        prefix: "pub trait ",
        kind: SymbolKind::Trait,
        language: Language::Rust,
    },
    DefPattern {
        prefix: "impl ",
        kind: SymbolKind::Impl,
        language: Language::Rust,
    },
    DefPattern {
        prefix: "type ",
        kind: SymbolKind::Type,
        language: Language::Rust,
    },
    DefPattern {
        prefix: "pub type ",
        kind: SymbolKind::Type,
        language: Language::Rust,
    },
    DefPattern {
        prefix: "const ",
        kind: SymbolKind::Const,
        language: Language::Rust,
    },
    DefPattern {
        prefix: "pub const ",
        kind: SymbolKind::Const,
        language: Language::Rust,
    },
    DefPattern {
        prefix: "mod ",
        kind: SymbolKind::Module,
        language: Language::Rust,
    },
    DefPattern {
        prefix: "pub mod ",
        kind: SymbolKind::Module,
        language: Language::Rust,
    },
    // JavaScript / TypeScript
    DefPattern {
        prefix: "function ",
        kind: SymbolKind::Function,
        language: Language::JavaScript,
    },
    DefPattern {
        prefix: "export function ",
        kind: SymbolKind::Function,
        language: Language::JavaScript,
    },
    DefPattern {
        prefix: "async function ",
        kind: SymbolKind::Function,
        language: Language::JavaScript,
    },
    DefPattern {
        prefix: "export async function ",
        kind: SymbolKind::Function,
        language: Language::JavaScript,
    },
    DefPattern {
        prefix: "class ",
        kind: SymbolKind::Class,
        language: Language::JavaScript,
    },
    DefPattern {
        prefix: "export class ",
        kind: SymbolKind::Class,
        language: Language::JavaScript,
    },
    DefPattern {
        prefix: "const ",
        kind: SymbolKind::Variable,
        language: Language::JavaScript,
    },
    DefPattern {
        prefix: "export const ",
        kind: SymbolKind::Variable,
        language: Language::JavaScript,
    },
    DefPattern {
        prefix: "let ",
        kind: SymbolKind::Variable,
        language: Language::JavaScript,
    },
    // TypeScript-specific
    DefPattern {
        prefix: "function ",
        kind: SymbolKind::Function,
        language: Language::TypeScript,
    },
    DefPattern {
        prefix: "export function ",
        kind: SymbolKind::Function,
        language: Language::TypeScript,
    },
    DefPattern {
        prefix: "async function ",
        kind: SymbolKind::Function,
        language: Language::TypeScript,
    },
    DefPattern {
        prefix: "export async function ",
        kind: SymbolKind::Function,
        language: Language::TypeScript,
    },
    DefPattern {
        prefix: "class ",
        kind: SymbolKind::Class,
        language: Language::TypeScript,
    },
    DefPattern {
        prefix: "export class ",
        kind: SymbolKind::Class,
        language: Language::TypeScript,
    },
    DefPattern {
        prefix: "interface ",
        kind: SymbolKind::Interface,
        language: Language::TypeScript,
    },
    DefPattern {
        prefix: "export interface ",
        kind: SymbolKind::Interface,
        language: Language::TypeScript,
    },
    DefPattern {
        prefix: "type ",
        kind: SymbolKind::Type,
        language: Language::TypeScript,
    },
    DefPattern {
        prefix: "export type ",
        kind: SymbolKind::Type,
        language: Language::TypeScript,
    },
    DefPattern {
        prefix: "const ",
        kind: SymbolKind::Variable,
        language: Language::TypeScript,
    },
    DefPattern {
        prefix: "export const ",
        kind: SymbolKind::Variable,
        language: Language::TypeScript,
    },
    // Python
    DefPattern {
        prefix: "def ",
        kind: SymbolKind::Function,
        language: Language::Python,
    },
    DefPattern {
        prefix: "async def ",
        kind: SymbolKind::Function,
        language: Language::Python,
    },
    DefPattern {
        prefix: "class ",
        kind: SymbolKind::Class,
        language: Language::Python,
    },
    // Go
    DefPattern {
        prefix: "func ",
        kind: SymbolKind::Function,
        language: Language::Go,
    },
    DefPattern {
        prefix: "type ",
        kind: SymbolKind::Type,
        language: Language::Go,
    },
];

// ── Core navigation functions ─────────────────────────────────────────

/// Find definitions of a symbol by scanning source files.
///
/// Searches for lines matching known definition patterns (e.g., `fn foo`,
/// `struct Foo`, `class Foo`) in the given files.
#[must_use]
pub fn find_definitions(symbol: &str, files: &[PathBuf]) -> Vec<SymbolLocation> {
    let mut results = Vec::new();

    for file_path in files {
        let lang = detect_language(file_path);
        if lang == Language::Unknown {
            continue;
        }

        let Ok(content) = std::fs::read_to_string(file_path) else {
            continue;
        };

        find_definitions_in_content(symbol, &content, file_path, lang, &mut results);
    }

    results
}

/// Find definitions within already-loaded content (for testing).
pub fn find_definitions_in_content(
    symbol: &str,
    content: &str,
    file_path: &Path,
    lang: Language,
    results: &mut Vec<SymbolLocation>,
) {
    let patterns: Vec<&DefPattern> = DEF_PATTERNS.iter().filter(|p| p.language == lang).collect();

    for (line_idx, line) in content.lines().enumerate() {
        let trimmed = line.trim_start();
        for pat in &patterns {
            if let Some(rest) = trimmed.strip_prefix(pat.prefix) {
                // Check if the symbol name follows the prefix
                if let Some(after) = rest.strip_prefix(symbol) {
                    // Verify it's a word boundary (next char is not alphanumeric/underscore)
                    if after.is_empty()
                        || !after.starts_with(|c: char| c.is_alphanumeric() || c == '_')
                    {
                        let col = line.len() - trimmed.len() + pat.prefix.len();
                        results.push(SymbolLocation {
                            path: file_path.to_path_buf(),
                            line: line_idx + 1,
                            column: col + 1,
                            kind: pat.kind,
                            context: trimmed.to_string(),
                        });
                    }
                }
            }
        }
    }
}

/// Find all references to a symbol (occurrences that are NOT definitions).
#[must_use]
pub fn find_references(symbol: &str, files: &[PathBuf]) -> Vec<SymbolLocation> {
    let mut results = Vec::new();
    let definitions = find_definitions(symbol, files);

    for file_path in files {
        let lang = detect_language(file_path);
        if lang == Language::Unknown {
            continue;
        }

        let Ok(content) = std::fs::read_to_string(file_path) else {
            continue;
        };

        find_references_in_content(
            symbol,
            &content,
            file_path,
            lang,
            &definitions,
            &mut results,
        );
    }

    results
}

/// Find references within already-loaded content (for testing).
pub fn find_references_in_content(
    symbol: &str,
    content: &str,
    file_path: &Path,
    _lang: Language,
    definitions: &[SymbolLocation],
    results: &mut Vec<SymbolLocation>,
) {
    for (line_idx, line) in content.lines().enumerate() {
        let line_num = line_idx + 1;

        // Skip comment-only lines
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('#') || trimmed.starts_with("/*") {
            continue;
        }

        // Find all occurrences of the symbol on this line
        let mut search_from = 0;
        while let Some(pos) = line[search_from..].find(symbol) {
            let abs_pos = search_from + pos;
            let col = abs_pos + 1;

            // Check word boundaries
            let before_ok = abs_pos == 0
                || !line.as_bytes()[abs_pos - 1].is_ascii_alphanumeric()
                    && line.as_bytes()[abs_pos - 1] != b'_';
            let after_pos = abs_pos + symbol.len();
            let after_ok = after_pos >= line.len()
                || !line.as_bytes()[after_pos].is_ascii_alphanumeric()
                    && line.as_bytes()[after_pos] != b'_';

            if before_ok && after_ok {
                // Check this isn't a definition location
                let is_def = definitions
                    .iter()
                    .any(|d| d.path == file_path && d.line == line_num);

                if !is_def {
                    results.push(SymbolLocation {
                        path: file_path.to_path_buf(),
                        line: line_num,
                        column: col,
                        kind: SymbolKind::Variable, // Generic for references
                        context: trimmed.to_string(),
                    });
                    // Only count one reference per line
                    break;
                }
            }

            search_from = abs_pos + symbol.len();
        }
    }
}

/// Find implementations (impl blocks in Rust, class methods that override, etc.).
#[must_use]
pub fn find_implementations(symbol: &str, files: &[PathBuf]) -> Vec<SymbolLocation> {
    let mut results = Vec::new();

    for file_path in files {
        let lang = detect_language(file_path);

        let Ok(content) = std::fs::read_to_string(file_path) else {
            continue;
        };

        find_implementations_in_content(symbol, &content, file_path, lang, &mut results);
    }

    results
}

/// Find implementations within already-loaded content (for testing).
pub fn find_implementations_in_content(
    symbol: &str,
    content: &str,
    file_path: &Path,
    lang: Language,
    results: &mut Vec<SymbolLocation>,
) {
    match lang {
        Language::Rust => {
            // Look for `impl Symbol`, `impl Trait for Symbol`, `impl<...> Symbol`
            for (line_idx, line) in content.lines().enumerate() {
                let trimmed = line.trim_start();
                if let Some(rest) = trimmed.strip_prefix("impl") {
                    // Skip if it's just "implement" or similar
                    if rest.starts_with(|c: char| c.is_alphanumeric()) {
                        continue;
                    }
                    // Check if symbol appears in the impl line
                    if rest.contains(symbol) {
                        let col = line.len() - trimmed.len() + 1;
                        results.push(SymbolLocation {
                            path: file_path.to_path_buf(),
                            line: line_idx + 1,
                            column: col,
                            kind: SymbolKind::Impl,
                            context: trimmed.to_string(),
                        });
                    }
                }
            }
        }
        Language::Python => {
            // Look for class inheritance: `class Foo(Symbol):`
            for (line_idx, line) in content.lines().enumerate() {
                let trimmed = line.trim_start();
                if trimmed.starts_with("class ") && trimmed.contains(&format!("({symbol})")) {
                    let col = line.len() - trimmed.len() + 1;
                    results.push(SymbolLocation {
                        path: file_path.to_path_buf(),
                        line: line_idx + 1,
                        column: col,
                        kind: SymbolKind::Class,
                        context: trimmed.to_string(),
                    });
                }
            }
        }
        Language::TypeScript | Language::JavaScript => {
            // Look for `extends Symbol` or `implements Symbol`
            for (line_idx, line) in content.lines().enumerate() {
                let trimmed = line.trim_start();
                if trimmed.contains(&format!("extends {symbol}"))
                    || trimmed.contains(&format!("implements {symbol}"))
                {
                    let col = line.len() - trimmed.len() + 1;
                    results.push(SymbolLocation {
                        path: file_path.to_path_buf(),
                        line: line_idx + 1,
                        column: col,
                        kind: SymbolKind::Class,
                        context: trimmed.to_string(),
                    });
                }
            }
        }
        _ => {}
    }
}

// ── Formatting ────────────────────────────────────────────────────────

/// Format navigation results for display.
#[must_use]
pub fn format_nav_results(results: &[SymbolLocation], label: &str) -> String {
    if results.is_empty() {
        return format!("No {label} found.");
    }

    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} ({} result{}):\n",
        label,
        results.len(),
        if results.len() == 1 { "" } else { "s" }
    );

    for loc in results {
        let _ = writeln!(
            out,
            "  {} [{}] {}:{}",
            loc.to_location_string(),
            loc.kind,
            loc.kind,
            loc.context,
        );
    }

    out
}

// ── High-level API ────────────────────────────────────────────────────

/// Code navigator wrapping a set of source files.
#[derive(Debug, Clone)]
pub struct CodeNavigator {
    files: Vec<PathBuf>,
}

impl CodeNavigator {
    /// Create a navigator for the given file list.
    #[must_use]
    pub fn new(files: Vec<PathBuf>) -> Self {
        Self { files }
    }

    /// Go-to-definition for a symbol.
    #[must_use]
    pub fn goto_definition(&self, symbol: &str) -> Vec<SymbolLocation> {
        find_definitions(symbol, &self.files)
    }

    /// Find all references to a symbol.
    #[must_use]
    pub fn find_references(&self, symbol: &str) -> Vec<SymbolLocation> {
        find_references(symbol, &self.files)
    }

    /// Find implementations of a trait/interface/class.
    #[must_use]
    pub fn find_implementations(&self, symbol: &str) -> Vec<SymbolLocation> {
        find_implementations(symbol, &self.files)
    }

    /// Number of files being navigated.
    #[must_use]
    pub fn file_count(&self) -> usize {
        self.files.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Language detection ──────────────────────────────────────────

    #[test]
    fn detect_rust() {
        assert_eq!(detect_language(Path::new("main.rs")), Language::Rust);
    }

    #[test]
    fn detect_javascript() {
        assert_eq!(detect_language(Path::new("app.js")), Language::JavaScript);
        assert_eq!(detect_language(Path::new("app.jsx")), Language::JavaScript);
    }

    #[test]
    fn detect_typescript() {
        assert_eq!(detect_language(Path::new("app.ts")), Language::TypeScript);
        assert_eq!(detect_language(Path::new("comp.tsx")), Language::TypeScript);
    }

    #[test]
    fn detect_python() {
        assert_eq!(detect_language(Path::new("main.py")), Language::Python);
    }

    #[test]
    fn detect_go() {
        assert_eq!(detect_language(Path::new("main.go")), Language::Go);
    }

    #[test]
    fn detect_unknown() {
        assert_eq!(detect_language(Path::new("data.csv")), Language::Unknown);
        assert_eq!(detect_language(Path::new("Makefile")), Language::Unknown);
    }

    // ── SymbolKind display ─────────────────────────────────────────

    #[test]
    fn symbol_kind_display() {
        assert_eq!(SymbolKind::Function.to_string(), "function");
        assert_eq!(SymbolKind::Struct.to_string(), "struct");
        assert_eq!(SymbolKind::Trait.to_string(), "trait");
        assert_eq!(SymbolKind::Class.to_string(), "class");
        assert_eq!(SymbolKind::Interface.to_string(), "interface");
    }

    // ── SymbolLocation ─────────────────────────────────────────────

    #[test]
    fn location_string() {
        let loc = SymbolLocation {
            path: PathBuf::from("src/main.rs"),
            line: 10,
            column: 5,
            kind: SymbolKind::Function,
            context: "fn main()".into(),
        };
        assert_eq!(loc.to_location_string(), "src/main.rs:10:5");
    }

    // ── find_definitions_in_content ────────────────────────────────

    #[test]
    fn find_rust_fn_definition() {
        let content = "pub fn query_loop(config: Config) -> Result<()> {\n    // body\n}";
        let mut results = Vec::new();
        find_definitions_in_content(
            "query_loop",
            content,
            Path::new("src/lib.rs"),
            Language::Rust,
            &mut results,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, SymbolKind::Function);
        assert_eq!(results[0].line, 1);
    }

    #[test]
    fn find_rust_struct_definition() {
        let content = "pub struct AgentSession {\n    config: Config,\n}";
        let mut results = Vec::new();
        find_definitions_in_content(
            "AgentSession",
            content,
            Path::new("src/lib.rs"),
            Language::Rust,
            &mut results,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, SymbolKind::Struct);
    }

    #[test]
    fn find_rust_enum_definition() {
        let content = "pub enum TaskStatus {\n    Pending,\n    Done,\n}";
        let mut results = Vec::new();
        find_definitions_in_content(
            "TaskStatus",
            content,
            Path::new("src/task.rs"),
            Language::Rust,
            &mut results,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, SymbolKind::Enum);
    }

    #[test]
    fn find_rust_trait_definition() {
        let content = "pub trait Tool {\n    fn name(&self) -> &str;\n}";
        let mut results = Vec::new();
        find_definitions_in_content(
            "Tool",
            content,
            Path::new("src/tool.rs"),
            Language::Rust,
            &mut results,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, SymbolKind::Trait);
    }

    #[test]
    fn find_rust_module_definition() {
        let content = "pub mod tools;\nmod internal;";
        let mut results = Vec::new();
        find_definitions_in_content(
            "tools",
            content,
            Path::new("src/lib.rs"),
            Language::Rust,
            &mut results,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, SymbolKind::Module);
    }

    #[test]
    fn find_rust_const_definition() {
        let content = "pub const MAX_RETRIES: u32 = 3;";
        let mut results = Vec::new();
        find_definitions_in_content(
            "MAX_RETRIES",
            content,
            Path::new("src/config.rs"),
            Language::Rust,
            &mut results,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, SymbolKind::Const);
    }

    #[test]
    fn find_rust_async_fn() {
        let content = "pub async fn process(input: &str) -> Result<()> {}";
        let mut results = Vec::new();
        find_definitions_in_content(
            "process",
            content,
            Path::new("src/lib.rs"),
            Language::Rust,
            &mut results,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, SymbolKind::Function);
    }

    #[test]
    fn no_partial_match() {
        let content = "pub fn query_loop_extended() {}";
        let mut results = Vec::new();
        find_definitions_in_content(
            "query_loop",
            content,
            Path::new("src/lib.rs"),
            Language::Rust,
            &mut results,
        );
        assert!(results.is_empty(), "Should not match partial name");
    }

    #[test]
    fn find_python_def() {
        let content = "def process_data(input):\n    pass";
        let mut results = Vec::new();
        find_definitions_in_content(
            "process_data",
            content,
            Path::new("main.py"),
            Language::Python,
            &mut results,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, SymbolKind::Function);
    }

    #[test]
    fn find_python_class() {
        let content = "class MyHandler:\n    pass";
        let mut results = Vec::new();
        find_definitions_in_content(
            "MyHandler",
            content,
            Path::new("handler.py"),
            Language::Python,
            &mut results,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, SymbolKind::Class);
    }

    #[test]
    fn find_python_async_def() {
        let content = "async def fetch_data():\n    await something()";
        let mut results = Vec::new();
        find_definitions_in_content(
            "fetch_data",
            content,
            Path::new("main.py"),
            Language::Python,
            &mut results,
        );
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn find_js_function() {
        let content = "export function handleRequest(req, res) {}";
        let mut results = Vec::new();
        find_definitions_in_content(
            "handleRequest",
            content,
            Path::new("app.js"),
            Language::JavaScript,
            &mut results,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, SymbolKind::Function);
    }

    #[test]
    fn find_js_class() {
        let content = "export class Router {\n  constructor() {}\n}";
        let mut results = Vec::new();
        find_definitions_in_content(
            "Router",
            content,
            Path::new("app.js"),
            Language::JavaScript,
            &mut results,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, SymbolKind::Class);
    }

    #[test]
    fn find_ts_interface() {
        let content = "export interface Config {\n  port: number;\n}";
        let mut results = Vec::new();
        find_definitions_in_content(
            "Config",
            content,
            Path::new("types.ts"),
            Language::TypeScript,
            &mut results,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, SymbolKind::Interface);
    }

    #[test]
    fn find_go_func() {
        let content = "func HandleRequest(w http.ResponseWriter, r *http.Request) {}";
        let mut results = Vec::new();
        find_definitions_in_content(
            "HandleRequest",
            content,
            Path::new("main.go"),
            Language::Go,
            &mut results,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, SymbolKind::Function);
    }

    #[test]
    fn find_go_type() {
        let content = "type Config struct {\n\tPort int\n}";
        let mut results = Vec::new();
        find_definitions_in_content(
            "Config",
            content,
            Path::new("config.go"),
            Language::Go,
            &mut results,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, SymbolKind::Type);
    }

    // ── find_references_in_content ─────────────────────────────────

    #[test]
    fn find_references_excludes_definition() {
        let content = "pub fn process() {}\nlet x = process();\nprocess();";
        let defs = vec![SymbolLocation {
            path: PathBuf::from("lib.rs"),
            line: 1,
            column: 8,
            kind: SymbolKind::Function,
            context: "pub fn process() {}".into(),
        }];
        let mut results = Vec::new();
        find_references_in_content(
            "process",
            content,
            Path::new("lib.rs"),
            Language::Rust,
            &defs,
            &mut results,
        );
        assert_eq!(results.len(), 2); // lines 2 and 3
    }

    #[test]
    fn find_references_skips_comments() {
        let content = "// process is important\nlet x = process();";
        let defs = vec![];
        let mut results = Vec::new();
        find_references_in_content(
            "process",
            content,
            Path::new("lib.rs"),
            Language::Rust,
            &defs,
            &mut results,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].line, 2);
    }

    #[test]
    fn find_references_word_boundary() {
        let content = "let processing = 1;\nlet x = process();";
        let defs = vec![];
        let mut results = Vec::new();
        find_references_in_content(
            "process",
            content,
            Path::new("lib.rs"),
            Language::Rust,
            &defs,
            &mut results,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].line, 2);
    }

    // ── find_implementations_in_content ────────────────────────────

    #[test]
    fn find_rust_impl() {
        let content = "impl Tool for ReadTool {\n    fn name(&self) -> &str { \"read\" }\n}";
        let mut results = Vec::new();
        find_implementations_in_content(
            "Tool",
            content,
            Path::new("read.rs"),
            Language::Rust,
            &mut results,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, SymbolKind::Impl);
    }

    #[test]
    fn find_rust_impl_direct() {
        let content = "impl MyStruct {\n    fn new() -> Self { Self {} }\n}";
        let mut results = Vec::new();
        find_implementations_in_content(
            "MyStruct",
            content,
            Path::new("lib.rs"),
            Language::Rust,
            &mut results,
        );
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn find_rust_impl_skips_implement_word() {
        let content = "// We need to implement this\nimpl Tool for X {}";
        let mut results = Vec::new();
        find_implementations_in_content(
            "Tool",
            content,
            Path::new("lib.rs"),
            Language::Rust,
            &mut results,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].line, 2);
    }

    #[test]
    fn find_python_impl() {
        let content = "class MyHandler(BaseHandler):\n    pass";
        let mut results = Vec::new();
        find_implementations_in_content(
            "BaseHandler",
            content,
            Path::new("handler.py"),
            Language::Python,
            &mut results,
        );
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn find_ts_extends() {
        let content = "export class Router extends BaseRouter {\n}";
        let mut results = Vec::new();
        find_implementations_in_content(
            "BaseRouter",
            content,
            Path::new("router.ts"),
            Language::TypeScript,
            &mut results,
        );
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn find_ts_implements() {
        let content = "class Service implements Runnable {\n}";
        let mut results = Vec::new();
        find_implementations_in_content(
            "Runnable",
            content,
            Path::new("service.ts"),
            Language::TypeScript,
            &mut results,
        );
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn find_no_impl_in_unknown_lang() {
        let content = "impl Something {}";
        let mut results = Vec::new();
        find_implementations_in_content(
            "Something",
            content,
            Path::new("data.csv"),
            Language::Unknown,
            &mut results,
        );
        assert!(results.is_empty());
    }

    // ── format_nav_results ─────────────────────────────────────────

    #[test]
    fn format_empty_results() {
        let text = format_nav_results(&[], "definitions");
        assert!(text.contains("No definitions found"));
    }

    #[test]
    fn format_single_result() {
        let results = vec![SymbolLocation {
            path: PathBuf::from("src/main.rs"),
            line: 42,
            column: 5,
            kind: SymbolKind::Function,
            context: "pub fn main() {}".into(),
        }];
        let text = format_nav_results(&results, "definitions");
        assert!(text.contains("1 result"));
        assert!(text.contains("src/main.rs:42:5"));
    }

    #[test]
    fn format_multiple_results_plural() {
        let results = vec![
            SymbolLocation {
                path: PathBuf::from("a.rs"),
                line: 1,
                column: 1,
                kind: SymbolKind::Function,
                context: "fn a()".into(),
            },
            SymbolLocation {
                path: PathBuf::from("b.rs"),
                line: 2,
                column: 1,
                kind: SymbolKind::Function,
                context: "fn b()".into(),
            },
        ];
        let text = format_nav_results(&results, "references");
        assert!(text.contains("2 results"));
    }

    // ── CodeNavigator ──────────────────────────────────────────────

    #[test]
    fn navigator_file_count() {
        let nav = CodeNavigator::new(vec![PathBuf::from("a.rs"), PathBuf::from("b.rs")]);
        assert_eq!(nav.file_count(), 2);
    }

    #[test]
    fn navigator_empty() {
        let nav = CodeNavigator::new(vec![]);
        assert_eq!(nav.file_count(), 0);
        assert!(nav.goto_definition("anything").is_empty());
        assert!(nav.find_references("anything").is_empty());
        assert!(nav.find_implementations("anything").is_empty());
    }

    // ── Integration: definitions across multiple patterns ──────────

    #[test]
    fn find_multiple_definitions_same_content() {
        // struct definition + type alias in same file
        let content = "pub struct Config {}\npub type Cfg = Config;";
        let mut results = Vec::new();
        find_definitions_in_content(
            "Config",
            content,
            Path::new("lib.rs"),
            Language::Rust,
            &mut results,
        );
        assert_eq!(results.len(), 1); // Only the struct definition
        assert_eq!(results[0].kind, SymbolKind::Struct);
    }

    #[test]
    fn find_indented_definition() {
        let content = "    pub fn helper() {}\n";
        let mut results = Vec::new();
        find_definitions_in_content(
            "helper",
            content,
            Path::new("lib.rs"),
            Language::Rust,
            &mut results,
        );
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn find_type_alias_definition() {
        let content = "pub type SharedStore = Arc<Mutex<Store>>;";
        let mut results = Vec::new();
        find_definitions_in_content(
            "SharedStore",
            content,
            Path::new("lib.rs"),
            Language::Rust,
            &mut results,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, SymbolKind::Type);
    }
}
