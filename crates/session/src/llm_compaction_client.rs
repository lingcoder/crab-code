//! Always-available [`CompactionClient`] fallback.
//!
//! The real LLM-backed implementation lives in `crab-api` (which can depend
//! on the LLM transport types). [`NullCompactionClient`] is a no-op that
//! returns an empty string, used when no backend is wired in or when callers
//! want to disable LLM-driven compaction without changing the call sites.

use std::future::Future;
use std::pin::Pin;

use crab_core::message::Message;

use crate::compaction::CompactionClient;

/// A [`CompactionClient`] that produces no summary.
///
/// Returns `Ok(String::new())` for every call, letting the heuristic
/// fallbacks in `compaction.rs` take over.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullCompactionClient;

impl NullCompactionClient {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl CompactionClient for NullCompactionClient {
    fn summarize(
        &self,
        _messages: &[Message],
        _instruction: &str,
    ) -> Pin<Box<dyn Future<Output = crab_core::Result<String>> + Send + '_>> {
        Box::pin(async { Ok(String::new()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn null_client_returns_empty_string() {
        let client = NullCompactionClient::new();
        let out = client
            .summarize(&[Message::user("hi")], "summarize")
            .await
            .unwrap();
        assert!(out.is_empty());
    }
}
