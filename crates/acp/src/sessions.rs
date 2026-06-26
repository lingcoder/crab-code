//! Session registry — per-session cancellation tokens keyed by id.

use std::collections::HashMap;

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Tracks live ACP sessions and their cancellation tokens.
#[derive(Default)]
pub struct SessionMap {
    inner: Mutex<HashMap<String, CancellationToken>>,
}

impl SessionMap {
    /// Register a new session under a fresh ULID and return its id.
    pub async fn create(&self) -> String {
        let id = crab_utils::id::new_ulid();
        self.inner
            .lock()
            .await
            .insert(id.clone(), CancellationToken::new());
        id
    }

    /// Begin a new prompt turn for `id`: install a fresh cancellation token
    /// (replacing any previously-fired one) and return it. Registers the
    /// session on the fly if the client skipped `new_session` (e.g. after
    /// `load_session`).
    ///
    /// A fresh token per turn is essential: a cancel from an earlier turn must
    /// never carry over and immediately abort later prompts on the same
    /// session, which would brick the session after a single cancel.
    pub async fn begin_turn(&self, id: &str) -> CancellationToken {
        let token = CancellationToken::new();
        self.inner
            .lock()
            .await
            .insert(id.to_string(), token.clone());
        token
    }

    /// Fire the session's current cancel token if the session exists.
    pub async fn cancel(&self, id: &str) {
        if let Some(token) = self.inner.lock().await.get(id) {
            token.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn begin_turn_hands_out_live_token() {
        let map = SessionMap::default();
        let id = map.create().await;
        let token = map.begin_turn(&id).await;
        assert!(!token.is_cancelled());
    }

    #[tokio::test]
    async fn cancel_fires_current_turn_token() {
        let map = SessionMap::default();
        let id = map.create().await;
        let token = map.begin_turn(&id).await;
        map.cancel(&id).await;
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn fresh_turn_token_survives_prior_cancel() {
        // A cancel from one turn must not brick the next turn on the same
        // session: begin_turn always installs a fresh, live token.
        let map = SessionMap::default();
        let id = map.create().await;
        let first = map.begin_turn(&id).await;
        map.cancel(&id).await;
        assert!(first.is_cancelled());
        let second = map.begin_turn(&id).await;
        assert!(!second.is_cancelled());
    }

    #[tokio::test]
    async fn begin_turn_registers_unknown_session() {
        let map = SessionMap::default();
        let token = map.begin_turn("ad-hoc").await;
        assert!(!token.is_cancelled());
        map.cancel("ad-hoc").await;
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn cancel_on_unknown_session_is_a_no_op() {
        let map = SessionMap::default();
        map.cancel("missing").await;
        let token = map.begin_turn("missing").await;
        assert!(!token.is_cancelled());
    }
}
