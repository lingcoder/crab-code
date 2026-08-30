//! The single owner of a session's team.
//!
//! Every session has one implicit team and one [`TeamRunner`] driving it.
//! `spawn_agent` markers become teammates — resident when the marker names
//! one, ephemeral otherwise — and `message_sent` markers route to teammates
//! by name. Both lifetimes share one roster, one job registry, one retry
//! tracker, and one result channel.
//!
//! Unrelated to [`crate::coordinator::CoordinatorMode`], which is the opt-in
//! tool-ACL + prompt overlay applied to the *main* agent.

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::Value;

use crab_core::job::{BackgroundJob, JobRegistry, JobStatus, JobType};
use crab_team::backend::{
    InProcessBackend, MAIN_AGENT, TeammateBackend, TeammateConfig, TeammateRunner,
};
use crab_team::bus::{AgentMessage, Envelope};
use crab_team::retry::{RetryDecision, RetryPolicy, RetryTracker};
use crab_team::roster::{Team, TeamMode, TeammateState};
use crab_tools::executor::PermissionHandler;
use tokio::sync::mpsc;

use super::spawn::{
    MESSAGE_SENT_ACTION, SPAWN_AGENT_ACTION, TeamHandles, TeammateMarker, agent_result_message,
    marker_name, teammate_config_from_marker,
};
use super::worker::{WorkerResult, fire_task_hook};
use crate::coordinator::PermissionSyncManager;

/// How many results may queue up before a teammate's send blocks.
const RESULT_CHANNEL_CAPACITY: usize = 64;

/// How many teammate-emitted markers may queue up before a teammate blocks.
const MARKER_CHANNEL_CAPACITY: usize = 64;

/// Owns the session's implicit team of teammates and drives them.
pub struct TeamRunner {
    backend: InProcessBackend,
    permission_sync: Arc<PermissionSyncManager>,
    retry: RetryTracker,
    job_registry: Option<Arc<std::sync::Mutex<JobRegistry>>>,
    hook_executor: Option<Arc<crab_hooks::HookExecutor>>,
    results_tx: mpsc::Sender<WorkerResult>,
    results_rx: mpsc::Receiver<WorkerResult>,
    /// Team markers teammates emitted from inside their own turns — the return
    /// path that lets a teammate's `SendMessage` reach the router.
    markers_tx: mpsc::Sender<TeammateMarker>,
    markers_rx: mpsc::Receiver<TeammateMarker>,
    /// Results from resident teammates that arrived outside a turn, waiting to
    /// be folded into the parent conversation.
    pending: Vec<WorkerResult>,
}

impl Default for TeamRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl TeamRunner {
    /// Create a runner with passive teammates (no agent loop). Used by tests
    /// and headless callers that cannot drive teammate agent loops.
    #[must_use]
    pub fn new() -> Self {
        let (results_tx, results_rx) = mpsc::channel(RESULT_CHANNEL_CAPACITY);
        let (markers_tx, markers_rx) = mpsc::channel(MARKER_CHANNEL_CAPACITY);
        Self {
            backend: InProcessBackend::new(),
            permission_sync: Arc::new(PermissionSyncManager::new(32)),
            retry: RetryTracker::new(RetryPolicy::default()),
            job_registry: None,
            hook_executor: None,
            results_tx,
            results_rx,
            markers_tx,
            markers_rx,
            pending: Vec::new(),
        }
    }

    /// Create a runner whose teammates run the given runner closure.
    ///
    /// `permission` is the session's permission handler; teammates ask through
    /// it so the user answers once for the whole team. Pass `None` for a
    /// non-interactive session.
    #[must_use]
    pub fn with_runner(
        permission: Option<Arc<dyn PermissionHandler>>,
        make: impl FnOnce(TeamHandles) -> TeammateRunner,
    ) -> Self {
        let mut runner = Self::new();
        let handles = runner.handles(permission);
        runner.backend = InProcessBackend::with_runner(make(handles));
        runner
    }

    /// Install a runner if none is set yet. Lets facades that only gain access
    /// to their LLM backend after construction (e.g. the TUI runtime, which
    /// receives it per query) upgrade passive teammates to real agent loops
    /// before the first spawn.
    pub fn ensure_runner(
        &mut self,
        permission: Option<Arc<dyn PermissionHandler>>,
        make: impl FnOnce(TeamHandles) -> TeammateRunner,
    ) {
        let handles = self.handles(permission);
        self.backend.ensure_runner(move || make(handles));
    }

    /// The shared handles every spawned teammate needs.
    fn handles(&self, permission: Option<Arc<dyn PermissionHandler>>) -> TeamHandles {
        TeamHandles {
            results_tx: self.results_tx.clone(),
            markers_tx: self.markers_tx.clone(),
            permission,
            permission_sync: Arc::clone(&self.permission_sync),
        }
    }

    /// Track spawned teammates as background jobs so `TaskList` / `TaskStop` /
    /// `TaskOutput` can see them.
    pub fn set_job_registry(&mut self, registry: Arc<std::sync::Mutex<JobRegistry>>) {
        self.job_registry = Some(registry);
    }

    /// Fire task lifecycle hooks for spawned teammates.
    pub fn set_hook_executor(&mut self, hooks: Option<Arc<crab_hooks::HookExecutor>>) {
        self.hook_executor = hooks;
    }

    /// Set the retry policy for failed ephemeral tasks.
    pub fn set_retry_policy(&mut self, policy: RetryPolicy) {
        self.retry = RetryTracker::new(policy);
    }

    /// Set the team's collaboration mode, which decides who may message whom.
    pub fn set_mode(&mut self, mode: TeamMode) {
        self.backend.set_mode(mode);
    }

    /// The permission-sync bus, so teammates can subscribe and producers can
    /// broadcast user decisions.
    #[must_use]
    pub fn permission_sync(&self) -> &PermissionSyncManager {
        &self.permission_sync
    }

    /// Read-only view of the roster.
    #[must_use]
    pub fn team(&self) -> &Team {
        self.backend.team()
    }

    /// Number of teammates currently on the roster.
    #[must_use]
    pub fn teammate_count(&self) -> usize {
        self.backend.list_teammates().len()
    }

    // ── Turn processing ─────────────────────────────────────────────────

    /// Handle every team marker emitted during a turn and return the messages
    /// to fold back into the parent conversation.
    ///
    /// Ephemeral spawns are awaited inline — the turn does not finish until
    /// they do, matching a Task-style "return the final text" model. Resident
    /// teammates keep running; anything they finished since the last turn is
    /// included too.
    pub async fn process_turn(
        &mut self,
        markers: &[Value],
        base_prompt: &str,
    ) -> Vec<crab_core::message::Message> {
        let mut awaiting: HashSet<String> = HashSet::new();

        for marker in markers {
            match marker.get("action").and_then(Value::as_str) {
                Some(SPAWN_AGENT_ACTION) => {
                    if let Some(id) = self.spawn_from_marker(marker, base_prompt).await
                        && marker_name(marker).is_none()
                    {
                        awaiting.insert(id);
                    }
                }
                Some(MESSAGE_SENT_ACTION) => self.route_message(MAIN_AGENT, marker).await,
                _ => {}
            }
        }

        let mut folded = self.drain_teammate_markers().await;
        let mut results = std::mem::take(&mut self.pending);
        results.extend(self.await_results(awaiting).await);
        // Awaiting an ephemeral teammate can surface markers it emitted while
        // the turn was still running, so drain once more before returning.
        folded.extend(self.drain_teammate_markers().await);
        folded.extend(results.iter().map(agent_result_message));
        folded
    }

    /// Drain what teammates produced since the last call: finished results,
    /// markers they emitted from their own turns, and anything they addressed
    /// to the main agent.
    pub async fn drain_results(&mut self) -> Vec<crab_core::message::Message> {
        while let Ok(result) = self.results_rx.try_recv() {
            self.finish_result(&result);
            self.pending.push(result);
        }
        let mut folded = self.drain_teammate_markers().await;
        folded.extend(
            std::mem::take(&mut self.pending)
                .iter()
                .map(agent_result_message),
        );
        folded
    }

    /// Act on every marker teammates emitted since the last call, and fold
    /// anything they addressed to the main agent into the conversation.
    ///
    /// A teammate may message peers (subject to the team's mode) but may not
    /// spawn: `Agent` is stripped from its registry, so a spawn marker here
    /// means something bypassed that and is worth a warning rather than a
    /// silent unbounded recursion.
    async fn drain_teammate_markers(&mut self) -> Vec<crab_core::message::Message> {
        let mut pending = Vec::new();
        while let Ok(entry) = self.markers_rx.try_recv() {
            pending.push(entry);
        }
        for TeammateMarker { from, marker } in pending {
            match marker.get("action").and_then(Value::as_str) {
                Some(MESSAGE_SENT_ACTION) => self.route_message(&from, &marker).await,
                Some(SPAWN_AGENT_ACTION) => {
                    tracing::warn!(
                        teammate = %from,
                        "ignoring a spawn request from a teammate; teammates may not spawn teammates"
                    );
                }
                _ => {}
            }
        }
        self.drain_main_inbox()
    }

    /// Take everything teammates addressed to the main agent and render it as
    /// conversation messages.
    fn drain_main_inbox(&mut self) -> Vec<crab_core::message::Message> {
        let mut messages = Vec::new();
        while let Some(envelope) = self.backend.try_recv_for_main() {
            let AgentMessage::AssignTask { prompt, .. } = envelope.payload else {
                continue;
            };
            let name = self
                .backend
                .team()
                .by_id(&envelope.from)
                .map_or_else(|| envelope.from.clone(), |t| t.name.clone());
            messages.push(crab_core::message::Message::user(format!(
                "<teammate-message from=\"{name}\">\n{prompt}\n</teammate-message>"
            )));
        }
        messages
    }

    /// Await every ephemeral teammate in `awaiting`, buffering any resident
    /// results that arrive meanwhile.
    ///
    /// A teammate that exits without publishing a result — a panicking runner,
    /// a cancellation, a passive backend — yields a synthesized failure rather
    /// than stalling the turn forever.
    async fn await_results(&mut self, mut awaiting: HashSet<String>) -> Vec<WorkerResult> {
        let mut collected = Vec::new();
        while !awaiting.is_empty() {
            tokio::select! {
                biased;

                // Results first: a runner publishes its result just before
                // exiting, so draining them takes priority over exit notices.
                result = self.results_rx.recv() => {
                    let Some(result) = result else {
                        tracing::warn!(
                            outstanding = awaiting.len(),
                            "result channel closed with ephemeral teammates outstanding"
                        );
                        break;
                    };
                    self.finish_result(&result);
                    if awaiting.remove(&result.worker_id) {
                        self.backend.reap(&result.worker_id);
                        collected.push(result);
                    } else {
                        self.pending.push(result);
                    }
                }

                exited = self.backend.recv_exit() => {
                    let Some(id) = exited else {
                        tracing::warn!(
                            outstanding = awaiting.len(),
                            "exit channel closed with ephemeral teammates outstanding"
                        );
                        break;
                    };
                    if !awaiting.remove(&id) {
                        continue;
                    }
                    // The task ended without reporting; surface that instead
                    // of leaving the parent waiting on nothing.
                    let name = self
                        .backend
                        .team()
                        .by_id(&id)
                        .map_or_else(|| id.clone(), |t| t.name.clone());
                    let result = WorkerResult {
                        worker_id: id.clone(),
                        name,
                        output: None,
                        success: false,
                        error: Some("teammate exited without producing a result".to_string()),
                        usage: crab_core::model::TokenUsage::default(),
                    };
                    self.finish_result(&result);
                    self.backend.reap(&id);
                    collected.push(result);
                }
            }
        }
        collected
    }

    /// Record a finished task against the job registry, roster, and retry
    /// tracker.
    fn finish_result(&mut self, result: &WorkerResult) {
        let state = if result.success {
            TeammateState::Done
        } else {
            TeammateState::Failed
        };
        self.backend.set_state(&result.worker_id, state);

        if let Some(reg) = &self.job_registry {
            let mut reg = reg.lock().unwrap_or_else(|e| {
                tracing::warn!("job registry mutex poisoned: {e}");
                e.into_inner()
            });
            if result.success {
                reg.set_status(&result.worker_id, JobStatus::Completed);
            } else {
                reg.set_error(
                    &result.worker_id,
                    result
                        .error
                        .clone()
                        .unwrap_or_else(|| "teammate failed".to_string()),
                );
            }
        }

        if result.success {
            self.retry.on_success(&result.worker_id);
        } else if let RetryDecision::GiveUp { attempts_made } =
            self.retry.on_failure(&result.worker_id)
        {
            tracing::warn!(
                teammate = %result.worker_id,
                attempts_made,
                "teammate task failed and the retry budget is spent"
            );
        }
    }

    // ── Spawning ────────────────────────────────────────────────────────

    /// Spawn a teammate from a `spawn_agent` marker, returning its id.
    ///
    /// A repeat spawn under an existing name starts fresh: the old teammate is
    /// killed first, matching the Agent-tool contract that `SendMessage`
    /// continues an agent while a new `Agent` call replaces it.
    async fn spawn_from_marker(&mut self, marker: &Value, base_prompt: &str) -> Option<String> {
        let config = teammate_config_from_marker(marker, base_prompt)?;

        if !config.name.is_empty() {
            for id in self.backend.team().ids_named(&config.name) {
                if let Err(e) = self.backend.kill_teammate(&id).await {
                    tracing::warn!(error = %e, teammate = %id, "failed to kill stale teammate before respawn");
                }
            }
        }

        let preview = truncate_preview(&config.seed_task, 80);
        let resident = !config.lifetime.is_ephemeral();
        match self.spawn(config).await {
            Ok(id) => {
                self.register_job(&id, resident, preview);
                Some(id)
            }
            Err(e) => {
                tracing::warn!(error = %e, "team runner failed to spawn teammate");
                None
            }
        }
    }

    /// Spawn a teammate directly from a config.
    pub async fn spawn(&mut self, config: TeammateConfig) -> crab_core::Result<String> {
        self.backend.spawn_teammate(config).await
    }

    /// Record a spawned teammate in the background-job registry.
    fn register_job(&self, id: &str, resident: bool, description: String) {
        let Some(reg) = &self.job_registry else {
            return;
        };
        let job_type = if resident {
            JobType::InProcessTeammate
        } else {
            JobType::LocalAgent
        };
        {
            let mut reg = reg.lock().unwrap_or_else(|e| {
                tracing::warn!("job registry mutex poisoned: {e}");
                e.into_inner()
            });
            reg.register(BackgroundJob::new(id.to_owned(), job_type, description));
            reg.set_status(id, JobStatus::Running);
        }
        fire_task_hook(
            self.hook_executor.as_ref(),
            id,
            crab_hooks::HookTrigger::TaskCreated,
        );
    }

    // ── Messaging ───────────────────────────────────────────────────────

    /// Deliver a `message_sent` marker to the named teammate, or to every
    /// teammate for a `*` broadcast.
    ///
    /// Delivery goes through the team's router, so the collaboration mode
    /// decides who may reach whom: in `LeaderWorker` the main agent talks to
    /// anyone and workers only answer the leader; in `PeerToPeer` anyone may
    /// address anyone.
    ///
    /// `from` is the sender's roster id — [`MAIN_AGENT`] for the main agent's
    /// own markers, or a teammate's id for one it emitted itself.
    async fn route_message(&self, from: &str, marker: &Value) {
        let Some(message) = marker.get("message").and_then(Value::as_str) else {
            return;
        };
        let to = marker.get("to").and_then(Value::as_str).unwrap_or_default();

        if to == "*" {
            let envelope = Envelope::broadcast(from, assign(message));
            match self.backend.route(&envelope).await {
                Ok(0) => tracing::warn!("broadcast reached no teammate"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "broadcast failed"),
            }
            return;
        }

        // Resolve names to ids up front so the roster borrow is released
        // before the async sends. A name can map to several ids only
        // transiently (during a respawn), so deliver to each.
        let targets = self.backend.team().ids_named(to);
        if targets.is_empty() {
            tracing::warn!(to = %to, "no teammate matched a message_sent marker");
            return;
        }
        for id in targets {
            if id == from {
                continue;
            }
            let envelope = Envelope::new(from, &id, assign(message));
            match self.backend.route(&envelope).await {
                Ok(0) => {
                    tracing::warn!(teammate = %id, "message routed to a teammate with no mailbox");
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(error = %e, teammate = %id, "failed to deliver message");
                }
            }
        }
    }

    // ── Teardown ────────────────────────────────────────────────────────

    /// Kill every teammate. Called when the session ends — the implicit team's
    /// lifetime is the session's lifetime.
    pub async fn shutdown_all(&mut self) {
        for id in self.backend.team().all_ids() {
            if let Err(e) = self.backend.kill_teammate(&id).await {
                tracing::warn!(error = %e, teammate = %id, "failed to kill teammate at shutdown");
            }
        }
    }
}

/// Wrap a message body as a task assignment for a teammate's mailbox.
fn assign(message: &str) -> AgentMessage {
    AgentMessage::AssignTask {
        task_id: String::new(),
        prompt: message.to_owned(),
    }
}

/// Truncate a task prompt to a single-line preview for the job registry.
fn truncate_preview(text: &str, max_chars: usize) -> String {
    let first_line = text.lines().next().unwrap_or_default();
    if first_line.chars().count() <= max_chars {
        return first_line.to_owned();
    }
    let truncated: String = first_line.chars().take(max_chars).collect();
    format!("{truncated}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_team::roster::Lifetime;

    fn spawn_marker(task: &str, name: Option<&str>) -> Value {
        let mut v = serde_json::json!({"action": "spawn_agent", "task": task});
        if let Some(n) = name {
            v["name"] = Value::String(n.into());
        }
        v
    }

    #[test]
    fn a_new_runner_holds_only_the_main_agent() {
        let runner = TeamRunner::new();
        assert_eq!(runner.teammate_count(), 0);
        // The roster always seats the main agent as leader so routing rules
        // have a sender to reason about.
        assert_eq!(runner.team().len(), 1);
        assert!(runner.team().leader().unwrap().is_leader);
        assert_eq!(runner.team().leader().unwrap().name, MAIN_AGENT);
        assert_eq!(runner.permission_sync().subscriber_count(), 0);
    }

    #[tokio::test]
    async fn named_marker_spawns_resident_teammate() {
        let mut runner = TeamRunner::new();
        let id = runner
            .spawn_from_marker(&spawn_marker("review it", Some("alice")), "BASE")
            .await
            .unwrap();

        assert_eq!(runner.teammate_count(), 1);
        let teammate = runner.team().by_id(&id).unwrap();
        assert_eq!(teammate.name, "alice");
        assert_eq!(teammate.lifetime, Lifetime::Resident);
        assert_eq!(teammate.state, TeammateState::Running);

        runner.shutdown_all().await;
    }

    #[tokio::test]
    async fn unnamed_marker_spawns_ephemeral_teammate() {
        let mut runner = TeamRunner::new();
        let id = runner
            .spawn_from_marker(&spawn_marker("do x", None), "BASE")
            .await
            .unwrap();

        let teammate = runner.team().by_id(&id).unwrap();
        assert!(teammate.lifetime.is_ephemeral());
        // Ephemeral spawns are still addressable, by id.
        assert_eq!(teammate.name, id);

        runner.shutdown_all().await;
    }

    #[tokio::test]
    async fn respawn_under_same_name_replaces_the_teammate() {
        let mut runner = TeamRunner::new();
        let first = runner
            .spawn_from_marker(&spawn_marker("t1", Some("alice")), "BASE")
            .await
            .unwrap();
        let second = runner
            .spawn_from_marker(&spawn_marker("t2", Some("alice")), "BASE")
            .await
            .unwrap();

        assert_ne!(first, second);
        assert_eq!(runner.teammate_count(), 1);
        assert!(runner.team().by_id(&first).is_none());

        runner.shutdown_all().await;
    }

    #[tokio::test]
    async fn ephemeral_spawns_do_not_replace_each_other() {
        let mut runner = TeamRunner::new();
        runner
            .spawn_from_marker(&spawn_marker("a", None), "BASE")
            .await
            .unwrap();
        runner
            .spawn_from_marker(&spawn_marker("b", None), "BASE")
            .await
            .unwrap();
        assert_eq!(runner.teammate_count(), 2);
        runner.shutdown_all().await;
    }

    #[tokio::test]
    async fn shutdown_clears_the_roster() {
        let mut runner = TeamRunner::new();
        runner
            .spawn_from_marker(&spawn_marker("t", Some("alice")), "BASE")
            .await
            .unwrap();
        runner
            .spawn_from_marker(&spawn_marker("t", Some("bob")), "BASE")
            .await
            .unwrap();
        assert_eq!(runner.teammate_count(), 2);

        runner.shutdown_all().await;
        assert_eq!(runner.teammate_count(), 0);
    }

    #[tokio::test]
    async fn message_routes_to_the_named_teammate() {
        let mut runner = TeamRunner::new();
        runner
            .spawn_from_marker(&spawn_marker("t", Some("alice")), "BASE")
            .await
            .unwrap();

        // Passive teammates drain their mailbox; the assertion here is that
        // routing resolves a target and tolerates misses and empty bodies.
        runner
            .route_message(
                MAIN_AGENT,
                &serde_json::json!({"to": "alice", "message": "hi"}),
            )
            .await;
        runner
            .route_message(
                MAIN_AGENT,
                &serde_json::json!({"to": "nobody", "message": "hi"}),
            )
            .await;
        runner
            .route_message(
                MAIN_AGENT,
                &serde_json::json!({"to": "*", "message": "all hands"}),
            )
            .await;
        runner
            .route_message(MAIN_AGENT, &serde_json::json!({"to": "alice"}))
            .await;

        runner.shutdown_all().await;
    }

    #[tokio::test]
    async fn leader_worker_mode_blocks_worker_to_worker_messages() {
        let mut runner = TeamRunner::new();
        runner
            .spawn_from_marker(&spawn_marker("t", Some("alice")), "BASE")
            .await
            .unwrap();
        runner
            .spawn_from_marker(&spawn_marker("t", Some("bob")), "BASE")
            .await
            .unwrap();

        let alice = runner.backend.team().ids_named("alice")[0].clone();
        let bob = runner.backend.team().ids_named("bob")[0].clone();

        // The main agent leads, so it reaches any teammate.
        assert_eq!(
            runner
                .backend
                .route(&Envelope::new(MAIN_AGENT, &alice, assign("hi")))
                .await
                .unwrap(),
            1
        );
        // Two workers may not talk directly in the default mode.
        let err = runner
            .backend
            .route(&Envelope::new(&alice, &bob, assign("hi")))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not allowed"), "{err}");

        runner.shutdown_all().await;
    }

    #[tokio::test]
    async fn peer_to_peer_mode_allows_worker_to_worker_messages() {
        let mut runner = TeamRunner::new();
        runner.set_mode(TeamMode::PeerToPeer);
        runner
            .spawn_from_marker(&spawn_marker("t", Some("alice")), "BASE")
            .await
            .unwrap();
        runner
            .spawn_from_marker(&spawn_marker("t", Some("bob")), "BASE")
            .await
            .unwrap();

        let alice = runner.backend.team().ids_named("alice")[0].clone();
        let bob = runner.backend.team().ids_named("bob")[0].clone();
        assert_eq!(
            runner
                .backend
                .route(&Envelope::new(&alice, &bob, assign("hi")))
                .await
                .unwrap(),
            1
        );

        runner.shutdown_all().await;
    }

    #[tokio::test]
    async fn spawn_registers_a_background_job() {
        let mut runner = TeamRunner::new();
        let registry = Arc::new(std::sync::Mutex::new(JobRegistry::new()));
        runner.set_job_registry(Arc::clone(&registry));

        let id = runner
            .spawn_from_marker(
                &spawn_marker("review the auth module", Some("alice")),
                "BASE",
            )
            .await
            .unwrap();

        let job = registry
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .expect("teammate registered as a job");
        assert_eq!(job.job_type, JobType::InProcessTeammate);
        assert_eq!(job.status, JobStatus::Running);
        assert_eq!(job.description, "review the auth module");
    }

    #[tokio::test]
    async fn ephemeral_spawn_registers_as_local_agent_job() {
        let mut runner = TeamRunner::new();
        let registry = Arc::new(std::sync::Mutex::new(JobRegistry::new()));
        runner.set_job_registry(Arc::clone(&registry));

        let id = runner
            .spawn_from_marker(&spawn_marker("do x", None), "BASE")
            .await
            .unwrap();

        let job_type = registry.lock().unwrap().get(&id).unwrap().job_type;
        assert_eq!(job_type, JobType::LocalAgent);
    }

    #[tokio::test]
    async fn finish_result_updates_roster_and_job_registry() {
        let mut runner = TeamRunner::new();
        let registry = Arc::new(std::sync::Mutex::new(JobRegistry::new()));
        runner.set_job_registry(Arc::clone(&registry));
        let id = runner
            .spawn_from_marker(&spawn_marker("t", Some("alice")), "BASE")
            .await
            .unwrap();

        runner.finish_result(&WorkerResult {
            worker_id: id.clone(),
            name: "alice".into(),
            output: Some("done".into()),
            success: true,
            error: None,
            usage: crab_core::model::TokenUsage::default(),
        });

        assert_eq!(runner.team().by_id(&id).unwrap().state, TeammateState::Done);
        let status = registry.lock().unwrap().get(&id).unwrap().status;
        assert_eq!(status, JobStatus::Completed);
    }

    #[tokio::test]
    async fn failed_result_marks_teammate_failed() {
        let mut runner = TeamRunner::new();
        let registry = Arc::new(std::sync::Mutex::new(JobRegistry::new()));
        runner.set_job_registry(Arc::clone(&registry));
        let id = runner
            .spawn_from_marker(&spawn_marker("t", Some("alice")), "BASE")
            .await
            .unwrap();

        runner.finish_result(&WorkerResult {
            worker_id: id.clone(),
            name: "alice".into(),
            output: None,
            success: false,
            error: Some("backend exploded".into()),
            usage: crab_core::model::TokenUsage::default(),
        });

        assert_eq!(
            runner.team().by_id(&id).unwrap().state,
            TeammateState::Failed
        );
        let job = registry.lock().unwrap().get(&id).cloned().unwrap();
        assert_eq!(job.status, JobStatus::Failed);
        assert_eq!(job.error.as_deref(), Some("backend exploded"));
    }

    #[tokio::test]
    async fn process_turn_folds_resident_results_and_routes_messages() {
        let mut runner = TeamRunner::new();
        let markers = vec![
            spawn_marker("review", Some("alice")),
            serde_json::json!({"action": "message_sent", "to": "alice", "message": "go"}),
            serde_json::json!({"action": "unrelated"}),
        ];
        // Passive teammates never report, so nothing is awaited and the turn
        // returns immediately with no folded messages.
        let folded = runner.process_turn(&markers, "BASE").await;
        assert!(folded.is_empty());
        assert_eq!(runner.teammate_count(), 1);
        runner.shutdown_all().await;
    }

    #[tokio::test]
    async fn drain_results_returns_buffered_resident_output() {
        let mut runner = TeamRunner::new();
        let id = runner
            .spawn_from_marker(&spawn_marker("t", Some("alice")), "BASE")
            .await
            .unwrap();

        runner
            .results_tx
            .send(WorkerResult {
                worker_id: id,
                name: "alice".into(),
                output: Some("finished the review".into()),
                success: true,
                error: None,
                usage: crab_core::model::TokenUsage::default(),
            })
            .await
            .unwrap();

        let folded = runner.drain_results().await;
        assert_eq!(folded.len(), 1);
        assert!(folded[0].text().contains("finished the review"));
        assert!(folded[0].text().contains("name=\"alice\""));
        // Draining twice does not repeat the result.
        assert!(runner.drain_results().await.is_empty());

        runner.shutdown_all().await;
    }

    #[tokio::test]
    async fn a_teammate_that_exits_without_reporting_yields_a_failure() {
        // A passive backend runs no agent loop, so an ephemeral teammate ends
        // after its seed task without publishing anything. The turn must still
        // finish, with the miss surfaced rather than swallowed.
        let mut runner = TeamRunner::new();
        let folded = runner
            .process_turn(&[spawn_marker("do x", None)], "BASE")
            .await;

        assert_eq!(folded.len(), 1);
        let text = folded[0].text();
        assert!(text.contains("status=\"failed\""), "{text}");
        assert!(text.contains("exited without producing a result"), "{text}");
        // The ephemeral teammate is off the roster once its task is over.
        assert_eq!(runner.teammate_count(), 0);
    }

    #[tokio::test]
    async fn a_resident_teammate_outlives_the_turn() {
        let mut runner = TeamRunner::new();
        let folded = runner
            .process_turn(&[spawn_marker("review", Some("alice"))], "BASE")
            .await;
        // Nothing is awaited for a resident spawn, so the turn returns at once.
        assert!(folded.is_empty());
        assert_eq!(runner.teammate_count(), 1);
        runner.shutdown_all().await;
    }

    // ─── Teammate-emitted markers ───

    #[tokio::test]
    async fn a_teammate_message_to_the_main_agent_is_folded_in() {
        let mut runner = TeamRunner::new();
        let alice = runner
            .spawn_from_marker(&spawn_marker("t", Some("alice")), "BASE")
            .await
            .unwrap();

        runner
            .markers_tx
            .send(TeammateMarker {
                from: alice,
                marker: serde_json::json!({
                    "action": "message_sent",
                    "to": MAIN_AGENT,
                    "message": "found the bug in auth.rs",
                }),
            })
            .await
            .unwrap();

        let folded = runner.drain_results().await;
        assert_eq!(folded.len(), 1);
        let text = folded[0].text();
        assert!(text.contains("found the bug in auth.rs"), "{text}");
        assert!(text.contains("from=\"alice\""), "{text}");

        runner.shutdown_all().await;
    }

    #[tokio::test]
    async fn leader_worker_mode_drops_a_teammate_message_to_a_peer() {
        let mut runner = TeamRunner::new();
        let alice = runner
            .spawn_from_marker(&spawn_marker("t", Some("alice")), "BASE")
            .await
            .unwrap();
        runner
            .spawn_from_marker(&spawn_marker("t", Some("bob")), "BASE")
            .await
            .unwrap();

        runner
            .markers_tx
            .send(TeammateMarker {
                from: alice,
                marker: serde_json::json!({
                    "action": "message_sent",
                    "to": "bob",
                    "message": "psst",
                }),
            })
            .await
            .unwrap();

        // The default mode routes teammates to the leader only, so nothing
        // reaches bob and nothing lands in the main agent's inbox either.
        assert!(runner.drain_results().await.is_empty());

        runner.shutdown_all().await;
    }

    #[tokio::test]
    async fn peer_to_peer_mode_lets_a_teammate_reach_a_peer() {
        let mut runner = TeamRunner::new();
        runner.set_mode(TeamMode::PeerToPeer);
        let alice = runner
            .spawn_from_marker(&spawn_marker("t", Some("alice")), "BASE")
            .await
            .unwrap();
        let bob = runner.backend.team().ids_named("bob").first().cloned();
        assert!(bob.is_none(), "bob is not on the roster yet");
        runner
            .spawn_from_marker(&spawn_marker("t", Some("bob")), "BASE")
            .await
            .unwrap();

        runner
            .markers_tx
            .send(TeammateMarker {
                from: alice.clone(),
                marker: serde_json::json!({
                    "action": "message_sent",
                    "to": "bob",
                    "message": "psst",
                }),
            })
            .await
            .unwrap();

        // Delivered to bob, so nothing is folded into the parent conversation.
        assert!(runner.drain_results().await.is_empty());

        runner.shutdown_all().await;
    }

    #[tokio::test]
    async fn a_spawn_marker_from_a_teammate_is_refused() {
        let mut runner = TeamRunner::new();
        let alice = runner
            .spawn_from_marker(&spawn_marker("t", Some("alice")), "BASE")
            .await
            .unwrap();
        assert_eq!(runner.teammate_count(), 1);

        runner
            .markers_tx
            .send(TeammateMarker {
                from: alice,
                marker: spawn_marker("nested work", Some("carol")),
            })
            .await
            .unwrap();

        assert!(runner.drain_results().await.is_empty());
        assert_eq!(
            runner.teammate_count(),
            1,
            "a teammate must not be able to spawn another"
        );

        runner.shutdown_all().await;
    }

    #[tokio::test]
    async fn a_teammate_does_not_message_itself_on_broadcast_by_name() {
        let mut runner = TeamRunner::new();
        let alice = runner
            .spawn_from_marker(&spawn_marker("t", Some("alice")), "BASE")
            .await
            .unwrap();

        // Addressing its own name is a no-op rather than a self-send loop.
        runner
            .route_message(
                &alice,
                &serde_json::json!({"to": "alice", "message": "hello me"}),
            )
            .await;
        assert!(runner.drain_results().await.is_empty());

        runner.shutdown_all().await;
    }

    #[test]
    fn truncate_preview_takes_first_line_and_caps_length() {
        assert_eq!(truncate_preview("short", 80), "short");
        assert_eq!(truncate_preview("first\nsecond", 80), "first");
        // Counts codepoints, so multi-byte input does not panic.
        let long = "字".repeat(100);
        let preview = truncate_preview(&long, 10);
        assert_eq!(preview.chars().count(), 11); // 10 + ellipsis
        assert!(preview.ends_with('…'));
    }

    #[tokio::test]
    async fn taskless_marker_spawns_nothing() {
        let mut runner = TeamRunner::new();
        let marker = serde_json::json!({"action": "spawn_agent"});
        assert!(runner.spawn_from_marker(&marker, "BASE").await.is_none());
        assert_eq!(runner.teammate_count(), 0);
    }

    #[tokio::test]
    async fn set_mode_reaches_the_roster() {
        let mut runner = TeamRunner::new();
        runner.set_mode(TeamMode::PeerToPeer);
        assert_eq!(runner.team().mode, TeamMode::PeerToPeer);
    }
}
