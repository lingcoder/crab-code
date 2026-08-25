//! Background job registry — the data model for tracking async jobs.
//!
//! Mirrors ccb's `AppState.tasks`: a central registry of all running/completed
//! background jobs (bash commands, sub-agents, teammates) keyed by job ID.
//! The registry is a pure data structure; execution handles (`JoinHandle`,
//! `CancellationToken`) live in the runtime layer that populates it.
//!
//! Distinct from [`crate::task`], which models the *work queue* agents claim
//! items from. A job is a running unit of execution with a handle; a task is
//! a unit of work waiting to be picked up.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── Job identity ──────────────────────────────────────────────────────

/// Type of background job — mirrors ccb's `TaskType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobType {
    /// Background bash command (`run_in_background`).
    LocalBash,
    /// Background sub-agent (Agent tool with `run_in_background`).
    LocalAgent,
    /// In-process teammate (named `Agent` spawn → teammate backend).
    InProcessTeammate,
}

/// Lifecycle status of a background job — mirrors ccb's `TaskStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
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

impl JobStatus {
    /// Returns `true` for `Completed`, `Failed`, or `Killed`.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Killed)
    }
}

// ── Job entry ─────────────────────────────────────────────────────────

/// A single entry in the job registry — the metadata the runtime exposes
/// about a background job. Execution handles live elsewhere.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundJob {
    /// Unique job ID (prefixed: `b<8>` for bash, `a<8>` for agent).
    pub id: String,
    /// What kind of job this is.
    pub job_type: JobType,
    /// Current lifecycle status.
    pub status: JobStatus,
    /// Human-readable description (e.g. `"git push"`, `"review auth module"`).
    pub description: String,
    /// The `tool_use_id` that spawned this job (for result routing).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    /// Unix timestamp (seconds) when the job was created.
    pub started_at: u64,
    /// Unix timestamp (seconds) when the job reached a terminal state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
    /// Path to the job's output file (for `TaskOutputTool` to read).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_path: Option<String>,
    /// Exit code for bash jobs (0 = success).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Error message if the job failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl BackgroundJob {
    /// Create a new pending job entry.
    #[must_use]
    pub fn new(id: String, job_type: JobType, description: String) -> Self {
        Self {
            id,
            job_type,
            status: JobStatus::Pending,
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
    pub fn finish(&mut self, status: JobStatus) {
        debug_assert!(
            status.is_terminal(),
            "finish() called with non-terminal status"
        );
        self.status = status;
        self.ended_at = Some(now_epoch_secs());
    }
}

// ── Job registry ─────────────────────────────────────────────────────

/// Central registry of all background jobs. Thread-safe via external
/// `Mutex<RwLock<...>>` or `Arc<Mutex<...>>` at the runtime layer.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JobRegistry {
    entries: HashMap<String, BackgroundJob>,
}

impl JobRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new job. Returns `false` if the ID is already taken.
    pub fn register(&mut self, entry: BackgroundJob) -> bool {
        if self.entries.contains_key(&entry.id) {
            return false;
        }
        self.entries.insert(entry.id.clone(), entry);
        true
    }

    /// Update a job's status. No-op if the job doesn't exist.
    pub fn set_status(&mut self, id: &str, status: JobStatus) {
        if let Some(entry) = self.entries.get_mut(id) {
            if status.is_terminal() {
                entry.finish(status);
            } else {
                entry.status = status;
            }
        }
    }

    /// Set the output path for a job.
    pub fn set_output_path(&mut self, id: &str, path: String) {
        if let Some(entry) = self.entries.get_mut(id) {
            entry.output_path = Some(path);
        }
    }

    /// Record a failure with an error message.
    pub fn set_error(&mut self, id: &str, error: String) {
        if let Some(entry) = self.entries.get_mut(id) {
            entry.error = Some(error);
            entry.finish(JobStatus::Failed);
        }
    }

    /// Record completion with an optional exit code.
    pub fn set_exit_code(&mut self, id: &str, exit_code: i32) {
        if let Some(entry) = self.entries.get_mut(id) {
            entry.exit_code = Some(exit_code);
            let status = if exit_code == 0 {
                JobStatus::Completed
            } else {
                JobStatus::Failed
            };
            entry.finish(status);
        }
    }

    /// Look up a job by ID.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&BackgroundJob> {
        self.entries.get(id)
    }

    /// Mutable look up a job by ID.
    pub fn get_mut(&mut self, id: &str) -> Option<&mut BackgroundJob> {
        self.entries.get_mut(id)
    }

    /// List all jobs (for `TaskListTool`).
    #[must_use]
    pub fn list(&self) -> Vec<&BackgroundJob> {
        self.entries.values().collect()
    }

    /// List only running jobs.
    #[must_use]
    pub fn list_running(&self) -> Vec<&BackgroundJob> {
        self.entries
            .values()
            .filter(|e| e.status == JobStatus::Running)
            .collect()
    }

    /// Remove terminal jobs that have been notified (eviction).
    /// Returns the IDs of evicted jobs.
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

    /// Number of registered jobs.
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

/// Generate a prefixed job ID (e.g. `b` for bash, `a` for agent).
#[must_use]
pub fn generate_job_id(prefix: char) -> String {
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
    fn job_status_terminal() {
        assert!(!JobStatus::Pending.is_terminal());
        assert!(!JobStatus::Running.is_terminal());
        assert!(JobStatus::Completed.is_terminal());
        assert!(JobStatus::Failed.is_terminal());
        assert!(JobStatus::Killed.is_terminal());
    }

    #[test]
    fn register_and_lookup() {
        let mut reg = JobRegistry::new();
        let entry = BackgroundJob::new("b12345678".into(), JobType::LocalBash, "git push".into());
        assert!(reg.register(entry));
        assert_eq!(reg.get("b12345678").unwrap().description, "git push");
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn register_duplicate_returns_false() {
        let mut reg = JobRegistry::new();
        let e1 = BackgroundJob::new("b1".into(), JobType::LocalBash, "cmd1".into());
        let e2 = BackgroundJob::new("b1".into(), JobType::LocalBash, "cmd2".into());
        assert!(reg.register(e1));
        assert!(!reg.register(e2));
    }

    #[test]
    fn set_status_transitions() {
        let mut reg = JobRegistry::new();
        reg.register(BackgroundJob::new(
            "a1".into(),
            JobType::LocalAgent,
            "review".into(),
        ));

        reg.set_status("a1", JobStatus::Running);
        assert_eq!(reg.get("a1").unwrap().status, JobStatus::Running);

        reg.set_status("a1", JobStatus::Completed);
        let entry = reg.get("a1").unwrap();
        assert_eq!(entry.status, JobStatus::Completed);
        assert!(entry.ended_at.is_some());
    }

    #[test]
    fn set_exit_code_marks_terminal() {
        let mut reg = JobRegistry::new();
        reg.register(BackgroundJob::new(
            "b1".into(),
            JobType::LocalBash,
            "ls".into(),
        ));
        reg.set_status("b1", JobStatus::Running);

        reg.set_exit_code("b1", 0);
        assert_eq!(reg.get("b1").unwrap().status, JobStatus::Completed);

        let mut reg = JobRegistry::new();
        reg.register(BackgroundJob::new(
            "b2".into(),
            JobType::LocalBash,
            "bad".into(),
        ));
        reg.set_exit_code("b2", 1);
        assert_eq!(reg.get("b2").unwrap().status, JobStatus::Failed);
    }

    #[test]
    fn evict_terminal() {
        let mut reg = JobRegistry::new();
        reg.register(BackgroundJob::new(
            "a1".into(),
            JobType::LocalAgent,
            "done".into(),
        ));
        reg.register(BackgroundJob::new(
            "a2".into(),
            JobType::LocalAgent,
            "running".into(),
        ));
        reg.set_status("a1", JobStatus::Completed);
        reg.set_status("a2", JobStatus::Running);

        let evicted = reg.evict_terminal();
        assert_eq!(evicted, vec!["a1".to_string()]);
        assert_eq!(reg.len(), 1);
        assert!(reg.get("a1").is_none());
    }

    #[test]
    fn list_running() {
        let mut reg = JobRegistry::new();
        reg.register(BackgroundJob::new(
            "a1".into(),
            JobType::LocalAgent,
            "r1".into(),
        ));
        reg.register(BackgroundJob::new(
            "a2".into(),
            JobType::LocalAgent,
            "r2".into(),
        ));
        reg.register(BackgroundJob::new(
            "a3".into(),
            JobType::LocalAgent,
            "done".into(),
        ));
        reg.set_status("a1", JobStatus::Running);
        reg.set_status("a2", JobStatus::Running);
        reg.set_status("a3", JobStatus::Completed);

        let running = reg.list_running();
        assert_eq!(running.len(), 2);
    }

    #[test]
    fn generate_job_id_unique() {
        let id1 = generate_job_id('b');
        let id2 = generate_job_id('b');
        assert_ne!(id1, id2);
        assert!(id1.starts_with('b'));
        assert!(id1.len() > 1); // prefix + hex chars
    }

    #[test]
    fn background_job_serde_roundtrip() {
        let entry = BackgroundJob {
            id: "a12345678".into(),
            job_type: JobType::LocalAgent,
            status: JobStatus::Running,
            description: "review code".into(),
            tool_use_id: Some("tu_123".into()),
            started_at: 1_700_000_000,
            ended_at: None,
            output_path: Some("/tmp/task.out".into()),
            exit_code: None,
            error: None,
        };
        let json = serde_json::to_string_pretty(&entry).unwrap();
        let parsed: BackgroundJob = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, entry.id);
        assert_eq!(parsed.job_type, entry.job_type);
        assert_eq!(parsed.status, entry.status);
    }

    #[test]
    fn job_registry_serde_roundtrip() {
        let mut reg = JobRegistry::new();
        reg.register(BackgroundJob::new(
            "b1".into(),
            JobType::LocalBash,
            "ls".into(),
        ));
        reg.register(BackgroundJob::new(
            "a1".into(),
            JobType::LocalAgent,
            "review".into(),
        ));

        let json = serde_json::to_string_pretty(&reg).unwrap();
        let parsed: JobRegistry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 2);
        assert!(parsed.get("b1").is_some());
    }
}
