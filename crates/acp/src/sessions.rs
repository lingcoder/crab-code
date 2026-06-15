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

    /// Get the session's cancel token, registering the session on the
    /// fly if the client skipped `new_session` (e.g. `load_session`).
    pub async fn cancel_token(&self, id: &str) -> CancellationToken {
        self.inner
            .lock()
            .await
            .entry(id.to_string())
            .or_default()
            .clone()
    }

    /// Fire the session's cancel token if the session exists.
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
    async fn create_registers_session_with_live_token() {
        let map = SessionMap::default();
        let id = map.create().await;
        let token = map.cancel_token(&id).await;
        assert!(!token.is_cancelled());
    }

    #[tokio::test]
    async fn cancel_fires_previously_handed_out_token() {
        let map = SessionMap::default();
        let id = map.create().await;
        let token = map.cancel_token(&id).await;
        map.cancel(&id).await;
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn cancel_token_registers_unknown_session() {
        let map = SessionMap::default();
        let first = map.cancel_token("ad-hoc").await;
        let second = map.cancel_token("ad-hoc").await;
        first.cancel();
        assert!(second.is_cancelled());
    }

    #[tokio::test]
    async fn cancel_on_unknown_session_is_a_no_op() {
        let map = SessionMap::default();
        map.cancel("missing").await;
        let token = map.cancel_token("missing").await;
        assert!(!token.is_cancelled());
    }
}
