//! Glue between `TeamCreateTool`'s `team_created` JSON marker and the
//! in-process teammate backend.
//!
//! The agent loop emits the marker as a text tool-result; [`TeamCoordinator`]
//! scans recent tool results and, on first seeing a team, spawns the
//! configured teammate via [`InProcessBackend`]. Permission decisions made
//! by any teammate flow through [`PermissionSyncManager`] so the rest of
//! the team does not re-prompt the user for the same tool.

use std::collections::HashSet;

use serde_json::Value;

use crate::coordinator::PermissionSyncManager;
use crab_swarm::backend::{InProcessBackend, SwarmBackend, TeammateConfig};

/// The JSON `action` value emitted by `TeamCreateTool` when a team is
/// created. Kept as a module constant so runtime callers and the tool
/// implementation can't drift apart.
pub const TEAM_CREATED_ACTION: &str = "team_created";

/// Coordinator that tracks created teams and owns their teammate runtime.
pub struct TeamCoordinator {
    backend: InProcessBackend,
    permission_sync: PermissionSyncManager,
    seen_teams: HashSet<String>,
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
            seen_teams: HashSet::new(),
        }
    }

    /// Create a coordinator whose teammates run the given runner closure (an
    /// agent loop). The runtime injects this so teammates do real work instead
    /// of acting as logging sinks.
    #[must_use]
    pub fn with_runner(runner: crab_swarm::backend::TeammateRunner) -> Self {
        Self {
            backend: InProcessBackend::with_runner(runner),
            permission_sync: PermissionSyncManager::new(32),
            seen_teams: HashSet::new(),
        }
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

    /// Inspect a tool-result payload for the `team_created` marker and,
    /// if present, spawn a default teammate for the named team.
    ///
    /// The payload is expected to be the JSON string the tool serialized
    /// into its Text block (see `crates/tools/src/builtin/team.rs`).
    /// Returns `Ok(Some(team_name))` when a new team was processed,
    /// `Ok(None)` when the payload was not a team-created marker, and
    /// an error if the backend failed to spawn.
    pub async fn process_tool_result(
        &mut self,
        payload: &str,
    ) -> crab_core::Result<Option<String>> {
        let Ok(value) = serde_json::from_str::<Value>(payload) else {
            return Ok(None);
        };
        match value.get("action").and_then(Value::as_str) {
            Some(TEAM_CREATED_ACTION) => self.spawn_team(payload).await,
            Some("message_sent") => {
                self.route_message(&value).await;
                Ok(None)
            }
            Some("team_deleted") => {
                self.delete_team().await;
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    /// Spawn a single default teammate for a newly-seen team.
    async fn spawn_team(&mut self, payload: &str) -> crab_core::Result<Option<String>> {
        let Some(team_name) = parse_team_created(payload) else {
            return Ok(None);
        };
        if !self.seen_teams.insert(team_name.clone()) {
            return Ok(Some(team_name));
        }
        let config = TeammateConfig::new(format!("{team_name}-lead"), "lead");
        self.backend.spawn_teammate(config).await?;
        Ok(Some(team_name))
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

    /// Tear down the whole team: kill every teammate and forget seen teams so a
    /// later `team_created` re-spawns.
    async fn delete_team(&mut self) {
        let ids: Vec<String> = self
            .backend
            .list_teammates()
            .iter()
            .map(|t| t.id.clone())
            .collect();
        for id in ids {
            if let Err(e) = self.backend.kill_teammate(&id).await {
                tracing::warn!(error = %e, teammate = %id, "failed to kill teammate");
            }
        }
        self.seen_teams.clear();
    }
}

/// Parse the `team_created` JSON marker, returning the team name when the
/// payload matches. Any parse failure or schema mismatch returns `None`.
fn parse_team_created(payload: &str) -> Option<String> {
    let value: Value = serde_json::from_str(payload).ok()?;
    if value.get("action").and_then(Value::as_str) != Some(TEAM_CREATED_ACTION) {
        return None;
    }
    value
        .get("team_name")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_marker_returns_team_name() {
        let payload = r#"{"action":"team_created","team_name":"alpha","description":""}"#;
        assert_eq!(parse_team_created(payload).as_deref(), Some("alpha"));
    }

    #[test]
    fn parse_rejects_non_team_action() {
        let payload = r#"{"action":"other","team_name":"x"}"#;
        assert!(parse_team_created(payload).is_none());
    }

    #[test]
    fn parse_rejects_invalid_json() {
        assert!(parse_team_created("not json").is_none());
    }

    #[tokio::test]
    async fn process_tool_result_spawns_once_per_team() {
        let mut coord = TeamCoordinator::new();
        let payload = r#"{"action":"team_created","team_name":"alpha","description":""}"#;

        assert_eq!(coord.teammate_count(), 0);
        let first = coord.process_tool_result(payload).await.unwrap();
        assert_eq!(first.as_deref(), Some("alpha"));
        assert_eq!(coord.teammate_count(), 1);

        // Second call with same team name is a no-op.
        let second = coord.process_tool_result(payload).await.unwrap();
        assert_eq!(second.as_deref(), Some("alpha"));
        assert_eq!(coord.teammate_count(), 1);
    }

    #[tokio::test]
    async fn team_deleted_kills_teammates_and_forgets_team() {
        let mut coord = TeamCoordinator::new();
        let created = r#"{"action":"team_created","team_name":"alpha","description":""}"#;
        coord.process_tool_result(created).await.unwrap();
        assert_eq!(coord.teammate_count(), 1);

        let deleted = r#"{"action":"team_deleted","team_name":"alpha"}"#;
        coord.process_tool_result(deleted).await.unwrap();
        assert_eq!(coord.teammate_count(), 0);

        // The team is forgotten, so re-creating it spawns again.
        coord.process_tool_result(created).await.unwrap();
        assert_eq!(coord.teammate_count(), 1);
    }

    #[tokio::test]
    async fn message_sent_to_known_teammate_is_delivered() {
        let mut coord = TeamCoordinator::new();
        coord
            .process_tool_result(
                r#"{"action":"team_created","team_name":"alpha","description":""}"#,
            )
            .await
            .unwrap();
        // The default teammate is named "alpha-lead"; routing to it must not error.
        let msg = r#"{"action":"message_sent","to":"alpha-lead","message":"do the thing","is_broadcast":false}"#;
        let result = coord.process_tool_result(msg).await.unwrap();
        assert!(result.is_none());
        assert_eq!(coord.teammate_count(), 1);
    }

    #[tokio::test]
    async fn message_sent_to_unknown_teammate_is_noop() {
        let mut coord = TeamCoordinator::new();
        let msg = r#"{"action":"message_sent","to":"ghost","message":"hi","is_broadcast":false}"#;
        assert!(coord.process_tool_result(msg).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn process_tool_result_ignores_unrelated_payloads() {
        let mut coord = TeamCoordinator::new();
        let result = coord.process_tool_result("unrelated output").await.unwrap();
        assert!(result.is_none());
        assert_eq!(coord.teammate_count(), 0);
    }

    #[test]
    fn new_coordinator_has_permission_sync() {
        let coord = TeamCoordinator::new();
        assert_eq!(coord.permission_sync().subscriber_count(), 0);
    }
}
