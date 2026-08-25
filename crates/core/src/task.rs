//! Work queue — the tasks agents create, claim, and complete.
//!
//! A task is a unit of work waiting to be picked up, with an optional owner
//! and a dependency graph (`blocked_by` / `blocks`). The `TaskCreate` /
//! `TaskList` / `TaskGet` / `TaskUpdate` tools drive it from the model side;
//! teammates claim from it on the execution side.
//!
//! Distinct from [`crate::job`], which tracks *running* background jobs by
//! handle. A task is work to be done; a job is an execution in flight.

use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// Status of a task in the work queue.
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

/// A single unit of work with optional dependency tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub subject: String,
    pub description: String,
    pub status: TaskStatus,
    /// Agent that has claimed this task, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Tasks that must complete before this one can start.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<String>,
    /// Tasks that cannot start until this one completes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<String>,
}

/// Work queue with auto-incrementing IDs and a dependency graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

    /// Create a new task with an auto-incremented ID. Returns the assigned ID.
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

    /// Get a task by ID, excluding deleted ones.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Task> {
        self.tasks
            .iter()
            .find(|t| t.id == id && t.status != TaskStatus::Deleted)
    }

    /// Get a mutable reference to a task by ID, excluding deleted ones.
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

    /// List tasks available for claiming: pending, unowned, and with every
    /// blocker completed.
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

    /// Update a task's fields. `None` leaves a field unchanged. Passing
    /// [`TaskStatus::Deleted`] as `status` deletes the task. Returns `true`
    /// if the task was found.
    #[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
    pub fn update(
        &mut self,
        id: &str,
        status: Option<TaskStatus>,
        subject: Option<String>,
        description: Option<String>,
        owner: Option<String>,
        add_blocked_by: Option<Vec<String>>,
        add_blocks: Option<Vec<String>>,
    ) -> bool {
        if status == Some(TaskStatus::Deleted) {
            return self.delete(id);
        }

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
        let task_id = task.id.clone();

        for dep in add_blocked_by.into_iter().flatten() {
            self.add_blocked_by(&task_id, &dep);
        }
        for blocked in add_blocks.into_iter().flatten() {
            self.add_blocks(&task_id, &blocked);
        }
        true
    }

    /// Claim `id` for `owner`, moving it to [`TaskStatus::InProgress`].
    ///
    /// Fails when the task is missing, already owned, not pending, or still
    /// blocked. Callers hold the list's lock across the check and the write,
    /// so two agents racing for the same task cannot both win.
    pub fn claim(&mut self, id: &str, owner: &str) -> bool {
        if !self.is_available(id) {
            return false;
        }
        let Some(task) = self.get_mut(id) else {
            return false;
        };
        task.owner = Some(owner.to_owned());
        task.status = TaskStatus::InProgress;
        true
    }

    /// Claim the first available task for `owner`, returning it.
    ///
    /// This is the competitive path agents share: whoever calls first gets the
    /// task, and later callers move on to the next one.
    pub fn claim_next(&mut self, owner: &str) -> Option<Task> {
        let id = self.available_tasks().first().map(|t| t.id.clone())?;
        self.claim(&id, owner).then(|| self.get(&id).cloned())?
    }

    /// Whether `id` is pending, unowned, and unblocked.
    #[must_use]
    fn is_available(&self, id: &str) -> bool {
        self.available_tasks().iter().any(|t| t.id == id)
    }

    /// Mark a task as deleted.
    pub fn delete(&mut self, id: &str) -> bool {
        let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) else {
            return false;
        };
        task.status = TaskStatus::Deleted;
        true
    }

    /// Record that `task_id` is blocked by `blocker_id`, maintaining both
    /// sides of the edge. Returns `false` if `task_id` does not exist.
    pub fn add_blocked_by(&mut self, task_id: &str, blocker_id: &str) -> bool {
        if let Some(task) = self.get_mut(task_id) {
            if !task.blocked_by.iter().any(|d| d == blocker_id) {
                task.blocked_by.push(blocker_id.to_owned());
            }
        } else {
            return false;
        }
        if let Some(blocker) = self.get_mut(blocker_id)
            && !blocker.blocks.iter().any(|b| b == task_id)
        {
            blocker.blocks.push(task_id.to_owned());
        }
        true
    }

    /// Record that `task_id` blocks `blocked_id`, maintaining both sides of
    /// the edge. Returns `false` if `blocked_id` does not exist.
    pub fn add_blocks(&mut self, task_id: &str, blocked_id: &str) -> bool {
        self.add_blocked_by(blocked_id, task_id)
    }
}

impl Default for TaskList {
    fn default() -> Self {
        Self::new()
    }
}

/// Thread-safe shared handle to a [`TaskList`].
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
            None,
            None,
        );
        let task = list.get(&id).unwrap();
        assert_eq!(task.subject, "New");
        assert_eq!(task.status, TaskStatus::InProgress);
        assert_eq!(task.owner.as_deref(), Some("alice"));
        assert_eq!(task.description, "old desc");
    }

    #[test]
    fn update_nonexistent_returns_false() {
        let mut list = TaskList::new();
        assert!(!list.update("999", None, None, None, None, None, None));
    }

    #[test]
    fn update_with_deleted_status_deletes() {
        let mut list = TaskList::new();
        let id = list.create("Doomed".into(), String::new());
        assert!(list.update(&id, Some(TaskStatus::Deleted), None, None, None, None, None));
        assert!(list.get(&id).is_none());
    }

    #[test]
    fn update_adds_dependencies_both_ways() {
        let mut list = TaskList::new();
        let upstream = list.create("Blocker".into(), String::new());
        let downstream = list.create("Blocked".into(), String::new());
        list.update(
            &downstream,
            None,
            None,
            None,
            None,
            Some(vec![upstream.clone()]),
            None,
        );
        assert!(
            list.get(&downstream)
                .unwrap()
                .blocked_by
                .contains(&upstream)
        );
        assert!(list.get(&upstream).unwrap().blocks.contains(&downstream));
    }

    #[test]
    fn available_tasks_filters_correctly() {
        let mut list = TaskList::new();
        let id1 = list.create("Available".into(), String::new());
        let id2 = list.create("Blocked".into(), String::new());
        let id3 = list.create("Owned".into(), String::new());
        list.create("In progress".into(), String::new());

        list.add_blocked_by(&id2, &id1);
        list.update(&id3, None, None, None, Some("bob".into()), None, None);
        list.update(
            "4",
            Some(TaskStatus::InProgress),
            None,
            None,
            None,
            None,
            None,
        );

        let available = list.available_tasks();
        assert_eq!(available.len(), 1);
        assert_eq!(available[0].id, id1);
    }

    #[test]
    fn unblocked_after_completion() {
        let mut list = TaskList::new();
        let id1 = list.create("Blocker".into(), String::new());
        let id2 = list.create("Blocked".into(), String::new());
        list.add_blocked_by(&id2, &id1);

        assert!(list.available_tasks().iter().all(|t| t.id != id2));

        list.update(
            &id1,
            Some(TaskStatus::Completed),
            None,
            None,
            None,
            None,
            None,
        );
        assert!(list.available_tasks().iter().any(|t| t.id == id2));
    }

    #[test]
    fn add_blocks_creates_bidirectional_dependency() {
        let mut list = TaskList::new();
        let id1 = list.create("Blocker".into(), String::new());
        let id2 = list.create("Blocked".into(), String::new());
        list.add_blocks(&id1, &id2);

        assert!(list.get(&id1).unwrap().blocks.contains(&id2));
        assert!(list.get(&id2).unwrap().blocked_by.contains(&id1));
    }

    #[test]
    fn claim_takes_an_available_task() {
        let mut list = TaskList::new();
        let id = list.create("Work".into(), String::new());

        assert!(list.claim(&id, "alice"));
        let task = list.get(&id).unwrap();
        assert_eq!(task.owner.as_deref(), Some("alice"));
        assert_eq!(task.status, TaskStatus::InProgress);
    }

    #[test]
    fn a_task_can_only_be_claimed_once() {
        let mut list = TaskList::new();
        let id = list.create("Work".into(), String::new());

        assert!(list.claim(&id, "alice"));
        assert!(
            !list.claim(&id, "bob"),
            "a claimed task is no longer available"
        );
        assert_eq!(list.get(&id).unwrap().owner.as_deref(), Some("alice"));
    }

    #[test]
    fn a_blocked_task_cannot_be_claimed() {
        let mut list = TaskList::new();
        let upstream = list.create("Blocker".into(), String::new());
        let downstream = list.create("Blocked".into(), String::new());
        list.add_blocked_by(&downstream, &upstream);

        assert!(!list.claim(&downstream, "alice"));

        list.update(
            &upstream,
            Some(TaskStatus::Completed),
            None,
            None,
            None,
            None,
            None,
        );
        assert!(list.claim(&downstream, "alice"));
    }

    #[test]
    fn claiming_a_missing_task_fails() {
        let mut list = TaskList::new();
        assert!(!list.claim("999", "alice"));
    }

    #[test]
    fn claim_next_hands_out_each_task_once() {
        let mut list = TaskList::new();
        let first = list.create("First".into(), String::new());
        let second = list.create("Second".into(), String::new());

        let a = list.claim_next("alice").unwrap();
        let b = list.claim_next("bob").unwrap();
        assert_eq!(a.id, first);
        assert_eq!(b.id, second);
        assert_eq!(a.owner.as_deref(), Some("alice"));
        assert_eq!(b.owner.as_deref(), Some("bob"));

        assert!(list.claim_next("carol").is_none(), "the queue is drained");
    }

    #[test]
    fn claim_next_skips_blocked_tasks() {
        let mut list = TaskList::new();
        let upstream = list.create("Blocker".into(), String::new());
        let downstream = list.create("Blocked".into(), String::new());
        list.add_blocked_by(&downstream, &upstream);

        assert_eq!(list.claim_next("alice").unwrap().id, upstream);
        assert!(list.claim_next("bob").is_none());
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
    fn task_serializes_camel_case() {
        let mut list = TaskList::new();
        let upstream = list.create("Blocker".into(), String::new());
        let downstream = list.create("Blocked".into(), String::new());
        list.add_blocked_by(&downstream, &upstream);
        let json = serde_json::to_string(list.get(&downstream).unwrap()).unwrap();
        assert!(json.contains("blockedBy"), "expected camelCase: {json}");
    }

    #[test]
    fn shared_task_list_is_thread_safe() {
        let shared = shared_task_list();
        let shared2 = Arc::clone(&shared);
        let handle = std::thread::spawn(move || {
            let mut list = shared2.lock().unwrap();
            list.create("From thread".into(), String::new());
        });
        handle.join().unwrap();
        let len = shared.lock().unwrap().list().len();
        assert_eq!(len, 1);
    }

    #[test]
    fn default_task_list_is_empty() {
        let list = TaskList::default();
        assert!(list.list().is_empty());
    }

    #[test]
    fn delete_nonexistent_returns_false() {
        let mut list = TaskList::new();
        assert!(!list.delete("999"));
    }

    #[test]
    fn update_description() {
        let mut list = TaskList::new();
        let id = list.create("Task".into(), "old".into());
        list.update(&id, None, None, Some("new desc".into()), None, None, None);
        assert_eq!(list.get(&id).unwrap().description, "new desc");
    }

    #[test]
    fn add_blocked_by_nonexistent_returns_false() {
        let mut list = TaskList::new();
        assert!(!list.add_blocked_by("999", "888"));
    }

    #[test]
    fn add_blocked_by_duplicate_is_idempotent() {
        let mut list = TaskList::new();
        let id1 = list.create("A".into(), String::new());
        let id2 = list.create("B".into(), String::new());
        list.add_blocked_by(&id2, &id1);
        list.add_blocked_by(&id2, &id1);
        assert_eq!(list.get(&id2).unwrap().blocked_by.len(), 1);
    }

    #[test]
    fn task_serde_roundtrip() {
        let task = Task {
            id: "1".into(),
            subject: "Test task".into(),
            description: "A test".into(),
            status: TaskStatus::InProgress,
            owner: Some("alice".into()),
            blocked_by: vec!["0".into()],
            blocks: vec!["2".into()],
        };
        let json = serde_json::to_string(&task).unwrap();
        let parsed: Task = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "1");
        assert_eq!(parsed.status, TaskStatus::InProgress);
        assert_eq!(parsed.owner.as_deref(), Some("alice"));
        assert_eq!(parsed.blocked_by, vec!["0".to_string()]);
    }

    #[test]
    fn available_tasks_excludes_completed() {
        let mut list = TaskList::new();
        let id = list.create("Done".into(), String::new());
        list.update(
            &id,
            Some(TaskStatus::Completed),
            None,
            None,
            None,
            None,
            None,
        );
        assert!(list.available_tasks().is_empty());
    }

    #[test]
    fn available_tasks_excludes_in_progress() {
        let mut list = TaskList::new();
        let id = list.create("Working".into(), String::new());
        list.update(
            &id,
            Some(TaskStatus::InProgress),
            None,
            None,
            None,
            None,
            None,
        );
        assert!(list.available_tasks().is_empty());
    }

    #[test]
    fn multiple_dependencies() {
        let mut list = TaskList::new();
        let id1 = list.create("Dep 1".into(), String::new());
        let id2 = list.create("Dep 2".into(), String::new());
        let id3 = list.create("Blocked".into(), String::new());
        list.add_blocked_by(&id3, &id1);
        list.add_blocked_by(&id3, &id2);

        assert!(list.available_tasks().iter().all(|t| t.id != id3));

        list.update(
            &id1,
            Some(TaskStatus::Completed),
            None,
            None,
            None,
            None,
            None,
        );
        assert!(list.available_tasks().iter().all(|t| t.id != id3));

        list.update(
            &id2,
            Some(TaskStatus::Completed),
            None,
            None,
            None,
            None,
            None,
        );
        assert!(list.available_tasks().iter().any(|t| t.id == id3));
    }

    #[test]
    fn get_mut_modifies_task() {
        let mut list = TaskList::new();
        let id = list.create("Mutable".into(), String::new());
        list.get_mut(&id).unwrap().subject = "Modified".into();
        assert_eq!(list.get(&id).unwrap().subject, "Modified");
    }

    #[test]
    fn task_status_all_variants_serde() {
        for status in [
            TaskStatus::Pending,
            TaskStatus::InProgress,
            TaskStatus::Completed,
            TaskStatus::Deleted,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let parsed: TaskStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, status);
        }
    }
}
