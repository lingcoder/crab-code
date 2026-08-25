//! Backend-agnostic teammate execution.
//!
//! [`TeammateBackend`] defines the trait for spawning and managing teammate
//! agent instances. The in-process implementation runs each teammate as a
//! tokio task with mpsc IPC, so permissions, tool registries, and state live
//! in the parent process.
//!
//! The backend owns the [`Team`] roster: every spawn — ephemeral or resident —
//! becomes a roster member, so the roster, the routing rules, and the TUI view
//! all read from one place.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::backend::teammate::TeammateConfig;
use crate::bus::{AgentMessage, Envelope};
use crate::mailbox::MessageRouter;
use crate::roster::{Lifetime, Team, TeamMode, Teammate, TeammateState};

/// Everything a teammate runner needs to drive one teammate task.
pub struct TeammateRunCtx {
    /// Backend-assigned teammate id.
    pub id: String,
    /// Spawn configuration (name, role, lifetime, prompt, limits).
    pub config: TeammateConfig,
    /// This teammate's mailbox, handed out by the router. Resident teammates
    /// loop on it; ephemeral teammates take the seed task and exit.
    pub rx: mpsc::Receiver<Envelope>,
    /// Cancellation token; the runner must exit promptly when triggered.
    pub cancel: CancellationToken,
}

/// A teammate runner: builds and drives a teammate's work loop.
///
/// Injected by an upper layer (the agents crate) so this crate stays free of
/// any LLM/engine dependency — `crab-team` only knows how to call the closure.
pub type TeammateRunner =
    Arc<dyn Fn(TeammateRunCtx) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Trait for teammate execution backends.
pub trait TeammateBackend: Send {
    /// Spawn a new teammate and return its unique id.
    fn spawn_teammate(
        &mut self,
        config: TeammateConfig,
    ) -> impl std::future::Future<Output = crab_core::Result<String>> + Send;

    /// Kill a teammate by id.
    fn kill_teammate(
        &mut self,
        id: &str,
    ) -> impl std::future::Future<Output = crab_core::Result<()>> + Send;

    /// Deliver a task straight to a teammate's mailbox, bypassing the team's
    /// routing rules. Used to seed a freshly spawned teammate.
    fn send_message(
        &self,
        id: &str,
        message: &str,
    ) -> impl std::future::Future<Output = crab_core::Result<()>> + Send;

    /// The teammates this backend spawned. Excludes the main agent, which is
    /// on the roster but is not a backend-managed task.
    fn list_teammates(&self) -> Vec<&Teammate>;
}

// ─── InProcessBackend ────────────────────────────────────────────────────────

/// How many teammate exits may queue up before a teammate's exit send blocks.
const EXIT_CHANNEL_CAPACITY: usize = 64;

/// The roster name of the session's main agent.
///
/// The main agent is a roster member like any teammate — it leads the team —
/// so one set of `can_communicate` rules covers parent/teammate traffic
/// instead of a parallel special case.
pub const MAIN_AGENT: &str = "main";

/// A running in-process teammate's execution handles.
struct InProcessEntry {
    cancel: CancellationToken,
    handle: tokio::task::JoinHandle<()>,
}

/// In-process teammate backend using tokio tasks and mpsc channels.
///
/// Each teammate runs as a spawned tokio task reading from its own mpsc
/// channel. The roster ([`Team`]) is the source of truth for identity and
/// state; `entries` holds the matching execution handles by id.
pub struct InProcessBackend {
    team: Team,
    /// Delivers every message, the parent's included, so there is one mailbox
    /// per teammate rather than a router beside a side channel.
    router: MessageRouter,
    entries: HashMap<String, InProcessEntry>,
    next_id: u64,
    /// Optional runner that drives each teammate's work loop. When unset,
    /// teammates are passive (drain their channel until cancelled) — used by
    /// tests and headless callers that have no LLM backend.
    runner: Option<TeammateRunner>,
    /// Every teammate task reports its own id here when it exits, whether it
    /// finished cleanly, was cancelled, or panicked. Without this a caller
    /// awaiting a teammate's result would hang forever on a runner that dies
    /// before reporting.
    exit_tx: mpsc::Sender<String>,
    exit_rx: mpsc::Receiver<String>,
    /// The main agent's own mailbox, so teammates can address it back.
    main_inbox: mpsc::Receiver<Envelope>,
}

impl Default for InProcessBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl InProcessBackend {
    /// Create an empty backend with passive teammates.
    #[must_use]
    pub fn new() -> Self {
        let (exit_tx, exit_rx) = mpsc::channel(EXIT_CHANNEL_CAPACITY);

        // Seat the main agent as the team leader so the routing rules have a
        // sender to reason about.
        let mut team = Team::new("session".into());
        let mut main = Teammate::new(MAIN_AGENT, MAIN_AGENT, "lead", Lifetime::Resident);
        main.is_leader = true;
        main.set_state(TeammateState::Running);
        team.add_member(main);

        let mut router = MessageRouter::new();
        let main_inbox = router.register(MAIN_AGENT);

        Self {
            team,
            router,
            entries: HashMap::new(),
            next_id: 0,
            runner: None,
            exit_tx,
            exit_rx,
            main_inbox,
        }
    }

    /// Take a message a teammate addressed to the main agent, if one is
    /// waiting. Non-blocking.
    pub fn try_recv_for_main(&mut self) -> Option<Envelope> {
        self.main_inbox.try_recv().ok()
    }

    /// Route an envelope under the team's collaboration rules.
    ///
    /// Returns how many teammates it reached, or an error when the team's mode
    /// forbids the pair. Broadcasts skip the check: `MessageRouter` already
    /// excludes the sender, and a broadcast is not a directed message.
    pub async fn route(&self, envelope: &Envelope) -> crab_core::Result<usize> {
        if !envelope.is_broadcast() && !self.team.can_communicate(&envelope.from, &envelope.to) {
            return Err(crab_core::Error::Other(format!(
                "communication not allowed from '{}' to '{}' in {} mode",
                envelope.from, envelope.to, self.team.mode,
            )));
        }
        Ok(self.router.route(envelope).await)
    }

    /// Create a backend whose teammates run the given runner closure.
    #[must_use]
    pub fn with_runner(runner: TeammateRunner) -> Self {
        Self {
            runner: Some(runner),
            ..Self::new()
        }
    }

    /// Install a runner if none is set yet. Idempotent: a second call is a
    /// no-op, so already-spawned teammates keep their original loop.
    pub fn ensure_runner(&mut self, make: impl FnOnce() -> TeammateRunner) {
        if self.runner.is_none() {
            self.runner = Some(make());
        }
    }

    /// Read-only access to the roster.
    #[must_use]
    pub fn team(&self) -> &Team {
        &self.team
    }

    /// Mutable access to the roster.
    pub fn team_mut(&mut self) -> &mut Team {
        &mut self.team
    }

    /// Set the team's collaboration mode.
    pub fn set_mode(&mut self, mode: TeamMode) {
        self.team.mode = mode;
    }

    /// Update a teammate's lifecycle state. No-op if the id is unknown.
    pub fn set_state(&mut self, id: &str, state: TeammateState) {
        if let Some(t) = self.team.by_id_mut(id) {
            t.set_state(state);
        }
    }

    /// Await the next teammate exit, yielding its id.
    ///
    /// Resolves for every teammate task that ends, including one that dies
    /// without publishing a result, so callers can stop waiting on it.
    pub async fn recv_exit(&mut self) -> Option<String> {
        self.exit_rx.recv().await
    }

    /// Ids of teammates whose task has finished, so the caller can reap them.
    #[must_use]
    pub fn finished_ids(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|(_, e)| e.handle.is_finished())
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Drop a finished teammate's roster entry and handles without cancelling
    /// (the task already exited). Returns the removed teammate.
    pub fn reap(&mut self, id: &str) -> Option<Teammate> {
        self.entries.remove(id);
        self.router.unregister(id);
        self.team.remove(id)
    }
}

impl TeammateBackend for InProcessBackend {
    async fn spawn_teammate(&mut self, config: TeammateConfig) -> crab_core::Result<String> {
        let id = format!("ip-{}", self.next_id);
        self.next_id += 1;

        // Ephemeral spawns carry no addressable name; use the id so every
        // roster member is still reachable by a stable name.
        let name = if config.name.trim().is_empty() {
            id.clone()
        } else {
            config.name.clone()
        };

        let mut teammate = Teammate::new(&id, name, &config.role, config.lifetime);
        teammate.model = config.model.clone();
        teammate.capabilities = config.capabilities.clone();
        teammate.set_state(TeammateState::Running);

        // Register by id: names repeat across respawns, ids never do.
        let mut rx = self.router.register(&id);
        let cancel = CancellationToken::new();

        let seed = config.seed_task.clone();
        let ephemeral = config.lifetime.is_ephemeral();
        let task: Pin<Box<dyn Future<Output = ()> + Send>> = if let Some(runner) = &self.runner {
            runner(TeammateRunCtx {
                id: id.clone(),
                config,
                rx,
                cancel: cancel.clone(),
            })
        } else {
            // Passive teammate: drain the mailbox until cancelled or closed.
            // An ephemeral one still exits after its single task, so callers
            // awaiting it are not left hanging.
            let cancel_clone = cancel.clone();
            let teammate_id = id.clone();
            Box::pin(async move {
                tracing::debug!(teammate_id, "in-process teammate started (passive)");
                loop {
                    tokio::select! {
                        () = cancel_clone.cancelled() => break,
                        msg = rx.recv() => {
                            if msg.is_none() || ephemeral {
                                break;
                            }
                        }
                    }
                }
            })
        };

        // Announce the exit whatever the reason, so no caller waits forever.
        let exit_tx = self.exit_tx.clone();
        let exit_id = id.clone();
        let handle = tokio::spawn(async move {
            task.await;
            let _ = exit_tx.send(exit_id).await;
        });

        self.team.add_member(teammate);
        self.entries
            .insert(id.clone(), InProcessEntry { cancel, handle });

        // Seed the teammate with its first task so a spawn is self-contained.
        if !seed.is_empty() {
            self.send_message(&id, &seed).await?;
        }

        Ok(id)
    }

    async fn kill_teammate(&mut self, id: &str) -> crab_core::Result<()> {
        let entry = self
            .entries
            .remove(id)
            .ok_or_else(|| crab_core::Error::Other(format!("teammate not found: {id}")))?;

        entry.cancel.cancel();
        // Best-effort await — if the task panicked we still succeed.
        let _ = entry.handle.await;
        self.router.unregister(id);
        self.team.remove(id);

        Ok(())
    }

    async fn send_message(&self, id: &str, message: &str) -> crab_core::Result<()> {
        let tx = self
            .router
            .get_sender(id)
            .ok_or_else(|| crab_core::Error::Other(format!("teammate not found: {id}")))?;

        tx.send(Envelope::new(
            MAIN_AGENT,
            id,
            AgentMessage::AssignTask {
                task_id: id.to_owned(),
                prompt: message.to_owned(),
            },
        ))
        .await
        .map_err(|e| crab_core::Error::Other(format!("send failed: {e}")))?;

        Ok(())
    }

    fn list_teammates(&self) -> Vec<&Teammate> {
        self.team
            .members()
            .iter()
            .filter(|t| self.entries.contains_key(&t.id))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roster::Lifetime;

    fn resident(name: &str, role: &str) -> TeammateConfig {
        TeammateConfig::new(name, role, Lifetime::Resident)
    }

    #[tokio::test]
    async fn spawn_registers_on_roster() {
        let mut backend = InProcessBackend::new();
        let id = backend
            .spawn_teammate(resident("Alice", "reviewer"))
            .await
            .unwrap();

        let teammates = backend.list_teammates();
        assert_eq!(teammates.len(), 1);
        assert_eq!(teammates[0].id, id);
        assert_eq!(teammates[0].name, "Alice");
        assert!(teammates[0].is_running());
        assert_eq!(backend.team().get_member("Alice").unwrap().id, id);

        backend.kill_teammate(&id).await.unwrap();
    }

    #[tokio::test]
    async fn ephemeral_spawn_is_named_after_its_id() {
        let mut backend = InProcessBackend::new();
        let id = backend
            .spawn_teammate(TeammateConfig::new("", "worker", Lifetime::ephemeral()))
            .await
            .unwrap();
        assert_eq!(backend.team().by_id(&id).unwrap().name, id);
        backend.kill_teammate(&id).await.unwrap();
    }

    #[tokio::test]
    async fn spawn_seeds_first_task() {
        let (seen_tx, mut seen_rx) = mpsc::channel::<String>(4);
        let runner: TeammateRunner = Arc::new(move |mut ctx: TeammateRunCtx| {
            let seen_tx = seen_tx.clone();
            Box::pin(async move {
                if let Some(envelope) = ctx.rx.recv().await
                    && let crate::bus::AgentMessage::AssignTask { prompt, .. } = envelope.payload
                {
                    let _ = seen_tx.send(prompt).await;
                }
                ctx.cancel.cancelled().await;
            })
        });

        let mut backend = InProcessBackend::with_runner(runner);
        let id = backend
            .spawn_teammate(resident("Alice", "reviewer").with_seed_task("do the thing"))
            .await
            .unwrap();

        assert_eq!(seen_rx.recv().await.unwrap(), "do the thing");
        backend.kill_teammate(&id).await.unwrap();
    }

    #[tokio::test]
    async fn send_and_kill() {
        let mut backend = InProcessBackend::new();
        let id = backend
            .spawn_teammate(resident("Bob", "tester"))
            .await
            .unwrap();

        backend.send_message(&id, "hello teammate").await.unwrap();

        backend.kill_teammate(&id).await.unwrap();
        assert!(backend.list_teammates().is_empty());
        // The main agent stays on the roster; only teammates are torn down.
        assert_eq!(backend.team().len(), 1);
        assert!(backend.team().by_id(&id).is_none());
    }

    #[tokio::test]
    async fn spawn_multiple_distinct_ids() {
        let mut backend = InProcessBackend::new();
        let id1 = backend
            .spawn_teammate(resident("Alice", "reviewer"))
            .await
            .unwrap();
        let id2 = backend
            .spawn_teammate(resident("Bob", "tester"))
            .await
            .unwrap();

        assert_ne!(id1, id2);
        assert_eq!(backend.list_teammates().len(), 2);

        backend.kill_teammate(&id1).await.unwrap();
        backend.kill_teammate(&id2).await.unwrap();
        assert!(backend.list_teammates().is_empty());
    }

    #[tokio::test]
    async fn with_runner_invokes_runner_on_spawn() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let ran = Arc::new(AtomicBool::new(false));
        let ran_in = Arc::clone(&ran);
        let runner: TeammateRunner = Arc::new(move |ctx: TeammateRunCtx| {
            let ran = Arc::clone(&ran_in);
            Box::pin(async move {
                ran.store(true, Ordering::SeqCst);
                ctx.cancel.cancelled().await;
            })
        });

        let mut backend = InProcessBackend::with_runner(runner);
        let id = backend
            .spawn_teammate(resident("A", "reviewer"))
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            ran.load(Ordering::SeqCst),
            "runner should have been invoked"
        );
        backend.kill_teammate(&id).await.unwrap();
    }

    #[tokio::test]
    async fn ensure_runner_is_idempotent() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = Arc::new(AtomicUsize::new(0));

        let mut backend = InProcessBackend::new();
        for _ in 0..2 {
            let calls = Arc::clone(&calls);
            backend.ensure_runner(move || {
                calls.fetch_add(1, Ordering::SeqCst);
                Arc::new(|ctx: TeammateRunCtx| {
                    Box::pin(async move {
                        ctx.cancel.cancelled().await;
                    })
                })
            });
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn finished_ids_and_reap() {
        let runner: TeammateRunner = Arc::new(|_ctx: TeammateRunCtx| Box::pin(async move {}));
        let mut backend = InProcessBackend::with_runner(runner);
        let id = backend
            .spawn_teammate(TeammateConfig::new("", "worker", Lifetime::ephemeral()))
            .await
            .unwrap();

        // Give the immediately-returning runner a chance to finish.
        for _ in 0..50 {
            if !backend.finished_ids().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(backend.finished_ids(), vec![id.clone()]);

        let reaped = backend.reap(&id).unwrap();
        assert_eq!(reaped.id, id);
        assert!(backend.list_teammates().is_empty());
        assert!(backend.reap(&id).is_none());
    }

    #[tokio::test]
    async fn set_state_updates_roster() {
        let mut backend = InProcessBackend::new();
        let id = backend
            .spawn_teammate(resident("Alice", "reviewer"))
            .await
            .unwrap();
        backend.set_state(&id, TeammateState::Done);
        assert_eq!(
            backend.team().by_id(&id).unwrap().state,
            TeammateState::Done
        );
        backend.set_state("nope", TeammateState::Failed);
        backend.kill_teammate(&id).await.unwrap();
    }

    #[tokio::test]
    async fn kill_and_send_to_nonexistent_error() {
        let mut backend = InProcessBackend::new();
        assert!(backend.kill_teammate("no-such-id").await.is_err());
        assert!(backend.send_message("no-such-id", "hello").await.is_err());
    }
}
