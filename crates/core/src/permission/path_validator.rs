//! File path permission engine.
//!
//! Maps to CCB `utils/permissions/filesystem.ts` (1778 LOC) + `pathValidation.ts` (486 LOC).
//!
//! Validates file paths against allowed/denied path rules. Detects directory
//! traversal attacks, symlink escapes, and ensures operations stay within
//! the designated working directory.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Errors related to path resolution or validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    /// Path contains directory traversal (`..`) that escapes allowed root.
    DirectoryTraversal(String),
    /// Symlink resolves to a location outside the allowed directories.
    SymlinkEscape {
        /// The original path that was a symlink.
        link: PathBuf,
        /// The resolved target outside allowed dirs.
        target: PathBuf,
    },
    /// The path is not valid UTF-8.
    InvalidUtf8(PathBuf),
    /// Generic I/O error during resolution.
    Io(String),
}

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DirectoryTraversal(path) => {
                write!(f, "directory traversal detected in path: {path}")
            }
            Self::SymlinkEscape { link, target } => {
                write!(
                    f,
                    "symlink '{}' resolves to '{}' which is outside allowed directories",
                    link.display(),
                    target.display()
                )
            }
            Self::InvalidUtf8(path) => {
                write!(f, "path is not valid UTF-8: {}", path.display())
            }
            Self::Io(msg) => write!(f, "I/O error during path resolution: {msg}"),
        }
    }
}

impl std::error::Error for PathError {}

// ---------------------------------------------------------------------------
// Permission result
// ---------------------------------------------------------------------------

/// Result of a file path permission check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PathPermission {
    /// The path is allowed for the requested operation.
    Allowed,
    /// The path is explicitly denied (includes reason).
    Denied(String),
    /// The path is outside the working directory tree.
    OutsideWorkDir,
}

// ---------------------------------------------------------------------------
// Shell expansion / UNC detection
// ---------------------------------------------------------------------------

/// Check whether a path string contains shell expansion syntax that could
/// cause TOCTOU vulnerabilities (validated literally, but expanded by shell).
fn contains_shell_expansion(path: &str) -> bool {
    // $VAR, ${VAR}, $(cmd)
    if path.contains('$') {
        return true;
    }
    // Windows %VAR%
    if path.contains('%') {
        let bytes = path.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%'
                && let Some(end) = path[i + 1..].find('%')
                && end > 0
            {
                return true;
            }
            i += 1;
        }
    }
    // Zsh equals expansion: =cmd
    if path.starts_with('=') && path.len() > 1 && path.as_bytes()[1].is_ascii_alphabetic() {
        return true;
    }
    false
}

/// Check whether a path looks like a UNC path (`\\server\share` or `//server/share`).
fn is_unc_path(path: &str) -> bool {
    path.starts_with("\\\\") || path.starts_with("//")
}

/// Check for suspicious Windows NTFS patterns that could bypass validation.
fn contains_suspicious_windows_pattern(path: &str) -> Option<&'static str> {
    // Alternate Data Streams (ADS): colon after position 2 (skip drive letter C:)
    if let Some(rest) = path.get(2..)
        && rest.contains(':')
    {
        return Some("NTFS alternate data stream detected");
    }
    // 8.3 short name: ~ followed by digit
    if path.contains('~') {
        for (i, c) in path.char_indices() {
            if c == '~' && path[i + 1..].starts_with(|c: char| c.is_ascii_digit()) {
                return Some("8.3 short name pattern detected");
            }
        }
    }
    // Long path prefixes
    if path.starts_with("\\\\?\\")
        || path.starts_with("\\\\.\\")
        || path.starts_with("//?/")
        || path.starts_with("//./")
    {
        return Some("long path prefix detected");
    }
    // Trailing dots or spaces (Windows strips these)
    if path.ends_with('.') || path.ends_with(' ') {
        return Some("trailing dot or space detected");
    }
    // Three+ consecutive dots
    if path.contains("...") {
        return Some("triple dot path segment detected");
    }
    None
}

// ---------------------------------------------------------------------------
// PathValidator
// ---------------------------------------------------------------------------

/// Validates whether a file path is allowed by permission rules.
///
/// Maintains a set of allowed directories and denied path patterns, and
/// checks candidate paths against them. Handles canonicalization, symlink
/// resolution, and directory traversal detection.
pub struct PathValidator {
    /// Directories that are allowed for file operations (canonicalized).
    allowed_dirs: Vec<PathBuf>,
    /// Denied path glob patterns (e.g. `"/etc/*"`, `"*.env"`).
    denied_patterns: Vec<String>,
}

impl PathValidator {
    /// Create a new path validator rooted at the given working directory.
    ///
    /// The working directory is always added as an allowed directory.
    pub fn new(working_dir: &Path) -> Self {
        let canonical = working_dir
            .canonicalize()
            .unwrap_or_else(|_| working_dir.to_path_buf());
        Self {
            allowed_dirs: vec![canonical],
            denied_patterns: Vec::new(),
        }
    }

    /// Add an additional allowed directory.
    pub fn add_allowed_dir(&mut self, dir: &Path) {
        let canonical = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        if !self.allowed_dirs.contains(&canonical) {
            self.allowed_dirs.push(canonical);
        }
    }

    /// Add a denied path pattern.
    pub fn add_denied_pattern(&mut self, pattern: &str) {
        self.denied_patterns.push(pattern.to_string());
    }

    /// Check whether a file path is allowed by the permission rules.
    ///
    /// This is the main entry point for path permission checks. It:
    /// 1. Rejects shell expansion syntax and UNC paths
    /// 2. Detects suspicious Windows patterns
    /// 3. Resolves the path (canonicalize, follow symlinks)
    /// 4. Checks against denied patterns
    /// 5. Verifies the path is within an allowed directory
    pub fn is_path_allowed(&self, path: &Path) -> PathPermission {
        let Some(path_str) = path.to_str() else {
            return PathPermission::Denied("path is not valid UTF-8".to_string());
        };

        // Gate 1: Reject shell expansion syntax (TOCTOU risk)
        if contains_shell_expansion(path_str) {
            return PathPermission::Denied(
                "path contains shell expansion syntax ($, %, =) which could cause security issues"
                    .to_string(),
            );
        }

        // Gate 2: Reject UNC paths (network access / credential leakage)
        if is_unc_path(path_str) {
            return PathPermission::Denied(
                "UNC paths (\\\\server\\share) are not allowed".to_string(),
            );
        }

        // Gate 3: Detect suspicious Windows NTFS patterns
        if let Some(reason) = contains_suspicious_windows_pattern(path_str) {
            return PathPermission::Denied(format!("suspicious path pattern rejected: {reason}"));
        }

        // Gate 4: Resolve the path safely
        let resolved = match self.resolve_safely(path) {
            Ok(p) => p,
            Err(PathError::DirectoryTraversal(msg)) => {
                return PathPermission::Denied(format!("directory traversal: {msg}"));
            }
            Err(PathError::SymlinkEscape { link, target }) => {
                return PathPermission::Denied(format!(
                    "symlink '{}' resolves outside allowed directories to '{}'",
                    link.display(),
                    target.display()
                ));
            }
            Err(PathError::InvalidUtf8(_)) => {
                return PathPermission::Denied("resolved path is not valid UTF-8".to_string());
            }
            Err(PathError::Io(_)) => {
                // For non-existent paths, try normalization instead of canonicalization
                match normalize_path(path) {
                    Some(normalized) => normalized,
                    None => return PathPermission::Denied("cannot resolve path".to_string()),
                }
            }
        };

        // Gate 5: Check against denied patterns
        if let Some(pattern) = self.matches_denied_pattern(&resolved) {
            return PathPermission::Denied(format!("path matches denied pattern '{pattern}'"));
        }

        // Gate 6: Verify within allowed directories
        if self.is_within_allowed_dirs(&resolved) {
            PathPermission::Allowed
        } else {
            PathPermission::OutsideWorkDir
        }
    }

    /// Expand a permission path that may contain `~` or `$HOME` references,
    /// resolving relative to the given settings directory.
    pub fn expand_permission_path(raw: &str, settings_dir: &Path) -> PathBuf {
        let raw = raw.trim();

        // Strip surrounding quotes
        let raw = raw
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .or_else(|| raw.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
            .unwrap_or(raw);

        // Tilde expansion: only expand bare `~` or `~/...`, NOT `~user`
        if raw == "~"
            && let Some(home) = home_dir()
        {
            return home;
        }
        if let Some(rest) = raw.strip_prefix("~/")
            && let Some(home) = home_dir()
        {
            return home.join(rest);
        }
        #[cfg(windows)]
        if let Some(rest) = raw.strip_prefix("~\\")
            && let Some(home) = home_dir()
        {
            return home.join(rest);
        }

        let path = Path::new(raw);

        // Relative paths resolved against settings directory
        if path.is_relative() {
            return settings_dir.join(raw);
        }

        path.to_path_buf()
    }

    /// Resolve a path safely, detecting directory traversal and symlink escapes.
    ///
    /// Returns the canonicalized path if it is within allowed directories,
    /// or a [`PathError`] if it escapes.
    fn resolve_safely(&self, path: &Path) -> Result<PathBuf, PathError> {
        // Try to canonicalize (follows all symlinks)
        let canonical = path
            .canonicalize()
            .map_err(|e| PathError::Io(e.to_string()))?;

        // Check UTF-8
        if canonical.to_str().is_none() {
            return Err(PathError::InvalidUtf8(canonical));
        }

        // If the input path differs from canonical AND canonical is outside allowed dirs,
        // this could be a symlink escape
        if path != canonical
            && !self.is_within_allowed_dirs(&canonical)
            && path.symlink_metadata().is_ok_and(|m| m.is_symlink())
        {
            return Err(PathError::SymlinkEscape {
                link: path.to_path_buf(),
                target: canonical,
            });
        }

        Ok(canonical)
    }

    /// Check whether a resolved path matches any denied pattern.
    fn matches_denied_pattern(&self, path: &Path) -> Option<String> {
        let path_str = path.to_str()?;

        for pattern in &self.denied_patterns {
            // Support both glob matching and simple suffix matching
            if super::filter::glob_match(pattern, path_str) {
                return Some(pattern.clone());
            }

            // Also check just the filename for basename-only patterns (e.g., "*.env")
            if let Some(filename) = path.file_name().and_then(|f| f.to_str())
                && super::filter::glob_match(pattern, filename)
            {
                return Some(pattern.clone());
            }
        }

        None
    }

    /// Check whether a resolved path is inside any allowed directory.
    fn is_within_allowed_dirs(&self, path: &Path) -> bool {
        self.allowed_dirs.iter().any(|dir| path_is_under(path, dir))
    }
}

/// Check whether `path` is under `dir` (inclusive of dir itself).
///
/// Uses case-insensitive comparison on Windows/macOS.
fn path_is_under(path: &Path, dir: &Path) -> bool {
    // Normalize both to strings for comparison
    let path_str = path.to_str().unwrap_or_default();
    let dir_str = dir.to_str().unwrap_or_default();

    if cfg!(any(target_os = "windows", target_os = "macos")) {
        let path_lower = path_str.to_lowercase();
        let dir_lower = dir_str.to_lowercase();
        path_lower == dir_lower || path_lower.starts_with(&format!("{dir_lower}{}", std::path::MAIN_SEPARATOR))
            // Also handle forward slashes on Windows
            || (cfg!(windows) && path_lower.starts_with(&format!("{dir_lower}/")))
    } else {
        path_str == dir_str || path_str.starts_with(&format!("{dir_str}/"))
    }
}

/// Normalize a path without requiring it to exist (for non-existent paths).
///
/// Resolves `.` and `..` components logically.
fn normalize_path(path: &Path) -> Option<PathBuf> {
    use std::path::Component;

    let mut normalized = if path.is_absolute() {
        PathBuf::new()
    } else {
        std::env::current_dir().ok()?
    };

    for component in path.components() {
        match component {
            Component::RootDir => {
                normalized.push(component.as_os_str());
            }
            Component::Prefix(prefix) => {
                normalized.push(prefix.as_os_str());
            }
            Component::CurDir => {} // skip `.`
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(segment) => {
                normalized.push(segment);
            }
        }
    }

    Some(normalized)
}

/// Get the user's home directory.
fn home_dir() -> Option<PathBuf> {
    // Try HOME env var first (works on all platforms)
    if let Ok(home) = std::env::var("HOME") {
        return Some(PathBuf::from(home));
    }
    // Windows: USERPROFILE
    #[cfg(windows)]
    if let Ok(profile) = std::env::var("USERPROFILE") {
        return Some(PathBuf::from(profile));
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn new_validator_allows_working_dir() {
        let dir = std::env::current_dir().unwrap();
        let validator = PathValidator::new(&dir);
        assert_eq!(validator.allowed_dirs.len(), 1);
    }

    #[test]
    fn path_within_working_dir_is_allowed() {
        let dir = std::env::current_dir().unwrap();
        let validator = PathValidator::new(&dir);
        // Use a path that actually exists (Cargo.toml is always present)
        let child = dir.join("Cargo.toml");
        let result = validator.is_path_allowed(&child);
        assert!(
            matches!(result, PathPermission::Allowed),
            "expected Allowed, got {result:?}"
        );
    }

    #[test]
    fn path_outside_working_dir_is_outside() {
        let temp = std::env::temp_dir().join("crab_test_workdir");
        let _ = fs::create_dir_all(&temp);
        let validator = PathValidator::new(&temp);

        // A path in a completely different tree
        let other = if cfg!(windows) {
            PathBuf::from("C:\\Windows\\System32\\cmd.exe")
        } else {
            PathBuf::from("/usr/bin/ls")
        };

        let result = validator.is_path_allowed(&other);
        assert!(
            matches!(
                result,
                PathPermission::OutsideWorkDir | PathPermission::Denied(_)
            ),
            "expected OutsideWorkDir or Denied, got {result:?}"
        );

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn denied_pattern_blocks_path() {
        let dir = std::env::current_dir().unwrap();
        let mut validator = PathValidator::new(&dir);
        validator.add_denied_pattern("*.env");

        let env_file = dir.join(".env");
        let result = validator.is_path_allowed(&env_file);
        assert!(matches!(result, PathPermission::Denied(_)));
    }

    #[test]
    fn multiple_allowed_dirs() {
        let dir1 = std::env::current_dir().unwrap();
        let dir2 = std::env::temp_dir();
        let mut validator = PathValidator::new(&dir1);
        validator.add_allowed_dir(&dir2);
        assert_eq!(validator.allowed_dirs.len(), 2);
    }

    #[test]
    fn add_allowed_dir_deduplicates() {
        let dir = std::env::current_dir().unwrap();
        let mut validator = PathValidator::new(&dir);
        validator.add_allowed_dir(&dir);
        assert_eq!(validator.allowed_dirs.len(), 1);
    }

    #[test]
    fn shell_expansion_rejected() {
        let dir = std::env::current_dir().unwrap();
        let validator = PathValidator::new(&dir);

        let paths = ["$HOME/secrets", "${HOME}/secrets", "$(whoami)/data"];
        for path_str in paths {
            let result = validator.is_path_allowed(Path::new(path_str));
            assert!(
                matches!(result, PathPermission::Denied(_)),
                "expected Denied for '{path_str}', got {result:?}"
            );
        }
    }

    #[test]
    fn unc_path_rejected() {
        let dir = std::env::current_dir().unwrap();
        let validator = PathValidator::new(&dir);

        let result = validator.is_path_allowed(Path::new("//server/share/file"));
        assert!(matches!(result, PathPermission::Denied(_)));
    }

    #[test]
    fn suspicious_windows_patterns_rejected() {
        let dir = std::env::current_dir().unwrap();
        let validator = PathValidator::new(&dir);

        // Alternate Data Streams
        let result = validator.is_path_allowed(Path::new("C:\\file.txt::$DATA"));
        assert!(matches!(result, PathPermission::Denied(_)));

        // Triple dots
        let result = validator.is_path_allowed(Path::new("/path/.../escape"));
        assert!(matches!(result, PathPermission::Denied(_)));
    }

    #[test]
    fn expand_permission_path_tilde() {
        let settings = PathBuf::from("/fake/settings");
        if let Some(home) = home_dir() {
            let expanded = PathValidator::expand_permission_path("~/projects", &settings);
            assert_eq!(expanded, home.join("projects"));
        }
    }

    #[test]
    fn expand_permission_path_bare_tilde() {
        let settings = PathBuf::from("/fake/settings");
        if let Some(home) = home_dir() {
            let expanded = PathValidator::expand_permission_path("~", &settings);
            assert_eq!(expanded, home);
        }
    }

    #[test]
    fn expand_permission_path_relative() {
        let settings = PathBuf::from("/fake/settings");
        let expanded = PathValidator::expand_permission_path("relative/path", &settings);
        assert_eq!(expanded, settings.join("relative/path"));
    }

    #[test]
    fn expand_permission_path_absolute() {
        let settings = PathBuf::from("/fake/settings");
        let abs_path = if cfg!(windows) {
            "C:\\absolute\\path"
        } else {
            "/absolute/path"
        };
        let expanded = PathValidator::expand_permission_path(abs_path, &settings);
        assert_eq!(expanded, PathBuf::from(abs_path));
    }

    #[test]
    fn expand_permission_path_strips_quotes() {
        let settings = PathBuf::from("/fake/settings");
        let expanded = PathValidator::expand_permission_path("\"relative/path\"", &settings);
        assert_eq!(expanded, settings.join("relative/path"));
    }

    #[test]
    fn contains_shell_expansion_detects_dollar() {
        assert!(contains_shell_expansion("$HOME/foo"));
        assert!(contains_shell_expansion("${VAR}"));
        assert!(contains_shell_expansion("$(whoami)"));
    }

    #[test]
    fn contains_shell_expansion_allows_normal_paths() {
        assert!(!contains_shell_expansion("/usr/local/bin"));
        assert!(!contains_shell_expansion("C:\\Users\\test"));
        assert!(!contains_shell_expansion("./relative/path"));
    }

    #[test]
    fn is_unc_path_detection() {
        assert!(is_unc_path("\\\\server\\share"));
        assert!(is_unc_path("//server/share"));
        assert!(!is_unc_path("/normal/path"));
        assert!(!is_unc_path("C:\\normal\\path"));
    }

    #[test]
    fn suspicious_patterns_trailing_dot() {
        assert!(contains_suspicious_windows_pattern("file.txt.").is_some());
    }

    #[test]
    fn suspicious_patterns_trailing_space() {
        assert!(contains_suspicious_windows_pattern("file.txt ").is_some());
    }

    #[test]
    fn suspicious_patterns_normal_path_ok() {
        assert!(contains_suspicious_windows_pattern("/normal/path/file.txt").is_none());
    }

    #[test]
    fn path_is_under_same_dir() {
        let dir = PathBuf::from("/home/user/project");
        assert!(path_is_under(&dir, &dir));
    }

    #[test]
    fn path_is_under_child() {
        let dir = PathBuf::from("/home/user/project");
        let child = PathBuf::from("/home/user/project/src/main.rs");
        assert!(path_is_under(&child, &dir));
    }

    #[test]
    fn path_is_under_sibling_not_under() {
        let dir = PathBuf::from("/home/user/project");
        let sibling = PathBuf::from("/home/user/other");
        assert!(!path_is_under(&sibling, &dir));
    }

    #[test]
    fn path_is_under_prefix_attack() {
        // "/home/user/project-evil" should NOT be under "/home/user/project"
        let dir = PathBuf::from("/home/user/project");
        let evil = PathBuf::from("/home/user/project-evil/src");
        assert!(!path_is_under(&evil, &dir));
    }

    #[test]
    fn normalize_path_resolves_parent() {
        // On Windows, absolute paths starting with / get a drive prefix
        let input = if cfg!(windows) {
            Path::new("C:\\a\\b\\..\\c")
        } else {
            Path::new("/a/b/../c")
        };
        let expected = if cfg!(windows) {
            PathBuf::from("C:\\a\\c")
        } else {
            PathBuf::from("/a/c")
        };
        let normalized = normalize_path(input).unwrap();
        assert_eq!(normalized, expected);
    }

    #[test]
    fn normalize_path_resolves_current_dir() {
        let input = if cfg!(windows) {
            Path::new("C:\\a\\.\\b\\.\\c")
        } else {
            Path::new("/a/./b/./c")
        };
        let expected = if cfg!(windows) {
            PathBuf::from("C:\\a\\b\\c")
        } else {
            PathBuf::from("/a/b/c")
        };
        let normalized = normalize_path(input).unwrap();
        assert_eq!(normalized, expected);
    }

    #[test]
    fn denied_pattern_glob_star() {
        let dir = std::env::current_dir().unwrap();
        let mut validator = PathValidator::new(&dir);
        validator.add_denied_pattern("*.key");

        let key_file = dir.join("secret.key");
        let result = validator.is_path_allowed(&key_file);
        assert!(matches!(result, PathPermission::Denied(_)));
    }

    #[test]
    fn zsh_equals_expansion_rejected() {
        assert!(contains_shell_expansion("=date"));
        assert!(!contains_shell_expansion("normal=value")); // not at start
    }
}
