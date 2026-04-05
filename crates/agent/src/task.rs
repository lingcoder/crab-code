use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// Status of a task in the task list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
    Deleted,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::InProgress => write!(f, "in_progress"),
            Self::Completed => write!(f, "completed"),
            Self::Deleted => write!(f, "deleted"),
        }
    }
}

/// A single task with optional dependency tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub subject: String,
    pub description: String,
    pub status: TaskStatus,
    pub owner: Option<String>,
    pub blocked_by: Vec<String>,
    pub blocks: Vec<String>,
}

/// Manages tasks and their dependency graph.
pub struct TaskList {
    tasks: Vec<Task>,
    next_id: u64,
}

impl TaskList {
    #[must_use]
    pub fn new() -> Self {
        Self {
            tasks: Vec::new(),
            next_id: 1,
        }
    }

    /// Create a new task with auto-incremented ID. Returns the assigned ID.
    pub fn create(&mut self, subject: String, description: String) -> String {
        let id = self.next_id.to_string();
        self.next_id += 1;
        self.tasks.push(Task {
            id: id.clone(),
            subject,
            description,
            status: TaskStatus::Pending,
            owner: None,
            blocked_by: Vec::new(),
            blocks: Vec::new(),
        });
        id
    }

    /// Get a task by ID (excluding deleted).
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Task> {
        self.tasks
            .iter()
            .find(|t| t.id == id && t.status != TaskStatus::Deleted)
    }

    /// Get a mutable reference to a task by ID (excluding deleted).
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Task> {
        self.tasks
            .iter_mut()
            .find(|t| t.id == id && t.status != TaskStatus::Deleted)
    }

    /// List all non-deleted tasks.
    #[must_use]
    pub fn list(&self) -> Vec<&Task> {
        self.tasks
            .iter()
            .filter(|t| t.status != TaskStatus::Deleted)
            .collect()
    }

    /// List tasks available for claiming (pending, unowned, unblocked).
    #[must_use]
    pub fn available_tasks(&self) -> Vec<&Task> {
        self.tasks
            .iter()
            .filter(|t| {
                t.status == TaskStatus::Pending
                    && t.owner.is_none()
                    && t.blocked_by.iter().all(|dep| {
                        self.get(dep)
                            .is_none_or(|d| d.status == TaskStatus::Completed)
                    })
            })
            .collect()
    }

    /// Update a task's fields. Returns `true` if the task was found.
    pub fn update(
        &mut self,
        id: &str,
        status: Option<TaskStatus>,
        subject: Option<String>,
        description: Option<String>,
        owner: Option<String>,
    ) -> bool {
        let Some(task) = self.get_mut(id) else {
            return false;
        };
        if let Some(s) = status {
            task.status = s;
        }
        if let Some(s) = subject {
            task.subject = s;
        }
        if let Some(d) = description {
            task.description = d;
        }
        if owner.is_some() {
            task.owner = owner;
        }
        true
    }

    /// Mark a task as deleted.
    pub fn delete(&mut self, id: &str) -> bool {
        let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) else {
            return false;
        };
        task.status = TaskStatus::Deleted;
        true
    }

    /// Add a "blocked by" dependency: `task_id` is blocked by `blocker_id`.
    pub fn add_blocked_by(&mut self, task_id: &str, blocker_id: &str) -> bool {
        // Add to task's blocked_by list
        if let Some(task) = self.get_mut(task_id) {
            if !task.blocked_by.contains(&blocker_id.to_string()) {
                task.blocked_by.push(blocker_id.to_string());
            }
        } else {
            return false;
        }
        // Add reverse: blocker blocks task
        if let Some(blocker) = self.get_mut(blocker_id)
            && !blocker.blocks.contains(&task_id.to_string())
        {
            blocker.blocks.push(task_id.to_string());
        }
        true
    }

    /// Add a "blocks" dependency: `task_id` blocks `blocked_id`.
    pub fn add_blocks(&mut self, task_id: &str, blocked_id: &str) -> bool {
        self.add_blocked_by(blocked_id, task_id)
    }
}

impl Default for TaskList {
    fn default() -> Self {
        Self::new()
    }
}

/// Thread-safe shared handle to a `TaskList`.
pub type SharedTaskList = Arc<Mutex<TaskList>>;

/// Create a new shared task list.
#[must_use]
pub fn shared_task_list() -> SharedTaskList {
    Arc::new(Mutex::new(TaskList::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_assigns_incrementing_ids() {
        let mut list = TaskList::new();
        let id1 = list.create("First".into(), "desc".into());
        let id2 = list.create("Second".into(), "desc".into());
        assert_eq!(id1, "1");
        assert_eq!(id2, "2");
    }

    #[test]
    fn get_returns_task() {
        let mut list = TaskList::new();
        let id = list.create("Test".into(), "desc".into());
        let task = list.get(&id).unwrap();
        assert_eq!(task.subject, "Test");
        assert_eq!(task.status, TaskStatus::Pending);
    }

    #[test]
    fn get_deleted_returns_none() {
        let mut list = TaskList::new();
        let id = list.create("Test".into(), "desc".into());
        list.delete(&id);
        assert!(list.get(&id).is_none());
    }

    #[test]
    fn list_excludes_deleted() {
        let mut list = TaskList::new();
        list.create("Keep".into(), "desc".into());
        let id2 = list.create("Delete".into(), "desc".into());
        list.delete(&id2);
        let visible = list.list();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].subject, "Keep");
    }

    #[test]
    fn update_changes_fields() {
        let mut list = TaskList::new();
        let id = list.create("Old".into(), "old desc".into());
        list.update(
            &id,
            Some(TaskStatus::InProgress),
            Some("New".into()),
            None,
            Some("alice".into()),
        );
        let task = list.get(&id).unwrap();
        assert_eq!(task.subject, "New");
        assert_eq!(task.status, TaskStatus::InProgress);
        assert_eq!(task.owner.as_deref(), Some("alice"));
        assert_eq!(task.description, "old desc"); // unchanged
    }

    #[test]
    fn update_nonexistent_returns_false() {
        let mut list = TaskList::new();
        assert!(!list.update("999", None, None, None, None));
    }

    #[test]
    fn available_tasks_filters_correctly() {
        let mut list = TaskList::new();
        let id1 = list.create("Available".into(), "".into());
        let id2 = list.create("Blocked".into(), "".into());
        let id3 = list.create("Owned".into(), "".into());
        list.create("In progress".into(), "".into());

        list.add_blocked_by(&id2, &id1);
        list.update(&id3, None, None, None, Some("bob".into()));
        list.update("4", Some(TaskStatus::InProgress), None, None, None);

        let available = list.available_tasks();
        assert_eq!(available.len(), 1);
        assert_eq!(available[0].id, id1);
    }

    #[test]
    fn unblocked_after_completion() {
        let mut list = TaskList::new();
        let id1 = list.create("Blocker".into(), "".into());
        let id2 = list.create("Blocked".into(), "".into());
        list.add_blocked_by(&id2, &id1);

        assert!(list.available_tasks().iter().all(|t| t.id != id2));

        list.update(&id1, Some(TaskStatus::Completed), None, None, None);
        let available = list.available_tasks();
        assert!(available.iter().any(|t| t.id == id2));
    }

    #[test]
    fn add_blocks_creates_bidirectional_dependency() {
        let mut list = TaskList::new();
        let id1 = list.create("Blocker".into(), "".into());
        let id2 = list.create("Blocked".into(), "".into());
        list.add_blocks(&id1, &id2);

        let blocker = list.get(&id1).unwrap();
        assert!(blocker.blocks.contains(&id2));

        let blocked = list.get(&id2).unwrap();
        assert!(blocked.blocked_by.contains(&id1));
    }

    #[test]
    fn task_status_display() {
        assert_eq!(TaskStatus::Pending.to_string(), "pending");
        assert_eq!(TaskStatus::InProgress.to_string(), "in_progress");
        assert_eq!(TaskStatus::Completed.to_string(), "completed");
        assert_eq!(TaskStatus::Deleted.to_string(), "deleted");
    }

    #[test]
    fn task_status_serde_roundtrip() {
        let status = TaskStatus::InProgress;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, r#""in_progress""#);
        let back: TaskStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back, status);
    }

    #[test]
    fn shared_task_list_is_thread_safe() {
        let shared = shared_task_list();
        let shared2 = Arc::clone(&shared);
        let handle = std::thread::spawn(move || {
            let mut list = shared2.lock().unwrap();
            list.create("From thread".into(), "".into());
        });
        handle.join().unwrap();
        let list = shared.lock().unwrap();
        assert_eq!(list.list().len(), 1);
    }
}
