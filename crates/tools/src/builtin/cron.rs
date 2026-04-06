use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput, ToolOutputContent};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

// ── Cron data model ────────────────────────────────────────────────────

/// A scheduled cron job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    pub id: String,
    pub cron: String,
    pub prompt: String,
    pub recurring: bool,
    pub durable: bool,
}

/// In-memory cron job store.
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

    pub fn create(
        &mut self,
        cron: String,
        prompt: String,
        recurring: bool,
        durable: bool,
    ) -> CronJob {
        let id = format!("cron_{}", self.next_id);
        self.next_id += 1;
        let job = CronJob {
            id,
            cron,
            prompt,
            recurring,
            durable,
        };
        self.jobs.push(job.clone());
        job
    }

    pub fn delete(&mut self, id: &str) -> bool {
        let len_before = self.jobs.len();
        self.jobs.retain(|j| j.id != id);
        self.jobs.len() < len_before
    }

    #[must_use]
    pub fn list(&self) -> Vec<CronJob> {
        self.jobs.clone()
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<&CronJob> {
        self.jobs.iter().find(|j| j.id == id)
    }
}

impl Default for CronStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Thread-safe shared handle to a `CronStore`.
pub type SharedCronStore = Arc<Mutex<CronStore>>;

/// Create a new shared cron store.
#[must_use]
pub fn shared_cron_store() -> SharedCronStore {
    Arc::new(Mutex::new(CronStore::new()))
}

// ── CronCreateTool ─────────────────────────────────────────────────────

/// Schedule a prompt to be enqueued at a future time.
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
        "Schedule a prompt to be enqueued on a cron schedule"
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "cron": {
                    "type": "string",
                    "description": "Standard 5-field cron expression (minute hour day-of-month month day-of-week)"
                },
                "prompt": {
                    "type": "string",
                    "description": "The prompt to enqueue at each fire time"
                },
                "recurring": {
                    "type": "boolean",
                    "description": "true (default) = fire on every cron match. false = fire once then auto-delete."
                },
                "durable": {
                    "type": "boolean",
                    "description": "true = persist to disk and survive restarts. false (default) = in-memory only."
                }
            },
            "required": ["cron", "prompt"]
        })
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            let cron_expr = input
                .get("cron")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    crab_common::Error::Other("missing required parameter: cron".into())
                })?;

            let prompt = input
                .get("prompt")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    crab_common::Error::Other("missing required parameter: prompt".into())
                })?;

            if cron_expr.trim().is_empty() {
                return Ok(ToolOutput::error("cron expression must not be empty"));
            }

            if prompt.trim().is_empty() {
                return Ok(ToolOutput::error("prompt must not be empty"));
            }

            // Basic validation: 5 fields
            let fields: Vec<&str> = cron_expr.split_whitespace().collect();
            if fields.len() != 5 {
                return Ok(ToolOutput::error(format!(
                    "cron expression must have exactly 5 fields, got {}",
                    fields.len()
                )));
            }

            let recurring = input
                .get("recurring")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true);

            let durable = input
                .get("durable")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);

            let job = {
                let mut store = store.lock().unwrap();
                store.create(cron_expr.to_string(), prompt.to_string(), recurring, durable)
            };

            let result = serde_json::json!({
                "job_id": job.id,
                "cron": job.cron,
                "prompt": job.prompt,
                "recurring": job.recurring,
                "durable": job.durable,
            });

            Ok(ToolOutput::with_content(
                vec![ToolOutputContent::Json { value: result }],
                false,
            ))
        })
    }
}

// ── CronDeleteTool ─────────────────────────────────────────────────────

/// Cancel a previously scheduled cron job.
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
        "Cancel a cron job previously scheduled with CronCreate"
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Job ID returned by CronCreate"
                }
            },
            "required": ["id"]
        })
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            let id = input
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    crab_common::Error::Other("missing required parameter: id".into())
                })?;

            let deleted = {
                let mut store = store.lock().unwrap();
                store.delete(id)
            };

            if deleted {
                let result = serde_json::json!({
                    "deleted": true,
                    "id": id,
                });
                Ok(ToolOutput::with_content(
                    vec![ToolOutputContent::Json { value: result }],
                    false,
                ))
            } else {
                Ok(ToolOutput::error(format!("cron job '{id}' not found")))
            }
        })
    }
}

// ── CronListTool ───────────────────────────────────────────────────────

/// List all scheduled cron jobs.
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
        "List all cron jobs scheduled in this session"
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
            let jobs: Vec<Value> = {
                let store = store.lock().unwrap();
                store
                    .list()
                    .into_iter()
                    .map(|j| {
                        serde_json::json!({
                            "job_id": j.id,
                            "cron": j.cron,
                            "prompt": j.prompt,
                            "recurring": j.recurring,
                            "durable": j.durable,
                        })
                    })
                    .collect()
            };

            Ok(ToolOutput::with_content(
                vec![ToolOutputContent::Json {
                    value: serde_json::json!(jobs),
                }],
                false,
            ))
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::permission::{PermissionMode, PermissionPolicy};
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    fn test_ctx() -> ToolContext {
        ToolContext {
            working_dir: std::path::PathBuf::from("/tmp/project"),
            permission_mode: PermissionMode::Dangerously,
            session_id: "test_session".into(),
            cancellation_token: CancellationToken::new(),
            permission_policy: PermissionPolicy::default(),
        }
    }

    // ─── CronStore unit tests ───

    #[test]
    fn store_create_and_list() {
        let mut store = CronStore::new();
        let job = store.create("0 9 * * *".into(), "morning check".into(), true, false);
        assert_eq!(job.id, "cron_1");
        assert_eq!(store.list().len(), 1);
    }

    #[test]
    fn store_auto_increment_ids() {
        let mut store = CronStore::new();
        let j1 = store.create("* * * * *".into(), "a".into(), true, false);
        let j2 = store.create("* * * * *".into(), "b".into(), true, false);
        assert_eq!(j1.id, "cron_1");
        assert_eq!(j2.id, "cron_2");
    }

    #[test]
    fn store_delete() {
        let mut store = CronStore::new();
        store.create("0 * * * *".into(), "hourly".into(), true, false);
        assert!(store.delete("cron_1"));
        assert!(store.list().is_empty());
    }

    #[test]
    fn store_delete_nonexistent() {
        let mut store = CronStore::new();
        assert!(!store.delete("cron_999"));
    }

    #[test]
    fn store_get() {
        let mut store = CronStore::new();
        store.create("0 9 * * *".into(), "test".into(), false, true);
        let job = store.get("cron_1").unwrap();
        assert_eq!(job.prompt, "test");
        assert!(!job.recurring);
        assert!(job.durable);
        assert!(store.get("cron_999").is_none());
    }

    #[test]
    fn cron_job_serde_roundtrip() {
        let job = CronJob {
            id: "cron_1".into(),
            cron: "0 9 * * *".into(),
            prompt: "do something".into(),
            recurring: true,
            durable: false,
        };
        let json = serde_json::to_string(&job).unwrap();
        let back: CronJob = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "cron_1");
        assert_eq!(back.cron, "0 9 * * *");
        assert!(back.recurring);
    }

    #[test]
    fn shared_store_thread_safe() {
        let store = shared_cron_store();
        let store2 = Arc::clone(&store);
        let handle = std::thread::spawn(move || {
            let mut s = store2.lock().unwrap();
            s.create("0 * * * *".into(), "from thread".into(), true, false);
        });
        handle.join().unwrap();
        let s = store.lock().unwrap();
        assert_eq!(s.list().len(), 1);
    }

    // ─── CronCreateTool ───

    #[test]
    fn cron_create_metadata() {
        let store = shared_cron_store();
        let tool = CronCreateTool::new(store);
        assert_eq!(tool.name(), "cron_create");
        assert!(!tool.requires_confirmation());
        assert!(!tool.is_read_only());
    }

    #[test]
    fn cron_create_schema() {
        let store = shared_cron_store();
        let tool = CronCreateTool::new(store);
        let schema = tool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("cron")));
        assert!(required.contains(&json!("prompt")));
        assert_eq!(required.len(), 2);
    }

    #[tokio::test]
    async fn cron_create_basic() {
        let store = shared_cron_store();
        let tool = CronCreateTool::new(Arc::clone(&store));
        let ctx = test_ctx();
        let input = json!({
            "cron": "0 9 * * *",
            "prompt": "run morning check"
        });
        let output = tool.execute(input, &ctx).await.unwrap();
        assert!(!output.is_error);

        match &output.content[0] {
            ToolOutputContent::Json { value } => {
                assert_eq!(value["job_id"], "cron_1");
                assert_eq!(value["cron"], "0 9 * * *");
                assert_eq!(value["prompt"], "run morning check");
                assert_eq!(value["recurring"], true);
                assert_eq!(value["durable"], false);
            }
            _ => panic!("expected JSON output"),
        }

        // Verify stored
        let s = store.lock().unwrap();
        assert_eq!(s.list().len(), 1);
    }

    #[tokio::test]
    async fn cron_create_one_shot() {
        let store = shared_cron_store();
        let tool = CronCreateTool::new(store);
        let ctx = test_ctx();
        let input = json!({
            "cron": "30 14 6 4 *",
            "prompt": "remind me",
            "recurring": false,
            "durable": true
        });
        let output = tool.execute(input, &ctx).await.unwrap();
        assert!(!output.is_error);

        match &output.content[0] {
            ToolOutputContent::Json { value } => {
                assert_eq!(value["recurring"], false);
                assert_eq!(value["durable"], true);
            }
            _ => panic!("expected JSON output"),
        }
    }

    #[tokio::test]
    async fn cron_create_rejects_empty_cron() {
        let store = shared_cron_store();
        let tool = CronCreateTool::new(store);
        let ctx = test_ctx();
        let input = json!({"cron": "  ", "prompt": "test"});
        let output = tool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("empty"));
    }

    #[tokio::test]
    async fn cron_create_rejects_empty_prompt() {
        let store = shared_cron_store();
        let tool = CronCreateTool::new(store);
        let ctx = test_ctx();
        let input = json!({"cron": "0 * * * *", "prompt": "  "});
        let output = tool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("empty"));
    }

    #[tokio::test]
    async fn cron_create_rejects_invalid_field_count() {
        let store = shared_cron_store();
        let tool = CronCreateTool::new(store);
        let ctx = test_ctx();
        let input = json!({"cron": "0 9 *", "prompt": "test"});
        let output = tool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("5 fields"));
    }

    #[tokio::test]
    async fn cron_create_missing_cron() {
        let store = shared_cron_store();
        let tool = CronCreateTool::new(store);
        let ctx = test_ctx();
        let input = json!({"prompt": "test"});
        let result = tool.execute(input, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn cron_create_missing_prompt() {
        let store = shared_cron_store();
        let tool = CronCreateTool::new(store);
        let ctx = test_ctx();
        let input = json!({"cron": "0 * * * *"});
        let result = tool.execute(input, &ctx).await;
        assert!(result.is_err());
    }

    // ─── CronDeleteTool ───

    #[test]
    fn cron_delete_metadata() {
        let store = shared_cron_store();
        let tool = CronDeleteTool::new(store);
        assert_eq!(tool.name(), "cron_delete");
        assert!(!tool.requires_confirmation());
    }

    #[tokio::test]
    async fn cron_delete_existing() {
        let store = shared_cron_store();
        {
            let mut s = store.lock().unwrap();
            s.create("0 * * * *".into(), "test".into(), true, false);
        }
        let tool = CronDeleteTool::new(Arc::clone(&store));
        let ctx = test_ctx();
        let input = json!({"id": "cron_1"});
        let output = tool.execute(input, &ctx).await.unwrap();
        assert!(!output.is_error);

        match &output.content[0] {
            ToolOutputContent::Json { value } => {
                assert_eq!(value["deleted"], true);
                assert_eq!(value["id"], "cron_1");
            }
            _ => panic!("expected JSON output"),
        }

        let s = store.lock().unwrap();
        assert!(s.list().is_empty());
    }

    #[tokio::test]
    async fn cron_delete_nonexistent() {
        let store = shared_cron_store();
        let tool = CronDeleteTool::new(store);
        let ctx = test_ctx();
        let input = json!({"id": "cron_999"});
        let output = tool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("not found"));
    }

    #[tokio::test]
    async fn cron_delete_missing_id() {
        let store = shared_cron_store();
        let tool = CronDeleteTool::new(store);
        let ctx = test_ctx();
        let input = json!({});
        let result = tool.execute(input, &ctx).await;
        assert!(result.is_err());
    }

    // ─── CronListTool ───

    #[test]
    fn cron_list_metadata() {
        let store = shared_cron_store();
        let tool = CronListTool::new(store);
        assert_eq!(tool.name(), "cron_list");
        assert!(tool.is_read_only());
    }

    #[tokio::test]
    async fn cron_list_empty() {
        let store = shared_cron_store();
        let tool = CronListTool::new(store);
        let ctx = test_ctx();
        let output = tool.execute(json!({}), &ctx).await.unwrap();
        assert!(!output.is_error);

        match &output.content[0] {
            ToolOutputContent::Json { value } => {
                assert!(value.as_array().unwrap().is_empty());
            }
            _ => panic!("expected JSON output"),
        }
    }

    #[tokio::test]
    async fn cron_list_with_jobs() {
        let store = shared_cron_store();
        {
            let mut s = store.lock().unwrap();
            s.create("0 9 * * *".into(), "morning".into(), true, false);
            s.create("0 17 * * *".into(), "evening".into(), true, false);
        }
        let tool = CronListTool::new(store);
        let ctx = test_ctx();
        let output = tool.execute(json!({}), &ctx).await.unwrap();
        assert!(!output.is_error);

        match &output.content[0] {
            ToolOutputContent::Json { value } => {
                let jobs = value.as_array().unwrap();
                assert_eq!(jobs.len(), 2);
                assert_eq!(jobs[0]["job_id"], "cron_1");
                assert_eq!(jobs[1]["job_id"], "cron_2");
            }
            _ => panic!("expected JSON output"),
        }
    }

    // ─── All tools have valid schemas ───

    #[test]
    fn all_cron_tools_have_valid_schemas() {
        let store = shared_cron_store();
        let tools: Vec<Box<dyn Tool>> = vec![
            Box::new(CronCreateTool::new(Arc::clone(&store))),
            Box::new(CronDeleteTool::new(Arc::clone(&store))),
            Box::new(CronListTool::new(store)),
        ];
        for tool in &tools {
            let schema = tool.input_schema();
            assert_eq!(schema["type"], "object");
            assert!(schema["properties"].is_object());
        }
    }
}
