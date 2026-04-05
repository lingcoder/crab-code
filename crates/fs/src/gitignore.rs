//! `.gitignore` rule parsing and path filtering.
//!
//! Thin wrapper around [`ignore::gitignore::Gitignore`] that loads the full
//! chain of ignore files for a directory tree: local `.gitignore`, parent
//! `.gitignore` files, `.git/info/exclude`, and the global gitignore.

use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Compiled gitignore rules for a directory tree.
///
/// Created from a root directory; automatically discovers and merges all
/// relevant ignore files.
pub struct GitIgnoreFilter {
    /// Root directory the filter was built from.
    root: PathBuf,
    /// The compiled gitignore matcher.
    gitignore: ignore::gitignore::Gitignore,
}

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

impl GitIgnoreFilter {
    /// Load all gitignore rules for the directory tree rooted at `root`.
    ///
    /// Discovers:
    /// - `.gitignore` files from `root` upward through parent directories.
    /// - `.git/info/exclude` (if present).
    /// - The global gitignore (e.g. `~/.config/git/ignore`).
    ///
    /// # Errors
    ///
    /// Returns an error if `root` is inaccessible.
    pub fn new(root: &Path) -> crab_common::Result<Self> {
        Self::build(root, None)
    }

    /// Load gitignore rules *plus* an additional custom ignore file
    /// (e.g. `.crabignore`).
    ///
    /// # Errors
    ///
    /// Returns an error if `root` or `ignore_file` is inaccessible.
    pub fn with_custom_ignore(root: &Path, ignore_file: &Path) -> crab_common::Result<Self> {
        Self::build(root, Some(ignore_file))
    }

    fn build(root: &Path, custom_ignore: Option<&Path>) -> crab_common::Result<Self> {
        let root = root.to_path_buf();
        let mut builder = ignore::gitignore::GitignoreBuilder::new(&root);

        // Walk from root upward to find .gitignore files
        let mut dir = Some(root.as_path());
        while let Some(d) = dir {
            let gitignore_path = d.join(".gitignore");
            if gitignore_path.exists() {
                builder.add(gitignore_path);
            }

            // .git/info/exclude
            let exclude_path = d.join(".git").join("info").join("exclude");
            if exclude_path.exists() {
                builder.add(exclude_path);
            }

            dir = d.parent();
        }

        // Global gitignore
        let global = global_gitignore_path();
        if global.exists() {
            builder.add(global);
        }

        // Custom ignore file (e.g. .crabignore)
        if let Some(custom) = custom_ignore
            && custom.exists()
        {
            builder.add(custom);
        }

        let gitignore = builder.build().map_err(|e| {
            crab_common::Error::Other(format!("failed to build gitignore filter: {e}"))
        })?;

        Ok(Self { root, gitignore })
    }
}

/// Resolve the global gitignore path (never fails — returns a default).
fn global_gitignore_path() -> PathBuf {
    if let Ok(path) = std::env::var("GIT_GLOBAL_IGNORE") {
        return PathBuf::from(path);
    }
    crab_common::path::home_dir()
        .join(".config")
        .join("git")
        .join("ignore")
}

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

impl GitIgnoreFilter {
    /// Check whether `path` should be ignored.
    ///
    /// Calls [`Path::is_dir`] internally to supply the directory hint
    /// required by gitignore trailing-`/` rules. Use
    /// [`is_ignored_dir`](Self::is_ignored_dir) if you already know whether
    /// the path is a directory (avoids a `stat` syscall).
    #[must_use]
    pub fn is_ignored(&self, path: &Path) -> bool {
        self.is_ignored_dir(path, path.is_dir())
    }

    /// Check whether `path` should be ignored, with an explicit directory hint.
    #[must_use]
    pub fn is_ignored_dir(&self, path: &Path, is_dir: bool) -> bool {
        matches!(
            self.gitignore.matched(path, is_dir),
            ignore::Match::Ignore(_)
        )
    }

    /// The root directory this filter was built for.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup_temp_dir() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        // Create .gitignore
        fs::write(tmp.path().join(".gitignore"), "*.log\nbuild/\n").unwrap();
        // Create some files
        fs::write(tmp.path().join("main.rs"), "fn main() {}").unwrap();
        fs::write(tmp.path().join("debug.log"), "log data").unwrap();
        fs::create_dir(tmp.path().join("build")).unwrap();
        fs::write(tmp.path().join("build").join("output.o"), "binary").unwrap();
        tmp
    }

    #[test]
    fn basic_ignore() {
        let tmp = setup_temp_dir();
        let filter = GitIgnoreFilter::new(tmp.path()).unwrap();
        assert!(filter.is_ignored_dir(&tmp.path().join("debug.log"), false));
        assert!(!filter.is_ignored_dir(&tmp.path().join("main.rs"), false));
    }

    #[test]
    fn directory_rule() {
        let tmp = setup_temp_dir();
        let filter = GitIgnoreFilter::new(tmp.path()).unwrap();
        assert!(filter.is_ignored_dir(&tmp.path().join("build"), true));
    }

    #[test]
    fn negation() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(".gitignore"), "*.log\n!important.log\n").unwrap();
        fs::write(tmp.path().join("debug.log"), "").unwrap();
        fs::write(tmp.path().join("important.log"), "").unwrap();

        let filter = GitIgnoreFilter::new(tmp.path()).unwrap();
        assert!(filter.is_ignored_dir(&tmp.path().join("debug.log"), false));
        assert!(!filter.is_ignored_dir(&tmp.path().join("important.log"), false));
    }

    #[test]
    fn no_gitignore_file() {
        let tmp = tempfile::tempdir().unwrap();
        // No .gitignore — should succeed, nothing ignored
        let filter = GitIgnoreFilter::new(tmp.path()).unwrap();
        fs::write(tmp.path().join("test.txt"), "").unwrap();
        assert!(!filter.is_ignored_dir(&tmp.path().join("test.txt"), false));
    }

    #[test]
    fn root_accessor() {
        let tmp = tempfile::tempdir().unwrap();
        let filter = GitIgnoreFilter::new(tmp.path()).unwrap();
        assert_eq!(filter.root(), tmp.path());
    }

    #[test]
    fn custom_ignore_file() {
        let tmp = tempfile::tempdir().unwrap();
        let crabignore = tmp.path().join(".crabignore");
        fs::write(&crabignore, "*.tmp\n").unwrap();
        fs::write(tmp.path().join("data.tmp"), "").unwrap();
        fs::write(tmp.path().join("data.txt"), "").unwrap();

        let filter = GitIgnoreFilter::with_custom_ignore(tmp.path(), &crabignore).unwrap();
        assert!(filter.is_ignored_dir(&tmp.path().join("data.tmp"), false));
        assert!(!filter.is_ignored_dir(&tmp.path().join("data.txt"), false));
    }
}
