use crab_common::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput, ToolOutputContent};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

// ── Trigger data model ─────────────────────────────────────────────────

/// A remote trigger definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trigger {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub prompt: String,
    pub created_at: String,
}

/// In-memory trigger store.
pub struct TriggerStore {
    triggers: Vec<Trigger>,
    next_id: u64,
}

impl TriggerStore {
    #[must_use]
    pub fn new() -> Self {
        Self {
            triggers: Vec::new(),
            next_id: 1,
        }
    }

    pub fn create(&mut self, name: String, description: Option<String>, prompt: String) -> Trigger {
        let id = format!("trigger_{}", self.next_id);
        self.next_id += 1;
        let trigger = Trigger {
            id,
            name,
            description,
            prompt,
            created_at: "2026-01-01T00:00:00Z".into(), // placeholder; real impl uses chrono
        };
        self.triggers.push(trigger.clone());
        trigger
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Trigger> {
        self.triggers.iter().find(|t| t.id == id)
    }

    pub fn update(
        &mut self,
        id: &str,
        name: Option<String>,
        description: Option<String>,
        prompt: Option<String>,
    ) -> bool {
        if let Some(trigger) = self.triggers.iter_mut().find(|t| t.id == id) {
            if let Some(n) = name {
                trigger.name = n;
            }
            if description.is_some() {
                trigger.description = description;
            }
            if let Some(p) = prompt {
                trigger.prompt = p;
            }
            true
        } else {
            false
        }
    }

    pub fn delete(&mut self, id: &str) -> bool {
        let len_before = self.triggers.len();
        self.triggers.retain(|t| t.id != id);
        self.triggers.len() < len_before
    }

    #[must_use]
    pub fn list(&self) -> Vec<Trigger> {
        self.triggers.clone()
    }
}

impl Default for TriggerStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Thread-safe shared handle to a `TriggerStore`.
pub type SharedTriggerStore = Arc<Mutex<TriggerStore>>;

/// Create a new shared trigger store.
#[must_use]
pub fn shared_trigger_store() -> SharedTriggerStore {
    Arc::new(Mutex::new(TriggerStore::new()))
}

// ── RemoteTriggerTool ──────────────────────────────────────────────────

/// Manage remote triggers (list/get/create/update/run).
pub struct RemoteTriggerTool {
    store: SharedTriggerStore,
}

impl RemoteTriggerTool {
    #[must_use]
    pub fn new(store: SharedTriggerStore) -> Self {
        Self { store }
    }
}

impl Tool for RemoteTriggerTool {
    fn name(&self) -> &'static str {
        "remote_trigger"
    }

    fn description(&self) -> &'static str {
        "Create, list, get, update, or run remote triggers"
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "get", "create", "update", "run", "delete"],
                    "description": "The action to perform"
                },
                "trigger_id": {
                    "type": "string",
                    "description": "Trigger ID (required for get/update/run/delete)"
                },
                "name": {
                    "type": "string",
                    "description": "Trigger name (required for create)"
                },
                "description": {
                    "type": "string",
                    "description": "Trigger description"
                },
                "prompt": {
                    "type": "string",
                    "description": "The prompt to execute (required for create)"
                }
            },
            "required": ["action"]
        })
    }

    #[allow(clippy::too_many_lines)]
    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            let action = input
                .get("action")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    crab_common::Error::Other("missing required parameter: action".into())
                })?;

            match action {
                "list" => {
                    let triggers: Vec<Value> = {
                        let s = store.lock().unwrap();
                        s.list()
                            .into_iter()
                            .map(|t| {
                                serde_json::json!({
                                    "id": t.id,
                                    "name": t.name,
                                    "description": t.description,
                                    "prompt": t.prompt,
                                })
                            })
                            .collect()
                    };
                    Ok(ToolOutput::with_content(
                        vec![ToolOutputContent::Json {
                            value: serde_json::json!(triggers),
                        }],
                        false,
                    ))
                }
                "get" => {
                    let trigger_id = input
                        .get("trigger_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| {
                            crab_common::Error::Other(
                                "trigger_id is required for 'get' action".into(),
                            )
                        })?;

                    let s = store.lock().unwrap();
                    s.get(trigger_id).map_or_else(
                        || Ok(ToolOutput::error(format!("trigger '{trigger_id}' not found"))),
                        |trigger| {
                            let result = serde_json::json!({
                                "id": trigger.id,
                                "name": trigger.name,
                                "description": trigger.description,
                                "prompt": trigger.prompt,
                                "created_at": trigger.created_at,
                            });
                            Ok(ToolOutput::with_content(
                                vec![ToolOutputContent::Json { value: result }],
                                false,
                            ))
                        },
                    )
                }
                "create" => {
                    let name = input
                        .get("name")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| {
                            crab_common::Error::Other(
                                "name is required for 'create' action".into(),
                            )
                        })?;

                    let prompt = input
                        .get("prompt")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| {
                            crab_common::Error::Other(
                                "prompt is required for 'create' action".into(),
                            )
                        })?;

                    if name.trim().is_empty() {
                        return Ok(ToolOutput::error("name must not be empty"));
                    }

                    let description = input
                        .get("description")
                        .and_then(|v| v.as_str())
                        .map(String::from);

                    let trigger = {
                        let mut s = store.lock().unwrap();
                        s.create(name.to_string(), description, prompt.to_string())
                    };

                    let result = serde_json::json!({
                        "action": "trigger_created",
                        "id": trigger.id,
                        "name": trigger.name,
                    });
                    Ok(ToolOutput::with_content(
                        vec![ToolOutputContent::Json { value: result }],
                        false,
                    ))
                }
                "update" => {
                    let trigger_id = input
                        .get("trigger_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| {
                            crab_common::Error::Other(
                                "trigger_id is required for 'update' action".into(),
                            )
                        })?;

                    let name = input.get("name").and_then(|v| v.as_str()).map(String::from);
                    let description = input
                        .get("description")
                        .and_then(|v| v.as_str())
                        .map(String::from);
                    let prompt = input
                        .get("prompt")
                        .and_then(|v| v.as_str())
                        .map(String::from);

                    let mut s = store.lock().unwrap();
                    if s.update(trigger_id, name, description, prompt) {
                        let result = serde_json::json!({
                            "action": "trigger_updated",
                            "id": trigger_id,
                        });
                        Ok(ToolOutput::with_content(
                            vec![ToolOutputContent::Json { value: result }],
                            false,
                        ))
                    } else {
                        Ok(ToolOutput::error(format!(
                            "trigger '{trigger_id}' not found"
                        )))
                    }
                }
                "run" => {
                    let trigger_id = input
                        .get("trigger_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| {
                            crab_common::Error::Other(
                                "trigger_id is required for 'run' action".into(),
                            )
                        })?;

                    let s = store.lock().unwrap();
                    if s.get(trigger_id).is_some() {
                        let result = serde_json::json!({
                            "action": "trigger_run",
                            "trigger_id": trigger_id,
                        });
                        Ok(ToolOutput::with_content(
                            vec![ToolOutputContent::Json { value: result }],
                            false,
                        ))
                    } else {
                        Ok(ToolOutput::error(format!(
                            "trigger '{trigger_id}' not found"
                        )))
                    }
                }
                "delete" => {
                    let trigger_id = input
                        .get("trigger_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| {
                            crab_common::Error::Other(
                                "trigger_id is required for 'delete' action".into(),
                            )
                        })?;

                    let mut s = store.lock().unwrap();
                    if s.delete(trigger_id) {
                        let result = serde_json::json!({
                            "action": "trigger_deleted",
                            "id": trigger_id,
                        });
                        Ok(ToolOutput::with_content(
                            vec![ToolOutputContent::Json { value: result }],
                            false,
                        ))
                    } else {
                        Ok(ToolOutput::error(format!(
                            "trigger '{trigger_id}' not found"
                        )))
                    }
                }
                other => Ok(ToolOutput::error(format!(
                    "unknown action: '{other}'. Expected one of: list, get, create, update, run, delete"
                ))),
            }
        })
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

    // ─── TriggerStore unit tests ───

    #[test]
    fn store_create_and_get() {
        let mut store = TriggerStore::new();
        let t = store.create("deploy".into(), Some("Deploy to prod".into()), "run deploy".into());
        assert_eq!(t.id, "trigger_1");
        assert_eq!(t.name, "deploy");
        let got = store.get("trigger_1").unwrap();
        assert_eq!(got.name, "deploy");
    }

    #[test]
    fn store_update() {
        let mut store = TriggerStore::new();
        store.create("old".into(), None, "old prompt".into());
        assert!(store.update("trigger_1", Some("new".into()), None, Some("new prompt".into())));
        let t = store.get("trigger_1").unwrap();
        assert_eq!(t.name, "new");
        assert_eq!(t.prompt, "new prompt");
    }

    #[test]
    fn store_update_nonexistent() {
        let mut store = TriggerStore::new();
        assert!(!store.update("trigger_999", None, None, None));
    }

    #[test]
    fn store_delete() {
        let mut store = TriggerStore::new();
        store.create("test".into(), None, "prompt".into());
        assert!(store.delete("trigger_1"));
        assert!(store.list().is_empty());
        assert!(!store.delete("trigger_1"));
    }

    #[test]
    fn trigger_serde_roundtrip() {
        let trigger = Trigger {
            id: "trigger_1".into(),
            name: "deploy".into(),
            description: Some("Deploy trigger".into()),
            prompt: "run deploy".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&trigger).unwrap();
        let back: Trigger = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "trigger_1");
        assert_eq!(back.name, "deploy");
    }

    // ─── RemoteTriggerTool ───

    #[test]
    fn tool_metadata() {
        let store = shared_trigger_store();
        let tool = RemoteTriggerTool::new(store);
        assert_eq!(tool.name(), "remote_trigger");
        assert!(!tool.requires_confirmation());
        assert!(!tool.is_read_only());
    }

    #[test]
    fn tool_schema() {
        let store = shared_trigger_store();
        let tool = RemoteTriggerTool::new(store);
        let schema = tool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("action")));
    }

    #[tokio::test]
    async fn action_create() {
        let store = shared_trigger_store();
        let tool = RemoteTriggerTool::new(Arc::clone(&store));
        let ctx = test_ctx();
        let input = json!({
            "action": "create",
            "name": "nightly-build",
            "description": "Run nightly build",
            "prompt": "cargo build --release"
        });
        let output = tool.execute(input, &ctx).await.unwrap();
        assert!(!output.is_error);

        match &output.content[0] {
            ToolOutputContent::Json { value } => {
                assert_eq!(value["action"], "trigger_created");
                assert_eq!(value["id"], "trigger_1");
                assert_eq!(value["name"], "nightly-build");
            }
            _ => panic!("expected JSON output"),
        }
    }

    #[tokio::test]
    async fn action_list() {
        let store = shared_trigger_store();
        {
            let mut s = store.lock().unwrap();
            s.create("t1".into(), None, "p1".into());
            s.create("t2".into(), None, "p2".into());
        }
        let tool = RemoteTriggerTool::new(store);
        let ctx = test_ctx();
        let output = tool.execute(json!({"action": "list"}), &ctx).await.unwrap();
        assert!(!output.is_error);

        match &output.content[0] {
            ToolOutputContent::Json { value } => {
                assert_eq!(value.as_array().unwrap().len(), 2);
            }
            _ => panic!("expected JSON output"),
        }
    }

    #[tokio::test]
    async fn action_list_empty() {
        let store = shared_trigger_store();
        let tool = RemoteTriggerTool::new(store);
        let ctx = test_ctx();
        let output = tool.execute(json!({"action": "list"}), &ctx).await.unwrap();
        assert!(!output.is_error);

        match &output.content[0] {
            ToolOutputContent::Json { value } => {
                assert!(value.as_array().unwrap().is_empty());
            }
            _ => panic!("expected JSON output"),
        }
    }

    #[tokio::test]
    async fn action_get() {
        let store = shared_trigger_store();
        {
            let mut s = store.lock().unwrap();
            s.create("deploy".into(), Some("Deploy".into()), "run deploy".into());
        }
        let tool = RemoteTriggerTool::new(store);
        let ctx = test_ctx();
        let output = tool
            .execute(json!({"action": "get", "trigger_id": "trigger_1"}), &ctx)
            .await
            .unwrap();
        assert!(!output.is_error);

        match &output.content[0] {
            ToolOutputContent::Json { value } => {
                assert_eq!(value["name"], "deploy");
                assert_eq!(value["description"], "Deploy");
            }
            _ => panic!("expected JSON output"),
        }
    }

    #[tokio::test]
    async fn action_get_not_found() {
        let store = shared_trigger_store();
        let tool = RemoteTriggerTool::new(store);
        let ctx = test_ctx();
        let output = tool
            .execute(json!({"action": "get", "trigger_id": "trigger_999"}), &ctx)
            .await
            .unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("not found"));
    }

    #[tokio::test]
    async fn action_update() {
        let store = shared_trigger_store();
        {
            let mut s = store.lock().unwrap();
            s.create("old".into(), None, "old prompt".into());
        }
        let tool = RemoteTriggerTool::new(store);
        let ctx = test_ctx();
        let output = tool
            .execute(
                json!({"action": "update", "trigger_id": "trigger_1", "name": "new"}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(!output.is_error);

        match &output.content[0] {
            ToolOutputContent::Json { value } => {
                assert_eq!(value["action"], "trigger_updated");
                assert_eq!(value["id"], "trigger_1");
            }
            _ => panic!("expected JSON output"),
        }
    }

    #[tokio::test]
    async fn action_run() {
        let store = shared_trigger_store();
        {
            let mut s = store.lock().unwrap();
            s.create("deploy".into(), None, "run deploy".into());
        }
        let tool = RemoteTriggerTool::new(store);
        let ctx = test_ctx();
        let output = tool
            .execute(json!({"action": "run", "trigger_id": "trigger_1"}), &ctx)
            .await
            .unwrap();
        assert!(!output.is_error);

        match &output.content[0] {
            ToolOutputContent::Json { value } => {
                assert_eq!(value["action"], "trigger_run");
                assert_eq!(value["trigger_id"], "trigger_1");
            }
            _ => panic!("expected JSON output"),
        }
    }

    #[tokio::test]
    async fn action_run_not_found() {
        let store = shared_trigger_store();
        let tool = RemoteTriggerTool::new(store);
        let ctx = test_ctx();
        let output = tool
            .execute(json!({"action": "run", "trigger_id": "trigger_999"}), &ctx)
            .await
            .unwrap();
        assert!(output.is_error);
    }

    #[tokio::test]
    async fn action_delete() {
        let store = shared_trigger_store();
        {
            let mut s = store.lock().unwrap();
            s.create("test".into(), None, "prompt".into());
        }
        let tool = RemoteTriggerTool::new(Arc::clone(&store));
        let ctx = test_ctx();
        let output = tool
            .execute(json!({"action": "delete", "trigger_id": "trigger_1"}), &ctx)
            .await
            .unwrap();
        assert!(!output.is_error);

        match &output.content[0] {
            ToolOutputContent::Json { value } => {
                assert_eq!(value["action"], "trigger_deleted");
            }
            _ => panic!("expected JSON output"),
        }

        let s = store.lock().unwrap();
        assert!(s.list().is_empty());
    }

    #[tokio::test]
    async fn action_unknown() {
        let store = shared_trigger_store();
        let tool = RemoteTriggerTool::new(store);
        let ctx = test_ctx();
        let output = tool
            .execute(json!({"action": "invalid"}), &ctx)
            .await
            .unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("unknown action"));
    }

    #[tokio::test]
    async fn action_create_empty_name() {
        let store = shared_trigger_store();
        let tool = RemoteTriggerTool::new(store);
        let ctx = test_ctx();
        let output = tool
            .execute(
                json!({"action": "create", "name": "  ", "prompt": "test"}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(output.is_error);
        assert!(output.text().contains("empty"));
    }

    #[tokio::test]
    async fn action_missing() {
        let store = shared_trigger_store();
        let tool = RemoteTriggerTool::new(store);
        let ctx = test_ctx();
        let result = tool.execute(json!({}), &ctx).await;
        assert!(result.is_err());
    }
}
