use std::path::PathBuf;

use crab_core::message::Message;
use serde::{Deserialize, Serialize};

/// On-disk session transcript format.
#[derive(Debug, Serialize, Deserialize)]
struct SessionFile {
    session_id: String,
    messages: Vec<Message>,
}

/// Persists and recovers session transcripts from disk.
///
/// Each session is stored as `{base_dir}/{session_id}.json`.
pub struct SessionHistory {
    pub base_dir: PathBuf,
}

impl SessionHistory {
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    /// Ensure the base directory exists.
    fn ensure_dir(&self) -> crab_common::Result<()> {
        std::fs::create_dir_all(&self.base_dir)?;
        Ok(())
    }

    /// Path to a session file.
    fn session_path(&self, session_id: &str) -> PathBuf {
        self.base_dir.join(format!("{session_id}.json"))
    }

    /// Save a session transcript to disk.
    pub fn save(&self, session_id: &str, messages: &[Message]) -> crab_common::Result<()> {
        self.ensure_dir()?;
        let file = SessionFile {
            session_id: session_id.to_string(),
            messages: messages.to_vec(),
        };
        let json = serde_json::to_string_pretty(&file)
            .map_err(|e| crab_common::Error::Other(format!("serialize session: {e}")))?;
        std::fs::write(self.session_path(session_id), json)?;
        Ok(())
    }

    /// Load a session transcript from disk. Returns `None` if the file doesn't exist.
    pub fn load(&self, session_id: &str) -> crab_common::Result<Option<Vec<Message>>> {
        let path = self.session_path(session_id);
        if !path.exists() {
            return Ok(None);
        }
        let data = std::fs::read_to_string(&path)?;
        let file: SessionFile = serde_json::from_str(&data)
            .map_err(|e| crab_common::Error::Other(format!("parse session: {e}")))?;
        Ok(Some(file.messages))
    }

    /// List all saved session IDs (sorted by name).
    pub fn list_sessions(&self) -> crab_common::Result<Vec<String>> {
        if !self.base_dir.exists() {
            return Ok(Vec::new());
        }
        let mut sessions = Vec::new();
        for entry in std::fs::read_dir(&self.base_dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(id) = name.strip_suffix(".json") {
                sessions.push(id.to_string());
            }
        }
        sessions.sort();
        Ok(sessions)
    }

    /// Delete a session file.
    pub fn delete(&self, session_id: &str) -> crab_common::Result<()> {
        let path = self.session_path(session_id);
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::message::Message;

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());

        let messages = vec![Message::user("Hello"), Message::assistant("Hi there!")];
        history.save("test-session", &messages).unwrap();

        let loaded = history.load("test-session").unwrap().unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].text(), "Hello");
        assert_eq!(loaded[1].text(), "Hi there!");
    }

    #[test]
    fn load_nonexistent_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());
        assert!(history.load("nope").unwrap().is_none());
    }

    #[test]
    fn list_sessions_empty() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().join("sessions"));
        assert!(history.list_sessions().unwrap().is_empty());
    }

    #[test]
    fn list_sessions_returns_ids() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());

        history.save("session-b", &[Message::user("b")]).unwrap();
        history.save("session-a", &[Message::user("a")]).unwrap();

        let sessions = history.list_sessions().unwrap();
        assert_eq!(sessions, vec!["session-a", "session-b"]);
    }

    #[test]
    fn delete_session() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());

        history.save("to-delete", &[Message::user("x")]).unwrap();
        assert!(history.load("to-delete").unwrap().is_some());

        history.delete("to-delete").unwrap();
        assert!(history.load("to-delete").unwrap().is_none());
    }

    #[test]
    fn delete_nonexistent_is_ok() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());
        history.delete("nope").unwrap(); // should not error
    }

    #[test]
    fn save_overwrites_existing_session() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());

        history.save("sess", &[Message::user("original")]).unwrap();
        history
            .save(
                "sess",
                &[Message::user("updated"), Message::assistant("ok")],
            )
            .unwrap();

        let loaded = history.load("sess").unwrap().unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].text(), "updated");
    }

    #[test]
    fn save_empty_messages() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());
        history.save("empty-session", &[]).unwrap();
        let loaded = history.load("empty-session").unwrap().unwrap();
        assert!(loaded.is_empty());
    }

    #[test]
    fn list_sessions_ignores_non_json_files() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());

        history
            .save("valid-session", &[Message::user("hi")])
            .unwrap();
        // Create non-json file
        std::fs::write(dir.path().join("notes.txt"), "not a session").unwrap();

        let sessions = history.list_sessions().unwrap();
        assert_eq!(sessions, vec!["valid-session"]);
    }

    #[test]
    fn list_sessions_nonexistent_dir_returns_empty() {
        let history = SessionHistory::new(std::path::PathBuf::from("/nonexistent/sessions"));
        let sessions = history.list_sessions().unwrap();
        assert!(sessions.is_empty());
    }

    #[test]
    fn save_creates_directory_if_missing() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("deep").join("sessions");
        let history = SessionHistory::new(nested.clone());

        history.save("sess", &[Message::user("test")]).unwrap();
        assert!(nested.join("sess.json").exists());
    }

    #[test]
    fn load_corrupt_json_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bad.json"), "not valid json").unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());
        let result = history.load("bad");
        assert!(result.is_err());
    }

    #[test]
    fn multiple_sessions_sorted_by_id() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());

        history.save("z-session", &[Message::user("z")]).unwrap();
        history.save("a-session", &[Message::user("a")]).unwrap();
        history.save("m-session", &[Message::user("m")]).unwrap();

        let sessions = history.list_sessions().unwrap();
        assert_eq!(sessions, vec!["a-session", "m-session", "z-session"]);
    }
}
