use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput, ToolOutputContent};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use crate::str_utils::truncate_chars;

pub const TASK_CREATE_TOOL_NAME: &str = "TaskCreate";
pub const TASK_LIST_TOOL_NAME: &str = "TaskList";
pub const TASK_GET_TOOL_NAME: &str = "TaskGet";
pub const TASK_UPDATE_TOOL_NAME: &str = "TaskUpdate";
pub const TASK_STOP_TOOL_NAME: &str = "TaskStop";
pub const TASK_OUTPUT_TOOL_NAME: &str = "TaskOutput";

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
        TASK_CREATE_TOOL_NAME
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

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        // Subjects are LLM-authored and can contain any Unicode (CJK, emoji);
        // truncate_chars counts codepoints to avoid panics on multi-byte input.
        let subject = input["subject"].as_str().unwrap_or("?");
        let truncated = truncate_chars(subject, 57, "…");
        Some(format!("TaskCreate ({truncated})"))
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
        TASK_LIST_TOOL_NAME
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

    fn format_use_summary(&self, _input: &Value) -> Option<String> {
        Some("TaskList".to_string())
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
        TASK_GET_TOOL_NAME
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

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        let id = input["taskId"]
            .as_str()
            .or_else(|| input["task_id"].as_str())
            .unwrap_or("?");
        Some(format!("TaskGet (#{id})"))
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
        TASK_UPDATE_TOOL_NAME
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

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        let id = input["taskId"]
            .as_str()
            .or_else(|| input["task_id"].as_str())
            .unwrap_or("?");
        let status = input["status"].as_str().unwrap_or("");
        if status.is_empty() {
            Some(format!("TaskUpdate (#{id})"))
        } else {
            Some(format!("TaskUpdate (#{id} → {status})"))
        }
    }
}

/// Task stop tool — requests cancellation of a background task.
///
/// Returns a structured JSON action for the agent layer to intercept
/// and cancel the corresponding worker via `CancellationToken`.
pub struct TaskStopTool;

impl Tool for TaskStopTool {
    fn name(&self) -> &'static str {
        TASK_STOP_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Stop a running background task by its ID"
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "The ID of the task to stop"
                }
            },
            "required": ["task_id"]
        })
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        Box::pin(async move {
            let task_id = input
                .get("task_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    crab_common::Error::Other("missing required parameter: task_id".into())
                })?;

            if task_id.trim().is_empty() {
                return Ok(ToolOutput::error("task_id must not be empty"));
            }

            let result = serde_json::json!({
                "action": "task_stop",
                "stopped": true,
                "task_id": task_id,
            });

            Ok(ToolOutput::with_content(
                vec![ToolOutputContent::Json { value: result }],
                false,
            ))
        })
    }

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        let id = input["task_id"].as_str().unwrap_or("?");
        Some(format!("TaskStop (#{id})"))
    }
}

/// Task output tool — retrieves the output of a background task.
///
/// Returns a structured JSON action for the agent layer to intercept
/// and fetch output from the corresponding worker.
pub struct TaskOutputTool;

impl Tool for TaskOutputTool {
    fn name(&self) -> &'static str {
        TASK_OUTPUT_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Get the output of a background task"
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "The ID of the task to get output from"
                },
                "block": {
                    "type": "boolean",
                    "description": "Whether to wait for the task to complete (default: true)"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in milliseconds when blocking (default: 30000)"
                }
            },
            "required": ["task_id"]
        })
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        Box::pin(async move {
            let task_id = input
                .get("task_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    crab_common::Error::Other("missing required parameter: task_id".into())
                })?;

            if task_id.trim().is_empty() {
                return Ok(ToolOutput::error("task_id must not be empty"));
            }

            let block = input
                .get("block")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true);

            let timeout = input
                .get("timeout")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(30_000);

            let result = serde_json::json!({
                "action": "task_output",
                "task_id": task_id,
                "block": block,
                "timeout_ms": timeout,
            });

            Ok(ToolOutput::with_content(
                vec![ToolOutputContent::Json { value: result }],
                false,
            ))
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        let id = input["task_id"].as_str().unwrap_or("?");
        Some(format!("TaskOutput (#{id})"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_store_create_and_get() {
        let mut store = TaskStore::new();
        let id = store.create("Test".into(), "desc".into()).id;
        let task = store.get(&id).unwrap();
        assert_eq!(task.subject, "Test");
        assert_eq!(task.status, TaskStatus::Pending);
    }

    #[test]
    fn task_store_list_excludes_deleted() {
        let mut store = TaskStore::new();
        store.create("Keep".into(), String::new());
        let id2 = store.create("Delete".into(), String::new()).id;
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
        let id = store.create("Task".into(), String::new()).id;
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
        let id1 = store.create("Blocker".into(), String::new()).id;
        let id2 = store.create("Blocked".into(), String::new()).id;
        store.update(&id2, None, None, None, None, Some(vec![id1.clone()]), None);

        let downstream = store.get(&id2).unwrap();
        assert!(downstream.blocked_by.contains(&id1));

        let upstream = store.get(&id1).unwrap();
        assert!(upstream.blocks.contains(&id2));
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
            list.create("From thread".into(), String::new());
        });
        handle.join().unwrap();
        let len = store.lock().unwrap().list().len();
        assert_eq!(len, 1);
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

    // ─── TaskStopTool ───

    fn test_ctx() -> crab_core::tool::ToolContext {
        crab_core::tool::ToolContext {
            working_dir: std::path::PathBuf::from("/tmp"),
            permission_mode: crab_core::permission::PermissionMode::Dangerously,
            session_id: "test".into(),
            cancellation_token: tokio_util::sync::CancellationToken::new(),
            permission_policy: crab_core::permission::PermissionPolicy::default(),
            ext: crab_core::tool::ToolContextExt::default(),
        }
    }

    #[test]
    fn task_stop_metadata() {
        let tool = TaskStopTool;
        assert_eq!(tool.name(), "TaskStop");
        assert!(!tool.is_read_only());
    }

    #[test]
    fn task_stop_schema() {
        let schema = TaskStopTool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("task_id")));
    }

    #[tokio::test]
    async fn task_stop_basic() {
        let ctx = test_ctx();
        let input = serde_json::json!({"task_id": "task_42"});
        let output = TaskStopTool.execute(input, &ctx).await.unwrap();
        assert!(!output.is_error);

        match &output.content[0] {
            ToolOutputContent::Json { value } => {
                assert_eq!(value["action"], "task_stop");
                assert_eq!(value["stopped"], true);
                assert_eq!(value["task_id"], "task_42");
            }
            _ => panic!("expected JSON output"),
        }
    }

    #[tokio::test]
    async fn task_stop_empty_id() {
        let ctx = test_ctx();
        let input = serde_json::json!({"task_id": "  "});
        let output = TaskStopTool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("empty"));
    }

    #[tokio::test]
    async fn task_stop_missing_id() {
        let ctx = test_ctx();
        let input = serde_json::json!({});
        let result = TaskStopTool.execute(input, &ctx).await;
        assert!(result.is_err());
    }

    // ─── TaskOutputTool ───

    #[test]
    fn task_output_metadata() {
        let tool = TaskOutputTool;
        assert_eq!(tool.name(), "TaskOutput");
        assert!(tool.is_read_only());
    }

    #[test]
    fn task_output_schema() {
        let schema = TaskOutputTool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("task_id")));
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("block"));
        assert!(props.contains_key("timeout"));
    }

    #[tokio::test]
    async fn task_output_basic() {
        let ctx = test_ctx();
        let input = serde_json::json!({"task_id": "task_42"});
        let output = TaskOutputTool.execute(input, &ctx).await.unwrap();
        assert!(!output.is_error);

        match &output.content[0] {
            ToolOutputContent::Json { value } => {
                assert_eq!(value["action"], "task_output");
                assert_eq!(value["task_id"], "task_42");
                assert_eq!(value["block"], true);
                assert_eq!(value["timeout_ms"], 30000);
            }
            _ => panic!("expected JSON output"),
        }
    }

    #[tokio::test]
    async fn task_output_custom_params() {
        let ctx = test_ctx();
        let input = serde_json::json!({
            "task_id": "task_7",
            "block": false,
            "timeout": 5000
        });
        let output = TaskOutputTool.execute(input, &ctx).await.unwrap();
        assert!(!output.is_error);

        match &output.content[0] {
            ToolOutputContent::Json { value } => {
                assert_eq!(value["block"], false);
                assert_eq!(value["timeout_ms"], 5000);
            }
            _ => panic!("expected JSON output"),
        }
    }

    #[tokio::test]
    async fn task_output_empty_id() {
        let ctx = test_ctx();
        let input = serde_json::json!({"task_id": "  "});
        let output = TaskOutputTool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
    }

    #[tokio::test]
    async fn task_output_missing_id() {
        let ctx = test_ctx();
        let input = serde_json::json!({});
        let result = TaskOutputTool.execute(input, &ctx).await;
        assert!(result.is_err());
    }
}
