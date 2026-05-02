//! Per-session file-history snapshots backing the `/rewind` slash command.
//!
//! Every time a file is about to be edited (Edit / Write / Notebook tool),
//! the pre-edit contents are saved to
//! `<base_dir>/{session_id}/{hash}@v{version}`. A user `/rewind N` restores
//! the file to its state as of version `N`.
//!
//! Storage uses the on-disk hash of the **file path** (not the content), so
//! repeated edits to the same file accumulate versions `@v1`, `@v2`, … and
//! a stable key for lookup. Each session gets its own subdirectory, and an
//! LRU cap of 100 snapshots per session prevents unbounded growth.
//!
//! This module is session-scoped — it does not touch the tool registry
//! directly. The agent runtime owns the [`FileHistory`] handle and exposes
//! it to Edit/Write tools through a [`crab_core::tool::ToolContextExt`]
//! callback.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// Maximum number of snapshots retained per session (oldest evicted first).
pub const MAX_SNAPSHOTS_PER_SESSION: usize = 100;

/// A single pre-edit snapshot of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// The original file's absolute path, as observed when the snapshot was taken.
    pub path: PathBuf,
    /// Monotonic version number, starting at 1 for the first snapshot of a
    /// given file within a session.
    pub version: u32,
    /// On-disk storage path (`{base}/{session_id}/{hash}@v{version}`).
    pub storage: PathBuf,
}

/// Errors raised by [`FileHistory`].
#[derive(Debug, Error)]
pub enum SnapshotError {
    #[error("i/o: {0}")]
    Io(#[from] io::Error),
    #[error("snapshot not found: {path} @v{version}")]
    NotFound { path: PathBuf, version: u32 },
    #[error("no snapshots recorded for {path}")]
    EmptyHistory { path: PathBuf },
}

/// Session-scoped file-edit snapshot store.
///
/// One `FileHistory` per session. Lives at `{base_dir}/{session_id}/` on
/// disk; cheap to construct, does not read anything until the first
/// `track_edit` call.
pub struct FileHistory {
    session_dir: PathBuf,
    /// In-memory index: path → ordered list of `(version, storage_path)`.
    ///
    /// Uses `BTreeMap` so versions stay sorted ascending — LRU eviction is
    /// "drop lowest version" which matches CCB's fileHistory semantics.
    index: BTreeMap<PathBuf, Vec<(u32, PathBuf)>>,
}

impl FileHistory {
    /// Create a new history rooted at `base_dir/session_id/`. The directory
    /// is not created until the first snapshot is taken.
    #[must_use]
    pub fn new(base_dir: impl Into<PathBuf>, session_id: impl AsRef<str>) -> Self {
        let session_dir = base_dir.into().join(session_id.as_ref());
        Self {
            session_dir,
            index: BTreeMap::new(),
        }
    }

    /// Returns the storage root (`{base_dir}/{session_id}`).
    #[must_use]
    pub fn session_dir(&self) -> &Path {
        self.session_dir.as_path()
    }

    /// Number of snapshots currently tracked across every file.
    #[must_use]
    pub fn len(&self) -> usize {
        self.index.values().map(Vec::len).sum()
    }

    /// Whether any snapshots have been recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.index.values().all(Vec::is_empty)
    }

    /// All file paths that currently have at least one tracked snapshot.
    #[must_use]
    pub fn tracked_files(&self) -> Vec<PathBuf> {
        self.index
            .iter()
            .filter(|(_, entries)| !entries.is_empty())
            .map(|(path, _)| path.clone())
            .collect()
    }

    /// Snapshot the pre-edit contents of `path`. Returns the new [`Snapshot`].
    ///
    /// Allocates a fresh version number for this file (previous max + 1),
    /// writes `contents` to `{session_dir}/{hash}@v{version}`, and records
    /// it in the in-memory index. When the session accumulates more than
    /// [`MAX_SNAPSHOTS_PER_SESSION`] snapshots total, the oldest version is
    /// deleted from disk and the index.
    pub fn track_edit(&mut self, path: &Path, contents: &[u8]) -> Result<Snapshot, SnapshotError> {
        std::fs::create_dir_all(&self.session_dir)?;

        let entries = self.index.entry(path.to_path_buf()).or_default();
        let version = entries.last().map_or(1, |(v, _)| v + 1);
        let storage = self.session_dir.join(snapshot_filename(path, version));
        std::fs::write(&storage, contents)?;
        entries.push((version, storage.clone()));

        self.evict_over_cap()?;

        Ok(Snapshot {
            path: path.to_path_buf(),
            version,
            storage,
        })
    }

    /// List snapshots recorded for a given file, oldest first.
    #[must_use]
    pub fn snapshots_for(&self, path: &Path) -> Vec<Snapshot> {
        self.index
            .get(path)
            .map(|entries| {
                entries
                    .iter()
                    .map(|(v, storage)| Snapshot {
                        path: path.to_path_buf(),
                        version: *v,
                        storage: storage.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Restore `path` to the snapshot at `version`. The current on-disk
    /// contents are replaced; no backup of the post-edit state is taken.
    pub fn rewind(&self, path: &Path, version: u32) -> Result<(), SnapshotError> {
        let entries = self
            .index
            .get(path)
            .ok_or_else(|| SnapshotError::EmptyHistory {
                path: path.to_path_buf(),
            })?;
        let storage = entries
            .iter()
            .find(|(v, _)| *v == version)
            .map(|(_, s)| s)
            .ok_or_else(|| SnapshotError::NotFound {
                path: path.to_path_buf(),
                version,
            })?;
        let bytes = std::fs::read(storage)?;
        std::fs::write(path, bytes)?;
        Ok(())
    }

    /// Restore `path` to its most recently snapshotted state.
    pub fn rewind_to_latest(&self, path: &Path) -> Result<u32, SnapshotError> {
        let entries = self
            .index
            .get(path)
            .ok_or_else(|| SnapshotError::EmptyHistory {
                path: path.to_path_buf(),
            })?;
        let version =
            entries
                .last()
                .map(|(v, _)| *v)
                .ok_or_else(|| SnapshotError::EmptyHistory {
                    path: path.to_path_buf(),
                })?;
        self.rewind(path, version)?;
        Ok(version)
    }

    /// Drop the oldest snapshot(s) until the total count is ≤ the per-session cap.
    fn evict_over_cap(&mut self) -> io::Result<()> {
        while self.len() > MAX_SNAPSHOTS_PER_SESSION {
            // Find the file with the lowest first-version snapshot (oldest).
            let oldest_path_opt = self
                .index
                .iter()
                .filter_map(|(p, entries)| entries.first().map(|(v, _)| (*v, p.clone())))
                .min_by_key(|(v, _)| *v)
                .map(|(_, p)| p);
            let Some(oldest_path) = oldest_path_opt else {
                break;
            };

            if let Some(entries) = self.index.get_mut(&oldest_path)
                && !entries.is_empty()
            {
                let (_, storage) = entries.remove(0);
                let _ = std::fs::remove_file(&storage);
                if entries.is_empty() {
                    self.index.remove(&oldest_path);
                }
            } else {
                break;
            }
        }
        Ok(())
    }
}

/// Build the on-disk filename for a snapshot: `{hash}@v{version}`.
///
/// The hash is a deterministic digest of the path string so different files
/// never share a storage slot; identical paths across versions do.
fn snapshot_filename(path: &Path, version: u32) -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write(path.as_os_str().as_encoded_bytes());
    let hash = hasher.finish();
    format!("{hash:016x}@v{version}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_history() -> (tempfile::TempDir, FileHistory) {
        let dir = tempfile::tempdir().unwrap();
        let history = FileHistory::new(dir.path(), "test-session");
        (dir, history)
    }

    #[test]
    fn new_history_is_empty_and_lazy() {
        let (dir, history) = temp_history();
        assert!(history.is_empty());
        assert_eq!(history.len(), 0);
        assert!(!dir.path().join("test-session").exists());
    }

    #[test]
    fn track_edit_stores_contents_on_disk() {
        let (_dir, mut history) = temp_history();
        let edit_target = tempfile::NamedTempFile::new().unwrap();
        let path = edit_target.path();

        let snap = history.track_edit(path, b"version 1 content").unwrap();
        assert_eq!(snap.version, 1);
        assert!(snap.storage.exists());
        assert_eq!(std::fs::read(&snap.storage).unwrap(), b"version 1 content");
    }

    #[test]
    fn track_edit_increments_version_per_file() {
        let (_dir, mut history) = temp_history();
        let t1 = tempfile::NamedTempFile::new().unwrap();
        let p1 = t1.path();
        let t2 = tempfile::NamedTempFile::new().unwrap();
        let p2 = t2.path();

        assert_eq!(history.track_edit(p1, b"p1v1").unwrap().version, 1);
        assert_eq!(history.track_edit(p1, b"p1v2").unwrap().version, 2);
        assert_eq!(history.track_edit(p2, b"p2v1").unwrap().version, 1);
        assert_eq!(history.track_edit(p1, b"p1v3").unwrap().version, 3);
    }

    #[test]
    fn rewind_restores_exact_bytes() {
        let (_dir, mut history) = temp_history();
        let file = tempfile::NamedTempFile::new().unwrap();
        let path = file.path();

        std::fs::write(path, b"original").unwrap();
        history.track_edit(path, b"original").unwrap();

        std::fs::write(path, b"post-edit garbage").unwrap();
        history.rewind(path, 1).unwrap();

        assert_eq!(std::fs::read(path).unwrap(), b"original");
    }

    #[test]
    fn rewind_to_latest_uses_highest_version() {
        let (_dir, mut history) = temp_history();
        let file = tempfile::NamedTempFile::new().unwrap();
        let path = file.path();

        history.track_edit(path, b"v1").unwrap();
        history.track_edit(path, b"v2").unwrap();
        history.track_edit(path, b"v3").unwrap();

        std::fs::write(path, b"scrambled").unwrap();
        let restored_version = history.rewind_to_latest(path).unwrap();
        assert_eq!(restored_version, 3);
        assert_eq!(std::fs::read(path).unwrap(), b"v3");
    }

    #[test]
    fn rewind_unknown_version_returns_not_found() {
        let (_dir, mut history) = temp_history();
        let file = tempfile::NamedTempFile::new().unwrap();
        let path = file.path();

        history.track_edit(path, b"v1").unwrap();
        let err = history.rewind(path, 99).unwrap_err();
        assert!(matches!(err, SnapshotError::NotFound { .. }));
    }

    #[test]
    fn rewind_unknown_path_returns_empty_history() {
        let (_dir, history) = temp_history();
        let err = history
            .rewind(Path::new("/nonexistent/foo.rs"), 1)
            .unwrap_err();
        assert!(matches!(err, SnapshotError::EmptyHistory { .. }));
    }

    #[test]
    fn snapshots_for_lists_in_order() {
        let (_dir, mut history) = temp_history();
        let file = tempfile::NamedTempFile::new().unwrap();
        let path = file.path();

        history.track_edit(path, b"a").unwrap();
        history.track_edit(path, b"b").unwrap();
        history.track_edit(path, b"c").unwrap();

        let snaps = history.snapshots_for(path);
        assert_eq!(snaps.len(), 3);
        assert_eq!(snaps[0].version, 1);
        assert_eq!(snaps[2].version, 3);
    }

    #[test]
    fn lru_caps_total_snapshots() {
        let (_dir, mut history) = temp_history();
        let file = tempfile::NamedTempFile::new().unwrap();
        let path = file.path();

        for i in 0..(MAX_SNAPSHOTS_PER_SESSION as u32 + 5) {
            history
                .track_edit(path, format!("content {i}").as_bytes())
                .unwrap();
        }

        assert_eq!(history.len(), MAX_SNAPSHOTS_PER_SESSION);
        let snaps = history.snapshots_for(path);
        assert_eq!(snaps.first().unwrap().version, 6);
    }

    #[test]
    fn session_dir_scopes_multiple_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = FileHistory::new(dir.path(), "session-a");
        let mut b = FileHistory::new(dir.path(), "session-b");
        let file = tempfile::NamedTempFile::new().unwrap();
        let path = file.path();

        let snap_a = a.track_edit(path, b"in A").unwrap();
        let snap_b = b.track_edit(path, b"in B").unwrap();

        assert_ne!(snap_a.storage, snap_b.storage);
        assert!(
            snap_a.storage.parent().unwrap().ends_with("session-a"),
            "session-a snapshot must live in session-a/"
        );
        assert!(
            snap_b.storage.parent().unwrap().ends_with("session-b"),
            "session-b snapshot must live in session-b/"
        );
    }

    #[test]
    fn tracked_files_lists_paths_with_snapshots() {
        let (_dir, mut history) = temp_history();
        let f1 = tempfile::NamedTempFile::new().unwrap();
        let f2 = tempfile::NamedTempFile::new().unwrap();

        history.track_edit(f1.path(), b"a").unwrap();
        history.track_edit(f2.path(), b"b").unwrap();

        let mut tracked = history.tracked_files();
        tracked.sort();
        let mut expected = vec![f1.path().to_path_buf(), f2.path().to_path_buf()];
        expected.sort();
        assert_eq!(tracked, expected);
    }
}
