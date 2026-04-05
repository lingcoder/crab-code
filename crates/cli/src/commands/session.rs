//! Session management subcommands: list, show, resume, delete.
//!
//! Uses [`SessionHistory`] from `crab-session` to persist and query
//! conversation transcripts stored in `~/.crab/sessions/`.

use std::path::PathBuf;

use crab_session::SessionHistory;

/// Resolve the sessions directory (`~/.crab/sessions/`).
fn sessions_dir() -> PathBuf {
    crab_config::settings::global_config_dir().join("sessions")
}

/// List all saved session IDs.
pub fn list_sessions() -> anyhow::Result<()> {
    let history = SessionHistory::new(sessions_dir());
    let sessions = history.list_sessions()?;

    if sessions.is_empty() {
        eprintln!("No saved sessions.");
        return Ok(());
    }

    eprintln!("Saved sessions ({}):", sessions.len());
    for id in &sessions {
        // Try to load first message for a preview
        let preview = match history.load(id) {
            Ok(Some(msgs)) if !msgs.is_empty() => {
                let text = msgs[0].text();
                if text.len() > 80 {
                    format!("{}...", &text[..80])
                } else {
                    text.clone()
                }
            }
            _ => String::new(),
        };

        if preview.is_empty() {
            println!("  {id}");
        } else {
            println!("  {id}  {preview}");
        }
    }

    Ok(())
}

/// Show the transcript of a saved session.
pub fn show_session(session_id: &str) -> anyhow::Result<()> {
    let history = SessionHistory::new(sessions_dir());
    let messages = history.load(session_id)?;

    let Some(messages) = messages else {
        anyhow::bail!("Session '{session_id}' not found.");
    };

    if messages.is_empty() {
        eprintln!("Session '{session_id}' has no messages.");
        return Ok(());
    }

    eprintln!("Session: {session_id} ({} messages)\n", messages.len());

    for msg in &messages {
        let role = &msg.role;
        let text = msg.text();
        let truncated = if text.len() > 2000 {
            format!("{}... [truncated]", &text[..2000])
        } else {
            text.clone()
        };
        println!("[{role}]\n{truncated}\n");
    }

    Ok(())
}

/// Delete a saved session.
pub fn delete_session(session_id: &str) -> anyhow::Result<()> {
    let history = SessionHistory::new(sessions_dir());

    // Verify it exists
    let loaded = history.load(session_id)?;
    if loaded.is_none() {
        anyhow::bail!("Session '{session_id}' not found.");
    }

    history.delete(session_id)?;
    eprintln!("Deleted session '{session_id}'.");
    Ok(())
}

/// Return the session ID for resuming (validates it exists).
pub fn validate_resume_id(session_id: &str) -> anyhow::Result<String> {
    let history = SessionHistory::new(sessions_dir());
    let loaded = history.load(session_id)?;

    if loaded.is_none() {
        anyhow::bail!(
            "Session '{session_id}' not found. Use `crab session list` to see available sessions."
        );
    }

    Ok(session_id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::message::Message;

    fn make_history(name: &str) -> (PathBuf, SessionHistory) {
        let dir = std::env::temp_dir()
            .join("crab_cli_session_test")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        let sessions = dir.join("sessions");
        let history = SessionHistory::new(sessions);
        (dir, history)
    }

    #[test]
    fn sessions_dir_returns_path() {
        let dir = sessions_dir();
        assert!(dir.ends_with("sessions"));
    }

    #[test]
    fn list_sessions_empty() {
        let (_dir, history) = make_history("list_empty");
        let sessions = history.list_sessions().unwrap();
        assert!(sessions.is_empty());
    }

    #[test]
    fn list_sessions_with_entries() {
        let (_dir, history) = make_history("list_entries");
        history.save("sess-a", &[Message::user("Hello")]).unwrap();
        history.save("sess-b", &[Message::user("World")]).unwrap();
        let sessions = history.list_sessions().unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0], "sess-a");
        assert_eq!(sessions[1], "sess-b");
    }

    #[test]
    fn load_and_show_session() {
        let (_dir, history) = make_history("load_show");
        let messages = vec![
            Message::user("How do I fix this?"),
            Message::assistant("Let me take a look."),
        ];
        history.save("sess-show", &messages).unwrap();

        let loaded = history.load("sess-show").unwrap().unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].text(), "How do I fix this?");
    }

    #[test]
    fn delete_session_removes_file() {
        let (_dir, history) = make_history("delete");
        history.save("sess-del", &[Message::user("temp")]).unwrap();
        assert!(history.load("sess-del").unwrap().is_some());

        history.delete("sess-del").unwrap();
        assert!(history.load("sess-del").unwrap().is_none());
    }

    #[test]
    fn load_nonexistent_returns_none() {
        let (_dir, history) = make_history("nonexistent");
        assert!(history.load("nonexistent").unwrap().is_none());
    }

    #[test]
    fn session_preview_truncation() {
        let long_text = "a".repeat(200);
        let truncated = if long_text.len() > 80 {
            format!("{}...", &long_text[..80])
        } else {
            long_text.clone()
        };
        assert_eq!(truncated.len(), 83); // 80 + "..."
    }
}
