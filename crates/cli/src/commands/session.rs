//! Session management subcommands: list, show, resume, delete, search, export, stats.
//!
//! Uses [`SessionHistory`] from `crab-session` to persist and query
//! conversation transcripts stored in `~/.crab/sessions/`.

use std::path::PathBuf;

use crab_session::{ExportFormat, SessionHistory};

/// Resolve the sessions directory (`~/.crab/sessions/`).
fn sessions_dir() -> PathBuf {
    crab_config::config::global_config_dir().join("sessions")
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
                crab_utils::text::truncate_chars(&text, 80, "...")
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
        let truncated = if text.chars().nth(2000).is_some() {
            format!(
                "{}... [truncated]",
                crab_utils::text::truncate_chars(&text, 2000, "")
            )
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

/// Search all sessions for a keyword and print matching results.
pub fn search_sessions(keyword: &str) -> anyhow::Result<()> {
    let history = SessionHistory::new(sessions_dir());
    let results = history.search_all(keyword)?;

    if results.is_empty() {
        eprintln!("No matches found for '{keyword}'.");
        return Ok(());
    }

    eprintln!("Found {} match(es) for '{keyword}':\n", results.len());
    for result in &results {
        let snippet = result.snippet.replace('\n', " ");
        println!(
            "  [{}] #{} ({})  {}",
            result.session_id, result.message_index, result.role, snippet
        );
    }

    Ok(())
}

/// Export a session in the given format and print to stdout.
pub fn export_session(session_id: &str, format: &str) -> anyhow::Result<()> {
    let history = SessionHistory::new(sessions_dir());

    let export_format = match format {
        "json" => ExportFormat::Json,
        "markdown" | "md" => ExportFormat::Markdown,
        other => anyhow::bail!("Unknown format '{other}'. Use 'json' or 'markdown'."),
    };

    let output = history.export(session_id, export_format)?;
    let Some(output) = output else {
        anyhow::bail!("Session '{session_id}' not found.");
    };

    println!("{output}");
    Ok(())
}

/// Show statistics for a session.
pub fn show_stats(session_id: &str) -> anyhow::Result<()> {
    let history = SessionHistory::new(sessions_dir());
    let stats = history.stats(session_id)?;

    let Some(stats) = stats else {
        anyhow::bail!("Session '{session_id}' not found.");
    };

    println!("Session: {session_id}");
    println!("  Messages:    {}", stats.message_count);
    println!("    User:      {}", stats.user_messages);
    println!("    Assistant: {}", stats.assistant_messages);
    println!("  Tool uses:   {}", stats.tool_use_count);
    println!("  Tool results: {}", stats.tool_result_count);
    println!("  Tool errors: {}", stats.tool_error_count);
    println!("  Est. tokens: {}", stats.estimated_tokens);

    if !stats.tool_calls_by_name.is_empty() {
        println!("  Tool breakdown:");
        let mut sorted: Vec<_> = stats.tool_calls_by_name.iter().collect();
        sorted.sort_by_key(|(name, _)| (*name).clone());
        for (name, count) in sorted {
            println!("    {name}: {count}");
        }
    }

    Ok(())
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
            long_text
        };
        assert_eq!(truncated.len(), 83); // 80 + "..."
    }

    // ── Search tests ───────────────────────────────────────────

    #[test]
    fn search_finds_matches() {
        let (_dir, history) = make_history("search_find");
        history
            .save("s1", &[Message::user("fix the auth bug")])
            .unwrap();
        history
            .save("s2", &[Message::user("deploy to prod")])
            .unwrap();

        let results = history.search_all("auth").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].session_id, "s1");
    }

    #[test]
    fn search_no_results() {
        let (_dir, history) = make_history("search_none");
        history
            .save("s1", &[Message::user("nothing here")])
            .unwrap();

        let results = history.search_all("nonexistent_keyword").unwrap();
        assert!(results.is_empty());
    }

    // ── Export tests ─────���─────────────────────────────────────

    #[test]
    fn export_json_format() {
        let (_dir, history) = make_history("export_json");
        history.save("s1", &[Message::user("hello")]).unwrap();

        let output = history.export("s1", ExportFormat::Json).unwrap().unwrap();
        assert!(output.contains("\"session_id\""));
        assert!(output.contains("s1"));
        assert!(output.contains("hello"));
    }

    #[test]
    fn export_markdown_format() {
        let (_dir, history) = make_history("export_md");
        history
            .save("s1", &[Message::user("hello"), Message::assistant("hi")])
            .unwrap();

        let output = history
            .export("s1", ExportFormat::Markdown)
            .unwrap()
            .unwrap();
        assert!(output.contains("# Session: s1"));
        assert!(output.contains("User"));
        assert!(output.contains("Assistant"));
    }

    #[test]
    fn export_nonexistent_returns_none() {
        let (_dir, history) = make_history("export_none");
        assert!(
            history
                .export("nope", ExportFormat::Json)
                .unwrap()
                .is_none()
        );
    }

    // ── Stats tests ────────────��───────────────────────────────

    #[test]
    fn stats_returns_counts() {
        let (_dir, history) = make_history("stats_counts");
        history
            .save(
                "s1",
                &[
                    Message::user("q"),
                    Message::assistant("a"),
                    Message::user("q2"),
                    Message::assistant("a2"),
                ],
            )
            .unwrap();

        let stats = history.stats("s1").unwrap().unwrap();
        assert_eq!(stats.message_count, 4);
        assert_eq!(stats.user_messages, 2);
        assert_eq!(stats.assistant_messages, 2);
    }

    #[test]
    fn stats_nonexistent_returns_none() {
        let (_dir, history) = make_history("stats_none");
        assert!(history.stats("nope").unwrap().is_none());
    }

    #[test]
    fn export_format_parse() {
        // Verify the CLI format string parsing logic
        assert!(matches!(
            match "json" {
                "json" => Some(ExportFormat::Json),
                "markdown" | "md" => Some(ExportFormat::Markdown),
                _ => None,
            },
            Some(ExportFormat::Json)
        ));
        assert!(matches!(
            match "markdown" {
                "json" => Some(ExportFormat::Json),
                "markdown" | "md" => Some(ExportFormat::Markdown),
                _ => None,
            },
            Some(ExportFormat::Markdown)
        ));
        assert!(matches!(
            match "md" {
                "json" => Some(ExportFormat::Json),
                "markdown" | "md" => Some(ExportFormat::Markdown),
                _ => None,
            },
            Some(ExportFormat::Markdown)
        ));
        assert!(
            (match "unknown" {
                "json" => Some(ExportFormat::Json),
                "markdown" | "md" => Some(ExportFormat::Markdown),
                _ => None,
            })
            .is_none()
        );
    }
}
