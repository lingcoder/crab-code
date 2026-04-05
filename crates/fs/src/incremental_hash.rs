//! Incremental file hashing and change detection.
//!
//! Provides fast file hashing via [`hash_file`], a [`FileHashCache`] that
//! avoids re-hashing unchanged files (mtime-based), and [`detect_changes`]
//! for diffing two snapshots.

use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hasher};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Serialize;

// ── Hashing ─────────────────────────────────────────────────────────

/// A 64-bit file content hash (`SipHash` via `DefaultHasher`).
pub type FileHash = u64;

/// Hash file contents. Returns the 64-bit `SipHash` of the full file content.
///
/// # Errors
///
/// Returns an error if the file cannot be read.
pub fn hash_file(path: &Path) -> crab_common::Result<FileHash> {
    let content = std::fs::read(path)?;
    Ok(hash_bytes(&content))
}

/// Hash a byte slice.
#[must_use]
pub fn hash_bytes(data: &[u8]) -> FileHash {
    let mut hasher = DefaultHasher::new();
    hasher.write(data);
    hasher.finish()
}

/// Hash file contents incrementally by reading in chunks.
///
/// Useful for large files where reading the entire content into memory
/// at once is undesirable.
///
/// # Errors
///
/// Returns an error if the file cannot be read.
pub fn hash_file_chunked(path: &Path, chunk_size: usize) -> crab_common::Result<FileHash> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = DefaultHasher::new();
    let mut buf = vec![0u8; chunk_size];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.write(&buf[..n]);
    }
    Ok(hasher.finish())
}

// ── Hash cache ──────────────────────────────────────────────────────

/// Cached hash entry with modification time for staleness detection.
#[derive(Debug, Clone)]
struct CacheEntry {
    hash: FileHash,
    mtime: SystemTime,
    size: u64,
}

/// Caches file hashes and only recomputes when the file's mtime or size changes.
#[derive(Debug)]
pub struct FileHashCache {
    entries: BTreeMap<PathBuf, CacheEntry>,
}

impl FileHashCache {
    /// Create an empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    /// Get the hash for a file, recomputing only if mtime/size changed.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or its metadata is unavailable.
    pub fn get_hash(&mut self, path: &Path) -> crab_common::Result<FileHash> {
        let meta = std::fs::metadata(path)?;
        let mtime = meta
            .modified()
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let size = meta.len();

        if let Some(entry) = self.entries.get(path)
            && entry.mtime == mtime && entry.size == size {
                return Ok(entry.hash);
            }

        let hash = hash_file(path)?;
        self.entries.insert(
            path.to_path_buf(),
            CacheEntry { hash, mtime, size },
        );
        Ok(hash)
    }

    /// Invalidate the cached hash for a specific file.
    pub fn invalidate(&mut self, path: &Path) {
        self.entries.remove(path);
    }

    /// Invalidate all entries whose path starts with `prefix`.
    pub fn invalidate_prefix(&mut self, prefix: &Path) {
        self.entries.retain(|k, _| !k.starts_with(prefix));
    }

    /// Clear the entire cache.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Number of cached entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for FileHashCache {
    fn default() -> Self {
        Self::new()
    }
}

// ── Snapshots & change detection ────────────────────────────────────

/// A snapshot of file hashes at a point in time.
pub type HashSnapshot = BTreeMap<PathBuf, FileHash>;

/// Kind of file change between two snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    /// File was added (not in old snapshot).
    Added,
    /// File was removed (not in new snapshot).
    Removed,
    /// File content changed (hash differs).
    Modified,
}

/// A detected file change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileChange {
    /// Path of the changed file.
    pub path: PathBuf,
    /// Kind of change.
    pub kind: ChangeKind,
}

impl std::fmt::Display for FileChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self.kind {
            ChangeKind::Added => "added",
            ChangeKind::Removed => "removed",
            ChangeKind::Modified => "modified",
        };
        write!(f, "{}: {}", label, self.path.display())
    }
}

/// Compare two hash snapshots and return all changes.
///
/// Changes are sorted by path for deterministic output.
#[must_use]
pub fn detect_changes(old: &HashSnapshot, new: &HashSnapshot) -> Vec<FileChange> {
    let mut changes = Vec::new();

    // Files in old but not in new → removed
    // Files in old and new with different hash → modified
    for (path, old_hash) in old {
        match new.get(path) {
            None => changes.push(FileChange {
                path: path.clone(),
                kind: ChangeKind::Removed,
            }),
            Some(new_hash) if new_hash != old_hash => changes.push(FileChange {
                path: path.clone(),
                kind: ChangeKind::Modified,
            }),
            _ => {}
        }
    }

    // Files in new but not in old → added
    for path in new.keys() {
        if !old.contains_key(path) {
            changes.push(FileChange {
                path: path.clone(),
                kind: ChangeKind::Added,
            });
        }
    }

    changes.sort_by(|a, b| a.path.cmp(&b.path));
    changes
}

/// Build a hash snapshot for a directory, respecting `.gitignore`.
///
/// # Errors
///
/// Returns an error if the directory cannot be walked.
pub fn snapshot_directory(root: &Path) -> crab_common::Result<HashSnapshot> {
    let mut snapshot = BTreeMap::new();

    let walker = ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .build();

    for entry in walker {
        let entry = entry.map_err(|e| crab_common::Error::Other(format!("walk error: {e}")))?;

        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }

        let path = entry.into_path();
        if let Ok(h) = hash_file(&path) {
            snapshot.insert(path, h);
        }
    }

    Ok(snapshot)
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    // ── hash_bytes ──────────────────────────────────────────────

    #[test]
    fn hash_bytes_deterministic() {
        let a = hash_bytes(b"hello world");
        let b = hash_bytes(b"hello world");
        assert_eq!(a, b);
    }

    #[test]
    fn hash_bytes_different_input() {
        let a = hash_bytes(b"hello");
        let b = hash_bytes(b"world");
        assert_ne!(a, b);
    }

    #[test]
    fn hash_empty() {
        let h = hash_bytes(b"");
        assert_ne!(h, 0); // SipHash of empty is non-zero
    }

    // ── hash_file ──────────────────────────────────────────────

    #[test]
    fn hash_file_works() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("test.txt");
        fs::write(&p, "hello").unwrap();
        let h = hash_file(&p).unwrap();
        assert_eq!(h, hash_bytes(b"hello"));
    }

    #[test]
    fn hash_file_nonexistent() {
        let result = hash_file(Path::new("/nonexistent/file.txt"));
        assert!(result.is_err());
    }

    // ── hash_file_chunked ──────────────────────────────────────

    #[test]
    fn hash_file_chunked_matches_full() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("data.bin");
        let data: Vec<u8> = (0..1000).map(|i| (i % 256) as u8).collect();
        fs::write(&p, &data).unwrap();

        let full = hash_file(&p).unwrap();
        let chunked = hash_file_chunked(&p, 64).unwrap();
        assert_eq!(full, chunked);
    }

    #[test]
    fn hash_file_chunked_small_chunk() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("small.txt");
        fs::write(&p, "abc").unwrap();

        let h = hash_file_chunked(&p, 1).unwrap();
        assert_eq!(h, hash_bytes(b"abc"));
    }

    // ── FileHashCache ──────────────────────────────────────────

    #[test]
    fn cache_new_is_empty() {
        let cache = FileHashCache::new();
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn cache_get_hash_populates() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("file.txt");
        fs::write(&p, "content").unwrap();

        let mut cache = FileHashCache::new();
        let h1 = cache.get_hash(&p).unwrap();
        assert_eq!(cache.len(), 1);

        // Second call should return cached value
        let h2 = cache.get_hash(&p).unwrap();
        assert_eq!(h1, h2);
    }

    #[test]
    fn cache_invalidate() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("file.txt");
        fs::write(&p, "content").unwrap();

        let mut cache = FileHashCache::new();
        let _ = cache.get_hash(&p).unwrap();
        assert_eq!(cache.len(), 1);

        cache.invalidate(&p);
        assert!(cache.is_empty());
    }

    #[test]
    fn cache_invalidate_prefix() {
        let dir = tempdir().unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        let p1 = sub.join("a.txt");
        let p2 = dir.path().join("b.txt");
        fs::write(&p1, "a").unwrap();
        fs::write(&p2, "b").unwrap();

        let mut cache = FileHashCache::new();
        let _ = cache.get_hash(&p1).unwrap();
        let _ = cache.get_hash(&p2).unwrap();
        assert_eq!(cache.len(), 2);

        cache.invalidate_prefix(&sub);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn cache_clear() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("f.txt");
        fs::write(&p, "x").unwrap();

        let mut cache = FileHashCache::new();
        let _ = cache.get_hash(&p).unwrap();
        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn cache_default() {
        let cache = FileHashCache::default();
        assert!(cache.is_empty());
    }

    // ── detect_changes ─────────────────────────────────────────

    #[test]
    fn detect_no_changes() {
        let mut snap: HashSnapshot = BTreeMap::new();
        snap.insert(PathBuf::from("a.rs"), 123);
        let changes = detect_changes(&snap, &snap);
        assert!(changes.is_empty());
    }

    #[test]
    fn detect_added() {
        let old: HashSnapshot = BTreeMap::new();
        let mut new: HashSnapshot = BTreeMap::new();
        new.insert(PathBuf::from("new.rs"), 42);

        let changes = detect_changes(&old, &new);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, ChangeKind::Added);
        assert_eq!(changes[0].path, PathBuf::from("new.rs"));
    }

    #[test]
    fn detect_removed() {
        let mut old: HashSnapshot = BTreeMap::new();
        old.insert(PathBuf::from("gone.rs"), 42);
        let new: HashSnapshot = BTreeMap::new();

        let changes = detect_changes(&old, &new);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, ChangeKind::Removed);
    }

    #[test]
    fn detect_modified() {
        let mut old: HashSnapshot = BTreeMap::new();
        old.insert(PathBuf::from("mod.rs"), 1);
        let mut new: HashSnapshot = BTreeMap::new();
        new.insert(PathBuf::from("mod.rs"), 2);

        let changes = detect_changes(&old, &new);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, ChangeKind::Modified);
    }

    #[test]
    fn detect_mixed_changes() {
        let mut old: HashSnapshot = BTreeMap::new();
        old.insert(PathBuf::from("kept.rs"), 1);
        old.insert(PathBuf::from("modified.rs"), 2);
        old.insert(PathBuf::from("removed.rs"), 3);

        let mut new: HashSnapshot = BTreeMap::new();
        new.insert(PathBuf::from("added.rs"), 10);
        new.insert(PathBuf::from("kept.rs"), 1);
        new.insert(PathBuf::from("modified.rs"), 99);

        let changes = detect_changes(&old, &new);
        assert_eq!(changes.len(), 3);
        // Sorted by path
        assert_eq!(changes[0].path, PathBuf::from("added.rs"));
        assert_eq!(changes[0].kind, ChangeKind::Added);
        assert_eq!(changes[1].path, PathBuf::from("modified.rs"));
        assert_eq!(changes[1].kind, ChangeKind::Modified);
        assert_eq!(changes[2].path, PathBuf::from("removed.rs"));
        assert_eq!(changes[2].kind, ChangeKind::Removed);
    }

    #[test]
    fn detect_both_empty() {
        let changes = detect_changes(&BTreeMap::new(), &BTreeMap::new());
        assert!(changes.is_empty());
    }

    // ── FileChange display ─────────────────────────────────────

    #[test]
    fn file_change_display() {
        let c = FileChange {
            path: PathBuf::from("src/main.rs"),
            kind: ChangeKind::Added,
        };
        assert_eq!(c.to_string(), "added: src/main.rs");
    }

    // ── snapshot_directory ──────────────────────────────────────

    #[test]
    fn snapshot_empty_dir() {
        let dir = tempdir().unwrap();
        let snap = snapshot_directory(dir.path()).unwrap();
        assert!(snap.is_empty());
    }

    #[test]
    fn snapshot_with_files() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "hello").unwrap();
        fs::write(dir.path().join("b.rs"), "fn main() {}").unwrap();

        let snap = snapshot_directory(dir.path()).unwrap();
        assert_eq!(snap.len(), 2);
    }

    #[test]
    fn snapshot_then_detect() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "v1").unwrap();

        let snap1 = snapshot_directory(dir.path()).unwrap();

        // Modify file and add new one
        fs::write(dir.path().join("a.txt"), "v2").unwrap();
        fs::write(dir.path().join("b.txt"), "new").unwrap();

        let snap2 = snapshot_directory(dir.path()).unwrap();
        let changes = detect_changes(&snap1, &snap2);

        assert_eq!(changes.len(), 2);
        // Should have one modified and one added
        let kinds: Vec<_> = changes.iter().map(|c| &c.kind).collect();
        assert!(kinds.contains(&&ChangeKind::Modified));
        assert!(kinds.contains(&&ChangeKind::Added));
    }

    // ── Serialization ──────────────────────────────────────────

    #[test]
    fn file_change_serializes() {
        let c = FileChange {
            path: PathBuf::from("test.rs"),
            kind: ChangeKind::Modified,
        };
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("modified"));
        assert!(json.contains("test.rs"));
    }
}
