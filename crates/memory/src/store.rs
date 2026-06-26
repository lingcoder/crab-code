use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::types::{MemoryMetadata, extract_body, parse_frontmatter};

/// Maximum number of memory files returned by [`MemoryStore::scan`].
const MAX_SCAN_RESULTS: usize = 200;

// ─── MemoryFile ─────────────────────────────────────────────────

/// A parsed memory file: metadata (frontmatter) + body + filesystem info.
#[derive(Debug, Clone)]
pub struct MemoryFile {
    pub filename: String,
    pub path: PathBuf,
    pub metadata: MemoryMetadata,
    pub body: String,
    pub mtime: Option<SystemTime>,
}

// ─── MemoryStore ────────────────────────────────────────────────

/// File-backed store for memory markdown files.
///
/// Each memory is a `.md` file inside `dir`, optionally containing YAML
/// frontmatter parsed into [`MemoryMetadata`].
#[derive(Debug, Clone)]
pub struct MemoryStore {
    dir: PathBuf,
}

impl MemoryStore {
    /// Create a new store rooted at `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// Return the root directory of this store.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Validate `filename` against path-traversal and resolve it inside `dir`.
    fn resolve(&self, filename: &str) -> crab_core::Result<PathBuf> {
        let safe = crate::security::validate_memory_key(filename)?;
        Ok(self.dir.join(safe))
    }

    /// Persist `content` to `<dir>/<filename>`, creating the directory tree
    /// if it does not exist.
    pub fn save(&self, filename: &str, content: &str) -> crab_core::Result<()> {
        let path = self.resolve(filename)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, content)?;
        Ok(())
    }

    /// Read the raw content of `<dir>/<filename>`.
    ///
    /// Returns `Ok(None)` when the file does not exist.
    pub fn load(&self, filename: &str) -> crab_core::Result<Option<String>> {
        let path = self.resolve(filename)?;
        match fs::read_to_string(&path) {
            Ok(content) => Ok(Some(content)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Delete `<dir>/<filename>`.
    ///
    /// Does **not** return an error when the file is already absent.
    pub fn delete(&self, filename: &str) -> crab_core::Result<()> {
        let path = self.resolve(filename)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Scan the store directory for `.md` files (excluding `MEMORY.md`),
    /// parse their frontmatter, and return them sorted by modification time
    /// (newest first), capped at [`MAX_SCAN_RESULTS`].
    pub fn scan(&self) -> crab_core::Result<Vec<MemoryFile>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };

        let mut files: Vec<MemoryFile> = Vec::new();

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            // Only .md files.
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }

            let filename = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_owned(),
                None => continue,
            };

            // Skip the index file.
            if filename.eq_ignore_ascii_case("MEMORY.md") {
                continue;
            }

            let Ok(content) = fs::read_to_string(&path) else {
                continue;
            };

            let Some(metadata) = parse_frontmatter(&content) else {
                continue;
            };

            let body = extract_body(&content).to_owned();
            let mtime = entry.metadata().ok().and_then(|m| m.modified().ok());

            files.push(MemoryFile {
                filename,
                path,
                metadata,
                body,
                mtime,
            });
        }

        // Sort by mtime descending (newest first). Files without mtime go last.
        files.sort_by(|a, b| {
            let ta = a.mtime.unwrap_or(SystemTime::UNIX_EPOCH);
            let tb = b.mtime.unwrap_or(SystemTime::UNIX_EPOCH);
            tb.cmp(&ta)
        });

        files.truncate(MAX_SCAN_RESULTS);
        Ok(files)
    }
}

// ─── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Duration;

    use super::*;

    fn sample_content(name: &str) -> String {
        format!("---\nname: {name}\ndescription: test memory\ntype: user\n---\nBody of {name}\n")
    }

    #[test]
    fn save_and_load() {
        let tmp = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(tmp.path());
        store.save("a.md", "hello").unwrap();
        let loaded = store.load("a.md").unwrap();
        assert_eq!(loaded.as_deref(), Some("hello"));
    }

    #[test]
    fn load_nonexistent_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(tmp.path());
        assert_eq!(store.load("missing.md").unwrap(), None);
    }

    #[test]
    fn save_creates_directory_if_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let deep = tmp.path().join("a").join("b").join("c");
        let store = MemoryStore::new(&deep);
        store.save("x.md", "content").unwrap();
        assert_eq!(store.load("x.md").unwrap().as_deref(), Some("content"));
    }

    #[test]
    fn save_overwrites_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(tmp.path());
        store.save("f.md", "v1").unwrap();
        store.save("f.md", "v2").unwrap();
        assert_eq!(store.load("f.md").unwrap().as_deref(), Some("v2"));
    }

    #[test]
    fn delete_existing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(tmp.path());
        store.save("del.md", "data").unwrap();
        store.delete("del.md").unwrap();
        assert_eq!(store.load("del.md").unwrap(), None);
    }

    #[test]
    fn delete_nonexistent_is_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(tmp.path());
        // Should not error.
        store.delete("nope.md").unwrap();
    }

    #[test]
    fn scan_parses_files_sorted_by_mtime() {
        let tmp = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(tmp.path());

        store.save("oldest.md", &sample_content("oldest")).unwrap();
        thread::sleep(Duration::from_millis(50));
        store.save("middle.md", &sample_content("middle")).unwrap();
        thread::sleep(Duration::from_millis(50));
        store.save("newest.md", &sample_content("newest")).unwrap();

        let files = store.scan().unwrap();
        assert_eq!(files.len(), 3);
        // Newest first.
        assert_eq!(files[0].filename, "newest.md");
        assert_eq!(files[1].filename, "middle.md");
        assert_eq!(files[2].filename, "oldest.md");
        // Body is parsed correctly.
        assert_eq!(files[0].body, "Body of newest\n");
    }

    #[test]
    fn scan_skips_memory_md_and_non_md() {
        let tmp = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(tmp.path());

        store.save("valid.md", &sample_content("valid")).unwrap();
        store.save("MEMORY.md", "# Index file").unwrap();
        store.save("notes.txt", "plain text").unwrap();

        let files = store.scan().unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].filename, "valid.md");
    }

    #[test]
    fn scan_nonexistent_dir_returns_empty() {
        let store = MemoryStore::new("/tmp/crab_memory_nonexistent_dir_test_12345");
        let files = store.scan().unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn dir_accessor() {
        let store = MemoryStore::new("/some/path");
        assert_eq!(store.dir(), Path::new("/some/path"));
    }
}
