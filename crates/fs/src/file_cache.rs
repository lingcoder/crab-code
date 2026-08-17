//! Per-session cached file read.
//!
//! Many crab workflows re-read the same file multiple times within a
//! single user turn (grep → read, model checks context, then tool reads
//! again, etc.). This module provides an LRU-backed in-process cache
//! keyed by `(canonical_path, mtime)` so unchanged files are served from
//! memory on subsequent reads.
//!
//! ## Invalidation
//!
//! Each entry stores the file's `mtime` at the time it was cached.
//! [`FileCache::read`] calls `fs::metadata(path)?.modified()` on every
//! lookup — if the mtime differs from the cached value (file was
//! written to on disk), the entry is invalidated and the file re-read
//! from disk.
//!
//! ## Sizing
//!
//! The cache is bounded: both by total entry count (default 256) and
//! by total bytes held (default 64 MiB). Oldest entries are evicted
//! when either limit is exceeded.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use lru::LruCache;

/// Default maximum number of files held in the cache.
pub const DEFAULT_MAX_ENTRIES: usize = 256;

/// Default maximum total bytes held in the cache (64 MiB).
pub const DEFAULT_MAX_BYTES: usize = 64 * 1024 * 1024;

/// A single cached file. Held behind `Arc<str>` so readers get
/// ref-counted sharing; the cache itself keeps ownership.
#[derive(Debug, Clone)]
struct CachedFile {
    /// File contents.
    contents: Arc<String>,
    /// mtime at the time this entry was populated.
    mtime: SystemTime,
}

impl CachedFile {
    fn bytes(&self) -> usize {
        self.contents.len()
    }
}

/// LRU cache of recently-read file contents keyed by canonical path.
///
/// Not thread-safe on its own — wrap in `tokio::sync::Mutex` or
/// `std::sync::Mutex` if sharing across tasks. The intended scope is
/// per-session, not process-global.
pub struct FileCache {
    inner: LruCache<PathBuf, CachedFile>,
    max_bytes: usize,
    current_bytes: usize,
}

impl FileCache {
    /// Create a cache with default limits ([`DEFAULT_MAX_ENTRIES`] +
    /// [`DEFAULT_MAX_BYTES`]).
    #[must_use]
    pub fn new() -> Self {
        Self::with_limits(DEFAULT_MAX_ENTRIES, DEFAULT_MAX_BYTES)
    }

    /// Create a cache with custom limits.
    ///
    /// `max_entries` must be non-zero; a zero value is clamped to 1.
    #[must_use]
    pub fn with_limits(max_entries: usize, max_bytes: usize) -> Self {
        let capacity =
            std::num::NonZeroUsize::new(max_entries.max(1)).expect("max_entries clamped to >= 1");
        Self {
            inner: LruCache::new(capacity),
            max_bytes,
            current_bytes: 0,
        }
    }

    /// Read a file, consulting the cache first.
    ///
    /// Fast path: file is cached AND on-disk mtime matches → return the
    /// cached bytes without touching the filesystem beyond the metadata
    /// check.
    ///
    /// Slow path: cache miss or mtime drift → `fs::read_to_string`
    /// (UTF-8 only) + cache on the way out.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the file cannot be read (missing, permission
    /// denied, not UTF-8).
    pub fn read(&mut self, path: &Path) -> crab_core::Result<Arc<String>> {
        // Canonicalise so `./foo` and `foo` hit the same cache entry.
        // Fall back to the raw path if canonicalise fails (e.g. path
        // doesn't yet exist — which would also fail fs::read below).
        let key = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

        let on_disk_mtime = fs::metadata(&key).and_then(|m| m.modified()).map_err(|e| {
            crab_core::Error::Other(format!(
                "file_cache: stat failed for {}: {e}",
                key.display()
            ))
        })?;

        // Cache hit with matching mtime → fast path.
        if let Some(entry) = self.inner.get(&key)
            && entry.mtime == on_disk_mtime
        {
            return Ok(Arc::clone(&entry.contents));
        }

        // Miss or drift: read + populate.
        let raw = fs::read_to_string(&key).map_err(|e| {
            crab_core::Error::Other(format!(
                "file_cache: read failed for {}: {e}",
                key.display()
            ))
        })?;
        let contents = Arc::new(raw);
        let entry = CachedFile {
            contents: Arc::clone(&contents),
            mtime: on_disk_mtime,
        };

        // Accounting: drop old entry's bytes before inserting.
        if let Some(old) = self.inner.pop(&key) {
            self.current_bytes = self.current_bytes.saturating_sub(old.bytes());
        }
        self.current_bytes = self.current_bytes.saturating_add(entry.bytes());
        self.inner.put(key, entry);
        self.enforce_byte_limit();
        Ok(contents)
    }

    /// Explicitly invalidate an entry (e.g. after a tool wrote to the file).
    ///
    /// Returns `true` if an entry was present.
    pub fn invalidate(&mut self, path: &Path) -> bool {
        let key = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if let Some(old) = self.inner.pop(&key) {
            self.current_bytes = self.current_bytes.saturating_sub(old.bytes());
            true
        } else {
            false
        }
    }

    /// Drop every cached entry.
    pub fn clear(&mut self) {
        self.inner.clear();
        self.current_bytes = 0;
    }

    /// Number of currently-cached entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// `true` when no entries are cached.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Total bytes of file content currently held in the cache.
    #[must_use]
    pub fn current_bytes(&self) -> usize {
        self.current_bytes
    }

    /// Evict LRU entries until `current_bytes <= max_bytes`.
    fn enforce_byte_limit(&mut self) {
        while self.current_bytes > self.max_bytes {
            match self.inner.pop_lru() {
                Some((_, old)) => {
                    self.current_bytes = self.current_bytes.saturating_sub(old.bytes());
                }
                None => break,
            }
        }
    }
}

impl Default for FileCache {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for FileCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileCache")
            .field("entries", &self.inner.len())
            .field("current_bytes", &self.current_bytes)
            .field("max_bytes", &self.max_bytes)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::thread;
    use std::time::Duration;

    /// No `sync_all` here on purpose. These tests only need the write to be
    /// visible to a later `fs::metadata` / `fs::read_to_string` in the same
    /// process, which the page cache already guarantees. Forcing a physical
    /// flush buys nothing and fails with `EIO` on GitHub's macOS runners.
    fn write_file(path: &Path, contents: &str) {
        let mut f = fs::File::create(path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
    }

    #[test]
    fn cold_read_populates_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a.txt");
        write_file(&path, "hello");

        let mut cache = FileCache::new();
        assert!(cache.is_empty());
        let contents = cache.read(&path).unwrap();
        assert_eq!(&**contents, "hello");
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.current_bytes(), 5);
    }

    #[test]
    fn warm_read_serves_from_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a.txt");
        write_file(&path, "hello");

        let mut cache = FileCache::new();
        let a = cache.read(&path).unwrap();
        let b = cache.read(&path).unwrap();
        // Same Arc → cache served the second read.
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn mtime_drift_invalidates_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a.txt");
        write_file(&path, "old");

        let mut cache = FileCache::new();
        let first = cache.read(&path).unwrap();
        assert_eq!(&**first, "old");

        // Ensure mtime can change.
        thread::sleep(Duration::from_millis(20));
        write_file(&path, "new");

        let second = cache.read(&path).unwrap();
        assert_eq!(&**second, "new");
        // Different Arc, since we re-read from disk.
        assert!(!Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn invalidate_removes_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a.txt");
        write_file(&path, "hello");

        let mut cache = FileCache::new();
        cache.read(&path).unwrap();
        assert_eq!(cache.len(), 1);

        assert!(cache.invalidate(&path));
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.current_bytes(), 0);
        assert!(!cache.invalidate(&path));
    }

    #[test]
    fn clear_empties_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.txt");
        let b = tmp.path().join("b.txt");
        write_file(&a, "AAA");
        write_file(&b, "BB");

        let mut cache = FileCache::new();
        cache.read(&a).unwrap();
        cache.read(&b).unwrap();
        assert_eq!(cache.len(), 2);
        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.current_bytes(), 0);
    }

    #[test]
    fn entry_count_limit_evicts_lru() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cache = FileCache::with_limits(2, DEFAULT_MAX_BYTES);
        for i in 0..3 {
            let p = tmp.path().join(format!("{i}.txt"));
            write_file(&p, &"x".repeat(10));
            cache.read(&p).unwrap();
        }
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn byte_limit_evicts_lru() {
        let tmp = tempfile::tempdir().unwrap();
        // 100-byte budget; each file is 60 bytes → 2nd file evicts 1st.
        let mut cache = FileCache::with_limits(256, 100);
        let a = tmp.path().join("a.txt");
        let b = tmp.path().join("b.txt");
        write_file(&a, &"a".repeat(60));
        write_file(&b, &"b".repeat(60));
        cache.read(&a).unwrap();
        cache.read(&b).unwrap();
        assert_eq!(cache.len(), 1);
        assert!(cache.current_bytes() <= 100);
    }

    #[test]
    fn missing_file_returns_err() {
        let mut cache = FileCache::new();
        let err = cache
            .read(Path::new("/definitely/does/not/exist.txt"))
            .unwrap_err();
        assert!(err.to_string().contains("stat failed"));
    }

    #[test]
    fn zero_capacity_clamped_to_one() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cache = FileCache::with_limits(0, DEFAULT_MAX_BYTES);
        let p = tmp.path().join("a.txt");
        write_file(&p, "x");
        cache.read(&p).unwrap();
        assert_eq!(cache.len(), 1);
    }
}
