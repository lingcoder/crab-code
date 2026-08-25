use crab_core::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput, ToolOutputContent};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tracing;

use crate::str_utils::truncate_chars;

pub const TASK_CREATE_TOOL_NAME: &str = "TaskCreate";
pub const TASK_LIST_TOOL_NAME: &str = "TaskList";
pub const TASK_GET_TOOL_NAME: &str = "TaskGet";
pub const TASK_UPDATE_TOOL_NAME: &str = "TaskUpdate";
pub const TASK_CLAIM_TOOL_NAME: &str = "TaskClaim";
pub const TASK_STOP_TOOL_NAME: &str = "TaskStop";
pub const TASK_OUTPUT_TOOL_NAME: &str = "TaskOutput";

// ── Task data model ────────────────────────────────────────────────────
//
// The work-queue model lives in `crab_core::task` so these tools and the
// teammate execution side share one definition.

pub use crab_core::task::{SharedTaskList, Task, TaskList, TaskStatus, shared_task_list};

// ── Tool implementations ────────────────────────────────────────────────

/// Task creation tool.
pub struct TaskCreateTool {
    store: SharedTaskList,
}

impl TaskCreateTool {
    #[must_use]
    pub fn new(store: SharedTaskList) -> Self {
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
                let mut list = store.lock().unwrap_or_else(|e| {
                    tracing::warn!("task store mutex poisoned: {e}");
                    e.into_inner()
                });
                let id = list.create(subject.clone(), description);
                serde_json::json!({
                    "id": id,
                    "subject": subject,
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
    store: SharedTaskList,
}

impl TaskListTool {
    #[must_use]
    pub fn new(store: SharedTaskList) -> Self {
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
                let list = store.lock().unwrap_or_else(|e| {
                    tracing::warn!("task store mutex poisoned: {e}");
                    e.into_inner()
                });
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
    store: SharedTaskList,
}

impl TaskGetTool {
    #[must_use]
    pub fn new(store: SharedTaskList) -> Self {
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
            let list = store.lock().unwrap_or_else(|e| {
                tracing::warn!("task store mutex poisoned: {e}");
                e.into_inner()
            });
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
    store: SharedTaskList,
}

impl TaskUpdateTool {
    #[must_use]
    pub fn new(store: SharedTaskList) -> Self {
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
                .map_err(|e| crab_core::Error::Tool(format!("invalid status: {e}")))?;
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

            let deleted = status == Some(TaskStatus::Deleted);
            let found = {
                let mut list = store.lock().unwrap_or_else(|e| {
                    tracing::warn!("task store mutex poisoned: {e}");
                    e.into_inner()
                });
                list.update(
                    task_id,
                    status,
                    subject,
                    description,
                    owner,
                    add_blocked_by,
                    add_blocks,
                )
            };
            Ok(ToolOutput::success(match (found, deleted) {
                (false, _) => format!("Task #{task_id} not found."),
                (true, true) => format!("Task #{task_id} deleted."),
                (true, false) => format!("Updated task #{task_id}"),
            }))
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

/// Task claim tool — takes ownership of the next available task.
///
/// This is what makes a shared queue safe for several agents at once: the
/// check for "is this task free" and the write that takes it happen under one
/// lock, so two agents racing for the same task cannot both win.
pub struct TaskClaimTool {
    store: SharedTaskList,
}

impl TaskClaimTool {
    #[must_use]
    pub fn new(store: SharedTaskList) -> Self {
        Self { store }
    }
}

impl Tool for TaskClaimTool {
    fn name(&self) -> &'static str {
        TASK_CLAIM_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Claim a task from the shared queue so no other agent picks it up. \
         Pass task_id to claim a specific task, or omit it to take the next \
         available one. Returns the claimed task, or reports that nothing was \
         available."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "owner": {
                    "type": "string",
                    "description": "Name to record as the task owner (your agent name)"
                },
                "task_id": {
                    "type": "string",
                    "description": "Specific task to claim; omit to take the next available one"
                }
            },
            "required": ["owner"]
        })
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            let owner = input["owner"].as_str().unwrap_or("").to_string();
            if owner.trim().is_empty() {
                return Ok(ToolOutput::error("owner is required to claim a task"));
            }
            let task_id = input["task_id"].as_str().map(String::from);

            let claimed = {
                let mut list = store.lock().unwrap_or_else(|e| {
                    tracing::warn!("task store mutex poisoned: {e}");
                    e.into_inner()
                });
                match &task_id {
                    Some(id) => list
                        .claim(id, &owner)
                        .then(|| list.get(id).cloned())
                        .flatten(),
                    None => list.claim_next(&owner),
                }
            };

            Ok(match claimed {
                Some(task) => ToolOutput::success(
                    serde_json::to_string_pretty(&task).unwrap_or_else(|_| "{}".into()),
                ),
                None => ToolOutput::success(match &task_id {
                    Some(id) => format!("Task #{id} is not available to claim."),
                    None => "No tasks are available to claim.".to_string(),
                }),
            })
        })
    }

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        Some(match input["task_id"].as_str() {
            Some(id) => format!("TaskClaim (#{id})"),
            None => "TaskClaim (next)".to_string(),
        })
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
        ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let job_registry = ctx.job_registry.clone();
        Box::pin(async move {
            let task_id = input
                .get("task_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    crab_core::Error::Other("missing required parameter: task_id".into())
                })?;

            if task_id.trim().is_empty() {
                return Ok(ToolOutput::error("task_id must not be empty"));
            }

            let Some(reg) = job_registry else {
                return Ok(ToolOutput::error(
                    "no task registry available in this context",
                ));
            };

            {
                let mut reg = reg.lock().unwrap_or_else(|e| {
                    tracing::warn!("task registry mutex poisoned: {e}");
                    e.into_inner()
                });
                let Some(entry) = reg.get(task_id) else {
                    return Ok(ToolOutput::error(format!(
                        "no task found with ID: {task_id}"
                    )));
                };

                if entry.status.is_terminal() {
                    return Ok(ToolOutput::error(format!(
                        "task {task_id} is not running (status: {:?})",
                        entry.status
                    )));
                }

                reg.set_status(task_id, crab_core::job::JobStatus::Killed);
            }
            Ok(ToolOutput::success(format!(
                "successfully stopped task: {task_id}"
            )))
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
        ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let job_registry = ctx.job_registry.clone();
        Box::pin(async move {
            let task_id = input
                .get("task_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    crab_core::Error::Other("missing required parameter: task_id".into())
                })?;

            if task_id.trim().is_empty() {
                return Ok(ToolOutput::error("task_id must not be empty"));
            }

            let block = input
                .get("block")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true);

            let timeout_ms = input
                .get("timeout")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(30_000);

            let Some(reg) = job_registry else {
                return Ok(ToolOutput::error(
                    "no task registry available in this context",
                ));
            };

            // If blocking, poll until the task reaches a terminal state.
            if block {
                let deadline =
                    std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
                loop {
                    {
                        let r = reg.lock().unwrap_or_else(|e| {
                            tracing::warn!("task registry mutex poisoned: {e}");
                            e.into_inner()
                        });
                        if let Some(entry) = r.get(task_id) {
                            if entry.status.is_terminal() {
                                return format_task_output(entry, task_id);
                            }
                        } else {
                            return Ok(ToolOutput::error(format!(
                                "no task found with ID: {task_id}"
                            )));
                        }
                    }
                    if std::time::Instant::now() >= deadline {
                        return Ok(ToolOutput::error(format!(
                            "timed out waiting for task {task_id}"
                        )));
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }

            // Non-blocking: snapshot current state.
            let r = reg.lock().unwrap_or_else(|e| {
                tracing::warn!("task registry mutex poisoned: {e}");
                e.into_inner()
            });
            match r.get(task_id) {
                Some(entry) => format_task_output(entry, task_id),
                None => Ok(ToolOutput::error(format!(
                    "no task found with ID: {task_id}"
                ))),
            }
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

/// Format a task registry entry as a `ToolOutput` for the LLM.
fn format_task_output(entry: &crab_core::job::BackgroundJob, task_id: &str) -> Result<ToolOutput> {
    let mut result = serde_json::json!({
        "task_id": task_id,
        "task_type": entry.job_type,
        "status": entry.status,
        "description": entry.description,
    });

    if let Some(ref path) = entry.output_path {
        // Read output file if it exists.
        if let Ok(output) = std::fs::read_to_string(path) {
            result["output"] = serde_json::Value::String(output);
        }
    }

    if let Some(exit_code) = entry.exit_code {
        result["exit_code"] = serde_json::json!(exit_code);
    }

    if let Some(ref error) = entry.error {
        result["error"] = serde_json::Value::String(error.clone());
    }

    Ok(ToolOutput::with_content(
        vec![ToolOutputContent::Json { value: result }],
        false,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_store_create_and_get() {
        let mut store = TaskList::new();
        let id = store.create("Test".into(), "desc".into());
        let task = store.get(&id).unwrap();
        assert_eq!(task.subject, "Test");
        assert_eq!(task.status, TaskStatus::Pending);
    }

    #[test]
    fn task_store_list_excludes_deleted() {
        let mut store = TaskList::new();
        store.create("Keep".into(), String::new());
        let id2 = store.create("Delete".into(), String::new());
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
        let mut store = TaskList::new();
        let id = store.create("Task".into(), String::new());
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
        let mut store = TaskList::new();
        let id1 = store.create("Blocker".into(), String::new());
        let id2 = store.create("Blocked".into(), String::new());
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
        let store = shared_task_list();
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
        let item = Task {
            id: "1".into(),
            subject: "Test task".into(),
            description: "Do something".into(),
            status: TaskStatus::Pending,
            owner: None,
            blocked_by: vec![],
            blocks: vec![],
        };
        let json = serde_json::to_string(&item).unwrap();
        let back: Task = serde_json::from_str(&json).unwrap();
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
            job_registry: None,
            nested_memory_triggers: Arc::new(tokio::sync::Mutex::new(
                std::collections::HashSet::new(),
            )),
        }
    }

    // ─── TaskClaim ───

    fn claim_input(owner: &str, task_id: Option<&str>) -> Value {
        let mut v = serde_json::json!({ "owner": owner });
        if let Some(id) = task_id {
            v["task_id"] = Value::String(id.into());
        }
        v
    }

    #[tokio::test]
    async fn task_claim_takes_the_next_available_task() {
        let store = shared_task_list();
        let id = store
            .lock()
            .unwrap()
            .create("Write docs".into(), "docs".into());
        let tool = TaskClaimTool::new(Arc::clone(&store));

        let out = tool
            .execute(claim_input("alice", None), &test_ctx())
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.text().contains("Write docs"), "{}", out.text());

        let task = store.lock().unwrap().get(&id).cloned().unwrap();
        let (owner, status) = (task.owner, task.status);
        assert_eq!(owner.as_deref(), Some("alice"));
        assert_eq!(status, TaskStatus::InProgress);
    }

    #[tokio::test]
    async fn two_agents_cannot_claim_the_same_task() {
        let store = shared_task_list();
        store
            .lock()
            .unwrap()
            .create("Only one".into(), String::new());
        let tool = TaskClaimTool::new(Arc::clone(&store));

        let first = tool
            .execute(claim_input("alice", None), &test_ctx())
            .await
            .unwrap();
        let second = tool
            .execute(claim_input("bob", None), &test_ctx())
            .await
            .unwrap();

        assert!(first.text().contains("Only one"));
        assert!(
            second.text().contains("No tasks are available"),
            "the second agent must come away empty: {}",
            second.text()
        );
    }

    #[tokio::test]
    async fn task_claim_can_target_a_specific_task() {
        let store = shared_task_list();
        let (first, second) = {
            let mut list = store.lock().unwrap();
            (
                list.create("First".into(), String::new()),
                list.create("Second".into(), String::new()),
            )
        };
        let tool = TaskClaimTool::new(Arc::clone(&store));

        tool.execute(claim_input("alice", Some(&second)), &test_ctx())
            .await
            .unwrap();

        let second_owner = store.lock().unwrap().get(&second).unwrap().owner.clone();
        let first_owner = store.lock().unwrap().get(&first).unwrap().owner.clone();
        assert_eq!(second_owner.as_deref(), Some("alice"));
        assert!(first_owner.is_none());
    }

    #[tokio::test]
    async fn claiming_an_unavailable_task_reports_it() {
        let store = shared_task_list();
        let id = store.lock().unwrap().create("Taken".into(), String::new());
        let tool = TaskClaimTool::new(Arc::clone(&store));

        tool.execute(claim_input("alice", Some(&id)), &test_ctx())
            .await
            .unwrap();
        let out = tool
            .execute(claim_input("bob", Some(&id)), &test_ctx())
            .await
            .unwrap();

        assert!(!out.is_error);
        assert!(out.text().contains("not available"), "{}", out.text());
    }

    #[tokio::test]
    async fn task_claim_requires_an_owner() {
        let tool = TaskClaimTool::new(shared_task_list());
        let out = tool
            .execute(serde_json::json!({"owner": "  "}), &test_ctx())
            .await
            .unwrap();
        assert!(out.is_error);
    }

    #[test]
    fn task_claim_metadata() {
        let tool = TaskClaimTool::new(shared_task_list());
        assert_eq!(tool.name(), "TaskClaim");
        assert!(!tool.is_read_only());
        assert_eq!(
            tool.format_use_summary(&claim_input("alice", Some("7")))
                .as_deref(),
            Some("TaskClaim (#7)")
        );
        assert_eq!(
            tool.format_use_summary(&claim_input("alice", None))
                .as_deref(),
            Some("TaskClaim (next)")
        );
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
        use crab_core::job::{BackgroundJob, JobRegistry, JobStatus, JobType};
        use std::sync::{Arc, Mutex};

        let registry = Arc::new(Mutex::new(JobRegistry::new()));
        registry.lock().unwrap().register(BackgroundJob::new(
            "task_42".into(),
            JobType::LocalBash,
            "test cmd".into(),
        ));
        registry
            .lock()
            .unwrap()
            .set_status("task_42", JobStatus::Running);

        let mut ctx = test_ctx();
        ctx.job_registry = Some(Arc::clone(&registry));

        let input = serde_json::json!({"task_id": "task_42"});
        let output = TaskStopTool.execute(input, &ctx).await.unwrap();
        assert!(!output.is_error);
        assert!(output.text().contains("successfully stopped"));

        let entry = registry.lock().unwrap().get("task_42").unwrap().status;
        assert_eq!(entry, JobStatus::Killed);
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
        use crab_core::job::{BackgroundJob, JobRegistry, JobType};
        use std::sync::{Arc, Mutex};

        let registry = Arc::new(Mutex::new(JobRegistry::new()));
        registry.lock().unwrap().register(BackgroundJob::new(
            "task_42".into(),
            JobType::LocalBash,
            "test".into(),
        ));

        let mut ctx = test_ctx();
        ctx.job_registry = Some(Arc::clone(&registry));

        let input = serde_json::json!({"task_id": "task_42", "block": false});
        let output = TaskOutputTool.execute(input, &ctx).await.unwrap();
        assert!(!output.is_error);
    }

    #[tokio::test]
    async fn task_output_custom_params() {
        use crab_core::job::{BackgroundJob, JobRegistry, JobType};
        use std::sync::{Arc, Mutex};

        let registry = Arc::new(Mutex::new(JobRegistry::new()));
        registry.lock().unwrap().register(BackgroundJob::new(
            "task_7".into(),
            JobType::LocalBash,
            "test".into(),
        ));

        let mut ctx = test_ctx();
        ctx.job_registry = Some(Arc::clone(&registry));

        let input = serde_json::json!({
            "task_id": "task_7",
            "block": false,
            "timeout": 5000
        });
        let output = TaskOutputTool.execute(input, &ctx).await.unwrap();
        assert!(!output.is_error);
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
