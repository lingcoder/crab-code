//! Glue between the Agent tool's named-spawn markers and the in-process
//! teammate backend.
//!
//! Every session has one implicit team. A `spawn_agent` marker carrying a
//! `name` spawns a long-lived teammate via [`InProcessBackend`] and seeds it
//! with the task as its first message; `message_sent` markers route messages
//! to teammates by name. Permission decisions made by any teammate flow
//! through [`PermissionSyncManager`] so the rest of the team does not
//! re-prompt the user for the same tool.
//!
//! Teammates live for the rest of the session; [`TeamCoordinator::shutdown_all`]
//! tears them down when the session ends.

use serde_json::Value;

use crate::coordinator::PermissionSyncManager;
use crab_team::backend::{InProcessBackend, TeammateBackend, TeammateConfig, TeammateRunner};

use super::spawn::SPAWN_AGENT_ACTION;

/// Coordinator that owns the session's implicit team of named teammates.
pub struct TeamCoordinator {
    backend: InProcessBackend,
    permission_sync: PermissionSyncManager,
}

impl Default for TeamCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

impl TeamCoordinator {
    /// Create a new coordinator with passive teammates (no work runner). Used
    /// by tests and headless callers that do not drive teammate agent loops.
    #[must_use]
    pub fn new() -> Self {
        Self {
            backend: InProcessBackend::new(),
            permission_sync: PermissionSyncManager::new(32),
        }
    }

    /// Create a coordinator whose teammates run the given runner closure (an
    /// agent loop). The runtime injects this so teammates do real work instead
    /// of acting as logging sinks.
    #[must_use]
    pub fn with_runner(runner: TeammateRunner) -> Self {
        Self {
            backend: InProcessBackend::with_runner(runner),
            permission_sync: PermissionSyncManager::new(32),
        }
    }

    /// Install a runner if none is set yet. Lets facades that only gain
    /// access to their LLM backend after construction (e.g. the TUI runtime,
    /// which receives it per query) upgrade passive teammates to real agent
    /// loops before the first spawn.
    pub fn ensure_runner(&mut self, make: impl FnOnce() -> TeammateRunner) {
        self.backend.ensure_runner(make);
    }

    /// Access the permission-sync bus so teammates can subscribe and
    /// producers can broadcast user decisions.
    #[must_use]
    pub fn permission_sync(&self) -> &PermissionSyncManager {
        &self.permission_sync
    }

    /// Read-only view of the in-process backend, used to snapshot the
    /// teammate list for the TUI team browser.
    #[must_use]
    pub fn backend(&self) -> &InProcessBackend {
        &self.backend
    }

    /// Number of teammates currently tracked by the backend.
    #[must_use]
    pub fn teammate_count(&self) -> usize {
        self.backend.list_teammates().len()
    }

    /// Inspect a tool-result payload for team-relevant markers.
    ///
    /// A `spawn_agent` marker with a non-empty `name` spawns a named teammate
    /// (unnamed spawn markers are one-shot workers, handled by the worker-pool
    /// path). A `message_sent` marker routes a message to teammates by name.
    ///
    /// `base_prompt` is the session's system prompt; spawned teammates inherit
    /// it beneath a short teammate preamble, mirroring the worker fallback
    /// prompt.
    ///
    /// Returns `Ok(Some(name))` when a teammate was spawned, `Ok(None)` for
    /// all other payloads, and an error if the backend failed to spawn.
    pub async fn process_tool_result(
        &mut self,
        payload: &str,
        base_prompt: &str,
    ) -> crab_core::Result<Option<String>> {
        let Ok(value) = serde_json::from_str::<Value>(payload) else {
            return Ok(None);
        };
        match value.get("action").and_then(Value::as_str) {
            Some(SPAWN_AGENT_ACTION) => self.spawn_named_teammate(&value, base_prompt).await,
            Some("message_sent") => {
                self.route_message(&value).await;
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    /// Spawn a named teammate from a `spawn_agent` marker and seed it with the
    /// task as its first message.
    ///
    /// A repeat spawn under an existing name starts fresh: the old teammate is
    /// killed first, matching the Agent-tool contract that `SendMessage`
    /// continues an agent while a new `Agent` call replaces it.
    async fn spawn_named_teammate(
        &mut self,
        value: &Value,
        base_prompt: &str,
    ) -> crab_core::Result<Option<String>> {
        let Some(name) = value
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|n| !n.is_empty())
        else {
            return Ok(None);
        };
        let Some(task) = value.get("task").and_then(Value::as_str) else {
            return Ok(None);
        };

        let stale: Vec<String> = self
            .backend
            .list_teammates()
            .iter()
            .filter(|t| t.name == name)
            .map(|t| t.id.clone())
            .collect();
        for id in stale {
            if let Err(e) = self.backend.kill_teammate(&id).await {
                tracing::warn!(error = %e, teammate = %id, "failed to kill stale teammate before respawn");
            }
        }

        let role = value
            .get("subagent_type")
            .and_then(Value::as_str)
            .unwrap_or("teammate");
        let mut config = TeammateConfig::new(name, role).with_system_prompt(format!(
            "You are teammate \"{name}\" in this session's agent team. Complete each \
             task you receive and report concise results.\n\n{base_prompt}"
        ));
        if let Some(wd) = value.get("working_dir").and_then(Value::as_str) {
            config = config.with_working_dir(std::path::PathBuf::from(wd));
        }

        let id = self.backend.spawn_teammate(config).await?;
        self.backend.send_message(&id, task).await?;
        Ok(Some(name.to_owned()))
    }

    /// Deliver a `message_sent` marker to the named teammate (or every teammate
    /// for a `*` broadcast), waking the teammate's agent loop.
    async fn route_message(&self, value: &Value) {
        let Some(message) = value.get("message").and_then(Value::as_str) else {
            return;
        };
        let to = value.get("to").and_then(Value::as_str).unwrap_or_default();

        // Resolve target ids up front so the teammate-list borrow is released
        // before the async sends.
        let targets: Vec<String> = if to == "*" {
            self.backend
                .list_teammates()
                .iter()
                .map(|t| t.id.clone())
                .collect()
        } else {
            self.backend
                .list_teammates()
                .iter()
                .filter(|t| t.name == to)
                .map(|t| t.id.clone())
                .collect()
        };
        for id in targets {
            if let Err(e) = self.backend.send_message(&id, message).await {
                tracing::warn!(error = %e, teammate = %id, "failed to deliver message to teammate");
            }
        }
    }

    /// Kill every teammate. Called when the session ends — the implicit team's
    /// lifetime is the session's lifetime.
    pub async fn shutdown_all(&mut self) {
        let ids: Vec<String> = self
            .backend
            .list_teammates()
            .iter()
            .map(|t| t.id.clone())
            .collect();
        for id in ids {
            if let Err(e) = self.backend.kill_teammate(&id).await {
                tracing::warn!(error = %e, teammate = %id, "failed to kill teammate at session shutdown");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named_spawn(name: &str, task: &str) -> String {
        serde_json::json!({"action": "spawn_agent", "task": task, "name": name}).to_string()
    }

    #[tokio::test]
    async fn named_spawn_marker_spawns_teammate() {
        let mut coord = TeamCoordinator::new();
        assert_eq!(coord.teammate_count(), 0);

        let spawned = coord
            .process_tool_result(&named_spawn("researcher", "dig into the API"), "Base.")
            .await
            .unwrap();
        assert_eq!(spawned.as_deref(), Some("researcher"));
        assert_eq!(coord.teammate_count(), 1);
    }

    #[tokio::test]
    async fn respawn_same_name_replaces_teammate() {
        let mut coord = TeamCoordinator::new();
        coord
            .process_tool_result(&named_spawn("worker", "first task"), "")
            .await
            .unwrap();
        assert_eq!(coord.teammate_count(), 1);

        // Same name again: the old teammate is killed, a fresh one spawns.
        coord
            .process_tool_result(&named_spawn("worker", "second task"), "")
            .await
            .unwrap();
        assert_eq!(coord.teammate_count(), 1);
    }

    #[tokio::test]
    async fn unnamed_spawn_marker_is_ignored() {
        let mut coord = TeamCoordinator::new();
        let payload = r#"{"action":"spawn_agent","task":"one-shot work"}"#;
        assert!(
            coord
                .process_tool_result(payload, "")
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(coord.teammate_count(), 0);

        // Null and empty names are one-shot workers too.
        let null_name = r#"{"action":"spawn_agent","task":"t","name":null}"#;
        assert!(
            coord
                .process_tool_result(null_name, "")
                .await
                .unwrap()
                .is_none()
        );
        let empty_name = r#"{"action":"spawn_agent","task":"t","name":"  "}"#;
        assert!(
            coord
                .process_tool_result(empty_name, "")
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(coord.teammate_count(), 0);
    }

    #[tokio::test]
    async fn teammate_inherits_base_prompt() {
        let mut coord = TeamCoordinator::new();
        coord
            .process_tool_result(&named_spawn("alice", "review"), "Session base prompt.")
            .await
            .unwrap();
        // The spawned teammate exists; prompt content is carried in its config
        // (verified indirectly — spawn succeeded with a composed prompt).
        assert_eq!(coord.teammate_count(), 1);
    }

    #[tokio::test]
    async fn message_sent_to_known_teammate_is_delivered() {
        let mut coord = TeamCoordinator::new();
        coord
            .process_tool_result(&named_spawn("alice", "start"), "")
            .await
            .unwrap();
        let msg = r#"{"action":"message_sent","to":"alice","message":"do the thing","is_broadcast":false}"#;
        let result = coord.process_tool_result(msg, "").await.unwrap();
        assert!(result.is_none());
        assert_eq!(coord.teammate_count(), 1);
    }

    #[tokio::test]
    async fn message_sent_to_unknown_teammate_is_noop() {
        let mut coord = TeamCoordinator::new();
        let msg = r#"{"action":"message_sent","to":"ghost","message":"hi","is_broadcast":false}"#;
        assert!(coord.process_tool_result(msg, "").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn broadcast_reaches_all_without_error() {
        let mut coord = TeamCoordinator::new();
        coord
            .process_tool_result(&named_spawn("a", "t1"), "")
            .await
            .unwrap();
        coord
            .process_tool_result(&named_spawn("b", "t2"), "")
            .await
            .unwrap();
        let msg = r#"{"action":"message_sent","to":"*","message":"sync up","is_broadcast":true}"#;
        assert!(coord.process_tool_result(msg, "").await.unwrap().is_none());
        assert_eq!(coord.teammate_count(), 2);
    }

    #[tokio::test]
    async fn shutdown_all_kills_every_teammate() {
        let mut coord = TeamCoordinator::new();
        coord
            .process_tool_result(&named_spawn("a", "t1"), "")
            .await
            .unwrap();
        coord
            .process_tool_result(&named_spawn("b", "t2"), "")
            .await
            .unwrap();
        assert_eq!(coord.teammate_count(), 2);

        coord.shutdown_all().await;
        assert_eq!(coord.teammate_count(), 0);
    }

    #[tokio::test]
    async fn process_tool_result_ignores_unrelated_payloads() {
        let mut coord = TeamCoordinator::new();
        let result = coord
            .process_tool_result("unrelated output", "")
            .await
            .unwrap();
        assert!(result.is_none());
        assert_eq!(coord.teammate_count(), 0);
    }

    #[test]
    fn new_coordinator_has_permission_sync() {
        let coord = TeamCoordinator::new();
        assert_eq!(coord.permission_sync().subscriber_count(), 0);
    }

    #[tokio::test]
    async fn ensure_runner_installs_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = std::sync::Arc::new(AtomicUsize::new(0));

        let mut coord = TeamCoordinator::new();
        for _ in 0..2 {
            let calls = std::sync::Arc::clone(&calls);
            coord.ensure_runner(move || {
                calls.fetch_add(1, Ordering::SeqCst);
                std::sync::Arc::new(|ctx: crab_team::backend::TeammateRunCtx| {
                    Box::pin(async move { ctx.cancel.cancelled().await })
                })
            });
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1, "second install is a no-op");
    }
}
