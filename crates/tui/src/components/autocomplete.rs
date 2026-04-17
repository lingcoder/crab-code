//! Tab-completion for file paths and `/command` names.
//!
//! Provides an `AutoComplete` engine that generates completion candidates
//! based on the current input context (slash command or file path).

use std::path::{Path, PathBuf};

/// A single completion candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    /// The text to insert (replaces the current token).
    pub text: String,
    /// Short description shown alongside the completion.
    pub description: String,
    /// Whether this completion represents a directory (for path completions).
    pub is_directory: bool,
}

/// Context type for autocompletion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionContext {
    /// Completing a `/command` name.
    SlashCommand,
    /// Completing a file path.
    FilePath,
    /// No completion context detected.
    None,
}

/// Tab-completion engine.
pub struct AutoComplete {
    /// Available slash commands for completion.
    commands: Vec<CommandInfo>,
    /// Current list of completion candidates.
    candidates: Vec<Completion>,
    /// Index of the currently selected candidate (None = no selection).
    selected: Option<usize>,
    /// The token being completed.
    completing_token: String,
    /// Working directory for file path completion.
    cwd: PathBuf,
}

/// Metadata about a slash command.
#[derive(Debug, Clone)]
pub struct CommandInfo {
    /// Command name without the leading `/`.
    pub name: String,
    /// Short description.
    pub description: String,
}

impl AutoComplete {
    /// Create a new autocomplete engine with the given working directory.
    #[must_use]
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            commands: Vec::new(),
            candidates: Vec::new(),
            selected: None,
            completing_token: String::new(),
            cwd: cwd.into(),
        }
    }

    /// Register available slash commands.
    pub fn set_commands(&mut self, commands: Vec<CommandInfo>) {
        self.commands = commands;
    }

    /// Update the working directory.
    pub fn set_cwd(&mut self, cwd: impl Into<PathBuf>) {
        self.cwd = cwd.into();
    }

    /// Determine the completion context from the current input text
    /// and cursor position.
    #[must_use]
    pub fn detect_context(input: &str, cursor_col: usize) -> CompletionContext {
        let before_cursor = &input[..cursor_col.min(input.len())];

        // Find the start of the current token (last whitespace boundary)
        let token_start = before_cursor
            .rfind(char::is_whitespace)
            .map_or(0, |i| i + 1);
        let token = &before_cursor[token_start..];

        // Slash command: starts with `/` and the rest is purely alphabetic
        // (no further `/` or `\` which would indicate a file path).
        if token.starts_with('/')
            && !token[1..].contains('/')
            && !token[1..].contains('\\')
            && token[1..]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return CompletionContext::SlashCommand;
        }

        // Check if the token looks like a file path
        if token.contains(std::path::MAIN_SEPARATOR)
            || token.contains('/')
            || token.starts_with('.')
            || token.starts_with('~')
        {
            return CompletionContext::FilePath;
        }

        CompletionContext::None
    }

    /// Generate completions for the given input and cursor position.
    /// Returns the number of candidates found.
    pub fn complete(&mut self, input: &str, cursor_col: usize) -> usize {
        self.candidates.clear();
        self.selected = None;

        let before_cursor = &input[..cursor_col.min(input.len())];
        let token_start = before_cursor
            .rfind(char::is_whitespace)
            .map_or(0, |i| i + 1);
        let token = &before_cursor[token_start..];
        self.completing_token = token.to_string();

        let context = Self::detect_context(input, cursor_col);
        match context {
            CompletionContext::SlashCommand => {
                self.complete_commands(token);
            }
            CompletionContext::FilePath => {
                self.complete_file_paths(token);
            }
            CompletionContext::None => {}
        }

        if !self.candidates.is_empty() {
            self.selected = Some(0);
        }

        self.candidates.len()
    }

    /// Get the current list of candidates.
    #[must_use]
    pub fn candidates(&self) -> &[Completion] {
        &self.candidates
    }

    /// Get the currently selected candidate.
    #[must_use]
    pub fn selected(&self) -> Option<&Completion> {
        self.selected.and_then(|i| self.candidates.get(i))
    }

    /// Get the selected index.
    #[must_use]
    pub fn selected_index(&self) -> Option<usize> {
        self.selected
    }

    /// Whether there are active candidates.
    #[must_use]
    pub fn is_active(&self) -> bool {
        !self.candidates.is_empty()
    }

    /// Cycle to the next candidate. Wraps around.
    pub fn next(&mut self) {
        if self.candidates.is_empty() {
            return;
        }
        self.selected = Some(match self.selected {
            Some(i) => (i + 1) % self.candidates.len(),
            None => 0,
        });
    }

    /// Cycle to the previous candidate. Wraps around.
    pub fn prev(&mut self) {
        if self.candidates.is_empty() {
            return;
        }
        self.selected = Some(match self.selected {
            Some(0) | None => self.candidates.len() - 1,
            Some(i) => i - 1,
        });
    }

    /// Accept the currently selected completion.
    /// Returns `(token_to_replace, replacement_text)` or `None`.
    #[must_use]
    pub fn accept(&mut self) -> Option<(String, String)> {
        let completion = self.selected()?.clone();
        let token = self.completing_token.clone();
        self.dismiss();

        let mut text = completion.text;
        if completion.is_directory && !text.ends_with('/') && !text.ends_with('\\') {
            text.push(std::path::MAIN_SEPARATOR);
        }

        Some((token, text))
    }

    /// Dismiss the completion popup without accepting.
    pub fn dismiss(&mut self) {
        self.candidates.clear();
        self.selected = None;
        self.completing_token.clear();
    }

    // ── Internal ──

    fn complete_commands(&mut self, token: &str) {
        let prefix = token.strip_prefix('/').unwrap_or(token).to_lowercase();
        for cmd in &self.commands {
            if cmd.name.to_lowercase().starts_with(&prefix) {
                self.candidates.push(Completion {
                    text: format!("/{}", cmd.name),
                    description: cmd.description.clone(),
                    is_directory: false,
                });
            }
        }
        self.candidates.sort_by_key(|a| a.text.len());
    }

    fn complete_file_paths(&mut self, token: &str) {
        let path = if let Some(stripped) = token.strip_prefix('~') {
            // Expand tilde
            if let Some(home) = home_dir() {
                home.join(stripped.trim_start_matches(['/', '\\']))
            } else {
                return;
            }
        } else if Path::new(token).is_absolute() {
            PathBuf::from(token)
        } else {
            self.cwd.join(token)
        };

        // Determine the directory to list and the prefix to filter
        let (dir, prefix) = if path.is_dir() && (token.ends_with('/') || token.ends_with('\\')) {
            (path, String::new())
        } else {
            let dir = path.parent().unwrap_or(&self.cwd).to_path_buf();
            let prefix = path
                .file_name()
                .map_or(String::new(), |n| n.to_string_lossy().to_string());
            (dir, prefix)
        };

        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };

        let prefix_lower = prefix.to_lowercase();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.to_lowercase().starts_with(&prefix_lower) {
                continue;
            }
            // Skip hidden files unless the user typed a dot
            if name.starts_with('.') && !prefix.starts_with('.') {
                continue;
            }

            let is_dir = entry.file_type().is_ok_and(|ft| ft.is_dir());
            let display_path = if token.ends_with('/') || token.ends_with('\\') {
                format!("{token}{name}")
            } else if let Some(last_sep) = token.rfind(['/', '\\']) {
                format!("{}{name}", &token[..=last_sep])
            } else {
                name.clone()
            };

            self.candidates.push(Completion {
                text: display_path,
                description: if is_dir {
                    "directory".into()
                } else {
                    "file".into()
                },
                is_directory: is_dir,
            });
        }

        self.candidates.sort_by(|a, b| {
            // Directories first, then alphabetical
            b.is_directory
                .cmp(&a.is_directory)
                .then_with(|| a.text.cmp(&b.text))
        });
    }
}

impl Default for AutoComplete {
    fn default() -> Self {
        Self::new(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }
}

/// Get the user's home directory.
fn home_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var("USERPROFILE").ok().map(PathBuf::from)
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::env::var("HOME").ok().map(PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_commands() -> Vec<CommandInfo> {
        vec![
            CommandInfo {
                name: "help".into(),
                description: "Show help".into(),
            },
            CommandInfo {
                name: "history".into(),
                description: "Show history".into(),
            },
            CommandInfo {
                name: "commit".into(),
                description: "Create a commit".into(),
            },
            CommandInfo {
                name: "clear".into(),
                description: "Clear screen".into(),
            },
        ]
    }

    #[test]
    fn detect_slash_command() {
        assert_eq!(
            AutoComplete::detect_context("/he", 3),
            CompletionContext::SlashCommand
        );
        assert_eq!(
            AutoComplete::detect_context("say /he", 7),
            CompletionContext::SlashCommand
        );
    }

    #[test]
    fn detect_file_path() {
        assert_eq!(
            AutoComplete::detect_context("./src", 5),
            CompletionContext::FilePath
        );
        assert_eq!(
            AutoComplete::detect_context("~/doc", 5),
            CompletionContext::FilePath
        );
    }

    #[test]
    fn detect_none() {
        assert_eq!(
            AutoComplete::detect_context("hello", 5),
            CompletionContext::None
        );
        assert_eq!(AutoComplete::detect_context("", 0), CompletionContext::None);
    }

    #[test]
    fn complete_commands_filters_by_prefix() {
        let mut ac = AutoComplete::new(".");
        ac.set_commands(test_commands());
        let count = ac.complete("/he", 3);
        assert_eq!(count, 1);
        assert_eq!(ac.candidates()[0].text, "/help");
    }

    #[test]
    fn complete_commands_multiple_matches() {
        let mut ac = AutoComplete::new(".");
        ac.set_commands(test_commands());
        let count = ac.complete("/c", 2);
        assert_eq!(count, 2);
        let texts: Vec<&str> = ac.candidates().iter().map(|c| c.text.as_str()).collect();
        assert!(texts.contains(&"/clear"));
        assert!(texts.contains(&"/commit"));
    }

    #[test]
    fn complete_commands_empty_prefix() {
        let mut ac = AutoComplete::new(".");
        ac.set_commands(test_commands());
        let count = ac.complete("/", 1);
        assert_eq!(count, 4);
    }

    #[test]
    fn complete_commands_no_match() {
        let mut ac = AutoComplete::new(".");
        ac.set_commands(test_commands());
        let count = ac.complete("/xyz", 4);
        assert_eq!(count, 0);
        assert!(!ac.is_active());
    }

    #[test]
    fn next_prev_cycle() {
        let mut ac = AutoComplete::new(".");
        ac.set_commands(test_commands());
        ac.complete("/", 1);
        assert_eq!(ac.selected_index(), Some(0));

        ac.next();
        assert_eq!(ac.selected_index(), Some(1));

        ac.next();
        ac.next();
        ac.next(); // wraps
        assert_eq!(ac.selected_index(), Some(0));

        ac.prev(); // wraps back
        assert_eq!(ac.selected_index(), Some(3));
    }

    #[test]
    fn accept_returns_replacement() {
        let mut ac = AutoComplete::new(".");
        ac.set_commands(test_commands());
        ac.complete("/he", 3);
        let result = ac.accept();
        assert!(result.is_some());
        let (token, text) = result.unwrap();
        assert_eq!(token, "/he");
        assert_eq!(text, "/help");
        assert!(!ac.is_active());
    }

    #[test]
    fn accept_no_candidates() {
        let mut ac = AutoComplete::new(".");
        let result = ac.accept();
        assert!(result.is_none());
    }

    #[test]
    fn dismiss_clears_state() {
        let mut ac = AutoComplete::new(".");
        ac.set_commands(test_commands());
        ac.complete("/", 1);
        assert!(ac.is_active());

        ac.dismiss();
        assert!(!ac.is_active());
        assert_eq!(ac.selected_index(), None);
    }

    #[test]
    fn file_path_completion_with_real_dir() {
        // Complete against the temp directory which always exists
        let temp = std::env::temp_dir();
        let mut ac = AutoComplete::new(&temp);

        // Create a test file
        let test_dir = temp.join("crab_ac_test");
        let _ = std::fs::create_dir_all(&test_dir);
        let _ = std::fs::write(test_dir.join("testfile.txt"), "test");

        let token = format!("crab_ac_test{}", std::path::MAIN_SEPARATOR);
        let count = ac.complete(&token, token.len());
        assert!(count > 0);

        let texts: Vec<&str> = ac.candidates().iter().map(|c| c.text.as_str()).collect();
        let expected = format!("crab_ac_test{}testfile.txt", std::path::MAIN_SEPARATOR);
        assert!(
            texts.contains(&expected.as_str()),
            "Expected {expected:?} in {texts:?}"
        );

        let _ = std::fs::remove_dir_all(&test_dir);
    }

    #[test]
    fn file_path_dirs_sorted_first() {
        let temp = std::env::temp_dir();
        let test_dir = temp.join("crab_ac_sort_test");
        // Clean up from any previous run
        let _ = std::fs::remove_dir_all(&test_dir);
        std::fs::create_dir_all(test_dir.join("adir")).unwrap();
        std::fs::write(test_dir.join("afile.txt"), "").unwrap();

        let mut ac = AutoComplete::new(&test_dir);
        let token = format!(".{}a", std::path::MAIN_SEPARATOR);
        let count = ac.complete(&token, token.len());
        assert!(
            count >= 2,
            "Expected >= 2 candidates, got {count}: {:?}",
            ac.candidates()
        );
        // First candidate should be the directory
        assert!(ac.candidates()[0].is_directory);

        let _ = std::fs::remove_dir_all(&test_dir);
    }

    #[test]
    fn set_cwd_changes_completion_root() {
        let mut ac = AutoComplete::new("/original");
        ac.set_cwd("/new/path");
        assert_eq!(ac.cwd, PathBuf::from("/new/path"));
    }

    #[test]
    fn completion_is_not_directory() {
        let c = Completion {
            text: "test.rs".into(),
            description: "file".into(),
            is_directory: false,
        };
        assert!(!c.is_directory);
    }

    #[test]
    fn accept_directory_appends_separator() {
        let mut ac = AutoComplete::new(".");
        ac.candidates.push(Completion {
            text: "src".into(),
            description: "directory".into(),
            is_directory: true,
        });
        ac.selected = Some(0);
        ac.completing_token = "sr".into();

        let result = ac.accept().unwrap();
        assert!(
            result.1.ends_with(std::path::MAIN_SEPARATOR),
            "Expected trailing separator in {:?}",
            result.1
        );
    }
}
