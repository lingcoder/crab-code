//! Read-before-edit gate shared by the Edit and Write tools.

use std::path::Path;

use crab_core::tool::{ReadRecord, ReadStateFn, RecordReadFn, ToolOutput};

/// Upper bound on file text retained in a [`ReadRecord`] for the
/// content-equality check below. Past this, the record keeps only the mtime and
/// the gate degrades to the stricter mtime-only comparison — a session that
/// reads many large files should not pin all of them in memory.
pub const MAX_TRACKED_READ_BYTES: usize = 1024 * 1024;

/// Snapshot `content` for a [`ReadRecord`], dropping it when the read was
/// ranged (`full_file` is false) or the text exceeds
/// [`MAX_TRACKED_READ_BYTES`].
pub fn trackable_content(content: &str, full_file: bool) -> Option<String> {
    (full_file && content.len() <= MAX_TRACKED_READ_BYTES).then(|| content.to_owned())
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
                if let Some(read_content) = &record.content
                    && std::fs::read_to_string(path).is_ok_and(|disk| disk == *read_content)
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
        let content = std::fs::read_to_string(path)
            .ok()
            .and_then(|c| trackable_content(&c, true));
        record_read(path, ReadRecord { mtime, content });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn state_for(record: Option<ReadRecord>) -> ReadStateFn {
        std::sync::Arc::new(move |_: &Path| record.clone())
    }

    /// An mtime from before any write in the test, so the gate always sees the
    /// on-disk file as newer.
    fn stale_mtime() -> Option<SystemTime> {
        Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1))
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
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "fn a() {}\n").unwrap();

        let state = state_for(Some(ReadRecord {
            mtime: stale_mtime(),
            content: Some("fn a() {}\n".to_owned()),
        }));
        assert!(check_read_before_edit(Some(&state), &file).is_none());
    }

    #[test]
    fn changed_content_is_blocked_despite_tracking() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "fn b() {}\n").unwrap();

        let state = state_for(Some(ReadRecord {
            mtime: stale_mtime(),
            content: Some("fn a() {}\n".to_owned()),
        }));
        let out = check_read_before_edit(Some(&state), &file);
        assert!(out.is_some_and(|o| o.text().contains("has been modified since it was read")));
    }

    #[test]
    fn ranged_read_falls_back_to_mtime_only() {
        // No tracked content, so a bumped mtime blocks even if the bytes match.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "fn a() {}\n").unwrap();

        let state = state_for(Some(ReadRecord {
            mtime: stale_mtime(),
            content: None,
        }));
        assert!(check_read_before_edit(Some(&state), &file).is_some());
    }

    #[test]
    fn trackable_content_drops_ranged_and_oversized_reads() {
        assert_eq!(trackable_content("abc", true), Some("abc".to_owned()));
        assert_eq!(trackable_content("abc", false), None);
        let big = "x".repeat(MAX_TRACKED_READ_BYTES + 1);
        assert_eq!(trackable_content(&big, true), None);
    }
}
