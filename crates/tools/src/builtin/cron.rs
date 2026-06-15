use crab_core::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput, ToolOutputContent};
use crab_cron::{JobHandler, JobId, JobScheduler, JobSpec};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

pub const CRON_CREATE_TOOL_NAME: &str = "CronCreate";
pub const CRON_DELETE_TOOL_NAME: &str = "CronDelete";
pub const CRON_LIST_TOOL_NAME: &str = "CronList";

// ── Cron data model ────────────────────────────────────────────────────

/// A scheduled cron job's user-facing metadata.
///
/// `scheduler_id` links this record to the live [`crab_cron::JobScheduler`]
/// entry so deletes can cancel the underlying task. It is not persisted —
/// scheduler ids are reallocated fresh on each process start.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    pub id: String,
    pub cron: String,
    pub prompt: String,
    pub recurring: bool,
    pub durable: bool,
    #[serde(skip)]
    scheduler_id: Option<JobId>,
}

// ── Fired-prompt queue ─────────────────────────────────────────────────

/// A prompt a cron job emitted when it fired, awaiting injection into the
/// session by the runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FiredPrompt {
    /// User-facing cron job id that produced this prompt.
    pub job_id: String,
    /// The prompt text to inject.
    pub prompt: String,
}

/// Queue of prompts fired by cron jobs, drained by the runtime.
///
/// Cron handlers run on the scheduler's tokio tasks with no access to the
/// session loop, so a fired job cannot inject a prompt directly. Instead it
/// pushes here; the engine polls [`Self::drain`] each turn and feeds the
/// prompts into the conversation. Cheap to clone (internally `Arc`ed).
#[derive(Clone, Default)]
pub struct FiredPromptQueue {
    inner: Arc<Mutex<VecDeque<FiredPrompt>>>,
}

impl FiredPromptQueue {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Enqueue a fired prompt.
    pub fn push(&self, prompt: FiredPrompt) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(prompt);
    }

    /// Remove and return every queued prompt, leaving the queue empty.
    pub fn drain(&self) -> Vec<FiredPrompt> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain(..)
            .collect()
    }

    /// Number of prompts currently queued.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// Whether the queue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ── Persistence ────────────────────────────────────────────────────────

/// Path to the durable cron job file: `<global_config_dir>/cron/jobs.json`.
fn durable_jobs_path() -> std::path::PathBuf {
    crab_config::config::global_config_dir()
        .join("cron")
        .join("jobs.json")
}

/// Persisted shape — only the fields that survive a restart.
#[derive(Serialize, Deserialize)]
struct PersistedJob {
    id: String,
    cron: String,
    prompt: String,
    recurring: bool,
}

fn load_persisted() -> Vec<PersistedJob> {
    let path = durable_jobs_path();
    let Ok(bytes) = std::fs::read(&path) else {
        return Vec::new();
    };
    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        tracing::warn!(path = %path.display(), "failed to parse durable cron jobs: {e}");
        Vec::new()
    })
}

fn save_persisted(jobs: &[PersistedJob]) {
    let path = durable_jobs_path();
    let Some(parent) = path.parent() else {
        return;
    };
    if let Err(e) = std::fs::create_dir_all(parent) {
        tracing::warn!(path = %parent.display(), "failed to create cron dir: {e}");
        return;
    }
    match serde_json::to_vec_pretty(jobs) {
        Ok(bytes) => {
            if let Err(e) = std::fs::write(&path, bytes) {
                tracing::warn!(path = %path.display(), "failed to write durable cron jobs: {e}");
            }
        }
        Err(e) => tracing::warn!("failed to serialize durable cron jobs: {e}"),
    }
}

// ── Cron store ─────────────────────────────────────────────────────────

/// The handler the scheduler invokes on every fire of a cron job.
///
/// Pushes the job's prompt onto the shared [`FiredPromptQueue`]. The runtime
/// drains the queue and injects the prompts. One-shot (`recurring == false`)
/// jobs are removed from the store by the firing path.
struct CronFireHandler {
    job_id: String,
    prompt: String,
    queue: FiredPromptQueue,
}

impl JobHandler for CronFireHandler {
    fn run(&self, _scheduler_id: JobId) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            self.queue.push(FiredPrompt {
                job_id: self.job_id.clone(),
                prompt: self.prompt.clone(),
            });
        })
    }
}

/// Live cron store backed by the real [`JobScheduler`].
///
/// Holds the user-facing job metadata, the scheduler that actually fires
/// jobs, and the [`FiredPromptQueue`] the runtime drains. Durable jobs are
/// reloaded from disk and re-registered with the scheduler at construction.
pub struct CronStore {
    jobs: Vec<CronJob>,
    next_id: u64,
    scheduler: JobScheduler,
    queue: FiredPromptQueue,
}

impl CronStore {
    /// Build a store with a fresh scheduler and empty fired-prompt queue.
    ///
    /// Does not load durable jobs — use [`Self::load`] to also re-register
    /// persisted durable jobs with the scheduler.
    #[must_use]
    pub fn new() -> Self {
        Self {
            jobs: Vec::new(),
            next_id: 1,
            scheduler: JobScheduler::new(),
            queue: FiredPromptQueue::new(),
        }
    }

    /// Clone of the fired-prompt queue, for the runtime to drain.
    #[must_use]
    pub fn fired_queue(&self) -> FiredPromptQueue {
        self.queue.clone()
    }

    /// Schedule a new job with the real scheduler. Returns the created
    /// metadata, or an error if the cron expression is rejected.
    ///
    /// Durable jobs are written through to disk. Non-recurring jobs still
    /// fire on every cron match here; the runtime is responsible for calling
    /// [`Self::delete`] after consuming a one-shot fire.
    pub async fn create(
        &mut self,
        cron: String,
        prompt: String,
        recurring: bool,
        durable: bool,
    ) -> std::result::Result<CronJob, String> {
        let id = format!("cron_{}", self.next_id);

        let handler: Arc<dyn JobHandler> = Arc::new(CronFireHandler {
            job_id: id.clone(),
            prompt: prompt.clone(),
            queue: self.queue.clone(),
        });
        let scheduler_id = self
            .scheduler
            .schedule(
                JobSpec::Cron {
                    expression: cron.clone(),
                },
                handler,
            )
            .await
            .map_err(|e| e.to_string())?;

        self.next_id += 1;
        let job = CronJob {
            id,
            cron,
            prompt,
            recurring,
            durable,
            scheduler_id: Some(scheduler_id),
        };
        self.jobs.push(job.clone());
        if durable {
            self.persist_durable();
        }
        Ok(job)
    }

    /// Cancel and remove a job by user-facing id. Returns `true` if found.
    pub async fn delete(&mut self, id: &str) -> bool {
        let Some(pos) = self.jobs.iter().position(|j| j.id == id) else {
            return false;
        };
        let job = self.jobs.remove(pos);
        if let Some(scheduler_id) = job.scheduler_id {
            self.scheduler.cancel(&scheduler_id).await;
        }
        if job.durable {
            self.persist_durable();
        }
        true
    }

    #[must_use]
    pub fn list(&self) -> Vec<CronJob> {
        self.jobs.clone()
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<&CronJob> {
        self.jobs.iter().find(|j| j.id == id)
    }

    /// Re-register every persisted durable job with the scheduler.
    async fn reload_durable(&mut self) {
        for p in load_persisted() {
            let handler: Arc<dyn JobHandler> = Arc::new(CronFireHandler {
                job_id: p.id.clone(),
                prompt: p.prompt.clone(),
                queue: self.queue.clone(),
            });
            match self
                .scheduler
                .schedule(
                    JobSpec::Cron {
                        expression: p.cron.clone(),
                    },
                    handler,
                )
                .await
            {
                Ok(scheduler_id) => {
                    let numeric =
                        p.id.strip_prefix("cron_")
                            .and_then(|n| n.parse::<u64>().ok())
                            .unwrap_or(0);
                    self.next_id = self.next_id.max(numeric + 1);
                    self.jobs.push(CronJob {
                        id: p.id,
                        cron: p.cron,
                        prompt: p.prompt,
                        recurring: p.recurring,
                        durable: true,
                        scheduler_id: Some(scheduler_id),
                    });
                }
                Err(e) => {
                    tracing::warn!(job = %p.id, "failed to reschedule durable cron job: {e}");
                }
            }
        }
    }

    /// Write all durable jobs to disk.
    fn persist_durable(&self) {
        let durable: Vec<PersistedJob> = self
            .jobs
            .iter()
            .filter(|j| j.durable)
            .map(|j| PersistedJob {
                id: j.id.clone(),
                cron: j.cron.clone(),
                prompt: j.prompt.clone(),
                recurring: j.recurring,
            })
            .collect();
        save_persisted(&durable);
    }
}

impl Default for CronStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Thread-safe shared handle to a `CronStore`.
pub type SharedCronStore = Arc<tokio::sync::Mutex<CronStore>>;

/// Create a new shared cron store with durable jobs reloaded and
/// re-registered with the scheduler.
pub async fn shared_cron_store() -> SharedCronStore {
    let mut store = CronStore::new();
    store.reload_durable().await;
    Arc::new(tokio::sync::Mutex::new(store))
}

/// Create a new shared cron store without touching the durable-jobs file.
///
/// Used by the default registry builder, which runs in a synchronous context;
/// durable jobs are reloaded lazily by the runtime via [`shared_cron_store`].
#[must_use]
pub fn shared_cron_store_empty() -> SharedCronStore {
    Arc::new(tokio::sync::Mutex::new(CronStore::new()))
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
        CRON_CREATE_TOOL_NAME
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
            let cron_expr = input.get("cron").and_then(|v| v.as_str()).ok_or_else(|| {
                crab_core::Error::Other("missing required parameter: cron".into())
            })?;

            let prompt = input
                .get("prompt")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    crab_core::Error::Other("missing required parameter: prompt".into())
                })?;

            if cron_expr.trim().is_empty() {
                return Ok(ToolOutput::error("cron expression must not be empty"));
            }

            if prompt.trim().is_empty() {
                return Ok(ToolOutput::error("prompt must not be empty"));
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
                let mut store = store.lock().await;
                store
                    .create(
                        cron_expr.to_string(),
                        prompt.to_string(),
                        recurring,
                        durable,
                    )
                    .await
            };

            let job = match job {
                Ok(job) => job,
                Err(e) => {
                    return Ok(ToolOutput::error(format!("invalid cron expression: {e}")));
                }
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

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        let cron = input["cron"].as_str().unwrap_or("?");
        Some(format!("CronCreate ({cron})"))
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
        CRON_DELETE_TOOL_NAME
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
                .ok_or_else(|| crab_core::Error::Other("missing required parameter: id".into()))?;

            let deleted = {
                let mut store = store.lock().await;
                store.delete(id).await
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

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        let id = input["id"].as_str().unwrap_or("?");
        Some(format!("CronDelete ({id})"))
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
        CRON_LIST_TOOL_NAME
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
                let store = store.lock().await;
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

    fn format_use_summary(&self, _input: &Value) -> Option<String> {
        Some("CronList".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::permission::{PermissionMode, PermissionPolicy};
    use serde_json::json;
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    fn test_ctx() -> ToolContext {
        ToolContext {
            working_dir: std::path::PathBuf::from("/tmp/project"),
            permission_mode: PermissionMode::Dangerously,
            session_id: "test_session".into(),
            cancellation_token: CancellationToken::new(),
            permission_policy: PermissionPolicy::default(),
            ext: crab_core::tool::ToolContextExt::default(),
            task_registry: None,
            nested_memory_triggers: Arc::new(tokio::sync::Mutex::new(
                std::collections::HashSet::new(),
            )),
        }
    }

    // ─── CronStore unit tests ───

    #[tokio::test]
    async fn store_create_and_list() {
        let mut store = CronStore::new();
        let job = store
            .create("0 9 * * *".into(), "morning check".into(), true, false)
            .await
            .unwrap();
        assert_eq!(job.id, "cron_1");
        assert_eq!(store.list().len(), 1);
    }

    #[tokio::test]
    async fn store_auto_increment_ids() {
        let mut store = CronStore::new();
        let j1 = store
            .create("* * * * *".into(), "a".into(), true, false)
            .await
            .unwrap();
        let j2 = store
            .create("* * * * *".into(), "b".into(), true, false)
            .await
            .unwrap();
        assert_eq!(j1.id, "cron_1");
        assert_eq!(j2.id, "cron_2");
    }

    #[tokio::test]
    async fn store_delete() {
        let mut store = CronStore::new();
        store
            .create("0 * * * *".into(), "hourly".into(), true, false)
            .await
            .unwrap();
        assert!(store.delete("cron_1").await);
        assert!(store.list().is_empty());
    }

    #[tokio::test]
    async fn store_delete_nonexistent() {
        let mut store = CronStore::new();
        assert!(!store.delete("cron_999").await);
    }

    #[tokio::test]
    async fn store_get() {
        let mut store = CronStore::new();
        store
            .create("0 9 * * *".into(), "test".into(), false, false)
            .await
            .unwrap();
        let job = store.get("cron_1").unwrap();
        assert_eq!(job.prompt, "test");
        assert!(!job.recurring);
    }

    #[tokio::test]
    async fn store_rejects_invalid_cron() {
        let mut store = CronStore::new();
        let err = store
            .create("not a cron".into(), "p".into(), true, false)
            .await
            .unwrap_err();
        assert!(!err.is_empty());
        assert!(store.list().is_empty());
    }

    /// A job on a per-second schedule must actually fire and surface its
    /// prompt on the fired-prompt queue.
    #[tokio::test]
    async fn job_fires_and_enqueues_prompt() {
        let mut store = CronStore::new();
        let queue = store.fired_queue();
        // 6-field expression with a seconds column: fire every second.
        store
            .create("* * * * * *".into(), "tick prompt".into(), true, false)
            .await
            .unwrap();

        assert!(queue.is_empty());
        // Wait past the next second boundary plus margin.
        tokio::time::sleep(Duration::from_millis(2_200)).await;

        let fired = queue.drain();
        assert!(!fired.is_empty(), "cron job never fired");
        assert_eq!(fired[0].job_id, "cron_1");
        assert_eq!(fired[0].prompt, "tick prompt");
    }

    /// Cancelling a job stops further fires.
    #[tokio::test]
    async fn deleted_job_stops_firing() {
        let mut store = CronStore::new();
        let queue = store.fired_queue();
        store
            .create("* * * * * *".into(), "p".into(), true, false)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(1_200)).await;
        assert!(store.delete("cron_1").await);
        let _ = queue.drain();
        tokio::time::sleep(Duration::from_millis(2_200)).await;
        assert!(queue.is_empty(), "deleted job kept firing");
    }

    #[test]
    fn fired_queue_push_drain() {
        let q = FiredPromptQueue::new();
        assert!(q.is_empty());
        q.push(FiredPrompt {
            job_id: "cron_1".into(),
            prompt: "hello".into(),
        });
        assert_eq!(q.len(), 1);
        let drained = q.drain();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].prompt, "hello");
        assert!(q.is_empty());
    }

    // ─── CronCreateTool ───

    #[test]
    fn cron_create_metadata() {
        let store = shared_cron_store_empty();
        let tool = CronCreateTool::new(store);
        assert_eq!(tool.name(), "CronCreate");
        assert!(!tool.requires_confirmation());
        assert!(!tool.is_read_only());
    }

    #[test]
    fn cron_create_schema() {
        let store = shared_cron_store_empty();
        let tool = CronCreateTool::new(store);
        let schema = tool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("cron")));
        assert!(required.contains(&json!("prompt")));
        assert_eq!(required.len(), 2);
    }

    #[tokio::test]
    async fn cron_create_basic() {
        let store = shared_cron_store_empty();
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

        let len = store.lock().await.list().len();
        assert_eq!(len, 1);
    }

    #[tokio::test]
    async fn cron_create_one_shot() {
        let store = shared_cron_store_empty();
        let tool = CronCreateTool::new(store);
        let ctx = test_ctx();
        let input = json!({
            "cron": "30 14 6 4 *",
            "prompt": "remind me",
            "recurring": false,
            "durable": false
        });
        let output = tool.execute(input, &ctx).await.unwrap();
        assert!(!output.is_error);

        match &output.content[0] {
            ToolOutputContent::Json { value } => {
                assert_eq!(value["recurring"], false);
            }
            _ => panic!("expected JSON output"),
        }
    }

    #[tokio::test]
    async fn cron_create_rejects_empty_cron() {
        let store = shared_cron_store_empty();
        let tool = CronCreateTool::new(store);
        let ctx = test_ctx();
        let input = json!({"cron": "  ", "prompt": "test"});
        let output = tool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("empty"));
    }

    #[tokio::test]
    async fn cron_create_rejects_empty_prompt() {
        let store = shared_cron_store_empty();
        let tool = CronCreateTool::new(store);
        let ctx = test_ctx();
        let input = json!({"cron": "0 * * * *", "prompt": "  "});
        let output = tool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("empty"));
    }

    #[tokio::test]
    async fn cron_create_rejects_invalid_expression() {
        let store = shared_cron_store_empty();
        let tool = CronCreateTool::new(store);
        let ctx = test_ctx();
        let input = json!({"cron": "0 9 *", "prompt": "test"});
        let output = tool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("invalid cron"));
    }

    #[tokio::test]
    async fn cron_create_missing_cron() {
        let store = shared_cron_store_empty();
        let tool = CronCreateTool::new(store);
        let ctx = test_ctx();
        let input = json!({"prompt": "test"});
        let result = tool.execute(input, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn cron_create_missing_prompt() {
        let store = shared_cron_store_empty();
        let tool = CronCreateTool::new(store);
        let ctx = test_ctx();
        let input = json!({"cron": "0 * * * *"});
        let result = tool.execute(input, &ctx).await;
        assert!(result.is_err());
    }

    // ─── CronDeleteTool ───

    #[test]
    fn cron_delete_metadata() {
        let store = shared_cron_store_empty();
        let tool = CronDeleteTool::new(store);
        assert_eq!(tool.name(), "CronDelete");
        assert!(!tool.requires_confirmation());
    }

    #[tokio::test]
    async fn cron_delete_existing() {
        let store = shared_cron_store_empty();
        store
            .lock()
            .await
            .create("0 * * * *".into(), "test".into(), true, false)
            .await
            .unwrap();
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

        let is_empty = store.lock().await.list().is_empty();
        assert!(is_empty);
    }

    #[tokio::test]
    async fn cron_delete_nonexistent() {
        let store = shared_cron_store_empty();
        let tool = CronDeleteTool::new(store);
        let ctx = test_ctx();
        let input = json!({"id": "cron_999"});
        let output = tool.execute(input, &ctx).await.unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("not found"));
    }

    #[tokio::test]
    async fn cron_delete_missing_id() {
        let store = shared_cron_store_empty();
        let tool = CronDeleteTool::new(store);
        let ctx = test_ctx();
        let input = json!({});
        let result = tool.execute(input, &ctx).await;
        assert!(result.is_err());
    }

    // ─── CronListTool ───

    #[test]
    fn cron_list_metadata() {
        let store = shared_cron_store_empty();
        let tool = CronListTool::new(store);
        assert_eq!(tool.name(), "CronList");
        assert!(tool.is_read_only());
    }

    #[tokio::test]
    async fn cron_list_empty() {
        let store = shared_cron_store_empty();
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
        let store = shared_cron_store_empty();
        {
            let mut s = store.lock().await;
            s.create("0 9 * * *".into(), "morning".into(), true, false)
                .await
                .unwrap();
            s.create("0 17 * * *".into(), "evening".into(), true, false)
                .await
                .unwrap();
            drop(s);
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
        let store = shared_cron_store_empty();
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
