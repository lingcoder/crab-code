//! Background task registry — the data model for tracking async tasks.
//!
//! Mirrors ccb's `AppState.tasks`: a central registry of all running/completed
//! background tasks (bash commands, sub-agents, teammates) keyed by task ID.
//! The registry is a pure data structure; execution handles (`JoinHandle`,
//! `CancellationToken`) live in the runtime layer that populates it.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── Task identity ─────────────────────────────────────────────────────

/// Type of background task — mirrors ccb's `TaskType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskType {
    /// Background bash command (`run_in_background`).
    LocalBash,
    /// Background sub-agent (Agent tool with `run_in_background`).
    LocalAgent,
    /// In-process teammate (named `Agent` spawn → swarm backend).
    InProcessTeammate,
}

/// Lifecycle status of a background task — mirrors ccb's `TaskStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Just created, not yet running.
    Pending,
    /// Actively executing.
    Running,
    /// Finished successfully.
    Completed,
    /// Finished with an error.
    Failed,
    /// Stopped by user (`TaskStop` / `KillShell`).
    Killed,
}

impl TaskStatus {
    /// Returns `true` for `Completed`, `Failed`, or `Killed`.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Killed)
    }
}

// ── Task entry ────────────────────────────────────────────────────────

/// A single entry in the task registry — the metadata the runtime exposes
/// about a background task. Execution handles live elsewhere.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskEntry {
    /// Unique task ID (prefixed: `b<8>` for bash, `a<8>` for agent).
    pub id: String,
    /// What kind of task this is.
    pub task_type: TaskType,
    /// Current lifecycle status.
    pub status: TaskStatus,
    /// Human-readable description (e.g. `"git push"`, `"review auth module"`).
    pub description: String,
    /// The `tool_use_id` that spawned this task (for result routing).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    /// Unix timestamp (seconds) when the task was created.
    pub started_at: u64,
    /// Unix timestamp (seconds) when the task reached a terminal state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
    /// Path to the task's output file (for `TaskOutputTool` to read).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_path: Option<String>,
    /// Exit code for bash tasks (0 = success).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Error message if the task failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl TaskEntry {
    /// Create a new pending task entry.
    #[must_use]
    pub fn new(id: String, task_type: TaskType, description: String) -> Self {
        Self {
            id,
            task_type,
            status: TaskStatus::Pending,
            description,
            tool_use_id: None,
            started_at: now_epoch_secs(),
            ended_at: None,
            output_path: None,
            exit_code: None,
            error: None,
        }
    }

    /// Transition to a terminal status, stamping `ended_at`.
    pub fn finish(&mut self, status: TaskStatus) {
        debug_assert!(
            status.is_terminal(),
            "finish() called with non-terminal status"
        );
        self.status = status;
        self.ended_at = Some(now_epoch_secs());
    }
}

// ── Task registry ─────────────────────────────────────────────────────

/// Central registry of all background tasks. Thread-safe via external
/// `Mutex<RwLock<...>>` or `Arc<Mutex<...>>` at the runtime layer.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskRegistry {
    entries: HashMap<String, TaskEntry>,
}

impl TaskRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new task. Returns `false` if the ID is already taken.
    pub fn register(&mut self, entry: TaskEntry) -> bool {
        if self.entries.contains_key(&entry.id) {
            return false;
        }
        self.entries.insert(entry.id.clone(), entry);
        true
    }

    /// Update a task's status. No-op if the task doesn't exist.
    pub fn set_status(&mut self, id: &str, status: TaskStatus) {
        if let Some(entry) = self.entries.get_mut(id) {
            if status.is_terminal() {
                entry.finish(status);
            } else {
                entry.status = status;
            }
        }
    }

    /// Set the output path for a task.
    pub fn set_output_path(&mut self, id: &str, path: String) {
        if let Some(entry) = self.entries.get_mut(id) {
            entry.output_path = Some(path);
        }
    }

    /// Record a failure with an error message.
    pub fn set_error(&mut self, id: &str, error: String) {
        if let Some(entry) = self.entries.get_mut(id) {
            entry.error = Some(error);
            entry.finish(TaskStatus::Failed);
        }
    }

    /// Record completion with an optional exit code.
    pub fn set_exit_code(&mut self, id: &str, exit_code: i32) {
        if let Some(entry) = self.entries.get_mut(id) {
            entry.exit_code = Some(exit_code);
            let status = if exit_code == 0 {
                TaskStatus::Completed
            } else {
                TaskStatus::Failed
            };
            entry.finish(status);
        }
    }

    /// Look up a task by ID.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&TaskEntry> {
        self.entries.get(id)
    }

    /// Mutable look up a task by ID.
    pub fn get_mut(&mut self, id: &str) -> Option<&mut TaskEntry> {
        self.entries.get_mut(id)
    }

    /// List all tasks (for `TaskListTool`).
    #[must_use]
    pub fn list(&self) -> Vec<&TaskEntry> {
        self.entries.values().collect()
    }

    /// List only running tasks.
    #[must_use]
    pub fn list_running(&self) -> Vec<&TaskEntry> {
        self.entries
            .values()
            .filter(|e| e.status == TaskStatus::Running)
            .collect()
    }

    /// Remove terminal tasks that have been notified (eviction).
    /// Returns the IDs of evicted tasks.
    pub fn evict_terminal(&mut self) -> Vec<String> {
        let terminal: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, e)| e.status.is_terminal())
            .map(|(id, _)| id.clone())
            .collect();
        for id in &terminal {
            self.entries.remove(id);
        }
        terminal
    }

    /// Number of registered tasks.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the registry is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ── Helpers ───────────────────────────────────────────────────────────

/// Current time as Unix epoch seconds.
fn now_epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Generate a prefixed task ID (e.g. `b` for bash, `a` for agent).
#[must_use]
pub fn generate_task_id(prefix: char) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .hash(&mut hasher);
    // Thread id adds uniqueness for concurrent spawns.
    std::thread::current().id().hash(&mut hasher);
    let hash = hasher.finish();
    format!("{prefix}{hash:08x}")
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_status_terminal() {
        assert!(!TaskStatus::Pending.is_terminal());
        assert!(!TaskStatus::Running.is_terminal());
        assert!(TaskStatus::Completed.is_terminal());
        assert!(TaskStatus::Failed.is_terminal());
        assert!(TaskStatus::Killed.is_terminal());
    }

    #[test]
    fn register_and_lookup() {
        let mut reg = TaskRegistry::new();
        let entry = TaskEntry::new("b12345678".into(), TaskType::LocalBash, "git push".into());
        assert!(reg.register(entry));
        assert_eq!(reg.get("b12345678").unwrap().description, "git push");
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn register_duplicate_returns_false() {
        let mut reg = TaskRegistry::new();
        let e1 = TaskEntry::new("b1".into(), TaskType::LocalBash, "cmd1".into());
        let e2 = TaskEntry::new("b1".into(), TaskType::LocalBash, "cmd2".into());
        assert!(reg.register(e1));
        assert!(!reg.register(e2));
    }

    #[test]
    fn set_status_transitions() {
        let mut reg = TaskRegistry::new();
        reg.register(TaskEntry::new(
            "a1".into(),
            TaskType::LocalAgent,
            "review".into(),
        ));

        reg.set_status("a1", TaskStatus::Running);
        assert_eq!(reg.get("a1").unwrap().status, TaskStatus::Running);

        reg.set_status("a1", TaskStatus::Completed);
        let entry = reg.get("a1").unwrap();
        assert_eq!(entry.status, TaskStatus::Completed);
        assert!(entry.ended_at.is_some());
    }

    #[test]
    fn set_exit_code_marks_terminal() {
        let mut reg = TaskRegistry::new();
        reg.register(TaskEntry::new(
            "b1".into(),
            TaskType::LocalBash,
            "ls".into(),
        ));
        reg.set_status("b1", TaskStatus::Running);

        reg.set_exit_code("b1", 0);
        assert_eq!(reg.get("b1").unwrap().status, TaskStatus::Completed);

        let mut reg = TaskRegistry::new();
        reg.register(TaskEntry::new(
            "b2".into(),
            TaskType::LocalBash,
            "bad".into(),
        ));
        reg.set_exit_code("b2", 1);
        assert_eq!(reg.get("b2").unwrap().status, TaskStatus::Failed);
    }

    #[test]
    fn evict_terminal() {
        let mut reg = TaskRegistry::new();
        reg.register(TaskEntry::new(
            "a1".into(),
            TaskType::LocalAgent,
            "done".into(),
        ));
        reg.register(TaskEntry::new(
            "a2".into(),
            TaskType::LocalAgent,
            "running".into(),
        ));
        reg.set_status("a1", TaskStatus::Completed);
        reg.set_status("a2", TaskStatus::Running);

        let evicted = reg.evict_terminal();
        assert_eq!(evicted, vec!["a1".to_string()]);
        assert_eq!(reg.len(), 1);
        assert!(reg.get("a1").is_none());
    }

    #[test]
    fn list_running() {
        let mut reg = TaskRegistry::new();
        reg.register(TaskEntry::new(
            "a1".into(),
            TaskType::LocalAgent,
            "r1".into(),
        ));
        reg.register(TaskEntry::new(
            "a2".into(),
            TaskType::LocalAgent,
            "r2".into(),
        ));
        reg.register(TaskEntry::new(
            "a3".into(),
            TaskType::LocalAgent,
            "done".into(),
        ));
        reg.set_status("a1", TaskStatus::Running);
        reg.set_status("a2", TaskStatus::Running);
        reg.set_status("a3", TaskStatus::Completed);

        let running = reg.list_running();
        assert_eq!(running.len(), 2);
    }

    #[test]
    fn generate_task_id_unique() {
        let id1 = generate_task_id('b');
        let id2 = generate_task_id('b');
        assert_ne!(id1, id2);
        assert!(id1.starts_with('b'));
        assert!(id1.len() > 1); // prefix + hex chars
    }

    #[test]
    fn task_entry_serde_roundtrip() {
        let entry = TaskEntry {
            id: "a12345678".into(),
            task_type: TaskType::LocalAgent,
            status: TaskStatus::Running,
            description: "review code".into(),
            tool_use_id: Some("tu_123".into()),
            started_at: 1_700_000_000,
            ended_at: None,
            output_path: Some("/tmp/task.out".into()),
            exit_code: None,
            error: None,
        };
        let json = serde_json::to_string_pretty(&entry).unwrap();
        let parsed: TaskEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, entry.id);
        assert_eq!(parsed.task_type, entry.task_type);
        assert_eq!(parsed.status, entry.status);
    }

    #[test]
    fn task_registry_serde_roundtrip() {
        let mut reg = TaskRegistry::new();
        reg.register(TaskEntry::new(
            "b1".into(),
            TaskType::LocalBash,
            "ls".into(),
        ));
        reg.register(TaskEntry::new(
            "a1".into(),
            TaskType::LocalAgent,
            "review".into(),
        ));

        let json = serde_json::to_string_pretty(&reg).unwrap();
        let parsed: TaskRegistry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 2);
        assert!(parsed.get("b1").is_some());
    }
}
