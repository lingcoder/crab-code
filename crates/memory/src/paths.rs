//! Memory directory path resolution.
//!
//! Maps logical memory scopes (auto/global/team) to filesystem locations
//! under `~/.crab/`.

use std::path::{Path, PathBuf};

use crab_utils::path::home_dir;

/// Auto memory directory for a specific project.
///
/// `~/.crab/projects/<sanitized-git-root>/memory/`. Falls back to
/// [`global_memory_dir`] if `git_root` is `None`.
#[must_use]
pub fn auto_memory_dir(git_root: Option<&Path>) -> PathBuf {
    match git_root {
        Some(root) => {
            let sanitized = sanitize_path_component(root);
            home_dir()
                .join(".crab")
                .join("projects")
                .join(sanitized)
                .join("memory")
        }
        None => global_memory_dir(),
    }
}

/// Global memory directory: `~/.crab/memory/`.
#[must_use]
pub fn global_memory_dir() -> PathBuf {
    home_dir().join(".crab").join("memory")
}

/// Team memory directory: `~/.crab/teams/<name>/memory/`.
#[must_use]
pub fn team_memory_dir(team_name: &str) -> PathBuf {
    home_dir()
        .join(".crab")
        .join("teams")
        .join(team_name)
        .join("memory")
}

/// Check whether a path appears to be inside any memory directory.
///
/// Looks for both `.crab` and `memory` components in the path string.
#[must_use]
pub fn is_memory_path(path: &Path) -> bool {
    let s = path.to_string_lossy();
    s.contains(".crab") && s.contains("memory")
}

/// Sanitize a filesystem path into a safe directory-name component.
///
/// - Path separators (`/`, `\`), drive colons (`:`), and spaces collapse to `-`.
/// - Alphanumerics plus `-`, `_`, `.` are kept as-is.
/// - Anything else becomes `-`.
/// - Leading/trailing `-` are trimmed.
#[must_use]
pub fn sanitize_path_component(path: &Path) -> String {
    let s = path.to_string_lossy();
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        let mapped = match ch {
            '/' | '\\' | ':' | ' ' => '-',
            c if c.is_ascii_alphanumeric() => c,
            '-' | '_' | '.' => ch,
            _ => '-',
        };
        // Collapse consecutive dashes.
        if mapped == '-' && out.ends_with('-') {
            continue;
        }
        out.push(mapped);
    }
    out.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn global_memory_dir_is_under_crab() {
        let dir = global_memory_dir();
        let s = dir.to_string_lossy();
        assert!(s.contains(".crab"), "expected '.crab' in {s}");
        assert!(
            dir.ends_with("memory"),
            "expected dir to end with 'memory': {s}"
        );
    }

    #[test]
    fn auto_memory_dir_with_git_root() {
        let root = PathBuf::from("/home/user/my-project");
        let dir = auto_memory_dir(Some(&root));
        let s = dir.to_string_lossy();
        assert!(s.contains(".crab"));
        assert!(s.contains("projects"));
        assert!(s.contains("memory"));
        // Must not leak raw path separators from the original path.
        assert!(!s.contains("/home/user/my-project"));
        assert!(!s.contains("\\home\\user\\my-project"));
    }

    #[test]
    fn auto_memory_dir_none_falls_back_to_global() {
        assert_eq!(auto_memory_dir(None), global_memory_dir());
    }

    #[test]
    fn team_memory_dir_structure() {
        let dir = team_memory_dir("alpha");
        let s = dir.to_string_lossy();
        assert!(s.contains("teams"));
        assert!(s.contains("alpha"));
        assert!(s.contains("memory"));
    }

    #[test]
    fn sanitize_path_component_replaces_separators() {
        let got = sanitize_path_component(Path::new("/home/user/my-project"));
        assert_eq!(got, "home-user-my-project");
    }

    #[test]
    fn sanitize_windows_drive_path() {
        let got = sanitize_path_component(Path::new(r"C:\Users\ling\project"));
        assert_eq!(got, "C-Users-ling-project");
    }

    #[test]
    fn sanitize_trims_leading_and_trailing_dashes() {
        let got = sanitize_path_component(Path::new("//foo//"));
        assert_eq!(got, "foo");
    }

    #[test]
    fn sanitize_keeps_underscore_dot_dash() {
        let got = sanitize_path_component(Path::new("a_b.c-d"));
        assert_eq!(got, "a_b.c-d");
    }

    #[test]
    fn is_memory_path_positive() {
        let p = global_memory_dir().join("user_role.md");
        assert!(is_memory_path(&p));
    }

    #[test]
    fn is_memory_path_negative() {
        assert!(!is_memory_path(Path::new("/tmp/random.md")));
    }
}
