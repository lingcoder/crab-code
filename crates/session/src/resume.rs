use std::path::Path;

use crate::history::SessionHistory;

/// Find the session to resume based on CLI flags.
///
/// When `continue_session` is true, looks up the most recent session for
/// `working_dir`. Prints a message to stderr when no previous session is found.
///
/// Returns `None` when `continue_session` is false or no session exists.
pub fn find_resume_session(
    sessions_dir: &Path,
    continue_session: bool,
    working_dir: &Path,
) -> Option<String> {
    if !continue_session {
        return None;
    }
    let history = SessionHistory::new(sessions_dir.to_path_buf());
    let found = history.find_latest_for_dir(working_dir);
    if found.is_none() {
        eprintln!("No previous session found to continue.");
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_resume_session_returns_none_when_not_continuing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(find_resume_session(dir.path(), false, dir.path()).is_none());
    }

    #[test]
    fn find_resume_session_returns_none_for_empty_history() {
        let dir = tempfile::tempdir().unwrap();
        assert!(find_resume_session(dir.path(), true, dir.path()).is_none());
    }
}
