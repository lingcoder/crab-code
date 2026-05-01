//! Local session transcript recording.
//!
//! Records a complete transcript of the session (messages, tool uses,
//! tool outputs) to a local JSONL file. This is purely local — nothing
//! is sent to any remote endpoint. The recording can be used for
//! debugging, auditing, and session replay.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Get current timestamp as milliseconds since epoch.
fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

// ── Recorder ──────────────────────────────────────────────────────────

/// Records session events to a local JSONL transcript file.
///
/// Each event is written as a single JSON line with a timestamp, event
/// type, and payload. The file is created on first write and flushed
/// on each event to minimize data loss on crash.
///
/// # Example
///
/// ```rust,no_run
/// use crab_session::recorder::SessionRecorder;
///
/// let mut recorder = SessionRecorder::new("sess_abc123");
/// recorder.record_message("user", "Hello!").unwrap();
/// recorder.record_message("assistant", "Hi there!").unwrap();
/// let path = recorder.finish().unwrap();
/// println!("Transcript saved to: {}", path.display());
/// ```
pub struct SessionRecorder {
    /// Path to the output JSONL file.
    output_path: PathBuf,
}

impl SessionRecorder {
    /// Create a new recorder for the given session.
    ///
    /// The transcript file is stored at
    /// `~/.crab/sessions/<session_id>/transcript.jsonl`.
    #[must_use]
    pub fn new(session_id: &str) -> Self {
        let output_path = crab_utils::path::home_dir()
            .join(".crab")
            .join("sessions")
            .join(session_id)
            .join("transcript.jsonl");
        Self { output_path }
    }

    /// Record a conversation message.
    ///
    /// # Arguments
    ///
    /// * `role` — The message role ("user", "assistant", "system").
    /// * `content` — The text content of the message.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the file cannot be opened or written.
    pub fn record_message(&mut self, role: &str, content: &str) -> std::io::Result<()> {
        let ts = now_epoch_ms();
        let record = serde_json::json!({
            "type": "message",
            "role": role,
            "content": content,
            "ts": ts,
        });
        self.append_line(&record)
    }

    /// Record a tool use event (invocation + result).
    ///
    /// # Arguments
    ///
    /// * `tool` — Tool name.
    /// * `input` — JSON string of the tool input.
    /// * `output` — JSON string of the tool output.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the file cannot be opened or written.
    pub fn record_tool_use(
        &mut self,
        tool: &str,
        input: &str,
        output: &str,
    ) -> std::io::Result<()> {
        let ts = now_epoch_ms();
        let record = serde_json::json!({
            "type": "tool_use",
            "tool": tool,
            "input": input,
            "output": output,
            "ts": ts,
        });
        self.append_line(&record)
    }

    /// Finalize the recording and return the path to the transcript file.
    ///
    /// Flushes any buffered data and closes the file handle.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the final flush fails.
    pub fn finish(&mut self) -> std::io::Result<PathBuf> {
        // Each write already flushes, so just return the path.
        Ok(self.output_path.clone())
    }

    /// Append a single JSONL line to the transcript file.
    fn append_line(&self, value: &serde_json::Value) -> std::io::Result<()> {
        if let Some(parent) = self.output_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.output_path)?;
        let json = serde_json::to_string(value)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        writeln!(file, "{json}")?;
        Ok(())
    }

    /// The path where the transcript will be (or has been) written.
    #[must_use]
    pub fn output_path(&self) -> &PathBuf {
        &self.output_path
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorder_output_path_contains_session_id() {
        let recorder = SessionRecorder::new("sess_test_123");
        let path = recorder.output_path();
        assert!(path.to_string_lossy().contains("sess_test_123"));
        assert!(path.to_string_lossy().contains("transcript.jsonl"));
    }

    #[test]
    fn recorder_new_does_not_create_file() {
        // Just constructing a recorder should not create the file
        let recorder = SessionRecorder::new("sess_no_create");
        assert!(!recorder.output_path().exists());
    }
}
