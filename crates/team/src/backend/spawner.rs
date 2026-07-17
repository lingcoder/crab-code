//! Backend-agnostic teammate execution.
//!
//! [`TeammateBackend`] defines the trait for spawning and managing teammate
//! sub-agents. A single in-process implementation aligns with Claude
//! Code's teammate lifetime model: each teammate is a tokio task with
//! mpsc IPC, so permissions, tool registries, and state live in the
//! parent process.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::backend::teammate::{Teammate, TeammateConfig, TeammateState};

/// Everything a teammate runner needs to drive one teammate task.
pub struct TeammateRunCtx {
    /// Backend-assigned teammate id.
    pub id: String,
    /// Spawn configuration (name, role, system prompt, working dir).
    pub config: TeammateConfig,
    /// Inbound message channel — the runner awaits messages (e.g. tasks) here.
    pub rx: mpsc::Receiver<String>,
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
///
/// Implementations manage the lifecycle of teammate sub-agents: spawning,
/// messaging, listing, and killing.
pub trait TeammateBackend: Send {
    /// Spawn a new teammate and return its unique ID.
    fn spawn_teammate(
        &mut self,
        config: TeammateConfig,
    ) -> impl std::future::Future<Output = crab_core::Result<String>> + Send;

    /// Kill a teammate by ID.
    fn kill_teammate(
        &mut self,
        id: &str,
    ) -> impl std::future::Future<Output = crab_core::Result<()>> + Send;

    /// Send a text message to a teammate.
    fn send_message(
        &self,
        id: &str,
        message: &str,
    ) -> impl std::future::Future<Output = crab_core::Result<()>> + Send;

    /// List all tracked teammates.
    fn list_teammates(&self) -> Vec<&Teammate>;
}

// ─── InProcessBackend ────────────────────────────────────────────────────────

/// A running in-process teammate entry.
struct InProcessEntry {
    teammate: Teammate,
    tx: mpsc::Sender<String>,
    cancel: CancellationToken,
    handle: tokio::task::JoinHandle<()>,
}

/// In-process teammate backend using tokio tasks and mpsc channels.
///
/// Each teammate runs as a spawned tokio task that reads from its own
/// mpsc channel. The task loops until cancelled or the channel closes.
pub struct InProcessBackend {
    entries: HashMap<String, InProcessEntry>,
    next_id: u64,
    /// Optional runner that drives each teammate's work loop. When unset,
    /// teammates are passive (drain their channel until cancelled) — the
    /// previous logging-sink behavior, kept for tests and headless callers.
    runner: Option<TeammateRunner>,
}

impl InProcessBackend {
    /// Create a new empty in-process backend with passive teammates.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            next_id: 0,
            runner: None,
        }
    }

    /// Create a backend whose teammates run the given runner closure.
    #[must_use]
    pub fn with_runner(runner: TeammateRunner) -> Self {
        Self {
            entries: HashMap::new(),
            next_id: 0,
            runner: Some(runner),
        }
    }

    /// Install a runner if none is set yet. Idempotent: a second call is a
    /// no-op, so already-spawned teammates keep their original loop.
    pub fn ensure_runner(&mut self, make: impl FnOnce() -> TeammateRunner) {
        if self.runner.is_none() {
            self.runner = Some(make());
        }
    }
}

impl TeammateBackend for InProcessBackend {
    async fn spawn_teammate(&mut self, config: TeammateConfig) -> crab_core::Result<String> {
        let id = format!("ip-{}", self.next_id);
        self.next_id += 1;

        let mut teammate = Teammate::new(&id, &config.name, &config.role);
        teammate.set_state(TeammateState::Running);

        let (tx, mut rx) = mpsc::channel::<String>(64);
        let cancel = CancellationToken::new();

        let handle = if let Some(runner) = &self.runner {
            // Drive a real teammate work loop via the injected runner.
            tokio::spawn(runner(TeammateRunCtx {
                id: id.clone(),
                config,
                rx,
                cancel: cancel.clone(),
            }))
        } else {
            // Passive teammate: drain the channel until cancelled or closed.
            let cancel_clone = cancel.clone();
            let teammate_id = id.clone();
            tokio::spawn(async move {
                tracing::debug!(teammate_id, "in-process teammate started (passive)");
                loop {
                    tokio::select! {
                        () = cancel_clone.cancelled() => break,
                        msg = rx.recv() => {
                            if msg.is_none() {
                                break;
                            }
                        }
                    }
                }
            })
        };

        self.entries.insert(
            id.clone(),
            InProcessEntry {
                teammate,
                tx,
                cancel,
                handle,
            },
        );

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

        Ok(())
    }

    async fn send_message(&self, id: &str, message: &str) -> crab_core::Result<()> {
        let entry = self
            .entries
            .get(id)
            .ok_or_else(|| crab_core::Error::Other(format!("teammate not found: {id}")))?;

        entry
            .tx
            .send(message.to_owned())
            .await
            .map_err(|e| crab_core::Error::Other(format!("send failed: {e}")))?;

        Ok(())
    }

    fn list_teammates(&self) -> Vec<&Teammate> {
        self.entries.values().map(|e| &e.teammate).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn in_process_spawn_and_list() {
        let mut backend = InProcessBackend::new();
        let config = TeammateConfig::new("Alice", "reviewer");
        let id = backend.spawn_teammate(config).await.unwrap();

        let teammates = backend.list_teammates();
        assert_eq!(teammates.len(), 1);
        assert_eq!(teammates[0].id, id);
        assert_eq!(teammates[0].name, "Alice");
        assert!(teammates[0].is_running());

        // Cleanup
        backend.kill_teammate(&id).await.unwrap();
    }

    #[tokio::test]
    async fn in_process_send_and_kill() {
        let mut backend = InProcessBackend::new();
        let config = TeammateConfig::new("Bob", "tester");
        let id = backend.spawn_teammate(config).await.unwrap();

        // Send a message — should not error
        backend.send_message(&id, "hello teammate").await.unwrap();

        // Kill the teammate
        backend.kill_teammate(&id).await.unwrap();
        assert!(backend.list_teammates().is_empty());
    }

    #[tokio::test]
    async fn in_process_spawn_multiple() {
        let mut backend = InProcessBackend::new();

        let id1 = backend
            .spawn_teammate(TeammateConfig::new("Alice", "reviewer"))
            .await
            .unwrap();
        let id2 = backend
            .spawn_teammate(TeammateConfig::new("Bob", "tester"))
            .await
            .unwrap();

        assert_ne!(id1, id2);
        assert_eq!(backend.list_teammates().len(), 2);

        // Cleanup
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
            .spawn_teammate(TeammateConfig::new("A", "reviewer"))
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
    async fn in_process_kill_nonexistent() {
        let mut backend = InProcessBackend::new();
        let result = backend.kill_teammate("no-such-id").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn in_process_send_to_nonexistent() {
        let backend = InProcessBackend::new();
        let result = backend.send_message("no-such-id", "hello").await;
        assert!(result.is_err());
    }
}
