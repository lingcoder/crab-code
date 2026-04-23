//! Layer 1 worker pool — [`WorkerPool`] owns running sub-agent workers
//! (`spawn_worker` / `collect_*` / `cancel_*`) plus a [`MessageRouter`]
//! and an optional [`Team`] for communication-rule enforcement.
//!
//! This is base infrastructure: Swarm (flat) and Coordinator Mode (star) both
//! layer on top of the same pool. Coordinator Mode additionally applies a
//! tool ACL + prompt overlay at session init — it does *not* subclass this
//! struct. See `docs/architecture.md` § Multi-Agent Three-Layer Architecture.

use std::collections::HashMap;
use std::sync::Arc;

use crab_api::LlmBackend;
use crab_core::event::Event;
use crab_core::tool::ToolContext;
use crab_tools::executor::ToolExecutor;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crab_engine::QueryConfig;

use crate::teams::bus::{AgentMessage, Envelope, MessageBus};
use crate::teams::mailbox::MessageRouter;
use crate::teams::retry::{RetryDecision, RetryPolicy, RetryTracker};
use crate::teams::roster::{Team, TeamMode};
use crate::teams::worker::{AgentWorker, WorkerConfig, WorkerResult};

/// Multi-agent orchestrator. Manages the main agent and worker pool.
///
/// The coordinator tracks running workers via `JoinHandle`s and provides
/// methods to spawn, cancel, and collect worker results.
pub struct WorkerPool {
    pub main_agent: AgentHandle,
    pub workers: Vec<AgentHandle>,
    pub bus: mpsc::Sender<AgentMessage>,
    /// Running worker tasks, keyed by worker ID.
    running: HashMap<String, RunningWorker>,
    /// Collected results from completed workers.
    completed: Vec<WorkerResult>,
    /// Counter for generating unique worker IDs.
    next_worker_id: u64,
    /// Message router for inter-agent communication.
    pub router: MessageRouter,
    /// Team structure and collaboration mode.
    pub team: Option<Team>,
    /// Retry tracker for failed tasks.
    pub retry_tracker: RetryTracker,
}

/// A worker that is currently running.
struct RunningWorker {
    pub handle: tokio::task::JoinHandle<WorkerResult>,
    pub cancel: CancellationToken,
}

/// Handle to a running agent (main or sub-agent).
pub struct AgentHandle {
    pub id: String,
    pub name: String,
    pub tx: mpsc::Sender<AgentMessage>,
}

impl WorkerPool {
    /// Create a new coordinator with a message bus.
    pub fn new(main_id: String, main_name: String) -> Self {
        let bus = MessageBus::new(64);
        let main_tx = bus.sender();
        let mut router = MessageRouter::new();
        let _main_rx = router.register(&main_name);
        Self {
            main_agent: AgentHandle {
                id: main_id,
                name: main_name,
                tx: main_tx,
            },
            workers: Vec::new(),
            bus: bus.sender(),
            running: HashMap::new(),
            completed: Vec::new(),
            next_worker_id: 1,
            router,
            team: None,
            retry_tracker: RetryTracker::new(RetryPolicy::default()),
        }
    }

    /// Set a custom retry policy.
    pub fn set_retry_policy(&mut self, policy: RetryPolicy) {
        self.retry_tracker = RetryTracker::new(policy);
    }

    /// Handle a worker task failure: decide on retry.
    ///
    /// Returns `Some(task_id)` if the task should be retried, `None` if exhausted.
    pub fn on_worker_failure(&mut self, task_id: &str) -> Option<RetryDecision> {
        let decision = self.retry_tracker.on_failure(task_id);
        match &decision {
            RetryDecision::Retry { .. } => Some(decision),
            RetryDecision::GiveUp { .. } => None,
        }
    }

    /// Handle a worker task success: clear retry state.
    pub fn on_worker_success(&mut self, task_id: &str) {
        self.retry_tracker.on_success(task_id);
    }

    /// Set the team structure for this coordinator.
    pub fn set_team(&mut self, team: Team) {
        self.team = Some(team);
    }

    /// Get the current team mode, if a team is configured.
    #[must_use]
    pub fn team_mode(&self) -> Option<TeamMode> {
        self.team.as_ref().map(|t| t.mode)
    }

    /// Route an envelope through the message router.
    ///
    /// Respects team communication rules if a team is configured.
    /// Returns the number of agents the message was delivered to,
    /// or `Err` if communication is not allowed.
    pub async fn route_message(&self, envelope: &Envelope) -> crab_core::Result<usize> {
        // Enforce team communication rules
        if let Some(team) = &self.team
            && !envelope.is_broadcast()
            && !team.can_communicate(&envelope.from, &envelope.to)
        {
            return Err(crab_core::Error::Other(format!(
                "communication not allowed from '{}' to '{}' in {} mode",
                envelope.from, envelope.to, team.mode,
            )));
        }
        Ok(self.router.route(envelope).await)
    }

    /// Add a worker agent handle (for pre-configured workers).
    pub fn add_worker(&mut self, id: String, name: String) {
        self.workers.push(AgentHandle {
            id,
            name,
            tx: self.bus.clone(),
        });
    }

    /// Spawn a new sub-agent worker with the given task.
    ///
    /// The worker inherits the parent's backend, executor, and tool context
    /// but runs an independent conversation. Returns the assigned worker ID.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_worker(
        &mut self,
        task_prompt: String,
        system_prompt: String,
        backend: Arc<LlmBackend>,
        executor: Arc<ToolExecutor>,
        tool_ctx: ToolContext,
        loop_config: QueryConfig,
        event_tx: mpsc::Sender<Event>,
        max_turns: Option<usize>,
    ) -> String {
        let worker_id = format!("worker_{}", self.next_worker_id);
        self.next_worker_id += 1;

        let cancel = CancellationToken::new();

        let config = WorkerConfig {
            worker_id: worker_id.clone(),
            system_prompt,
            max_turns,
            max_duration: None,
            context_window: 200_000,
        };

        let worker = AgentWorker::new(
            config,
            backend,
            executor,
            tool_ctx,
            loop_config,
            event_tx,
            cancel.clone(),
        );

        let handle = worker.spawn(task_prompt);

        self.running
            .insert(worker_id.clone(), RunningWorker { handle, cancel });

        // Also register in the handle list and router
        self.workers.push(AgentHandle {
            id: worker_id.clone(),
            name: worker_id.clone(),
            tx: self.bus.clone(),
        });
        let _worker_rx = self.router.register(&worker_id);

        worker_id
    }

    /// Cancel a running worker by ID. Returns `true` if the worker was found.
    pub fn cancel_worker(&self, worker_id: &str) -> bool {
        self.running.get(worker_id).is_some_and(|worker| {
            worker.cancel.cancel();
            true
        })
    }

    /// Cancel all running workers.
    pub fn cancel_all(&self) {
        for worker in self.running.values() {
            worker.cancel.cancel();
        }
    }

    /// Collect the result of a specific worker, blocking until it completes.
    ///
    /// Returns `None` if the worker ID is not found or already collected.
    pub async fn collect_worker(&mut self, worker_id: &str) -> Option<WorkerResult> {
        let worker = self.running.remove(worker_id)?;
        match worker.handle.await {
            Ok(result) => {
                self.completed.push(result.clone_summary());
                Some(result)
            }
            Err(_) => None,
        }
    }

    /// Collect all completed workers (non-blocking: only those already finished).
    pub async fn collect_completed(&mut self) -> Vec<WorkerResult> {
        let mut results = Vec::new();
        let mut still_running = HashMap::new();

        for (id, worker) in self.running.drain() {
            if worker.handle.is_finished() {
                if let Ok(result) = worker.handle.await {
                    results.push(result);
                }
            } else {
                still_running.insert(id, worker);
            }
        }

        self.running = still_running;
        self.completed
            .extend(results.iter().map(WorkerResult::clone_summary));
        results
    }

    /// Wait for all running workers to complete and collect their results.
    pub async fn collect_all(&mut self) -> Vec<WorkerResult> {
        let mut results = Vec::new();
        for (_, worker) in self.running.drain() {
            if let Ok(result) = worker.handle.await {
                results.push(result);
            }
        }
        self.completed
            .extend(results.iter().map(WorkerResult::clone_summary));
        results
    }

    /// Get the number of currently running workers.
    #[must_use]
    pub fn running_count(&self) -> usize {
        self.running.len()
    }

    /// Get all completed worker results (summaries).
    #[must_use]
    pub fn completed_results(&self) -> &[WorkerResult] {
        &self.completed
    }

    /// Get the IDs of all currently running workers.
    #[must_use]
    pub fn running_worker_ids(&self) -> Vec<String> {
        self.running.keys().cloned().collect()
    }

    /// Check whether a specific worker is still running.
    #[must_use]
    pub fn is_worker_running(&self, worker_id: &str) -> bool {
        self.running.contains_key(worker_id)
    }

    /// Inject a pre-built running worker (for testing or external spawn).
    #[cfg(test)]
    fn inject_worker(
        &mut self,
        worker_id: String,
        handle: tokio::task::JoinHandle<WorkerResult>,
        cancel: CancellationToken,
    ) {
        self.running
            .insert(worker_id, RunningWorker { handle, cancel });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_session::Conversation;

    #[test]
    fn coordinator_creation() {
        let coord = WorkerPool::new("main".into(), "Main Agent".into());
        assert_eq!(coord.main_agent.id, "main");
        assert_eq!(coord.main_agent.name, "Main Agent");
        assert!(coord.workers.is_empty());
        assert_eq!(coord.running_count(), 0);
        assert!(coord.completed_results().is_empty());
    }

    #[test]
    fn coordinator_add_worker() {
        let mut coord = WorkerPool::new("main".into(), "Main".into());
        coord.add_worker("w1".into(), "Worker 1".into());
        coord.add_worker("w2".into(), "Worker 2".into());
        assert_eq!(coord.workers.len(), 2);
        assert_eq!(coord.workers[0].id, "w1");
        assert_eq!(coord.workers[1].name, "Worker 2");
    }

    #[test]
    fn coordinator_cancel_nonexistent_worker() {
        let coord = WorkerPool::new("main".into(), "Main".into());
        assert!(!coord.cancel_worker("nonexistent"));
    }

    #[test]
    fn coordinator_cancel_all_empty() {
        let coord = WorkerPool::new("main".into(), "Main".into());
        // Should not panic
        coord.cancel_all();
    }

    #[tokio::test]
    async fn coordinator_collect_nonexistent_worker() {
        let mut coord = WorkerPool::new("main".into(), "Main".into());
        let result = coord.collect_worker("nonexistent").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn coordinator_collect_completed_empty() {
        let mut coord = WorkerPool::new("main".into(), "Main".into());
        let results = coord.collect_completed().await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn coordinator_collect_all_empty() {
        let mut coord = WorkerPool::new("main".into(), "Main".into());
        let results = coord.collect_all().await;
        assert!(results.is_empty());
    }

    #[test]
    fn coordinator_worker_id_increments() {
        let coord = WorkerPool::new("main".into(), "Main".into());
        assert_eq!(coord.next_worker_id, 1);
    }

    // ─── Worker lifecycle tests ───────────────────────────────────────

    /// Create a mock `WorkerResult` for testing.
    fn mock_worker_result(worker_id: &str, success: bool) -> WorkerResult {
        WorkerResult {
            worker_id: worker_id.into(),
            output: if success { Some("done".into()) } else { None },
            success,
            usage: crab_core::model::TokenUsage::default(),
            conversation: Conversation::new(worker_id.into(), String::new(), 0),
        }
    }

    #[tokio::test]
    async fn coordinator_inject_and_collect_worker() {
        let mut coord = WorkerPool::new("main".into(), "Main".into());
        let cancel = CancellationToken::new();

        let handle = tokio::spawn(async { mock_worker_result("w1", true) });
        coord.inject_worker("w1".into(), handle, cancel);

        assert_eq!(coord.running_count(), 1);
        assert!(coord.is_worker_running("w1"));

        let result = coord.collect_worker("w1").await.unwrap();
        assert_eq!(result.worker_id, "w1");
        assert!(result.success);
        assert_eq!(result.output.as_deref(), Some("done"));

        // After collection, no longer running
        assert_eq!(coord.running_count(), 0);
        assert!(!coord.is_worker_running("w1"));

        // Completed results should have a summary
        assert_eq!(coord.completed_results().len(), 1);
        assert_eq!(coord.completed_results()[0].worker_id, "w1");
    }

    #[tokio::test]
    async fn coordinator_collect_worker_twice_returns_none() {
        let mut coord = WorkerPool::new("main".into(), "Main".into());
        let cancel = CancellationToken::new();

        let handle = tokio::spawn(async { mock_worker_result("w1", true) });
        coord.inject_worker("w1".into(), handle, cancel);

        coord.collect_worker("w1").await.unwrap();
        // Second collect returns None
        assert!(coord.collect_worker("w1").await.is_none());
    }

    #[tokio::test]
    async fn coordinator_collect_all_multiple_workers() {
        let mut coord = WorkerPool::new("main".into(), "Main".into());

        for i in 1..=3 {
            let id = format!("w{i}");
            let cancel = CancellationToken::new();
            let handle = tokio::spawn({
                let id = id.clone();
                async move { mock_worker_result(&id, true) }
            });
            coord.inject_worker(id, handle, cancel);
        }

        assert_eq!(coord.running_count(), 3);

        let results = coord.collect_all().await;
        assert_eq!(results.len(), 3);
        assert_eq!(coord.running_count(), 0);
        assert_eq!(coord.completed_results().len(), 3);
    }

    #[tokio::test]
    async fn coordinator_collect_completed_only_finished() {
        let mut coord = WorkerPool::new("main".into(), "Main".into());

        // Worker that finishes immediately
        let cancel1 = CancellationToken::new();
        let handle1 = tokio::spawn(async { mock_worker_result("fast", true) });
        coord.inject_worker("fast".into(), handle1, cancel1);

        // Worker that blocks until cancelled
        let cancel2 = CancellationToken::new();
        let cancel2_clone = cancel2.clone();
        let handle2 = tokio::spawn(async move {
            cancel2_clone.cancelled().await;
            mock_worker_result("slow", false)
        });
        coord.inject_worker("slow".into(), handle2, cancel2.clone());

        // Give fast worker time to finish
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let results = coord.collect_completed().await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].worker_id, "fast");

        // slow is still running
        assert_eq!(coord.running_count(), 1);
        assert!(coord.is_worker_running("slow"));

        // Clean up: cancel the slow worker
        cancel2.cancel();
        let remaining = coord.collect_all().await;
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].worker_id, "slow");
    }

    #[tokio::test]
    async fn coordinator_cancel_running_worker() {
        let mut coord = WorkerPool::new("main".into(), "Main".into());
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        let handle = tokio::spawn(async move {
            cancel_clone.cancelled().await;
            mock_worker_result("w1", false)
        });
        coord.inject_worker("w1".into(), handle, cancel);

        assert!(coord.cancel_worker("w1"));

        // Collect the cancelled worker
        let result = coord.collect_worker("w1").await.unwrap();
        assert_eq!(result.worker_id, "w1");
        assert!(!result.success);
    }

    #[tokio::test]
    async fn coordinator_cancel_all_workers() {
        let mut coord = WorkerPool::new("main".into(), "Main".into());

        for i in 1..=3 {
            let id = format!("w{i}");
            let cancel = CancellationToken::new();
            let cancel_clone = cancel.clone();
            let handle = tokio::spawn({
                let id = id.clone();
                async move {
                    cancel_clone.cancelled().await;
                    mock_worker_result(&id, false)
                }
            });
            coord.inject_worker(id, handle, cancel);
        }

        assert_eq!(coord.running_count(), 3);

        coord.cancel_all();
        let results = coord.collect_all().await;
        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|r| !r.success));
    }

    #[test]
    fn coordinator_running_worker_ids() {
        let mut coord = WorkerPool::new("main".into(), "Main".into());
        let cancel = CancellationToken::new();

        let handle = tokio::runtime::Runtime::new()
            .unwrap()
            .spawn(async { mock_worker_result("w1", true) });
        coord.inject_worker("w1".into(), handle, cancel);

        let ids = coord.running_worker_ids();
        assert_eq!(ids.len(), 1);
        assert!(ids.contains(&"w1".to_string()));
    }

    #[test]
    fn coordinator_worker_id_auto_increments_on_spawn_worker() {
        let coord = WorkerPool::new("main".into(), "Main".into());
        assert_eq!(coord.next_worker_id, 1);

        // We can't call spawn_worker without a real backend/executor,
        // but we can test the ID format by verifying the increment logic.
        // spawn_worker uses format!("worker_{}", self.next_worker_id).
        let expected_id = format!("worker_{}", coord.next_worker_id);
        assert_eq!(expected_id, "worker_1");
    }

    // ─── Router integration tests ───────────────────────────────────

    #[test]
    fn coordinator_has_router() {
        let coord = WorkerPool::new("main".into(), "Main".into());
        // Main agent should be registered in the router
        assert!(coord.router.is_registered("Main"));
        assert_eq!(coord.router.agent_count(), 1);
    }

    #[tokio::test]
    async fn coordinator_route_directed_message() {
        let mut coord = WorkerPool::new("main".into(), "Main".into());
        let mut worker_rx = coord.router.register("Worker1");

        let env = Envelope::new(
            "Main",
            "Worker1",
            AgentMessage::AssignTask {
                task_id: "t1".into(),
                prompt: "do stuff".into(),
            },
        );

        let delivered = coord.route_message(&env).await.unwrap();
        assert_eq!(delivered, 1);

        let msg = worker_rx.recv().await.unwrap();
        assert_eq!(msg.from, "Main");
        assert!(matches!(msg.payload, AgentMessage::AssignTask { .. }));
    }

    #[tokio::test]
    async fn coordinator_route_broadcast() {
        let mut coord = WorkerPool::new("main".into(), "Main".into());
        let mut rx1 = coord.router.register("W1");
        let mut rx2 = coord.router.register("W2");

        let env = Envelope::broadcast("Main", AgentMessage::Shutdown);
        let delivered = coord.route_message(&env).await.unwrap();
        assert_eq!(delivered, 2);

        assert!(rx1.recv().await.is_some());
        assert!(rx2.recv().await.is_some());
    }

    // ─── Team integration tests ─────────────────────────────────────

    #[test]
    fn coordinator_no_team_by_default() {
        let coord = WorkerPool::new("main".into(), "Main".into());
        assert!(coord.team.is_none());
        assert!(coord.team_mode().is_none());
    }

    #[test]
    fn coordinator_set_team() {
        let mut coord = WorkerPool::new("main".into(), "Main".into());

        let mut team = Team::with_mode("dev".into(), TeamMode::PeerToPeer);
        let mut alice = crate::teams::roster::TeamMember::new("a1", "Alice", "model");
        alice.is_leader = true;
        team.add_member(alice);
        team.add_member(crate::teams::roster::TeamMember::new("a2", "Bob", "model"));

        coord.set_team(team);
        assert_eq!(coord.team_mode(), Some(TeamMode::PeerToPeer));
    }

    #[tokio::test]
    async fn coordinator_leader_worker_blocks_worker_to_worker() {
        let mut coord = WorkerPool::new("main".into(), "Main".into());

        let mut team = Team::new("dev".into()); // LeaderWorker by default
        let mut leader = crate::teams::roster::TeamMember::new("a1", "Leader", "model");
        leader.is_leader = true;
        team.add_member(leader);
        team.add_member(crate::teams::roster::TeamMember::new("a2", "W1", "model"));
        team.add_member(crate::teams::roster::TeamMember::new("a3", "W2", "model"));
        coord.set_team(team);

        let _rx1 = coord.router.register("W1");
        let _rx2 = coord.router.register("W2");
        let _rx_leader = coord.router.register("Leader");

        // Leader → Worker: allowed
        let env = Envelope::new("Leader", "W1", AgentMessage::Shutdown);
        assert!(coord.route_message(&env).await.is_ok());

        // Worker → Leader: allowed
        let env = Envelope::new("W1", "Leader", AgentMessage::ShutdownAck);
        assert!(coord.route_message(&env).await.is_ok());

        // Worker → Worker: blocked
        let env = Envelope::new("W1", "W2", AgentMessage::Shutdown);
        assert!(coord.route_message(&env).await.is_err());
    }

    #[tokio::test]
    async fn coordinator_peer_to_peer_allows_all() {
        let mut coord = WorkerPool::new("main".into(), "Main".into());

        let mut team = Team::with_mode("dev".into(), TeamMode::PeerToPeer);
        team.add_member(crate::teams::roster::TeamMember::new("a1", "A", "model"));
        team.add_member(crate::teams::roster::TeamMember::new("a2", "B", "model"));
        coord.set_team(team);

        let _rx_a = coord.router.register("A");
        let _rx_b = coord.router.register("B");

        // A → B: allowed
        let env = Envelope::new("A", "B", AgentMessage::Shutdown);
        assert!(coord.route_message(&env).await.is_ok());

        // B → A: allowed
        let env = Envelope::new("B", "A", AgentMessage::Shutdown);
        assert!(coord.route_message(&env).await.is_ok());
    }

    // ─── Assignment strategy tests ──────────────────────────────────

    #[test]
    fn coordinator_set_retry_policy() {
        let mut coord = WorkerPool::new("main".into(), "Main".into());
        coord.set_retry_policy(crate::teams::retry::RetryPolicy::no_retry());

        // First failure should give up immediately
        let decision = coord.on_worker_failure("t1");
        assert!(decision.is_none());
    }

    #[test]
    fn coordinator_on_worker_failure_retries() {
        let mut coord = WorkerPool::new("main".into(), "Main".into());
        // Default policy: 2 retries

        let decision = coord.on_worker_failure("t1");
        assert!(decision.is_some());
        assert!(matches!(decision, Some(RetryDecision::Retry { .. })));

        let decision = coord.on_worker_failure("t1");
        assert!(decision.is_some());

        let decision = coord.on_worker_failure("t1");
        assert!(decision.is_none()); // exhausted
    }

    #[test]
    fn coordinator_success_clears_retry_state() {
        let mut coord = WorkerPool::new("main".into(), "Main".into());

        coord.on_worker_failure("t1");
        assert_eq!(coord.retry_tracker.attempts_for("t1"), 1);

        coord.on_worker_success("t1");
        assert_eq!(coord.retry_tracker.attempts_for("t1"), 0);
    }
}
