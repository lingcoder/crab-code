//! Message router for inter-agent communication.
//!
//! The `MessageRouter` maintains a registry of agent inboxes (mpsc channels)
//! and routes `Envelope` messages by agent name or ID. Broadcast messages
//! (`to = "*"`) are delivered to all registered agents except the sender.

use std::collections::HashMap;

use tokio::sync::mpsc;

use crate::teams::bus::Envelope;

/// Capacity for each agent's inbox channel.
const DEFAULT_INBOX_SIZE: usize = 128;

/// Routes envelopes between registered agents.
///
/// Each agent is registered with a unique name and gets a dedicated
/// mpsc inbox. The router delivers messages by matching the `to` field
/// of the envelope against registered agent names.
pub struct MessageRouter {
    /// Map of agent name → sender half of their inbox.
    inboxes: HashMap<String, mpsc::Sender<Envelope>>,
    /// Inbox capacity for newly registered agents.
    inbox_size: usize,
    /// Pending inboxes: receiver halves to hand out during registration.
    /// Once an agent registers and takes its receiver, the entry is removed.
    receivers: HashMap<String, mpsc::Receiver<Envelope>>,
}

impl MessageRouter {
    /// Create a new empty router with default inbox capacity.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inboxes: HashMap::new(),
            inbox_size: DEFAULT_INBOX_SIZE,
            receivers: HashMap::new(),
        }
    }

    /// Create a new router with a custom inbox buffer size.
    #[must_use]
    pub fn with_inbox_size(inbox_size: usize) -> Self {
        Self {
            inboxes: HashMap::new(),
            inbox_size,
            receivers: HashMap::new(),
        }
    }

    /// Register an agent by name and return its inbox receiver.
    ///
    /// If an agent with the same name is already registered, the old
    /// entry is replaced and the old receiver is dropped.
    pub fn register(&mut self, name: impl Into<String>) -> mpsc::Receiver<Envelope> {
        let name = name.into();
        let (tx, rx) = mpsc::channel(self.inbox_size);
        self.inboxes.insert(name, tx);
        rx
    }

    /// Unregister an agent by name. Returns `true` if the agent was found.
    pub fn unregister(&mut self, name: &str) -> bool {
        let removed = self.inboxes.remove(name).is_some();
        self.receivers.remove(name);
        removed
    }

    /// Check if an agent is registered.
    #[must_use]
    pub fn is_registered(&self, name: &str) -> bool {
        self.inboxes.contains_key(name)
    }

    /// Get the names of all registered agents.
    #[must_use]
    pub fn registered_agents(&self) -> Vec<String> {
        self.inboxes.keys().cloned().collect()
    }

    /// Number of registered agents.
    #[must_use]
    pub fn agent_count(&self) -> usize {
        self.inboxes.len()
    }

    /// Route an envelope to its destination.
    ///
    /// - If `to == "*"`: delivers to all agents except the sender.
    /// - Otherwise: delivers to the named agent.
    ///
    /// Returns the number of agents the message was delivered to.
    /// Returns 0 if the target agent is not registered (for directed messages).
    pub async fn route(&self, envelope: &Envelope) -> usize {
        if envelope.is_broadcast() {
            self.broadcast(envelope).await
        } else {
            usize::from(self.send_to(&envelope.to, envelope).await)
        }
    }

    /// Send an envelope to a specific agent. Returns `true` if delivered.
    async fn send_to(&self, name: &str, envelope: &Envelope) -> bool {
        if let Some(tx) = self.inboxes.get(name) {
            tx.send(envelope.clone()).await.is_ok()
        } else {
            false
        }
    }

    /// Broadcast an envelope to all agents except the sender.
    async fn broadcast(&self, envelope: &Envelope) -> usize {
        let mut delivered = 0;
        for (name, tx) in &self.inboxes {
            if *name != envelope.from && tx.send(envelope.clone()).await.is_ok() {
                delivered += 1;
            }
        }
        delivered
    }

    /// Get a sender handle for a specific agent's inbox.
    ///
    /// Useful when an agent needs to send directly to another agent's
    /// inbox without going through the router.
    #[must_use]
    pub fn get_sender(&self, name: &str) -> Option<mpsc::Sender<Envelope>> {
        self.inboxes.get(name).cloned()
    }
}

impl Default for MessageRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teams::bus::{AgentMessage, AgentStatus};

    #[test]
    fn router_new_is_empty() {
        let router = MessageRouter::new();
        assert_eq!(router.agent_count(), 0);
        assert!(router.registered_agents().is_empty());
    }

    #[test]
    fn router_default() {
        let router = MessageRouter::default();
        assert_eq!(router.agent_count(), 0);
    }

    #[test]
    fn router_with_inbox_size() {
        let router = MessageRouter::with_inbox_size(64);
        assert_eq!(router.inbox_size, 64);
    }

    #[test]
    fn register_agent() {
        let mut router = MessageRouter::new();
        let _rx = router.register("alice");
        assert!(router.is_registered("alice"));
        assert_eq!(router.agent_count(), 1);
    }

    #[test]
    fn register_multiple_agents() {
        let mut router = MessageRouter::new();
        let _rx1 = router.register("alice");
        let _rx2 = router.register("bob");
        let _rx3 = router.register("charlie");
        assert_eq!(router.agent_count(), 3);
        assert!(router.is_registered("alice"));
        assert!(router.is_registered("bob"));
        assert!(router.is_registered("charlie"));
    }

    #[test]
    fn unregister_agent() {
        let mut router = MessageRouter::new();
        let _rx = router.register("alice");
        assert!(router.unregister("alice"));
        assert!(!router.is_registered("alice"));
        assert_eq!(router.agent_count(), 0);
    }

    #[test]
    fn unregister_nonexistent() {
        let mut router = MessageRouter::new();
        assert!(!router.unregister("nobody"));
    }

    #[test]
    fn registered_agents_list() {
        let mut router = MessageRouter::new();
        let _rx1 = router.register("alice");
        let _rx2 = router.register("bob");
        let mut names = router.registered_agents();
        names.sort();
        assert_eq!(names, vec!["alice", "bob"]);
    }

    #[test]
    fn get_sender() {
        let mut router = MessageRouter::new();
        let _rx = router.register("alice");
        assert!(router.get_sender("alice").is_some());
        assert!(router.get_sender("nobody").is_none());
    }

    #[tokio::test]
    async fn route_directed_message() {
        let mut router = MessageRouter::new();
        let mut rx = router.register("bob");

        let env = Envelope::new("alice", "bob", AgentMessage::Shutdown);
        let delivered = router.route(&env).await;
        assert_eq!(delivered, 1);

        let received = rx.recv().await.unwrap();
        assert_eq!(received.from, "alice");
        assert_eq!(received.to, "bob");
        assert!(matches!(received.payload, AgentMessage::Shutdown));
    }

    #[tokio::test]
    async fn route_to_nonexistent_agent() {
        let router = MessageRouter::new();
        let env = Envelope::new("alice", "nobody", AgentMessage::Shutdown);
        let delivered = router.route(&env).await;
        assert_eq!(delivered, 0);
    }

    #[tokio::test]
    async fn route_broadcast() {
        let mut router = MessageRouter::new();
        let mut rx_alice = router.register("alice");
        let mut rx_bob = router.register("bob");
        let mut rx_charlie = router.register("charlie");

        // Broadcast from alice — should go to bob and charlie, not alice
        let env = Envelope::broadcast(
            "alice",
            AgentMessage::StatusUpdate {
                status: AgentStatus::Working,
                detail: None,
            },
        );

        let delivered = router.route(&env).await;
        assert_eq!(delivered, 2);

        // bob and charlie should receive it
        let msg_bob = rx_bob.recv().await.unwrap();
        assert_eq!(msg_bob.from, "alice");
        let msg_charlie = rx_charlie.recv().await.unwrap();
        assert_eq!(msg_charlie.from, "alice");

        // alice should NOT receive it (try_recv should fail)
        assert!(rx_alice.try_recv().is_err());
    }

    #[tokio::test]
    async fn route_broadcast_empty_router() {
        let router = MessageRouter::new();
        let env = Envelope::broadcast("alice", AgentMessage::Shutdown);
        let delivered = router.route(&env).await;
        assert_eq!(delivered, 0);
    }

    #[tokio::test]
    async fn route_multiple_messages() {
        let mut router = MessageRouter::new();
        let mut rx = router.register("bob");

        for i in 0..5 {
            let env = Envelope::new(
                "alice",
                "bob",
                AgentMessage::AssignTask {
                    task_id: format!("t{i}"),
                    prompt: format!("task {i}"),
                },
            );
            router.route(&env).await;
        }

        for i in 0..5 {
            let msg = rx.recv().await.unwrap();
            if let AgentMessage::AssignTask { task_id, .. } = &msg.payload {
                assert_eq!(task_id, &format!("t{i}"));
            } else {
                panic!("expected AssignTask");
            }
        }
    }

    #[tokio::test]
    async fn re_register_replaces_inbox() {
        let mut router = MessageRouter::new();
        let _rx_old = router.register("alice");
        let mut rx_new = router.register("alice");

        // Message should go to the new inbox
        let env = Envelope::new("bob", "alice", AgentMessage::Shutdown);
        let delivered = router.route(&env).await;
        assert_eq!(delivered, 1);

        let msg = rx_new.recv().await.unwrap();
        assert!(matches!(msg.payload, AgentMessage::Shutdown));
    }

    #[tokio::test]
    async fn route_after_unregister() {
        let mut router = MessageRouter::new();
        let _rx = router.register("alice");
        router.unregister("alice");

        let env = Envelope::new("bob", "alice", AgentMessage::Shutdown);
        let delivered = router.route(&env).await;
        assert_eq!(delivered, 0);
    }

    #[tokio::test]
    async fn get_sender_and_send_directly() {
        let mut router = MessageRouter::new();
        let mut rx = router.register("bob");

        let tx = router.get_sender("bob").unwrap();
        let env = Envelope::new("alice", "bob", AgentMessage::ShutdownAck);
        tx.send(env).await.unwrap();

        let msg = rx.recv().await.unwrap();
        assert!(matches!(msg.payload, AgentMessage::ShutdownAck));
    }
}
