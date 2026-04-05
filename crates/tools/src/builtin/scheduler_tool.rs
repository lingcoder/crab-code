//! `SchedulerTool` — one-shot scheduled reminders.
//!
//! Provides a simple in-memory scheduler for one-time reminders with
//! a description and a target timestamp. Reminders are session-scoped.

use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt::Write;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

// ── Scheduler data model ───────────────────────────────────────────

/// A one-shot scheduled reminder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reminder {
    pub id: String,
    pub message: String,
    pub scheduled_at: String,
    pub fired: bool,
}

/// In-memory reminder store.
pub struct SchedulerStore {
    reminders: Vec<Reminder>,
    next_id: u64,
}

impl SchedulerStore {
    #[must_use]
    pub fn new() -> Self {
        Self {
            reminders: Vec::new(),
            next_id: 1,
        }
    }

    pub fn create(&mut self, message: String, scheduled_at: String) -> Reminder {
        let id = self.next_id.to_string();
        self.next_id += 1;
        let reminder = Reminder {
            id,
            message,
            scheduled_at,
            fired: false,
        };
        self.reminders.push(reminder.clone());
        reminder
    }

    pub fn cancel(&mut self, id: &str) -> Option<Reminder> {
        if let Some(r) = self.reminders.iter_mut().find(|r| r.id == id && !r.fired) {
            r.fired = true;
            Some(r.clone())
        } else {
            None
        }
    }

    #[must_use]
    pub fn list_pending(&self) -> Vec<Reminder> {
        self.reminders
            .iter()
            .filter(|r| !r.fired)
            .cloned()
            .collect()
    }
}

impl Default for SchedulerStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Thread-safe shared scheduler store.
pub type SharedSchedulerStore = Arc<Mutex<SchedulerStore>>;

/// Create a new shared scheduler store.
#[must_use]
pub fn shared_scheduler_store() -> SharedSchedulerStore {
    Arc::new(Mutex::new(SchedulerStore::new()))
}

// ── SchedulerCreateTool ────────────────────────────────────────────

/// Tool that creates a one-shot scheduled reminder.
pub struct SchedulerCreateTool {
    store: SharedSchedulerStore,
}

impl SchedulerCreateTool {
    #[must_use]
    pub fn new(store: SharedSchedulerStore) -> Self {
        Self { store }
    }
}

impl Tool for SchedulerCreateTool {
    fn name(&self) -> &'static str {
        "scheduler_create"
    }

    fn description(&self) -> &'static str {
        "Schedule a one-shot reminder at a specified time. The reminder will \
         fire once and then be marked as completed. Useful for 'remind me at X' \
         style requests. Times should be in ISO 8601 format or a human-readable \
         datetime string."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "The reminder message or prompt to deliver"
                },
                "scheduled_at": {
                    "type": "string",
                    "description": "When to fire the reminder (ISO 8601 or human-readable datetime)"
                }
            },
            "required": ["message", "scheduled_at"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let message = input["message"].as_str().unwrap_or("").to_owned();
        let scheduled_at = input["scheduled_at"].as_str().unwrap_or("").to_owned();

        Box::pin(async move {
            if message.is_empty() {
                return Ok(ToolOutput::error("message is required"));
            }
            if scheduled_at.is_empty() {
                return Ok(ToolOutput::error("scheduled_at is required"));
            }

            let reminder = self
                .store
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .create(message, scheduled_at);

            let mut out = String::new();
            let _ = write!(out, "Scheduled reminder #{}", reminder.id);
            let _ = write!(out, "\ntime: {}", reminder.scheduled_at);
            let _ = write!(out, "\nmessage: {}", reminder.message);

            Ok(ToolOutput::success(out))
        })
    }
}

// ── SchedulerListTool ──────────────────────────────────────────────

/// Tool that lists pending reminders.
pub struct SchedulerListTool {
    store: SharedSchedulerStore,
}

impl SchedulerListTool {
    #[must_use]
    pub fn new(store: SharedSchedulerStore) -> Self {
        Self { store }
    }
}

impl Tool for SchedulerListTool {
    fn name(&self) -> &'static str {
        "scheduler_list"
    }

    fn description(&self) -> &'static str {
        "List all pending (not yet fired) scheduled reminders."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn execute(
        &self,
        _input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        Box::pin(async move {
            let reminders = self
                .store
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .list_pending();

            if reminders.is_empty() {
                return Ok(ToolOutput::success("No pending reminders."));
            }

            let mut out = String::new();
            let _ = writeln!(out, "Pending reminders ({}):", reminders.len());
            for r in &reminders {
                let _ = write!(
                    out,
                    "\n#{} | at: {} | message: {}",
                    r.id, r.scheduled_at, r.message
                );
            }

            Ok(ToolOutput::success(out))
        })
    }
}

// ── SchedulerCancelTool ────────────────────────────────────────────

/// Tool that cancels a pending reminder.
pub struct SchedulerCancelTool {
    store: SharedSchedulerStore,
}

impl SchedulerCancelTool {
    #[must_use]
    pub fn new(store: SharedSchedulerStore) -> Self {
        Self { store }
    }
}

impl Tool for SchedulerCancelTool {
    fn name(&self) -> &'static str {
        "scheduler_cancel"
    }

    fn description(&self) -> &'static str {
        "Cancel a pending scheduled reminder by its ID."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "The ID of the reminder to cancel"
                }
            },
            "required": ["id"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let id = input["id"].as_str().unwrap_or("").to_owned();

        Box::pin(async move {
            if id.is_empty() {
                return Ok(ToolOutput::error("id is required"));
            }

            let cancelled = self
                .store
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .cancel(&id);

            match cancelled {
                Some(r) => {
                    let mut out = String::new();
                    let _ = write!(out, "Cancelled reminder #{}", r.id);
                    let _ = write!(out, "\nmessage: {}", r.message);
                    Ok(ToolOutput::success(out))
                }
                None => Ok(ToolOutput::error(format!(
                    "reminder #{id} not found or already fired"
                ))),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::tool::ToolContext;
    use serde_json::json;
    use std::path::PathBuf;
    use tokio_util::sync::CancellationToken;

    fn test_ctx() -> ToolContext {
        ToolContext {
            working_dir: PathBuf::from("/tmp"),
            permission_mode: crab_core::permission::PermissionMode::Dangerously,
            session_id: "test".into(),
            cancellation_token: CancellationToken::new(),
            permission_policy: crab_core::permission::PermissionPolicy::default(),
        }
    }

    fn test_store() -> SharedSchedulerStore {
        shared_scheduler_store()
    }

    // ── SchedulerStore unit tests ──────────────────────────────────

    #[test]
    fn store_create_and_list() {
        let mut store = SchedulerStore::new();
        let r = store.create("remind me".into(), "2026-04-05T15:00:00".into());
        assert_eq!(r.id, "1");
        assert!(!r.fired);
        assert_eq!(store.list_pending().len(), 1);
    }

    #[test]
    fn store_cancel_removes_from_pending() {
        let mut store = SchedulerStore::new();
        store.create("test".into(), "2026-04-05T15:00:00".into());
        let cancelled = store.cancel("1");
        assert!(cancelled.is_some());
        assert!(store.list_pending().is_empty());
    }

    #[test]
    fn store_cancel_nonexistent_returns_none() {
        let mut store = SchedulerStore::new();
        assert!(store.cancel("999").is_none());
    }

    #[test]
    fn store_auto_increments_ids() {
        let mut store = SchedulerStore::new();
        let r1 = store.create("a".into(), "t1".into());
        let r2 = store.create("b".into(), "t2".into());
        assert_eq!(r1.id, "1");
        assert_eq!(r2.id, "2");
    }

    // ── SchedulerCreateTool tests ──────────────────────────────────

    #[tokio::test]
    async fn create_empty_message_returns_error() {
        let tool = SchedulerCreateTool::new(test_store());
        let result = tool
            .execute(
                json!({"message": "", "scheduled_at": "2026-04-05T15:00:00"}),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.text().contains("message is required"));
    }

    #[tokio::test]
    async fn create_empty_time_returns_error() {
        let tool = SchedulerCreateTool::new(test_store());
        let result = tool
            .execute(json!({"message": "hi", "scheduled_at": ""}), &test_ctx())
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.text().contains("scheduled_at is required"));
    }

    #[tokio::test]
    async fn create_valid_reminder() {
        let store = test_store();
        let tool = SchedulerCreateTool::new(Arc::clone(&store));
        let result = tool
            .execute(
                json!({"message": "check deploy", "scheduled_at": "2026-04-05T15:00:00"}),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains("Scheduled reminder #1"));
        assert!(text.contains("2026-04-05T15:00:00"));
        assert!(text.contains("check deploy"));
        assert_eq!(store.lock().unwrap().list_pending().len(), 1);
    }

    // ── SchedulerListTool tests ────────────────────────────────────

    #[tokio::test]
    async fn list_empty() {
        let tool = SchedulerListTool::new(test_store());
        let result = tool.execute(json!({}), &test_ctx()).await.unwrap();
        assert!(!result.is_error);
        assert!(result.text().contains("No pending reminders"));
    }

    #[tokio::test]
    async fn list_with_reminders() {
        let store = test_store();
        store
            .lock()
            .unwrap()
            .create("meeting".into(), "2026-04-05T14:00:00".into());
        store
            .lock()
            .unwrap()
            .create("lunch".into(), "2026-04-05T12:00:00".into());

        let tool = SchedulerListTool::new(store);
        let result = tool.execute(json!({}), &test_ctx()).await.unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains("Pending reminders (2)"));
        assert!(text.contains("#1"));
        assert!(text.contains("#2"));
    }

    // ── SchedulerCancelTool tests ──────────────────────────────────

    #[tokio::test]
    async fn cancel_empty_id_returns_error() {
        let tool = SchedulerCancelTool::new(test_store());
        let result = tool.execute(json!({"id": ""}), &test_ctx()).await.unwrap();
        assert!(result.is_error);
        assert!(result.text().contains("id is required"));
    }

    #[tokio::test]
    async fn cancel_nonexistent_returns_error() {
        let tool = SchedulerCancelTool::new(test_store());
        let result = tool
            .execute(json!({"id": "99"}), &test_ctx())
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.text().contains("not found"));
    }

    #[tokio::test]
    async fn cancel_existing_reminder() {
        let store = test_store();
        store
            .lock()
            .unwrap()
            .create("test".into(), "2026-04-05T15:00:00".into());

        let tool = SchedulerCancelTool::new(Arc::clone(&store));
        let result = tool.execute(json!({"id": "1"}), &test_ctx()).await.unwrap();
        assert!(!result.is_error);
        assert!(result.text().contains("Cancelled reminder #1"));
        assert!(store.lock().unwrap().list_pending().is_empty());
    }

    // ── Tool metadata tests ────────────────────────────────────────

    #[test]
    fn create_tool_metadata() {
        let tool = SchedulerCreateTool::new(test_store());
        assert_eq!(tool.name(), "scheduler_create");
        assert!(!tool.is_read_only());
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn list_tool_metadata() {
        let tool = SchedulerListTool::new(test_store());
        assert_eq!(tool.name(), "scheduler_list");
        assert!(tool.is_read_only());
    }

    #[test]
    fn cancel_tool_metadata() {
        let tool = SchedulerCancelTool::new(test_store());
        assert_eq!(tool.name(), "scheduler_cancel");
        assert!(!tool.is_read_only());
    }

    #[test]
    fn create_schema_has_required_fields() {
        let tool = SchedulerCreateTool::new(test_store());
        let schema = tool.input_schema();
        assert_eq!(schema["required"], json!(["message", "scheduled_at"]));
        assert!(schema["properties"]["message"].is_object());
        assert!(schema["properties"]["scheduled_at"].is_object());
    }

    #[test]
    fn cancel_schema_has_required_fields() {
        let tool = SchedulerCancelTool::new(test_store());
        let schema = tool.input_schema();
        assert_eq!(schema["required"], json!(["id"]));
    }
}
