//! Cross-agent permission decision synchronization.
//!
//! When one teammate receives a permission decision (allow/deny) from the user,
//! [`PermissionSyncManager`] broadcasts it to all other teammates so they can
//! apply the same decision without re-prompting.

use std::time::Instant;

use crab_core::permission::PermissionDecision;
use tokio::sync::broadcast;

/// A permission decision event broadcast across teammates.
#[derive(Debug, Clone)]
#[allow(dead_code)]
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

#[allow(dead_code)]
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

/// Broadcasts permission decisions across all teammates in a swarm.
///
/// Backed by a [`tokio::sync::broadcast`] channel so that every subscriber
/// receives every event. Subscribers that fall behind will skip older events
/// (lagged messages are silently dropped by the broadcast channel).
#[allow(dead_code)]
pub struct PermissionSyncManager {
    tx: broadcast::Sender<PermissionDecisionEvent>,
}

#[allow(dead_code)]
impl PermissionSyncManager {
    /// Create a new manager with the given channel capacity.
    ///
    /// `capacity` determines how many un-consumed events the broadcast
    /// channel can buffer before older entries are dropped for slow
    /// subscribers.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Subscribe to permission decision events.
    ///
    /// Returns a receiver that will get all future events. Existing events
    /// before the subscription are not replayed.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<PermissionDecisionEvent> {
        self.tx.subscribe()
    }

    /// Broadcast a permission decision to all subscribers.
    ///
    /// # Errors
    ///
    /// Returns an error if there are no active subscribers (the event is
    /// still lost in that case).
    pub fn broadcast(&self, event: PermissionDecisionEvent) -> crab_common::Result<()> {
        self.tx
            .send(event)
            .map_err(|e| crab_common::Error::Other(format!("permission broadcast failed: {e}")))?;
        Ok(())
    }

    /// Number of active subscribers.
    #[must_use]
    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn broadcast_and_receive() {
        let mgr = PermissionSyncManager::new(16);
        let mut rx = mgr.subscribe();

        let event = PermissionDecisionEvent::new("Bash", PermissionDecision::Allow, "agent-1");
        mgr.broadcast(event).unwrap();

        let received = rx.recv().await.unwrap();
        assert_eq!(received.tool_name, "Bash");
        assert_eq!(received.decision, PermissionDecision::Allow);
        assert_eq!(received.agent_id, "agent-1");
    }

    #[tokio::test]
    async fn multiple_subscribers() {
        let mgr = PermissionSyncManager::new(16);
        let mut rx1 = mgr.subscribe();
        let mut rx2 = mgr.subscribe();

        let event = PermissionDecisionEvent::new(
            "Edit",
            PermissionDecision::Deny("not allowed".into()),
            "agent-2",
        );
        mgr.broadcast(event).unwrap();

        let e1 = rx1.recv().await.unwrap();
        let e2 = rx2.recv().await.unwrap();
        assert_eq!(e1.tool_name, "Edit");
        assert_eq!(e2.tool_name, "Edit");
        assert_eq!(e1.agent_id, "agent-2");
        assert_eq!(e2.agent_id, "agent-2");
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

    #[test]
    fn broadcast_with_no_subscribers_fails() {
        let mgr = PermissionSyncManager::new(16);
        let event = PermissionDecisionEvent::new("Bash", PermissionDecision::Allow, "agent-1");
        assert!(mgr.broadcast(event).is_err());
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
