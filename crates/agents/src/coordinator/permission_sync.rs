//! Cross-agent permission decision synchronization.
//!
//! Teammates have no terminal of their own, so a teammate that needs
//! confirmation asks through the *session's* permission handler — the same one
//! the main agent uses. That alone gives one prompt per decision and lets the
//! frontend's existing session grants cover every agent.
//!
//! What only teams need is coalescing: teammates run concurrently, so two of
//! them reaching for the same tool at the same moment would raise two cards for
//! one question. [`PermissionSyncManager::resolve`] lets the first asker prompt
//! and every concurrent asker await that answer, then broadcasts the decision
//! so the rest of the session can observe it.

use std::collections::HashMap;
use std::time::Instant;

use crab_core::permission::PermissionDecision;
use crab_tools::executor::PermissionResult;
use tokio::sync::{Mutex, broadcast, watch};

/// A permission decision event broadcast across teammates.
#[derive(Debug, Clone)]
pub struct PermissionDecisionEvent {
    /// The tool name the decision applies to (e.g. `"Bash"`, `"Edit"`).
    pub tool_name: String,
    /// The decision that was made.
    pub decision: PermissionDecision,
    /// Which agent originated this decision.
    pub agent_id: String,
    /// When the decision was made.
    pub timestamp: Instant,
}

impl PermissionDecisionEvent {
    /// Create a new permission decision event stamped at the current instant.
    #[must_use]
    pub fn new(
        tool_name: impl Into<String>,
        decision: PermissionDecision,
        agent_id: impl Into<String>,
    ) -> Self {
        Self {
            tool_name: tool_name.into(),
            decision,
            agent_id: agent_id.into(),
            timestamp: Instant::now(),
        }
    }
}

/// Coalesces concurrent permission requests and broadcasts their outcomes.
///
/// The broadcast half is backed by [`tokio::sync::broadcast`], so every
/// subscriber receives every event and slow subscribers silently skip older
/// ones.
pub struct PermissionSyncManager {
    tx: broadcast::Sender<PermissionDecisionEvent>,
    /// Requests currently awaiting a user answer, keyed by request key. The
    /// first asker owns the entry; concurrent askers clone its receiver.
    inflight: Mutex<HashMap<String, watch::Receiver<Option<PermissionResult>>>>,
}

impl PermissionSyncManager {
    /// Create a new manager with the given broadcast capacity.
    ///
    /// `capacity` determines how many un-consumed events the channel buffers
    /// before older entries are dropped for slow subscribers.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self {
            tx,
            inflight: Mutex::new(HashMap::new()),
        }
    }

    /// Subscribe to permission decision events.
    ///
    /// Returns a receiver for all future events; earlier events are not
    /// replayed.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<PermissionDecisionEvent> {
        self.tx.subscribe()
    }

    /// Broadcast a permission decision to all subscribers.
    ///
    /// A decision with no subscribers is simply not observed; that is not an
    /// error, because the decision itself still stands.
    pub fn broadcast(&self, event: PermissionDecisionEvent) {
        let _ = self.tx.send(event);
    }

    /// Number of active subscribers.
    #[must_use]
    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }

    /// Number of requests currently awaiting an answer.
    pub async fn inflight_count(&self) -> usize {
        self.inflight.lock().await.len()
    }

    /// Resolve a permission request for `key`, prompting at most once across
    /// all concurrent askers.
    ///
    /// The first caller for a key runs `ask` and publishes the answer; callers
    /// that arrive while that prompt is outstanding await it instead of raising
    /// a second one. If the asking task dies without answering, waiters fall
    /// back to a denial rather than hanging.
    pub async fn resolve<F, Fut>(&self, key: &str, agent_id: &str, ask: F) -> PermissionResult
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = PermissionResult>,
    {
        let mut leader_tx = None;
        let waiting = {
            let mut inflight = self.inflight.lock().await;
            let existing = inflight.get(key).cloned();
            if existing.is_none() {
                let (tx, rx) = watch::channel(None);
                inflight.insert(key.to_owned(), rx);
                leader_tx = Some(tx);
            }
            drop(inflight);
            existing
        };

        if let Some(mut rx) = waiting {
            // Another agent is already asking; take whatever it is told.
            while rx.changed().await.is_ok() {
                let answer = rx.borrow().clone();
                if let Some(result) = answer {
                    return result;
                }
            }
            // The asking task vanished without answering; fail closed.
            tracing::warn!(
                key,
                agent_id,
                "permission request holder dropped before answering"
            );
            return PermissionResult::deny();
        }
        let leader_tx = leader_tx.expect("the first asker owns the request");

        let result = ask().await;

        // Publish before clearing so a waiter cloning the receiver in between
        // still observes the answer.
        let _ = leader_tx.send(Some(result.clone()));
        self.inflight.lock().await.remove(key);

        self.broadcast(PermissionDecisionEvent::new(
            key,
            if result.allowed {
                PermissionDecision::Allow
            } else {
                PermissionDecision::Deny(
                    result
                        .feedback
                        .clone()
                        .unwrap_or_else(|| "denied by user".to_string()),
                )
            },
            agent_id,
        ));

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn broadcast_and_receive() {
        let mgr = PermissionSyncManager::new(16);
        let mut rx = mgr.subscribe();

        mgr.broadcast(PermissionDecisionEvent::new(
            "Bash",
            PermissionDecision::Allow,
            "agent-1",
        ));

        let received = rx.recv().await.unwrap();
        assert_eq!(received.tool_name, "Bash");
        assert_eq!(received.decision, PermissionDecision::Allow);
        assert_eq!(received.agent_id, "agent-1");
    }

    #[tokio::test]
    async fn multiple_subscribers_all_see_the_event() {
        let mgr = PermissionSyncManager::new(16);
        let mut rx1 = mgr.subscribe();
        let mut rx2 = mgr.subscribe();

        mgr.broadcast(PermissionDecisionEvent::new(
            "Edit",
            PermissionDecision::Deny("not allowed".into()),
            "agent-2",
        ));

        assert_eq!(rx1.recv().await.unwrap().tool_name, "Edit");
        assert_eq!(rx2.recv().await.unwrap().agent_id, "agent-2");
    }

    #[test]
    fn subscriber_count_tracking() {
        let mgr = PermissionSyncManager::new(16);
        assert_eq!(mgr.subscriber_count(), 0);

        let rx1 = mgr.subscribe();
        assert_eq!(mgr.subscriber_count(), 1);

        let _rx2 = mgr.subscribe();
        assert_eq!(mgr.subscriber_count(), 2);

        drop(rx1);
        assert_eq!(mgr.subscriber_count(), 1);
    }

    #[tokio::test]
    async fn broadcast_without_subscribers_is_not_an_error() {
        let mgr = PermissionSyncManager::new(16);
        // Nothing to assert beyond "this does not panic or report failure":
        // a decision with no observers still stands.
        mgr.broadcast(PermissionDecisionEvent::new(
            "Bash",
            PermissionDecision::Allow,
            "agent-1",
        ));
    }

    #[tokio::test]
    async fn resolve_prompts_once_and_returns_the_answer() {
        let mgr = PermissionSyncManager::new(16);
        let asked = AtomicUsize::new(0);

        let result = mgr
            .resolve("Bash", "alice", || async {
                asked.fetch_add(1, Ordering::SeqCst);
                PermissionResult::allow()
            })
            .await;

        assert!(result.allowed);
        assert_eq!(asked.load(Ordering::SeqCst), 1);
        assert_eq!(mgr.inflight_count().await, 0);
    }

    #[tokio::test]
    async fn concurrent_askers_share_one_prompt() {
        let mgr = Arc::new(PermissionSyncManager::new(16));
        let asked = Arc::new(AtomicUsize::new(0));
        // Gate the prompt so the second asker is guaranteed to arrive while
        // the first is still outstanding.
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();

        let leader = {
            let mgr = Arc::clone(&mgr);
            let asked = Arc::clone(&asked);
            tokio::spawn(async move {
                mgr.resolve("Bash", "alice", || async {
                    asked.fetch_add(1, Ordering::SeqCst);
                    let _ = release_rx.await;
                    PermissionResult::allow()
                })
                .await
            })
        };

        // Wait until the leader is registered as in-flight.
        while mgr.inflight_count().await == 0 {
            tokio::task::yield_now().await;
        }

        let follower = {
            let mgr = Arc::clone(&mgr);
            let asked = Arc::clone(&asked);
            tokio::spawn(async move {
                mgr.resolve("Bash", "bob", || async {
                    asked.fetch_add(1, Ordering::SeqCst);
                    PermissionResult::deny()
                })
                .await
            })
        };

        let _ = release_tx.send(());
        let leader_result = leader.await.unwrap();
        let follower_result = follower.await.unwrap();

        assert!(leader_result.allowed);
        assert!(
            follower_result.allowed,
            "the follower must inherit the leader's answer, not ask again"
        );
        assert_eq!(
            asked.load(Ordering::SeqCst),
            1,
            "only one prompt should reach the user"
        );
    }

    #[tokio::test]
    async fn distinct_keys_prompt_separately() {
        let mgr = PermissionSyncManager::new(16);
        let asked = AtomicUsize::new(0);

        for key in ["Bash", "Edit"] {
            mgr.resolve(key, "alice", || async {
                asked.fetch_add(1, Ordering::SeqCst);
                PermissionResult::allow()
            })
            .await;
        }
        assert_eq!(asked.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_denial_is_broadcast_with_its_feedback() {
        let mgr = PermissionSyncManager::new(16);
        let mut rx = mgr.subscribe();

        let result = mgr
            .resolve("Bash", "alice", || async {
                PermissionResult::deny_with_feedback("use Read instead")
            })
            .await;
        assert!(!result.allowed);

        let event = rx.recv().await.unwrap();
        assert_eq!(event.tool_name, "Bash");
        assert_eq!(
            event.decision,
            PermissionDecision::Deny("use Read instead".into())
        );
    }

    #[tokio::test]
    async fn a_dropped_holder_denies_rather_than_hangs() {
        let mgr = Arc::new(PermissionSyncManager::new(16));

        // Register an in-flight entry whose sender is then dropped, standing in
        // for an asking task that died mid-prompt.
        {
            let (tx, rx) = watch::channel(None);
            mgr.inflight.lock().await.insert("Bash".into(), rx);
            drop(tx);
        }

        let result = mgr
            .resolve("Bash", "bob", || async { PermissionResult::allow() })
            .await;
        assert!(!result.allowed, "a lost prompt must fail closed");
    }

    #[test]
    fn permission_decision_event_construction() {
        let event = PermissionDecisionEvent::new(
            "Read",
            PermissionDecision::AskUser("confirm?".into()),
            "agent-3",
        );
        assert_eq!(event.tool_name, "Read");
        assert_eq!(
            event.decision,
            PermissionDecision::AskUser("confirm?".into())
        );
        assert_eq!(event.agent_id, "agent-3");
    }
}
