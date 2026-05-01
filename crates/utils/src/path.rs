use std::path::{Path, PathBuf};

/// Normalizes a path by canonicalizing it (resolving symlinks, `.`, `..`)
/// and stripping the `\\?\` prefix on Windows (via dunce).
/// Falls back to the original path if canonicalization fails.
#[must_use]
pub fn normalize(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Returns the user's home directory.
///
/// # Panics
///
/// Panics if the home directory cannot be determined.
#[must_use]
pub fn home_dir() -> PathBuf {
    directories::BaseDirs::new()
        .expect("failed to resolve home directory")
        .home_dir()
        .to_path_buf()
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
    fn home_dir_is_absolute() {
        let home = home_dir();
        assert!(home.is_absolute());
    }

    #[test]
    fn home_dir_exists() {
        let home = home_dir();
        assert!(home.exists());
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
