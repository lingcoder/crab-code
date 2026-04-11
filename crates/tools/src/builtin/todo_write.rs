//! `TodoWriteTool` — structured TODO list management.
//!
//! Allows the LLM to create and update a structured TODO list with
//! task descriptions and status tracking.

use std::fmt::Write;
use std::future::Future;
use std::pin::Pin;

use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Tool name constant for `TodoWriteTool`.
pub const TODO_WRITE_TOOL_NAME: &str = "TodoWrite";

/// A single TODO item with task description and status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    /// Description of the task.
    pub task: String,
    /// Status: "pending", "`in_progress`", "completed", "cancelled".
    pub status: String,
}

impl TodoItem {
    /// Valid status values.
    const VALID_STATUSES: &[&str] = &["pending", "in_progress", "completed", "cancelled"];

    /// Check whether the status value is valid.
    #[must_use]
    pub fn is_valid_status(&self) -> bool {
        Self::VALID_STATUSES.contains(&self.status.as_str())
    }
}

/// Structured TODO management tool.
///
/// Input:
/// - `todos`: Array of `TodoItem` objects with `task` and `status` fields
pub struct TodoWriteTool;

impl Tool for TodoWriteTool {
    fn name(&self) -> &'static str {
        TODO_WRITE_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Create or update a structured TODO list. Each item has a task \
         description and a status (pending, in_progress, completed, cancelled). \
         The full list is replaced on each call."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "task": {
                                "type": "string",
                                "description": "Description of the task"
                            },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed", "cancelled"],
                                "description": "Current status of the task"
                            }
                        },
                        "required": ["task", "status"]
                    },
                    "description": "The complete TODO list (replaces the previous list)"
                }
            },
            "required": ["todos"]
        })
    }

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        let count = input["todos"].as_array().map_or(0, Vec::len);
        Some(format!("TodoWrite ({count} items)"))
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let todos_value = input.get("todos").cloned().unwrap_or(Value::Null);

        Box::pin(async move {
            let todos: Vec<TodoItem> = match serde_json::from_value(todos_value) {
                Ok(t) => t,
                Err(e) => {
                    return Ok(ToolOutput::error(format!("invalid todos format: {e}")));
                }
            };

            // Validate statuses
            for item in &todos {
                if !item.is_valid_status() {
                    return Ok(ToolOutput::error(format!(
                        "invalid status '{}' for task '{}'. \
                         Valid: pending, in_progress, completed, cancelled",
                        item.status, item.task
                    )));
                }
            }

            write_todos(&todos).await
        })
    }
}

/// Persist the TODO list.
async fn write_todos(todos: &[TodoItem]) -> Result<ToolOutput> {
    // Serialize the TODO list to JSON.
    let json = serde_json::to_string_pretty(todos)
        .map_err(|e| crab_common::Error::Tool(format!("failed to serialize TODO list: {e}")))?;

    // Write to .crab/todos.json in the current working directory.
    let crab_dir = std::path::Path::new(".crab");
    if !crab_dir.exists() {
        tokio::fs::create_dir_all(crab_dir).await.map_err(|e| {
            crab_common::Error::Tool(format!("failed to create .crab directory: {e}"))
        })?;
    }
    let path = crab_dir.join("todos.json");
    tokio::fs::write(&path, &json).await.map_err(|e| {
        crab_common::Error::Tool(format!("failed to write {}: {e}", path.display()))
    })?;

    // Build a summary of statuses.
    let pending = todos.iter().filter(|t| t.status == "pending").count();
    let in_progress = todos.iter().filter(|t| t.status == "in_progress").count();
    let completed = todos.iter().filter(|t| t.status == "completed").count();
    let cancelled = todos.iter().filter(|t| t.status == "cancelled").count();

    let mut msg = String::new();
    let _ = write!(
        msg,
        "TODO list updated ({} items): \
         {pending} pending, {in_progress} in progress, \
         {completed} completed, {cancelled} cancelled. \
         Saved to {}",
        todos.len(),
        path.display()
    );
    Ok(ToolOutput::success(msg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_metadata() {
        let tool = TodoWriteTool;
        assert_eq!(tool.name(), "TodoWrite");
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn schema_requires_todos() {
        let schema = TodoWriteTool.input_schema();
        assert_eq!(schema["required"], serde_json::json!(["todos"]));
    }

    #[test]
    fn todo_item_valid_statuses() {
        let item = TodoItem {
            task: "test".into(),
            status: "pending".into(),
        };
        assert!(item.is_valid_status());

        let item = TodoItem {
            task: "test".into(),
            status: "invalid".into(),
        };
        assert!(!item.is_valid_status());
    }

    #[test]
    fn todo_item_serde_roundtrip() {
        let item = TodoItem {
            task: "Fix bug".into(),
            status: "in_progress".into(),
        };
        let json = serde_json::to_string(&item).unwrap();
        let parsed: TodoItem = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.task, "Fix bug");
        assert_eq!(parsed.status, "in_progress");
    }
}
