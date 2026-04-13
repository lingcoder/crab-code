//! Prompt injection — turns the current IDE state into
//! `<system-reminder>` meta-user messages for LLM context.
//!
//! Called by `crab-agent` right before sending a user prompt. The
//! output string (wrapped by the caller) follows the upstream template
//! so LLMs trained against that prompt pattern recognize the shape.

#![allow(dead_code)] // R1 scaffolding; wired into agent in R3

use crab_core::ide::IdeSelection;
use std::path::Path;

/// Build the "selected lines" system-reminder body, if applicable.
///
/// Returns `None` when there is nothing to inject (no selection or
/// missing fields). Caller wraps it in `<system-reminder>...</system-reminder>`
/// and pushes as a `user`-role meta message.
pub fn build_selected_lines_reminder(sel: &IdeSelection, _cwd: &Path) -> Option<String> {
    if !sel.has_text() {
        return None;
    }
    let file = sel.file_path.as_deref()?;
    let start = sel.line_start?;
    let end = sel.line_end()?;
    let text = sel.text.as_deref()?;
    // R3: relativize `file` against `_cwd` for prettier display.
    Some(format!(
        "The user selected the lines {start} to {end} from {display}:\n\
         {text}\n\n\
         This may or may not be related to the current task.",
        display = file.display(),
    ))
}

/// Build the "opened file" system-reminder body (used when no text is
/// selected but a file is active).
pub fn build_opened_file_reminder(sel: &IdeSelection) -> Option<String> {
    if sel.has_text() {
        return None;
    }
    let file = sel.file_path.as_deref()?;
    Some(format!(
        "The user currently has {display} open in their editor.",
        display = file.display(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn builds_selected_lines_text() {
        let sel = IdeSelection {
            line_count: 3,
            line_start: Some(10),
            text: Some("fn main() {}".to_string()),
            file_path: Some(PathBuf::from("/work/foo.rs")),
        };
        let out = build_selected_lines_reminder(&sel, Path::new("/work")).unwrap();
        assert!(out.contains("lines 10 to 12"));
        assert!(out.contains("fn main() {}"));
        assert!(out.contains("foo.rs"));
        assert!(out.contains("This may or may not be related"));
    }

    #[test]
    fn returns_none_when_no_selection() {
        let sel = IdeSelection::default();
        assert!(build_selected_lines_reminder(&sel, Path::new("/")).is_none());
    }

    #[test]
    fn opened_file_triggers_only_without_selection() {
        let sel = IdeSelection {
            line_count: 0,
            file_path: Some(PathBuf::from("/work/foo.rs")),
            ..Default::default()
        };
        assert!(build_opened_file_reminder(&sel).is_some());

        let sel_with_text = IdeSelection {
            line_count: 2,
            line_start: Some(1),
            text: Some("x".to_string()),
            file_path: Some(PathBuf::from("/work/foo.rs")),
        };
        assert!(build_opened_file_reminder(&sel_with_text).is_none());
    }
}
