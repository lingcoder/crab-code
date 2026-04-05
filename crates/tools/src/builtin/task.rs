use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

// ── Task data model (self-contained within tools crate) ─────────────────

/// Status of a task.
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

/// A single task item.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskItem {
    pub id: String,
    pub subject: String,
    pub description: String,
    pub status: TaskStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<String>,
}

/// In-memory task store with auto-incrementing IDs.
pub struct TaskStore {
    tasks: Vec<TaskItem>,
    next_id: u64,
}

impl TaskStore {
    #[must_use]
    pub fn new() -> Self {
        Self {
            tasks: Vec::new(),
            next_id: 1,
        }
    }

    pub fn create(&mut self, subject: String, description: String) -> TaskItem {
        let id = self.next_id.to_string();
        self.next_id += 1;
        self.tasks.push(TaskItem {
            id,
            subject,
            description,
            status: TaskStatus::Pending,
            owner: None,
            blocked_by: Vec::new(),
            blocks: Vec::new(),
        });
        self.tasks.last().unwrap().clone()
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<&TaskItem> {
        self.tasks
            .iter()
            .find(|t| t.id == id && t.status != TaskStatus::Deleted)
    }

    fn get_mut(&mut self, id: &str) -> Option<&mut TaskItem> {
        self.tasks
            .iter_mut()
            .find(|t| t.id == id && t.status != TaskStatus::Deleted)
    }

    #[must_use]
    pub fn list(&self) -> Vec<TaskItem> {
        self.tasks
            .iter()
            .filter(|t| t.status != TaskStatus::Deleted)
            .cloned()
            .collect()
    }

    /// Update task fields. Returns the updated task summary or None.
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
    ) -> Option<String> {
        // Handle deletion
        if status == Some(TaskStatus::Deleted) {
            if let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) {
                task.status = TaskStatus::Deleted;
                return Some(format!("Task #{id} deleted."));
            }
            return None;
        }

        let task = self.get_mut(id)?;
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
        if let Some(deps) = &add_blocked_by {
            for dep in deps {
                if !task.blocked_by.contains(dep) {
                    task.blocked_by.push(dep.clone());
                }
            }
        }
        let task_id_owned = task.id.clone();
        let summary = format!("Updated task #{}", task.id);

        // Handle add_blocks: add reverse deps
        if let Some(blocked_ids) = &add_blocks {
            for blocked_id in blocked_ids {
                if let Some(t) = self.get_mut(&task_id_owned)
                    && !t.blocks.contains(blocked_id)
                {
                    t.blocks.push(blocked_id.clone());
                }
                if let Some(blocked) = self.get_mut(blocked_id)
                    && !blocked.blocked_by.contains(&task_id_owned)
                {
                    blocked.blocked_by.push(task_id_owned.clone());
                }
            }
        }
        // Handle add_blocked_by reverse: add to blocker's blocks list
        if let Some(blocker_ids) = &add_blocked_by {
            for blocker_id in blocker_ids {
                if let Some(blocker) = self.get_mut(blocker_id)
                    && !blocker.blocks.contains(&task_id_owned)
                {
                    blocker.blocks.push(task_id_owned.clone());
                }
            }
        }

        Some(summary)
    }
}

impl Default for TaskStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Thread-safe shared handle to a `TaskStore`.
pub type SharedTaskStore = Arc<Mutex<TaskStore>>;

/// Create a new shared task store.
#[must_use]
pub fn shared_task_store() -> SharedTaskStore {
    Arc::new(Mutex::new(TaskStore::new()))
}

// ── Tool implementations ────────────────────────────────────────────────

/// Task creation tool.
pub struct TaskCreateTool {
    store: SharedTaskStore,
}

impl TaskCreateTool {
    #[must_use]
    pub fn new(store: SharedTaskStore) -> Self {
        Self { store }
    }
}

impl Tool for TaskCreateTool {
    fn name(&self) -> &'static str {
        "task_create"
    }

    fn description(&self) -> &'static str {
        "Create a new task in the task list"
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "subject": { "type": "string", "description": "Brief title for the task" },
                "description": { "type": "string", "description": "What needs to be done" }
            },
            "required": ["subject", "description"]
        })
    }

    #[allow(clippy::significant_drop_tightening)]
    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            let subject = input["subject"].as_str().unwrap_or("").to_string();
            let description = input["description"].as_str().unwrap_or("").to_string();

            let response = {
                let mut list = store.lock().unwrap();
                let task = list.create(subject, description);
                serde_json::json!({
                    "id": task.id,
                    "subject": task.subject,
                    "status": "pending"
                })
            };
            Ok(ToolOutput::success(response.to_string()))
        })
    }
}

/// Task listing tool.
pub struct TaskListTool {
    store: SharedTaskStore,
}

impl TaskListTool {
    #[must_use]
    pub fn new(store: SharedTaskStore) -> Self {
        Self { store }
    }
}

impl Tool for TaskListTool {
    fn name(&self) -> &'static str {
        "task_list"
    }

    fn description(&self) -> &'static str {
        "List all tasks with their status"
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {}
        })
    }

    fn execute(
        &self,
        _input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            let summary: Vec<Value> = {
                let list = store.lock().unwrap();
                list.list()
                    .into_iter()
                    .map(|t| {
                        serde_json::json!({
                            "id": t.id,
                            "subject": t.subject,
                            "status": t.status,
                            "owner": t.owner,
                            "blockedBy": t.blocked_by,
                        })
                    })
                    .collect()
            };
            Ok(ToolOutput::success(
                serde_json::to_string_pretty(&summary).unwrap_or_else(|_| "[]".into()),
            ))
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

/// Task retrieval tool.
pub struct TaskGetTool {
    store: SharedTaskStore,
}

impl TaskGetTool {
    #[must_use]
    pub fn new(store: SharedTaskStore) -> Self {
        Self { store }
    }
}

impl Tool for TaskGetTool {
    fn name(&self) -> &'static str {
        "task_get"
    }

    fn description(&self) -> &'static str {
        "Get full details of a specific task"
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task_id": { "type": "string", "description": "The ID of the task to retrieve" }
            },
            "required": ["task_id"]
        })
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            let task_id = input["task_id"].as_str().unwrap_or("");
            #[allow(clippy::significant_drop_tightening)]
            let list = store.lock().unwrap();
            list.get(task_id).map_or_else(
                || Ok(ToolOutput::success(format!("Task #{task_id} not found."))),
                |task| {
                    let json = serde_json::to_string_pretty(task).unwrap_or_else(|_| "{}".into());
                    Ok(ToolOutput::success(json))
                },
            )
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

/// Task update tool.
pub struct TaskUpdateTool {
    store: SharedTaskStore,
}

impl TaskUpdateTool {
    #[must_use]
    pub fn new(store: SharedTaskStore) -> Self {
        Self { store }
    }
}

impl Tool for TaskUpdateTool {
    fn name(&self) -> &'static str {
        "task_update"
    }

    fn description(&self) -> &'static str {
        "Update an existing task's status, subject, description, owner, or dependencies"
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task_id": { "type": "string", "description": "The ID of the task to update" },
                "status": { "type": "string", "enum": ["pending", "in_progress", "completed", "deleted"] },
                "subject": { "type": "string" },
                "description": { "type": "string" },
                "owner": { "type": "string" },
                "add_blocked_by": { "type": "array", "items": { "type": "string" } },
                "add_blocks": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["task_id"]
        })
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            let task_id = input["task_id"].as_str().unwrap_or("");
            let status = input["status"]
                .as_str()
                .map(|s| serde_json::from_value::<TaskStatus>(Value::String(s.into())))
                .transpose()
                .map_err(|e| crab_common::Error::Tool(format!("invalid status: {e}")))?;
            let subject = input["subject"].as_str().map(String::from);
            let description = input["description"].as_str().map(String::from);
            let owner = input["owner"].as_str().map(String::from);
            let add_blocked_by = input["add_blocked_by"].as_array().map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            });
            let add_blocks = input["add_blocks"].as_array().map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            });

            #[allow(clippy::significant_drop_tightening)]
            let mut list = store.lock().unwrap();
            list.update(
                task_id,
                status,
                subject,
                description,
                owner,
                add_blocked_by,
                add_blocks,
            )
            .map_or_else(
                || Ok(ToolOutput::success(format!("Task #{task_id} not found."))),
                |msg| Ok(ToolOutput::success(msg)),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_store_create_and_get() {
        let mut store = TaskStore::new();
        let id = store.create("Test".into(), "desc".into()).id.clone();
        let task = store.get(&id).unwrap();
        assert_eq!(task.subject, "Test");
        assert_eq!(task.status, TaskStatus::Pending);
    }

    #[test]
    fn task_store_list_excludes_deleted() {
        let mut store = TaskStore::new();
        store.create("Keep".into(), "".into());
        let id2 = store.create("Delete".into(), "".into()).id.clone();
        store.update(
            &id2,
            Some(TaskStatus::Deleted),
            None,
            None,
            None,
            None,
            None,
        );
        assert_eq!(store.list().len(), 1);
    }

    #[test]
    fn task_store_update_status() {
        let mut store = TaskStore::new();
        let id = store.create("Task".into(), "".into()).id.clone();
        store.update(
            &id,
            Some(TaskStatus::InProgress),
            None,
            None,
            Some("me".into()),
            None,
            None,
        );
        let task = store.get(&id).unwrap();
        assert_eq!(task.status, TaskStatus::InProgress);
        assert_eq!(task.owner.as_deref(), Some("me"));
    }

    #[test]
    fn task_store_dependencies() {
        let mut store = TaskStore::new();
        let id1 = store.create("Blocker".into(), "".into()).id.clone();
        let id2 = store.create("Blocked".into(), "".into()).id.clone();
        store.update(&id2, None, None, None, None, Some(vec![id1.clone()]), None);

        let blocked = store.get(&id2).unwrap();
        assert!(blocked.blocked_by.contains(&id1));

        let blocker = store.get(&id1).unwrap();
        assert!(blocker.blocks.contains(&id2));
    }

    #[test]
    fn task_status_serde() {
        let json = serde_json::to_string(&TaskStatus::InProgress).unwrap();
        assert_eq!(json, r#""in_progress""#);
        let back: TaskStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back, TaskStatus::InProgress);
    }

    #[test]
    fn shared_store_thread_safe() {
        let store = shared_task_store();
        let store2 = Arc::clone(&store);
        let handle = std::thread::spawn(move || {
            let mut list = store2.lock().unwrap();
            list.create("From thread".into(), "".into());
        });
        handle.join().unwrap();
        let list = store.lock().unwrap();
        assert_eq!(list.list().len(), 1);
    }

    #[test]
    fn task_item_serde_roundtrip() {
        let item = TaskItem {
            id: "1".into(),
            subject: "Test task".into(),
            description: "Do something".into(),
            status: TaskStatus::Pending,
            owner: None,
            blocked_by: vec![],
            blocks: vec![],
        };
        let json = serde_json::to_string(&item).unwrap();
        let back: TaskItem = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "1");
        assert_eq!(back.subject, "Test task");
    }
}
