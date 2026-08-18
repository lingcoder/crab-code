//! Read-before-edit gate shared by the Edit and Write tools.

use std::path::Path;

use crab_core::tool::{ReadRecord, ReadStateFn, RecordReadFn, ToolOutput};

/// Digest `content` for a [`ReadRecord`]. `None` for a ranged read, whose slice
/// says nothing about the rest of the file.
pub fn content_hash(content: &str, full_file: bool) -> Option<[u8; 32]> {
    full_file.then(|| digest(content))
}

/// SHA-256 of `text`, the form file contents are retained in.
fn digest(text: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes()).into()
}

/// Verify `path` was read before being edited/overwritten and has not changed
/// on disk since. Returns `Some(error)` to block the mutation, `None` to
/// proceed. A no-op (returns `None`) when read-state tracking is not wired —
/// tests and one-off invocations leave the closures unset and edit unchecked.
pub fn check_read_before_edit(read_state: Option<&ReadStateFn>, path: &Path) -> Option<ToolOutput> {
    let read_state = read_state?;
    match read_state(path) {
        None => Some(ToolOutput::error(
            "File has not been read yet. Read it first before editing it.",
        )),
        Some(record) => {
            if let Some(read_mtime) = record.mtime
                && let Ok(meta) = std::fs::metadata(path)
                && let Ok(disk_mtime) = meta.modified()
                && disk_mtime > read_mtime
            {
                // A newer mtime is only evidence of a write, not of a change.
                // When the read captured the whole file, compare the bytes: a
                // formatter that reproduced the file verbatim leaves the read
                // still valid, and blocking there would send the model back to
                // re-read identical content.
                if let Some(read_hash) = record.content_hash
                    && std::fs::read_to_string(path).is_ok_and(|disk| digest(&disk) == read_hash)
                {
                    return None;
                }
                return Some(ToolOutput::error(
                    "File has been modified since it was read, either by the user or by a \
                     linter. Read it again before editing it.",
                ));
            }
            None
        }
    }
}

/// Record a fresh read of `path` after a successful write, so a follow-up edit
/// in the same turn is not blocked and the staleness baseline tracks the new
/// contents.
pub fn record_after_write(record_read: Option<&RecordReadFn>, path: &Path) {
    if let Some(record_read) = record_read {
        let mtime = std::fs::metadata(path).ok().and_then(|m| m.modified().ok());
        let content_hash = std::fs::read_to_string(path)
            .ok()
            .and_then(|c| content_hash(&c, true));
        record_read(
            path,
            ReadRecord {
                mtime,
                content_hash,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    const SRC: &str = "fn a() {}
";

    fn state_for(record: Option<ReadRecord>) -> ReadStateFn {
        std::sync::Arc::new(move |_: &Path| record.clone())
    }

    /// An mtime from before any write in the test, so the gate always sees the
    /// on-disk file as newer.
    fn stale_mtime() -> Option<SystemTime> {
        Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1))
    }

    fn file_with(contents: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        std::fs::write(&file, contents).unwrap();
        (dir, file)
    }

    #[test]
    fn unread_file_is_blocked() {
        let state = state_for(None);
        let out = check_read_before_edit(Some(&state), Path::new("nope.rs"));
        assert!(out.is_some_and(|o| o.text().contains("has not been read yet")));
    }

    #[test]
    fn untracked_gate_allows_everything() {
        assert!(check_read_before_edit(None, Path::new("nope.rs")).is_none());
    }

    #[test]
    fn identical_content_survives_a_bumped_mtime() {
        // A formatter that rewrites a file verbatim moves the mtime without
        // changing a byte. Claude Code lets the edit through; so must we.
        let (_dir, file) = file_with(SRC);
        let state = state_for(Some(ReadRecord {
            mtime: stale_mtime(),
            content_hash: content_hash(SRC, true),
        }));
        assert!(check_read_before_edit(Some(&state), &file).is_none());
    }

    #[test]
    fn changed_content_is_blocked_despite_tracking() {
        let (_dir, file) = file_with(
            "fn b() {}
",
        );
        let state = state_for(Some(ReadRecord {
            mtime: stale_mtime(),
            content_hash: content_hash(SRC, true),
        }));
        let out = check_read_before_edit(Some(&state), &file);
        assert!(out.is_some_and(|o| o.text().contains("has been modified since it was read")));
    }

    #[test]
    fn ranged_read_falls_back_to_mtime_only() {
        // No digest, so a bumped mtime blocks even though the bytes match.
        let (_dir, file) = file_with(SRC);
        let state = state_for(Some(ReadRecord {
            mtime: stale_mtime(),
            content_hash: None,
        }));
        assert!(check_read_before_edit(Some(&state), &file).is_some());
    }

    #[test]
    fn content_hash_tracks_only_whole_file_reads() {
        assert_eq!(content_hash("abc", true), Some(digest("abc")));
        assert_eq!(content_hash("abc", false), None);
        assert_ne!(content_hash("abc", true), content_hash("abd", true));
    }

    #[test]
    fn tracking_cost_is_flat_in_file_size() {
        // The point of storing a digest rather than the text: a 10 MB file
        // costs the same 32 bytes as a 10 B one, so the read-state map — which
        // lives as long as the session and never evicts — stays bounded.
        let big = "x".repeat(10 * 1024 * 1024);
        assert_eq!(content_hash(&big, true).map(|h| h.len()), Some(32));
    }
}
