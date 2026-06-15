use std::path::{Path, PathBuf};

pub use directories::{BaseDirs, ProjectDirs, UserDirs};

/// Normalizes a path by canonicalizing it (resolving symlinks, `.`, `..`)
/// and stripping the `\\?\` prefix on Windows (via dunce).
/// Falls back to the original path if canonicalization fails.
#[must_use]
pub fn normalize(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Returns the user's home directory, if it can be determined.
#[must_use]
pub fn home_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf())
}

/// Returns the user's home directory, with a non-panicking fallback.
///
/// Falls back to the current working directory (or `.`) when no home can be
/// determined — e.g. stripped-down containers without `HOME` or a user
/// profile. Intended for call sites that build best-effort default paths
/// like `~/.crab/...` and must not panic.
#[must_use]
pub fn home_dir_or_cwd() -> PathBuf {
    home_dir().unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_existing_path() {
        // Current directory should always be normalizable
        let normalized = normalize(Path::new("."));
        assert!(normalized.is_absolute());
    }

    #[test]
    fn normalize_nonexistent_path_returns_original() {
        let fake = Path::new("/this/path/does/not/exist/at/all");
        let result = normalize(fake);
        assert_eq!(result, fake);
    }

    #[test]
    fn home_dir_is_absolute_and_exists() {
        let home = home_dir().expect("home dir resolvable in test env");
        assert!(home.is_absolute());
        assert!(home.exists());
    }

    #[test]
    fn home_dir_or_cwd_never_empty() {
        let home = home_dir_or_cwd();
        assert!(!home.as_os_str().is_empty());
    }

    #[test]
    fn normalize_resolves_dot_dot() {
        // temp_dir()/.. should resolve to parent of temp dir
        let mut p = std::env::temp_dir();
        p.push("..");
        let normalized = normalize(&p);
        assert!(normalized.is_absolute());
        // Should not contain ".." after normalization
        assert!(!normalized.to_string_lossy().contains(".."));
    }

    #[test]
    fn normalize_empty_path_fallback() {
        // Empty path can't be canonicalized, should return as-is
        let result = normalize(Path::new(""));
        assert_eq!(result, Path::new(""));
    }
}
