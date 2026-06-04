//! Read-before-edit gate shared by the Edit and Write tools.

use std::path::Path;

use crab_core::tool::{ReadRecord, ReadStateFn, RecordReadFn, ToolOutput};

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
        record_read(path, ReadRecord { mtime });
    }
}
