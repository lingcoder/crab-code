use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use crab_core::message::{ContentBlock, Message, Role};
use serde::{Deserialize, Serialize};

/// On-disk session transcript format.
#[derive(Debug, Serialize, Deserialize)]
struct SessionFile {
    session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    working_dir: Option<String>,
    messages: Vec<Message>,
}

/// Metadata for a saved session (returned by list operations).
#[derive(Debug, Clone)]
pub struct SessionMetadata {
    pub session_id: String,
    pub name: Option<String>,
    pub working_dir: Option<String>,
    pub message_count: usize,
    pub modified: Option<std::time::SystemTime>,
}

/// A search hit within a session.
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// The session ID containing the match.
    pub session_id: String,
    /// Zero-based index of the matching message.
    pub message_index: usize,
    /// Role of the matching message.
    pub role: Role,
    /// The matched text snippet (first matching content block text).
    pub snippet: String,
}

/// Statistics for a single session.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SessionStats {
    /// Total number of messages.
    pub message_count: usize,
    /// Number of user messages.
    pub user_messages: usize,
    /// Number of assistant messages.
    pub assistant_messages: usize,
    /// Number of tool use invocations.
    pub tool_use_count: usize,
    /// Number of tool results.
    pub tool_result_count: usize,
    /// Number of error tool results.
    pub tool_error_count: usize,
    /// Per-tool invocation counts (`tool_name` -> count).
    pub tool_calls_by_name: HashMap<String, usize>,
    /// Estimated total tokens (rough heuristic).
    pub estimated_tokens: u64,
}

/// Export format for sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    /// Pretty-printed JSON (same as on-disk format).
    Json,
    /// Human-readable Markdown transcript.
    Markdown,
}

/// Callback for auto-persisting messages during the query loop.
///
/// Implementors should write the message to durable storage (e.g. JSONL
/// append) so that crash recovery can reconstruct the conversation.
pub trait SessionPersister: Send + Sync {
    fn persist_message(&self, msg: &Message);
}

/// Persists and recovers session transcripts from disk.
///
/// Each session is stored as `{base_dir}/{session_id}.json`.
/// JSONL transcripts (one message per line) are stored alongside as
/// `{base_dir}/{session_id}.jsonl` for crash-resilient per-turn saving.
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
        self.save_with_metadata(session_id, None, None, messages)
    }

    /// Save a session with optional name and working directory metadata.
    pub fn save_with_metadata(
        &self,
        session_id: &str,
        name: Option<&str>,
        working_dir: Option<&str>,
        messages: &[Message],
    ) -> crab_common::Result<()> {
        self.ensure_dir()?;
        let file = SessionFile {
            session_id: session_id.to_string(),
            name: name.map(std::string::ToString::to_string),
            working_dir: working_dir.map(std::string::ToString::to_string),
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

    /// List all sessions with metadata (name, `working_dir`, message count, mtime).
    /// Sorted by modification time (newest first).
    pub fn list_sessions_with_metadata(&self) -> crab_common::Result<Vec<SessionMetadata>> {
        if !self.base_dir.exists() {
            return Ok(Vec::new());
        }
        let mut results = Vec::new();
        for entry in std::fs::read_dir(&self.base_dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(id) = name.strip_suffix(".json") {
                let modified = entry.metadata().ok().and_then(|m| m.modified().ok());
                // Read the file to extract metadata
                let (session_name, working_dir, message_count) =
                    std::fs::read_to_string(entry.path())
                        .ok()
                        .and_then(|data| serde_json::from_str::<SessionFile>(&data).ok())
                        .map_or((None, None, 0), |file| {
                            (file.name, file.working_dir, file.messages.len())
                        });
                results.push(SessionMetadata {
                    session_id: id.to_string(),
                    name: session_name,
                    working_dir,
                    message_count,
                    modified,
                });
            }
        }
        // Sort by mtime, newest first
        results.sort_by_key(|r| std::cmp::Reverse(r.modified));
        Ok(results)
    }

    /// Delete a session file.
    pub fn delete(&self, session_id: &str) -> crab_common::Result<()> {
        let path = self.session_path(session_id);
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }

    /// Find the most recent session (by file modification time).
    ///
    /// This is used by the `-c` / `--continue` flag to resume the latest
    /// session. The `_dir` parameter is accepted for future use when sessions
    /// store their working directory; currently it is unused and the most
    /// recent session file (by mtime) is returned regardless.
    pub fn find_latest_for_dir(&self, _dir: &std::path::Path) -> Option<String> {
        if !self.base_dir.exists() {
            return None;
        }
        let mut best: Option<(String, std::time::SystemTime)> = None;
        if let Ok(entries) = std::fs::read_dir(&self.base_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if let Some(id) = name.strip_suffix(".json")
                    && let Ok(meta) = entry.metadata()
                    && let Ok(mtime) = meta.modified()
                    && best.as_ref().is_none_or(|(_, t)| mtime > *t)
                {
                    best = Some((id.to_string(), mtime));
                }
            }
        }
        best.map(|(id, _)| id)
    }

    // ── Search ─────────────────────────────────────────────────────

    /// Search a single session's messages for a keyword (case-insensitive).
    /// Returns matching results with message index and snippet.
    pub fn search_session(
        &self,
        session_id: &str,
        query: &str,
    ) -> crab_common::Result<Vec<SearchResult>> {
        let Some(messages) = self.load(session_id)? else {
            return Ok(Vec::new());
        };
        Ok(find_matches(session_id, &messages, query))
    }

    /// Search all sessions for a keyword (case-insensitive).
    /// Returns matches across all sessions, ordered by session ID.
    pub fn search_all(&self, query: &str) -> crab_common::Result<Vec<SearchResult>> {
        let session_ids = self.list_sessions()?;
        let mut all_results = Vec::new();
        for sid in &session_ids {
            let results = self.search_session(sid, query)?;
            all_results.extend(results);
        }
        Ok(all_results)
    }

    // ── Export ──────────────────────────────────────────────────────

    /// Export a session in the given format.
    /// Returns `None` if the session doesn't exist.
    pub fn export(
        &self,
        session_id: &str,
        format: ExportFormat,
    ) -> crab_common::Result<Option<String>> {
        let Some(messages) = self.load(session_id)? else {
            return Ok(None);
        };
        let output = match format {
            ExportFormat::Json => {
                let file = SessionFile {
                    session_id: session_id.to_string(),
                    name: None,
                    working_dir: None,
                    messages,
                };
                serde_json::to_string_pretty(&file)
                    .map_err(|e| crab_common::Error::Other(format!("export json: {e}")))?
            }
            ExportFormat::Markdown => export_markdown(session_id, &messages),
        };
        Ok(Some(output))
    }

    // ── Statistics ──────────────────────────────────────────────────

    /// Compute statistics for a single session.
    /// Returns `None` if the session doesn't exist.
    pub fn stats(&self, session_id: &str) -> crab_common::Result<Option<SessionStats>> {
        let Some(messages) = self.load(session_id)? else {
            return Ok(None);
        };
        Ok(Some(compute_stats(&messages)))
    }

    // ── JSONL per-turn persistence ─────────────────────────────────

    fn jsonl_path(&self, session_id: &str) -> PathBuf {
        self.base_dir.join(format!("{session_id}.jsonl"))
    }

    /// Append a single message as one JSONL line (crash-resilient).
    pub fn append_jsonl(&self, session_id: &str, msg: &Message) -> crab_common::Result<()> {
        self.ensure_dir()?;
        let mut line = serde_json::to_string(msg)
            .map_err(|e| crab_common::Error::Other(format!("serialize message: {e}")))?;
        line.push('\n');

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.jsonl_path(session_id))?;
        file.write_all(line.as_bytes())?;
        Ok(())
    }

    /// Load messages from a JSONL transcript. Returns `None` if the file
    /// doesn't exist. Silently skips malformed lines (partial writes from
    /// crashes).
    pub fn load_jsonl(&self, session_id: &str) -> crab_common::Result<Option<Vec<Message>>> {
        let path = self.jsonl_path(session_id);
        if !path.exists() {
            return Ok(None);
        }
        let data = std::fs::read_to_string(&path)?;
        let messages: Vec<Message> = data
            .lines()
            .filter(|l| !l.is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        Ok(Some(messages))
    }

    /// Create a [`SessionPersister`] bound to a specific session ID.
    pub fn persister(self: &Arc<Self>, session_id: String) -> BoundSessionPersister {
        BoundSessionPersister {
            history: Arc::clone(self),
            session_id,
        }
    }
}

/// A [`SessionPersister`] bound to one session, suitable for passing into
/// the query loop.
pub struct BoundSessionPersister {
    history: Arc<SessionHistory>,
    session_id: String,
}

impl SessionPersister for BoundSessionPersister {
    fn persist_message(&self, msg: &Message) {
        if let Err(e) = self.history.append_jsonl(&self.session_id, msg) {
            eprintln!("[session] failed to persist message to JSONL: {e}");
        }
    }
}

// ── Helper functions ───────────────────────────────────────────────────

/// Extract searchable text from a content block.
fn block_text(block: &ContentBlock) -> Option<&str> {
    match block {
        ContentBlock::Text { text } => Some(text),
        ContentBlock::ToolResult { content, .. } => Some(content),
        ContentBlock::ToolUse { name, .. } => Some(name),
        ContentBlock::Thinking { thinking } => Some(thinking),
        ContentBlock::Image { .. } => None,
    }
}

/// Find messages matching `query` (case-insensitive) within a message list.
fn find_matches(session_id: &str, messages: &[Message], query: &str) -> Vec<SearchResult> {
    let query_lower = query.to_lowercase();
    let mut results = Vec::new();

    for (idx, msg) in messages.iter().enumerate() {
        for block in &msg.content {
            if let Some(text) = block_text(block)
                && text.to_lowercase().contains(&query_lower)
            {
                // Take a snippet: first 120 chars of the matching text
                let snippet = if text.len() > 120 {
                    format!("{}...", &text[..120])
                } else {
                    text.to_string()
                };
                results.push(SearchResult {
                    session_id: session_id.to_string(),
                    message_index: idx,
                    role: msg.role,
                    snippet,
                });
                break; // one hit per message is enough
            }
        }
    }

    results
}

/// Render messages as a Markdown transcript.
fn export_markdown(session_id: &str, messages: &[Message]) -> String {
    use std::fmt::Write;

    let mut md = String::new();
    let _ = writeln!(md, "# Session: {session_id}\n");

    for (i, msg) in messages.iter().enumerate() {
        let role_label = match msg.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
            Role::System => "System",
        };
        let _ = writeln!(md, "## [{i}] {role_label}\n");

        for block in &msg.content {
            match block {
                ContentBlock::Text { text } => {
                    let _ = writeln!(md, "{text}\n");
                }
                ContentBlock::ToolUse { id, name, input } => {
                    let _ = writeln!(md, "**Tool Use:** `{name}` (id: `{id}`)\n");
                    let _ = writeln!(
                        md,
                        "```json\n{}\n```\n",
                        serde_json::to_string_pretty(input).unwrap_or_default()
                    );
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    let label = if *is_error {
                        "Tool Error"
                    } else {
                        "Tool Result"
                    };
                    let _ = writeln!(md, "**{label}** (id: `{tool_use_id}`)\n");
                    let _ = writeln!(md, "```\n{content}\n```\n");
                }
                ContentBlock::Thinking { thinking } => {
                    let _ = writeln!(md, "*[thinking]* {thinking}\n");
                }
                ContentBlock::Image { .. } => {
                    let _ = writeln!(md, "*[image]*\n");
                }
            }
        }
    }

    md
}

/// Compute statistics from a message list.
fn compute_stats(messages: &[Message]) -> SessionStats {
    let mut stats = SessionStats {
        message_count: messages.len(),
        ..SessionStats::default()
    };

    for msg in messages {
        match msg.role {
            Role::User => stats.user_messages += 1,
            Role::Assistant => stats.assistant_messages += 1,
            Role::System => {}
        }
        stats.estimated_tokens += msg.estimated_tokens();

        for block in &msg.content {
            match block {
                ContentBlock::ToolUse { name, .. } => {
                    stats.tool_use_count += 1;
                    *stats.tool_calls_by_name.entry(name.clone()).or_insert(0) += 1;
                }
                ContentBlock::ToolResult { is_error, .. } => {
                    stats.tool_result_count += 1;
                    if *is_error {
                        stats.tool_error_count += 1;
                    }
                }
                _ => {}
            }
        }
    }

    stats
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

    // ── Search tests ───────────────────────────────────────────────

    #[test]
    fn search_session_finds_keyword() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());
        history
            .save(
                "s1",
                &[
                    Message::user("fix the authentication bug"),
                    Message::assistant("I'll look into the auth module"),
                ],
            )
            .unwrap();

        let results = history.search_session("s1", "auth").unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].message_index, 0);
        assert_eq!(results[0].role, Role::User);
        assert_eq!(results[1].message_index, 1);
        assert_eq!(results[1].role, Role::Assistant);
    }

    #[test]
    fn search_session_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());
        history.save("s1", &[Message::user("Hello World")]).unwrap();

        let results = history.search_session("s1", "hello world").unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn search_session_no_match() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());
        history
            .save("s1", &[Message::user("something else")])
            .unwrap();

        let results = history.search_session("s1", "nonexistent").unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn search_session_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());
        let results = history.search_session("nope", "test").unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn search_all_across_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());
        history
            .save("s1", &[Message::user("deploy to production")])
            .unwrap();
        history
            .save("s2", &[Message::user("no match here")])
            .unwrap();
        history
            .save("s3", &[Message::assistant("deploying now")])
            .unwrap();

        let results = history.search_all("deploy").unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].session_id, "s1");
        assert_eq!(results[1].session_id, "s3");
    }

    #[test]
    fn search_matches_tool_use_name() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());
        let msg = Message::new(
            Role::Assistant,
            vec![ContentBlock::tool_use(
                "t1",
                "bash",
                serde_json::json!({"command": "ls"}),
            )],
        );
        history.save("s1", &[msg]).unwrap();

        let results = history.search_session("s1", "bash").unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn search_matches_tool_result_content() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());
        let msg = Message::tool_result("t1", "file not found: main.rs", true);
        history.save("s1", &[msg]).unwrap();

        let results = history.search_session("s1", "main.rs").unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn search_snippet_truncates_long_text() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());
        let long_text = "keyword ".repeat(50); // ~400 chars
        history.save("s1", &[Message::user(&long_text)]).unwrap();

        let results = history.search_session("s1", "keyword").unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].snippet.ends_with("..."));
        assert!(results[0].snippet.len() <= 124); // 120 + "..."
    }

    // ── Export tests ───────────────────────────────────────────────

    #[test]
    fn export_json_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());
        let msgs = vec![Message::user("Hello"), Message::assistant("Hi")];
        history.save("s1", &msgs).unwrap();

        let json_str = history.export("s1", ExportFormat::Json).unwrap().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["session_id"], "s1");
        assert_eq!(parsed["messages"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn export_markdown_has_headers() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());
        let msgs = vec![
            Message::user("What files exist?"),
            Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::text("Let me check."),
                    ContentBlock::tool_use("t1", "bash", serde_json::json!({"command": "ls"})),
                ],
            ),
            Message::tool_result("t1", "main.rs\nlib.rs", false),
        ];
        history.save("s1", &msgs).unwrap();

        let md = history
            .export("s1", ExportFormat::Markdown)
            .unwrap()
            .unwrap();
        assert!(md.contains("# Session: s1"));
        assert!(md.contains("## [0] User"));
        assert!(md.contains("## [1] Assistant"));
        assert!(md.contains("**Tool Use:** `bash`"));
        assert!(md.contains("**Tool Result**"));
        assert!(md.contains("main.rs"));
    }

    #[test]
    fn export_markdown_tool_error() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());
        let msgs = vec![Message::tool_result("t1", "command failed", true)];
        history.save("s1", &msgs).unwrap();

        let md = history
            .export("s1", ExportFormat::Markdown)
            .unwrap()
            .unwrap();
        assert!(md.contains("**Tool Error**"));
    }

    #[test]
    fn export_nonexistent_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());
        assert!(
            history
                .export("nope", ExportFormat::Json)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn export_empty_session() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());
        history.save("empty", &[]).unwrap();

        let json = history
            .export("empty", ExportFormat::Json)
            .unwrap()
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed["messages"].as_array().unwrap().is_empty());

        let md = history
            .export("empty", ExportFormat::Markdown)
            .unwrap()
            .unwrap();
        assert!(md.contains("# Session: empty"));
    }

    #[test]
    fn export_markdown_image_block() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());
        let msgs = vec![Message::new(
            Role::User,
            vec![ContentBlock::Image {
                source: crab_core::message::ImageSource::base64("image/png", "data"),
            }],
        )];
        history.save("s1", &msgs).unwrap();

        let md = history
            .export("s1", ExportFormat::Markdown)
            .unwrap()
            .unwrap();
        assert!(md.contains("*[image]*"));
    }

    // ── Stats tests ────────────────────────────────────────────────

    #[test]
    fn stats_basic_counts() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());
        let msgs = vec![
            Message::user("question"),
            Message::assistant("answer"),
            Message::user("follow up"),
            Message::assistant("more info"),
        ];
        history.save("s1", &msgs).unwrap();

        let stats = history.stats("s1").unwrap().unwrap();
        assert_eq!(stats.message_count, 4);
        assert_eq!(stats.user_messages, 2);
        assert_eq!(stats.assistant_messages, 2);
        assert_eq!(stats.tool_use_count, 0);
        assert_eq!(stats.tool_result_count, 0);
    }

    #[test]
    fn stats_tool_counts() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());
        let msgs = vec![
            Message::user("do something"),
            Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::tool_use("t1", "bash", serde_json::json!({})),
                    ContentBlock::tool_use("t2", "read_file", serde_json::json!({})),
                ],
            ),
            Message::tool_result("t1", "ok", false),
            Message::tool_result("t2", "error!", true),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::tool_use("t3", "bash", serde_json::json!({}))],
            ),
        ];
        history.save("s1", &msgs).unwrap();

        let stats = history.stats("s1").unwrap().unwrap();
        assert_eq!(stats.tool_use_count, 3);
        assert_eq!(stats.tool_result_count, 2);
        assert_eq!(stats.tool_error_count, 1);
        assert_eq!(stats.tool_calls_by_name.get("bash"), Some(&2));
        assert_eq!(stats.tool_calls_by_name.get("read_file"), Some(&1));
    }

    #[test]
    fn stats_estimated_tokens_positive() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());
        history
            .save(
                "s1",
                &[
                    Message::user("Hello world this is a test"),
                    Message::assistant("Thanks for the message"),
                ],
            )
            .unwrap();

        let stats = history.stats("s1").unwrap().unwrap();
        assert!(stats.estimated_tokens > 0);
    }

    #[test]
    fn stats_nonexistent_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());
        assert!(history.stats("nope").unwrap().is_none());
    }

    #[test]
    fn stats_empty_session() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());
        history.save("empty", &[]).unwrap();

        let stats = history.stats("empty").unwrap().unwrap();
        assert_eq!(stats.message_count, 0);
        assert_eq!(stats.user_messages, 0);
        assert_eq!(stats.tool_use_count, 0);
        assert_eq!(stats.estimated_tokens, 0);
    }

    #[test]
    fn stats_serializes_to_json() {
        let stats = SessionStats {
            message_count: 10,
            user_messages: 5,
            assistant_messages: 5,
            tool_use_count: 3,
            tool_result_count: 3,
            tool_error_count: 1,
            tool_calls_by_name: HashMap::from([("bash".into(), 2), ("read_file".into(), 1)]),
            estimated_tokens: 500,
        };
        let json = serde_json::to_string(&stats).unwrap();
        assert!(json.contains("\"message_count\":10"));
        assert!(json.contains("\"tool_error_count\":1"));
    }

    // ── Helper function tests ──────────────────────────────────────

    #[test]
    fn compute_stats_system_messages_not_counted_as_user_or_assistant() {
        let msgs = vec![
            Message::system("system prompt"),
            Message::user("hi"),
            Message::assistant("hello"),
        ];
        let stats = compute_stats(&msgs);
        assert_eq!(stats.message_count, 3);
        assert_eq!(stats.user_messages, 1);
        assert_eq!(stats.assistant_messages, 1);
    }

    #[test]
    fn find_matches_empty_messages() {
        let results = find_matches("s1", &[], "test");
        assert!(results.is_empty());
    }

    #[test]
    fn export_markdown_system_message() {
        let md = export_markdown("s1", &[Message::system("Be helpful")]);
        assert!(md.contains("## [0] System"));
        assert!(md.contains("Be helpful"));
    }

    // ── find_latest_for_dir tests ─────────────────────────────────

    #[test]
    fn find_latest_for_dir_empty_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().join("sessions"));
        assert!(
            history
                .find_latest_for_dir(std::path::Path::new("/tmp"))
                .is_none()
        );
    }

    #[test]
    fn find_latest_for_dir_returns_most_recent() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());

        history
            .save("session-old", &[Message::user("old")])
            .unwrap();
        // Small delay to ensure different mtime
        std::thread::sleep(std::time::Duration::from_millis(50));
        history
            .save("session-new", &[Message::user("new")])
            .unwrap();

        let latest = history
            .find_latest_for_dir(std::path::Path::new("/tmp"))
            .unwrap();
        assert_eq!(latest, "session-new");
    }

    // ── Session metadata tests ────────────────────────────────────

    #[test]
    fn save_with_metadata_stores_name() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());

        history
            .save_with_metadata(
                "s1",
                Some("my feature"),
                Some("/tmp/project"),
                &[Message::user("hi")],
            )
            .unwrap();

        // Load raw file to verify name is stored
        let data = std::fs::read_to_string(dir.path().join("s1.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert_eq!(parsed["name"], "my feature");
        assert_eq!(parsed["working_dir"], "/tmp/project");
    }

    #[test]
    fn save_with_metadata_none_name() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());

        history
            .save_with_metadata("s1", None, None, &[Message::user("hi")])
            .unwrap();

        let data = std::fs::read_to_string(dir.path().join("s1.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert!(parsed.get("name").is_none());
    }

    #[test]
    fn list_sessions_with_metadata_returns_names() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());

        history
            .save_with_metadata("s1", Some("bug fix"), None, &[Message::user("a")])
            .unwrap();
        history
            .save("s2", &[Message::user("b"), Message::assistant("c")])
            .unwrap();

        let metas = history.list_sessions_with_metadata().unwrap();
        assert_eq!(metas.len(), 2);

        let s1 = metas.iter().find(|m| m.session_id == "s1").unwrap();
        assert_eq!(s1.name.as_deref(), Some("bug fix"));
        assert_eq!(s1.message_count, 1);

        let s2 = metas.iter().find(|m| m.session_id == "s2").unwrap();
        assert!(s2.name.is_none());
        assert_eq!(s2.message_count, 2);
    }

    #[test]
    fn list_sessions_with_metadata_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().join("sessions"));
        assert!(history.list_sessions_with_metadata().unwrap().is_empty());
    }

    #[test]
    fn save_with_metadata_backward_compatible_load() {
        // Sessions saved with metadata can still be loaded by plain load()
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());

        history
            .save_with_metadata("s1", Some("named"), Some("/proj"), &[Message::user("test")])
            .unwrap();

        let msgs = history.load("s1").unwrap().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text(), "test");
    }

    #[test]
    fn load_legacy_session_without_name_field() {
        // Sessions saved without name/working_dir (old format) should still load
        let dir = tempfile::tempdir().unwrap();
        let legacy_json = r#"{"session_id":"legacy","messages":[{"role":"user","content":[{"type":"text","text":"hello"}]}]}"#;
        std::fs::write(dir.path().join("legacy.json"), legacy_json).unwrap();

        let history = SessionHistory::new(dir.path().to_path_buf());
        let msgs = history.load("legacy").unwrap().unwrap();
        assert_eq!(msgs.len(), 1);

        let metas = history.list_sessions_with_metadata().unwrap();
        assert_eq!(metas.len(), 1);
        assert!(metas[0].name.is_none());
    }

    // ── JSONL tests ───────────────────────────────────────────────

    #[test]
    fn jsonl_append_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());

        history.append_jsonl("s1", &Message::user("hello")).unwrap();
        history
            .append_jsonl("s1", &Message::assistant("hi"))
            .unwrap();

        let msgs = history.load_jsonl("s1").unwrap().unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].text(), "hello");
        assert_eq!(msgs[1].text(), "hi");
    }

    #[test]
    fn jsonl_load_nonexistent_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());
        assert!(history.load_jsonl("nope").unwrap().is_none());
    }

    #[test]
    fn jsonl_skips_malformed_lines() {
        let dir = tempfile::tempdir().unwrap();
        let history = SessionHistory::new(dir.path().to_path_buf());

        history.append_jsonl("s1", &Message::user("good")).unwrap();
        // Simulate a partial write (crash mid-line)
        let path = dir.path().join("s1.jsonl");
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"truncated\n")
            .unwrap();
        history
            .append_jsonl("s1", &Message::assistant("also good"))
            .unwrap();

        let msgs = history.load_jsonl("s1").unwrap().unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].text(), "good");
        assert_eq!(msgs[1].text(), "also good");
    }

    #[test]
    fn jsonl_persister_via_trait() {
        let dir = tempfile::tempdir().unwrap();
        let history = Arc::new(SessionHistory::new(dir.path().to_path_buf()));
        let persister = history.persister("s1".into());

        persister.persist_message(&Message::user("from trait"));
        persister.persist_message(&Message::assistant("reply"));

        let msgs = history.load_jsonl("s1").unwrap().unwrap();
        assert_eq!(msgs.len(), 2);
    }
}
