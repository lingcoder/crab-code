//! `CronTool` — create, delete, and list in-memory cron jobs.
//!
//! Provides an in-memory cron scheduler that stores jobs with cron
//! expressions and prompts. Jobs are session-scoped and do not persist.

use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt::Write;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

// ── Cron data model ────────────────────────────────────────────────

/// A single cron job entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    pub id: String,
    pub cron: String,
    pub prompt: String,
    pub recurring: bool,
    pub active: bool,
}

/// In-memory cron job store with auto-incrementing IDs.
pub struct CronStore {
    jobs: Vec<CronJob>,
    next_id: u64,
}

impl CronStore {
    #[must_use]
    pub fn new() -> Self {
        Self {
            jobs: Vec::new(),
            next_id: 1,
        }
    }

    pub fn create(&mut self, cron: String, prompt: String, recurring: bool) -> CronJob {
        let id = self.next_id.to_string();
        self.next_id += 1;
        let job = CronJob {
            id,
            cron,
            prompt,
            recurring,
            active: true,
        };
        self.jobs.push(job.clone());
        job
    }

    pub fn delete(&mut self, id: &str) -> Option<CronJob> {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id && j.active) {
            job.active = false;
            Some(job.clone())
        } else {
            None
        }
    }

    #[must_use]
    pub fn list(&self) -> Vec<CronJob> {
        self.jobs.iter().filter(|j| j.active).cloned().collect()
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<&CronJob> {
        self.jobs.iter().find(|j| j.id == id && j.active)
    }
}

impl Default for CronStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Thread-safe shared cron store.
pub type SharedCronStore = Arc<Mutex<CronStore>>;

/// Create a new shared cron store.
#[must_use]
pub fn shared_cron_store() -> SharedCronStore {
    Arc::new(Mutex::new(CronStore::new()))
}

// ── CronCreateTool ─────────────────────────────────────────────────

/// Tool that creates a new cron job.
pub struct CronCreateTool {
    store: SharedCronStore,
}

impl CronCreateTool {
    #[must_use]
    pub fn new(store: SharedCronStore) -> Self {
        Self { store }
    }
}

impl Tool for CronCreateTool {
    fn name(&self) -> &'static str {
        "cron_create"
    }

    fn description(&self) -> &'static str {
        "Create a new cron job with a cron expression and a prompt to execute. \
         Jobs are stored in memory and scoped to the current session. \
         Use standard 5-field cron syntax: minute hour day-of-month month day-of-week."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "cron": {
                    "type": "string",
                    "description": "Standard 5-field cron expression (e.g. '*/5 * * * *' for every 5 minutes)"
                },
                "prompt": {
                    "type": "string",
                    "description": "The prompt to enqueue at each fire time"
                },
                "recurring": {
                    "type": "boolean",
                    "description": "If true (default), fire on every match. If false, fire once then auto-delete."
                }
            },
            "required": ["cron", "prompt"]
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
        let cron_expr = input["cron"].as_str().unwrap_or("").to_owned();
        let prompt = input["prompt"].as_str().unwrap_or("").to_owned();
        let recurring = input["recurring"].as_bool().unwrap_or(true);

        Box::pin(async move {
            if cron_expr.is_empty() {
                return Ok(ToolOutput::error("cron expression is required"));
            }
            if prompt.is_empty() {
                return Ok(ToolOutput::error("prompt is required"));
            }

            // Validate cron expression: must have exactly 5 fields
            if cron_expr.split_whitespace().count() != 5 {
                return Ok(ToolOutput::error(
                    "invalid cron expression: must have exactly 5 fields (minute hour dom month dow)",
                ));
            }

            let job = self
                .store
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .create(cron_expr, prompt, recurring);

            let mut out = String::new();
            let _ = write!(out, "Created cron job #{}", job.id);
            let _ = write!(out, "\ncron: {}", job.cron);
            let _ = write!(out, "\nrecurring: {}", job.recurring);
            let _ = write!(out, "\nprompt: {}", job.prompt);

            Ok(ToolOutput::success(out))
        })
    }
}

// ── CronDeleteTool ─────────────────────────────────────────────────

/// Tool that deletes a cron job by ID.
pub struct CronDeleteTool {
    store: SharedCronStore,
}

impl CronDeleteTool {
    #[must_use]
    pub fn new(store: SharedCronStore) -> Self {
        Self { store }
    }
}

impl Tool for CronDeleteTool {
    fn name(&self) -> &'static str {
        "cron_delete"
    }

    fn description(&self) -> &'static str {
        "Delete a cron job by its ID. The job will no longer fire."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "The ID of the cron job to delete"
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

            let deleted = self
                .store
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .delete(&id);

            match deleted {
                Some(job) => {
                    let mut out = String::new();
                    let _ = write!(out, "Deleted cron job #{}", job.id);
                    let _ = write!(out, "\ncron: {}", job.cron);
                    Ok(ToolOutput::success(out))
                }
                None => Ok(ToolOutput::error(format!(
                    "cron job #{id} not found or already deleted"
                ))),
            }
        })
    }
}

// ── CronListTool ───────────────────────────────────────────────────

/// Tool that lists all active cron jobs.
pub struct CronListTool {
    store: SharedCronStore,
}

impl CronListTool {
    #[must_use]
    pub fn new(store: SharedCronStore) -> Self {
        Self { store }
    }
}

impl Tool for CronListTool {
    fn name(&self) -> &'static str {
        "cron_list"
    }

    fn description(&self) -> &'static str {
        "List all active cron jobs in the current session."
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
            let jobs = self
                .store
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .list();

            if jobs.is_empty() {
                return Ok(ToolOutput::success("No active cron jobs."));
            }

            let mut out = String::new();
            let _ = writeln!(out, "Active cron jobs ({}):", jobs.len());
            for job in &jobs {
                let _ = write!(
                    out,
                    "\n#{} | cron: {} | recurring: {} | prompt: {}",
                    job.id, job.cron, job.recurring, job.prompt
                );
            }

            Ok(ToolOutput::success(out))
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

    fn test_store() -> SharedCronStore {
        shared_cron_store()
    }

    // ── CronStore unit tests ───────────────────────────────────────

    #[test]
    fn store_create_and_list() {
        let mut store = CronStore::new();
        let job = store.create("*/5 * * * *".into(), "check status".into(), true);
        assert_eq!(job.id, "1");
        assert!(job.active);
        assert_eq!(store.list().len(), 1);
    }

    #[test]
    fn store_delete_removes_from_list() {
        let mut store = CronStore::new();
        store.create("0 9 * * *".into(), "morning".into(), true);
        let deleted = store.delete("1");
        assert!(deleted.is_some());
        assert!(store.list().is_empty());
    }

    #[test]
    fn store_delete_nonexistent_returns_none() {
        let mut store = CronStore::new();
        assert!(store.delete("999").is_none());
    }

    #[test]
    fn store_get_active_job() {
        let mut store = CronStore::new();
        store.create("0 * * * *".into(), "hourly".into(), true);
        assert!(store.get("1").is_some());
        store.delete("1");
        assert!(store.get("1").is_none());
    }

    #[test]
    fn store_auto_increments_ids() {
        let mut store = CronStore::new();
        let j1 = store.create("* * * * *".into(), "a".into(), true);
        let j2 = store.create("* * * * *".into(), "b".into(), false);
        assert_eq!(j1.id, "1");
        assert_eq!(j2.id, "2");
    }

    #[test]
    fn shared_store_thread_safe() {
        let store = test_store();
        let s2 = Arc::clone(&store);
        std::thread::spawn(move || {
            s2.lock()
                .unwrap()
                .create("* * * * *".into(), "bg".into(), true);
        })
        .join()
        .unwrap();
        assert_eq!(store.lock().unwrap().list().len(), 1);
    }

    // ── CronCreateTool tests ───────────────────────────────────────

    #[tokio::test]
    async fn create_empty_cron_returns_error() {
        let tool = CronCreateTool::new(test_store());
        let result = tool
            .execute(json!({"cron": "", "prompt": "hi"}), &test_ctx())
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.text().contains("cron expression is required"));
    }

    #[tokio::test]
    async fn create_empty_prompt_returns_error() {
        let tool = CronCreateTool::new(test_store());
        let result = tool
            .execute(json!({"cron": "* * * * *", "prompt": ""}), &test_ctx())
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.text().contains("prompt is required"));
    }

    #[tokio::test]
    async fn create_invalid_cron_fields_returns_error() {
        let tool = CronCreateTool::new(test_store());
        let result = tool
            .execute(json!({"cron": "* * *", "prompt": "hi"}), &test_ctx())
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.text().contains("5 fields"));
    }

    #[tokio::test]
    async fn create_valid_job() {
        let store = test_store();
        let tool = CronCreateTool::new(Arc::clone(&store));
        let result = tool
            .execute(
                json!({"cron": "*/5 * * * *", "prompt": "check deploy"}),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains("Created cron job #1"));
        assert!(text.contains("*/5 * * * *"));
        assert!(text.contains("recurring: true"));
        assert_eq!(store.lock().unwrap().list().len(), 1);
    }

    #[tokio::test]
    async fn create_one_shot_job() {
        let tool = CronCreateTool::new(test_store());
        let result = tool
            .execute(
                json!({"cron": "30 14 5 4 *", "prompt": "remind", "recurring": false}),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.text().contains("recurring: false"));
    }

    // ── CronDeleteTool tests ───────────────────────────────────────

    #[tokio::test]
    async fn delete_empty_id_returns_error() {
        let tool = CronDeleteTool::new(test_store());
        let result = tool.execute(json!({"id": ""}), &test_ctx()).await.unwrap();
        assert!(result.is_error);
        assert!(result.text().contains("id is required"));
    }

    #[tokio::test]
    async fn delete_nonexistent_returns_error() {
        let tool = CronDeleteTool::new(test_store());
        let result = tool
            .execute(json!({"id": "99"}), &test_ctx())
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.text().contains("not found"));
    }

    #[tokio::test]
    async fn delete_existing_job() {
        let store = test_store();
        store
            .lock()
            .unwrap()
            .create("* * * * *".into(), "test".into(), true);

        let tool = CronDeleteTool::new(Arc::clone(&store));
        let result = tool.execute(json!({"id": "1"}), &test_ctx()).await.unwrap();
        assert!(!result.is_error);
        assert!(result.text().contains("Deleted cron job #1"));
        assert!(store.lock().unwrap().list().is_empty());
    }

    // ── CronListTool tests ─────────────────────────────────────────

    #[tokio::test]
    async fn list_empty_store() {
        let tool = CronListTool::new(test_store());
        let result = tool.execute(json!({}), &test_ctx()).await.unwrap();
        assert!(!result.is_error);
        assert!(result.text().contains("No active cron jobs"));
    }

    #[tokio::test]
    async fn list_with_jobs() {
        let store = test_store();
        store
            .lock()
            .unwrap()
            .create("*/5 * * * *".into(), "check".into(), true);
        store
            .lock()
            .unwrap()
            .create("0 9 * * 1-5".into(), "standup".into(), true);

        let tool = CronListTool::new(store);
        let result = tool.execute(json!({}), &test_ctx()).await.unwrap();
        assert!(!result.is_error);
        let text = result.text();
        assert!(text.contains("Active cron jobs (2)"));
        assert!(text.contains("#1"));
        assert!(text.contains("#2"));
        assert!(text.contains("*/5 * * * *"));
        assert!(text.contains("0 9 * * 1-5"));
    }

    // ── Tool metadata tests ────────────────────────────────────────

    #[test]
    fn create_tool_metadata() {
        let tool = CronCreateTool::new(test_store());
        assert_eq!(tool.name(), "cron_create");
        assert!(!tool.is_read_only());
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn delete_tool_metadata() {
        let tool = CronDeleteTool::new(test_store());
        assert_eq!(tool.name(), "cron_delete");
        assert!(!tool.is_read_only());
    }

    #[test]
    fn list_tool_metadata() {
        let tool = CronListTool::new(test_store());
        assert_eq!(tool.name(), "cron_list");
        assert!(tool.is_read_only());
    }

    #[test]
    fn create_schema_has_required_fields() {
        let tool = CronCreateTool::new(test_store());
        let schema = tool.input_schema();
        assert_eq!(schema["required"], json!(["cron", "prompt"]));
        assert!(schema["properties"]["cron"].is_object());
        assert!(schema["properties"]["prompt"].is_object());
        assert!(schema["properties"]["recurring"].is_object());
    }

    #[test]
    fn delete_schema_has_required_fields() {
        let tool = CronDeleteTool::new(test_store());
        let schema = tool.input_schema();
        assert_eq!(schema["required"], json!(["id"]));
    }
}
