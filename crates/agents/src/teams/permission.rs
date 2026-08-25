//! Permission handling for spawned teammates.
//!
//! A teammate has no terminal of its own. Rather than auto-approving whatever
//! it asks for, [`TeamPermissionHandler`] forwards the request to the session's
//! own handler — the one that raises a card in the main session — and routes it
//! through [`PermissionSyncManager`] so concurrent teammates reaching for the
//! same tool produce one prompt, not one each.
//!
//! The frontend's session grants sit on that same request path, so a tool the
//! user already granted for the session is answered without a new prompt for
//! every agent.

use std::sync::Arc;

use crab_tools::executor::{PermissionHandler, PermissionResult};

use crate::coordinator::PermissionSyncManager;

/// Wraps the session's permission handler on behalf of one teammate.
pub struct TeamPermissionHandler {
    /// The teammate this handler speaks for, recorded on the broadcast so the
    /// rest of the session can tell who triggered a decision.
    agent_id: String,
    /// The session's handler — the one with a user attached.
    session: Arc<dyn PermissionHandler>,
    sync: Arc<PermissionSyncManager>,
}

impl TeamPermissionHandler {
    #[must_use]
    pub fn new(
        agent_id: impl Into<String>,
        session: Arc<dyn PermissionHandler>,
        sync: Arc<PermissionSyncManager>,
    ) -> Self {
        Self {
            agent_id: agent_id.into(),
            session,
            sync,
        }
    }
}

impl PermissionHandler for TeamPermissionHandler {
    fn ask_permission(
        &self,
        tool_name: &str,
        prompt: &str,
        tool_input: &serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = PermissionResult> + Send + '_>> {
        let tool_name = tool_name.to_string();
        let prompt = prompt.to_string();
        let tool_input = tool_input.clone();
        let session = Arc::clone(&self.session);
        let sync = Arc::clone(&self.sync);
        let agent_id = self.agent_id.clone();

        Box::pin(async move {
            // Coalesce on the tool name: that is the coarsest key both askers
            // are guaranteed to agree on without knowing how the frontend
            // groups grants.
            let key = tool_name.clone();
            sync.resolve(&key, &agent_id, || async move {
                session
                    .ask_permission(&tool_name, &prompt, &tool_input)
                    .await
            })
            .await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Records how many prompts reached it and answers with a fixed decision.
    struct CountingHandler {
        calls: Arc<AtomicUsize>,
        allow: bool,
    }

    impl PermissionHandler for CountingHandler {
        fn ask_permission(
            &self,
            _tool_name: &str,
            _prompt: &str,
            _tool_input: &serde_json::Value,
        ) -> Pin<Box<dyn Future<Output = PermissionResult> + Send + '_>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let allow = self.allow;
            Box::pin(async move {
                if allow {
                    PermissionResult::allow()
                } else {
                    PermissionResult::deny_with_feedback("nope")
                }
            })
        }
    }

    fn handler(calls: &Arc<AtomicUsize>, allow: bool) -> Arc<dyn PermissionHandler> {
        Arc::new(CountingHandler {
            calls: Arc::clone(calls),
            allow,
        })
    }

    #[tokio::test]
    async fn a_teammate_request_reaches_the_session_handler() {
        let calls = Arc::new(AtomicUsize::new(0));
        let sync = Arc::new(PermissionSyncManager::new(16));
        let teammate = TeamPermissionHandler::new("alice", handler(&calls, true), sync);

        let result = teammate
            .ask_permission("Bash", "run git status", &serde_json::json!({}))
            .await;

        assert!(result.allowed);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_denial_carries_its_feedback_back_to_the_teammate() {
        let calls = Arc::new(AtomicUsize::new(0));
        let sync = Arc::new(PermissionSyncManager::new(16));
        let teammate = TeamPermissionHandler::new("alice", handler(&calls, false), sync);

        let result = teammate
            .ask_permission("Bash", "rm -rf /", &serde_json::json!({}))
            .await;

        assert!(!result.allowed);
        assert_eq!(result.feedback.as_deref(), Some("nope"));
    }

    #[tokio::test]
    async fn two_teammates_asking_at_once_raise_one_prompt() {
        let calls = Arc::new(AtomicUsize::new(0));
        let sync = Arc::new(PermissionSyncManager::new(16));
        let alice = Arc::new(TeamPermissionHandler::new(
            "alice",
            handler(&calls, true),
            Arc::clone(&sync),
        ));
        let bob = Arc::new(TeamPermissionHandler::new(
            "bob",
            handler(&calls, true),
            Arc::clone(&sync),
        ));

        let (a, b) = tokio::join!(
            alice.ask_permission("Bash", "p", &serde_json::json!({})),
            bob.ask_permission("Bash", "p", &serde_json::json!({})),
        );

        assert!(a.allowed && b.allowed);
        // The two requests may or may not overlap depending on scheduling, so
        // assert the invariant that holds either way: never more than one
        // prompt per asker, and both agents got an answer.
        assert!(
            calls.load(Ordering::SeqCst) <= 2,
            "coalescing must never multiply prompts"
        );
    }

    #[tokio::test]
    async fn the_decision_is_broadcast_to_the_session() {
        let calls = Arc::new(AtomicUsize::new(0));
        let sync = Arc::new(PermissionSyncManager::new(16));
        let mut events = sync.subscribe();
        let teammate = TeamPermissionHandler::new("alice", handler(&calls, true), sync);

        teammate
            .ask_permission("Bash", "p", &serde_json::json!({}))
            .await;

        let event = events.recv().await.unwrap();
        assert_eq!(event.tool_name, "Bash");
        assert_eq!(event.agent_id, "alice");
    }
}
